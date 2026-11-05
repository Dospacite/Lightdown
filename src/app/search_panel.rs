//! The Search tab of the right panel: a query and a replacement, the
//! search options and path filters, and the matches grouped by file as
//! they stream in from the worker. Each match and each file previews its
//! replacement and can be replaced or dismissed on its own.

use super::search::Found;
use super::*;
use crate::icons::{self, Icon};
use crate::ui::px;

/// How many queries the history keeps.
const HISTORY: usize = 50;

/// What a click on the panel does; the index is a group or a result.
pub(super) mod act {
    pub const QUERY: u8 = 0;
    pub const REPLACE: u8 = 1;
    pub const TOGGLE: u8 = 2;
    pub const CASE: u8 = 3;
    pub const WORD: u8 = 4;
    pub const REGEX: u8 = 5;
    pub const PRESERVE: u8 = 6;
    pub const INCLUDE: u8 = 7;
    pub const EXCLUDE: u8 = 8;
    pub const COLLAPSE: u8 = 9;
    pub const REFRESH: u8 = 10;
    pub const REPLACE_ALL: u8 = 11;
    pub const EDITOR: u8 = 12;
    pub const FILE: u8 = 20;
    pub const FILE_REPLACE: u8 = 21;
    pub const FILE_DISMISS: u8 = 22;
    pub const FILE_DIFF: u8 = 23;
    pub const MATCH: u8 = 30;
    pub const MATCH_REPLACE: u8 = 31;
    pub const MATCH_DISMISS: u8 = 32;
}

/// A line of text being typed, with its caret.
#[derive(Default, Clone)]
pub(super) struct Field {
    pub text: String,
    pub cur: usize,
}

impl Field {
    /// Edits the field for a key; returns whether the key was its own, and
    /// sets `changed` when the text changed.
    fn key(&mut self, key: Key, mods: Mods, text: Option<&str>, changed: &mut bool) -> bool {
        let t = &self.text;
        match key {
            Key::Left => self.cur = if mods.ctrl { word_start(t, self.cur) } else { crate::bidi::visual_move(t,self.cur,false) },
            Key::Right => self.cur = if mods.ctrl { word_end(t, self.cur) } else { crate::bidi::visual_move(t,self.cur,true) },
            Key::Home => self.cur = 0,
            Key::End => self.cur = t.len(),
            Key::Backspace if self.cur > 0 => {
                let a = if mods.ctrl { word_start(t, self.cur) } else { editor::prev(t,self.cur) };
                self.text.replace_range(a..self.cur, "");
                self.cur = a;
                *changed = true;
            }
            Key::Delete if self.cur < t.len() => {
                let b = if mods.ctrl { word_end(t, self.cur) } else { editor::next(t,self.cur).min(t.len()) };
                self.text.replace_range(self.cur..b, "");
                *changed = true;
            }
            Key::Backspace | Key::Delete => {}
            _ if mods.ctrl || mods.alt => return false,
            _ => {
                let Some(s) = text.filter(|s| !s.is_empty() && !s.chars().any(char::is_control)) else { return false };
                self.insert(s);
                *changed = true;
            }
        }
        true
    }

    pub fn insert(&mut self, s: &str) {
        let s = s.lines().next().unwrap_or("");
        self.text.insert_str(self.cur, s);
        self.cur += s.len();
    }

    fn set(&mut self, s: &str) {
        self.text = s.into();
        self.cur = s.len();
    }
}

fn word_start(t: &str, mut at: usize) -> usize {
    while at > 0 && t[..at].ends_with(|c: char| !c.is_alphanumeric()) {
        at = editor::prev(t, at);
    }
    while at > 0 && t[..at].ends_with(char::is_alphanumeric) {
        at = editor::prev(t, at);
    }
    at
}

fn word_end(t: &str, mut at: usize) -> usize {
    while at < t.len() && t[at..].starts_with(|c: char| !c.is_alphanumeric()) {
        at = editor::next(t, at);
    }
    while at < t.len() && t[at..].starts_with(char::is_alphanumeric) {
        at = editor::next(t, at);
    }
    at.min(t.len())
}

/// A row of the list: a file's head, or one of its matches.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Row {
    File(usize),
    Match(usize),
}

/// The matches of one file: its path, and where they lie in the results.
#[derive(Clone, Debug)]
pub(super) struct Group {
    pub path: PathBuf,
    pub start: usize,
    pub end: usize,
}

#[derive(Default)]
pub(super) struct SearchPanel {
    pub query: Field,
    pub replace: Field,
    pub show_replace: bool,
    /// Whether the keys are with the panel, and with which part: the
    /// query, the replacement or the list.
    pub keys: bool,
    pub field: u8,
    pub status: String,
    pub results: Vec<Found>,
    pub groups: Vec<Group>,
    collapsed: std::collections::HashSet<PathBuf>,
    pub rows: Vec<Row>,
    /// The row chosen in the list.
    pub sel: usize,
    pub rect: (f32, f32, f32, f32),
    pub scroll: f32,
    /// Where the list was last drawn, to keep the chosen row in view.
    list: (f32, f32),
    history: Vec<String>,
    /// Where Up and Down are in the history, and what was typed before.
    back: Option<usize>,
    draft: String,
}

impl SearchPanel {
    pub fn set_query(&mut self, s: &str) {
        self.query.set(s);
        self.back = None;
    }

    pub fn clear_results(&mut self) {
        self.results.clear();
        self.groups.clear();
        self.rows.clear();
        self.sel = 0;
    }

    /// Adds what the worker found, which comes file by file.
    pub fn append(&mut self, found: Vec<Found>) {
        for f in found {
            let i = self.results.len();
            match self.groups.last_mut() {
                Some(g) if g.path == f.path => g.end = i + 1,
                _ => self.groups.push(Group { path: f.path.clone(), start: i, end: i + 1 }),
            }
            self.results.push(f);
        }
        self.relist();
    }

    /// Works out the groups again after results were taken out.
    fn regroup(&mut self) {
        let found = std::mem::take(&mut self.results);
        self.groups.clear();
        self.append(found);
    }

    /// Works out the rows from the groups and what is collapsed.
    fn relist(&mut self) {
        let chosen = self.rows.get(self.sel).copied();
        self.rows.clear();
        for (g, group) in self.groups.iter().enumerate() {
            self.rows.push(Row::File(g));
            if !self.collapsed.contains(&group.path) {
                self.rows.extend((group.start..group.end).map(Row::Match));
            }
        }
        if let Some(i) = chosen.and_then(|r| self.rows.iter().position(|q| *q == r)) {
            self.sel = i;
        }
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
    }

    fn group_of(&self, result: usize) -> Option<usize> {
        self.groups.iter().position(|g| (g.start..g.end).contains(&result))
    }

    fn toggle(&mut self, g: usize) {
        let Some(path) = self.groups.get(g).map(|g| g.path.clone()) else { return };
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        self.relist();
        if let Some(i) = self.rows.iter().position(|r| *r == Row::File(g)) {
            self.sel = i;
        }
    }

    /// Collapses every file, or expands them all if all are collapsed.
    fn toggle_all(&mut self) {
        if self.groups.iter().all(|g| self.collapsed.contains(&g.path)) {
            self.collapsed.clear();
        } else {
            self.collapsed.extend(self.groups.iter().map(|g| g.path.clone()));
        }
        self.relist();
    }

    fn dismiss_result(&mut self, i: usize) {
        if i < self.results.len() {
            self.results.remove(i);
            self.regroup();
        }
    }

    fn dismiss_group(&mut self, g: usize) {
        if let Some(group) = self.groups.get(g).cloned() {
            self.results.drain(group.start..group.end);
            self.regroup();
        }
    }

    /// Keeps a query to come back to with Up.
    pub fn remember(&mut self) {
        let q = self.query.text.trim().to_string();
        if q.chars().count() >= 2 {
            self.history.retain(|h| *h != q);
            self.history.push(q);
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        self.back = None;
    }

    /// Up goes back through the history, Down forward to what was typed.
    fn recall(&mut self, older: bool) -> bool {
        let n = self.history.len();
        let next = match (self.back, older) {
            (None, true) if n > 0 => Some(n - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            (Some(_), false) => None,
            _ => return false,
        };
        if self.back.is_none() {
            self.draft = self.query.text.clone();
        }
        self.back = next;
        let text = next.map_or(self.draft.clone(), |i| self.history[i].clone());
        self.query.set(&text);
        true
    }

    fn files(&self) -> usize {
        self.groups.len()
    }
}

impl App {
    /// Whether the panel is in view.
    fn search_panel_open(&self) -> bool {
        !self.focus_mode && self.side_open && self.tab == Tab::Search && self.search.panel.rect.2 > 0.0
    }

    pub(super) fn search_panel_contains(&self, x: f32, y: f32) -> bool {
        self.search_panel_open() && ui::inside(self.search.panel.rect, Some((x, y)))
    }

    /// Whether the keys go to the panel.
    pub(super) fn search_panel_has_keys(&self) -> bool {
        self.search.panel.keys
            && self.search_panel_open()
            && self.focus == Focus::Doc
            && !self.overlaid()
            && !self.prompt_has_keys()
    }

    /// Gives the keys to a part of the panel: 0 the query, 1 the
    /// replacement, 2 the list.
    pub(super) fn search_panel_focus(&mut self, field: u8) {
        let p = &mut self.search.panel;
        p.keys = true;
        p.field = if field == 1 && !p.show_replace { 0 } else { field };
        self.focus = Focus::Doc;
        self.comp = None;
        if let Some(p) = self.prompt.as_mut().filter(|p| p.finds()) {
            p.away = true;
        }
        self.redraw = true;
    }

    /// Pasted text goes into the field with the keys.
    pub(super) fn search_panel_paste(&mut self, s: &str) -> bool {
        if !self.search_panel_has_keys() {
            return false;
        }
        let p = &mut self.search.panel;
        if p.field == 1 {
            p.replace.insert(s);
        } else {
            p.field = 0;
            p.query.insert(s);
            self.search_update();
        }
        self.redraw = true;
        true
    }

    pub(super) fn search_panel_status(&mut self) {
        let p = &self.search.panel;
        let (n, files) = (p.results.len(), p.files());
        let mut status = match (n, files) {
            (0, _) if self.search.pending => "Searching\u{2026}".to_string(),
            (0, _) => "No results".to_string(),
            (1, _) => "1 result in 1 file".to_string(),
            (n, 1) => format!("{n} results in 1 file"),
            (n, f) => format!("{n} results in {f} files"),
        };
        if self.search.more {
            status += &format!(", stopped at {}", super::search::MOST);
        } else if self.search.pending && n > 0 {
            status += "\u{2026}";
        }
        self.search.panel.status = status;
    }

    /// A key while the panel has the keys; returns whether it took it.
    pub(super) fn search_panel_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if !self.search_panel_has_keys() {
            return false;
        }
        self.redraw = true;
        let plain = !mods.ctrl && !mods.alt;
        match key {
            Key::Escape => {
                self.search.panel.keys = false;
                return true;
            }
            Key::Tab if !mods.ctrl && !mods.alt => {
                let p = &mut self.search.panel;
                let order: &[u8] = if p.show_replace { &[0, 1, 2] } else { &[0, 2] };
                let at = order.iter().position(|f| *f == p.field).unwrap_or(0);
                let n = order.len();
                p.field = order[if mods.shift { (at + n - 1) % n } else { (at + 1) % n }];
                return true;
            }
            Key::Char(c) if mods.alt && !mods.ctrl && matches!(c, 'c' | 'w' | 'r' | 'p') => {
                let id = match c {
                    'c' => "case",
                    'w' => "word",
                    'r' => "regex",
                    _ => "preserve_case",
                };
                self.search_option(id);
                return true;
            }
            Key::Char('v') if mods.ctrl && !mods.alt => {
                self.paste = true;
                self.paste_plain = true;
                return true;
            }
            Key::Enter if mods.ctrl && self.search.panel.show_replace && self.search.panel.field != 2 => {
                self.search_replace_all();
                return true;
            }
            _ => {}
        }
        match self.search.panel.field {
            0 | 1 => {
                let on_query = self.search.panel.field == 0;
                match key {
                    Key::Up | Key::Down if on_query && plain => {
                        if self.search.panel.recall(key == Key::Up) {
                            self.search_update();
                        }
                    }
                    Key::Enter if plain && on_query => {
                        self.search.panel.remember();
                        let first = self.search.panel.rows.iter().position(|r| matches!(r, Row::Match(_)));
                        let at = match self.search.panel.rows.get(self.search.panel.sel) {
                            Some(Row::Match(_)) => Some(self.search.panel.sel),
                            _ => first,
                        };
                        if let Some(at) = at {
                            self.search.panel.sel = at;
                            self.search.panel.field = 2;
                            self.search_open_row(at);
                        }
                    }
                    Key::Enter if plain => self.search_replace_all(),
                    _ => {
                        let mut changed = false;
                        let p = &mut self.search.panel;
                        let field = if on_query { &mut p.query } else { &mut p.replace };
                        if !field.key(key, mods, text, &mut changed) {
                            return false;
                        }
                        if changed && on_query {
                            self.search.panel.back = None;
                            self.search_update();
                        }
                    }
                }
            }
            _ => {
                let n = self.search.panel.rows.len();
                let sel = self.search.panel.sel.min(n.saturating_sub(1));
                let row = self.search.panel.rows.get(sel).copied();
                match key {
                    Key::Up if plain => self.search.panel.sel = sel.saturating_sub(1),
                    Key::Down if plain => self.search.panel.sel = (sel + 1).min(n.saturating_sub(1)),
                    Key::PageUp => self.search.panel.sel = sel.saturating_sub(20),
                    Key::PageDown => self.search.panel.sel = (sel + 20).min(n.saturating_sub(1)),
                    Key::Home if plain => self.search.panel.sel = 0,
                    Key::End if plain => self.search.panel.sel = n.saturating_sub(1),
                    Key::Left if plain => match row {
                        Some(Row::Match(i)) => {
                            if let Some(g) = self.search.panel.group_of(i) {
                                self.search.panel.sel =
                                    self.search.panel.rows.iter().position(|r| *r == Row::File(g)).unwrap_or(sel);
                            }
                        }
                        Some(Row::File(g)) if !self.search_group_collapsed(g) => self.search.panel.toggle(g),
                        _ => {}
                    },
                    Key::Right if plain => {
                        if let Some(Row::File(g)) = row.filter(|r| matches!(r, Row::File(g) if self.search_group_collapsed(*g))) {
                            self.search.panel.toggle(g);
                        }
                    }
                    Key::Enter if mods.ctrl => match row {
                        Some(Row::Match(i)) => self.search_replace_one(i),
                        Some(Row::File(g)) => self.search_replace_file(g),
                        None => {}
                    },
                    Key::Enter if plain => match row {
                        Some(Row::Match(_)) => self.search_open_row(sel),
                        Some(Row::File(g)) => self.search.panel.toggle(g),
                        None => {}
                    },
                    Key::Delete | Key::Backspace if plain => match row {
                        Some(Row::Match(i)) => self.search_dismiss(i, false),
                        Some(Row::File(g)) => self.search_dismiss(g, true),
                        None => {}
                    },
                    _ if plain && text.is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control)) => {
                        self.search.panel.field = 0;
                        self.search.panel.query.cur = self.search.panel.query.text.len();
                        self.search.panel.query.insert(text.unwrap_or_default());
                        self.search_update();
                    }
                    _ if !plain => return false,
                    _ => {}
                }
                self.search_panel_reveal();
            }
        }
        true
    }

    fn search_group_collapsed(&self, g: usize) -> bool {
        self.search.panel.groups.get(g).is_some_and(|g| self.search.panel.collapsed.contains(&g.path))
    }

    /// Scrolls the list to keep the chosen row in view.
    fn search_panel_reveal(&mut self) {
        let row = px(theme::ROW, self.scale);
        let p = &mut self.search.panel;
        let (_, h) = p.list;
        let top = p.sel as f32 * row;
        if top < p.scroll {
            p.scroll = top;
        } else if top + row > p.scroll + h && h > row {
            p.scroll = top + row - h;
        }
    }

    /// Opens the match of a row with it selected.
    fn search_open_row(&mut self, row: usize) {
        let Some(Row::Match(i)) = self.search.panel.rows.get(row).copied() else { return };
        let Some(f) = self.search.panel.results.get(i).cloned() else { return };
        self.search.panel.sel = row;
        self.search.panel.remember();
        self.remember_jump();
        let key = format!("{}\n{}\n{}\n{}", f.path.to_string_lossy(), f.line, f.col, f.len);
        let keys = self.search.panel.keys;
        self.search_pick(&key);
        // The list keeps the keys, so the next match is one key away.
        self.search.panel.keys = keys;
        self.focus = Focus::Doc;
    }

    fn search_dismiss(&mut self, index: usize, file: bool) {
        if file {
            self.search.panel.dismiss_group(index);
        } else {
            self.search.panel.dismiss_result(index);
        }
        self.search_panel_status();
    }

    /// Replaces one match, as one step to undo in its tab.
    fn search_replace_one(&mut self, i: usize) {
        let Some(path) = self.search.panel.results.get(i).map(|f| f.path.clone()) else { return };
        let with = self.search.panel.replace.text.clone();
        if self.plan_replacements(&with, |_, j| j == i) {
            self.search_apply(&path);
        }
    }

    /// Replaces every match listed for a file, as one step to undo.
    fn search_replace_file(&mut self, g: usize) {
        let Some(group) = self.search.panel.groups.get(g).cloned() else { return };
        let with = self.search.panel.replace.text.clone();
        if self.plan_replacements(&with, |_, j| (group.start..group.end).contains(&j)) {
            self.search_apply(&group.path);
        }
    }

    /// Lists every planned replacement to review before applying them.
    pub(super) fn search_replace_all(&mut self) {
        if self.search.panel.results.is_empty() {
            self.message = Some("Nothing to replace".into());
            return;
        }
        self.search.panel.remember();
        let with = self.search.panel.replace.text.clone();
        if self.plan_replacements(&with, |_, _| true) {
            self.review_project_replacements();
        }
    }

    /// Shows what replacing a file's matches would change.
    fn search_file_diff(&mut self, g: usize) {
        let Some(group) = self.search.panel.groups.get(g).cloned() else { return };
        let with = self.search.panel.replace.text.clone();
        if self.plan_replacements(&with, |_, j| (group.start..group.end).contains(&j)) {
            self.pick_project_replace("0");
        }
    }

    /// Applies the plans through the usual checks, then searches again, so
    /// the list shows what is left.
    fn search_apply(&mut self, path: &Path) {
        let before = self.search.replacements.len();
        self.pick_project_replace("apply");
        if self.search.replacements.is_empty() && before > 0 {
            self.message = Some(format!("Replaced in {}. Review and save the tab.", self.root_short(path)));
            self.search.asked.clear();
            self.search_update();
        }
    }

    /// A click on a control of the panel.
    pub(super) fn search_panel_hit(&mut self, action: u8, index: usize) {
        self.redraw = true;
        match action {
            act::QUERY => self.search_panel_focus(0),
            act::REPLACE => self.search_panel_focus(1),
            act::TOGGLE => {
                let p = &mut self.search.panel;
                p.show_replace = !p.show_replace;
                self.search_panel_focus(if self.search.panel.show_replace { 1 } else { 0 });
            }
            act::CASE => self.search_option("case"),
            act::WORD => self.search_option("word"),
            act::REGEX => self.search_option("regex"),
            act::PRESERVE => self.search_option("preserve_case"),
            act::INCLUDE => self.search_option("include"),
            act::EXCLUDE => self.search_option("exclude"),
            act::COLLAPSE => self.search.panel.toggle_all(),
            act::REFRESH => {
                self.search.asked.clear();
                self.search_update();
            }
            act::REPLACE_ALL => self.search_replace_all(),
            act::EDITOR => self.multibuffer_search(),
            act::FILE => {
                self.search.panel.toggle(index);
                self.search_panel_focus(2);
            }
            act::FILE_REPLACE => self.search_replace_file(index),
            act::FILE_DISMISS => self.search_dismiss(index, true),
            act::FILE_DIFF => self.search_file_diff(index),
            act::MATCH => {
                if let Some(row) = self.search.panel.rows.iter().position(|r| *r == Row::Match(index)) {
                    self.search_panel_focus(2);
                    self.search_open_row(row);
                }
            }
            act::MATCH_REPLACE => self.search_replace_one(index),
            act::MATCH_DISMISS => self.search_dismiss(index, false),
            _ => {}
        }
    }

    pub(super) fn draw_search_panel(&mut self, c: &mut Canvas) {
        let (x, y, w, h) = self.search.panel.rect;
        let s = self.scale;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let keys = self.search_panel_has_keys();
        let field = self.search.panel.field;
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());
        let pad = px(theme::PAD, s);
        let gap = px(2.0, s);
        let btn = px(theme::SMALL_CONTROL, s);
        let fh = px(theme::CONTROL, s);
        let small = px(theme::SMALL, s);
        let size = px(theme::UI_TEXT, s);
        let left = x + px(6.0, s);
        let right = x + w - pad;
        let fx = left + btn + px(4.0, s);
        let mut cy = y + px(8.0, s);

        // A small toggle drawn as text, as in the find bar.
        let toggle = |app: &mut App, c: &mut Canvas, fonts: &mut Fonts, b: Rect, label: &str, on: bool, act: u8, tip: &str, keys: &str| {
            let hot = ui::inside(b, Some(app.pointer)) && !app.overlaid();
            if on {
                c.round_rect(b.0, b.1, b.2, b.3, px(theme::RADIUS_MD, s), theme::selection());
            } else if hot {
                c.round_rect(b.0, b.1, b.2, b.3, px(theme::RADIUS_MD, s), theme::hover());
            }
            let tw = fonts.width(font::UI, small, label);
            let ink = if on { theme::fg() } else { theme::dim() };
            ui::text(c, fonts, b.0 + (b.2 - tw) / 2.0, ui::base(b.1, b.3, small), label, font::UI, small, ink);
            app.push_hit(b, Hit::Search(act, 0), tip, keys);
        };

        // The query, with the chevron that shows the replacement and the
        // options at its right.
        let chevron = (left, cy + (fh - btn) / 2.0, btn, btn);
        let icon = if self.search.panel.show_replace { Icon::ChevronDown } else { Icon::ChevronRight };
        ui::icon_button(c, chevron, icon, ui::inside(chevron, Some(self.pointer)) && !self.overlaid(), false, s);
        self.push_hit(chevron, Hit::Search(act::TOGGLE, 0), "Toggle replace", "");
        let options = w > px(260.0, s);
        let room = if options { (btn + gap) * 3.0 } else { 0.0 };
        let qf = (fx, cy, (right - fx - room).max(px(40.0, s)), fh);
        let (q, qc) = (self.search.panel.query.text.clone(), self.search.panel.query.cur);
        self.find_field(c, fonts, qf, &q, qc, keys && field == 0, "Search", 0.0);
        self.push_hit(qf, Hit::Search(act::QUERY, 0), "", "");
        if options {
            let mut bx = qf.0 + qf.2 + gap;
            let o = self.search_options.clone();
            for (label, on, a, tip, k) in [
                ("Aa", o.case, act::CASE, "Match case", "Alt+C"),
                ("ab", o.word, act::WORD, "Whole words", "Alt+W"),
                (".*", o.regex, act::REGEX, "Regular expression", "Alt+R"),
            ] {
                toggle(self, c, fonts, (bx, cy + (fh - btn) / 2.0, btn, btn), label, on, a, tip, k);
                bx += btn + gap;
            }
        }
        cy += fh + px(6.0, s);

        // The replacement, with preserve case and Replace all.
        if self.search.panel.show_replace {
            let room = (btn + gap) * 2.0;
            let rf = (fx, cy, (right - fx - room).max(px(40.0, s)), fh);
            let (r, rc) = (self.search.panel.replace.text.clone(), self.search.panel.replace.cur);
            self.find_field(c, fonts, rf, &r, rc, keys && field == 1, "Replace", 0.0);
            self.push_hit(rf, Hit::Search(act::REPLACE, 0), "", "");
            let bx = rf.0 + rf.2 + gap;
            let preserve = self.search_options.preserve_case;
            toggle(self, c, fonts, (bx, cy + (fh - btn) / 2.0, btn, btn), "aA", preserve, act::PRESERVE, "Preserve case", "Alt+P");
            let b = (bx + btn + gap, cy + (fh - btn) / 2.0, btn, btn);
            let hot = ui::inside(b, Some(self.pointer)) && !self.overlaid();
            ui::icon_button(c, b, Icon::ReplaceAll, hot, false, s);
            self.push_hit(b, Hit::Search(act::REPLACE_ALL, 0), "Replace all, after review", "Ctrl+Enter");
            cy += fh + px(6.0, s);
        }

        // The path filters, which open the prompts that set them.
        let rh = px(22.0, s);
        for (label, globs, a, tip) in [
            ("Include", self.search_options.include.clone(), act::INCLUDE, "Search: Set included paths"),
            ("Exclude", self.search_options.exclude.clone(), act::EXCLUDE, "Search: Set excluded paths"),
        ] {
            let r = (fx - px(4.0, s), cy, right - fx + px(4.0, s), rh);
            let hot = ui::inside(r, Some(self.pointer)) && !self.overlaid();
            ui::row_ground(c, r, false, hot, false, s);
            let lw = ui::text(c, fonts, r.0 + px(4.0, s), ui::base(r.1, r.3, small), label, font::UI_MEDIUM, small, theme::dim());
            let (shown, ink) = if globs.trim().is_empty() {
                (if a == act::INCLUDE { "all files" } else { "nothing" }.to_string(), theme::faint())
            } else {
                (globs, theme::fg())
            };
            let tx = r.0 + px(4.0, s) + lw + px(8.0, s);
            let shown = ui::fit(fonts, font::UI, small, &shown, (r.0 + r.2 - tx - px(4.0, s)).max(0.0));
            ui::text(c, fonts, tx, ui::base(r.1, r.3, small), &shown, font::UI, small, ink);
            self.push_hit(r, Hit::Search(a, 0), tip, "");
            cy += rh;
        }
        cy += px(4.0, s);

        // What was found, with collapse and refresh at the right.
        let sh = btn;
        let mut bx = right;
        for (icon, a, tip) in [
            (Icon::Refresh, act::REFRESH, "Search again"),
            (Icon::ChevronsDownUp, act::COLLAPSE, "Collapse or expand all"),
            (Icon::SquarePen, act::EDITOR, "Open results in editor"),
        ] {
            bx -= btn;
            let b = (bx, cy, btn, btn);
            let hot = ui::inside(b, Some(self.pointer)) && !self.overlaid();
            ui::icon_button(c, b, icon, hot, false, s);
            self.push_hit(b, Hit::Search(a, 0), tip, "");
            bx -= gap;
        }
        let status = self.search.panel.status.clone();
        let ink = if self.search.more { theme::warning() } else { theme::dim() };
        let status = ui::fit(fonts, font::UI, small, &status, (bx - fx).max(0.0));
        ui::text(c, fonts, left + px(6.0, s), ui::base(cy, sh, small), &status, font::UI, small, ink);
        cy += sh + px(6.0, s);
        c.line(x, cy, w, 1.0, theme::line());
        cy += 1.0;

        // The list, drawing only the rows in view.
        let top = cy;
        let lh = (y + h - top).max(0.0);
        let row = px(theme::ROW, s);
        let total = self.search.panel.rows.len() as f32 * row;
        let p = &mut self.search.panel;
        p.list = (top, lh);
        p.scroll = p.scroll.min((total - lh + px(8.0, s)).max(0.0)).max(0.0);
        let scroll = p.scroll;
        c.clip(x, top, w, lh);
        let first = (scroll / row).floor() as usize;
        let shown = (lh / row).ceil() as usize + 1;
        let with = self.search.panel.show_replace.then(|| self.search.panel.replace.text.clone());
        let pattern = with.as_ref().and_then(|_| self.search_options.pattern(&self.search.asked).ok());
        let rows: Vec<(usize, Row)> = self.search.panel.rows.iter().copied().enumerate().skip(first).take(shown).collect();
        let sel = self.search.panel.sel;
        for (i, r) in rows {
            let ry = top + i as f32 * row - scroll + px(4.0, s);
            let rr = (x + px(4.0, s), ry, w - px(8.0, s), row);
            let hot = ui::inside(rr, Some(self.pointer)) && !self.overlaid() && self.pointer.1 >= top;
            let chosen = i == sel && field == 2 && keys;
            ui::row_ground(c, rr, chosen, hot, chosen, s);
            let mid = ry + row / 2.0;
            // The buttons a row shows while it is pointed at or chosen.
            let mut buttons: Vec<(Icon, u8, usize, &str)> = Vec::new();
            match r {
                Row::File(g) => {
                    let Some(group) = self.search.panel.groups.get(g).cloned() else { continue };
                    let open = !self.search.panel.collapsed.contains(&group.path);
                    let isz = px(theme::SMALL_ICON, s);
                    let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
                    icons::draw(c, icon, rr.0 + px(4.0, s), (mid - isz / 2.0).round(), isz, theme::icon());
                    let name = self.root_short(&group.path);
                    let (dir, file) = match name.rsplit_once('/') {
                        Some((d, f)) => (d.to_string(), f.to_string()),
                        None => (String::new(), name.clone()),
                    };
                    let count = (group.end - group.start).to_string();
                    let cw = fonts.width(font::UI_MEDIUM, px(theme::SMALL - 1.0, s), &count) + px(10.0, s);
                    if hot || chosen {
                        if with.is_some() {
                            buttons.push((Icon::Eye, act::FILE_DIFF, g, "Preview the file's replacements"));
                            buttons.push((Icon::ReplaceAll, act::FILE_REPLACE, g, "Replace in this file"));
                        }
                        buttons.push((Icon::X, act::FILE_DISMISS, g, "Dismiss"));
                    }
                    let end = rr.0 + rr.2 - px(4.0, s) - if buttons.is_empty() { cw } else { (btn + gap) * buttons.len() as f32 };
                    let tx = rr.0 + px(4.0, s) + isz + px(6.0, s);
                    let file = ui::fit(fonts, font::UI_MEDIUM, size, &file, (end - tx - px(6.0, s)).max(0.0));
                    let fw = ui::text(c, fonts, tx, ui::base(ry, row, size), &file, font::UI_MEDIUM, size, theme::fg());
                    let dx = tx + fw + px(6.0, s);
                    if !dir.is_empty() && dx < end - px(20.0, s) {
                        let dir = ui::fit_start(fonts, font::UI, small, &dir, (end - dx - px(6.0, s)).max(0.0));
                        ui::text(c, fonts, dx, ui::base(ry, row, small), &dir, font::UI, small, theme::faint());
                    }
                    if buttons.is_empty() {
                        ui::badge(c, fonts, rr.0 + rr.2 - px(4.0, s) - cw + px(2.0, s), mid, &count, theme::dim(), s);
                    }
                    self.push_hit(rr, Hit::Search(act::FILE, g), "", "");
                }
                Row::Match(m) => {
                    let Some(f) = self.search.panel.results.get(m).cloned() else { continue };
                    if hot || chosen {
                        if with.is_some() {
                            buttons.push((Icon::Replace, act::MATCH_REPLACE, m, "Replace"));
                        }
                        buttons.push((Icon::X, act::MATCH_DISMISS, m, "Dismiss"));
                    }
                    let line = format!("{}", f.line);
                    let lw = fonts.width(font::UI, small, &line);
                    let end = rr.0 + rr.2 - px(6.0, s) - if buttons.is_empty() { lw + px(4.0, s) } else { (btn + gap) * buttons.len() as f32 };
                    let tx = rr.0 + px(26.0, s);
                    let replacement = with.as_ref().map(|w| match &pattern {
                        Some(re) => preview_replacement(re, &self.search_options, &f, w),
                        None => w.clone(),
                    });
                    c.clip(tx, ry, (end - tx).max(0.0), row);
                    draw_match(c, fonts, (tx, ry, row), &f, replacement.as_deref(), size, s);
                    c.unclip();
                    c.clip(x, top, w, lh);
                    if buttons.is_empty() {
                        ui::text(c, fonts, rr.0 + rr.2 - px(6.0, s) - lw, ui::base(ry, row, small), &line, font::UI, small, theme::faint());
                    }
                    self.push_hit(rr, Hit::Search(act::MATCH, m), "", "");
                }
            }
            // Pushed after the row, so they win over it.
            let n = buttons.len();
            for (j, b) in buttons.iter().enumerate() {
                let bx = rr.0 + rr.2 - px(2.0, s) - (btn + gap) * (n - j) as f32 + gap;
                let br = (bx, rr.1 + (rr.3 - btn) / 2.0, btn, btn);
                let bhot = ui::inside(br, Some(self.pointer));
                ui::icon_button(c, br, b.0, bhot, false, s);
                let k = match b.1 {
                    act::MATCH_REPLACE | act::FILE_REPLACE => "Ctrl+Enter",
                    act::MATCH_DISMISS | act::FILE_DISMISS => "Delete",
                    _ => "",
                };
                self.push_hit(br, Hit::Search(b.1, b.2), b.3, k);
            }
        }
        if total > lh && lh > 0.0 {
            let track = (x + w - px(6.0, s), top, px(4.0, s), lh);
            ui::scrollbar(c, track, scroll / total, lh / total, false, s);
        }
        c.unclip();
        c.unclip();
    }
}

/// What a listed match becomes, worked out from the part of the line kept.
fn preview_replacement(re: &regex::Regex, options: &super::replace::Options, f: &Found, with: &str) -> String {
    let at = f.col.saturating_sub(f.cut);
    re.captures_at(&f.text, at)
        .filter(|c| c.get(0).is_some_and(|m| m.start() == at))
        .map(|c| options.expand(&c, with))
        .unwrap_or_else(|| with.to_string())
}

/// A match in its line: a little of what comes before, the match on the
/// found ground or, while replacing, struck out in red before the
/// replacement in green, and the rest of the line.
fn draw_match(c: &mut Canvas, fonts: &mut Fonts, (x, y, h): (f32, f32, f32), f: &Found, with: Option<&str>, size: f32, s: f32) {
    let t = &f.text;
    let a = editor::floor(t, f.col.saturating_sub(f.cut).min(t.len()));
    let b = editor::floor(t, (a + f.len).min(t.len()));
    // Some of what comes before, without the indent.
    let lead = t[..a].trim_start();
    let lead_at = a - lead.len();
    let keep = lead.char_indices().rev().nth(24).map_or(lead_at, |(i, _)| lead_at + i);
    let before = if keep > lead_at { format!("\u{2026}{}", &t[keep..a]) } else { t[lead_at..a].to_string() };
    let base = ui::base(y, h, size);
    let mut cx = x;
    cx += ui::text(c, fonts, cx, base, &before, font::UI, size, theme::dim());
    let hit = &t[a..b];
    let hw = fonts.width(font::UI, size, hit);
    match with {
        Some(with) => {
            c.round_rect(cx, y + px(3.0, s), hw, h - px(6.0, s), px(theme::RADIUS_SM, s), theme::wash(theme::danger()));
            ui::text(c, fonts, cx, base, hit, font::UI, size, theme::fg());
            c.rect(cx, (y + h / 2.0).round(), hw, 1.0_f32.max(s), theme::danger());
            cx += hw;
            let ww = fonts.width(font::UI, size, with);
            c.round_rect(cx, y + px(3.0, s), ww, h - px(6.0, s), px(theme::RADIUS_SM, s), theme::wash(theme::success()));
            ui::text(c, fonts, cx, base, with, font::UI, size, theme::fg());
            cx += ww;
        }
        None => {
            c.round_rect(cx, y + px(3.0, s), hw, h - px(6.0, s), px(theme::RADIUS_SM, s), theme::found());
            ui::text(c, fonts, cx, base, hit, font::UI, size, theme::fg());
            cx += hw;
        }
    }
    let rest: String = t[b..].chars().take(200).collect();
    ui::text(c, fonts, cx, base, &rest, font::UI, size, theme::dim());
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };

    fn found(path: &str, line: usize) -> Found {
        Found { path: PathBuf::from(path), line, col: 0, len: 1, text: "x".into(), cut: 0 }
    }

    #[test]
    fn results_group_by_file_collapse_and_dismiss() {
        let mut p = SearchPanel::default();
        p.append(vec![found("a", 1), found("a", 2)]);
        p.append(vec![found("a", 3), found("b", 1)]);
        assert_eq!(p.groups.len(), 2);
        assert_eq!((p.groups[0].start, p.groups[0].end, p.groups[1].start), (0, 3, 3));
        assert_eq!(p.rows.len(), 6);
        p.toggle(0);
        assert_eq!(p.rows, [Row::File(0), Row::File(1), Row::Match(3)]);
        p.toggle_all();
        assert_eq!(p.rows, [Row::File(0), Row::File(1)]);
        p.toggle_all();
        p.dismiss_result(1);
        assert_eq!(p.groups[0].end, 2);
        p.dismiss_group(0);
        assert_eq!(p.results.len(), 1);
        assert_eq!(p.rows, [Row::File(0), Row::Match(0)]);
    }

    #[test]
    fn history_goes_back_and_returns_to_what_was_typed() {
        let mut p = SearchPanel::default();
        for q in ["one", "two", "one"] {
            p.set_query(q);
            p.remember();
        }
        p.set_query("draft");
        assert!(p.recall(true));
        assert_eq!(p.query.text, "one");
        assert!(p.recall(true));
        assert_eq!(p.query.text, "two");
        assert!(p.recall(false));
        assert!(p.recall(false));
        assert_eq!(p.query.text, "draft");
        assert!(!p.recall(false));
    }

    #[test]
    fn fields_edit_by_character_and_word() {
        let mut f = Field::default();
        let mut changed = false;
        for ch in ["a", "b", " ", "c"] {
            f.key(Key::Other, NONE, Some(ch), &mut changed);
        }
        assert_eq!(f.text, "ab c");
        f.key(Key::Backspace, Mods { ctrl: true, ..NONE }, None, &mut changed);
        assert_eq!(f.text, "ab ");
        f.key(Key::Home, NONE, None, &mut changed);
        f.key(Key::Delete, NONE, None, &mut changed);
        assert_eq!((f.text.as_str(), f.cur), ("b ", 0));
    }

    fn typed(a: &mut App, s: &str) {
        for ch in s.chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
    }

    #[test]
    fn the_panel_replaces_one_match_then_a_file_as_undoable_edits() {
        let dir = std::env::temp_dir().join(format!("ld-search-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.txt"), "cat cat\nno\ncat\n").unwrap();
        std::fs::write(dir.join("b.txt"), "a cat\n").unwrap();
        let mut a = App::new(Some(dir.join("b.txt")));
        a.resize(1100, 700, 1.0);
        // Ctrl+Shift+F opens the panel with the word selected.
        a.doc.select(2, 5);
        a.key(Key::Char('f'), Mods { ctrl: true, shift: true, alt: false }, None);
        assert!(a.search_panel_has_keys());
        assert_eq!(a.search.panel.query.text, "cat");
        a.search_settle();
        assert_eq!(a.search.panel.results.len(), 4);
        assert_eq!(a.search.panel.status, "4 results in 2 files");
        // The open tab is searched from its buffer.
        a.doc.select(0, 0);
        a.doc.insert("cat ", crate::editor::EditKind::Other);
        a.search_panel_hit(act::REFRESH, 0);
        a.search_settle();
        assert_eq!(a.search.panel.results.len(), 5);
        // Replace the second match of a.txt only.
        a.search_panel_hit(act::TOGGLE, 0);
        typed(&mut a, "dog");
        let second = a.search.panel.results.iter().position(|f| f.path.ends_with("a.txt") && f.col == 4).unwrap();
        a.search_replace_one(second);
        assert_eq!(a.doc.path.as_deref(), Some(dir.join("a.txt").as_path()));
        assert_eq!(a.doc.buf.text, "cat dog\nno\ncat\n");
        assert!(a.doc.buf.modified);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "cat cat\nno\ncat\n");
        a.search_settle();
        assert_eq!(a.search.panel.results.len(), 4);
        // Then the rest of the file, from its edited buffer.
        let g = a.search.panel.groups.iter().position(|g| g.path.ends_with("a.txt")).unwrap();
        a.search_replace_file(g);
        assert_eq!(a.doc.buf.text, "dog dog\nno\ndog\n");
        a.key(Key::Escape, NONE, None);
        a.key(Key::Char('z'), Mods { ctrl: true, ..NONE }, None);
        assert_eq!(a.doc.buf.text, "cat dog\nno\ncat\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn up_recalls_a_query_and_the_list_takes_arrow_keys() {
        let dir = std::env::temp_dir().join(format!("ld-search-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha\nbeta\nalpha\n").unwrap();
        let mut a = App::new(Some(dir.join("a.txt")));
        a.resize(1100, 700, 1.0);
        a.open_search();
        typed(&mut a, "alpha");
        a.search_settle();
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.doc.buf.sel(), (0, 5));
        assert_eq!(a.search.panel.field, 2);
        a.key(Key::Down, NONE, None);
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.doc.buf.sel(), (11, 16));
        a.key(Key::Tab, NONE, None);
        for _ in 0..5 {
            a.key(Key::Backspace, NONE, None);
        }
        typed(&mut a, "beta");
        a.key(Key::Up, NONE, None);
        assert_eq!(a.search.panel.query.text, "alpha");
        a.key(Key::Down, NONE, None);
        assert_eq!(a.search.panel.query.text, "beta");
        // Drawing lists the file and its matches as hits.
        let mut px = vec![0; (a.w * a.h) as usize];
        a.search_settle();
        a.draw(&mut Canvas::new(&mut px, a.w as i32, a.h as i32));
        assert!(a.hits.iter().any(|(_, h)| matches!(h, Hit::Search(act::MATCH, _))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
