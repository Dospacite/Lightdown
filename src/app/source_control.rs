//! The Source Control view in the left sidebar: the repository's changed
//! files in sections, a commit message and the buttons that act on them.
//! Git work goes through `git_actions`, on its worker and with its checks.
use super::git::{Change, Status};
use super::git_actions::Action;
use super::*;
use crate::icons::{self, Icon};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Section {
    Conflicted,
    Staged,
    Unstaged,
    Untracked,
    /// Heads a repository's sections, in a window of several folders.
    Repository,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Conflicted => "Merge conflicts",
            Section::Staged => "Staged changes",
            Section::Unstaged => "Changes",
            Section::Untracked => "Untracked files",
            Section::Repository => "Repository",
        }
    }
}

/// A row of the list: a section's head, or one of its files.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Row {
    pub section: Section,
    pub path: Option<PathBuf>,
    pub change: Option<Change>,
    pub count: usize,
    /// The window's folder whose repository the row is of, when it has
    /// several; else the active one's.
    pub repo: Option<PathBuf>,
}

/// What a row's small buttons do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Button {
    Stage,
    Unstage,
    Discard,
    Open,
}

/// The view's own state; what it lists comes from the git polling.
#[derive(Default)]
pub(super) struct View {
    pub row: usize,
    scroll: f32,
    pub message: String,
    pub cur: usize,
    /// The message field has the keys.
    pub typing: bool,
    /// A commit message being committed, to restore if the commit fails.
    pub sent: Option<String>,
    /// A drafted message goes into the field rather than a prompt.
    pub drafting: bool,
}

/// The rows for a status: sections that hold files, in a fixed order, each
/// with its files sorted by path.
pub(super) fn rows(status: Option<&Status>) -> Vec<Row> {
    let mut rows = Vec::new();
    let Some(status) = status else { return rows };
    for section in [Section::Conflicted, Section::Staged, Section::Unstaged, Section::Untracked] {
        let mut files: Vec<(&PathBuf, Change)> = status
            .changes
            .iter()
            .filter(|(p, change)| match section {
                Section::Conflicted => **change == Change::Conflict,
                Section::Untracked => **change == Change::Untracked,
                Section::Staged => status.staged.contains(*p) && !matches!(change, Change::Conflict | Change::Untracked),
                Section::Unstaged => {
                    status.unstaged.contains(*p) && !matches!(change, Change::Conflict | Change::Untracked)
                }
                Section::Repository => false,
            })
            .map(|(p, c)| (p, *c))
            .collect();
        if files.is_empty() {
            continue;
        }
        files.sort_by(|a, b| a.0.cmp(b.0));
        rows.push(Row { section, path: None, change: None, count: files.len(), repo: None });
        for (path, change) in files {
            // A staged addition that has since changed on disk reads as modified.
            let change = if section == Section::Unstaged && change == Change::Added { Change::Modified } else { change };
            rows.push(Row { section, path: Some(path.clone()), change: Some(change), count: 0, repo: None });
        }
    }
    rows
}

/// The rows of several repositories, each under a head that names its
/// folder and counts its changed files.
pub(super) fn grouped(groups: &[(PathBuf, Option<&Status>)]) -> Vec<Row> {
    let mut out = Vec::new();
    for (root, status) in groups {
        let count = status.map_or(0, |s| s.changes.len());
        out.push(Row { section: Section::Repository, path: None, change: None, count, repo: Some(root.clone()) });
        out.extend(rows(*status).into_iter().map(|r| Row { repo: Some(root.clone()), ..r }));
    }
    out
}

/// The files of a section, of the same repository.
fn section_paths(rows: &[Row], section: Section, repo: &Option<PathBuf>) -> Vec<PathBuf> {
    rows.iter().filter(|r| r.section == section && r.repo == *repo).filter_map(|r| r.path.clone()).collect()
}

/// The buttons a row offers, from the right.
pub(super) fn buttons(row: &Row) -> &'static [Button] {
    match (row.section, row.path.is_some()) {
        (Section::Repository, _) => &[],
        (Section::Staged, _) => &[Button::Unstage],
        (Section::Conflicted, true) => &[Button::Stage, Button::Open],
        (Section::Conflicted, false) => &[Button::Stage],
        (_, true) => &[Button::Stage, Button::Discard, Button::Open],
        (_, false) => &[Button::Stage, Button::Discard],
    }
}

fn button_look(button: Button, section: Section, head: bool) -> (Icon, &'static str, &'static str) {
    match (button, head) {
        (Button::Stage, false) if section == Section::Conflicted => (Icon::Check, "Mark resolved and stage", "Space"),
        (Button::Stage, false) => (Icon::Plus, "Stage file", "Space"),
        (Button::Stage, true) => (Icon::Plus, "Stage all in section", "Space"),
        (Button::Unstage, false) => (Icon::Minus, "Unstage file", "Space"),
        (Button::Unstage, true) => (Icon::Minus, "Unstage all", "Space"),
        (Button::Discard, false) if section == Section::Untracked => (Icon::Trash, "Delete file…", "Delete"),
        (Button::Discard, true) if section == Section::Untracked => (Icon::Trash, "Delete all untracked files…", "Delete"),
        (Button::Discard, false) => (Icon::RotateCcw, "Discard changes…", "Delete"),
        (Button::Discard, true) => (Icon::RotateCcw, "Discard all changes…", "Delete"),
        (Button::Open, _) => (Icon::File, "Open file", "Alt+Enter"),
    }
}

impl App {
    /// Whether the left sidebar shows Source Control.
    pub(super) fn source_shown(&self) -> bool {
        self.cfg.source_control && self.files_open
    }

    /// Shows the explorer or Source Control in the left sidebar, with the
    /// keys, and remembers the choice.
    pub(super) fn source_view(&mut self, on: bool) {
        if self.cfg.source_control != on {
            self.cfg.source_control = on;
            let _ = self.cfg.save();
        }
        self.source.typing = false;
        if !self.files_open || self.files_width() <= 0.0 {
            self.toggle_side(Tab::Files);
        }
        self.focus = Focus::Files;
        if on {
            self.git_stale();
        }
        self.redraw = true;
    }

    /// Goes to Source Control, or back to the document from it.
    pub(super) fn source_toggle(&mut self) {
        if self.source_shown() && self.focus == Focus::Files && self.files_width() > 0.0 {
            self.source.typing = false;
            self.focus = Focus::Doc;
            self.redraw = true;
        } else {
            self.source_view(true);
        }
    }

    /// Goes to the explorer and back; from Source Control, to the explorer.
    pub(super) fn explorer_toggle(&mut self) {
        if self.source_shown() && self.files_width() > 0.0 {
            self.source_view(false);
        } else {
            self.toggle_side(Tab::Files);
        }
    }

    fn source_rows(&self) -> Vec<Row> {
        if !self.multi_root() {
            return rows(self.git.status.as_ref());
        }
        let status = |root: &PathBuf| match root == &self.root {
            true => self.git.status.as_ref(),
            false => self.git.others.iter().find(|(d, _)| d == root).and_then(|(_, s)| s.as_ref()),
        };
        // Folders outside any repository have nothing to list.
        let groups: Vec<_> = self.folders.iter().filter_map(|r| status(r).map(|s| (r.clone(), Some(s)))).collect();
        grouped(&groups)
    }

    /// Runs Source Control work in the repository of a row.
    fn source_in<T>(&mut self, repo: &Option<PathBuf>, f: impl FnOnce(&mut App) -> T) -> T {
        match repo.clone().filter(|r| *r != self.root) {
            Some(r) => self.with_root(&r, f),
            None => f(self),
        }
    }

    /// Starts Git work from the view. Its results don't reopen the palette's
    /// source-control list.
    fn source_git(&mut self, action: Action) {
        self.git_actions.quiet = true;
        self.git_action(action);
        if !self.git_actions.busy() {
            self.git_actions.quiet = false;
        }
    }

    /// Carries out a row's button, or its section's on a head.
    pub(super) fn source_button(&mut self, i: usize, button: Button) {
        let rows = self.source_rows();
        let Some(row) = rows.get(i).cloned() else { return };
        self.source.row = i;
        let paths = match &row.path {
            Some(p) => vec![p.clone()],
            None => section_paths(&rows, row.section, &row.repo),
        };
        if paths.is_empty() {
            return;
        }
        if row.repo.as_ref().is_some_and(|r| *r != self.root) {
            let repo = row.repo.clone();
            let row = Row { repo: None, ..row };
            return self.source_in(&repo, |a| a.source_row_button(row, paths, button));
        }
        self.source_row_button(row, paths, button);
    }

    fn source_row_button(&mut self, row: Row, paths: Vec<PathBuf>, button: Button) {
        match button {
            Button::Stage => self.source_git(Action::Stage(paths, false)),
            Button::Unstage => self.source_git(Action::Stage(paths, true)),
            Button::Discard => self.source_git(Action::DiscardPrepare(Some(paths))),
            Button::Open => {
                if let Some(p) = row.path {
                    self.open_tab(p);
                }
            }
        }
    }

    /// Opens what a row shows: a file's diff, or folds nothing on a head.
    pub(super) fn source_open(&mut self, i: usize) {
        let rows = self.source_rows();
        let Some(row) = rows.get(i).cloned() else { return };
        self.source.row = i;
        let Some(path) = row.path else { return };
        if row.repo.as_ref().is_some_and(|r| *r != self.root) && row.section != Section::Conflicted {
            let repo = row.repo.clone();
            return self.source_in(&repo, |a| match row.section {
                Section::Staged => a.source_git(Action::StagedDiff(path)),
                Section::Untracked => a.source_git(Action::SavedDiff(path, false)),
                _ => a.source_git(Action::SavedDiff(path, true)),
            });
        }
        match row.section {
            Section::Repository => {}
            Section::Conflicted => self.open_tab(path),
            Section::Staged => self.source_git(Action::StagedDiff(path)),
            // Changes compare with the index; untracked files with nothing.
            Section::Unstaged => self.source_git(Action::SavedDiff(path, true)),
            Section::Untracked => self.source_git(Action::SavedDiff(path, false)),
        }
    }

    /// Commits the staged changes with the message in the field.
    pub(super) fn source_commit(&mut self) {
        let message = self.source.message.trim().to_string();
        if message.is_empty() {
            self.source.typing = true;
            self.focus = Focus::Files;
            self.message = Some("Type a commit message first".into());
            return;
        }
        // In a window of several folders, the chosen row's repository.
        let repo = self.source_rows().get(self.source.row).and_then(|r| r.repo.clone()).filter(|r| *r != self.root);
        if let Some(repo) = repo {
            let staged = self.git.others.iter().find(|(d, _)| *d == repo).and_then(|(_, s)| s.as_ref()).is_some_and(|s| !s.staged.is_empty());
            if !staged {
                self.message = Some("Stage changes before committing".into());
                return;
            }
            return self.with_root(&repo, |a| a.source_commit_here(message));
        }
        if self.git.status.as_ref().is_some_and(|s| s.staged.is_empty()) {
            self.message = Some("Stage changes before committing".into());
            return;
        }
        self.source_commit_here(message);
    }

    fn source_commit_here(&mut self, message: String) {
        self.source.typing = false;
        self.git_actions.panel_commit = Some(message.clone());
        self.source_git(Action::CommitPrompt);
        if self.git_actions.busy() {
            self.source.sent = Some(std::mem::take(&mut self.source.message));
            self.source.cur = 0;
        } else {
            self.git_actions.panel_commit = None;
        }
    }

    /// Amends HEAD; the review prompt opens with the field's message, or
    /// HEAD's subject when the field is empty.
    pub(super) fn source_amend(&mut self) {
        self.source.typing = false;
        self.source_git(Action::AmendPrompt);
    }

    /// Asks the suggestion model for a message, which lands in the field.
    pub(super) fn source_draft(&mut self) {
        self.source.drafting = true;
        self.source_git(Action::Draft);
        if !self.git_actions.busy() {
            self.source.drafting = false;
        }
    }

    /// A drafted message, put into the field for review.
    pub(super) fn source_drafted(&mut self, text: &str) {
        self.source.message = text.to_string();
        self.source.cur = self.source.message.len();
        self.source.typing = true;
        self.focus = Focus::Files;
        self.message = Some("Review the draft, then commit with Enter".into());
    }

    /// Text pasted while the message field has the keys.
    pub(super) fn source_paste(&mut self, s: &str) -> bool {
        if !(self.source.typing && self.source_shown() && self.focus == Focus::Files) {
            return false;
        }
        let line = s.lines().next().unwrap_or("");
        self.source.message.insert_str(self.source.cur, line);
        self.source.cur += line.len();
        self.redraw = true;
        true
    }

    pub(super) fn source_key(&mut self, key: Key, mods: Mods, text: Option<&str>) {
        self.redraw = true;
        if key == Key::Enter && mods.ctrl {
            return self.source_commit();
        }
        if self.source.typing {
            let v = &mut self.source;
            match key {
                Key::Escape | Key::Tab => v.typing = false,
                Key::Down => v.typing = false,
                Key::Enter => self.source_commit(),
                Key::Left => v.cur = crate::bidi::visual_move(&v.message,v.cur,false),
                Key::Right => v.cur = crate::bidi::visual_move(&v.message,v.cur,true),
                Key::Home => v.cur = 0,
                Key::End => v.cur = v.message.len(),
                Key::Backspace | Key::Delete => {
                    let (a, b) = match (key == Key::Delete, mods.ctrl) {
                        (true, _) => (v.cur, editor::next(&v.message, v.cur)),
                        (false, true) => (editor::word_left(&v.message, v.cur), v.cur),
                        (false, false) => (editor::prev(&v.message, v.cur), v.cur),
                    };
                    v.message.replace_range(a..b, "");
                    v.cur = a;
                }
                Key::Char('v') if mods.ctrl => {
                    self.paste = true;
                    self.paste_plain = true;
                }
                _ => {
                    if let Some(s) = text.filter(|s| !mods.ctrl && !mods.alt && !s.chars().any(char::is_control)) {
                        v.message.insert_str(v.cur, s);
                        v.cur += s.len();
                    }
                }
            }
            return;
        }
        let rows = self.source_rows();
        let len = rows.len();
        let row = rows.get(self.source.row).cloned();
        match key {
            Key::Up if self.source.row == 0 => self.source.typing = true,
            Key::Up => self.source.row -= 1,
            Key::Down => self.source.row = (self.source.row + 1).min(len.saturating_sub(1)),
            Key::Home => self.source.row = 0,
            Key::End => self.source.row = len.saturating_sub(1),
            Key::Tab => self.source.typing = true,
            Key::Enter if mods.alt => self.source_button(self.source.row, Button::Open),
            Key::Enter => self.source_open(self.source.row),
            Key::Char(' ') => {
                if let Some(row) = row {
                    let b = if row.section == Section::Staged { Button::Unstage } else { Button::Stage };
                    self.source_button(self.source.row, b);
                }
            }
            Key::Delete | Key::Backspace => {
                if row.is_some_and(|r| buttons(&r).contains(&Button::Discard)) {
                    self.source_button(self.source.row, Button::Discard);
                }
            }
            Key::Escape => self.focus = Focus::Doc,
            _ => {}
        }
        self.source.row = self.source.row.min(len.saturating_sub(1));
        self.source_reveal();
    }

    fn source_list_top(&self) -> f32 {
        let s = self.scale;
        let (_, y, _, _) = self.tree.rect();
        y + px(8.0 + theme::CONTROL + 8.0 + theme::CONTROL + 10.0, s)
    }

    fn source_row_h(&self) -> f32 {
        px(theme::ROW, self.scale)
    }

    fn source_reveal(&mut self) {
        let (_, y, _, h) = self.tree.rect();
        let top = self.source_list_top();
        let tall = (y + h - top).max(0.0);
        let rh = self.source_row_h();
        let at = self.source.row as f32 * rh;
        self.source.scroll = self.source.scroll.max(at + rh - tall).min(at).max(0.0);
    }

    pub(super) fn source_scroll(&mut self, dy: f32) {
        let (_, y, _, h) = self.tree.rect();
        let tall = (y + h - self.source_list_top()).max(0.0);
        let all = self.source_rows().len() as f32 * self.source_row_h();
        self.source.scroll = (self.source.scroll + dy).min(all - tall).max(0.0);
        self.redraw = true;
    }

    /// A click on the view outside its controls takes the keys.
    pub(super) fn source_click(&mut self) {
        self.focus = Focus::Files;
        self.source.typing = false;
        self.redraw = true;
    }

    /// The head's two views, as tabs, from the left. Returns where the
    /// space after them starts.
    pub(super) fn sidebar_views(&mut self, c: &mut Canvas, y: f32, h: f32, w: f32) -> f32 {
        let s = self.scale;
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let size = px(theme::SMALL, s);
        let th = px(24.0, s);
        let ty = (y + (h - th) / 2.0).round();
        let isz = px(12.0, s);
        let count = self.source_rows().iter().filter(|r| r.path.is_some()).count();
        let mut x = px(6.0, s);
        let views = [
            (false, Icon::Folder, "Explorer", "files"),
            (true, Icon::GitBranch, "Source Control", "git_source_view"),
        ];
        // Both labels where they fit beside room for actions; else the shown
        // view's label only; else icons.
        let width = |label: &str, fonts: &mut crate::font::Fonts| fonts.width(font::UI_MEDIUM, size, label) + isz + px(21.0, s);
        let both: f32 = views.iter().map(|v| width(v.2, fonts)).sum();
        let shown = views.iter().find(|v| v.0 == self.cfg.source_control).map_or(0.0, |v| width(v.2, fonts));
        let room = w - px(70.0, s);
        for (on_source, icon, label, id) in views {
            let on = self.cfg.source_control == on_source;
            let labelled = both < room || (on && shown + isz + px(20.0, s) < room);
            let tw = if labelled { fonts.width(font::UI_MEDIUM, size, label) + px(5.0, s) } else { 0.0 };
            let badge = if on_source && count > 0 { fonts.width(font::UI_MEDIUM, px(theme::SMALL - 1.0, s), &count.to_string()) + px(14.0, s) } else { 0.0 };
            let r = (x, ty, (tw + isz + badge + px(16.0, s)).round(), th);
            let hot = self.hot(r);
            if on {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::active());
            } else if hot {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::hover());
            }
            let ink = if on || hot { theme::fg() } else { theme::dim() };
            icons::draw(c, icon, r.0 + px(8.0, s), (ty + (th - isz) / 2.0).round(), isz, if on { theme::fg() } else { theme::icon() });
            let mut tx = r.0 + px(8.0, s) + isz + px(5.0, s);
            if labelled {
                ui::text(c, fonts, tx, ui::base(ty, th, size), label, font::UI_MEDIUM, size, ink);
                tx += tw;
            }
            if badge > 0.0 {
                ui::badge(c, fonts, tx - px(2.0, s), ty + th / 2.0, &count.to_string(), theme::dim(), s);
            }
            let keys = self.keys_of(id);
            self.push_hit(r, Hit::SidebarView(on_source), label, &keys);
            x += r.2 + px(2.0, s);
        }
        x
    }

    /// Source Control's head actions, from the right edge leftwards.
    pub(super) fn source_head_actions(&mut self, c: &mut Canvas, by: f32, mut right: f32, left: f32) {
        let s = self.scale;
        let btn = px(theme::SMALL_CONTROL, s);
        let actions = [
            (Icon::Refresh, Hit::SourceRefresh, "Refresh", ""),
            (Icon::Ellipsis, Hit::SourceMenu, "Source control list", "git_changes"),
        ];
        for (icon, hit, label, id) in actions {
            right -= btn;
            if right < left {
                break;
            }
            let r = (right, by, btn, btn);
            ui::icon_button(c, r, icon, self.hot(r), false, s);
            let keys = if id.is_empty() { String::new() } else { self.keys_of(id) };
            self.push_hit(r, hit, label, &keys);
            right -= px(2.0, s);
        }
    }

    pub(super) fn draw_source_control(&mut self, c: &mut Canvas) {
        let (x, y, w, h) = self.tree.rect();
        if w < 1.0 || h < 1.0 {
            return;
        }
        let s = self.scale;
        let focused = self.focus == Focus::Files && !self.prompt_has_keys() && !self.overlaid();
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());
        let size = px(theme::UI_TEXT, s);
        let small = px(theme::SMALL, s);
        let inset = px(8.0, s);
        let ctl = px(theme::CONTROL, s);

        if self.git.status.is_none() && self.git.others.iter().all(|(_, s)| s.is_none()) {
            let said = "This folder is not in a Git repository";
            let said = ui::fit(fonts, font::UI, size, said, w - inset * 2.0);
            ui::text(c, fonts, x + inset + px(4.0, s), ui::base(y + inset, px(theme::ROW, s), size), &said, font::UI, size, theme::dim());
            c.unclip();
            return;
        }

        // The message field.
        let field = (x + inset, y + inset, w - inset * 2.0, ctl);
        let typing = focused && self.source.typing;
        ui::input(c, field, typing, s);
        let tx = field.0 + px(8.0, s);
        let room = field.2 - px(16.0, s);
        let base = ui::base(field.1, field.3, size);
        if self.source.message.is_empty() {
            let hint = ui::fit(fonts, font::UI, size, "Commit message", room);
            ui::text(c, fonts, tx, base, &hint, font::UI, size, theme::faint());
            if typing {
                ui::caret(c, tx, field.1 + field.3 / 2.0, px(16.0, s), s);
            }
        } else {
            // Keep the caret in view: show the end of long messages.
            let upto = &self.source.message[..self.source.cur];
            let before = crate::bidi::caret_x(fonts,font::UI,size,&self.source.message,upto.len());
            let shift = (before - room + px(2.0, s)).max(0.0);
            c.clip(field.0 + 1.0, field.1, field.2 - 2.0, field.3);
            ui::text(c, fonts, tx - shift, base, &self.source.message, font::UI, size, theme::fg());
            if typing {
                ui::remember_input(70,&self.source.message,font::UI,size,field,tx-shift);
                ui::caret(c, tx - shift + before, field.1 + field.3 / 2.0, px(16.0, s), s);
            }
            c.unclip();
            c.clip(x, y, w, h);
        }
        self.push_hit(field, Hit::SourceMessage, "", "");

        // Commit, Amend and Draft.
        let by = field.1 + ctl + px(8.0, s);
        let draft_w = ctl;
        let amend_w = ui::button_width(fonts, "Amend", false, s);
        let commit_w = (field.2 - amend_w - draft_w - px(12.0, s)).max(px(40.0, s));
        let staged = self.git.status.as_ref().map_or(0, |st| st.staged.len());
        let commit_label = if staged > 0 && commit_w > px(110.0, s) { format!("Commit {staged}") } else { "Commit".into() };
        let commit = (field.0, by, commit_w, ctl);
        ui::button(c, fonts, commit, &commit_label, Some(Icon::Check), ui::Variant::Primary, self.hot(commit), s);
        self.push_hit(commit, Hit::SourceCommit, "Commit staged changes", "Ctrl+Enter");
        let amend = (commit.0 + commit_w + px(6.0, s), by, amend_w, ctl);
        ui::button(c, fonts, amend, "Amend", None, ui::Variant::Outline, self.hot(amend), s);
        self.push_hit(amend, Hit::SourceAmend, "Amend HEAD with staged changes", &self.keys_of("git_amend"));
        let draft = (amend.0 + amend_w + px(6.0, s), by, draft_w, ctl);
        ui::icon_button(c, draft, Icon::Bot, self.hot(draft), self.git_actions.drafting(), s);
        self.push_hit(draft, Hit::SourceDraft, "Draft commit message with the suggestion model", "");

        // The sections.
        let top = self.source_list_top();
        let tall = (y + h - top).max(0.0);
        let rows = self.source_rows();
        let rh = self.source_row_h();
        self.source.row = self.source.row.min(rows.len().saturating_sub(1));
        let all = rows.len() as f32 * rh;
        self.source.scroll = self.source.scroll.min((all - tall).max(0.0));
        if rows.is_empty() {
            ui::text(c, fonts, x + inset + px(4.0, s), ui::base(top, rh, size), "No changes", font::UI, size, theme::dim());
        }
        c.clip(x, top, w, tall);
        let btn = px(theme::SMALL_CONTROL - 2.0, s);
        let first = (self.source.scroll / rh) as usize;
        for (i, row) in rows.iter().enumerate().skip(first) {
            let ry = top + i as f32 * rh - self.source.scroll;
            if ry >= top + tall {
                break;
            }
            let r = (x + px(6.0, s), ry, w - px(12.0, s), rh);
            let chosen = focused && !self.source.typing && i == self.source.row;
            let hot = self.hot(r);
            let open = row.path.is_some() && row.path == self.doc.path;
            ui::row_ground(c, r, chosen || open, hot && !chosen, chosen, s);
            self.push_hit(r, Hit::SourceRow(i), "", "");
            let base = ui::base(ry, rh, size);
            let mut right = r.0 + r.2 - px(4.0, s);
            // The change's letter, tinted as in the explorer, or the count.
            if let Some(change) = row.change {
                let tint = match change {
                    Change::Untracked | Change::Added => theme::success(),
                    Change::Conflict | Change::Deleted => theme::danger(),
                    Change::Modified | Change::Renamed => theme::warning(),
                };
                let mark = change.letter();
                let mw = fonts.width(font::UI_MEDIUM, small, mark);
                ui::text(c, fonts, right - mw - px(2.0, s), ui::base(ry, rh, small), mark, font::UI_MEDIUM, small, tint);
                right -= mw + px(8.0, s);
            } else {
                let n = row.count.to_string();
                let nw = fonts.width(font::UI_MEDIUM, small, &n) + px(10.0, s);
                ui::badge(c, fonts, right - nw, ry + rh / 2.0, &n, theme::dim(), s);
                right -= nw + px(6.0, s);
            }
            // Buttons show on heads, and on a file under the pointer or chosen.
            if row.path.is_none() || hot || chosen {
                for &b in buttons(row) {
                    let (icon, label, keys) = button_look(b, row.section, row.path.is_none());
                    right -= btn;
                    let br = (right, ry + (rh - btn) / 2.0, btn, btn);
                    ui::icon_button(c, br, icon, self.hot(br), false, s);
                    self.push_hit(br, Hit::SourceButton(i, b), label, keys);
                    right -= px(2.0, s);
                }
            }
            let left = r.0 + px(8.0, s);
            let room = (right - left - px(4.0, s)).max(0.0);
            match &row.path {
                None if row.section == Section::Repository => {
                    let name = row.repo.as_deref().map_or(String::new(), |r| self.root_name(r));
                    let t = ui::fit(fonts, font::UI_MEDIUM, size, &name, room);
                    ui::text(c, fonts, left, base, &t, font::UI_MEDIUM, size, theme::fg());
                }
                None => {
                    let t = ui::fit(fonts, font::UI_MEDIUM, small, row.section.title(), room);
                    ui::text(c, fonts, left, ui::base(ry, rh, small), &t, font::UI_MEDIUM, small, theme::dim());
                }
                Some(path) => {
                    let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    let dir = path
                        .parent()
                        .and_then(|d| d.strip_prefix(row.repo.as_ref().unwrap_or(&self.root)).ok())
                        .map_or(String::new(), |d| d.to_string_lossy().into_owned());
                    let face = if open { font::UI_MEDIUM } else { font::UI };
                    let deleted = row.change == Some(Change::Deleted);
                    let fitted = ui::fit(fonts, face, size, &name, room);
                    let nw = ui::text(c, fonts, left + px(8.0, s), base, &fitted, face, size, if deleted { theme::dim() } else { theme::fg() });
                    if deleted {
                        c.rect(left + px(8.0, s), (ry + rh / 2.0).round(), nw, 1.0, theme::dim());
                    }
                    let rest = room - nw - px(14.0, s);
                    if !dir.is_empty() && rest > px(24.0, s) {
                        let d = ui::fit_start(fonts, font::UI, small, &dir, rest);
                        ui::text(c, fonts, left + px(8.0, s) + nw + px(6.0, s), base, &d, font::UI, small, theme::faint());
                    }
                }
            }
        }
        c.unclip();
        if all > tall && self.hot((x, top, w, tall)) {
            ui::scrollbar(c, (x, top, w, tall), self.source.scroll / all, tall / all, false, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(entries: &[(&str, Change, bool, bool)]) -> Status {
        let mut s = Status::default();
        for (p, change, staged, unstaged) in entries {
            let p = PathBuf::from(p);
            s.changes.insert(p.clone(), *change);
            if *staged {
                s.staged.insert(p.clone());
            }
            if *unstaged {
                s.unstaged.insert(p);
            }
        }
        s
    }

    #[test]
    fn sections_hold_their_files_in_order_with_counts() {
        let s = status(&[
            ("/r/b.rs", Change::Modified, true, true),
            ("/r/a.rs", Change::Modified, false, true),
            ("/r/new", Change::Untracked, false, true),
            ("/r/x", Change::Conflict, true, true),
            ("/r/added", Change::Added, true, false),
        ]);
        let listed = rows(Some(&s));
        let heads: Vec<_> = listed.iter().filter(|r| r.path.is_none()).map(|r| (r.section, r.count)).collect();
        assert_eq!(
            heads,
            [(Section::Conflicted, 1), (Section::Staged, 2), (Section::Unstaged, 2), (Section::Untracked, 1)]
        );
        // A file staged and changed again shows in both sections.
        assert_eq!(section_paths(&listed, Section::Staged, &None), [PathBuf::from("/r/added"), PathBuf::from("/r/b.rs")]);
        assert_eq!(section_paths(&listed, Section::Unstaged, &None), [PathBuf::from("/r/a.rs"), PathBuf::from("/r/b.rs")]);
        assert!(rows(None).is_empty());
        assert!(rows(Some(&Status::default())).is_empty());
    }

    #[test]
    fn several_repositories_group_their_changes_under_their_heads() {
        let one = status(&[("/a/x", Change::Modified, false, true)]);
        let two = status(&[("/b/y", Change::Modified, false, true), ("/b/z", Change::Untracked, false, true)]);
        let listed = grouped(&[(PathBuf::from("/a"), Some(&one)), (PathBuf::from("/b"), Some(&two))]);
        let heads: Vec<_> = listed.iter().filter(|r| r.section == Section::Repository).map(|r| (r.repo.clone(), r.count)).collect();
        assert_eq!(heads, [(Some(PathBuf::from("/a")), 1), (Some(PathBuf::from("/b")), 2)]);
        assert!(buttons(&listed[0]).is_empty());
        // A section's files are its own repository's.
        assert_eq!(section_paths(&listed, Section::Unstaged, &Some(PathBuf::from("/b"))), [PathBuf::from("/b/y")]);
    }

    #[test]
    fn rows_offer_the_buttons_their_section_allows() {
        let s = status(&[("/r/a", Change::Modified, true, true), ("/r/n", Change::Untracked, false, true)]);
        let listed = rows(Some(&s));
        let staged = listed.iter().find(|r| r.section == Section::Staged && r.path.is_some()).unwrap();
        assert_eq!(buttons(staged), [Button::Unstage]);
        let new = listed.iter().find(|r| r.section == Section::Untracked && r.path.is_some()).unwrap();
        assert!(buttons(new).contains(&Button::Discard));
    }
}
