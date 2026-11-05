//! Multiple selections share one undoable edit.
use super::*;
impl Pane {
    fn selections(&self) -> Vec<(usize, usize)> {
        let mut s = self.extra_cursors.clone();
        s.push((self.buf.anchor, self.buf.cur));
        s
    }
    pub(crate) fn select_all_occurrences(&mut self) {
        if self.doc == Doc::Cells { return; }
        if !self.buf.has_sel() {
            let (a, b) = editor::word_at(&self.buf.text, self.buf.cur);
            self.select(a, b);
        }
        let (a, b) = self.buf.sel();
        if a == b { return; }
        self.extra_cursors = self.buf.text.match_indices(&self.buf.text[a..b])
            .map(|(p, s)| (p, p + s.len()))
            .filter(|r| *r != (a,b)).take(9999).collect();
        self.moved();
    }
    pub(crate) fn skip_occurrence(&mut self) {
        let old = (self.buf.anchor, self.buf.cur);
        self.add_next_cursor();
        if old != (self.buf.anchor, self.buf.cur) {
            self.extra_cursors.retain(|r| *r != old);
        }
    }
    pub(crate) fn add_next_cursor(&mut self) {
        if self.doc == Doc::Cells {
            return;
        }
        if !self.buf.has_sel() {
            let (a, b) = editor::word_at(&self.buf.text, self.buf.cur);
            self.select(a, b);
            return;
        }
        let (a, b) = self.buf.sel();
        let needle = &self.buf.text[a..b];
        let mut selections = self.selections();
        let from = selections.iter().map(|(a, b)| a.max(b)).max().copied().unwrap_or(b);
        let hit = self
            .buf
            .text
            .match_indices(needle)
            .map(|(p, s)| (p, p + s.len()))
            .filter(|&(a, b)| !selections.iter().any(|&(x, y)| a < x.max(y) && b > x.min(y)))
            .min_by_key(|(p, _)| (*p < from, *p));
        if let Some((a, b)) = hit {
            selections.push((a, b));
            self.extra_cursors = selections;
            self.extra_cursors.pop();
            (self.buf.anchor, self.buf.cur) = (a, b);
            self.moved();
        }
    }
    pub(crate) fn add_line_cursor(&mut self, down: bool) {
        if self.doc == Doc::Cells {
            return;
        }
        let line = self.buf.line_of(self.buf.cur);
        let col = self.buf.text[self.buf.starts[line]..self.buf.cur].chars().count();
        let target = if down { line + 1 } else { line.saturating_sub(1) };
        if target >= self.buf.lines() || target == line {
            return;
        }
        let (s, e) = self.buf.line(target);
        let at = self.buf.text[s..e].char_indices().nth(col).map_or(e, |(i, _)| s + i);
        self.extra_cursors.push((self.buf.anchor, self.buf.cur));
        self.buf.cur = at;
        self.buf.anchor = at;
        self.normalize_cursors();
        self.moved();
    }
    fn normalize_cursors(&mut self) {
        let primary = (self.buf.anchor.min(self.buf.cur), self.buf.anchor.max(self.buf.cur));
        let mut s = self.selections();
        s.sort_by_key(|(a, b)| a.min(b).to_owned());
        s.dedup();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (a, b) in s {
            let r = (a.min(b), a.max(b));
            if let Some(last) = merged.last_mut().filter(|last| r.0 <= last.1) {
                last.1 = last.1.max(r.1);
            } else {
                merged.push(r);
            }
        }
        let active =
            merged.iter().position(|r| r.0 <= primary.0 && r.1 >= primary.1).unwrap_or(merged.len().saturating_sub(1));
        if !merged.is_empty() {
            let (a, b) = merged.remove(active);
            self.buf.anchor = a;
            self.buf.cur = b;
        }
        self.extra_cursors = merged;
    }
    pub(crate) fn column_select(&mut self, from: usize, to: usize) {
        let (a, b) = (self.buf.line_of(from), self.buf.line_of(to));
        let ca = self.buf.text[self.buf.starts[a]..from].chars().count();
        let cb = self.buf.text[self.buf.starts[b]..to].chars().count();
        let mut ranges = Vec::new();
        for l in a.min(b)..=a.max(b) {
            let (s, e) = self.buf.line(l);
            let at = |col| self.buf.text[s..e].char_indices().nth(col).map_or(e, |(i, _)| s + i);
            ranges.push((at(ca), at(cb)));
        }
        if let Some((anchor, cur)) = ranges.pop() {
            self.buf.anchor = anchor;
            self.buf.cur = cur;
        }
        self.extra_cursors = ranges;
        self.redraw = true;
    }
    pub(crate) fn multiple_insert(&mut self, text: &str) -> bool {
        if self.extra_cursors.is_empty() {
            return false;
        }
        self.multiple_replace(Some(text), false, false);
        true
    }
    fn multiple_replace(&mut self, insert: Option<&str>, delete: bool, word: bool) {
        self.normalize_cursors();
        let old = self.selections();
        let mut edits: Vec<_> = old
            .into_iter()
            .map(|(a, b)| {
                let (mut a, mut b) = (a.min(b), a.max(b));
                if a == b && insert.is_none() {
                    if delete {
                        b = if word { editor::word_right(&self.buf.text, b) } else { editor::next(&self.buf.text, b) };
                    } else {
                        a = if word { editor::word_left(&self.buf.text, a) } else { editor::prev(&self.buf.text, a) };
                    }
                }
                (a, b, insert.unwrap_or("").to_string())
            })
            .collect();
        edits.sort_by_key(|r| r.0);
        // Backspacing neighboring carets may create touching ranges.
        let mut merged: Vec<(usize, usize, String)> = Vec::new();
        for e in edits {
            if let Some(last) = merged.last_mut().filter(|r| e.0 < r.1) {
                last.1 = last.1.max(e.1);
            } else {
                merged.push(e);
            }
        }
        let mut out = String::new();
        let mut prev = 0;
        let mut cursors = Vec::new();
        for (a, b, s) in merged {
            out.push_str(&self.buf.text[prev..a]);
            out.push_str(&s);
            let at = out.len();
            cursors.push((at, at));
            prev = b;
        }
        out.push_str(&self.buf.text[prev..]);
        self.extra_cursors.clear();
        if self.edit(0, self.buf.text.len(), &out, EditKind::Other) {
            self.buf.commit();
            if let Some((a, b)) = cursors.pop() {
                self.buf.anchor = a;
                self.buf.cur = b;
            }
            self.extra_cursors = cursors;
            self.moved();
        }
    }
    pub(crate) fn multiple_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if mods.ctrl && key == Key::Char('d') && !mods.alt {
            self.add_next_cursor();
            return true;
        }
        if mods.ctrl && mods.alt && matches!(key, Key::Up | Key::Down) {
            self.add_line_cursor(key == Key::Down);
            return true;
        }
        if self.extra_cursors.is_empty() {
            return false;
        }
        match key {
            Key::Escape => self.extra_cursors.clear(),
            Key::Backspace | Key::Delete => self.multiple_replace(None, key == Key::Delete, mods.ctrl),
            Key::Enter => self.multiple_replace(Some("\n"), false, false),
            Key::Tab => self.multiple_replace(Some("    "), false, false),
            Key::Char('c' | 'x') if mods.ctrl => {
                let mut s = self.selections();
                s.sort_by_key(|r| r.0.min(r.1));
                self.copy =
                    Some(s.iter().map(|&(a, b)| &self.buf.text[a.min(b)..a.max(b)]).collect::<Vec<_>>().join("\n"));
                if key == Key::Char('x') {
                    self.multiple_replace(Some(""), false, false);
                }
            }
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => {
                let mut ranges = std::mem::take(&mut self.extra_cursors);
                ranges.push((self.buf.anchor, self.buf.cur));
                let mut out = Vec::new();
                for (a, b) in ranges {
                    self.buf.anchor = a;
                    self.buf.cur = b;
                    self.key(key, mods, None);
                    out.push((self.buf.anchor, self.buf.cur));
                }
                if let Some((a, b)) = out.pop() {
                    self.buf.anchor = a;
                    self.buf.cur = b;
                }
                self.extra_cursors = out;
                self.normalize_cursors();
            }
            _ if !mods.ctrl && !mods.alt && text.is_some_and(|t| !t.chars().any(char::is_control)) => {
                self.multiple_replace(text, false, false)
            }
            _ => {
                self.extra_cursors.clear();
                return false;
            }
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use crate::app::{App, Key, Mods};
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
    #[test]
    fn all_occurrences_and_skipping_preserve_undo() {
        let mut a = App::new(None);
        a.paste_text("é é é");
        a.doc.select(0, 2);
        a.doc.skip_occurrence();
        assert_eq!(a.doc.buf.sel(), (3, 5));
        assert!(a.doc.extra_cursors.is_empty());
        a.doc.select_all_occurrences();
        assert_eq!(a.doc.extra_cursors.len(), 2);
        a.doc.paste_text("x");
        assert_eq!(a.doc.buf.text, "x x x");
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "é é é");
    }
    #[test]
    fn matching_unicode_selections_edit_and_undo_together() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("écho écho écho");
        a.doc.select(0, 5);
        a.key(Key::Char('d'), CTRL, None);
        a.key(Key::Char('d'), CTRL, None);
        assert_eq!(a.doc.extra_cursors.len(), 2);
        a.key(Key::Char('x'), Mods::default(), Some("x"));
        assert_eq!(a.doc.buf.text, "x x x");
        a.key(Key::Backspace, Mods::default(), None);
        assert_eq!(a.doc.buf.text, "  ");
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "x x x");
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "écho écho écho");
    }
    #[test]
    fn cursors_extend_upward_and_columns_replace_short_lines() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("abc\nx\néfg");
        a.doc.select(10, 10);
        let mods = Mods { ctrl: true, alt: true, shift: false };
        a.key(Key::Up, mods, None);
        a.key(Key::Up, mods, None);
        assert_eq!(a.doc.extra_cursors.len(), 2);
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 0);
        a.doc.column_select(1, 9);
        a.doc.paste_text("!");
        assert_eq!(a.doc.buf.text, "a!c\nx!\né!g");
    }
}
