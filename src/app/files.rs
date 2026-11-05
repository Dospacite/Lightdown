//! What can be done with the document as a file, beyond opening and saving
//! it: making a new one, naming it anew, copying it, parting with it. The
//! File menu of the top bar lists those that apply to the current document.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::assist::MenuKind;
use super::{App, Key, Mods, PromptKind, spawn_self, write_file};
use crate::chooser::{Chooser, Chosen};
use crate::tools;
use crate::ui::{Entry, Menu};

/// The File menu, top to bottom: each shortcut's name and what its row says.
const MENU: [(&str, &str); 15] = [
    ("new_window", "New window"),
    ("new_file", "New file or folder\u{2026}"),
    ("open", "Open a file or folder\u{2026}"),
    ("close_tab", "Close tab"),
    ("save", "Save"),
    ("save_as", "Save as\u{2026}"),
    ("save_copy", "Save a copy\u{2026}"),
    ("rename", "Rename or move\u{2026}"),
    ("preview", "Build and preview"),
    ("export_pdf", "Export as PDF\u{2026}"),
    ("export_tex", "Export markdown as LaTeX\u{2026}"),
    ("trash", "Move to the trash\u{2026}"),
    ("reveal", "Show in its folder"),
    ("copy_path", "Copy the path"),
    ("doc_kind", "Show as\u{2026}"),
];

impl App {
    /// File operations that make sense for the current document or tree row.
    fn file_action_visible(&self, id: &str) -> bool {
        match id {
            "preview" | "export_pdf" => self.buildable(),
            "export_tex" => self.latex_exportable(),
            "doc_kind" => self.can_change_kind(),
            "save" => !self.doc.read_only,
            "save_as" | "save_copy" => !self.doc.read_only || self.doc.path.as_deref().is_some_and(Path::is_file),
            "rename" if self.focus == super::Focus::Files && self.files_open => self.tree.has_selection(),
            "rename" | "copy_path" => self.doc.path.is_some(),
            "trash" => self.doc.path.as_ref().is_some_and(|p| p.symlink_metadata().is_ok()),
            "reveal" => self.doc.path.as_ref().and_then(|p| p.parent()).is_some_and(Path::is_dir),
            _ => true,
        }
    }

    /// The list of what can be done with the file, dropping from the top
    /// bar's File.
    pub(super) fn file_menu(&self) -> Menu {
        let entry = |(id, label): &(&str, &str)| Entry::new(id, label, "").keys(self.keys_of(id));
        let mut entries: Vec<Entry> = MENU.iter().filter(|(id, _)| self.file_action_visible(id)).map(entry).collect();
        if self.w < 640.0 * self.scale {
            entries.extend(
                [
                    ("sidebar", "Show agent"),
                    ("files", "Show files"),
                    ("outline", "Show outline"),
                    ("terminal", "Toggle terminal"),
                    ("settings", "Settings"),
                ]
                .iter()
                .map(entry),
            );
        }
        let mut menu = Menu::new("File", entries, "");
        let at = self.hits.iter().find(|(_, hit)| *hit == super::Hit::FileMenu);
        let (x, y) = at.map_or((0.0, self.title_h()), |(r, _)| (r.0, r.1 + r.3));
        menu.anchor = Some((x, y));
        menu.width = 400.0;
        menu
    }

    /// Does one of the things the File menu lists, by its shortcut's name.
    pub(super) fn file_action(&mut self, id: &str) {
        self.comp = None;
        self.message = None;
        match id {
            "file_menu" => self.menu = Some((MenuKind::File, self.file_menu())),
            "sidebar" => self.toggle_side(super::Tab::Agent),
            "files" => self.explorer_toggle(),
            "outline" => self.toggle_outline(),
            "terminal" => self.show_term(!self.term_open),
            "settings" => self.settings = Some(super::Settings::new()),
            "new_window" => spawn_self(None),
            "close_tab" => self.close_file_tab(self.active_file),
            "new_file" => self.ask(PromptKind::New),
            "open" => self.choose(),
            "save" => self.save(),
            "save_as" => self.ask(PromptKind::SaveAs),
            "save_copy" => self.ask(PromptKind::Copy),
            // In the folder view it is the chosen file or folder that is renamed.
            "rename" if self.focus == super::Focus::Files && self.files_open => {
                if let Some((path, _)) = self.tree.selected().filter(|(p, _)| !self.tree.is_root(p)) {
                    self.ask_about(PromptKind::Rename, &path);
                }
            }
            // A document without a file gets its name by being saved.
            "rename" if self.doc.path.is_none() => self.ask(PromptKind::SaveAs),
            "rename" => self.ask(PromptKind::Rename),
            "preview" => self.preview(),
            "export_pdf" if !self.buildable() => {
                self.message = Some("Only markdown and LaTeX are exported as PDF".into())
            }
            "export_pdf" => self.ask(PromptKind::Export),
            "export_tex" if !self.latex_exportable() => {
                self.message = Some("Only markdown is exported as LaTeX".into())
            }
            "export_tex" => self.ask(PromptKind::ExportTex),
            "doc_kind" if self.nb.is_some() => self.message = Some("A notebook is always shown as its cells".into()),
            "doc_kind" if self.reader.as_ref().is_some_and(|r| r.picture() && !r.pages.vector) => {
                self.message = Some("A picture is shown as itself".into())
            }
            "doc_kind" if self.reader.is_some() => self.message = Some("A PDF is shown as its pages".into()),
            "doc_kind" if !self.can_change_kind() => self.message = Some("This file is read-only".into()),
            "doc_kind" => self.open_menu(MenuKind::Kind),
            "trash" | "reveal" | "copy_path" => {
                let on_disk = |p: &PathBuf| id != "trash" || p.symlink_metadata().is_ok();
                let Some(path) = self.doc.path.clone().filter(on_disk) else {
                    self.message = Some("This document is not a file yet".into());
                    return;
                };
                match id {
                    "trash" => self.ask(PromptKind::Trash),
                    "reveal" => self.reveal(&path),
                    _ => {
                        self.copy = Some(path.to_string_lossy().into_owned());
                        self.message = Some("Path copied".into());
                    }
                }
            }
            _ => {}
        }
    }

    /// Puts up the file chooser, on the file's folder.
    fn choose(&mut self) {
        let path = self.doc.path.as_deref();
        let dir = path.and_then(Path::parent).filter(|d| d.is_dir()).unwrap_or(&self.root);
        let name = path.and_then(Path::file_name).map(|n| n.to_string_lossy());
        self.chooser = Some(Chooser::new(dir, name.as_deref()));
    }

    /// A key pressed while the file chooser is up.
    pub(super) fn chooser_key(&mut self, key: Key, mods: Mods, text: Option<&str>) {
        let Some(chooser) = &mut self.chooser else { return };
        match key {
            Key::Char('v') if mods.ctrl => self.paste = true,
            _ => {
                let chosen = chooser.key(key, mods, text);
                self.chosen(chosen);
            }
        }
    }

    /// Carries out what a key or click in the file chooser came to.
    pub(super) fn chosen(&mut self, chosen: Chosen) {
        match chosen {
            Chosen::None => {}
            Chosen::Close => { self.chooser = None; self.git_actions.compare_file = None; self.window.adding = false; },
            Chosen::File(_) if self.window.adding => self.message = Some("Choose a folder to add".into()),
            Chosen::File(path) => {
                self.chooser = None;
                if let Some(left) = self.git_actions.compare_file.take() {
                    match std::fs::read_to_string(&path) {
                        Ok(right) => {
                            let name = |p: Option<&Path>| p.and_then(|p| p.file_name()).map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
                            let title = format!("{} \u{2194} {}", name(self.doc.path.as_deref()), name(Some(&path)));
                            self.git_action(super::git_actions::Action::TextDiff(path, title, left, right))
                        }
                        Err(e) => self.message = Some(format!("Cannot compare file: {e}")),
                    }
                } else { self.open(path); }
            }
            Chosen::Folder(dir) if self.window.adding => self.roots_chosen(dir),
            Chosen::Folder(dir) => {
                if self.git_actions.compare_file.is_some() { self.message = Some("Choose a text file".into()); return; }
                self.chooser = None;
                self.open_folder(dir);
            }
            Chosen::Made(dir) => {
                self.tree.refresh();
                self.message = Some(format!("Created {}", tools::short(&self.root, &dir)));
            }
        }
    }

    /// Where the file is, for the top bar: its folders within the project,
    /// and its name.
    pub(super) fn place(&self) -> (String, String) {
        let Some(path) = &self.doc.path else { return (String::new(), self.name()) };
        let short = tools::short(&self.root, path);
        match short.rsplit_once('/') {
            Some((dir, name)) => (format!("{dir}/"), name.to_string()),
            None => (String::new(), short),
        }
    }

    /// The name a copy is offered: the file's own, marked as a copy.
    pub(super) fn copy_name(&self) -> String {
        let Some(path) = &self.doc.path else { return String::new() };
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let name = match path.extension() {
            Some(ext) => format!("{stem} copy.{}", ext.to_string_lossy()),
            None => format!("{stem} copy"),
        };
        path.with_file_name(name).to_string_lossy().into_owned()
    }

    /// Where a name typed for the file leads. A folder means the file's
    /// own name inside it.
    pub(super) fn target(&self, input: &str) -> PathBuf {
        let path = self.resolve(input);
        match self.doc.path.as_deref().and_then(Path::file_name) {
            Some(name) if input.ends_with('/') || path.is_dir() => path.join(name),
            _ => path,
        }
    }

    /// Whether something is at `path` already, which is then said: nothing
    /// here writes over a file.
    pub(super) fn taken(&mut self, path: &Path) -> bool {
        let taken = path.symlink_metadata().is_ok();
        if taken {
            self.message = Some(format!("{} already exists", tools::short(&self.root, path)));
        }
        taken
    }

    /// Makes an empty file and opens it. A name ending in a slash makes a
    /// folder instead.
    pub(super) fn create(&mut self, input: &str) {
        let path = self.resolve(input);
        if self.taken(&path) {
            return;
        }
        let folder = input.ends_with('/');
        if self.language_before_file("Create",&path,None,super::languages::file_events::Resume::Create(format!("{}{}",path.display(),if folder { "/" } else { "" }))) { return; }
        let made = match folder {
            true => std::fs::create_dir_all(&path),
            false => make_dirs(&path).and_then(|()| std::fs::File::create_new(&path).map(drop)),
        };
        if let Err(e) = made {
            self.message = Some(format!("Cannot create: {e}"));
            return;
        }
        self.language_after_file("Create",&path,None,folder);
        self.tree.refresh();
        self.prompt = None;
        if folder {
            self.message = Some(format!("Created {}", tools::short(&self.root, &path)));
        } else {
            self.open(path);
        }
    }

    /// Gives the file another name or place, on disk and in the window.
    pub(super) fn rename(&mut self, input: &str) {
        let Some(old) = self.doc.path.clone() else { return };
        let new = self.target(input);
        if new == old {
            self.prompt = None;
            return;
        }
        if self.taken(&new) {
            return;
        }
        if self.language_before_file("Rename",&old,Some(&new),super::languages::file_events::Resume::Rename(old.clone(),input.into())) { return; }
        let was_folder = old.is_dir();
        // A file that was never saved has only its name to change.
        if old.symlink_metadata().is_ok() {
            if let Err(e) = make_dirs(&new).and_then(|()| move_file(&old, &new)) {
                self.message = Some(format!("Cannot rename: {e}"));
                return;
            }
        }
        let done = if new.parent() == old.parent() { "Renamed to" } else { "Moved to" };
        self.prompt = None;
        self.message = Some(format!("{done} {}", tools::short(&self.root, &new)));
        self.language_follow_path(&old,&new);
        self.language_after_file("Rename",&old,Some(&new),was_folder);
        self.offer_link_updates(&old, &new);
    }

    /// Writes the document as it stands to another file, and stays with
    /// this one.
    pub(super) fn copy_to(&mut self, input: &str) {
        let path = self.target(input);
        if self.taken(&path) {
            return;
        }
        // What cannot be edited is not all here: the file itself is copied.
        let from = self.doc.path.as_deref().filter(|_| self.doc.read_only);
        let wrote = make_dirs(&path).and_then(|()| match from {
            Some(from) => std::fs::copy(from, &path).map(drop),
            None => write_file(&path, self.contents().as_bytes()),
        });
        match wrote {
            Ok(()) => {
                self.tree.refresh();
                self.prompt = None;
                self.message = Some(format!("Copied to {}", tools::short(&self.root, &path)));
            }
            Err(e) => self.message = Some(format!("Cannot copy: {e}")),
        }
    }

    /// Parts with the file, to the trash or for good, and leaves the window
    /// with an empty document.
    pub(super) fn discard(&mut self, for_good: bool) {
        let Some(path) = self.doc.path.clone() else { return };
        if self.language_before_file("Delete",&path,None,super::languages::file_events::Resume::Delete(path.clone(),for_good)) { return; }
        let gone = match for_good {
            true => std::fs::remove_file(&path).map_err(|e| format!("Cannot delete: {e}")),
            false => trash(&path).map_err(|why| format!("{why}. Shift+Enter deletes it for good")),
        };
        match gone {
            Ok(()) => {
                self.language_after_file("Delete",&path,None,false);
                let name = self.name();
                self.prompt = None;
                self.load(None);
                self.message = Some(format!("{name} {}", if for_good { "is deleted" } else { "is in the trash" }));
            }
            Err(why) => self.message = Some(why),
        }
    }

    /// Opens the file's folder in the desktop's file manager.
    fn reveal(&mut self, path: &Path) {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(&self.root);
        if Command::new("xdg-open").arg(dir).spawn().is_err() {
            self.message = Some("Cannot run xdg-open".into());
        }
    }
}

/// Makes the folders a file is to go into.
pub(super) fn make_dirs(path: &Path) -> std::io::Result<()> {
    path.parent().map_or(Ok(()), std::fs::create_dir_all)
}

/// Moves a file or a folder, copying it where a rename cannot reach: onto
/// another filesystem.
pub(super) fn move_file(old: &Path, new: &Path) -> std::io::Result<()> {
    match std::fs::rename(old, new) {
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_all(old, new)?;
            match old.is_dir() {
                true => std::fs::remove_dir_all(old),
                false => std::fs::remove_file(old),
            }
        }
        done => done,
    }
}

/// Copies a file, or a folder with everything in it. Links are copied as
/// links, not followed.
pub(super) fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    let kind = from.symlink_metadata()?.file_type();
    if kind.is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else if kind.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(drop)
    }
}

/// Moves a file to the desktop's trash, from where it can be restored. The
/// desktop's own tools know where that is for each filesystem and how the
/// file is recorded there.
pub(super) fn trash(path: &Path) -> Result<(), String> {
    let mut why = "Neither gio nor trash-put is installed".to_string();
    for tool in [&["gio", "trash", "--"][..], &["trash-put", "--"]] {
        match Command::new(tool[0]).args(&tool[1..]).arg(path).stdin(Stdio::null()).output() {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => {
                // The last thing said is the reason, after the file's name.
                let said = String::from_utf8_lossy(&out.stderr);
                let reason = said.trim().lines().last().and_then(|l| l.rsplit(": ").next()).unwrap_or("");
                why = if reason.is_empty() { format!("{} failed", tool[0]) } else { reason.to_string() };
            }
            Err(_) => {}
        }
    }
    Err(why)
}

#[cfg(test)]
mod tests {
    use super::super::Hit;
    use super::*;
    use crate::app::{Key, Mods, Zone};
    use crate::gfx::Canvas;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
    const CTRL_ALT: Mods = Mods { ctrl: true, shift: false, alt: true };

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open(path: Option<PathBuf>) -> App {
        let mut a = App::new(path);
        a.resize(900, 600, 1.0);
        a
    }

    /// Clears what a prompt holds and types `text` into it instead.
    fn answer(a: &mut App, text: &str) {
        let p = a.prompt.as_mut().unwrap();
        (p.text, p.cur) = (text.to_string(), text.len());
        a.key(Key::Enter, NONE, None);
    }

    fn draw(a: &mut App) {
        let mut px = vec![0u32; 900 * 600];
        a.draw(&mut Canvas::new(&mut px, 900, 600));
    }

    #[test]
    fn files_are_renamed_moved_and_copied() {
        let dir = folder("rename");
        std::fs::write(dir.join("a.md"), "one").unwrap();
        std::fs::write(dir.join("taken.md"), "other").unwrap();
        let mut a = open(Some(dir.join("a.md")));
        a.paste_text("more ");

        // F2 offers the name, with the cursor before the extension.
        a.key(Key::F(2), NONE, None);
        let p = a.prompt.as_ref().unwrap();
        assert!(p.kind == PromptKind::Rename && p.text[p.cur..] == *".md");
        // Nothing is written over: the prompt stays and says why.
        answer(&mut a, "taken.md");
        assert!(a.prompt.is_some() && a.message.as_deref() == Some("taken.md already exists"));
        assert_eq!(std::fs::read_to_string(dir.join("taken.md")).unwrap(), "other");
        // A new name moves the file; what was not saved stays unsaved.
        answer(&mut a, "b.py");
        assert!(a.prompt.is_none() && !dir.join("a.md").exists());
        assert_eq!(std::fs::read_to_string(dir.join("b.py")).unwrap(), "one");
        assert_eq!((a.doc.path.clone(), a.doc.buf.modified), (Some(dir.join("b.py")), true));
        assert!(a.doc.doc == crate::pane::Doc::Python && a.message.as_deref() == Some("Renamed to b.py"));
        // A folder, made if need be, takes the file under its own name.
        a.key(Key::F(2), NONE, None);
        answer(&mut a, "deep/er/");
        assert_eq!(a.doc.path, Some(dir.join("deep/er/b.py")));
        assert!(dir.join("deep/er/b.py").exists() && a.message.as_deref() == Some("Moved to deep/er/b.py"));

        // A copy holds the document as it stands, and is not opened.
        a.key(Key::Char('s'), CTRL_ALT, None);
        assert!(a.prompt.as_ref().unwrap().text.ends_with("deep/er/b copy.py"));
        a.key(Key::Enter, NONE, None);
        assert_eq!(std::fs::read_to_string(dir.join("deep/er/b copy.py")).unwrap(), "more one");
        assert_eq!(a.doc.path, Some(dir.join("deep/er/b.py")));
        a.key(Key::Char('c'), CTRL_ALT, None);
        assert_eq!(a.copy.take(), Some(dir.join("deep/er/b.py").to_string_lossy().into_owned()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_and_folders_are_made_and_deleted() {
        let dir = folder("make");
        let mut a = open(None);
        // Without a file there is nothing to trash, and renaming is saving.
        a.key(Key::Char('x'), CTRL_ALT, None);
        assert!(a.prompt.is_none() && a.message.is_some());
        a.key(Key::F(2), NONE, None);
        assert!(a.prompt.as_ref().unwrap().kind == PromptKind::SaveAs);
        a.key(Key::Escape, NONE, None);

        // A name ending in a slash makes a folder; another, a file, opened.
        a.key(Key::Char('n'), CTRL_ALT, None);
        answer(&mut a, &format!("{}/notes/", dir.display()));
        assert!(dir.join("notes").is_dir() && a.doc.path.is_none());
        a.key(Key::Char('n'), CTRL_ALT, None);
        answer(&mut a, &format!("{}/notes/day.md", dir.display()));
        let file = dir.join("notes/day.md");
        assert!(file.is_file() && a.doc.path.as_ref() == Some(&file));
        a.key(Key::Char('n'), CTRL_ALT, None);
        answer(&mut a, "day.md");
        assert!(a.prompt.is_some() && a.message.as_deref() == Some("day.md already exists"));
        a.key(Key::Escape, NONE, None);

        // Deleting asks first. Esc keeps the file; Shift+Enter does not,
        // and the window is left with an empty document in its project.
        let root = a.root.clone();
        a.paste_text("text");
        a.key(Key::Char('x'), CTRL_ALT, None);
        a.key(Key::Char('y'), NONE, Some("y"));
        a.key(Key::Escape, NONE, None);
        assert!(file.exists() && a.prompt.is_none());
        a.key(Key::Char('x'), CTRL_ALT, None);
        a.key(Key::Enter, SHIFT, None);
        assert!(!file.exists() && a.doc.path.is_none() && a.doc.buf.text.is_empty() && !a.doc.buf.modified);
        assert_eq!((a.root.clone(), a.message.as_deref()), (root, Some("day.md is deleted")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_is_opened_from_the_chooser() {
        let dir = folder("chooser").canonicalize().unwrap();
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join("notes/day.md"), "one").unwrap();
        std::fs::write(dir.join("a.md"), "two").unwrap();
        let ctrl = Mods { ctrl: true, shift: false, alt: false };
        let mut a = open(None);
        a.root = dir.clone();

        // Ctrl+O lists the folder; keys go to the list, not the document.
        a.key(Key::Char('o'), ctrl, None);
        draw(&mut a);
        assert_eq!(a.chooser.as_ref().unwrap().listing(), ["./", "/new", "../", ">notes/", "a.md"]);
        assert_eq!(a.zone(450.0, 300.0, false), Zone::Plain);
        a.key(Key::Char('q'), NONE, Some("q"));
        assert_eq!((a.chooser.as_ref().unwrap().listing().len(), a.doc.buf.text.as_str()), (0, ""));
        a.key(Key::Escape, NONE, None);
        assert!(a.chooser.is_none() && a.doc.path.is_none());

        // A path that is pasted leads to its file, which Enter opens.
        a.file_action("open");
        a.paste_text(&format!("{}/notes/day.md\n", dir.display()));
        assert_eq!(a.chooser.as_ref().unwrap().listing(), [">day.md"]);
        a.key(Key::Enter, NONE, None);
        assert!(a.chooser.is_none());
        assert_eq!((a.doc.path.clone(), a.doc.buf.text.as_str()), (Some(dir.join("notes/day.md")), "one"));

        // It starts in the file's folder, on the file; a click on a row
        // does what Enter does, and one outside the panel closes it.
        a.key(Key::Char('o'), ctrl, None);
        draw(&mut a);
        assert_eq!(a.chooser.as_ref().unwrap().listing(), ["./", "/new", "../", ">day.md"]);
        let (x, y) = a.chooser.as_ref().unwrap().point("..");
        a.press(x, y, NONE, 1);
        assert_eq!(a.chooser.as_ref().unwrap().listing(), ["./", "/new", "../", ">notes/", "a.md"]);
        a.press(2.0, 300.0, NONE, 1);
        assert!(a.chooser.is_none() && a.doc.path == Some(dir.join("notes/day.md")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn new_files_go_in_the_window_s_folder() {
        let dir = folder("here").canonicalize().unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // A window opened on a folder, with no file: names are typed from
        // that folder, and a bare name lands in it.
        let mut a = open(Some(dir.clone()));
        a.key(Key::Char('n'), CTRL_ALT, None);
        assert_eq!(a.prompt.as_ref().unwrap().text, format!("{}/", dir.display()));
        answer(&mut a, "plans/");
        a.key(Key::Char('n'), CTRL_ALT, None);
        answer(&mut a, "idea.md");
        assert!(dir.join("plans").is_dir() && dir.join("idea.md").is_file());
        assert_eq!(a.doc.path, Some(dir.join("idea.md")));
        // With a file open, its own folder is the place.
        a.load(Some(dir.join("sub/a.md")));
        a.key(Key::Char('n'), CTRL_ALT, None);
        answer(&mut a, "b.md");
        assert!(dir.join("sub/b.md").is_file());
        // Saving a document that has no file starts there too.
        let mut a = open(Some(dir.clone()));
        a.paste_text("text");
        a.key(Key::Char('s'), Mods { ctrl: true, shift: false, alt: false }, None);
        answer(&mut a, "saved.md");
        assert_eq!(std::fs::read_to_string(dir.join("saved.md")).unwrap(), "text\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_is_opened_from_the_chooser() {
        let dir = folder("open-folder").canonicalize().unwrap();
        std::fs::create_dir_all(dir.join("notes/deep")).unwrap();
        std::fs::write(dir.join("notes/deep/day.md"), "one").unwrap();
        let ctrl = Mods { ctrl: true, shift: false, alt: false };
        let mut a = open(None);
        a.root = dir.clone();

        // Ctrl+Enter on a folder makes it the window's project, shown in
        // the sidebar, with nothing open yet.
        a.key(Key::Char('o'), ctrl, None);
        a.key(Key::Enter, ctrl, None);
        assert!(a.chooser.is_none() && a.doc.path.is_none());
        assert_eq!((a.root.clone(), a.folders.first().cloned()), (dir.join("notes"), Some(dir.join("notes"))));
        assert!(a.files_open && a.focus == super::super::Focus::Files);
        // A file deeper in it keeps the folder as the project, though it
        // has no repository to say so.
        a.file_action("open");
        a.paste_text(&format!("{}/notes/deep/day.md", dir.display()));
        a.key(Key::Enter, NONE, None);
        assert_eq!((a.doc.path.clone(), a.root.clone()), (Some(dir.join("notes/deep/day.md")), dir.join("notes")));
        // A file outside it is a project of its own again.
        a.load(Some(dir.join("b.md")));
        assert_eq!((a.root.clone(), a.folders.first().cloned()), (dir.clone(), None));

        // A folder given at launch opens the same way.
        let a = open(Some(dir.join("notes")));
        assert!(a.doc.path.is_none() && a.root == dir.join("notes") && a.files_open);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_top_bar_leads_to_the_file_menu() {
        let dir = folder("bar");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let mut a = open(Some(dir.join("docs/a.md")));
        assert_eq!(a.place(), ("docs/".to_string(), "a.md".to_string()));
        draw(&mut a);
        let at = |a: &App, hit: Hit| {
            let (r, _) = a.hits.iter().find(|(_, h)| *h == hit).unwrap();
            (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
        };
        // The bar's controls are buttons; the rest of it is not.
        let (x, y) = at(&a, Hit::FileMenu);
        let free = a.hits.iter().find(|(_, h)| *h == Hit::FileMenu).unwrap().0;
        let gap = free.0 + free.2 + 4.0;
        assert_eq!((a.zone(x, y, false), a.zone(gap, y, false)), (Zone::Button, Zone::Plain));
        a.csd = true;
        draw(&mut a);
        let (cx, cy) = at(&a, Hit::Close);
        assert_eq!((a.zone(gap, y, false), a.zone(cx, cy, false)), (Zone::Title, Zone::Close));
        a.csd = false;
        draw(&mut a);

        // File lists what can be done; typing narrows it, Enter does it.
        a.press(x, y, NONE, 1);
        assert!(matches!(&a.menu, Some((MenuKind::File, m))
            if m.entries.iter().any(|e| e.key == "rename")
                && !m.entries.iter().any(|e| e.key == "trash" || e.key == "reveal")));
        assert_eq!(a.zone(x, y, false), Zone::Plain);
        for ch in "rename".chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
        a.key(Key::Enter, NONE, None);
        assert!(a.menu.is_none() && a.prompt.as_ref().unwrap().kind == PromptKind::Rename);
        draw(&mut a);
        a.key(Key::Escape, NONE, None);

        let (x, y) = at(&a, Hit::TogglePanel);
        let was = a.side_open;
        a.press(x, y, NONE, 1);
        assert_eq!(a.side_open, !was);
        draw(&mut a);
        let (x, y) = at(&a, Hit::Settings);
        a.press(x, y, NONE, 1);
        assert!(a.settings.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_menu_matches_document_capabilities() {
        let dir = folder("capabilities");
        let notebook = serde_json::json!({"nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": [
            {"cell_type": "code", "metadata": {}, "source": ["1 + 1"], "execution_count": null, "outputs": []}
        ]})
        .to_string();
        for (name, source, pdf, tex, kind) in [
            ("notes.md", "# Notes", true, true, true),
            ("paper.tex", "\\section{Notes}", true, false, true),
            ("script.py", "print(42)", false, false, true),
            ("main.rs", "fn main() {}", false, false, true),
            ("notes.txt", "Notes", false, false, true),
            ("book.ipynb", notebook.as_str(), false, false, false),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, source).unwrap();
            let mut a = open(Some(path));
            let menu = a.file_menu();
            let has = |key| menu.entries.iter().any(|e| e.key == key);
            assert_eq!(has("preview"), pdf, "{name}");
            assert_eq!(has("export_pdf"), pdf, "{name}");
            assert_eq!(has("export_tex"), tex, "{name}");
            assert_eq!(has("doc_kind"), kind, "{name}");
            assert!(has("save") && has("rename") && has("trash") && has("copy_path"));
            draw(&mut a);
            assert_eq!(a.hits.iter().any(|(_, hit)| *hit == Hit::Preview), pdf, "{name}");
            assert_eq!(a.kind_hit.is_some(), kind, "{name}");
            if !pdf {
                a.file_action("export_pdf");
                assert!(a.prompt.is_none(), "{name}: shortcut cannot start an unsupported PDF export");
            }
            if !tex {
                a.file_action("export_tex");
                assert!(a.prompt.is_none(), "{name}: shortcut cannot start an unsupported LaTeX export");
            }
        }
        let mut untitled = open(None);
        let menu = untitled.file_menu();
        for key in ["rename", "trash", "reveal", "copy_path"] {
            assert!(!menu.entries.iter().any(|e| e.key == key), "{key}");
        }
        // Switching presentation changes export eligibility immediately.
        untitled.set_kind("python");
        assert!(!untitled.file_menu().entries.iter().any(|e| e.key == "export_pdf"));
        untitled.set_kind("markdown");
        assert!(untitled.file_menu().entries.iter().any(|e| e.key == "export_pdf"));

        let path = dir.join("pages.pdf");
        std::fs::write(&path, crate::pdf::tests::sample(1, 100, 100)).unwrap();
        let mut pdf = open(Some(path));
        assert!(pdf.reader.is_some());
        let menu = pdf.file_menu();
        for key in ["save", "preview", "export_pdf", "export_tex", "doc_kind"] {
            assert!(!menu.entries.iter().any(|e| e.key == key), "PDF: {key}");
        }
        assert!(menu.entries.iter().any(|e| e.key == "save_copy"));
        draw(&mut pdf);
        assert!(pdf.kind_hit.is_none());
        pdf.set_kind("markdown");
        assert!(pdf.reader.is_some() && !pdf.buildable());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
