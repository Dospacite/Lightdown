//! The find widget: a box at the top right of the editor, as in other
//! editors, with the text to find, how many matches there are and which
//! one is chosen, the way to the next and the previous, and below it, for
//! replacing, what each match becomes. Every match is marked in the text.

use super::*;
use crate::icons::Icon;
use crate::ui::{Rect, px};

/// A control of the find widget.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum FindHit {
    /// Shows or hides the row for replacing.
    Toggle,
    Regex,
    Word,
    Case,
    PreserveCase,
    Selection,
    /// The field to find, or with `true` the one to replace with.
    Field(bool),
    Prev,
    Next,
    Close,
    /// Replaces the match chosen.
    One,
    /// Replaces every match.
    All,
}

impl App {
    pub(super) fn toggle_find_selection(&mut self) {
        self.doc.find_scope = if self.doc.find_scope.is_some() { None } else {
            let (a, b) = self.doc.buf.sel();
            if a == b { self.message = Some("Select text before limiting find".into()); return; }
            Some((a, b))
        };
        self.found_for = None;
    }
    pub(super) fn scoped_matches(&self, needle: &str) -> Result<Vec<(usize, usize)>, String> {
        let mut matches = self.search_options.matches(&self.doc.buf.text, needle)?;
        if let Some((a, b)) = self.doc.find_scope {
            matches.retain(|&(start, end)| start >= a && end <= b);
        }
        Ok(matches)
    }
    /// Whether a find is open and has the keys, rather than having left
    /// them to the document.
    pub(super) fn prompt_has_keys(&self) -> bool {
        self.prompt.as_ref().is_some_and(|p| !p.away)
    }

    /// Where the find widget lies, while there is one.
    pub(super) fn find_rect(&self) -> Option<Rect> {
        let p = self.prompt.as_ref().filter(|p| p.finds())?;
        let (x, w) = self.document_area();
        let s = self.scale;
        if w < px(160.0, s) {
            return None;
        }
        let fw = px(510.0, s).min(w - px(20.0, s));
        let row = px(36.0, s);
        let h = if p.kind == PromptKind::Replace { row * 2.0 - px(6.0, s) } else { row };
        let top = self.title_h() + self.head_h() + px(4.0, s);
        Some(((x + w - fw - px(16.0, s)).round(), top, fw, h))
    }

    /// Marks every match of the find in the document, and works out how
    /// many there are and which is chosen, once for each text and needle.
    pub(super) fn find_marks(&mut self) {
        self.doc.show_find_scope = self.prompt.as_ref().is_some_and(|p| p.finds());
        let needle = match &self.prompt {
            Some(p) if p.finds() && self.reader.is_none() => p.text.clone(),
            _ => String::new(),
        };
        if needle.is_empty() {
            self.doc.found.clear();
            self.found_for = None;
            self.found_count = (0, None);
            return;
        }
        let stamp = (self.doc.buf.version, needle.clone(), self.search_options.clone());
        if self.found_for.as_ref() != Some(&stamp) {
            self.doc.found = self.scoped_matches(&needle).unwrap_or_default();
            self.found_for = Some(stamp);
        }
        let sel = self.doc.buf.sel();
        let current = self.doc.found.binary_search(&sel).ok();
        self.found_count = (self.doc.found.len(), current);
    }

    /// Goes to the next match, or with `back` the one before.
    pub(super) fn find_step(&mut self, back: bool) {
        if self.reader.is_some() {
            return self.find_in_pages(if back { reader::Seek::Back } else { reader::Seek::Next });
        }
        let a = self.doc.buf.sel().0;
        self.find(if back { a } else { editor::next(&self.doc.buf.text, a) }, !back);
    }

    /// A click on the find widget.
    pub(super) fn find_hit(&mut self, hit: FindHit) {
        let replaceable = self.reader.is_none() && !self.doc.read_only;
        let Some(p) = &mut self.prompt else { return };
        p.away = false;
        match hit {
            FindHit::Selection => self.toggle_find_selection(),
            FindHit::Regex => self.search_option("regex"),
            FindHit::Word => self.search_option("word"),
            FindHit::PreserveCase => self.search_option("preserve_case"),
            FindHit::Case => self.search_option("case"),
            FindHit::Toggle if replaceable => {
                p.kind = if p.kind == PromptKind::Replace { PromptKind::Find } else { PromptKind::Replace };
                p.on_with = false;
            }
            FindHit::Toggle => {}
            FindHit::Field(with) => p.on_with = with && p.kind == PromptKind::Replace,
            FindHit::Prev => self.find_step(true),
            FindHit::Next => self.find_step(false),
            FindHit::Close => { self.prompt = None; self.doc.find_scope = None; },
            FindHit::One => self.replace_key(Mods::default()),
            FindHit::All => self.replace_key(Mods { ctrl: true, ..Mods::default() }),
        }
    }

    /// A field of the find widget: its text, scrolled to keep the caret in
    /// view, and the caret while it has the keys.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn find_field(
        &mut self,
        c: &mut Canvas,
        fonts: &mut Fonts,
        r: Rect,
        text: &str,
        cur: usize,
        keys: bool,
        hint: &str,
        right: f32,
    ) {
        let s = self.scale;
        ui::input(c, r, keys, s);
        let size = px(theme::UI_TEXT, s);
        let pad = px(8.0, s);
        let room = r.2 - pad * 2.0 - right;
        let b = ui::base(r.1, r.3, size);
        c.clip(r.0 + 1.0, r.1, room + pad, r.3);
        if text.is_empty() {
            ui::text(c, fonts, r.0 + pad, b, hint, font::UI, size, theme::faint());
            if keys {
                ui::caret(c, r.0 + pad, r.1 + r.3 / 2.0, size * 1.3, s);
            }
        } else {
            let before = crate::bidi::caret_x(fonts,font::UI,size,text,cur.min(text.len()));
            let offset = (before - room + px(2.0, s)).max(0.0);
            ui::text(c, fonts, r.0 + pad - offset, b, text, font::UI, size, theme::fg());
            if keys {
                let id = if self.search_panel_has_keys() { 10+self.search.panel.field as u64 }
                    else { 20+self.prompt.as_ref().is_some_and(|p| p.on_with) as u64 };
                ui::remember_input(id,text,font::UI,size,r,r.0+pad-offset);
                ui::caret(c, r.0 + pad - offset + before, r.1 + r.3 / 2.0, size * 1.3, s);
            }
        }
        c.unclip();
    }

    /// Draws the find widget over the editor.
    pub(super) fn draw_find(&mut self, c: &mut Canvas) {
        let Some(r) = self.find_rect() else { return };
        let Some(p) = &self.prompt else { return };
        let (kind, text, cur, with, with_cur, on_with, away) =
            (p.kind, p.text.clone(), p.cur, p.with.clone(), p.with_cur, p.on_with, p.away);
        let s = self.scale;
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        ui::surface(c, r, px(theme::RADIUS, s), s);
        let row = px(36.0, s) - px(6.0, s);
        let inset = px(3.0, s);
        let btn = px(24.0, s);
        let small = px(theme::SMALL, s);

        // The first row: the toggle, the field and the way between matches.
        let y = r.1 + inset;
        let toggle = (r.0 + inset, y + (row - btn) / 2.0, btn, btn);
        let replaceable = self.reader.is_none() && !self.doc.read_only;
        if replaceable {
            let icon = if kind == PromptKind::Replace { Icon::ChevronDown } else { Icon::ChevronRight };
            ui::icon_button(c, toggle, icon, ui::inside(toggle, Some(self.pointer)), false, s);
            self.push_hit(toggle, Hit::Find(FindHit::Toggle), "Replace", &self.keys_of("replace"));
        }
        let mut fx = toggle.0 + btn + px(3.0, s);
        if r.2 > px(400.0, s) && self.reader.is_none() {
            for (label, hit, on, tip) in [
                ("[]", FindHit::Selection, self.doc.find_scope.is_some(), "Find in selection"),
                ("aA", FindHit::PreserveCase, self.search_options.preserve_case, "Preserve replacement case"),
                ("Aa", FindHit::Case, self.search_options.case, "Match case"),
                ("ab", FindHit::Word, self.search_options.word, "Whole words"),
                (".*", FindHit::Regex, self.search_options.regex, "Regular expression"),
            ] {
                let b = (fx, y + (row - btn) / 2.0, btn, btn);
                if on {
                    c.round_rect(b.0, b.1, b.2, b.3, px(theme::RADIUS, s), theme::selection());
                }
                ui::text(
                    c,
                    fonts,
                    b.0 + px(3.0, s),
                    ui::base(b.1, b.3, small),
                    label,
                    font::UI,
                    small,
                    if on { theme::fg() } else { theme::dim() },
                );
                let keys = if hit == FindHit::Selection { self.keys_of("find_selection") } else { String::new() };
                self.push_hit(b, Hit::Find(hit), tip, &keys);
                fx += btn + px(2.0, s);
            }
        }
        let buttons = btn * 3.0 + px(2.0, s) * 2.0 + px(6.0, s);
        let fw = r.0 + r.2 - inset - buttons - fx;
        let field = (fx, y + px(2.0, s), fw, row - px(4.0, s));
        let (total, current) = self.found_count;
        let count = match (total, current) {
            _ if text.is_empty() || self.reader.is_some() => String::new(),
            (0, _) => "No results".to_string(),
            (n, Some(i)) => format!("{} of {n}", i + 1),
            (n, None) => format!("{n} found"),
        };
        let cw = if count.is_empty() { 0.0 } else { fonts.width(font::UI, small, &count) + px(8.0, s) };
        let keys = !away && !on_with;
        self.find_field(c, fonts, field, &text, cur, keys, "Find", cw);
        if !count.is_empty() {
            let ink = if total == 0 { theme::danger() } else { theme::faint() };
            let cx = field.0 + field.2 - px(8.0, s) - (cw - px(8.0, s));
            ui::text(c, fonts, cx, ui::base(field.1, field.3, small), &count, font::UI, small, ink);
        }
        self.push_hit(field, Hit::Find(FindHit::Field(false)), "", "");
        let mut bx = field.0 + field.2 + px(6.0, s);
        for (icon, hit, label, keys) in [
            (Icon::ArrowUp, FindHit::Prev, "Previous match", "Shift+Enter"),
            (Icon::ArrowDown, FindHit::Next, "Next match", "Enter"),
            (Icon::X, FindHit::Close, "Close", "Esc"),
        ] {
            let b = (bx, y + (row - btn) / 2.0, btn, btn);
            ui::icon_button(c, b, icon, ui::inside(b, Some(self.pointer)), false, s);
            self.push_hit(b, Hit::Find(hit), label, keys);
            bx += btn + px(2.0, s);
        }

        // The second row, for replacing.
        if kind == PromptKind::Replace {
            let y = y + row;
            let field = (fx, y + px(2.0, s), fw, row - px(4.0, s));
            self.find_field(c, fonts, field, &with, with_cur, !away && on_with, "Replace", 0.0);
            self.push_hit(field, Hit::Find(FindHit::Field(true)), "", "");
            let mut bx = field.0 + field.2 + px(6.0, s);
            for (icon, hit, label, keys) in [
                (Icon::Replace, FindHit::One, "Replace", "Enter"),
                (Icon::ReplaceAll, FindHit::All, "Replace all", "Ctrl+Enter"),
            ] {
                let b = (bx, y + (row - btn) / 2.0, btn, btn);
                ui::icon_button(c, b, icon, ui::inside(b, Some(self.pointer)), false, s);
                self.push_hit(b, Hit::Find(hit), label, keys);
                bx += btn + px(2.0, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    fn draw(a: &mut App) {
        let mut px = vec![0; (a.w * a.h) as usize];
        a.draw(&mut Canvas::new(&mut px, a.w as i32, a.h as i32));
    }

    #[test]
    fn selection_limits_matches_and_replace_all() {
        let mut a = App::new(None);
        a.paste_text("one one one");
        a.doc.select(4, 7);
        a.toggle_find_selection();
        assert_eq!(a.scoped_matches("one").unwrap(), vec![(4, 7)]);
        // The range moves with edits before it and grows with edits in it.
        a.doc.select(0, 0);
        a.doc.insert("one ", crate::editor::EditKind::Other);
        assert_eq!(a.scoped_matches("one").unwrap(), vec![(8, 11)]);
        a.doc.select(9, 9);
        a.doc.insert("xx", crate::editor::EditKind::Other);
        assert_eq!(a.doc.find_scope, Some((8, 13)));
        a.doc.select(3, 10);
        a.doc.insert("", crate::editor::EditKind::Other);
        assert_eq!(a.doc.find_scope, Some((3, 6)));
        a.toggle_find_selection();
        assert_eq!(a.scoped_matches("one").unwrap(), a.search_options.matches(&a.doc.buf.text, "one").unwrap());
    }
    #[test]
    fn the_find_bar_counts_matches_and_stays_while_the_document_has_the_keys() {
        let mut a = App::new(None);
        a.resize(1000, 700, 1.0);
        a.paste_text("one two one three one");
        a.key(Key::Home, CTRL, None);
        a.key(Key::Char('f'), CTRL, None);
        for ch in "one".chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
        draw(&mut a);
        assert_eq!(a.found_count, (3, Some(0)));
        assert_eq!(a.doc.found.len(), 3);
        a.key(Key::Enter, NONE, None);
        draw(&mut a);
        assert_eq!(a.found_count, (3, Some(1)));
        // Its buttons go back and forth.
        let prev = a.hits.iter().find(|(_, h)| *h == Hit::Find(FindHit::Prev)).unwrap().0;
        a.press(prev.0 + 4.0, prev.1 + 4.0, NONE, 1);
        draw(&mut a);
        assert_eq!(a.found_count, (3, Some(0)));
        // A click in the text gives the keys back; the bar stays.
        let (x, y) = (a.doc.column().0 + 4.0, a.title_h() + a.head_h() + 60.0);
        a.press(x, y, NONE, 1);
        a.release();
        assert!(a.prompt.as_ref().is_some_and(|p| p.away) && a.doc_has_keys());
        a.key(Key::Char('x'), NONE, Some("x"));
        assert!(a.doc.buf.text.contains('x'));
        // F3 still goes on; Ctrl+F takes the keys back; Esc closes it.
        a.key(Key::Char('f'), CTRL, None);
        assert!(a.prompt_has_keys());
        a.key(Key::Escape, NONE, None);
        assert!(a.prompt.is_none());
        draw(&mut a);
        assert!(a.doc.found.is_empty());
    }
}
