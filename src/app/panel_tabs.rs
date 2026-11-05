//! Which of the right panel's tabs fit in its head, and the ⋯ list of the
//! rest, so every tab stays reachable and the open one stays in view.
use super::*;
use crate::icons::Icon;

/// The tabs shown, in their order, and whether some are left over for the
/// ⋯ list. `widths` include the gap after each tab; `more` is the ⋯
/// button's width. The active tab is always shown, cut short if it must be.
pub(super) fn fit(widths: &[f32], active: Option<usize>, room: f32, more: f32) -> (Vec<usize>, bool) {
    if widths.iter().sum::<f32>() <= room {
        return ((0..widths.len()).collect(), false);
    }
    let mut left = room - more - active.map_or(0.0, |a| widths[a]);
    let mut shown: Vec<usize> = active.into_iter().collect();
    for (i, w) in widths.iter().enumerate() {
        if Some(i) == active {
            continue;
        }
        if *w > left {
            break;
        }
        left -= w;
        shown.push(i);
    }
    shown.sort_unstable();
    (shown, true)
}

impl App {
    /// The right panel's views, as tabs: the tab, its icon, its name and
    /// the command that opens it.
    pub(super) fn panel_views(&self) -> Vec<(Tab, Icon, &'static str, &'static str)> {
        let mut views = vec![(Tab::Agent, Icon::Message, "Agent", "sidebar")];
        views.push((Tab::Search, Icon::Search, "Search", "search"));
        views.push((Tab::Problems, Icon::ListTree, "Problems", "show_problems"));
        views.push((Tab::Tests, Icon::Check, "Tests", "tests_list"));
        views.push((Tab::Hierarchy, Icon::ListTree, "Hierarchy", "lsp_hierarchy_incoming"));
        views.push((Tab::Outline, Icon::ListTree, "Outline", "outline"));
        views.push((Tab::Backlinks, Icon::FileText, "Backlinks", "backlinks"));
        views.push((Tab::Debug, Icon::Play, "Debug", "debug_show"));
        if self.buildable() || self.tab == Tab::Preview {
            views.push((Tab::Preview, Icon::FileText, "Preview", "preview"));
        }
        views
    }

    /// Lists the panel's tabs to choose one: those that do not fit, from
    /// the ⋯ button, or all of them, from the palette.
    pub(super) fn panel_tabs_menu(&mut self, hidden_only: bool) {
        let views = self.panel_views();
        let entries: Vec<Entry> = views
            .iter()
            .filter(|(tab, ..)| !hidden_only || self.panel_hidden.contains(tab))
            .map(|(tab, icon, label, id)| {
                let note = if *tab == self.tab && self.side_open { "Open" } else { "" };
                Entry::new(label, label, note).keys(self.keys_of(id)).icon(*icon)
            })
            .collect();
        let current = views.iter().find(|v| v.0 == self.tab).map_or("", |v| v.2);
        let mut menu = Menu::new("Panel tabs", entries, current);
        if hidden_only && let Some(r) = self.panel_more {
            menu.width = 240.0;
            menu.anchor = Some(((r.0 + r.2 - menu.width * self.scale).max(0.0), r.1 + r.3));
        }
        self.menu = Some((MenuKind::PanelTabs, menu));
        self.redraw = true;
    }

    pub(super) fn pick_panel_tab(&mut self, key: &str) {
        if let Some((tab, ..)) = self.panel_views().into_iter().find(|v| v.2 == key) {
            self.do_hit(chrome::Hit::View(tab));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_open_tab_stays_in_view_and_the_rest_overflow() {
        let w = [50.0, 60.0, 40.0, 70.0, 55.0];
        assert_eq!(fit(&w, Some(0), 400.0, 24.0), (vec![0, 1, 2, 3, 4], false));
        // 160 of room: the ⋯ button takes 24, the open tab 70.
        assert_eq!(fit(&w, Some(3), 160.0, 24.0), (vec![0, 3], true));
        assert_eq!(fit(&w, Some(0), 160.0, 24.0), (vec![0, 1], true));
        // Too narrow even for the open tab: it is still shown.
        assert_eq!(fit(&w, Some(4), 30.0, 24.0), (vec![4], true));
        assert_eq!(fit(&w, None, 100.0, 24.0), (vec![0], true));
    }

    #[test]
    fn every_tab_is_reachable_in_a_narrow_window_with_extra_tabs() {
        let mut a = App::new(None);
        a.w = 520.0;
        a.show_side(Some(Tab::Tests));
        let mut pixels = vec![0; 520 * 400];
        let mut c = Canvas::new(&mut pixels, 520, 400);
        a.panel_head(&mut c);
        assert!(a.panel_more.is_some(), "the ⋯ button shows when tabs overflow");
        assert!(!a.panel_hidden.is_empty());
        assert!(!a.panel_hidden.contains(&Tab::Tests), "the open tab stays in view");
        a.do_hit(chrome::Hit::ViewOverflow);
        let hidden = a.panel_hidden.clone();
        let (_, menu) = a.menu.as_ref().unwrap();
        assert_eq!(menu.entries.len(), hidden.len());
        let label = a.panel_views().into_iter().find(|v| v.0 == hidden[0]).unwrap().2;
        a.menu = None;
        a.menu_pick(MenuKind::PanelTabs, label.into());
        assert_eq!(a.tab, hidden[0]);
    }
}
