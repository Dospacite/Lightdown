//! Selection-first editing. All ranges use the editor's native byte selections.
use super::*;

#[derive(Default)]
pub(super) struct Helix {
    pub enabled: bool,
    pub select: bool,
    pending: String,
    count: String,
    regex: Option<(bool, String)>,
}

impl Pane {
    pub(super) fn hx_cursor(&self) -> usize {
        self.hx_head(self.buf.anchor, self.buf.cur)
    }

    fn hx_ranges(&self) -> Vec<(usize, usize)> {
        self.extra_cursors.iter().copied().chain([(self.buf.anchor, self.buf.cur)]).collect()
    }

    fn hx_set(&mut self, mut ranges: Vec<(usize, usize)>) {
        if let Some((a, b)) = ranges.pop() {
            (self.buf.anchor, self.buf.cur) = (a, b);
            self.extra_cursors = ranges;
        }
        self.moved();
    }

    fn hx_character(&mut self) {
        if self.buf.anchor == self.buf.cur {
            self.buf.anchor = self.buf.cur;
            self.buf.cur = editor::next(&self.buf.text, self.buf.cur);
        }
    }

    fn hx_head(&self, a: usize, b: usize) -> usize {
        if b > a { editor::prev(&self.buf.text, b) } else { b }
    }

    fn hx_motion(&mut self, motion: Motion, n: usize) {
        let mut out = Vec::new();
        for (a, b) in self.hx_ranges() {
            let head = self.hx_head(a, b);
            let from = if matches!(motion, Motion::WordNext(_) | Motion::WordEnd(_))
                && !self.vim.helix.select
                && self.buf.text[a.min(b)..a.max(b)].chars().count() > 1
            {
                b
            } else {
                head
            };
            if let Some((to, _)) = self.vim_motion(motion, Some(n), from) {
                let to = if motion == Motion::LineEnd && to > line_start(&self.buf.text, to) {
                    editor::prev(&self.buf.text, to)
                } else {
                    to.min(self.buf.text.len())
                };
                let end = editor::next(&self.buf.text, to);
                let range = if self.vim.helix.select {
                    if to >= a { (a, end) } else { (a.max(editor::next(&self.buf.text, head)), to) }
                } else if matches!(motion, Motion::Find { .. }) {
                    if to >= head { (head, end) } else { (editor::next(&self.buf.text, head), to) }
                } else if matches!(motion, Motion::WordNext(_)) {
                    (from, to)
                } else if matches!(motion, Motion::WordEnd(_)) {
                    (from, end)
                } else if matches!(motion, Motion::WordBack(_)) {
                    (b.max(editor::next(&self.buf.text, head)), to)
                } else {
                    (to, end)
                };
                out.push(range);
            } else {
                out.push((a, b));
            }
        }
        self.hx_set(out);
    }

    /// Rewrite each selection in one undo transaction, retaining all cursors.
    fn hx_rewrite(&mut self, mut edits: Vec<(usize, usize, String)>, select: bool) {
        if self.read_only {
            return;
        }
        if self.doc == Doc::Cells && edits.iter().any(|(a, b, t)| !self.keeps_cells(*a, *b, t)) {
            self.note = Some("Selection crosses a protected cell header".into());
            return;
        }
        edits.sort_by_key(|e| (e.0, e.1));
        edits.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        if edits.windows(2).any(|w| w[0].1 > w[1].0) {
            self.note = Some("Selections overlap".into());
            return;
        }
        let checkpoint = self.buf.checkpoint();
        self.vim_note_change();
        let mut ranges = Vec::new();
        let mut shift: isize = 0;
        for (a, b, text) in &edits {
            let start = (*a as isize + shift) as usize;
            let end = start + text.len();
            ranges.push(if select { (start, end) } else { (end, end) });
            shift += text.len() as isize - (b - a) as isize;
        }
        self.extra_cursors.clear();
        let mut changed = false;
        for (a, b, text) in edits.into_iter().rev() {
            changed |= self.edit(a, b, &text, EditKind::Other);
        }
        self.buf.finish(checkpoint);
        if changed {
            self.hx_set(ranges);
        }
    }

    fn hx_operator(&mut self, c: char) {
        self.hx_character();
        let ranges = self.hx_ranges();
        if matches!(c, 'd' | 'c' | 'y') {
            let text = ranges.iter().map(|&(a, b)| &self.buf.text[a.min(b)..a.max(b)]).collect::<Vec<_>>().join("\n");
            self.vim_store(None, text, false, c == 'y');
            if c == 'y' {
                return;
            }
        }
        if matches!(c, '>' | '<') {
            let checkpoint = self.buf.checkpoint();
            let mut lines = std::collections::BTreeSet::new();
            for (a, b) in ranges {
                for line in self.buf.line_of(a.min(b))..=self.buf.line_of(editor::prev(&self.buf.text, a.max(b))) {
                    lines.insert(line);
                }
            }
            for line in lines.into_iter().rev() {
                self.vim_shift(line, line, c == '<', 1);
            }
            self.buf.finish(checkpoint);
            self.hx_character();
            return;
        }
        let edits = ranges
            .into_iter()
            .map(|(a, b)| {
                let (a, b) = (a.min(b), a.max(b));
                let text = match c {
                    '~' => recase(&self.buf.text[a..b], Op::Toggle),
                    'U' => recase(&self.buf.text[a..b], Op::Upper),
                    _ => String::new(),
                };
                (a, b, text)
            })
            .collect();
        let checkpoint = (c == 'c').then(|| self.buf.checkpoint());
        self.hx_rewrite(edits, matches!(c, '~' | 'U'));
        if c == 'c' {
            self.vim_begin_insert(Mode::Insert, 1);
            self.vim.group = checkpoint;
        } else {
            self.hx_character();
        }
        self.vim.helix.select = false;
    }

    fn hx_regex(&mut self, split: bool, pattern: &str) {
        let re = match regex::Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => {
                self.note = Some(e.to_string());
                return;
            }
        };
        let mut out = Vec::new();
        for (a, b) in self.hx_ranges() {
            let (a, b) = (a.min(b), a.max(b));
            let mut from = a;
            for m in re.find_iter(&self.buf.text[a..b]) {
                if split {
                    if from < a + m.start() {
                        out.push((from, a + m.start()));
                    }
                    from = a + m.end();
                } else if !m.is_empty() {
                    out.push((a + m.start(), a + m.end()));
                }
            }
            if split && from < b {
                out.push((from, b));
            }
        }
        if out.is_empty() {
            self.note = Some("No selections matched".into());
        } else {
            self.hx_set(out);
        }
    }

    fn hx_pending(&mut self, c: char, n: usize) {
        let prefix = std::mem::take(&mut self.vim.helix.pending);
        match prefix.as_str() {
            "f" | "t" | "F" | "T" => self.hx_motion(
                Motion::Find { ch: c, forward: prefix == "f" || prefix == "t", till: prefix == "t" || prefix == "T" },
                n,
            ),
            "r" => {
                let edits = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        let (a, b) = (a.min(b), a.max(b));
                        (a, b, c.to_string().repeat(self.buf.text[a..b].chars().count()))
                    })
                    .collect();
                self.hx_rewrite(edits, true);
            }
            "g" => match c {
                'g' => self.hx_motion(Motion::FirstLine, n),
                'e' => {
                    let line = self.buf.lines();
                    self.hx_motion(Motion::LastLine, line);
                }
                'h' => self.hx_motion(Motion::LineStart, 1),
                'l' => self.hx_motion(Motion::LineEnd, 1),
                's' => self.hx_motion(Motion::FirstNonBlank, 1),
                'd' => self.vim.request = Some(Request::Command("lsp_definition")),
                'r' => self.vim.request = Some(Request::Command("lsp_references")),
                'i' => self.vim.request = Some(Request::Command("lsp_implementation")),
                'y' => self.vim.request = Some(Request::Command("lsp_type_definition")),
                'n' => self.vim.request = Some(Request::Command("next_tab")),
                'p' => self.vim.request = Some(Request::Command("previous_tab")),
                '.' => {
                    self.vim_extended("g;", Some(n));
                    self.hx_character();
                }
                _ => (),
            },
            " " => {
                let id = match c {
                    'f' => "quick_open",
                    'b' => "helix_buffers",
                    's' => "symbols",
                    '/' => "search",
                    'k' => "lsp_hover",
                    'r' => "lsp_rename",
                    'a' => "lsp_actions",
                    'y' => "copy",
                    'p' => "paste",
                    _ => return,
                };
                if c == 'y' {
                    self.copy = Some(
                        self.hx_ranges()
                            .iter()
                            .map(|&(a, b)| &self.buf.text[a.min(b)..a.max(b)])
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                } else {
                    self.vim.request = Some(Request::Command(id));
                }
            }
            "m" => match c {
                'm' => self.hx_motion(Motion::Bracket, 1),
                'i' | 'a' | 's' | 'r' | 'd' => self.vim.helix.pending = format!("m{c}"),
                _ => (),
            },
            "mi" | "ma" => {
                let mut out = Vec::new();
                for (a, b) in self.hx_ranges() {
                    if let Some((s, e, _)) = object(&self.buf.text, self.hx_head(a, b), prefix == "ma", c, n) {
                        out.push((s, e));
                    }
                }
                self.hx_set(out);
            }
            "ms" => {
                let (open, close) = hx_pair(c);
                let edits = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        let (a, b) = (a.min(b), a.max(b));
                        (a, b, format!("{open}{}{close}", &self.buf.text[a..b]))
                    })
                    .collect();
                self.hx_rewrite(edits, true);
            }
            "mr" => self.vim.helix.pending = format!("mr{c}"),
            "md" => self.hx_surround(c, None),
            _ if prefix.starts_with("mr") => self.hx_surround(prefix.chars().nth(2).unwrap(), Some(c)),
            _ => (),
        }
    }

    fn hx_surround(&mut self, old: char, new: Option<char>) {
        let mut edits = Vec::new();
        for (a, b) in self.hx_ranges() {
            if let Some((s, e, _)) = object(&self.buf.text, self.hx_head(a, b), true, old, 1) {
                if s == e {
                    continue;
                }
                let inner = &self.buf.text[editor::next(&self.buf.text, s)..editor::prev(&self.buf.text, e)];
                let text = if let Some(c) = new {
                    let (o, c) = hx_pair(c);
                    format!("{o}{inner}{c}")
                } else {
                    inner.into()
                };
                edits.push((s, e, text));
            }
        }
        self.hx_rewrite(edits, true);
    }

    pub(super) fn helix_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if self.vim.mode == Mode::Insert {
            if key == Key::Escape {
                let ranges = self.hx_ranges();
                self.vim_leave_insert();
                let ranges = ranges
                    .into_iter()
                    .map(|(_, b)| {
                        let h = if b > line_start(&self.buf.text, b) { editor::prev(&self.buf.text, b) } else { b };
                        (h, editor::next(&self.buf.text, h))
                    })
                    .collect();
                self.hx_set(ranges);
                return true;
            }
            return self.vim_key_inner(key, mods, text);
        }
        if let Some((split, mut pattern)) = self.vim.helix.regex.take() {
            match key {
                Key::Escape => return true,
                Key::Enter => {
                    self.hx_regex(split, &pattern);
                    return true;
                }
                Key::Backspace => {
                    pattern.pop();
                }
                _ => {
                    if let Some(t) = text.filter(|_| !mods.ctrl && !mods.alt) {
                        pattern.push_str(t);
                    }
                }
            }
            self.note = Some(format!("{} /{pattern}", if split { "Split" } else { "Select" }));
            self.vim.helix.regex = Some((split, pattern));
            self.redraw = true;
            return true;
        }
        if self.vim.command.is_some() {
            return self.vim_key_inner(key, mods, text);
        }
        if key == Key::Escape {
            self.vim.helix.pending.clear();
            self.vim.helix.count.clear();
            self.vim.helix.select = false;
            self.extra_cursors.clear();
            let head = self.hx_head(self.buf.anchor, self.buf.cur);
            self.select(head, head);
            self.hx_character();
            return true;
        }
        let Some(c) = Self::vim_char(key, mods, text, true) else {
            return false;
        };
        if mods.ctrl {
            return self.vim_key_inner(key, mods, text);
        }
        if mods.alt {
            match c {
                ';' => {
                    let out = self.hx_ranges().into_iter().map(|(a, b)| (b, a)).collect();
                    self.hx_set(out);
                }
                ',' => {
                    if let Some((a, b)) = self.extra_cursors.pop() {
                        self.select(a, b);
                    }
                }
                's' => self.hx_regex(true, "\\n"),
                _ => return false,
            }
            return true;
        }
        if self.vim.helix.pending.is_empty() && c.is_ascii_digit() && (c != '0' || !self.vim.helix.count.is_empty()) {
            if self.vim.helix.count.len() < 5 {
                self.vim.helix.count.push(c);
            }
            return true;
        }
        let n = self.vim.helix.count.parse::<usize>().unwrap_or(1).clamp(1, 9999);
        self.vim.helix.count.clear();
        if !self.vim.helix.pending.is_empty() {
            self.hx_pending(c, n);
            self.redraw = true;
            return true;
        }
        self.hx_character();
        match c {
            'f' | 't' | 'F' | 'T' | 'r' | 'g' | 'm' | ' ' => {
                self.vim.helix.pending = c.to_string();
                self.vim.helix.count = n.to_string();
            }
            'w' | 'W' | 'b' | 'B' | 'e' | 'E' | 'h' | 'j' | 'k' | 'l' | '0' | '$' => {
                if let Part::Yes(m) = motion_of(&c.to_string()) {
                    self.hx_motion(m, n);
                }
            }
            'v' => self.vim.helix.select = !self.vim.helix.select,
            '%' => self.hx_set(vec![(0, self.buf.text.len())]),
            ';' => {
                let out = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        let h = self.hx_head(a, b);
                        (h, editor::next(&self.buf.text, h))
                    })
                    .collect();
                self.hx_set(out);
            }
            ',' => {
                self.extra_cursors.clear();
                self.moved();
            }
            'x' | 'X' => {
                let out = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        let (a, b) = (a.min(b), a.max(b));
                        let first = self.buf.line_of(a);
                        let last = self.buf.line_of(editor::prev(&self.buf.text, b));
                        let already =
                            a == self.buf.starts[first] && b == (self.buf.line(last).1 + 1).min(self.buf.text.len());
                        let (first, last) = if c == 'X' {
                            (first.saturating_sub(n), last)
                        } else {
                            (first, (last + n - usize::from(!already)).min(self.buf.lines() - 1))
                        };
                        (self.buf.starts[first], (self.buf.line(last).1 + 1).min(self.buf.text.len()))
                    })
                    .collect();
                self.hx_set(out);
            }
            'C' => {
                let mut out = self.hx_ranges();
                for (a, b) in self.hx_ranges() {
                    let line = self.buf.line_of(a.min(b));
                    if line + n < self.buf.lines() {
                        let start = at_column(&self.buf.text, self.buf.starts[line + n], column(&self.buf.text, a));
                        let end = at_column(&self.buf.text, self.buf.starts[line + n], column(&self.buf.text, b));
                        out.push((start, end));
                    }
                }
                self.hx_set(out);
            }
            's' | 'S' => {
                self.vim.helix.regex = Some((c == 'S', String::new()));
                self.note = Some("Type a selection pattern, then Enter".into());
            }
            '_' => {
                let out = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        let (a, b) = (a.min(b), a.max(b));
                        let t = &self.buf.text[a..b];
                        let s = a + t.len() - t.trim_start().len();
                        (s, s + t.trim().len())
                    })
                    .collect();
                self.hx_set(out);
            }
            '&' => {
                let ranges = self.hx_ranges();
                let col = ranges.iter().map(|&(a, b)| column(&self.buf.text, a.min(b))).max().unwrap_or(0);
                let edits = ranges
                    .into_iter()
                    .map(|(a, b)| {
                        let a = a.min(b);
                        (a, a, " ".repeat(col - column(&self.buf.text, a)))
                    })
                    .collect();
                self.hx_rewrite(edits, false);
            }
            'd' | 'c' | 'y' | '~' | '>' | '<' => self.hx_operator(c),
            'p' | 'P' => {
                if let Some((value, _)) = self.vim.registers.get(&'"').cloned() {
                    let edits = self
                        .hx_ranges()
                        .into_iter()
                        .map(|(a, b)| {
                            let at = if c == 'p' { a.max(b) } else { a.min(b) };
                            (at, at, value.repeat(n))
                        })
                        .collect();
                    self.hx_rewrite(edits, true);
                }
            }
            'U' => {
                if let Some(change) = self.buf.redo() {
                    self.changed(change);
                }
                self.hx_character();
            }
            'u' => {
                if let Some(change) = self.buf.undo() {
                    self.changed(change);
                }
                self.hx_character();
            }
            'i' | 'a' | 'I' | 'A' => {
                let out = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        let at = match c {
                            'i' => a.min(b),
                            'a' => a.max(b),
                            'I' => first_non_blank(&self.buf.text, a.min(b)),
                            _ => line_end(&self.buf.text, a.max(b)),
                        };
                        (at, at)
                    })
                    .collect();
                let checkpoint = self.buf.checkpoint();
                self.hx_set(out);
                self.vim_note_change();
                self.vim_begin_insert(Mode::Insert, 1);
                self.vim.group = Some(checkpoint);
            }
            'o' | 'O' => {
                let checkpoint = self.buf.checkpoint();
                let out = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        let at = if c == 'o' {
                            line_end(&self.buf.text, a.max(b))
                        } else {
                            line_start(&self.buf.text, a.min(b))
                        };
                        (at, at)
                    })
                    .collect();
                self.hx_set(out);
                let edits = self.hx_ranges().into_iter().map(|(a, _)| (a, a, "\n".repeat(n))).collect();
                self.hx_rewrite(edits, false);
                if c == 'O' {
                    let out = self.hx_ranges().into_iter().map(|(a, _)| (a - n, a - n)).collect();
                    self.hx_set(out);
                }
                self.vim_begin_insert(Mode::Insert, 1);
                self.vim.group = Some(checkpoint);
            }
            'J' => {
                let checkpoint = self.buf.checkpoint();
                let mut positions: Vec<_> = self
                    .hx_ranges()
                    .into_iter()
                    .map(|(a, b)| {
                        (
                            a.min(b),
                            self.buf
                                .line_of(editor::prev(&self.buf.text, a.max(b)))
                                .saturating_sub(self.buf.line_of(a.min(b)))
                                .max(n),
                        )
                    })
                    .collect();
                positions.sort_unstable();
                positions.dedup();
                for (at, n) in positions.into_iter().rev() {
                    self.vim_join(at, n, false);
                }
                self.buf.finish(checkpoint);
                self.hx_character();
            }
            ':' => self.vim.command = Some((String::new(), None)),
            '/' => self.vim.request = Some(Request::Search(false)),
            'n' | 'N' => self.vim.request = Some(Request::Next(c == 'N')),
            _ => (),
        }
        self.redraw = true;
        true
    }
}

fn hx_pair(c: char) -> (char, char) {
    match c {
        '(' | ')' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        c => (c, c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pane(text: &str) -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text(text.into(), Doc::Code);
        p.set_modal_editing("helix");
        p
    }
    fn keys(p: &mut Pane, keys: &str) {
        for c in keys.chars() {
            let key = match c {
                '\u{1b}' => Key::Escape,
                '\n' => Key::Enter,
                _ => Key::Char(c.to_ascii_lowercase()),
            };
            let text = c.to_string();
            p.key(key, Mods::default(), if c.is_control() { None } else { Some(&text) });
        }
    }
    fn run(text: &str, k: &str) -> Pane {
        let mut p = pane(text);
        keys(&mut p, k);
        p
    }
    fn selected(p: &Pane) -> &str {
        let (a, b) = p.buf.sel();
        &p.buf.text[a..b]
    }
    #[test]
    fn word_motions_replace_and_extend_selections() {
        for (key, expected) in [("w", "one "), ("e", "one"), ("W", "one "), ("E", "one")] {
            assert_eq!(selected(&run("one two", key)), expected, "{key}");
        }
        assert_eq!(selected(&run("one two three", "ww")), "two ");
        assert_eq!(selected(&run("one two three", "2w")), "one two ");
        assert_eq!(selected(&run("one two", "wv e".replace(' ', "").as_str())), "one two");
        assert_eq!(selected(&run("one two", "wwb")), "two");
        assert_eq!(selected(&run("one two", "wwB")), "two");
        assert_eq!(run("one", "v").vim_label(), Some("SEL"));
        assert_eq!(run("one", "i").vim_label(), Some("INS"));
        assert_eq!(run("one", "i\u{1b}").vim_label(), Some("NOR"));
    }
    #[test]
    fn finds_lines_and_selection_controls() {
        assert_eq!(selected(&run("abc def", "fd")), "abc d");
        assert_eq!(selected(&run("abc def", "td")), "abc ");
        assert_eq!(selected(&run("abc def", "glFb")), "bc def");
        assert_eq!(selected(&run("abc def", "glTb")), "c def");
        assert_eq!(selected(&run("a\nb\nc", "x")), "a\n");
        assert_eq!(selected(&run("a\nb\nc", "xx")), "a\nb\n");
        assert_eq!(selected(&run("a\nb\nc", "jX")), "a\nb\n");
        assert_eq!(selected(&run("abc", "%;")), "c");
        let mut p = run("abc", "%");
        p.key(Key::Char(';'), Mods { alt: true, ..Mods::default() }, Some(";"));
        assert_eq!((p.buf.anchor, p.buf.cur), (3, 0));
    }
    #[test]
    fn multiple_selections_regex_splitting_and_trimming() {
        let p = run("ab ab", "%sab\n");
        assert_eq!(p.extra_cursors.len(), 1);
        assert_eq!(selected(&p), "ab");
        assert_eq!(run("ab ab", "%sab\ncX\u{1b}").buf.text, "X X");
        let p = run("a,b,c", "%S,\n");
        assert_eq!(p.extra_cursors.len(), 2);
        assert_eq!(run("  ab  ", "%_").buf.sel(), (2, 4));
        let p = run("abc\nxyz", "lC");
        assert_eq!(p.extra_cursors.len(), 1);
        assert_eq!(selected(&p), "y");
        let mut p = run("a\nb\nc", "%");
        p.key(Key::Char('s'), Mods { alt: true, ..Mods::default() }, Some("s"));
        assert_eq!(p.extra_cursors.len(), 2);
        keys(&mut p, ",");
        assert!(p.extra_cursors.is_empty());
        let p = run("a x\nbb x", "%sx\n&");
        assert_eq!(p.buf.text, "a  x\nbb x");
    }
    #[test]
    fn selection_operators_and_undo() {
        assert_eq!(run("one two", "wd").buf.text, "two");
        assert_eq!(run("one two", "wcnew\u{1b}").buf.text, "newtwo");
        assert_eq!(run("one two", "wyP").buf.text, "one one two");
        assert_eq!(run("one two", "wyp").buf.text, "one one two");
        assert_eq!(run("one two", "wrX").buf.text, "XXXXtwo");
        assert_eq!(run("one two", "w~").buf.text, "ONE two");
        assert_eq!(run("one two", "wduU").buf.text, "two");
        assert_eq!(run("one two", "wdu").buf.text, "one two");
        assert_eq!(run("a\nb", "x>").buf.text, "\ta\nb");
        assert_eq!(run("\ta\nb", "x<").buf.text, "a\nb");
        assert_eq!(run("a\nb", "J").buf.text, "a b");
    }
    #[test]
    fn insertion_commands_share_one_undo_group() {
        for (key, expected) in [("i", "Xab"), ("a", "aXb"), ("I", "Xab"), ("A", "abX"), ("o", "ab\nX"), ("O", "X\nab")]
        {
            assert_eq!(run("ab", &format!("{key}X\u{1b}")).buf.text, expected, "{key}");
            assert_eq!(run("ab", &format!("{key}X\u{1b}u")).buf.text, "ab", "undo {key}");
        }
        let p = run("ab\ncd", "CiX\u{1b}");
        assert_eq!(p.buf.text, "Xab\nXcd");
        assert_eq!(p.extra_cursors.len(), 1);
    }
    #[test]
    fn goto_and_space_dispatch_existing_commands() {
        for (key, id) in [
            ("gd", "lsp_definition"),
            ("gr", "lsp_references"),
            ("gi", "lsp_implementation"),
            ("gy", "lsp_type_definition"),
            ("gn", "next_tab"),
            ("gp", "previous_tab"),
            (" f", "quick_open"),
            (" b", "helix_buffers"),
            (" s", "symbols"),
            (" /", "search"),
            (" k", "lsp_hover"),
            (" r", "lsp_rename"),
            (" a", "lsp_actions"),
            (" p", "paste"),
        ] {
            assert_eq!(run("abc", key).vim.request, Some(Request::Command(id)), "{key}");
        }
        assert_eq!(selected(&run("  abc\ndef", "ge")), "d");
        assert_eq!(selected(&run("  abc\ndef", "gegg")), "a");
        assert_eq!(selected(&run("  abc", "gs")), "a");
        assert_eq!(selected(&run("abc", "gl")), "c");
        assert_eq!(selected(&run("abc", "glgh")), "a");
        assert_eq!(run("abc", "w y").copy.as_deref(), Some("abc"));
    }
    #[test]
    fn match_objects_and_surrounds() {
        assert_eq!(selected(&run("(abc)", "mm")), ")");
        assert_eq!(selected(&run("(abc)", "lmi(")), "abc");
        assert_eq!(selected(&run("(abc)", "lma(")), "(abc)");
        assert_eq!(run("abc", "%ms(").buf.text, "(abc)");
        assert_eq!(run("(abc)", "lmd(").buf.text, "abc");
        assert_eq!(run("(abc)", "lmr([ ".trim_end()).buf.text, "[abc]");
    }
    #[test]
    fn ex_commands_reach_the_window_and_readonly_is_safe() {
        for c in ["w", "q", "wq", "o file", "buffer-close", "format"] {
            assert_eq!(run("abc", &format!(":{c}\n")).vim.request, Some(Request::Ex(c.into())));
        }
        let mut p = pane("abc");
        p.read_only = true;
        keys(&mut p, "%dcX\u{1b}");
        assert_eq!(p.buf.text, "abc");
    }
}
