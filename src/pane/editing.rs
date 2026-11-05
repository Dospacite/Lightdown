use super::*;

pub(super) fn matching(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '\'' => Some('\''),
        '"' => Some('"'),
        '`' => Some('`'),
        _ => None,
    }
}
impl Pane {
    pub(super) fn selected_lines(&self) -> (usize, usize) {
        let (a, b) = self.buf.sel();
        let last = if b > a && self.buf.starts.binary_search(&b).is_ok() { b - 1 } else { b };
        (self.buf.starts[self.buf.line_of(a)], self.buf.line(self.buf.line_of(last)).1)
    }
    pub(crate) fn comment_mark(&self) -> Option<(&'static str, &'static str)> {
        if self.python_at(self.buf.cur) {
            return Some(("#", ""));
        }
        if self.doc == Doc::Latex {
            return Some(("%", ""));
        }
        if self.prose_at(self.buf.cur) {
            return Some(("<!--", "-->"));
        }
        let ext =
            self.path.as_deref().and_then(Path::extension).and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "rs" | "js" | "jsx" | "ts" | "tsx" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs" | "java" | "go" | "swift"
            | "kt" | "jsonc" => Some(("//", "")),
            "sh" | "bash" | "zsh" | "py" | "pyi" | "rb" | "pl" | "toml" | "yaml" | "yml" | "r" | "R" => Some(("#", "")),
            "sql" | "lua" | "hs" => Some(("--", "")),
            "css" | "scss" => Some(("/*", "*/")),
            "html" | "htm" | "xml" | "svg" | "md" | "markdown" => Some(("<!--", "-->")),
            "tex" | "latex" => Some(("%", "")),
            _ if self
                .path
                .as_deref()
                .and_then(Path::file_name)
                .is_some_and(|s| s == "Makefile" || s == "Dockerfile" || s == "GNUmakefile") =>
            {
                Some(("#", ""))
            }
            _ => None,
        }
    }
    pub(super) fn line_operation(&mut self, action: &str) {
        let (start, end) = self.selected_lines();
        let last = self.buf.line_of(end);
        let (cur, anchor) = (self.buf.cur, self.buf.anchor);
        let block = self.buf.text[start..end].to_string();
        let (a, b, text, shift) = match action {
            "up" if start > 0 => {
                let previous = self.buf.starts[self.buf.line_of(start) - 1];
                let text = format!("{block}\n{}", &self.buf.text[previous..start - 1]);
                (previous, end, text, -((start - previous) as isize))
            }
            "down" if last + 1 < self.buf.lines() => {
                let next_end = self.buf.line(last + 1).1;
                let text = format!("{}\n{block}", &self.buf.text[end + 1..next_end]);
                (start, next_end, text, (next_end - end) as isize)
            }
            "duplicate" => (end, end, format!("\n{block}"), (block.len() + 1) as isize),
            "delete" if end < self.buf.text.len() => (start, end + 1, String::new(), 0),
            "delete" if start > 0 => (start - 1, end, String::new(), 0),
            "delete" => (start, end, String::new(), 0),
            _ => return,
        };
        if self.edit(a, b, &text, EditKind::Other) {
            if action != "delete" {
                let offset = |at: usize| editor::floor(&self.buf.text, (at as isize + shift).max(0) as usize);
                self.select(offset(anchor), offset(cur));
            }
            self.moved();
        }
    }
    pub(super) fn type_pair(&mut self, text: &str) -> bool {
        if self.read_only || text.len() != 1 {
            return false;
        }
        let ch = text.chars().next().unwrap();
        let (a, b) = self.buf.sel();
        if a == b
            && let Some(i) = self.auto_pairs.iter().position(|&(at, c)| at == a && c == ch)
        {
            self.auto_pairs.remove(i);
            self.select(a + 1, a + 1);
            return true;
        }
        let Some(close) = matching(ch) else { return false };
        let quote = matches!(ch, '\'' | '"' | '`');
        let previous = self.buf.text[..a].chars().next_back();
        let next = self.buf.text[b..].chars().next();
        let escaped = self.buf.text[..a].chars().rev().take_while(|&c| c == '\\').count() % 2 == 1;
        if a == b
            && (escaped
                || next.is_some_and(|c| c.is_alphanumeric() || c == '_')
                || quote && (previous.is_some_and(|c| c.is_alphanumeric() || c == '_') || next == Some(ch)))
        {
            return false;
        }
        let wrapped = format!("{ch}{}{close}", &self.buf.text[a..b]);
        if self.edit(a, b, &wrapped, EditKind::Other) {
            self.auto_pairs.push((b + 1, close));
            self.select(a + 1, b + 1);
        }
        true
    }
    pub(crate) fn jump_bracket(&mut self, select: bool) {
        let pair = self.matching_brackets();
        if pair.len() != 2 { return; }
        let (from, to) = (pair[0], pair[1]);
        if select {
            if from < to { self.select(from, to + 1); }
            else { self.select(from + 1, to); }
        } else { self.select(to, to); }
        self.moved();
    }
    /// Ignore brackets inside strings and comments marked by the highlighter.
    pub(super) fn matching_brackets(&self) -> Vec<usize> {
        let text = &self.buf.text;
        let cur = self.buf.cur;
        let candidate = [Some(cur), cur.checked_sub(1)]
            .into_iter()
            .flatten()
            .find(|&i| text.as_bytes().get(i).is_some_and(|b| b"()[]{}".contains(b)));
        let Some(at) = candidate else { return Vec::new() };
        let mut ranges: Vec<_> = self
            .md
            .as_ref()
            .into_iter()
            .flat_map(|md| &md.spans)
            .filter(|s| s.attr & (crate::md::SOFT | crate::md::DIM) != 0)
            .map(|s| (s.start as usize, s.end as usize))
            .collect();
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (a, b) in ranges {
            if let Some(last) = merged.last_mut().filter(|last| last.1 >= a) {
                last.1 = last.1.max(b);
            } else {
                merged.push((a, b));
            }
        }
        let excluded = |i: usize| {
            let at = merged.partition_point(|&(a, _)| a <= i);
            at > 0 && i < merged[at - 1].1
        };
        if excluded(at) {
            return Vec::new();
        }
        let ch = text.as_bytes()[at] as char;
        let forward = matches!(ch, '(' | '[' | '{');
        let mut stack = vec![ch];
        let iter: Box<dyn Iterator<Item = (usize, char)>> = if forward {
            Box::new(text[at + 1..].char_indices().map(move |(i, c)| (at + 1 + i, c)))
        } else {
            Box::new(text[..at].char_indices().rev())
        };
        for (i, c) in iter {
            if excluded(i) {
                continue;
            }
            if !"()[]{}".contains(c) {
                continue;
            }
            if forward == matches!(c, '(' | '[' | '{') {
                stack.push(c);
            } else {
                let open = stack.pop().unwrap();
                if !(if forward { matching(open) == Some(c) } else { matching(c) == Some(open) }) {
                    return Vec::new();
                }
                if stack.is_empty() {
                    return vec![at, i];
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pane(text: &str, name: &str) -> Pane {
        let fonts = Rc::new(RefCell::new(Fonts::new()));
        let pics = Rc::new(RefCell::new(Pics::new()));
        let mut p = Pane::new(fonts, pics);
        p.path = Some(PathBuf::from(name));
        p.set_text(text.into(), Doc::Code);
        p
    }
    #[test]
    fn bracket_navigation_selects_in_both_directions() {
        let mut p = pane("(é [x])", "a.rs");
        p.select(0, 0);
        p.jump_bracket(false);
        assert_eq!(p.buf.cur, 7);
        p.jump_bracket(true);
        assert_eq!(p.buf.sel(), (0, 8));
        p.select(0, 0);
        p.jump_bracket(true);
        assert_eq!(p.buf.sel(), (0, 8));
    }
    #[test]
    fn inferred_indentation_applies_to_tab_outdent_and_python() {
        let mut p = pane("root\n  child\n", "a.rs");
        let end = p.buf.text.len();
        p.select(end, end);
        p.indent(false);
        assert!(p.buf.text.ends_with("\n  "));
        p.indent(true);
        assert!(p.buf.text.ends_with('\n'));
        let mut p = pane("if yes:\n  pass\nif no:", "a.py");
        p.set_doc(Doc::Python);
        let end = p.buf.text.len();
        p.select(end, end);
        p.newline();
        assert!(p.buf.text.ends_with("\n  "));
    }
    #[test]
    fn comments_use_language_syntax_and_exclude_next_line_at_column_zero() {
        for (name, open, close) in [
            ("x.rs", "//", ""),
            ("x.ts", "//", ""),
            ("x.go", "//", ""),
            ("x.c", "//", ""),
            ("x.sql", "--", ""),
            ("x.css", "/*", " */"),
            ("x.html", "<!--", " -->"),
            ("x.toml", "#", ""),
        ] {
            let mut p = pane("  α\n  b\n", name);
            p.select(0, 5);
            p.comment();
            assert_eq!(p.buf.text, format!("  {open} α{close}\n  b\n"), "{name}");
            p.comment();
            assert_eq!(p.buf.text, "  α\n  b\n", "{name}");
            p.buf.undo();
            assert!(p.buf.text.contains(open));
        }
        let mut p = pane("{}", "x.json");
        p.comment();
        assert_eq!(p.buf.text, "{}");
        assert!(p.note.is_some());
    }
    #[test]
    fn line_operations_preserve_unicode_selection_and_final_newline() {
        let mut p = pane("one\nα\nlast", "x.rs");
        p.select(4, 6);
        p.line_operation("up");
        assert_eq!(p.buf.text, "α\none\nlast");
        assert_eq!(p.buf.sel(), (0, 2));
        p.line_operation("down");
        assert_eq!(p.buf.text, "one\nα\nlast");
        p.line_operation("duplicate");
        assert_eq!(p.buf.text, "one\nα\nα\nlast");
        assert_eq!(p.buf.sel(), (7, 9));
        p.line_operation("delete");
        assert_eq!(p.buf.text, "one\nα\nlast");
        p.select(p.buf.text.len(), p.buf.text.len());
        p.line_operation("delete");
        assert_eq!(p.buf.text, "one\nα");
        p.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(p.buf.text, "one\nα\nlast");
        let mut p = pane("a\nb\nc\n", "x.rs");
        p.select(0, 4);
        p.line_operation("down");
        assert_eq!(p.buf.text, "c\na\nb\n");
    }
    #[test]
    fn pairs_wrap_skip_delete_and_match_nested_brackets_outside_strings() {
        let mut p = pane("", "x.rs");
        p.key(Key::Char('('), Mods::default(), Some("("));
        assert_eq!(p.buf.text, "()");
        p.key(Key::Char('x'), Mods::default(), Some("x"));
        p.key(Key::Char(')'), Mods::default(), Some(")"));
        assert_eq!(p.buf.text, "(x)");
        assert_eq!(p.buf.cur, 3);
        p.set_text(String::new(), Doc::Code);
        p.key(Key::Char('['), Mods::default(), Some("["));
        p.key(Key::Backspace, Mods::default(), None);
        assert_eq!(p.buf.text, "");
        p.set_text("α".into(), Doc::Code);
        p.select(0, 2);
        p.key(Key::Char('"'), Mods::default(), Some("\""));
        assert_eq!(p.buf.text, "\"α\"");
        assert_eq!(p.buf.sel(), (1, 3));
        p.set_text("foo([\"]\"]) // [".into(), Doc::Code);
        p.select(3, 3);
        assert_eq!(p.matching_brackets(), vec![3, 9]);
        p.set_text("([)]".into(), Doc::Code);
        p.select(0, 0);
        assert!(p.matching_brackets().is_empty());
        p.set_text("don't".into(), Doc::Code);
        p.select(3, 3);
        p.key(Key::Char('\''), Mods::default(), Some("'"));
        assert_eq!(p.buf.text, "don''t");
    }
}
