//! The file's format in the status bar: its indentation, line endings and
//! encoding, each a control with a menu, and the commands behind them.
use super::assist::MenuKind;
use super::chrome::Hit;
use super::*;
use crate::editor::Eol;
use crate::pane::{IndentSource, unit_label};
use crate::ui::{Entry, Menu};

/// The status bar leaves at least this share of its width to the branch
/// and messages at its left; items that do not fit drop out.
const LEFT_ROOM: f32 = 0.4;

impl App {
    /// Whether the file has a format to show: text in a pane, not a PDF,
    /// a picture, a notebook or command output.
    pub(super) fn shows_file_format(&self) -> bool {
        self.reader.is_none() && self.nb.is_none() && self.doc.output_name.is_none()
    }

    /// The format items, from `right` leftwards. Returns the new right edge.
    pub(super) fn file_format_status(&mut self, c: &mut Canvas, fonts: &mut Fonts, right: f32) -> f32 {
        if !self.shows_file_format() {
            return right;
        }
        // Each drops out when it does not fit, and those further left with it.
        let at = self.status_indent(c, fonts, right);
        if at == right {
            return right;
        }
        let right = at;
        let at = self.status_line_endings(c, fonts, right);
        if at == right {
            return right;
        }
        self.status_encoding(c, fonts, at)
    }

    fn status_indent(&mut self, c: &mut Canvas, fonts: &mut Fonts, right: f32) -> f32 {
        let label = self.doc.indent_label();
        let from = match self.doc.indent_source() {
            IndentSource::File => "set for this file",
            IndentSource::EditorConfig => "from .editorconfig",
            IndentSource::Detected => "detected",
            IndentSource::Default => "the default",
        };
        let tip = format!("Indentation: {label}, {from}. Set the unit or reindent");
        self.format_item(c, fonts, right, &label, Hit::FileIndent, &tip)
    }

    fn status_line_endings(&mut self, c: &mut Canvas, fonts: &mut Fonts, right: f32) -> f32 {
        let label = self.doc.buf.eol.label();
        let tip = format!("Line endings: {label}. Convert the file");
        self.format_item(c, fonts, right, label, Hit::FileEol, &tip)
    }

    fn status_encoding(&mut self, c: &mut Canvas, fonts: &mut Fonts, right: f32) -> f32 {
        let label = if self.doc.has_bom() { "UTF-8 with BOM" } else { "UTF-8" };
        let tip = format!("Encoding: {label}. Add or remove the byte order mark");
        self.format_item(c, fonts, right, label, Hit::FileEncoding, &tip)
    }

    /// One format item, if it fits; a control unless the file is read-only.
    fn format_item(&mut self, c: &mut Canvas, fonts: &mut Fonts, right: f32, label: &str, hit: Hit, tip: &str) -> f32 {
        let s = self.scale;
        let width = fonts.width(font::UI, px(theme::SMALL, s), label) + px(12.0, s);
        if right - width < (self.w * LEFT_ROOM).max(px(200.0, s)) {
            return right;
        }
        let hit = (!self.doc.read_only).then(|| (hit, tip, String::new()));
        self.status_item(c, fonts, right, label, None, theme::dim(), hit) - px(2.0, s)
    }

    /// The menu for one of the format items, rising from it.
    pub(super) fn open_format_menu(&mut self, hit: Hit) {
        if !self.shows_file_format() {
            self.message = Some("Only text files have indentation, line endings and an encoding".into());
            return;
        }
        if self.doc.read_only {
            self.message = Some("This file is read-only".into());
            return;
        }
        let (title, entries, current) = match hit {
            Hit::FileIndent => {
                let unit = self.doc.indent_unit_at(self.doc.buf.cur);
                let mut entries: Vec<Entry> = ["\t", "  ", "    ", "        "]
                    .iter()
                    .map(|u| Entry::new(&format!("indent:{u}"), &unit_label(u), "for this file"))
                    .collect();
                let detail = format!("rewrite leading whitespace as {}", unit_label(&unit));
                entries.push(Entry::new("reindent", "Reindent with this unit", &detail));
                ("Indentation", entries, format!("indent:{unit}"))
            }
            Hit::FileEol => {
                let entries = [(Eol::Lf, "Linux and macOS"), (Eol::Crlf, "Windows"), (Eol::Cr, "classic Mac OS")]
                    .iter()
                    .map(|(e, what)| Entry::new(&format!("eol:{}", e.label()), e.label(), what))
                    .collect();
                ("Line endings, saved on the next save", entries, format!("eol:{}", self.doc.buf.eol.label()))
            }
            _ => {
                let entries = vec![
                    Entry::new("bom:off", "UTF-8", "no byte order mark"),
                    Entry::new("bom:on", "UTF-8 with BOM", "starts with a byte order mark"),
                ];
                ("Encoding", entries, if self.doc.has_bom() { "bom:on" } else { "bom:off" }.to_string())
            }
        };
        let mut menu = Menu::new(title, entries, &current);
        if let Some((r, _)) = self.hits.iter().find(|(_, h)| *h == hit) {
            (menu.anchor, menu.rise) = (Some((r.0, r.1)), true);
        }
        self.menu = Some((MenuKind::FileFormat, menu));
        self.redraw = true;
    }

    /// Carries out a row of a format menu.
    pub(super) fn pick_file_format(&mut self, key: &str) {
        let changed = if let Some(unit) = key.strip_prefix("indent:") {
            self.doc.set_indent_unit(unit);
            self.message = Some(format!("Indenting with {} in this file", unit_label(unit)));
            false
        } else if key == "reindent" {
            let done = self.doc.reindent_file();
            if !done {
                self.message = Some("Already indented with this unit".into());
            }
            done
        } else if let Some(name) = key.strip_prefix("eol:") {
            let eol = [Eol::Lf, Eol::Crlf, Eol::Cr].into_iter().find(|e| e.label() == name).unwrap_or_default();
            let done = self.doc.set_line_ending(eol);
            if done {
                self.message = Some(format!("Lines end in {name} from the next save"));
            }
            done
        } else if let Some(on) = key.strip_prefix("bom:") {
            self.doc.set_bom(on == "on")
        } else {
            false
        };
        if changed {
            self.edited();
        }
        self.redraw = true;
    }

    /// The palette's format commands.
    pub(super) fn file_format_command(&mut self, id: &str) {
        match id {
            "file_indent" => self.open_format_menu(Hit::FileIndent),
            "file_eol" => self.open_format_menu(Hit::FileEol),
            "file_encoding" => self.open_format_menu(Hit::FileEncoding),
            "file_reindent" if self.shows_file_format() => self.pick_file_format("reindent"),
            "file_bom" if self.shows_file_format() => {
                let key = if self.doc.has_bom() { "bom:off" } else { "bom:on" };
                self.pick_file_format(key)
            }
            _ => self.message = Some("Only text files have indentation, line endings and an encoding".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_files_convert_in_one_undo_step_and_save_with_the_chosen_endings() {
        let dir = std::env::temp_dir().join(format!("ld-e4-eol-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "\u{feff}one\r\ntwo\r\n").unwrap();
        let mut a = App::new(Some(path.clone()));
        assert_eq!(a.doc.buf.eol, Eol::Crlf);
        assert!(a.doc.has_bom());
        assert!(a.shows_file_format());
        a.pick_file_format("eol:LF");
        a.pick_file_format("bom:off");
        assert_eq!(a.contents(), "one\ntwo\n");
        a.doc.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        a.doc.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(a.contents(), "\u{feff}one\r\ntwo\r\n");
        a.file_format_command("file_eol");
        assert!(matches!(a.menu, Some((MenuKind::FileFormat, _))));
        a.menu = None;
        a.pick_file_format("indent:\t");
        assert_eq!(a.doc.indent_label(), "Tabs");
        std::fs::write(&path, "x\ry\r").unwrap();
        let a = App::new(Some(path.clone()));
        assert_eq!((a.doc.buf.eol, a.doc.buf.text.as_str()), (Eol::Cr, "x\ny\n"));
        assert_eq!(a.contents(), "x\ry\r");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn word_wrap_is_toggled_per_kind_and_shift_wheel_scrolls_unwrapped_code() {
        let dir = std::env::temp_dir().join(format!("ld-e4-wrap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("long.rs");
        std::fs::write(&path, format!("fn f() {{}}\n// {}\n", "wide ".repeat(200))).unwrap();
        let mut a = App::new(Some(path));
        a.resize(900, 600, 1.0);
        let mut px = vec![0u32; 900 * 600];
        assert!(a.doc.word_wrap, "code wraps by default");
        a.key(Key::Char('z'), Mods { alt: true, ..Mods::default() }, None);
        assert!(!a.doc.word_wrap);
        assert_eq!(a.cfg.word_wrap, [false, true, true], "only code changed");
        a.draw(&mut Canvas::new(&mut px, 900, 600));
        let (x, y, w, h) = a.doc.bounds();
        a.pointer = (x + w / 2.0, y + h / 2.0);
        a.scroll_across_lines(3.0);
        assert!(a.doc.hoffset() > 0.0, "the wheel moved the text sideways");
        a.key(Key::Char('z'), Mods { alt: true, ..Mods::default() }, None);
        assert_eq!(a.doc.hoffset(), 0.0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
