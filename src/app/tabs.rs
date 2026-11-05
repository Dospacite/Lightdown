//! Open documents. The active document lives on App; inactive tabs own all
//! editor and kernel state so switching never reloads a file or loses undo.
use super::*;

pub(super) struct Document {
    pub(super) pane: Pane,
    pub(super) notebook: Option<Notebook>,
    pub(super) reader: Option<Reader>,
    pub(super) helper: Option<Kernel>,
    typeset: Typeset,
    pub(super) root: PathBuf,
}

pub(super) struct FileTab {
    pub(super) state: Option<Document>,
    pub(super) pinned: bool,
    pub(super) id: u64,
    pub(super) disk: super::disk::Disk,
}

static TAB_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl FileTab {
    pub fn preview() -> Self {
        Self {
            state: None,
            pinned: false,
            disk: Default::default(),
            id: TAB_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}

impl App {
    pub(crate) fn open_cli(&mut self, target: &crate::cli::Target) {
        if target.path.is_dir() { return; }
        self.file_tabs[self.active_file].pinned = true;
        self.open_tab(target.path.clone());
        self.file_tabs[self.active_file].pinned = true;
        if self.reader.is_none() && self.nb.is_none() {
            let buf = &self.doc.buf;
            let (start, end) = buf.line(target.line.saturating_sub(1).min(buf.lines() - 1));
            let at = buf.text[start..end].char_indices().nth(target.column.saturating_sub(1))
                .map_or(end, |(i, _)| start + i);
            self.doc.select(at, at);
        }
    }

    fn swap_document(&mut self, state: &mut Document) {
        std::mem::swap(&mut self.doc, &mut state.pane);
        std::mem::swap(&mut self.nb, &mut state.notebook);
        std::mem::swap(&mut self.reader, &mut state.reader);
        std::mem::swap(&mut self.helper, &mut state.helper);
        std::mem::swap(&mut self.typeset, &mut state.typeset);
        std::mem::swap(&mut self.root, &mut state.root);
    }

    /// Each tab's file, if it has one, and where its cursor is.
    pub(super) fn tab_files(&self) -> Vec<Option<(PathBuf, usize)>> {
        let at = |pane: &Pane| pane.path.clone().map(|p| (p, pane.buf.cur));
        let pane = |i: usize| {
            if i == self.active_file { Some(&self.doc) } else { self.file_tabs[i].state.as_ref().map(|s| &s.pane) }
        };
        (0..self.file_tabs.len()).map(|i| pane(i).and_then(at)).collect()
    }

    pub(super) fn pin_edited_tab(&mut self) {
        if self.doc.buf.modified || self.nb.as_ref().is_some_and(Notebook::busy) {
            self.file_tabs[self.active_file].pinned = true;
        }
    }

    fn clear_document_input(&mut self) {
        self.unsuggest();
        self.doc.release();
        self.comp = None;
        self.comp_box = None;
        self.docs = Default::default();
        self.prompt = None;
        self.popup = None;
        self.tab_close_armed = None;
        self.words = None;
        self.focus = Focus::Doc;
    }

    pub(super) fn activate_file(&mut self, index: usize) {
        self.doc.find_scope = None;
        self.found_for = None;
        self.autosave_due = None;
        if index == self.active_file || index >= self.file_tabs.len() {
            return;
        }
        self.pin_edited_tab();
        self.clear_document_input();
        let mut state = self.file_tabs[index].state.take().unwrap();
        let old_root = self.root.clone();
        if state.root != self.root {
            self.stop();
        }
        self.swap_document(&mut state);
        self.file_tabs[self.active_file].state = Some(state);
        self.active_file = index;
        if let Err(e) = self.cfg.ensure_project(&self.root) { self.message = Some(e); }
        if self.root != old_root {
            self.ai.images.clear();
            self.ai.thread = agent::Thread::new();
            self.show_thread();
        }
        self.arrange();
        self.retitle = true;
        self.redraw = true;
        self.disk_poll(true);
    }

    pub(super) fn open_tab(&mut self, path: PathBuf) {
        self.remember_jump();
        let path = path.canonicalize().unwrap_or_else(|_| std::path::absolute(&path).unwrap_or(path));
        let existing = self.file_tabs.iter().enumerate().position(|(i, t)| {
            let p = if i == self.active_file {
                self.doc.path.as_ref()
            } else {
                t.state.as_ref().and_then(|s| s.pane.path.as_ref())
            };
            p.is_some_and(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == path)
        });
        if let Some(index) = existing {
            self.activate_file(index);
            self.focus = Focus::Doc;
            return;
        }
        self.pin_edited_tab();
        self.clear_document_input();
        if self.file_tabs[self.active_file].pinned {
            let mut empty = Document {
                pane: Pane::new(self.fonts.clone(), self.pics.clone()),
                notebook: None,
                reader: None,
                helper: None,
                typeset: Typeset::new(Preview::new(self.fonts.clone(), self.pics.clone())),
                root: self.root.clone(),
            };
            self.swap_document(&mut empty);
            self.file_tabs[self.active_file].state = Some(empty);
            self.file_tabs.push(FileTab::preview());
            self.active_file = self.file_tabs.len() - 1;
        }
        self.file_tabs[self.active_file] = FileTab::preview();
        self.load(Some(path));
        self.arrange();
    }

    pub(super) fn open_output(&mut self, text: String) {
        self.pin_edited_tab();
        self.clear_document_input();
        let mut state = Document {
            pane: Pane::new(self.fonts.clone(), self.pics.clone()),
            notebook: None,
            reader: None,
            helper: None,
            typeset: Typeset::new(Preview::new(self.fonts.clone(), self.pics.clone())),
            root: self.root.clone(),
        };
        self.swap_document(&mut state);
        self.file_tabs[self.active_file].state = Some(state);
        self.file_tabs.push(FileTab::preview());
        self.active_file = self.file_tabs.len() - 1;
        self.doc.set_text(text, Doc::Code);
        self.doc.read_only = true;
        self.doc.output_name = Some("Output".into());
        self.file_tabs[self.active_file].pinned = true;
        self.arrange();
        self.retitle = true;
        self.redraw = true;
    }

    pub(super) fn close_file_tab(&mut self, index: usize) {
        if index >= self.file_tabs.len() {
            return;
        }
        let (modified, busy) = if index == self.active_file {
            (self.doc.buf.modified, self.nb.as_ref().is_some_and(Notebook::busy))
        } else {
            let state = self.file_tabs[index].state.as_ref().unwrap();
            (state.pane.buf.modified, state.notebook.as_ref().is_some_and(Notebook::busy))
        };
        if (modified || busy) && self.tab_close_armed != Some(index) {
            self.activate_file(index);
            self.tab_close_armed = Some(index);
            self.message = Some(if modified {
                "Unsaved changes. Ctrl+S saves; close this tab again to discard.".into()
            } else {
                "A cell is running. Close this tab again to stop it.".into()
            });
            self.redraw = true;
            return;
        }
        self.tab_close_armed = None;
        let next_view = self.views_forget(self.file_tabs[index].id);
        let mut closed = if self.file_tabs.len() == 1 {
            self.clear_document_input();
            let mut state = Document {
                pane: Pane::new(self.fonts.clone(), self.pics.clone()), notebook: None, reader: None, helper: None,
                typeset: Typeset::new(Preview::new(self.fonts.clone(), self.pics.clone())),
                root: self.root.clone(),
            };
            self.swap_document(&mut state);
            let mut tab = std::mem::replace(&mut self.file_tabs[0], FileTab::preview());
            tab.state = Some(state);
            self.load(None);
            tab
        } else {
            if index == self.active_file {
                self.activate_file(next_view.unwrap_or(if index > 0 { index - 1 } else { 1 }));
            }
            let tab = self.file_tabs.remove(index);
            if self.active_file > index { self.active_file -= 1; }
            tab
        };
        if let Some(state) = &mut closed.state {
            state.helper = None;
            state.typeset = Typeset::new(Preview::new(self.fonts.clone(), self.pics.clone()));
            if let Some(nb) = &mut state.notebook { nb.restart(&mut state.pane); }
        }
        self.closed_tabs.push(closed);
        if self.closed_tabs.len() > 20 { self.closed_tabs.remove(0); }
        self.arrange();
        self.retitle = true;
        self.redraw = true;
        if let Err(e) = self.recovery_save() {
            self.message = Some(format!("Recovery checkpoint failed: {e}"));
        }
    }

    pub(super) fn reopen_closed_tab(&mut self) {
        let Some(mut tab) = self.closed_tabs.pop() else {
            self.message = Some("No closed tabs to reopen".into());
            return;
        };
        let mut state = tab.state.take().unwrap();
        if let Some(path) = &state.pane.path {
            if let Some(i) = self.tab_files().iter().position(|p| p.as_ref().is_some_and(|p| &p.0 == path)) {
                self.activate_file(i);
                return;
            }
        }
        if self.doc.path.is_some() || !self.doc.buf.text.is_empty() || self.reader.is_some() || self.nb.is_some() {
            self.open_output(String::new());
        }
        self.swap_document(&mut state);
        if let Err(e) = self.cfg.ensure_project(&self.root) { self.message = Some(e); }
        self.file_tabs[self.active_file].disk = tab.disk;
        self.file_tabs[self.active_file].pinned = true;
        self.clear_document_input();
        self.arrange();
        self.disk_poll(true);
        self.retitle = true;
        self.redraw = true;
    }

    pub(super) fn close_saved_tabs(&mut self) {
        // Inspect by stable ID: closing the active tab changes its index.
        let ids: Vec<_> = self.file_tabs.iter().enumerate().filter(|(i, _)| !self.tab_unsaved(*i))
            .map(|(_, t)| t.id).collect();
        for id in ids {
            if let Some(i) = self.file_tabs.iter().position(|t| t.id == id) { self.close_file_tab(i); }
        }
    }

    pub(super) fn poll_file_tabs(&mut self) {
        for tab in &mut self.file_tabs {
            let Some(state) = &mut tab.state else { continue };
            let python = self.cfg.python_for(&state.root);
            let dir = state.pane.path.as_deref().and_then(Path::parent).unwrap_or(&state.root).to_path_buf();
            if let Some(nb) = &mut state.notebook {
                nb.poll(&mut state.pane, &python, &dir, &self.waker);
            }
        }
    }

    /// Follow moves even when the affected document is in a background tab.
    pub(super) fn move_stored_paths(&mut self, old: &Path, new: &Path) {
        for tab in &mut self.file_tabs {
            let Some(state) = &mut tab.state else { continue };
            let Some(path) = &state.pane.path else { continue };
            let Ok(rest) = path.strip_prefix(old) else { continue };
            let path = new.join(rest);
            state.pane.path = Some(path.clone());
            if state.notebook.is_none() {
                let kind = doc_for(Some(&path));
                if kind != Doc::Cells {
                    state.pane.set_doc(kind);
                }
            }
            if let Some(reader) = &mut state.reader {
                reader.pages.path = path.clone();
            }
            if let Ok(rest) = state.root.strip_prefix(old) {
                state.root = new.join(rest);
            } else if !path.starts_with(&state.root) {
                state.root = config::project_root(Some(&path));
            }
        }
    }

    /// Remove clean background tabs whose files were deleted. Edited tabs
    /// retain their text so they can still be saved.
    pub(super) fn remove_deleted_tabs(&mut self, path: &Path) {
        for i in (0..self.file_tabs.len()).rev() {
            let Some(state) = &self.file_tabs[i].state else { continue };
            let removed = state.pane.path.as_ref().is_some_and(|p| p.starts_with(path));
            if removed && !state.pane.buf.modified && !state.notebook.as_ref().is_some_and(Notebook::busy) {
                self.file_tabs.remove(i);
                if i < self.active_file {
                    self.active_file -= 1;
                }
            }
        }
        self.tab_close_armed = None;
    }

    pub(super) fn any_unsaved_tab(&self) -> bool {
        self.doc.buf.modified || self.file_tabs.iter().any(|t| t.state.as_ref().is_some_and(|s| s.pane.buf.modified))
    }

    pub(super) fn file_tab_hit(&self, x: f32, y: f32) -> Option<(usize, bool)> {
        self.file_tab_hits.iter().find(|(r, _, _)| ui::inside(*r, Some((x, y)))).map(|(_, i, close)| (*i, *close))
    }

    /// Whether a tab holds unsaved changes.
    pub(super) fn tab_unsaved(&self, index: usize) -> bool {
        if index == self.active_file {
            return self.doc.buf.modified;
        }
        self.file_tabs.get(index).and_then(|t| t.state.as_ref()).is_some_and(|s| s.pane.buf.modified)
    }

    /// The editor's head: a tab for each open file, and at the right what
    /// can be done with the one shown.
    pub(super) fn draw_file_tabs(&mut self, c: &mut Canvas) {
        use crate::icons::{self, Icon};
        use chrome::Hit;
        self.pin_edited_tab();
        let (left, width) = self.document_area();
        let (top, height) = (self.title_h(), self.head_h() - 1.0);
        self.file_tab_hits.clear();
        if width <= 0.0 {
            return;
        }
        let s = self.scale;
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        c.clip(left, top, width, height);

        // What can be done with the file, at the right.
        let btn = px(theme::CONTROL, s);
        let by = (top + (height - btn) / 2.0).round();
        let mut right = left + width - px(6.0, s);
        let mut actions = vec![(Icon::Ellipsis, Hit::More, "File actions", self.keys_of("file_menu"), false)];
        if self.nb.is_some() {
            actions.push((Icon::RotateCcw, Hit::RestartKernel, "Restart the kernel", self.keys_of("restart"), false));
            actions.push((Icon::PlayAll, Hit::RunAll, "Run all cells", self.keys_of("run_all"), false));
        } else if self.doc.doc == Doc::Python && self.doc.path.is_some() {
            actions.push((Icon::Play, Hit::RunFile, "Run in the terminal", String::new(), false));
        }
        if self.buildable() {
            let on = self.side_open && self.tab == Tab::Preview && self.side_width() > 0.0;
            actions.push((Icon::FileText, Hit::Preview, "Preview as PDF", self.keys_of("preview"), on));
        }
        if self.doc.doc == Doc::Markdown && self.reader.is_none() {
            let label = if self.doc.edit_mode { "Render markdown in place" } else { "Show all markdown syntax" };
            actions.push((Icon::Code, Hit::EditMode, label, self.keys_of("edit_mode"), self.doc.edit_mode));
        }
        for (icon, hit, label, keys, on) in actions {
            if right - btn < left + px(120.0, s) {
                break;
            }
            right -= btn;
            let r = (right, by, btn, btn);
            let hot = !self.overlaid() && ui::inside(r, Some(self.pointer));
            ui::icon_button(c, r, icon, hot, on, s);
            self.push_hit(r, hit, label, &keys);
            right -= px(2.0, s);
        }
        let room = (right - px(6.0, s) - (left + px(6.0, s))).max(1.0);

        // The tabs, each as wide as its name, narrowed together where they
        // do not fit, with arrows to the rest where even that is too much.
        let size = px(theme::UI_TEXT, s);
        let count = self.file_tabs.len();
        let names: Vec<(String, Option<PathBuf>, bool)> = (0..count)
            .map(|i| {
                let pane =
                    if i == self.active_file { &self.doc } else { &self.file_tabs[i].state.as_ref().unwrap().pane };
                let name = pane.path.as_deref().and_then(|p| p.file_name()).map_or_else(
                    || pane.output_name.clone().unwrap_or_else(|| "Untitled".into()),
                    |n| n.to_string_lossy().into_owned(),
                );
                (name, pane.path.clone(), pane.buf.modified)
            })
            .collect();
        let pad = px(10.0, s);
        let close = px(20.0, s);
        let natural: Vec<f32> = names
            .iter()
            .map(|(n, ..)| {
                let icon = if crate::icon_theme::everywhere() { px(theme::SMALL_ICON + 6.0, s) } else { 0.0 };
                (fonts.width(font::UI_MEDIUM, size, n) + pad * 2.0 + close + icon).clamp(px(90.0, s), px(220.0, s) + icon).round()
            })
            .collect();
        let total: f32 = natural.iter().sum::<f32>() + px(2.0, s) * count as f32;
        let min_w = px(96.0, s);
        let (widths, first, shown, arrows) = if total <= room {
            (natural, 0, count, false)
        } else {
            let k = (room / total).max(0.0);
            let squeezed: Vec<f32> = natural.iter().map(|w| (w * k).max(min_w).round()).collect();
            let sum: f32 = squeezed.iter().sum::<f32>() + px(2.0, s) * count as f32;
            if sum <= room {
                (squeezed, 0, count, false)
            } else {
                let arrows_w = btn * 2.0;
                let fit = (((room - arrows_w) / (min_w + px(2.0, s))).floor() as usize).max(1).min(count);
                let first = self.active_file.saturating_sub(fit - 1).min(count - fit);
                (vec![min_w; count], first, fit, true)
            }
        };
        // Tabs in the other views are lit, as the active one is.
        let in_views = self.split_others();
        let th = px(28.0, s);
        let ty = (top + (height - th) / 2.0).round();
        let radius = px(theme::RADIUS_MD, s);
        let mut x = left + px(6.0, s);
        for i in first..(first + shown).min(count) {
            let (name, path, modified) = &names[i];
            let tw = widths[i];
            let r = (x, ty, tw, th);
            let active = i == self.active_file;
            let hot = !self.overlaid() && ui::inside(r, Some(self.pointer));
            if active {
                c.round_rect(r.0, r.1, r.2, r.3, radius, theme::active());
            } else if hot {
                c.round_rect(r.0, r.1, r.2, r.3, radius, theme::hover());
            }
            let pinned = self.file_tabs[i].pinned;
            let face = match (pinned, active) {
                (false, _) => font::UI_ITALIC,
                (true, true) => font::UI_MEDIUM,
                (true, false) => font::UI,
            };
            let ink = if active || hot || in_views.contains(&i) { theme::fg() } else { theme::dim() };
            let mut lx = x + pad;
            if crate::icon_theme::everywhere() {
                let isz = px(theme::SMALL_ICON, s);
                let kind = crate::icon_theme::Kind::File;
                crate::icon_theme::draw(c, name, kind, lx, (ty + (th - isz) / 2.0).round(), isz, theme::icon());
                lx += isz + px(6.0, s);
            }
            let label = ui::fit(fonts, face, size, name, (tw - (lx - x) - close - px(6.0, s)).max(0.0));
            ui::text(c, fonts, lx, ui::base(ty, th, size), &label, face, size, ink);
            // The close button, or a dot for unsaved changes until the
            // pointer comes to the tab.
            let cr = (x + tw - close - px(4.0, s), ty + (th - close) / 2.0, close, close);
            if hot || (active && !*modified) {
                let ch = !self.overlaid() && ui::inside(cr, Some(self.pointer));
                if ch {
                    c.round_rect(cr.0, cr.1, cr.2, cr.3, px(theme::RADIUS_SM, s), theme::field());
                }
                let ink = if ch { theme::fg() } else { theme::icon() };
                icons::centred(c, Icon::X, cr, px(12.0, s), ink);
                self.file_tab_hits.push((cr, i, true));
                ui::tip(cr, "Close", &self.keys_of("close_tab"));
            } else if *modified {
                c.circle(cr.0 + close / 2.0, cr.1 + close / 2.0, px(3.5, s), theme::warning());
            }
            self.file_tab_hits.push((r, i, false));
            let tip =
                path.as_deref().map_or_else(|| "Not saved yet".to_string(), |p| crate::tools::short(&self.root, p));
            ui::tip(r, &tip, "");
            x += tw + px(2.0, s);
        }
        if arrows {
            for (j, icon, index) in [
                (0, Icon::ChevronLeft, self.active_file.saturating_sub(1)),
                (1, Icon::ChevronRight, (self.active_file + 1).min(count - 1)),
            ] {
                let r = (right - btn * (2 - j) as f32, by, btn, btn);
                let hot = !self.overlaid() && ui::inside(r, Some(self.pointer));
                ui::icon_button(c, r, icon, hot, false, s);
                self.file_tab_hits.push((r, index, false));
            }
        }
        c.unclip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-tabs-{}-{name}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.md"), "first\nsecond").unwrap();
        std::fs::write(dir.join("b.py"), "x = 1").unwrap();
        std::fs::write(dir.join("c.md"), "third").unwrap();
        dir
    }

    fn draw(a: &mut App) {
        let mut pixels = vec![0; (a.w * a.h) as usize];
        a.draw(&mut Canvas::new(&mut pixels, a.w as i32, a.h as i32));
    }

    #[test]
    fn reopen_restores_untitled_history_and_close_saved_leaves_dirty_tabs() {
        let mut a = App::new(None);
        a.paste_text("café draft");
        a.doc.select(2, 5);
        a.close_file_tab(0);
        assert!(!a.doc.buf.text.is_empty());
        a.close_file_tab(0);
        assert!(a.doc.buf.text.is_empty());
        a.reopen_closed_tab();
        assert_eq!(a.file_tabs.len(), 1);
        assert_eq!(a.doc.buf.text, "café draft");
        assert_eq!(a.doc.buf.sel(), (2, 5));
        assert!(a.doc.buf.can_undo());
        a.open_output("saved output".into());
        a.close_saved_tabs();
        assert_eq!(a.file_tabs.len(), 1);
        assert_eq!(a.doc.buf.text, "café draft");
        a.reopen_closed_tab();
        assert_eq!(a.doc.buf.text, "saved output");
        assert!(a.doc.read_only);
    }

    #[test]
    fn cli_targets_keep_tabs_and_use_unicode_columns() {
        let dir = project("cli");
        std::fs::write(dir.join("a.md"), "one\ncafé tea\n").unwrap();
        let mut a = App::new(Some(dir.join("a.md")));
        let target = crate::cli::Target { path: dir.join("a.md"), line: 2, column: 5 };
        a.open_cli(&target);
        assert_eq!(a.doc.buf.cur, "one\ncafé".len());
        a.open_cli(&crate::cli::Target { path: dir.join("b.py"), line: 500, column: 500 });
        assert_eq!(a.file_tabs.len(), 2);
        a.wait_mode = true;
        a.doc.insert("changed", EditKind::Other);
        a.close();
        assert!(!a.exit);
        a.close();
        assert!(a.exit && a.wait_cancelled);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn previews_replace_and_edited_tabs_survive_save_and_switching() {
        let dir = project("preview");
        let mut a = App::new(Some(dir.join("a.md")));
        a.show_side(None);
        a.resize(1000, 700, 1.0);
        a.open(dir.join("b.py"));
        assert_eq!(a.file_tabs.len(), 1);
        a.doc.select(0, 0);
        a.paste_text("# changed\n");
        a.save();
        a.open(dir.join("c.md"));
        assert_eq!(a.file_tabs.len(), 2);
        a.open(dir.join("a.md"));
        assert_eq!(a.file_tabs.len(), 2);
        a.key(Key::Tab, CTRL, None);
        assert_eq!(a.doc.path, Some(dir.join("b.py")));
        assert_eq!(a.doc.buf.cur, 10);
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "# changed\nx = 1");
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "x = 1");
        a.open(dir.join("./a.md"));
        assert_eq!(a.file_tabs.len(), 2);
        a.key(Key::Tab, Mods { shift: true, ..CTRL }, None);
        assert_eq!(a.doc.doc, Doc::Python);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tab_buttons_switch_and_guard_dirty_closes_and_window_close() {
        let dir = project("close");
        let mut a = App::new(Some(dir.join("a.md")));
        a.show_side(None);
        a.resize(800, 600, 1.0);
        a.paste_text("changed");
        a.open(dir.join("b.py"));
        a.close();
        assert!(a.exit);
        a.exit = false;
        a.quit_armed = false;
        draw(&mut a);
        let r = a.file_tab_hits.iter().find(|(_, i, close)| *i == 0 && !*close).unwrap().0;
        a.press(r.0 + 5.0, r.1 + 5.0, NONE, 1);
        assert_eq!(a.active_file, 0);
        a.key(Key::Char('w'), CTRL, None);
        assert_eq!(a.file_tabs.len(), 2);
        assert!(a.message.as_deref().unwrap().contains("Unsaved"));
        a.key(Key::Char('w'), CTRL, None);
        assert_eq!(a.file_tabs.len(), 1);
        assert_eq!(a.doc.path, Some(dir.join("b.py")));
        draw(&mut a);
        let r = a.file_tab_hits[0].0;
        a.press_button(r.0 + 5.0, r.1 + 5.0, 1);
        assert!(a.doc.path.is_none());
        assert_eq!(a.file_tabs.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn notebook_kernel_and_outputs_survive_a_tab_switch() {
        let dir = project("kernel");
        let file = dir.join("n.ipynb");
        std::fs::write(&file, "").unwrap();
        let mut a = App::new(Some(file.clone()));
        a.cfg.python = "python3".into();
        a.paste_text("value = 40\nprint(value)");
        assert!(a.run_cell(0));
        a.open(dir.join("a.md"));
        let state = a.file_tabs[0].state.as_mut().unwrap();
        state.notebook.as_mut().unwrap().settle(&mut state.pane, "python3", &dir, &a.waker);
        a.poll_file_tabs();
        a.open(file);
        assert!(a.nb.as_ref().unwrap().kernel_started());
        a.doc.select(a.doc.buf.text.len(), a.doc.buf.text.len());
        a.paste_text("\nprint(value + 2)");
        assert!(a.run_cell(0));
        a.settle();
        assert!(a.nb.as_ref().unwrap().output_text(a.doc.cells[0].id, 1000).contains("42"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn overflow_tabs_remain_clickable_at_small_sizes_and_display_scales() {
        let dir = project("overflow");
        let mut a = App::new(Some(dir.join("a.md")));
        a.show_side(None);
        for i in 0..8 {
            a.paste_text("edit");
            a.open(dir.join(format!("{i}.md")));
        }
        for scale in [1.0, 2.0] {
            a.resize((320.0 * scale) as u32, (300.0 * scale) as u32, scale);
            draw(&mut a);
            let (r, index, close) = a.file_tab_hits[a.file_tab_hits.len() - 2];
            assert!(!close && index < a.active_file);
            let point = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
            assert_eq!(a.zone(point.0, point.1, false), Zone::Button);
            a.press(point.0, point.1, NONE, 1);
            assert_eq!(a.active_file, index);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn background_tabs_follow_renames_and_keep_unsaved_deleted_files() {
        let dir = project("moves");
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/a.md"), "saved").unwrap();
        std::fs::write(dir.join("docs/b.py"), "unsaved").unwrap();
        let mut a = App::new(Some(dir.join("docs/a.md")));
        a.paste_text("edited ");
        a.save();
        a.open(dir.join("docs/b.py"));
        a.paste_text("keep ");
        a.open(dir.join("c.md"));
        assert_eq!(a.file_tabs.len(), 3);
        a.rename_path(&dir.join("docs"), dir.join("moved").to_str().unwrap());
        a.open(dir.join("moved/a.md"));
        assert_eq!(a.file_tabs.len(), 3);
        assert_eq!(a.doc.buf.text, "edited saved\n");
        a.open(dir.join("c.md"));
        a.discard_path(&dir.join("moved"), true);
        assert_eq!(a.file_tabs.len(), 2);
        a.activate_file(0);
        assert_eq!(a.doc.buf.text, "keep unsaved");
        assert!(a.doc.buf.modified);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn both_sidebars_keep_their_contents_and_focus_when_opening_files() {
        let dir = project("both-sidebars");
        let mut a = App::new(Some(dir.join("a.md")));
        a.resize(1400, 800, 1.0);
        a.toggle_side(Tab::Agent);
        a.paste_text("Draft for the agent");
        a.toggle_side(Tab::Files);
        draw(&mut a);
        assert!(a.files_open && a.side_open);
        assert!(a.tree.contains(20.0, 200.0) && a.ai.side.contains(1380.0, 200.0));
        // The editor lies between the two, each with its hairline.
        assert_eq!(a.document_area(), (260.0, 700.0));
        assert!(a.focus == Focus::Files && a.in_side());
        let original = a.doc.buf.text.clone();
        a.key(Key::Char('x'), NONE, Some("x"));
        assert_eq!(a.doc.buf.text, original);
        a.pick(dir.join("b.py"), false);
        draw(&mut a);
        assert!(a.focus == Focus::Doc && a.doc.focused);
        assert!(a.files_open && a.side_open);
        assert_eq!(a.ai.side.input.buf.text, "Draft for the agent");
        assert_eq!(a.doc.path, Some(dir.join("b.py")));
        a.show_files(false);
        assert!(a.side_open && a.ai.side.contains(1380.0, 200.0));
        a.show_files(true);
        a.show_side(Some(Tab::Preview));
        assert!(a.files_open && a.tab == Tab::Preview);
        a.toggle_preview();
        assert!(!a.side_open && a.files_open);
        a.toggle_side(Tab::Agent);
        assert_eq!(a.ai.side.input.buf.text, "Draft for the agent");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn each_sidebar_resizes_independently_and_preserves_document_space() {
        let dir = project("two-dividers");
        for scale in [1.0, 2.0] {
            let mut a = App::new(Some(dir.join("a.md")));
            a.resize((1400.0 * scale) as u32, (800.0 * scale) as u32, scale);
            a.show_files(true);
            a.show_side(Some(Tab::Agent));
            let y = 300.0 * scale;
            assert_eq!(a.zone(a.files_width(), y, false), Zone::Divider);
            a.press(a.files_width(), y, NONE, 1);
            a.motion(320.0 * scale, y);
            a.release();
            assert_eq!(a.cfg.files_width, 320.0);
            assert_eq!(a.cfg.sidebar_width, 440.0);
            a.press(a.w - a.side_width(), y, NONE, 1);
            a.motion(a.w - 460.0 * scale, y);
            a.release();
            assert_eq!(a.cfg.files_width, 320.0);
            assert_eq!(a.cfg.sidebar_width, 460.0);
            for width in [640.0, 800.0, 1000.0] {
                a.resize((width * scale) as u32, (800.0 * scale) as u32, scale);
                let (left, document) = a.document_area();
                assert!(left > 0.0 && a.side_width() > 0.0);
                // The reserved room includes the mat around the sheet.
                assert!(document >= 239.0 * scale);
                assert!(left + document <= a.w - a.side_width());
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compact_layout_switches_visible_sidebars_and_restores_both_on_widening() {
        let dir = project("compact-sidebars");
        let mut a = App::new(Some(dir.join("a.md")));
        a.resize(1000, 700, 1.0);
        a.toggle_side(Tab::Agent);
        draw(&mut a);
        a.resize(320, 400, 1.0);
        a.toggle_side(Tab::Files);
        draw(&mut a);
        assert!(a.files_open && a.side_open);
        assert_eq!(a.sidebar_widths(), (320.0, 0.0));
        assert!(!a.ai.side.input.contains(300.0, 350.0));
        let bar_point = |a: &App, hit: chrome::Hit| {
            let r = a.hits.iter().find(|(_, h)| *h == hit).unwrap().0;
            (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
        };
        let (x, y) = bar_point(&a, chrome::Hit::TogglePanel);
        a.press(x, y, NONE, 1);
        draw(&mut a);
        assert_eq!(a.sidebar_widths(), (0.0, 320.0));
        assert!(a.files_open && a.side_open && a.focus == Focus::Chat);
        let (x, y) = bar_point(&a, chrome::Hit::ToggleFiles);
        a.press(x, y, NONE, 1);
        assert_eq!(a.sidebar_widths(), (320.0, 0.0));
        a.resize(1400, 800, 1.0);
        draw(&mut a);
        assert_eq!(a.sidebar_widths(), (260.0, 440.0));
        assert!(a.tree.contains(20.0, 200.0) && a.ai.side.contains(1380.0, 200.0));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn files_and_their_resize_edge_are_on_the_left() {
        let dir = project("layout");
        let mut a = App::new(Some(dir.join("a.md")));
        a.resize(1200, 800, 1.0);
        a.show_side(Some(Tab::Files));
        assert!(a.tree.contains(20.0, 200.0));
        assert!(!a.tree.contains(1180.0, 200.0));
        let edge = a.files_width();
        assert_eq!(a.zone(edge, 200.0, false), Zone::Divider);
        a.press(edge, 200.0, NONE, 1);
        a.motion(300.0, 200.0);
        a.release();
        assert_eq!(a.files_width(), 300.0);
        assert!(!a.doc.contains(250.0, 200.0));
        assert!(a.doc.contains(400.0, 200.0));
        a.resize(320, 240, 1.0);
        draw(&mut a);
        assert!(a.file_tab_hits.is_empty());
        assert!(!a.doc.contains(310.0, 100.0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
