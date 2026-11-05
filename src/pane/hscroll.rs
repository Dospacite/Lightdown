//! Unwrapped text and horizontal scrolling. Lines of a pane that does not
//! wrap run past the column's right edge; the text slides under a fixed
//! gutter, with a scrollbar along the pane's bottom.
//!
//! Anything drawn at a place in the text goes through [`Pane::text_screen_x`]
//! or, for text after a line's end, [`Pane::line_end_x`], so that it scrolls
//! with the text. The gutter, line numbers and sticky headers use
//! `column().0` and stay put.
use super::*;

impl Pane {
    /// Whether lines wrap at the column's edge. Notebook cells always do.
    pub(crate) fn wraps(&self) -> bool {
        self.word_wrap || self.doc == Doc::Cells
    }

    /// How far the text is scrolled to the left, in pixels.
    pub(crate) fn hoffset(&self) -> f32 {
        if self.wraps() { 0.0 } else { self.horizontal }
    }

    /// Where a layout x of a line, measured from the text column's left
    /// edge, is on screen, after horizontal scrolling.
    pub(crate) fn text_screen_x(&self, x: f32) -> f32 {
        self.column().0 + x - self.hoffset()
    }

    /// Where text put after the end of a row starts on screen: a gap of a
    /// space's width past its last letter. For blame, review notes and
    /// other decorations at line ends.
    pub(crate) fn line_end_x(&self, lay: &Layout, row: &layout::Row) -> f32 {
        self.text_screen_x(text_end(lay, row)) + self.px()
    }

    /// Scrolls the text sideways by `dx` pixels, within its widest line.
    pub(crate) fn scroll_horizontal(&mut self, dx: f32) {
        if self.wraps() || dx == 0.0 {
            return;
        }
        self.horizontal = (self.horizontal + dx).clamp(0.0, self.horizontal_max());
        self.redraw = true;
    }

    /// The widest line seen, with room for the caret after it.
    fn horizontal_extent(&self) -> f32 {
        self.hwidest + self.px() * 2.0
    }

    fn horizontal_max(&self) -> f32 {
        (self.horizontal_extent() - self.column().1).max(0.0).max(self.horizontal_floor)
    }

    /// Notes the widest of the lines in view, so the scrollbar knows how
    /// far the text goes. Only lines laid out for drawing are measured.
    pub(super) fn measure_widths(&mut self) {
        if self.wraps() {
            return;
        }
        for &(li, _) in &self.visible {
            if let Some(Some(lay)) = self.cache.get(li) {
                let w = lay.clusters.last().map_or(0.0, |k| k.x + k.w);
                self.hwidest = self.hwidest.max(w);
            }
        }
        self.horizontal_floor = 0.0;
        self.horizontal = self.horizontal.clamp(0.0, self.horizontal_max());
    }

    /// Scrolls so the caret, at layout x `x`, is in view with a margin.
    pub(super) fn reveal_across(&mut self, x: f32) {
        if self.wraps() {
            return;
        }
        let width = self.column().1;
        let margin = (self.px() * 2.0).min(width / 4.0);
        if x < self.horizontal + margin {
            self.horizontal = (x - margin).max(0.0);
        } else if x > self.horizontal + width - margin {
            self.horizontal = x - width + margin;
        }
        // The caret's line may be wider than any measured yet.
        self.horizontal_floor = self.horizontal;
        self.hwidest = self.hwidest.max(x);
        self.redraw = true;
    }

    /// Text drawn while scrolled is kept off the gutter.
    pub(super) fn clip_text(&self, c: &mut Canvas) {
        if self.hoffset() > 0.0 {
            let left = self.column().0 - (self.px() * 0.15).round();
            c.clip(left, self.y, self.x + self.w - self.minimap_width() - left, self.h);
        }
    }

    /// The whole pane again, for the gutter.
    pub(super) fn clip_pane(&self, c: &mut Canvas) {
        if self.hoffset() > 0.0 {
            c.clip(self.x, self.y, self.w, self.h);
        }
    }

    /// The horizontal scrollbar's thumb: its left edge and length, when the
    /// text is wider than the column.
    fn horizontal_bar(&self) -> Option<(f32, f32)> {
        if self.wraps() {
            return None;
        }
        let width = self.column().1;
        let extent = self.horizontal_extent().max(width + self.horizontal);
        if extent <= width + 0.5 {
            return None;
        }
        let len = (width * width / extent).max(24.0 * self.scale).min(width);
        let travel = (extent - width).max(1.0);
        Some((self.column().0 + (width - len) * (self.horizontal / travel).min(1.0), len))
    }

    fn horizontal_bar_band(&self) -> f32 {
        (12.0 * self.scale).round()
    }

    /// Whether a point is on the horizontal scrollbar's band.
    pub(crate) fn on_horizontal_bar(&self, x: f32, y: f32) -> bool {
        self.horizontal_bar().is_some()
            && self.contains(x, y)
            && y >= self.y + self.h - self.horizontal_bar_band()
            && x >= self.column().0
    }

    pub(super) fn draw_horizontal_bar(&self, c: &mut Canvas) {
        let Some((x, len)) = self.horizontal_bar() else { return };
        let (h, ink) = match self.hbar_hot {
            true => ((8.0 * self.scale).round(), theme::mix(theme::bg(), theme::fg(), 70)),
            false => ((6.0 * self.scale).round(), theme::mix(theme::bg(), theme::fg(), 34)),
        };
        let inset = (3.0 * self.scale).round();
        c.round_rect(x, self.y + self.h - h - inset, len, h, h / 2.0, ink);
    }

    /// A press on the bar takes the thumb, or brings it to the pointer.
    pub(super) fn horizontal_bar_press(&mut self, x: f32, y: f32) -> bool {
        if !self.on_horizontal_bar(x, y) {
            return false;
        }
        let Some((left, len)) = self.horizontal_bar() else { return false };
        let grab = if x >= left && x <= left + len { x - left } else { len / 2.0 };
        self.drag = Drag::Horizontal(grab);
        self.drag_horizontal(x, grab);
        true
    }

    pub(super) fn drag_horizontal(&mut self, x: f32, grab: f32) {
        let Some((_, len)) = self.horizontal_bar() else { return };
        let width = self.column().1;
        let ratio = ((x - self.column().0 - grab) / (width - len).max(1.0)).clamp(0.0, 1.0);
        self.horizontal = ratio * self.horizontal_max();
        self.horizontal_floor = 0.0;
        self.redraw = true;
    }

    /// Lights the bar under the pointer. Returns whether that changed.
    pub(crate) fn hover_horizontal_bar(&mut self, x: f32, y: f32) -> bool {
        let hot = self.on_horizontal_bar(x, y) || matches!(self.drag, Drag::Horizontal(_));
        let changed = hot != self.hbar_hot;
        self.hbar_hot = hot;
        changed
    }

    /// While a selection is dragged past the column's left or right edge,
    /// the text slides that way.
    pub(super) fn drag_across_edges(&mut self, x: f32) {
        if self.wraps() {
            return;
        }
        let (left, width) = self.column();
        if x < left {
            self.scroll_horizontal(x - left);
        } else if x > left + width {
            self.scroll_horizontal(x - left - width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(text: &str) -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.numbers = true;
        p.word_wrap = false;
        p.set_rect(0.0, 0.0, 400.0, 300.0, 1.0);
        p.set_text(text.into(), Doc::Code);
        p
    }

    fn frame(p: &mut Pane) {
        let mut pixels = vec![0u32; 400 * 300];
        let mut c = Canvas::new(&mut pixels, 400, 300);
        p.draw(&mut c);
    }

    #[test]
    fn unwrapped_lines_stay_on_one_row_and_scroll_within_their_width() {
        let long = "word ".repeat(80);
        let mut p = pane(&format!("short\n{long}\n"));
        assert_eq!(p.layout(1).rows.len(), 1, "an unwrapped line is one row");
        frame(&mut p);
        let (_, width) = p.column();
        p.scroll_horizontal(-50.0);
        assert_eq!(p.horizontal, 0.0);
        p.scroll_horizontal(1.0e6);
        let widest = p.layout(1).clusters.last().map(|k| k.x + k.w).unwrap();
        assert!((p.horizontal - (widest + p.px() * 2.0 - width)).abs() < 1.0, "stops at the widest line");
        assert!(p.on_horizontal_bar(p.column().0 + 10.0, 296.0));
        p.word_wrap = true;
        assert!(p.layout(1).rows.len() > 1, "wrapping again breaks it into rows");
        assert_eq!(p.text_screen_x(3.0), p.column().0 + 3.0, "and the offset no longer applies");
        assert!(!p.on_horizontal_bar(p.column().0 + 10.0, 296.0));
    }

    #[test]
    fn the_caret_is_kept_in_view_and_the_gutter_does_not_move() {
        let long = "x".repeat(400);
        let mut p = pane(&format!("{long}\n"));
        frame(&mut p);
        let gutter = p.column().0;
        p.select(390, 390);
        p.moved();
        let x = p.caret().unwrap().0;
        let (left, width) = p.column();
        assert!(p.horizontal > 0.0);
        assert!(x >= left && x <= left + width, "caret at {x} in {left}..{}", left + width);
        assert_eq!(p.column().0, gutter);
        // A click lands on the letter under it, scrolled.
        let top = p.caret().unwrap().1;
        let off = p.offset_at(x, top + 2.0);
        assert!((off as i64 - 390).abs() <= 1, "clicked offset {off}");
        p.select(0, 0);
        p.moved();
        assert_eq!(p.horizontal, 0.0);
    }

    #[test]
    fn line_end_decorations_follow_the_scrolled_text() {
        let mut p = pane("abc\n");
        frame(&mut p);
        let lay = p.layout(0);
        let row = &lay.rows[0];
        let at = p.line_end_x(&lay, row);
        p.horizontal = 30.0;
        assert_eq!(p.line_end_x(&lay, row), at - 30.0);
    }

    #[test]
    fn notebooks_always_wrap() {
        let mut p = pane("x");
        p.set_text(format!("{}\n{}", crate::cells::header(Tag::Code, ""), "y ".repeat(300)), Doc::Cells);
        assert!(p.wraps());
        p.scroll_horizontal(100.0);
        assert_eq!(p.hoffset(), 0.0);
    }
}
