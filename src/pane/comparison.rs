//! Reusable read-only diff view, carried with its tab like any other pane state.
use super::*;
impl Pane {
    pub(crate) fn toggle_comparison(&mut self) {
        if self.read_only && self.diff {
            self.comparison = if self.comparison.is_some() { None } else { Some(crate::diff::View::parse(&self.buf.text)) };
            self.redraw = true;
        } else { self.note = Some("Open a text diff first".into()); }
    }
    pub(super) fn comparison_scroll(&mut self, dy: f32) -> bool {
        let row = (self.px() * theme::LINE_HEIGHT).round();
        if let Some(view) = &mut self.comparison {
            view.scroll = (view.scroll + dy).clamp(0.0, (view.rows.len() as f32 * row - self.h + row).max(0.0));
            self.redraw = true;
            return true;
        }
        false
    }
    pub(super) fn comparison_key(&mut self, key: Key, mods: Mods) -> bool {
        if self.comparison.is_none() || mods.ctrl || mods.alt { return false; }
        let row = self.px() * theme::LINE_HEIGHT;
        match key {
            Key::Up => { self.comparison_scroll(-row); }
            Key::Down => { self.comparison_scroll(row); }
            Key::PageUp => { self.comparison_scroll(-self.h * 0.85); }
            Key::PageDown => { self.comparison_scroll(self.h * 0.85); }
            Key::Home => { self.comparison.as_mut().unwrap().scroll = 0.0; }
            Key::End => { self.comparison_scroll(f32::MAX); }
            Key::Left | Key::Right => {
                let v = self.comparison.as_mut().unwrap();
                v.horizontal = (v.horizontal + if key == Key::Left { -80.0 } else { 80.0 }).max(0.0);
            }
            _ => return false,
        }
        self.redraw = true;
        true
    }
    pub(super) fn draw_comparison(&mut self, c: &mut Canvas) -> bool {
        let px = self.px();
        let row_h = (px * theme::LINE_HEIGHT).round();
        let Some(v) = &self.comparison else { return false };
        let (x, y, w, h) = (self.x, self.y, self.w, self.h);
        if w < 1.0 || h < 1.0 { return true; }
        let half = w / 2.0;
        let mut fonts = self.fonts.borrow_mut();
        let face = fonts.mono();
        c.clip(x, y, w, h);
        c.rect(x, y, w, row_h, theme::inset());
        crate::ui::text(c, &mut fonts, x + 8.0, y + px, "Before", crate::font::UI, px * 0.8, theme::dim());
        crate::ui::text(c, &mut fonts, x + half + 8.0, y + px, "After", crate::font::UI, px * 0.8, theme::dim());
        for (i, row) in v.rows.iter().enumerate().skip((v.scroll / row_h).floor() as usize) {
            let ry = y + row_h + i as f32 * row_h - v.scroll;
            if ry > y + h { break; }
            if row.heading {
                c.clip(x, y + row_h, w, (h - row_h).max(0.0));
                c.rect(x, ry, w, row_h, theme::inset());
                let label = crate::ui::fit(&mut fonts, face, px * 0.8, &row.left.text, (w - 16.0).max(0.0));
                crate::ui::text(c, &mut fonts, x + 8.0, ry + px, &label, face, px * 0.8, theme::dim());
                continue;
            }
            for (col, cell) in [&row.left, &row.right].iter().enumerate() {
                let cx = x + half * col as f32;
                let color = if col == 0 { theme::danger() } else { theme::success() };
                c.clip(cx, y + row_h, half, (h - row_h).max(0.0));
                if !cell.changed.is_empty() { c.rect(cx, ry, half, row_h, theme::mix(theme::bg(), color, 16)); }
                if let Some(n) = cell.number {
                    crate::ui::text(c, &mut fonts, cx + 4.0, ry + px, &n.to_string(), face, px * 0.7, theme::faint());
                }
                let text_x = cx + 46.0 * self.scale - v.horizontal;
                c.clip(cx + 44.0 * self.scale, y + row_h, (half - 44.0 * self.scale).max(0.0), (h - row_h).max(0.0));
                for span in &cell.changed {
                    let offset = fonts.width(face, px, &cell.text[..span.start]);
                    let width = fonts.width(face, px, &cell.text[span.clone()]);
                    c.rect(text_x + offset, ry, width, row_h, theme::mix(theme::bg(), color, 45));
                }
                crate::ui::text(c, &mut fonts, text_x, ry + px, &cell.text, face, px, theme::fg());
            }
        }
        c.clip(x, y, w, h);
        c.line(x + half, y, self.scale, h, theme::line());
        c.unclip();
        true
    }
}
