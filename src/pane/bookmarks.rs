//! Line bookmarks: which lines are marked, how the marks follow their
//! lines through edits, and the mark in the gutter.
use super::*;

/// Moves bookmarked line numbers through an edit. `at_start` says the edit
/// began at the start of its first line. Lines the edit deletes lose their
/// mark; a mark on a line that new lines were put before moves down with
/// it; a block of whole lines replaced by others keeps its marks by line
/// where it can, so a reformatted or reindented file keeps them.
pub(super) fn track(lines: &mut Vec<usize>, c: &Change, at_start: bool) -> bool {
    let before = lines.clone();
    let (l0, r, a) = (c.line, c.removed, c.added);
    lines.retain_mut(|line| {
        let l = *line;
        if l < l0 {
            return true;
        }
        if l > l0 + r {
            *line = l + a - r;
            return true;
        }
        if r == 0 {
            // Text put into one line: it moves down when the text went in
            // before all of it.
            if at_start && a > 0 {
                *line = l0 + a;
            }
            return true;
        }
        if a > 0 {
            // Lines replaced by lines.
            *line = l0 + (l - l0).min(a);
            return true;
        }
        // Text taken away across lines.
        if l == l0 {
            !(at_start && c.ends_line)
        } else if l == l0 + r && c.ends_line {
            *line = l0;
            true
        } else {
            false
        }
    });
    lines.sort_unstable();
    lines.dedup();
    *lines != before
}

impl Pane {
    pub(crate) fn bookmark_toggle(&mut self) {
        let line = self.buf.line_of(self.buf.cur);
        match self.bookmarks.binary_search(&line) {
            Ok(i) => {
                self.bookmarks.remove(i);
            }
            Err(i) => self.bookmarks.insert(i, line),
        }
        self.bookmarks_changed = true;
        self.redraw = true;
    }

    pub(crate) fn bookmarks_clear(&mut self) -> usize {
        let n = self.bookmarks.len();
        self.bookmarks.clear();
        self.bookmarks_changed |= n > 0;
        self.redraw = true;
        n
    }

    /// Takes marks read back for this text, dropping those past its end.
    pub(crate) fn keep_bookmarks(&mut self, mut lines: Vec<usize>) {
        let n = self.buf.lines();
        lines.retain(|&line| line < n);
        lines.sort_unstable();
        lines.dedup();
        self.bookmarks = lines;
    }

    /// The next bookmarked line after the cursor's, or before it, wrapping
    /// round. None when the file has none.
    pub(crate) fn bookmark_after(&self, back: bool) -> Option<usize> {
        let line = self.buf.line_of(self.buf.cur);
        let marks = &self.bookmarks;
        if back {
            marks.iter().rev().find(|&&n| n < line).or(marks.last()).copied()
        } else {
            marks.iter().find(|&&n| n > line).or(marks.first()).copied()
        }
    }

    pub(crate) fn bookmark_step(&mut self, back: bool) -> bool {
        let Some(n) = self.bookmark_after(back) else { return false };
        let at = self.buf.starts[n.min(self.buf.lines() - 1)];
        self.select(at, at);
        self.moved();
        true
    }

    /// The slot the mark takes, in the margin just left of the gutter, so
    /// it never covers the change bar, breakpoints or numbers.
    pub(super) fn bookmark_slot(&self) -> f32 {
        (6.0 * self.scale).round()
    }

    /// The mark for a bookmarked line: a short bar in its own slot left of
    /// the gutter, whose left edge is `gutter_x`.
    pub(super) fn draw_bookmark(&self, c: &mut Canvas, line: usize, gutter_x: f32, y: f32, lay: &Layout) {
        if !self.numbers || self.bookmarks.binary_search(&line).is_err() {
            return;
        }
        let px = self.px();
        let h = lay.rows.first().map_or(px, |r| r.h);
        let top = lay.rows.first().map_or(0.0, |r| r.y);
        let (w, mh) = ((3.0 * self.scale).round().max(2.0), (h * 0.6).round());
        let x = gutter_x - self.bookmark_slot();
        c.round_rect(x, y + top + ((h - mh) / 2.0).round(), w, mh, w / 2.0, theme::info());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(line: usize, removed: usize, added: usize, ends_line: bool) -> Change {
        Change { line, removed, added, pos: 0, del: 0, ends_line }
    }

    #[test]
    fn deleted_lines_drop_their_marks_and_later_lines_move_up() {
        let mut marks = vec![0, 2, 3, 5];
        // Lines 2 and 3 deleted whole.
        assert!(track(&mut marks, &change(2, 2, 0, true), true));
        assert_eq!(marks, vec![0, 3]);
        // A selection from the middle of line 0 into line 2 merges them.
        let mut marks = vec![0, 1, 2, 4];
        track(&mut marks, &change(0, 2, 0, false), false);
        assert_eq!(marks, vec![0, 2]);
    }

    #[test]
    fn marks_follow_their_line_when_lines_go_in_above_or_it_is_joined() {
        let mut marks = vec![1, 3];
        // Enter at the start of line 1 pushes its text down.
        track(&mut marks, &change(1, 0, 1, false), true);
        assert_eq!(marks, vec![2, 4]);
        // Enter in the middle of line 2 leaves the mark on its first half.
        track(&mut marks, &change(2, 0, 1, false), false);
        assert_eq!(marks, vec![2, 5]);
        // Backspace at the start of line 5 joins it to line 4.
        track(&mut marks, &change(4, 1, 0, true), false);
        assert_eq!(marks, vec![2, 4]);
        // Undoing a deleted line puts the mark back where it was.
        let mut marks = vec![2];
        track(&mut marks, &change(2, 0, 1, false), true);
        assert_eq!(marks, vec![3]);
    }

    #[test]
    fn a_whole_file_rewrite_keeps_marks_by_line() {
        let mut marks = vec![0, 4, 9];
        assert!(!track(&mut marks, &change(0, 10, 10, false), true));
        assert_eq!(marks, vec![0, 4, 9]);
        track(&mut marks, &change(0, 10, 6, true), true);
        assert_eq!(marks, vec![0, 4, 6]);
    }

    #[test]
    fn toggling_and_stepping_wrap_round_the_file() {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text("a\nb\nc\nd\n".into(), Doc::Code);
        assert!(!p.bookmark_step(false));
        p.bookmark_toggle();
        p.select(p.buf.starts[2], p.buf.starts[2]);
        p.bookmark_toggle();
        assert_eq!(p.bookmarks, vec![0, 2]);
        assert!(p.bookmark_step(false));
        assert_eq!(p.buf.line_of(p.buf.cur), 0);
        assert!(p.bookmark_step(true));
        assert_eq!(p.buf.line_of(p.buf.cur), 2);
        p.select(p.buf.starts[1], p.buf.starts[2]);
        p.insert("", EditKind::Other);
        assert_eq!(p.bookmarks, vec![0, 1], "a deleted line takes its mark, later marks move up");
        p.keep_bookmarks(vec![7, 1, 1, 3]);
        assert_eq!(p.bookmarks, vec![1, 3]);
        assert_eq!(p.bookmarks_clear(), 2);
    }
}
