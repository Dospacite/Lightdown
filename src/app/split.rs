//! Editor views in nested rows and columns, each showing a tab or the
//! second view of one. All views share the one tab strip: the focused view
//! shows the active tab. Views know tabs by id only, whatever they hold.
use super::*;
use crate::split_tree::{self, Dir, Tree};

/// What a view shows: a tab's first view, or its second.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Leaf {
    Tab(u64),
    Second(u64),
}

impl Leaf {
    fn id(self) -> u64 {
        match self {
            Leaf::Tab(id) | Leaf::Second(id) => id,
        }
    }
}

#[derive(Default)]
pub(super) struct Views {
    /// The views, when there are two or more.
    pub(super) tree: Option<Tree<Leaf>>,
    /// The view that had the keyboard when last arranged.
    focus: Option<Leaf>,
    /// Whether the focused view fills the editor alone.
    pub(super) zoomed: bool,
    /// The divider being dragged: its split, its index and whether views
    /// meet above and below it.
    divider: Option<(Vec<usize>, usize, bool)>,
    /// When a tab is pressed: the tab shown before, and whether the pressed
    /// tab was in a view already.
    drag_from: Option<(u64, bool)>,
}

const DIRS: [Dir; 4] = [Dir::Left, Dir::Up, Dir::Right, Dir::Down];

impl App {
    fn view_gap(&self) -> f32 {
        self.scale.max(1.0)
    }

    /// The room for editor views: below the tabs, above the terminal.
    fn editor_area(&self) -> Rect {
        let (left, width) = self.document_area();
        let y = self.title_h() + self.head_h();
        (left, y, width, (self.body_bottom() - self.term_height() - y).max(0.0))
    }

    /// The view where the active tab's live view is drawn.
    fn live_leaf(&self) -> Leaf {
        let id = self.file_tabs[self.active_file].id;
        if self.split_mirrors.iter().any(|&(m, flipped)| m == id && flipped) { Leaf::Second(id) } else { Leaf::Tab(id) }
    }

    /// Every view and its place, zoomed or not.
    fn all_view_rects(&self) -> Vec<(Leaf, Rect)> {
        match &self.views.tree {
            Some(tree) => tree.layout(self.editor_area(), self.view_gap()).0,
            None => vec![(self.live_leaf(), self.editor_area())],
        }
    }

    /// The views shown and their places: with a zoomed view, that one alone.
    pub(super) fn view_rects(&self) -> Vec<(Leaf, Rect)> {
        if self.views.zoomed { vec![(self.live_leaf(), self.editor_area())] } else { self.all_view_rects() }
    }

    fn view_dividers(&self) -> Vec<split_tree::Divider> {
        match &self.views.tree {
            Some(tree) if !self.views.zoomed => tree.layout(self.editor_area(), self.view_gap()).1,
            _ => Vec::new(),
        }
    }

    fn rect_of(&self, leaf: Leaf) -> Option<Rect> {
        self.view_rects().into_iter().find(|(l, _)| *l == leaf).map(|(_, r)| r)
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.file_tabs.iter().position(|t| t.id == id)
    }

    /// The tabs shown in views of their own, other than the active one.
    pub(super) fn split_others(&self) -> Vec<usize> {
        let tabs = self.view_rects().into_iter().filter_map(|(l, _)| matches!(l, Leaf::Tab(_)).then_some(l.id()));
        tabs.filter_map(|id| self.tab_index(id)).filter(|i| *i != self.active_file).collect()
    }

    /// The tab of the next view after the focused one that shows another tab.
    pub(super) fn split_other(&self) -> Option<usize> {
        let rects = self.view_rects();
        let at = rects.iter().position(|(l, _)| *l == self.live_leaf()).unwrap_or(0);
        (1..rects.len()).map(|n| rects[(at + n) % rects.len()].0).find_map(|l| match l {
            Leaf::Tab(id) => self.tab_index(id).filter(|i| *i != self.active_file),
            Leaf::Second(_) => None,
        })
    }

    /// Whether a tab holds text that a second view can show.
    fn tab_is_text(&self, id: u64) -> bool {
        match self.tab_index(id) {
            Some(i) if i == self.active_file => self.reader.is_none() && self.nb.is_none(),
            Some(i) => self.file_tabs[i].state.as_ref().is_some_and(|s| s.reader.is_none() && s.notebook.is_none()),
            None => false,
        }
    }

    pub(super) fn split_editor(&mut self) {
        if self.views.tree.is_some() || !self.split_mirrors.is_empty() {
            self.mirrors_close();
            self.views.tree = None;
            self.views.zoomed = false;
            self.arrange();
            return;
        }
        self.split_add(false);
    }

    /// Adds a view of another open tab beside the focused view, or, with
    /// one view of the only tab, a second view of it.
    pub(super) fn split_add(&mut self, horizontal: bool) {
        let dir = if horizontal { Dir::Down } else { Dir::Right };
        match self.unshown_tab() {
            Some(id) => self.view_add(Leaf::Tab(id), dir),
            None if self.views.tree.is_none() => self.mirror_toggle(horizontal),
            None => self.message = Some("Every open tab is already in a view".into()),
        }
    }

    /// The next open tab after the active one that no view shows.
    fn unshown_tab(&self) -> Option<u64> {
        let n = self.file_tabs.len();
        let shown = self.views.tree.as_ref().map(Tree::leaves).unwrap_or_else(|| vec![self.live_leaf()]);
        (1..n).map(|k| self.file_tabs[(self.active_file + k) % n].id).find(|id| !shown.contains(&Leaf::Tab(*id)))
    }

    /// Puts a new view beside the focused one.
    fn view_add(&mut self, leaf: Leaf, dir: Dir) {
        let live = self.live_leaf();
        self.views.tree.get_or_insert_with(|| Tree::new(live)).split(live, leaf, dir);
        self.views.zoomed = false;
        self.pin_view_tabs();
        self.arrange();
    }

    /// Tabs in views are kept: opening a file does not replace them.
    fn pin_view_tabs(&mut self) {
        let Some(tree) = &self.views.tree else { return };
        let ids: Vec<u64> = tree.leaves().into_iter().map(Leaf::id).collect();
        for tab in &mut self.file_tabs {
            if ids.contains(&tab.id) {
                tab.pinned = true;
            }
        }
    }

    /// View: Split right, left, up or down: the file again in a new view
    /// there, or another open tab where the file cannot be shown twice.
    pub(super) fn view_split(&mut self, dir: Dir) {
        let id = self.file_tabs[self.active_file].id;
        if self.tab_is_text(id) && !self.split_mirrors.iter().any(|m| m.0 == id) {
            self.mirror_open_at(dir);
        } else if let Some(other) = self.unshown_tab() {
            self.view_add(Leaf::Tab(other), dir);
        } else {
            self.message = Some("Every open tab is already in a view".into());
        }
    }

    /// Shows the current file in a second view, or closes its second view.
    pub(super) fn mirror_toggle(&mut self, horizontal: bool) {
        let id = self.file_tabs[self.active_file].id;
        // Closing keeps the view that has the keyboard, where it stands.
        if let Some(j) = self.split_mirrors.iter().position(|(m, _)| *m == id) {
            let flipped = self.split_mirrors.remove(j).1;
            self.doc.mirror = None;
            if let Some(tree) = &mut self.views.tree {
                if flipped {
                    tree.remove(Leaf::Tab(id));
                    tree.replace(Leaf::Second(id), Leaf::Tab(id));
                } else {
                    tree.remove(Leaf::Second(id));
                }
            }
            self.arrange();
            return;
        }
        self.mirror_open_at(if horizontal { Dir::Down } else { Dir::Right });
    }

    fn mirror_open_at(&mut self, dir: Dir) {
        if self.reader.is_some() || self.nb.is_some() {
            self.message = Some("Only text documents can be shown twice".into());
            return;
        }
        let id = self.file_tabs[self.active_file].id;
        self.doc.mirror_open();
        self.split_mirrors.push((id, false));
        self.view_add(Leaf::Second(id), dir);
    }

    /// Ends every second view, keeping each tab's live view.
    fn mirrors_close(&mut self) {
        for (id, _) in std::mem::take(&mut self.split_mirrors) {
            if let Some(pane) = self.tab_pane(id) {
                pane.mirror = None;
            }
        }
    }

    /// Ends one second view, putting back the first view's place.
    fn mirror_end(&mut self, id: u64, flipped: bool) {
        if let Some(pane) = self.tab_pane(id) {
            if flipped {
                pane.mirror_swap();
            }
            pane.mirror = None;
        }
    }

    /// A tab's text pane: the current document's, or the one it keeps.
    fn tab_pane(&mut self, id: u64) -> Option<&mut Pane> {
        let i = self.tab_index(id)?;
        if i == self.active_file {
            return Some(&mut self.doc);
        }
        self.file_tabs[i].state.as_mut().map(|s| &mut s.pane)
    }

    /// Keeps the views to open tabs and live second views, with the active
    /// tab in the view that had the keyboard.
    pub(super) fn views_tidy(&mut self) {
        let ids: Vec<u64> = self.file_tabs.iter().map(|t| t.id).collect();
        let live = self.live_leaf();
        let mut tree = self.views.tree.take();
        if let Some(t) = &mut tree
            && !t.contains(live)
        {
            let leaves = t.leaves();
            let at = self.views.focus.filter(|f| leaves.contains(f));
            let at = at.or_else(|| leaves.iter().copied().find(|l| !ids.contains(&l.id()))).unwrap_or(leaves[0]);
            t.replace(at, live);
        }
        let mirrored: Vec<u64> = self.split_mirrors.iter().map(|m| m.0).collect();
        let mut tree = tree.and_then(|t| {
            t.retain(|l| ids.contains(&l.id()) && (matches!(l, Leaf::Tab(_)) || mirrored.contains(&l.id())))
        });
        let mut keep = Vec::new();
        for (id, flipped) in std::mem::take(&mut self.split_mirrors) {
            let text = self.tab_is_text(id) && self.tab_pane(id).is_some_and(|p| p.mirror.is_some());
            let Some(t) = tree.as_mut().filter(|t| text && t.contains(Leaf::Second(id))) else {
                if let Some(t) = &mut tree {
                    t.remove(Leaf::Second(id));
                }
                self.mirror_end(id, flipped);
                continue;
            };
            if !t.contains(Leaf::Tab(id)) {
                // The second view stays as the tab's only view.
                t.replace(Leaf::Second(id), Leaf::Tab(id));
                self.mirror_end(id, !flipped);
                continue;
            }
            keep.push((id, flipped));
        }
        self.split_mirrors = keep;
        let tree = tree.filter(|t| t.len() >= 2);
        if tree.is_none() {
            self.views.zoomed = false;
            for (id, flipped) in std::mem::take(&mut self.split_mirrors) {
                self.mirror_end(id, flipped);
            }
        }
        self.views.tree = tree;
        let active = self.file_tabs[self.active_file].id;
        if !self.split_mirrors.iter().any(|(m, _)| *m == active) {
            self.doc.mirror = None;
        }
        self.views.focus = Some(self.live_leaf());
    }

    /// Where a tab's live view is drawn.
    pub(super) fn editor_rect(&self, id: u64) -> Rect {
        let flipped = self.split_mirrors.iter().any(|&(m, f)| m == id && f);
        self.rect_of(if flipped { Leaf::Second(id) } else { Leaf::Tab(id) }).unwrap_or_else(|| self.editor_area())
    }

    /// Where second view `j` keeps the view that is not live, if shown.
    fn mirror_rect(&self, j: usize) -> Rect {
        let (id, flipped) = self.split_mirrors[j];
        self.rect_of(if flipped { Leaf::Tab(id) } else { Leaf::Second(id) }).unwrap_or_default()
    }

    /// The second view under a point.
    pub(super) fn mirror_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.split_mirrors.len()).find(|&j| ui::inside(self.mirror_rect(j), Some((x, y))))
    }

    /// Does something to second view `j` as if it were live: scrolls it,
    /// or draws it.
    pub(super) fn with_mirror(&mut self, j: usize, f: impl FnOnce(&mut Pane)) {
        let (id, _) = self.split_mirrors[j];
        let (x, y, w, h) = self.mirror_rect(j);
        let live = self.editor_rect(id);
        let scale = self.scale;
        let Some(pane) = self.tab_pane(id) else { return };
        pane.mirror_swap();
        pane.set_rect(x, y, w, h, scale);
        f(pane);
        pane.mirror_swap();
        pane.set_rect(live.0, live.1, live.2, live.3, scale);
    }

    /// Puts tab `id`'s live view in its second view, or back in its first.
    fn mirror_set(&mut self, id: u64, second: bool) {
        if let Some(i) = self.tab_index(id) {
            self.activate_file(i);
        }
        self.views_tidy();
        if let Some(j) = self.split_mirrors.iter().position(|(m, _)| *m == id)
            && self.split_mirrors[j].1 != second
        {
            self.doc.mirror_swap();
            self.split_mirrors[j].1 = second;
        }
        self.arrange();
    }

    /// Gives a view the keyboard, keeping what each view shows.
    fn focus_leaf(&mut self, leaf: Leaf) {
        match leaf {
            Leaf::Tab(id) => self.mirror_set(id, false),
            Leaf::Second(id) => self.mirror_set(id, true),
        }
    }

    pub(super) fn draw_split(&mut self, c: &mut Canvas) {
        for i in self.split_others() {
            let state = self.file_tabs[i].state.as_mut().unwrap();
            state.pane.focused = false;
            match &mut state.reader {
                Some(r) => r.draw(c),
                None => state.pane.draw(c),
            }
        }
        for j in 0..self.split_mirrors.len() {
            if self.mirror_rect(j).2 <= 0.0 {
                continue;
            }
            self.with_mirror(j, |pane| {
                let focused = std::mem::replace(&mut pane.focused, false);
                pane.draw(c);
                pane.focused = focused;
            });
        }
        let line = self.view_gap();
        if self.view_rects().len() > 1 {
            for d in self.view_dividers() {
                c.line(d.rect.0, d.rect.1, d.rect.2, d.rect.3, theme::line());
            }
            let (x, y, w, _) = self.editor_rect(self.file_tabs[self.active_file].id);
            c.line(x, y, w, line, theme::primary());
        }
        if self.views.zoomed {
            self.draw_zoomed(c);
        }
        if let Some((_, _, (x, y, w, h))) = self.drop_zone() {
            c.shade(x, y, w, h, theme::primary(), 40);
            c.frame(x, y, w, h, line, theme::primary());
        }
    }

    /// A badge at the top right of a zoomed view, which restores the others.
    fn draw_zoomed(&mut self, c: &mut Canvas) {
        let s = self.scale;
        let (x, y, w, _) = self.editor_area();
        let label = "Zoomed";
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let bw = (fonts.width(font::UI_MEDIUM, px(theme::SMALL - 1.0, s), label) + px(10.0, s)).round();
        let r = ((x + w - bw - px(20.0, s)).round(), y + px(6.0, s), bw, px(18.0, s));
        ui::badge(c, fonts, r.0, r.1 + r.3 / 2.0, label, theme::primary(), s);
        let keys = self.keys_of("view_zoom");
        self.push_hit(r, chrome::Hit::ViewZoom, "Show all views (Esc)", &keys);
    }

    pub(super) fn focus_split_at(&mut self, x: f32, y: f32) {
        if self.view_divider_at(x, y).is_some() {
            return;
        }
        let hit = self.view_rects().into_iter().find(|(_, r)| ui::inside(*r, Some((x, y))));
        if let Some((leaf, _)) = hit.filter(|(l, _)| *l != self.live_leaf()) {
            self.focus_leaf(leaf);
        }
    }

    /// View: Focus other editor goes to the next view, second views too.
    pub(super) fn focus_next_view(&mut self) {
        let rects = self.all_view_rects();
        let at = rects.iter().position(|(l, _)| *l == self.live_leaf()).unwrap_or(0);
        if rects.len() > 1 {
            self.focus_leaf(rects[(at + 1) % rects.len()].0);
        }
    }

    /// The view on side `dir` of the focused one, zoomed or not.
    fn view_beside(&self, dir: Dir) -> Option<Leaf> {
        split_tree::neighbour(&self.all_view_rects(), self.live_leaf(), dir)
    }

    pub(super) fn view_focus(&mut self, dir: Dir) {
        if let Some(leaf) = self.view_beside(dir) {
            self.focus_leaf(leaf);
        }
    }

    /// Trades the focused view's place with the one beside it.
    pub(super) fn view_move(&mut self, dir: Dir) {
        let (live, other) = (self.live_leaf(), self.view_beside(dir));
        if let (Some(tree), Some(other)) = (&mut self.views.tree, other) {
            tree.swap(live, other);
            self.arrange();
        }
    }

    /// View: Close view. The tab stays open.
    pub(super) fn view_close(&mut self) {
        if self.views.tree.is_none() {
            self.message = Some("Only one view is open".into());
            return;
        }
        self.views.zoomed = false;
        let live = self.live_leaf();
        let id = live.id();
        if let Some(j) = self.split_mirrors.iter().position(|m| m.0 == id) {
            // The file's other view stays, as it stood.
            let flipped = self.split_mirrors.remove(j).1;
            self.doc.mirror_swap();
            self.doc.mirror = None;
            if let Some(tree) = &mut self.views.tree {
                if flipped {
                    tree.remove(Leaf::Second(id));
                } else {
                    tree.remove(Leaf::Tab(id));
                    tree.replace(Leaf::Second(id), Leaf::Tab(id));
                }
            }
            self.arrange();
            return;
        }
        let rects = self.all_view_rects();
        let next = DIRS.iter().find_map(|d| split_tree::neighbour(&rects, live, *d));
        let next = next.or_else(|| rects.iter().map(|r| r.0).find(|l| *l != live));
        if let Some(tree) = &mut self.views.tree {
            tree.remove(live);
        }
        match next {
            Some(leaf) => self.focus_leaf(leaf),
            None => self.arrange(),
        }
    }

    pub(super) fn view_zoom(&mut self) {
        if self.views.tree.is_none() {
            self.views.zoomed = false;
            self.message = Some("Only one view is open".into());
            return;
        }
        self.views.zoomed = !self.views.zoomed;
        self.arrange();
        self.redraw = true;
    }

    /// The keys of the view commands, by their default chords. Esc also
    /// restores a zoomed view when nothing else wants it.
    pub(super) fn view_key(&mut self, key: Key, mods: Mods) -> bool {
        let (c, a, s) = (mods.ctrl, mods.alt, mods.shift);
        let esc = key == Key::Escape && !c && !a && !s && self.views.zoomed && self.focus == Focus::Doc;
        let esc = esc && !self.vim_takes(key, mods) && self.doc.extra_cursors.is_empty() && !self.doc.buf.has_sel();
        match key {
            Key::Char('\\') if c && !a => self.view_split(Dir::Right),
            Key::Char('\\') if c && a => self.view_split(Dir::Left),
            Key::Char('-') if c && a => self.view_split(Dir::Down),
            Key::Char('=') if c && a => self.view_split(Dir::Up),
            Key::Char('w') if c && a && s => self.view_close(),
            Key::Char('z') if c && a && s => self.view_zoom(),
            _ if esc => self.view_zoom(),
            Key::Left if c && a && s => self.view_focus(Dir::Left),
            Key::Right if c && a && s => self.view_focus(Dir::Right),
            Key::Up if c && a && s => self.view_focus(Dir::Up),
            Key::Down if c && a && s => self.view_focus(Dir::Down),
            Key::Left if c && a => self.view_move(Dir::Left),
            Key::Right if c && a => self.view_move(Dir::Right),
            Key::PageUp if c && a && !s => self.view_move(Dir::Up),
            Key::PageDown if c && a && !s => self.view_move(Dir::Down),
            _ => return false,
        }
        self.redraw = true;
        true
    }

    // ---- dividers --------------------------------------------------------

    /// The divider between views under a point, with the reach of the
    /// panels' hairlines.
    fn view_divider_at(&self, x: f32, y: f32) -> Option<split_tree::Divider> {
        let reach = 4.0 * self.scale;
        self.view_dividers().into_iter().find(|d| {
            let (dx, dy, dw, dh) = d.rect;
            if d.stacked {
                x >= dx && x < dx + dw && (y - (dy + dh / 2.0)).abs() <= reach
            } else {
                y >= dy && y < dy + dh && (x - (dx + dw / 2.0)).abs() <= reach
            }
        })
    }

    pub(super) fn view_divider_zone(&self, x: f32, y: f32) -> Option<Zone> {
        self.view_divider_at(x, y).map(|d| if d.stacked { Zone::RowDivider } else { Zone::Divider })
    }

    /// Starts dragging a divider; a double click gives its views equal room.
    pub(super) fn view_divider_press(&mut self, x: f32, y: f32, clicks: u32) -> bool {
        let Some(d) = self.view_divider_at(x, y) else { return false };
        if clicks >= 2 {
            if let Some(tree) = &mut self.views.tree {
                tree.equalize(&d.path);
            }
            self.views.divider = None;
            self.arrange();
        } else {
            self.views.divider = Some((d.path, d.index, d.stacked));
        }
        true
    }

    pub(super) fn view_divider_motion(&mut self, x: f32, y: f32) {
        if self.tab_drag.is_some() {
            self.redraw = true;
        }
        let Some((path, index, stacked)) = self.views.divider.clone() else { return };
        let (area, gap, min) = (self.editor_area(), self.view_gap(), 80.0 * self.scale);
        if let Some(tree) = &mut self.views.tree {
            tree.drag(&path, index, if stacked { y } else { x }, area, gap, min);
        }
        self.arrange();
        self.redraw = true;
    }

    // ---- dragging tabs ---------------------------------------------------

    /// Notes, as a tab is pressed, what the focused view showed.
    pub(super) fn press_tab(&mut self, index: usize, x: f32, y: f32) {
        let id = self.file_tabs[index].id;
        let shown = self.views.tree.as_ref().map_or(index == self.active_file, |t| t.contains(Leaf::Tab(id)));
        self.views.drag_from = Some((self.file_tabs[self.active_file].id, shown));
        self.activate_file(index);
        self.tab_drag = Some((id, x, y));
    }

    /// Where a dragged tab would land: a view, and the side of it to split
    /// (none for its middle), with the part to mark.
    fn drop_zone(&self) -> Option<(Leaf, Option<Dir>, Rect)> {
        let (id, x0, y0) = self.tab_drag?;
        let (px, py) = self.pointer;
        if (px - x0).abs() + (py - y0).abs() < 8.0 * self.scale {
            return None;
        }
        let (leaf, r) = self.view_rects().into_iter().find(|(_, r)| ui::inside(*r, Some((px, py))))?;
        let (x, y, w, h) = r;
        let (fx, fy) = ((px - x) / w.max(1.0), (py - y) / h.max(1.0));
        let sides = [(fx, Dir::Left), (1.0 - fx, Dir::Right), (fy, Dir::Up), (1.0 - fy, Dir::Down)];
        let (near, dir) = sides.into_iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
        let (hw, hh) = ((w / 2.0).round(), (h / 2.0).round());
        let own = leaf == Leaf::Tab(id) && self.views.drag_from.is_none_or(|(prev, shown)| shown || prev == id);
        if near >= 0.25 {
            return (!own).then_some((leaf, None, r));
        }
        if own && (!self.tab_is_text(id) || self.split_mirrors.iter().any(|m| m.0 == id)) {
            return None;
        }
        let half = match dir {
            Dir::Left => (x, y, hw, h),
            Dir::Right => (x + w - hw, y, hw, h),
            Dir::Up => (x, y, w, hh),
            Dir::Down => (x, y + h - hh, w, hh),
        };
        Some((leaf, Some(dir), half))
    }

    /// Drops a dragged tab: on the side of a view, a new view there; in its
    /// middle, the two trade places.
    pub(super) fn drop_tab_in_view(&mut self) {
        self.views.divider = None;
        let zone = self.drop_zone();
        let origin = self.views.drag_from.take();
        let Some((id, ..)) = self.tab_drag.take() else { return };
        let Some((mut target, dir, _)) = zone else { return };
        self.redraw = true;
        if dir.is_some() && target == Leaf::Tab(id) && origin.is_none_or(|(prev, shown)| shown || prev == id) {
            // The tab's own view: the file again beside it.
            return self.mirror_open_at(dir.unwrap());
        }
        if dir.is_some() && let Some(j) = self.split_mirrors.iter().position(|m| m.0 == id) {
            let flipped = self.split_mirrors.remove(j).1;
            self.mirror_end(id, flipped);
            if let Some(tree) = &mut self.views.tree {
                tree.remove(Leaf::Second(id));
            }
        }
        let live = self.live_leaf();
        let mut tree = self.views.tree.take().unwrap_or_else(|| Tree::new(live));
        // Pressing the tab put it in the focused view; that view gets back
        // what it showed.
        if let Some((prev, false)) = origin
            && prev != id
            && self.tab_index(prev).is_some()
            && !tree.contains(Leaf::Tab(prev))
        {
            tree.replace(Leaf::Tab(id), Leaf::Tab(prev));
            if target == Leaf::Tab(id) {
                target = Leaf::Tab(prev);
            }
        }
        match dir {
            Some(dir) => {
                if tree.contains(Leaf::Tab(id)) {
                    tree.remove(Leaf::Tab(id));
                }
                tree.split(target, Leaf::Tab(id), dir);
            }
            None if tree.contains(Leaf::Tab(id)) => {
                tree.swap(Leaf::Tab(id), target);
            }
            None => {
                tree.replace(target, Leaf::Tab(id));
            }
        }
        self.views.tree = Some(tree);
        self.views.focus = Some(Leaf::Tab(id));
        self.views.zoomed = false;
        self.pin_view_tabs();
        self.arrange();
    }

    // ---- closing tabs and sessions --------------------------------------

    /// Takes a closing tab out of the views. Returns the tab of the view
    /// to give the keyboard to, when the tab had it.
    pub(super) fn views_forget(&mut self, id: u64) -> Option<usize> {
        let live = self.live_leaf();
        let rects = self.all_view_rects();
        let tree = self.views.tree.as_mut()?;
        let next = DIRS.iter().filter_map(|d| split_tree::neighbour(&rects, live, *d)).find(|l| l.id() != id);
        let next = next.or_else(|| rects.iter().map(|r| r.0).find(|l| l.id() != id)).filter(|_| live.id() == id);
        tree.remove(Leaf::Second(id));
        tree.remove(Leaf::Tab(id));
        next.and_then(|l| self.tab_index(l.id()))
    }

    /// The views for the session file, by path.
    pub(super) fn views_json(&self) -> Option<serde_json::Value> {
        let paths = self.tab_files();
        let tree = self.views.tree.as_ref()?;
        tree.to_json(|leaf| {
            let (path, _) = paths.get(self.tab_index(leaf.id())?)?.as_ref()?;
            Some(serde_json::json!({ "path": path.to_string_lossy(), "second": matches!(leaf, Leaf::Second(_)) }))
        })
    }

    /// Puts back the views a session file holds, leaving out those whose
    /// files are gone. Older files hold a list of paths and a direction.
    pub(super) fn views_restore(&mut self, s: &serde_json::Value) {
        let files = self.tab_files();
        let id_of = |p: &str| {
            let i = files.iter().position(|f| f.as_ref().is_some_and(|(path, _)| path == Path::new(p)))?;
            Some(self.file_tabs[i].id)
        };
        let mut seen = Vec::new();
        let tree = if s["views"].is_object() {
            Tree::from_json(&s["views"], |v| {
                let id = id_of(v["path"].as_str()?)?;
                let leaf = if v["second"].as_bool() == Some(true) { Leaf::Second(id) } else { Leaf::Tab(id) };
                (!seen.contains(&leaf)).then(|| seen.push(leaf)).map(|_| leaf)
            })
        } else {
            let paths = s["split"].as_array().into_iter().flatten().filter_map(|p| id_of(p.as_str()?));
            let leaves: Vec<Leaf> = paths.map(Leaf::Tab).fold(Vec::new(), |mut v, l| {
                if !v.contains(&l) {
                    v.push(l);
                }
                v
            });
            Tree::even(&leaves, s["split_horizontal"].as_bool().unwrap_or(false))
        };
        let Some(mut tree) = tree.filter(|t| t.len() >= 2) else { return };
        for leaf in tree.leaves() {
            if let Leaf::Second(id) = leaf
                && !(tree.contains(Leaf::Tab(id)) && self.tab_is_text(id))
            {
                tree.remove(leaf);
            }
        }
        let first = tree.leaves().into_iter().find(|l| matches!(l, Leaf::Tab(_)));
        if !tree.contains(self.live_leaf())
            && let Some(i) = first.and_then(|l| self.tab_index(l.id()))
        {
            self.activate_file(i);
        }
        for leaf in tree.leaves() {
            if let Leaf::Second(id) = leaf
                && let Some(pane) = self.tab_pane(id)
            {
                pane.mirror_open();
                self.split_mirrors.push((id, false));
            }
        }
        self.views.tree = Some(tree);
        self.pin_view_tabs();
        self.arrange();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_file_in_two_views_keeps_each_place_and_follows_edits() {
        let mut a = App::new(None);
        a.resize(1200, 700, 1.0);
        a.paste_text(&format!("first\n{}last\n", "middle\n".repeat(80)));
        a.doc.select(0, 0);
        a.mirror_toggle(false);
        assert_eq!(a.split_mirrors.len(), 1);
        let id = a.file_tabs[a.active_file].id;
        let (lx, _, lw, _) = a.editor_rect(id);
        assert!(a.mirror_rect(0).0 >= lx + lw, "the second view stands beside the first");
        // The second view gets the keyboard where it stands and moves alone.
        a.focus_next_view();
        assert!(a.split_mirrors[0].1);
        assert!(a.editor_rect(id).0 > lx);
        let end = a.doc.buf.text.len();
        a.doc.select(end, end);
        a.doc.moved();
        a.focus_next_view();
        assert_eq!((a.doc.buf.cur, a.editor_rect(id).0), (0, lx));
        // Typing in the first view moves the second view's place with the text.
        a.paste_text("new ");
        a.focus_next_view();
        assert_eq!(a.doc.buf.cur, a.doc.buf.text.len());
        a.focus_next_view();
        // The wheel scrolls the view under the pointer only.
        let live = a.doc.scroll_position();
        let before = a.doc.mirror;
        let (mx, my, mw, mh) = a.mirror_rect(0);
        a.pointer = (mx + mw / 2.0, my + mh / 2.0);
        a.with_mirror(0, |p| p.scroll(-10_000.0));
        a.with_mirror(0, |p| p.scroll(200.0));
        assert_eq!(a.doc.scroll_position(), live);
        assert_ne!(a.doc.mirror, before);
        let mut px = vec![0u32; 1200 * 700];
        a.draw(&mut Canvas::new(&mut px, 1200, 700));
        // A click in the second view gives it the keyboard.
        a.press(mx + mw / 2.0, my + mh / 2.0, Mods::default(), 1);
        a.release();
        assert!(a.split_mirrors[0].1);
        // Closing keeps the view that had the keyboard.
        let cur = a.doc.buf.cur;
        a.mirror_toggle(false);
        assert!(a.split_mirrors.is_empty() && a.doc.mirror.is_none());
        assert_eq!(a.doc.buf.cur, cur);
        assert_eq!(a.editor_rect(id).2, a.document_area().1);
    }
    #[test]
    fn views_nest_in_rows_and_columns_and_tab_drag_swaps_them() {
        let mut a = App::new(None);
        a.resize(1200, 900, 1.0);
        a.open_output("first".into());
        a.open_output("second".into());
        a.open_output("third".into());
        a.activate_file(1);
        let id = |a: &App, i: usize| a.file_tabs[i].id;
        let (one, two, three) = (id(&a, 1), id(&a, 2), id(&a, 3));
        a.split_add(false);
        a.split_add(true);
        let leaves = a.views.tree.as_ref().unwrap().leaves();
        assert_eq!(leaves, [Leaf::Tab(one), Leaf::Tab(three), Leaf::Tab(two)]);
        // The first view is split below; the other keeps the whole height.
        let (r1, r2, r3) = (a.editor_rect(one), a.editor_rect(two), a.editor_rect(three));
        assert_eq!((r1.0, r3.0), (r1.0, r1.0));
        assert!(r3.1 > r1.1 + r1.3 && r2.0 > r1.0 + r1.2);
        assert_eq!(r2.3, a.editor_area().3);
        // Dropping a tab in the middle of a view trades their places.
        a.press_tab(3, 10.0, 10.0);
        a.pointer = (r2.0 + r2.2 / 2.0, r2.1 + r2.3 / 2.0);
        a.drop_tab_in_view();
        assert_eq!(a.views.tree.as_ref().unwrap().leaves(), [Leaf::Tab(one), Leaf::Tab(two), Leaf::Tab(three)]);
        assert_eq!(a.file_tabs[a.active_file].id, three);
        let middle = a.file_tabs.iter().position(|t| t.id == two).unwrap();
        a.close_file_tab(middle);
        assert_eq!(a.views.tree.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn tabs_dropped_on_a_side_split_there_and_views_close() {
        let mut a = App::new(None);
        a.resize(1200, 900, 1.0);
        a.open_output("a".into());
        a.open_output("b".into());
        a.activate_file(1);
        let (first, second) = (a.file_tabs[1].id, a.file_tabs[2].id);
        // Dragging another tab to the right edge of the only view.
        a.press_tab(2, 10.0, 10.0);
        let area = a.editor_area();
        a.pointer = (area.0 + area.2 - 20.0, area.1 + area.3 / 2.0);
        let (_, dir, mark) = a.drop_zone().unwrap();
        assert_eq!(dir, Some(Dir::Right));
        assert!(mark.0 > area.0 + area.2 / 3.0);
        let mut px = vec![0u32; 1200 * 900];
        a.draw(&mut Canvas::new(&mut px, 1200, 900));
        a.release();
        assert_eq!(a.views.tree.as_ref().unwrap().leaves(), [Leaf::Tab(first), Leaf::Tab(second)]);
        assert_eq!(a.file_tabs[a.active_file].id, second);
        // Dropping it on its own view's bottom edge shows the file twice.
        a.press_tab(2, 10.0, 10.0);
        let r = a.editor_rect(second);
        a.pointer = (r.0 + r.2 / 2.0, r.1 + r.3 - 10.0);
        a.release();
        assert_eq!(a.views.tree.as_ref().unwrap().leaves(), [Leaf::Tab(first), Leaf::Tab(second), Leaf::Second(second)]);
        // Closing views: the file's other view stays, then the views go.
        a.focus_next_view();
        assert_eq!(a.live_leaf(), Leaf::Second(second));
        a.view_close();
        assert!(a.split_mirrors.is_empty());
        assert_eq!(a.views.tree.as_ref().unwrap().leaves(), [Leaf::Tab(first), Leaf::Tab(second)]);
        a.view_close();
        assert!(a.views.tree.is_none());
        assert_eq!(a.file_tabs.len(), 3, "closing a view keeps its tab");
        assert_eq!(a.file_tabs[a.active_file].id, first);
    }

    #[test]
    fn dividers_drag_and_reset_and_keys_move_between_views() {
        let mut a = App::new(None);
        a.resize(1200, 900, 1.0);
        a.open_output("a".into());
        a.open_output("b".into());
        a.open_output("c".into());
        a.activate_file(1);
        let ids: Vec<u64> = a.file_tabs.iter().map(|t| t.id).collect();
        a.split_add(false);
        a.split_add(true);
        let d = a.view_dividers().into_iter().find(|d| !d.stacked).unwrap();
        let x = d.rect.0;
        assert_eq!(a.zone(x + 2.0, 300.0, false), Zone::Divider);
        a.press(x + 2.0, 300.0, Mods::default(), 1);
        a.motion(300.0, 310.0);
        a.release();
        assert!((a.editor_rect(ids[1]).2 - 300.0).abs() <= 2.0, "{:?}", a.editor_rect(ids[1]));
        // The share is kept when the window changes size.
        a.resize(2400, 900, 1.0);
        assert!((a.editor_rect(ids[1]).2 - 600.0).abs() <= 3.0);
        a.resize(1200, 900, 1.0);
        a.press(300.0, 310.0, Mods::default(), 2);
        a.release();
        assert!((a.editor_rect(ids[1]).2 - 600.0).abs() <= 1.0);
        let row = a.view_dividers().into_iter().find(|d| d.stacked).unwrap();
        assert_eq!(a.zone(100.0, row.rect.1, false), Zone::RowDivider);

        let keys = Mods { ctrl: true, alt: true, shift: true };
        a.key(Key::Right, keys, None);
        assert_eq!(a.file_tabs[a.active_file].id, ids[2]);
        a.key(Key::Left, keys, None);
        a.key(Key::Down, keys, None);
        assert_eq!(a.file_tabs[a.active_file].id, ids[3]);
        a.key(Key::Up, keys, None);
        assert_eq!(a.file_tabs[a.active_file].id, ids[1]);
        // Moving trades places with the view beside.
        a.key(Key::Right, Mods { ctrl: true, alt: true, shift: false }, None);
        assert_eq!(a.views.tree.as_ref().unwrap().leaves(), [Leaf::Tab(ids[2]), Leaf::Tab(ids[3]), Leaf::Tab(ids[1])]);
        assert!(a.editor_rect(ids[1]).0 > 500.0);
        assert_eq!(a.file_tabs[a.active_file].id, ids[1]);
        // A split key adds the file again beside the focused view.
        a.key(Key::Char('-'), Mods { ctrl: true, alt: true, shift: false }, None);
        assert_eq!(a.views.tree.as_ref().unwrap().len(), 4);
        assert!(a.views.tree.as_ref().unwrap().contains(Leaf::Second(ids[1])));
    }

    #[test]
    fn a_zoomed_view_fills_the_editor_until_esc() {
        let mut a = App::new(None);
        a.resize(1200, 900, 1.0);
        a.open_output("a".into());
        a.open_output("b".into());
        a.activate_file(1);
        a.split_add(false);
        let id = a.file_tabs[a.active_file].id;
        a.key(Key::Char('z'), Mods { ctrl: true, alt: true, shift: true }, None);
        assert!(a.views.zoomed);
        assert_eq!(a.editor_rect(id), a.editor_area());
        assert!(a.split_others().is_empty() && a.view_dividers().is_empty());
        let mut px = vec![0u32; 1200 * 900];
        a.draw(&mut Canvas::new(&mut px, 1200, 900));
        assert!(a.hits.iter().any(|(_, h)| *h == chrome::Hit::ViewZoom));
        // Focus moves stay zoomed, on the other view.
        a.focus_next_view();
        assert!(a.views.zoomed && a.file_tabs[a.active_file].id != id);
        a.key(Key::Escape, Mods::default(), None);
        assert!(!a.views.zoomed);
        assert_eq!(a.split_others().len(), 1);
    }

    #[test]
    fn sessions_keep_the_tree_and_its_shares() {
        let dir = std::env::temp_dir().join(format!("ld-split-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.md", "b.md", "c.md"] {
            std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
        }
        let dir = dir.canonicalize().unwrap();
        let mut a = App::new(Some(dir.clone()));
        a.resize(1200, 900, 1.0);
        for name in ["a.md", "b.md", "c.md"] {
            a.pick(dir.join(name), false);
            a.file_tabs[a.active_file].pinned = true;
        }
        a.activate_file(0);
        a.split_add(false);
        a.split_add(true);
        a.view_split(Dir::Right);
        let d = a.view_dividers().into_iter().find(|d| !d.stacked && d.path.is_empty()).unwrap();
        a.views.divider = Some((d.path, d.index, false));
        let left = a.editor_area().0;
        a.view_divider_motion(left + 300.0, 0.0);
        a.views.divider = None;
        let before = a.all_view_rects().into_iter().map(|(l, r)| (a.tab_index(l.id()).unwrap(), l, r)).collect::<Vec<_>>();
        a.save_session();
        std::fs::remove_file(dir.join("c.md")).unwrap();

        let mut b = App::new(Some(dir.clone()));
        b.resize(1200, 900, 1.0);
        let after = b.all_view_rects();
        // c.md is gone, so three views are left: a.md twice beside b.md.
        assert_eq!(after.len(), 3);
        assert_eq!(b.split_mirrors.len(), 1);
        let path = |b: &App, l: Leaf| b.tab_files()[b.tab_index(l.id()).unwrap()].as_ref().unwrap().0.clone();
        assert_eq!(path(&b, after[0].0), dir.join("a.md"));
        assert!(matches!(after[1].0, Leaf::Second(_)));
        assert_eq!(path(&b, after[2].0), dir.join("b.md"));
        assert!((after[0].1.2 - before[0].2.2).abs() <= 1.0, "{after:?} {before:?}");
        assert!((after[2].1.2 - before[3].2.2).abs() <= 1.0 && after[0].1.3 == after[2].1.3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_switches_buffers_without_losing_edits_or_undo() {
        let mut a = App::new(None);
        a.resize(1200, 800, 1.0);
        a.open_output("left".into());
        a.doc.read_only = false;
        let left = a.active_file;
        a.open_output("right".into());
        a.doc.read_only = false;
        a.activate_file(left);
        a.split_editor();
        a.doc.insert("edited ", EditKind::Other);
        let other = a.split_other().unwrap();
        a.activate_file(other);
        assert_ne!(a.doc.buf.text, "edited left");
        assert_eq!(a.split_other(), Some(left));
        a.activate_file(left);
        assert_eq!(a.doc.buf.text, "edited left");
        a.doc.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(a.doc.buf.text, "left");
        a.close_file_tab(other);
        assert!(a.split_other().is_none());
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    fn mouse_focus_save_and_output_preserve_the_other_split_document() {
        let dir = std::env::temp_dir().join(format!("ld-split-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "left\n").unwrap();
        std::fs::write(dir.join("b.rs"), "right\n").unwrap();
        let mut a = App::new(Some(dir.join("a.rs")));
        a.file_tabs[0].pinned = true;
        a.open_tab(dir.join("b.rs"));
        a.split_editor();
        for (w, h, scale) in [(1200, 800, 1.0), (640, 600, 1.0), (1800, 1200, 1.5)] {
            a.resize(w, h, scale);
            let mut pixels = vec![0; (w * h) as usize];
            let mut c = Canvas::new(&mut pixels, w as i32, h as i32);
            a.draw(&mut c);
            let other = a.split_other().unwrap();
            let p = &mut a.file_tabs[other].state.as_mut().unwrap().pane;
            let (x, y, line) = p.spot(0).unwrap();
            a.press(x + 3.0, y + line * 0.5, Mods::default(), 1);
            a.release();
            assert_eq!(a.active_file, other);
        }
        a.doc.select(0, 0);
        a.doc.insert("saved ", EditKind::Other);
        a.save();
        assert!(std::fs::read_to_string(a.doc.path.as_ref().unwrap()).unwrap().starts_with("saved "));
        let other_id = a.file_tabs[a.split_other().unwrap()].id;
        a.open_output("comparison".into());
        assert_eq!(a.file_tabs[a.split_other().unwrap()].id, other_id);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
