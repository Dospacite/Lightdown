//! A bounded, versioned overview. Each raster row samples a source line;
//! work and storage depend on the pane's size, never the document's size.
use super::*;

#[derive(Default)]
pub(super) struct Overview {
    key: (u64, usize),
    rows: Vec<Vec<(u8, u8, crate::md::Attr)>>,
    next: usize,
}

impl Pane {
    pub(super) fn minimap_width(&self) -> f32 {
        let source = matches!(self.doc, Doc::Code | Doc::Python | Doc::Latex | Doc::Text)
            || (self.doc == Doc::Markdown && self.edit_mode);
        let longer = self.buf.lines() as f32 * self.px() * theme::LINE_HEIGHT > self.h;
        if !source
            || !self.numbers
            || self.chat
            || self.comparison.is_some()
            || self.merge.is_some()
            || self.minimap == "off"
            || (self.minimap != "on" && !longer)
            || self.w < 280.0 * self.scale
        {
            return 0.0;
        }
        (self.minimap_size.clamp(32.0, 160.0) * self.scale).round().min(self.w / 4.0)
    }

    pub(crate) fn on_minimap(&self, x: f32, y: f32) -> bool {
        self.minimap_width() > 0.0 && self.contains(x, y) && x >= self.x + self.w - self.minimap_width()
    }

    fn minimap_box(&self) -> (f32, f32) {
        let n = self.buf.lines().max(1) as f32;
        let shown = self.visible.last().map_or(1, |(li, _)| li + 1 - self.top).max(1) as f32;
        let height = (self.h * shown / n).max(12.0 * self.scale).min(self.h);
        let y = (self.h * self.top as f32 / n).min(self.h - height);
        (self.y + y, height)
    }

    pub(super) fn minimap_press(&mut self, x: f32, y: f32) -> bool {
        if !self.on_minimap(x, y) {
            return false;
        }
        let (top, height) = self.minimap_box();
        let grab = if y >= top && y <= top + height { y - top } else { height / 2.0 };
        self.drag = Drag::Minimap(grab);
        self.minimap_drag(y, grab);
        true
    }

    pub(super) fn minimap_drag(&mut self, y: f32, grab: f32) {
        let ratio = ((y - self.y - grab) / self.h.max(1.0)).clamp(0.0, 1.0);
        self.top = ((ratio * self.buf.lines() as f32) as usize).min(self.buf.lines() - 1);
        self.top_off = 0.0;
        self.clamp_scroll();
        self.at_tail = false;
        self.redraw = true;
    }

    pub(super) fn draw_minimap(&mut self, c: &mut Canvas) {
        let width = self.minimap_width();
        if width == 0.0 {
            return;
        }
        self.bar = None;
        let x = self.x + self.w - width;
        c.clip(x, self.y, width, self.h);
        c.rect(x, self.y, width, self.h, theme::bg());
        c.rect(x, self.y, self.scale.max(1.0), self.h, theme::line());
        // Keep the sample grid independent of view geometry. A mirror can
        // draw this pane at another height without invalidating its cache.
        let rows = 512;
        let key = (self.buf.version, rows);
        if self.overview.key != key {
            self.overview = Overview { key, rows: vec![Vec::new(); rows], next: 0 };
        }
        // At most 64 rows of at most 120 bytes per frame, then reuse them.
        let end = (self.overview.next + 64).min(rows);
        for row in self.overview.next..end {
            let li = row * self.buf.lines() / rows;
            let (start, stop) = self.buf.line(li);
            let bytes = &self.buf.text.as_bytes()[start..stop];
            let mut blocks = Vec::new();
            let mut col = 0;
            let mut offset = 0;
            while offset < bytes.len().min(120) && col < 120 {
                let b = bytes[offset];
                if b.is_ascii_whitespace() {
                    col += if b == b'\t' { 4 } else { 1 };
                    offset += 1;
                    continue;
                }
                let at = offset;
                while offset < bytes.len().min(120) && !bytes[offset].is_ascii_whitespace() {
                    offset += 1;
                }
                let attr = self
                    .md
                    .as_ref()
                    .and_then(|md| {
                        let i = md.spans.partition_point(|s| s.start as usize <= start + at);
                        md.spans.get(i.saturating_sub(1)).filter(|s| s.end as usize > start + at).map(|s| s.attr)
                    })
                    .unwrap_or(0);
                let len = (offset - at).min(120 - col);
                blocks.push((col as u8, len as u8, attr));
                col += len;
            }
            self.overview.rows[row] = blocks;
        }
        self.overview.next = end;
        if end < rows {
            self.redraw = true;
        }
        let unit = (width - 10.0 * self.scale).max(1.0) / 120.0;
        let dy = self.h / rows as f32;
        for (row, blocks) in self.overview.rows.iter().enumerate() {
            for &(col, len, attr) in blocks {
                let ink = if attr & md::KEYWORD != 0 {
                    theme::keyword()
                } else if attr & md::SOFT != 0 {
                    theme::string()
                } else if attr & md::NUMBER != 0 {
                    theme::number()
                } else if attr & md::FUNCTION != 0 {
                    theme::function()
                } else if attr & md::SPECIAL != 0 {
                    theme::special()
                } else {
                    theme::dim()
                };
                c.rect(
                    x + 3.0 * self.scale + col as f32 * unit,
                    self.y + row as f32 * dy,
                    (len as f32 * unit).max(1.0),
                    dy.min(self.scale.max(1.0)),
                    ink,
                );
            }
        }
        let (top, height) = self.minimap_box();
        c.shade(x + self.scale, top, width - self.scale, height, theme::primary(), 38);
        c.rect(x + self.scale, top, self.scale.max(1.0), height, theme::primary());
        // Separate edge lanes keep the kinds of marker distinguishable.
        let mark = |c: &mut Canvas, li: usize, lane: f32, ink| {
            let y = self.y + self.h * li as f32 / self.buf.lines().max(1) as f32;
            c.rect(
                x + width - lane * self.scale,
                y.min(self.y + self.h - 2.0),
                2.0 * self.scale,
                2.0 * self.scale,
                ink,
            );
        };
        // Bound dense marker lists too. Sampling represents the whole list.
        for &(li, ref kind) in self.git_marks.iter().step_by((self.git_marks.len() / 2048).max(1)) {
            use crate::app::repository::Mark;
            mark(
                c,
                li,
                7.0,
                match kind {
                    Mark::Added => theme::success(),
                    Mark::Modified => theme::warning(),
                    Mark::Deleted => theme::danger(),
                },
            );
        }
        for &(at, _, _) in self.diagnostics.iter().step_by((self.diagnostics.len() / 2048).max(1)) {
            mark(c, self.buf.line_of(at), 5.0, theme::danger());
        }
        for &(at, _) in self.found.iter().step_by((self.found.len() / 2048).max(1)) {
            mark(c, self.buf.line_of(at), 3.0, theme::warning());
        }
        mark(c, self.buf.line_of(self.buf.cur), 9.0, theme::caret());
        c.clip(self.x, self.y, self.w, self.h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pane(n: usize) -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.numbers = true;
        p.set_rect(0.0, 0.0, 600.0, 300.0, 1.0);
        p.set_text("let value = 42;\n".repeat(n), Doc::Code);
        p
    }
    #[test]
    fn auto_source_modes_and_narrow_views() {
        let mut p = pane(2);
        assert_eq!(p.minimap_width(), 0.0);
        p.minimap = "on".into();
        assert!(p.minimap_width() > 0.0);
        p.doc = Doc::Markdown;
        assert_eq!(p.minimap_width(), 0.0);
        p.edit_mode = true;
        assert!(p.minimap_width() > 0.0);
        p.set_rect(0.0, 0.0, 220.0, 300.0, 1.0);
        assert_eq!(p.minimap_width(), 0.0);
    }
    #[test]
    fn cache_is_bounded_versioned_and_click_drag_keeps_cursor() {
        let mut p = pane(100_000);
        let mut pixels = vec![0; 600 * 300];
        for _ in 0..8 {
            p.draw(&mut Canvas::new(&mut pixels, 600, 300));
        }
        assert_eq!(p.overview.rows.len(), 512);
        assert_eq!(p.overview.next, 512);
        p.set_rect(0.0, 0.0, 600.0, 200.0, 1.0);
        p.draw(&mut Canvas::new(&mut pixels, 600, 300));
        assert_eq!(p.overview.next, 512, "a second view's height reuses the cache");
        p.set_rect(0.0, 0.0, 600.0, 300.0, 1.0);
        let cur = p.buf.cur;
        assert!(p.minimap_press(590.0, 150.0));
        assert!(p.top > 40_000 && p.top < 60_000);
        p.minimap_drag(290.0, 0.0);
        assert!(p.top > 90_000);
        p.release();
        assert_eq!(p.buf.cur, cur);
        p.buf = Buffer::new("changed\n".repeat(100));
        p.reparse();
        p.draw(&mut Canvas::new(&mut pixels, 600, 300));
        assert_eq!(p.overview.key.0, p.buf.version);
        assert_eq!(p.overview.next, 64);
    }
}
