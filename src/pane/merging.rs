//! The three merge stages drawn above the result they are merged into.
use super::*;
use crate::merge::{Merge, Row};

impl Pane {
    /// Shows a merge above the text, which becomes its result.
    pub(crate) fn merge_open(&mut self, merge: Merge) {
        let full = self.merge.as_ref().map_or((self.x, self.y, self.w, self.h), |m| m.full);
        self.merge = Some(merge);
        self.set_rect(full.0, full.1, full.2, full.3, self.scale);
        self.redraw = true;
    }

    pub(crate) fn merge_close(&mut self) {
        let Some(m) = self.merge.take() else { return };
        self.set_rect(m.full.0, m.full.1, m.full.2, m.full.3, self.scale);
        self.redraw = true;
    }

    fn merge_row(&self) -> f32 { (self.px() * theme::LINE_HEIGHT).round() }

    /// Gives the columns the upper part of the pane's place, and returns
    /// what is left for the text.
    pub(super) fn merge_place(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) -> (f32, f32, f32, f32) {
        let Some(m) = &mut self.merge else { return (x, y, w, h) };
        m.full = (x, y, w, h);
        let ph = (h * 0.45).round().min((h - 80.0 * scale).max(0.0)).max(0.0);
        m.panel = (x, y, w, ph);
        let gap = scale.max(1.0);
        (x, y + ph + gap, w, (h - ph - gap).max(0.0))
    }

    /// The wheel over the columns scrolls them.
    pub(super) fn merge_scroll(&mut self, dy: f32) -> bool {
        let row = self.merge_row();
        let Some(m) = &mut self.merge else { return false };
        if !crate::ui::inside(m.panel, Some(m.pointer)) { return false; }
        let most = (m.rows.len() as f32 * row - (m.panel.3 - row)).max(0.0);
        m.scroll = (m.scroll + dy).clamp(0.0, most);
        self.redraw = true;
        true
    }

    /// The text that conflict `k` is now, if it is still unresolved.
    pub(crate) fn merge_range(&mut self, k: usize) -> Option<std::ops::Range<usize>> {
        let (text, version) = (&self.buf.text, self.buf.version);
        self.merge.as_mut()?.found(text, version).get(k).cloned().flatten()
    }

    /// The conflict at or after the cursor, if any is left.
    pub(crate) fn merge_current(&mut self) -> Option<usize> {
        let (text, version, cur) = (&self.buf.text, self.buf.version, self.buf.cur);
        let found = self.merge.as_mut()?.found(text, version);
        Merge::at_cursor(found, cur)
    }

    /// Puts a choice in place of conflict `k` in the result.
    pub(crate) fn merge_accept(&mut self, k: usize, choice: u8) -> Result<(), String> {
        let text = self.merge.as_ref().and_then(|m| m.choice(k, choice)).ok_or("No merge editor here")?;
        let range = self.merge_range(k).ok_or("This conflict is already resolved; edit the result or undo")?;
        self.edit(range.start, range.end, &text, EditKind::Other);
        let at = range.start.min(self.buf.text.len());
        self.select(at, at);
        self.moved();
        Ok(())
    }

    /// The headings of conflicts shown whole, and their row tops, for the
    /// controls drawn on them.
    pub(crate) fn merge_heads(&self) -> Vec<(usize, f32, f32)> {
        let row = self.merge_row();
        let Some(m) = &self.merge else { return Vec::new() };
        let (_, py, _, ph) = m.panel;
        m.heads.iter().enumerate().filter_map(|(k, &r)| {
            let top = py + row + r as f32 * row - m.scroll;
            (top >= py + row && top + row <= py + ph).then_some((k, top, row))
        }).collect()
    }

    pub(super) fn draw_merge(&mut self, c: &mut Canvas) {
        let row = self.merge_row();
        let px = self.px();
        // The columns follow the result's cursor from conflict to conflict.
        let current = self.merge_current();
        let resolved: Vec<bool> = (0..self.merge.as_ref().map_or(0, |m| m.heads.len()))
            .map(|k| self.merge_range(k).is_none()).collect();
        let scale = self.scale;
        let Some(m) = &mut self.merge else { return };
        m.current = current;
        if current != m.followed {
            m.followed = current;
            if let Some(k) = current {
                let top = m.heads[k] as f32 * row;
                let body = m.panel.3 - row;
                if top < m.scroll || top + row * 3.0 > m.scroll + body {
                    m.scroll = (top - row).max(0.0);
                }
            }
        }
        let (x, y, w, h) = m.panel;
        if w < 1.0 || h < row { return; }
        let third = (w / 3.0).floor();
        let mut fonts = self.fonts.borrow_mut();
        let face = fonts.mono();
        let small = (px * 0.8).round();
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());
        c.rect(x, y, w, row, theme::inset());
        for (col, label) in m.labels.iter().enumerate() {
            let label = crate::ui::fit(&mut fonts, crate::font::UI, small, label, (third - 16.0).max(0.0));
            crate::ui::text(c, &mut fonts, x + third * col as f32 + 8.0, y + px * 0.9, &label, crate::font::UI, small, theme::dim());
        }
        let body = (x, y + row, w, (h - row).max(0.0));
        let tint = [theme::info(), theme::faint(), theme::warning()];
        for (i, r) in m.rows.iter().enumerate().skip((m.scroll / row).floor() as usize) {
            let ry = body.1 + i as f32 * row - m.scroll;
            if ry > y + h { break; }
            c.clip(body.0, body.1, body.2, body.3);
            match r {
                Row::Same(text) => {
                    for col in 0..3 {
                        c.clip(x + third * col as f32, body.1, third - 1.0, body.3);
                        crate::ui::text(c, &mut fonts, x + third * col as f32 + 8.0, ry + px, text, face, px, theme::dim());
                    }
                }
                Row::Fold(n) => {
                    let label = format!("\u{22ef} {n} unchanged lines");
                    crate::ui::text(c, &mut fonts, x + 8.0, ry + px * 0.9, &label, crate::font::UI, small, theme::faint());
                }
                Row::Head(k) => {
                    let on = current == Some(*k);
                    c.rect(x, ry, w, row, if on { theme::mix(theme::bg(), theme::primary(), 28) } else { theme::inset() });
                    let state = if resolved[*k] { "resolved in the result" } else if on { "at the cursor" } else { "unresolved" };
                    let label = format!("Conflict {} of {} \u{b7} {state}", k + 1, m.heads.len());
                    crate::ui::text(c, &mut fonts, x + 8.0, ry + px * 0.9, &label, crate::font::UI, small,
                        if resolved[*k] { theme::faint() } else { theme::fg() });
                }
                Row::Line(k, cells, changed) => {
                    for col in 0..3 {
                        let cx = x + third * col as f32;
                        c.clip(cx, body.1, third - 1.0, body.3);
                        match &cells[col] {
                            None => c.rect(cx, ry, third, row, theme::inset()),
                            Some(_) if changed[col] && !resolved[*k] => c.rect(cx, ry, third, row, theme::mix(theme::bg(), tint[col], 30)),
                            Some(_) => {}
                        }
                        if let Some(text) = &cells[col] {
                            let ink = if resolved[*k] { theme::faint() } else { theme::fg() };
                            crate::ui::text(c, &mut fonts, cx + 8.0, ry + px, text, face, px, ink);
                        }
                    }
                }
            }
        }
        c.clip(x, y, w, h);
        for col in 1..3 {
            c.line(x + third * col as f32 - 1.0, y, scale.max(1.0), h, theme::line());
        }
        c.unclip();
        c.line(x, y + h, w, scale.max(1.0), theme::edge());
    }
}
