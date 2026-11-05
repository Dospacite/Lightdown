//! The pointer beyond the left button's click: menus of what can be done
//! with what is under the right button, pasting with the middle one, and
//! what lights up under the pointer as it moves.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::terminal::quote;
use super::{App, Focus, Key, Mods, PromptKind, Zone};
use crate::cells::Tag;
use crate::pane::Doc;
use crate::ui::{Item, Popup};

/// What a context menu is about.
#[derive(Clone, Debug)]
pub(super) enum Target {
    Doc { link: Option<String> },
    Reader,
    Tree { path: PathBuf, folder: bool },
    Transcript { link: Option<String> },
    Input,
    Term,
    Preview,
    Problem(super::problems::Problem),
}

const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

impl App {
    /// A shortcut's keys, for the right of a menu's row; nothing for one
    /// that has none.
    fn shortcut(&self, id: &str) -> String {
        let keys = self.keys_of(id);
        if keys == "none" { String::new() } else { keys }
    }

    /// A row that does what a shortcut does.
    fn act_item(&self, id: &str, label: &str) -> Item {
        Item::new(id, label, self.shortcut(id))
    }

    /// A press of the middle (1) or right (2) button.
    pub fn press_button(&mut self, x: f32, y: f32, button: u8) {
        self.redraw = true;
        self.pointer = (x, y);
        let mods = self.mods;
        let free = !self.overlaid() && self.term_open;
        if !self.overlaid() {
            self.term_aim(x, y);
        }
        // A program in the terminal that takes the mouse is told of every
        // button, as it would be in any terminal.
        if let Some(t) = self.terms.get_mut(self.term_cur).filter(|t| free && t.on_text(x, y) && t.grabs_mouse(mods)) {
            self.focus = Focus::Term;
            t.press(x, y, mods, 1, button);
            return;
        }
        if button == 1 {
            // The middle button closes a terminal by its row in the list.
            if let Some(super::Hit::TermTab(i) | super::Hit::TermClose(i)) = self.hit_at(x, y).filter(|_| free) {
                return self.close_term(i);
            }
            if !self.overlaid()
                && let Some((index, _)) = self.file_tab_hit(x, y)
            {
                self.close_file_tab(index);
                return;
            }
            return self.middle(x, y);
        }
        if self.settings.is_some() {
            return self.settings_back(x, y);
        }
        if self.menu.is_some() || self.chooser.is_some() {
            return;
        }
        self.popup = None;
        self.prompt = None;
        self.comp = None;
        self.unsuggest();
        self.context_menu(x, y);
        self.collect();
    }

    pub fn release_button(&mut self, button: u8) {
        for t in &mut self.terms {
            t.release(button, self.mods);
        }
    }

    /// The middle button pastes the primary selection where it is pressed.
    fn middle(&mut self, x: f32, y: f32) {
        if self.overlaid() {
            return;
        }
        self.prompt = None;
        self.comp = None;
        self.unsuggest();
        let term = self.term().is_some_and(|t| self.term_open && t.on_text(x, y));
        let side = &mut self.ai.side;
        if term {
            self.focus = Focus::Term;
        } else if side.input.contains(x, y) {
            self.focus = Focus::Chat;
            side.input.place_at(x, y);
        } else if self.reader.is_none() && self.doc.contains(x, y) && !self.doc.on_bar(x, y) && !self.doc.read_only {
            self.focus = Focus::Doc;
            self.doc.place_at(x, y);
        } else {
            return;
        }
        self.paste_primary = true;
    }

    /// Opens the menu of what can be done with what is at a point.
    fn context_menu(&mut self, x: f32, y: f32) {
        if y < self.title_h() || y >= self.body_bottom() {
            return;
        }
        let rule = || Item::Rule;
        let (target, items) = if let Some(t) =
            self.terms.get(self.term_cur).filter(|t| self.term_open && t.contains(x, y))
        {
            self.focus = Focus::Term;
            let items = vec![
                Item::new("term_copy", "Copy", "Ctrl+Shift+C").when(t.has_selection()),
                Item::new("term_paste", "Paste", "Ctrl+Shift+V"),
                Item::new("term_select_all", "Select all", ""),
                rule(),
                Item::new("term_ask", "Ask the agent about this output", ""),
                Item::new("term_clear", "Clear", ""),
                Item::new("term_restart", "Restart the shell", ""),
                rule(),
                self.act_item("terminal_new", "Open another terminal"),
                self.act_item(
                    "terminal_split",
                    if self.term_panes.is_some() { "Show one terminal" } else { "Split: two side by side" },
                ),
                self.act_item("terminal_close", "Close this terminal"),
                self.act_item("terminal_hide", "Hide the terminal"),
            ];
            (Target::Term, items)
        } else if self.problems.contains(x,y) {
            let Some(p) = self.problem_at(x,y) else { return };
            (Target::Problem(p),vec![Item::new("problem_ask", "Ask agent to fix", "")])
        } else if self.tree.contains(x, y) {
            self.focus = Focus::Files;
            let (path, folder, unfolded) = match self.tree.take(x, y) {
                Some(row) => row,
                None => (self.tree.root().to_path_buf(), true, true),
            };
            let one_of = self.tree.is_root(&path);
            let root = path == self.tree.root() || one_of;
            let mut items = Vec::new();
            if folder && (!root || one_of) {
                items.push(Item::new("tree_flip", if unfolded { "Fold" } else { "Unfold" }, "Enter"));
            } else if !folder {
                items.push(Item::new("tree_open", "Open", "Enter"));
                items.push(Item::new("tree_apart", "Open in a new window", "Ctrl+Enter"));
            }
            items.extend([
                rule(),
                Item::new("tree_new", "New file\u{2026}", ""),
                Item::new("tree_new_folder", "New folder\u{2026}", ""),
            ]);
            if !root {
                items.extend([
                    rule(),
                    Item::new("tree_rename", "Rename or move\u{2026}", self.shortcut("rename")),
                    Item::new("tree_duplicate", "Duplicate", ""),
                    Item::new("tree_trash", "Move to the trash\u{2026}", "Delete"),
                    rule(),
                    Item::new("tree_cut", "Cut", ""),
                    Item::new("tree_copy", "Copy", ""),
                ]);
            }
            let paste = match &self.file_clip {
                Some((clip, _)) => format!("Paste {} here", clip.file_name().unwrap_or_default().to_string_lossy()),
                None => "Paste here".to_string(),
            };
            if self.file_clip.is_some() {
                items.push(Item::new("tree_paste", &paste, ""));
            }
            items.extend([rule(), Item::new("tree_copy_path", "Copy the path", "")]);
            if !root {
                items.push(Item::new("tree_copy_relative", "Copy the path within the project", ""));
            }
            if one_of {
                items.push(Item::new("tree_remove_root", "Remove folder from window", ""));
            }
            items.extend([
                rule(),
                Item::new("tree_terminal", "Open a terminal here", ""),
                Item::new("tree_reveal", if folder { "Open in the file manager" } else { "Show in its folder" }, ""),
            ]);
            (Target::Tree { path, folder }, items)
        } else if self.typeset.view.contains(x, y) {
            (Target::Preview, self.preview_items())
        } else if self.ai.side.contains(x, y) {
            self.focus = Focus::Chat;
            let copy_keys = self.shortcut("copy");
            let side = &mut self.ai.side;
            if side.input.contains(x, y) {
                side.transcript.select(0, 0);
                if !side.input.on_selection(x, y) {
                    side.input.place_at(x, y);
                }
                let (has, empty) = (side.input.buf.has_sel(), side.input.buf.text.trim().is_empty());
                let (undo, redo) = (side.input.buf.can_undo(), side.input.buf.can_redo());
                let items = vec![
                    self.act_item("cut", "Cut").when(has),
                    self.act_item("copy", "Copy").when(has),
                    self.act_item("paste", "Paste"),
                    self.act_item("select_all", "Select all"),
                    rule(),
                    self.act_item("undo", "Undo").when(undo),
                    self.act_item("redo", "Redo").when(redo),
                    rule(),
                    Item::new("send", "Send", "Enter").when(!empty && !self.ai.busy()),
                ];
                (Target::Input, items)
            } else if side.transcript.contains(x, y) {
                let link = side.transcript.link_at(x, y).map(str::to_string);
                let mut items = Vec::new();
                if link.is_some() {
                    items.extend([
                        Item::new("open_link", "Open the link", ""),
                        Item::new("copy_link", "Copy the link", ""),
                    ]);
                    items.push(rule());
                }
                items.push(Item::new("t_copy", "Copy", copy_keys).when(side.transcript.buf.has_sel()));
                items.push(Item::new("t_select_all", "Select all", ""));
                items.push(rule());
                items.push(self.act_item("new_thread", "New thread"));
                items.push(self.act_item("threads", "Threads\u{2026}"));
                (Target::Transcript { link }, items)
            } else {
                return;
            }
        } else if let Some(reader) = self.reader.as_ref().filter(|r| r.contains(x, y)) {
            self.focus = Focus::Doc;
            let mut items = Vec::new();
            // A picture has no text to copy or find.
            if !reader.picture() {
                items.extend([
                    self.act_item("copy", "Copy").when(reader.selected().is_some()),
                    self.act_item("select_all", "Select all"),
                    rule(),
                    self.act_item("find", "Find\u{2026}"),
                    rule(),
                ]);
            }
            items.extend([
                self.act_item("zoom_in", "Zoom in"),
                self.act_item("zoom_out", "Zoom out"),
                self.act_item("zoom_reset", "Actual size"),
                rule(),
                Item::new("pdf_open", "Open in its viewer", ""),
            ]);
            (Target::Reader, items)
        } else if self.doc.contains(x, y) && !self.doc.on_bar(x, y) {
            self.focus = Focus::Doc;
            // A click outside the selection moves the cursor there, so
            // that what is done is done there.
            if !self.doc.on_selection(x, y) {
                self.doc.place_at(x, y);
                self.edited();
            }
            let link = self.doc.link_at(x, y).map(str::to_string);
            let mut items = self.grammar_items();
            if items.is_empty() { items = self.spell_items(); }
            items.extend(self.doc_items(link.is_some()));
            (Target::Doc { link: link.clone() }, items)
        } else {
            return;
        };
        let popup = Popup::new(items, (x, y));
        if !popup.is_empty() {
            self.popup = Some((target, popup));
        }
    }

    pub(super) fn open_cell_menu(&mut self, x: f32, y: f32) {
        self.popup = Some((Target::Doc { link: None }, Popup::new(self.doc_items(false), (x, y))));
    }

    /// What can be done in the document, by what it is and what is at the
    /// cursor.
    fn doc_items(&self, link: bool) -> Vec<Item> {
        let (ro, has) = (self.doc.read_only, self.doc.buf.has_sel());
        let cur = self.doc.buf.cur;
        let mut items = Vec::new();
        if link {
            items.extend([
                Item::new("open_link", "Open the link", "Ctrl+Click"),
                Item::new("copy_link", "Copy the link", ""),
            ]);
            items.push(Item::Rule);
        }
        if !ro {
            items.push(self.act_item("cut", "Cut").when(has));
        }
        items.push(self.act_item("copy", "Copy").when(has));
        if !ro {
            items.push(self.act_item("paste", "Paste"));
        }
        items.push(self.act_item("select_all", "Select all"));
        if !ro {
            items.extend([
                Item::Rule,
                self.act_item("undo", "Undo").when(self.doc.buf.can_undo()),
                self.act_item("redo", "Redo").when(self.doc.buf.can_redo()),
            ]);
        }
        items.push(Item::Rule);
        if !ro && (self.doc.formatting_at(cur) || self.doc.doc == Doc::Latex) {
            items.extend([self.act_item("bold", "Bold"), self.act_item("italic", "Italic")]);
        }
        if !ro && self.doc.comment_mark().is_some() {
            items.push(self.act_item("comment", "Comment out or in"));
        }
        if !ro && self.doc.doc == Doc::Markdown {
            let label = if self.doc.edit_mode { "Hide the syntax" } else { "Show all syntax" };
            items.push(self.act_item("edit_mode", label));
        }
        if let Some(nb) = self.nb.as_ref().filter(|_| !ro) {
            items.push(Item::Rule);
            if self.doc.python_at(cur) {
                items.extend([
                    self.act_item("run", "Run the cell"),
                    self.act_item("run_next", "Run the cell and go on"),
                ]);
            }
            if self.doc.cells.iter().any(|c| c.tag == Tag::Code) {
                items.push(self.act_item("run_all", "Run all cells"));
            }
            if nb.busy() {
                items.push(self.act_item("interrupt", "Interrupt"));
            }
            if nb.kernel_started() {
                items.push(self.act_item("restart", "Restart the kernel"));
            }
            let convert = if self.doc.cells[self.doc.cell_at(cur)].tag == Tag::Markdown {
                "Convert cell to code"
            } else {
                "Convert cell to markdown"
            };
            items.extend([
                Item::Rule,
                self.act_item("cell_above", "Add a cell above"),
                self.act_item("cell_below", "Add a cell below"),
                self.act_item("cell_kind", convert),
                self.act_item("cell_delete", "Delete the cell"),
            ]);
            if nb.can_clear_outputs() {
                items.push(self.act_item("clear_outputs", "Clear all outputs"));
            }
        } else if !ro && self.doc.doc == Doc::Python && self.doc.path.is_some() {
            items.extend([Item::Rule, Item::new("run_file", "Run in the terminal", "")]);
        }
        items.push(Item::Rule);
        items.push(self.act_item("find", "Find\u{2026}"));
        if !ro {
            items.push(self.act_item("replace", "Replace\u{2026}"));
        }
        if !ro && !has && self.cfg.key().is_some() {
            items.push(self.act_item("suggest", "Suggest what comes next"));
        }
        if self.buildable() {
            items.push(self.act_item("preview", "Build and preview"));
        }
        if matches!(self.doc.doc, crate::pane::Doc::Latex | crate::pane::Doc::Markdown) && self.doc.path.is_some() {
            items.push(self.act_item("sync", "Show in the PDF"));
        }
        items
    }

    /// A preview without a source, engine or finished PDF offers only its next valid step.
    fn preview_items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        if !self.buildable() {
            return items;
        }
        if !self.typeset.busy() {
            if self.typeset.view.no_engine {
                items.push(Item::new("preview_setup", "Set up Tectonic", ""));
            } else {
                items.push(self.act_item("preview", "Build"));
            }
        }
        if !self.typeset.view.problems.is_empty() {
            items.push(Item::new("agent_build", "Ask the agent about this failure", ""));
        }
        if self.typeset.view.pdf.is_some() {
            items.push(Item::new("preview_open", "Open the PDF in its viewer", ""));
        }
        items
    }

    /// Does what a row of a context menu says.
    pub(super) fn context_pick(&mut self, target: Target, key: &str) {
        self.redraw = true;
        match (target, key) {
            (Target::Problem(p), "problem_ask") => self.ask_problem(&p),
            (Target::Preview, "agent_build") => self.ask_build(),
            (Target::Doc { link: Some(url) } | Target::Transcript { link: Some(url) }, "open_link") => {
                self.follow(&url)
            }
            (Target::Doc { link: Some(url) } | Target::Transcript { link: Some(url) }, "copy_link") => {
                self.copy = Some(url);
            }
            (Target::Tree { path, folder }, key) => self.tree_pick(&path, folder, key),
            (Target::Doc { .. }, key) if key.starts_with("grammar_") => self.grammar_pick(key),
            (Target::Doc { .. }, key) if key.starts_with("spell_") => self.spell_pick(key),
            (Target::Term, key) => {
                let Some(t) = self.terms.get_mut(self.term_cur) else { return };
                match key {
                    "term_copy" => self.copy = t.selected(),
                    "term_paste" => self.paste = true,
                    "term_select_all" => t.select_all(),
                    "term_clear" => t.clear(),
                    "term_restart" => self.restart_term(),
                    "term_ask" => self.ask_terminal(),
                    _ => self.act(key),
                }
            }
            (Target::Transcript { .. }, "t_copy") => {
                self.ai.side.transcript.key(Key::Char('c'), CTRL, None);
            }
            (Target::Transcript { .. }, "t_select_all") => {
                self.ai.side.transcript.key(Key::Char('a'), CTRL, None);
            }
            (Target::Input, "send") => self.send(),
            (_, "run_file") => self.run_file(),
            (_, "pdf_open") => {
                if let Some(path) = self.doc.path.clone() {
                    self.open_with_desktop(&path);
                }
            }
            (_, "preview_open") => self.preview_hit(crate::preview::Hit::Open),
            (_, "preview_setup") => self.preview_hit(crate::preview::Hit::Setup),
            (_, id) => self.act(id),
        }
        self.collect();
    }

    fn open_with_desktop(&mut self, path: &Path) {
        if Command::new("xdg-open").arg(path).spawn().is_err() {
            self.message = Some("Cannot run xdg-open".into());
        }
    }

    /// Does what a row of the folder's menu says.
    fn tree_pick(&mut self, path: &Path, folder: bool, key: &str) {
        let dir = if folder { path } else { path.parent().unwrap_or(path) };
        match key {
            "tree_open" => self.pick(path.to_path_buf(), false),
            "tree_apart" => self.pick(path.to_path_buf(), true),
            "tree_flip" => self.tree.flip(path),
            "tree_new" | "tree_new_folder" => {
                self.ask(if key == "tree_new" { PromptKind::New } else { PromptKind::NewFolder });
                if let Some(p) = &mut self.prompt {
                    p.text = format!("{}/", dir.to_string_lossy().trim_end_matches('/'));
                    p.cur = p.text.len();
                }
            }
            "tree_rename" => self.ask_about(PromptKind::Rename, path),
            "tree_trash" => self.ask_about(PromptKind::Trash, path),
            "tree_duplicate" => self.duplicate(path),
            "tree_cut" | "tree_copy" => {
                let cut = key == "tree_cut";
                self.file_clip = Some((path.to_path_buf(), cut));
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                self.message =
                    Some(format!("{name} {}. Right-click a folder to paste it", if cut { "cut" } else { "copied" }));
            }
            "tree_paste" => self.paste_files(dir),
            "tree_copy_relative" => {
                let root = self.root_of(path).unwrap_or_else(|| self.root.clone());
                self.copy = Some(crate::tools::short(&root, path));
                self.message = Some("Path copied".into());
            }
            "tree_remove_root" => self.remove_folder(path),
            "tree_terminal" => self.term_at(dir),
            "tree_copy_path" => {
                self.copy = Some(path.to_string_lossy().into_owned());
                self.message = Some("Path copied".into());
            }
            "tree_reveal" => self.open_with_desktop(dir),
            _ => {}
        }
    }

    /// Runs the Python file in the terminal, saved first.
    pub(super) fn run_file(&mut self) {
        let Some(path) = self.doc.path.clone() else { return };
        if self.doc.buf.modified {
            self.save();
            if self.doc.buf.modified {
                return;
            }
        }
        let (python, _) = self.python();
        let dir = path.parent().map_or_else(|| self.root.clone(), Path::to_path_buf);
        self.term_run(&dir, &format!("{} {}", quote(Path::new(&python)), quote(&path)));
    }

    // ---- the pointer moving ----------------------------------------------

    /// Lights up what is under the pointer. Moving over something that
    /// lights up, or off it, means drawing again.
    pub(super) fn hover(&mut self, x: f32, y: f32) {
        self.tip_motion(x, y);
        let at = Some((x, y));
        self.tree.hover = at.filter(|_| !self.overlaid());
        self.ai.side.hover = self.tree.hover;
        self.typeset.view.hover = self.tree.hover;
        self.outline.hover = self.tree.hover;
        let mut lively = true;
        if let Some((_, popup)) = &mut self.popup {
            popup.motion(x, y);
        } else if let Some((_, menu)) = &mut self.menu {
            menu.motion(x, y);
        } else if let Some(chooser) = &mut self.chooser {
            chooser.motion(x, y);
        } else if self.settings.is_some() {
            self.settings_motion(x, y);
        } else if self.comp_box.is_some_and(|r| crate::ui::inside(r, at)) {
            self.completion_motion(x, y);
        } else {
            let zone = self.zone(x, y, false);
            lively = matches!(zone, Zone::Button | Zone::Close | Zone::Minimize | Zone::Maximize)
                || self.tree.contains(x, y)
                || self.outline.contains(x, y);
        }
        if lively || self.lively {
            self.redraw = true;
        }
        self.lively = lively;
    }

    /// The pointer has left the window: nothing is under it any more.
    pub fn leave(&mut self) {
        self.pointer = (-1.0, -1.0);
        (self.tree.hover, self.ai.side.hover, self.typeset.view.hover) = (None, None, None);
        self.outline.hover = None;
        if let Some((_, popup)) = &mut self.popup {
            popup.leave();
        }
        for t in &mut self.terms {
            t.leave();
        }
        self.doc.leave();
        self.ai.side.transcript.leave();
        self.ai.side.input.leave();
        self.lively = false;
        self.redraw = true;
    }

    /// Files dropped on the window: in the terminal or the message box
    /// their paths are typed; anywhere else they are opened.
    pub fn drop_files(&mut self, paths: Vec<PathBuf>, x: f32, y: f32) {
        if paths.is_empty() {
            return;
        }
        self.redraw = true;
        let quoted: Vec<String> = paths.iter().map(|p| quote(p)).collect();
        self.term_aim(x, y);
        if let Some(t) = self.terms.get_mut(self.term_cur).filter(|t| self.term_open && t.contains(x, y)) {
            self.focus = Focus::Term;
            t.paste(&format!("{} ", quoted.join(" ")));
        } else if self.ai.side.contains(x, y) {
            self.focus = Focus::Chat;
            let names: Vec<String> = paths.iter().map(|p| crate::tools::short(&self.root, p)).collect();
            self.ai.side.input.paste_text(&names.join(" "));
        } else if self.doc.contains(x, y) && self.accepts_markdown_drop() && paths.iter().all(|p| super::is_picture(p)) {
            self.focus = Focus::Doc;
            self.doc.place_at(x, y);
            self.drop_markdown_images(&paths);
        } else {
            for (i, path) in paths.into_iter().enumerate() {
                match (i, path.is_dir()) {
                    (0, true) => self.open_folder(path),
                    (0, false) => self.pick(path, false),
                    _ => super::spawn_self(Some(&path)),
                }
            }
        }
        self.collect();
    }

    /// Offers what was selected with the pointer as the primary selection.
    pub(super) fn offer_primary(&mut self) {
        let text = match self.focus {
            Focus::Term => self.term().and_then(|t| t.selected()),
            Focus::Chat => self.ai.side.transcript.selected().or_else(|| self.ai.side.input.selected()),
            Focus::Doc => match &self.reader {
                Some(reader) => reader.selected(),
                None => self.doc.selected().map(|t| self.plain(&t)),
            },
            Focus::Files => None,
        };
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            self.primary = Some(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::Canvas;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };

    fn draw(a: &mut App) {
        let mut px = vec![0u32; 800 * 600];
        a.draw(&mut Canvas::new(&mut px, 800, 600));
    }

    /// The middle of a context menu's row, by its label.
    fn row(a: &mut App, label: &str) -> (f32, f32) {
        draw(a);
        a.popup.as_ref().unwrap().1.point(label)
    }

    #[test]
    fn a_right_click_offers_what_can_be_done_there() {
        let mut a = App::new(None);
        a.resize(800, 600, 1.0);
        a.paste_text("one two three");
        draw(&mut a);
        // Right on a word, away from any selection: the cursor goes there.
        let (li, top) = a.doc.visible[0];
        let lay = a.doc.layout(li);
        let x = a.doc.column().0 + lay.clusters[5].x + 1.0;
        let y = top + lay.rows[0].y + 4.0;
        a.press_button(x, y, 2);
        assert!(a.popup.is_some() && a.doc.buf.cur == 5);
        // Copy is offered only with something to copy; the pointer lights
        // up the row under it, and a click on it does it.
        let (cx, cy) = row(&mut a, "Select all");
        a.motion(cx, cy);
        assert!(a.redraw);
        a.press(cx, cy, NONE, 1);
        a.release();
        assert!(a.popup.is_none() && a.doc.buf.sel() == (0, 13));
        a.press_button(x, y, 2);
        assert_eq!(a.doc.buf.sel(), (0, 13), "a click on the selection keeps it");
        let (cx, cy) = row(&mut a, "Cut");
        a.press(cx, cy, NONE, 1);
        assert_eq!((a.doc.buf.text.as_str(), a.copy.take().as_deref()), ("", Some("one two three")));
        // Keys work in it too, and Esc closes it.
        a.press_button(x, y, 2);
        a.key(Key::Escape, NONE, None);
        assert!(a.popup.is_none());
        a.press_button(x, y, 2);
        a.key(Key::Down, NONE, None);
        a.key(Key::Enter, NONE, None);
        assert!(a.popup.is_none() && a.paste, "the first row that can be done is Paste");
    }

    #[test]
    fn the_middle_button_pastes_the_primary_selection() {
        let mut a = App::new(None);
        a.resize(800, 600, 1.0);
        a.paste_text("alpha beta");
        draw(&mut a);
        let (li, top) = a.doc.visible[0];
        let lay = a.doc.layout(li);
        let left = a.doc.column().0;
        let at = |i: usize| (left + lay.clusters[i].x + 1.0, top + lay.rows[0].y + 4.0);
        let (x, y) = at(1);
        a.press(x, y, NONE, 2);
        a.release();
        assert_eq!(a.primary.take().as_deref(), Some("alpha"));
        let (x, y) = at(6);
        a.press_button(x, y, 1);
        assert!(a.paste_primary && a.doc.buf.cur == 6);
    }
    fn action(items: &[Item], key: &str) -> Option<bool> {
        items.iter().find_map(|item| match item {
            Item::Do { key: k, enabled, .. } if k == key => Some(*enabled),
            _ => None,
        })
    }

    #[test]
    fn editing_controls_follow_content_history_and_read_only_state() {
        let mut a = App::new(None);
        a.resize(800, 600, 1.0);
        assert_eq!(action(&a.doc_items(false), "undo"), Some(false));
        assert_eq!(action(&a.doc_items(false), "redo"), Some(false));
        a.paste_text("A note");
        assert_eq!(action(&a.doc_items(false), "undo"), Some(true));
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(action(&a.doc_items(false), "undo"), Some(false));
        assert_eq!(action(&a.doc_items(false), "redo"), Some(true));

        a.doc.set_text("```python\nprint(42)\n```\n\nProse".into(), Doc::Markdown);
        let code = a.doc.buf.text.find("print").unwrap();
        a.doc.select(code, code);
        let items = a.doc_items(false);
        assert_eq!(action(&items, "bold"), None);
        assert_eq!(action(&items, "italic"), None);
        assert_eq!(action(&items, "edit_mode"), Some(true));
        let prose = a.doc.buf.text.find("Prose").unwrap();
        a.doc.select(prose, prose);
        assert_eq!(action(&a.doc_items(false), "bold"), Some(true));

        a.doc.read_only = true;
        let items = a.doc_items(false);
        for key in
            ["cut", "paste", "undo", "redo", "bold", "italic", "comment", "edit_mode", "run_file", "suggest", "preview"]
        {
            assert_eq!(action(&items, key), None, "{key}");
        }
        assert_eq!(action(&items, "copy"), Some(false));
        assert_eq!(action(&items, "find"), Some(true));
        assert!(!a.file_menu().entries.iter().any(|e| e.key == "export_pdf"));
    }

    #[test]
    fn notebook_controls_follow_cell_and_kernel_state() {
        let source = serde_json::json!({"nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": [
            {"cell_type": "markdown", "metadata": {}, "source": ["A note"]},
            {"cell_type": "code", "metadata": {}, "source": ["print(42)"], "execution_count": null, "outputs": []}
        ]})
        .to_string();
        let mut a = App::new(None);
        a.resize(800, 600, 1.0);
        let (mut nb, text, raw) = crate::nb::Notebook::open(&source, a.pics.clone()).unwrap();
        a.doc.set_text(text, Doc::Cells);
        nb.attach(&mut a.doc, raw);
        a.nb = Some(nb);
        a.doc.headed = true;
        let markdown = a.doc.cell_body(0).0;
        a.doc.select(markdown, markdown);
        let items = a.doc_items(false);
        for key in ["run", "run_next", "interrupt", "restart", "clear_outputs"] {
            assert_eq!(action(&items, key), None, "{key}");
        }
        assert_eq!(action(&items, "run_all"), Some(true));
        assert_eq!(action(&items, "cell_kind"), Some(true));
        let code = a.doc.cell_body(1).0;
        a.doc.select(code, code);
        assert_eq!(action(&a.doc_items(false), "run"), Some(true));
        assert!(a.run_cell(1));
        assert_eq!(action(&a.doc_items(false), "interrupt"), Some(true));
        assert_eq!(action(&a.doc_items(false), "restart"), Some(true));
        a.settle();
        assert_eq!(action(&a.doc_items(false), "interrupt"), None);
        assert_eq!(action(&a.doc_items(false), "restart"), Some(true));
        assert_eq!(action(&a.doc_items(false), "clear_outputs"), Some(true));
        a.nb.as_mut().unwrap().clear_outputs(&mut a.doc);
        assert_eq!(action(&a.doc_items(false), "clear_outputs"), None);
        a.nb.as_mut().unwrap().restart(&mut a.doc);
        assert_eq!(action(&a.doc_items(false), "restart"), None);
    }

    #[test]
    fn preview_menu_offers_only_the_next_valid_steps() {
        let mut a = App::new(None);
        assert_eq!(action(&a.preview_items(), "preview"), Some(true));
        assert_eq!(action(&a.preview_items(), "preview_open"), None);
        a.typeset.view.no_engine = true;
        assert_eq!(action(&a.preview_items(), "preview"), None);
        assert_eq!(action(&a.preview_items(), "preview_setup"), Some(true));
        a.doc.set_doc(Doc::Python);
        assert!(a.preview_items().is_empty());
        a.typeset.view.pdf = Some("earlier.pdf".into());
        assert!(a.preview_items().is_empty(), "a stale preview cannot expose actions for a different source");
    }
}
