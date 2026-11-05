//! A pane showing a multibuffer: excerpts of files under headers. Edits stay
//! inside one excerpt's lines, Undo goes to the files' buffers, and the
//! gutter shows each file's own line numbers.
use super::*;

/// Between the controls at the right of an excerpt's header.
pub(crate) const GAP: &str = "    ";

impl Pane {
    /// Whether an edit of a multibuffer stays within one excerpt's lines,
    /// leaving every header and the line break before it alone.
    pub(super) fn excerpts_allow(&mut self, start: usize, end: usize, ins: &str) -> bool {
        let fits = !ins.contains(cells::MARK) && !self.cells.is_empty() && {
            let i = self.cell_at(start);
            let (a, b) = self.cell_body(i);
            self.cells[i].headed && start >= a && end <= b && self.multi.as_ref().is_some_and(|m| i < m.excerpts.len())
        };
        if !fits {
            self.note = Some("Edits in a multibuffer stay within one excerpt".into());
        }
        fits
    }

    /// Asks the owner to undo or redo the multibuffer's last step in the
    /// files, rather than in the pane's own text.
    pub(crate) fn multi_undo(&mut self, redo: bool) -> bool {
        let Some(m) = &mut self.multi else { return false };
        m.request = Some(redo);
        self.redraw = true;
        true
    }

    /// The file's own number for a line of an excerpt.
    pub(super) fn excerpt_number(&self, li: usize) -> Option<usize> {
        let m = self.multi.as_ref()?;
        let ci = self.cells.partition_point(|c| c.line <= li).checked_sub(1)?;
        let cell = &self.cells[ci];
        (cell.headed && li > cell.line).then_some(())?;
        Some(m.excerpts.get(ci)?.start + (li - cell.line - 1) + 1)
    }

    /// The language an excerpt is highlighted as: its file's extension.
    pub(super) fn excerpt_language(&self, cell: usize) -> Option<String> {
        let x = self.multi.as_ref()?.excerpts.get(cell)?;
        let ext = x.path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let name = x.path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        Some(if ext.is_empty() { name.to_ascii_lowercase() } else { ext.to_ascii_lowercase() })
    }

    /// The excerpt the cursor is in, and the line of its body.
    pub(crate) fn excerpt_at(&self, off: usize) -> Option<(usize, usize)> {
        let m = self.multi.as_ref()?;
        if self.cells.len() != m.excerpts.len() {
            return None;
        }
        let i = self.cell_at(off);
        let first = self.cells[i].line + 1;
        Some((i, self.buf.line_of(off).saturating_sub(first)))
    }

    /// The offset of a line of an excerpt's body and a byte column in it.
    pub(crate) fn excerpt_offset(&self, cell: usize, line: usize, col: usize) -> Option<usize> {
        let first = self.cells.get(cell)?.line + 1;
        let end = self.cells.get(cell + 1).map_or(self.buf.lines(), |c| c.line);
        let li = (first + line).min(end.saturating_sub(1)).max(first.min(self.buf.lines() - 1));
        let (a, b) = self.buf.line(li);
        Some(editor::floor(&self.buf.text, (a + col).min(b)))
    }

    /// An excerpt header under a point, and which of its controls, counted
    /// from the left, if one; None for the rest of the header.
    pub(crate) fn header_hit(&mut self, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
        let m = self.multi.as_ref()?;
        if !self.contains(x, y) || self.on_bar(x, y) {
            return None;
        }
        let &(li, top) = self.visible.iter().rev().find(|(_, top)| *top <= y)?;
        let ci = self.cells.binary_search_by_key(&li, |c| c.line).ok()?;
        if ci >= m.excerpts.len() {
            return None;
        }
        let controls = m.controls(ci);
        let lay = self.layout(li);
        if y >= top + lay.h {
            return None;
        }
        let px = self.px();
        let size = (px * 0.78).round().max(px.min(theme::SMALL));
        let mut fonts = self.fonts.borrow_mut();
        let full = controls.join(GAP);
        let right = self.text_screen_x(self.column().1);
        let mut left = right - fonts.width(font::UI_ITALIC, size, &full);
        if x < left {
            return Some((ci, None));
        }
        let gap = fonts.width(font::UI_ITALIC, size, GAP);
        for (k, label) in controls.iter().enumerate() {
            let w = fonts.width(font::UI_ITALIC, size, label);
            if x < left + w + gap / 2.0 {
                return Some((ci, Some(k)));
            }
            left += w + gap;
        }
        Some((ci, Some(controls.len() - 1)))
    }

    /// Undoes or redoes the last edit of this pane's own buffer, as for a
    /// multibuffer's step. Returns where the edit was.
    pub(crate) fn history_step(&mut self, redo: bool) -> Option<usize> {
        let c = if redo { self.buf.redo()? } else { self.buf.undo()? };
        let at = c.pos;
        self.changed(c);
        self.redraw = true;
        Some(at)
    }

    /// Marks the lines a reviewed change added, in the gutter.
    pub(super) fn excerpts_draw(&mut self, c: &mut Canvas) {
        let Some(m) = &self.multi else { return };
        if m.added.is_empty() || !self.numbers {
            return;
        }
        let added = m.added.clone();
        let x = self.column().0 - self.marks_gutter() + self.scale;
        c.clip(self.x, self.y, self.w, self.h);
        for (li, y) in self.visible.clone() {
            if added.binary_search(&li).is_ok() {
                let h = self.layout(li).h.max(3.0 * self.scale);
                c.rect(x, y, 3.0 * self.scale, h, theme::success());
            }
        }
        c.unclip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multibuffer::{Kind, Multi, Source};

    fn pane() -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        let mut m = Multi::new(Kind::Search, "t", Path::new("/p"));
        let text = (0..30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        m.sources.push(Source {
            path: "/p/a.rs".into(),
            text: text.clone(),
            version: 0,
            hidden: None,
            touched: false,
            disk: 0,
        });
        m.add_file(Path::new("/p/a.rs"), &text, &[3, 20], 1);
        let shown = m.render();
        p.multi = Some(Box::new(m));
        p.numbers = true;
        p.fill = true;
        p.set_text(shown, Doc::Cells);
        p
    }

    #[test]
    fn edits_stay_in_one_excerpt_and_numbers_are_the_files() {
        let mut p = pane();
        assert_eq!(p.cells.len(), 2);
        let numbers: Vec<_> = (0..p.buf.lines()).map(|li| p.source_number(li)).collect();
        assert_eq!(numbers, vec![None, Some(3), Some(4), Some(5), None, Some(20), Some(21), Some(22)]);
        let (a, _) = p.cell_body(0);
        assert!(p.edit(a, a, "x", EditKind::Type));
        // Across the header, or taking the line break before it, is refused.
        let b = p.cell_body(0).1;
        assert!(!p.edit(b, b + 3, "", EditKind::Delete));
        assert!(!p.edit(b, b + 1, "", EditKind::Delete));
        assert!(p.note.take().unwrap().contains("one excerpt"));
        assert!(!p.edit(a, a, "\u{1e}code", EditKind::Type));
        assert_eq!(p.excerpt_at(p.cell_body(1).0 + 2), Some((1, 0)));
        assert_eq!(p.excerpt_offset(1, 1, 2), Some(p.buf.starts[6] + 2));
    }

    #[test]
    fn undo_keys_go_to_the_owner() {
        let mut p = pane();
        let (a, _) = p.cell_body(0);
        p.select(a, a);
        p.key(Key::Char('q'), Mods::default(), Some("q"));
        let text = p.buf.text.clone();
        p.key(Key::Char('z'), Mods { ctrl: true, ..Default::default() }, None);
        assert_eq!(p.buf.text, text);
        assert_eq!(p.multi.as_ref().unwrap().request, Some(false));
        p.key(Key::Char('z'), Mods { ctrl: true, shift: true, ..Default::default() }, None);
        assert_eq!(p.multi.as_ref().unwrap().request, Some(true));
    }

    #[test]
    fn headers_show_the_file_and_their_controls_take_clicks() {
        let mut p = pane();
        p.set_rect(0.0, 0.0, 900.0, 600.0, 1.0);
        let mut pixels = vec![0; 900 * 600];
        p.draw(&mut Canvas::new(&mut pixels, 900, 600));
        let (li, y) = p.visible[0];
        assert_eq!(li, 0);
        let h = p.layout(0).h;
        let right = p.text_screen_x(p.column().1);
        assert_eq!(p.header_hit(p.column().0 + 4.0, y + h - 4.0), Some((0, None)));
        assert_eq!(p.header_hit(right - 2.0, y + h - 4.0), Some((0, Some(1))));
        assert_eq!(p.excerpt_language(0).as_deref(), Some("rs"));
    }
}
