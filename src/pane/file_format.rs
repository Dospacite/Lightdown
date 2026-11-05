//! How the file is laid out on disk: its indentation unit, line endings
//! and byte order mark, and the edits that change them.
use super::*;
use crate::editor::Eol;

/// The byte order mark, kept as the text's first character when present.
pub(crate) const BOM: char = '\u{feff}';

/// Where the indentation unit came from, for the status bar's tooltip.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum IndentSource {
    File,
    EditorConfig,
    Detected,
    Default,
}

impl Pane {
    /// The unit Tab inserts at `off`: the file's own, else spaces in prose,
    /// Python and LaTeX and a tab in other code.
    pub(crate) fn indent_unit_at(&self, off: usize) -> String {
        self.indentation().unwrap_or_else(|| {
            let spaces = self.prose_at(off) || self.python_at(off) || self.doc == Doc::Latex;
            if spaces { "    " } else { "\t" }.into()
        })
    }

    pub(crate) fn indent_source(&self) -> IndentSource {
        if self.file_indent.is_some() {
            IndentSource::File
        } else if self.path.as_deref().and_then(|p| crate::app::saving::Rules::read(p).indent()).is_some() {
            IndentSource::EditorConfig
        } else if self.inferred_indent.is_some() {
            IndentSource::Detected
        } else {
            IndentSource::Default
        }
    }

    /// "Spaces: 4" or "Tabs".
    pub(crate) fn indent_label(&self) -> String {
        unit_label(&self.indent_unit_at(self.buf.cur))
    }

    /// Uses `unit` for this file until it is closed, over EditorConfig and
    /// what was detected.
    pub(crate) fn set_indent_unit(&mut self, unit: &str) {
        self.file_indent = Some(unit.to_string());
        self.redraw = true;
    }

    /// Rewrites every line's leading whitespace in the current unit, as one
    /// undo step. Returns whether anything changed.
    pub(crate) fn reindent_file(&mut self) -> bool {
        let unit = self.indent_unit_at(self.buf.cur);
        let from = indentation::detect(&self.buf.text).unwrap_or_else(|| "    ".into());
        let text = reindent(&self.buf.text, &from, &unit);
        self.replace_keeping_place(text)
    }

    /// Replaces only what differs between the text and `text`, as one undo
    /// step, and keeps the cursor on its line at the same place in its
    /// content.
    fn replace_keeping_place(&mut self, text: String) -> bool {
        let old = &self.buf.text;
        if *old == text {
            return false;
        }
        let head = old.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
        let head = editor::floor(&text, editor::floor(old, head));
        let room = old.len().min(text.len()) - head;
        let tail = old.bytes().rev().zip(text.bytes().rev()).take(room).take_while(|(a, b)| a == b).count();
        let (mut old_end, mut new_end) = (old.len() - tail, text.len() - tail);
        while !old.is_char_boundary(old_end) || !text.is_char_boundary(new_end) {
            (old_end, new_end) = (old_end + 1, new_end + 1);
        }
        let line = self.buf.line_of(self.buf.cur);
        let (ls, le) = self.buf.line(line);
        let lead = |s: &str| s.len() - s.trim_start_matches([' ', '\t']).len();
        let into = self.buf.cur.saturating_sub(ls + lead(&old[ls..le]));
        if !self.edit(head, old_end, &text[head..new_end], EditKind::Other) {
            return false;
        }
        let (ls, le) = self.buf.line(line.min(self.buf.lines() - 1));
        let at = editor::floor(&self.buf.text, (ls + lead(&self.buf.text[ls..le]) + into).min(le));
        self.select(at, at);
        self.moved();
        true
    }

    /// Changes the line endings written on saving, as one undo step.
    pub(crate) fn set_line_ending(&mut self, eol: Eol) -> bool {
        if self.read_only {
            self.note = Some("This file is read-only".into());
            return false;
        }
        let Some(c) = self.buf.set_eol(eol) else { return false };
        self.changed(c);
        self.redraw = true;
        true
    }

    pub(crate) fn has_bom(&self) -> bool {
        self.buf.text.starts_with(BOM)
    }

    /// Adds or removes the byte order mark, as one undo step.
    pub(crate) fn set_bom(&mut self, on: bool) -> bool {
        if on == self.has_bom() {
            return false;
        }
        let (cur, anchor) = (self.buf.cur, self.buf.anchor);
        let width = BOM.len_utf8();
        let done = if on {
            self.edit(0, 0, "\u{feff}", EditKind::Other)
        } else {
            self.edit(0, width, "", EditKind::Other)
        };
        if done {
            let shift = |at: usize| if on { at + width } else { at.saturating_sub(width) };
            self.select(shift(anchor), shift(cur));
            self.moved();
        }
        done
    }
}

pub(crate) fn unit_label(unit: &str) -> String {
    if unit == "\t" { "Tabs".into() } else { format!("Spaces: {}", unit.len()) }
}

/// Leading whitespace measured in `from` units, rewritten in `unit`s. What
/// is left over, less than a unit, stays as spaces. Tabs count to the next
/// multiple of four columns.
fn reindent(text: &str, from: &str, unit: &str) -> String {
    let width = if from == "\t" { 4 } else { from.len().max(1) };
    text.split('\n')
        .map(|line| {
            let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
            if lead == line.len() {
                return line.to_string();
            }
            let columns = line[..lead].chars().fold(0, |n, c| if c == '\t' { n + 4 - n % 4 } else { n + 1 });
            format!("{}{}{}", unit.repeat(columns / width), " ".repeat(columns % width), &line[lead..])
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(text: &str, doc: Doc) -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text(text.into(), doc);
        p
    }

    #[test]
    fn reindenting_converts_depth_and_keeps_content_and_blank_lines() {
        assert_eq!(reindent("a\n  b\n    c\n  \n", "  ", "\t"), "a\n\tb\n\t\tc\n  \n");
        assert_eq!(reindent("a\n\tb\n\t\t c", "\t", "  "), "a\n  b\n     c");
    }

    #[test]
    fn a_unit_set_for_the_file_wins_and_reindent_is_one_undo_step() {
        let mut p = pane("fn a() {\n    b();\n        c();\n}\n", Doc::Code);
        assert_eq!((p.indent_label(), p.indent_source()), ("Spaces: 4".into(), IndentSource::Detected));
        let at = p.buf.starts[2] + 8;
        p.select(at, at);
        p.set_indent_unit("  ");
        assert_eq!((p.indent_label(), p.indent_source()), ("Spaces: 2".into(), IndentSource::File));
        assert!(p.reindent_file());
        assert_eq!(p.buf.text, "fn a() {\n  b();\n    c();\n}\n");
        assert_eq!(&p.buf.text[p.buf.cur..p.buf.cur + 2], "c(", "the cursor keeps its place in the line");
        assert!(!p.reindent_file(), "nothing left to change");
        p.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(p.buf.text, "fn a() {\n    b();\n        c();\n}\n");
        assert_eq!(pane("x", Doc::Code).indent_label(), "Tabs");
        assert_eq!(pane("x", Doc::Python).indent_label(), "Spaces: 4");
    }

    #[test]
    fn line_endings_and_the_byte_order_mark_change_as_single_undo_steps() {
        let mut p = pane("a\nb", Doc::Text);
        assert!(p.set_line_ending(Eol::Crlf));
        assert!(p.buf.modified);
        assert!(p.set_bom(true));
        assert!(p.has_bom());
        assert!(!p.set_bom(true));
        let undo = Mods { ctrl: true, ..Mods::default() };
        p.key(Key::Char('z'), undo, None);
        assert!(!p.has_bom());
        assert_eq!(p.buf.eol, Eol::Crlf);
        p.key(Key::Char('z'), undo, None);
        assert_eq!(p.buf.eol, Eol::Lf);
        assert_eq!(p.buf.text, "a\nb");
    }
}
