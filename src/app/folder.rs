//! The folder view at work in the window: the keys and clicks that reach
//! it, and the opening of what is picked there.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::files::{copy_all, make_dirs, move_file, trash};
use super::{App, Focus, Key, Mods, PromptKind, spawn_self};
use crate::{pane, tools};

/// Whether a file can be shown as text. One with a zero byte near its start
/// is a picture, an archive or some other thing the editor has no use for.
fn is_text(path: &Path) -> bool {
    let mut head = [0u8; 4096];
    let read = std::fs::File::open(path).and_then(|mut f| f.read(&mut head));
    read.is_ok_and(|n| !head[..n].contains(&0))
}

/// A free name for a copy beside a file or folder: `a copy.md`, and then
/// `a copy 2.md` and on, for a file that already has a copy.
fn copy_beside(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !path.is_dir() => (stem.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    (1..)
        .map(|n| match n {
            1 => path.with_file_name(format!("{stem} copy{ext}")),
            n => path.with_file_name(format!("{stem} copy {n}{ext}")),
        })
        .find(|p| p.symlink_metadata().is_err())
        .unwrap()
}

impl App {
    /// Asks for something to be done to a file or folder of the folder
    /// view: a new name for it, or whether it is to go.
    pub(super) fn ask_about(&mut self, kind: PromptKind, path: &Path) {
        self.ask(kind);
        let Some(p) = &mut self.prompt else { return };
        p.subject = Some(path.to_path_buf());
        if kind == PromptKind::Rename {
            p.text = path.to_string_lossy().into_owned();
            // A name is changed more often than its extension.
            let ext = path.extension().filter(|_| path.is_file()).map_or(0, |e| e.len() + 1);
            p.cur = p.text.len() - ext;
        }
    }

    /// The open document has moved with a file or folder from `old` to
    /// `new`: it is known by its new path.
    fn followed(&mut self, old: &Path, new: &Path) {
        self.move_stored_paths(old, new);
        let Some(doc) = self.doc.path.clone() else { return };
        if let Ok(rest) = doc.strip_prefix(old) {
            let moved = if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) };
            self.set_path(moved);
        }
    }

    /// Gives a file or folder of the folder view another name or place.
    /// A name ending in `/`, or that of a folder, moves it into that folder.
    pub(super) fn rename_path(&mut self, old: &Path, input: &str) {
        if self.doc.path.as_deref() == Some(old) {
            return self.rename(input);
        }
        let mut new = pane::resolve(Some(old), input);
        if (input.ends_with('/') || new.is_dir()) && new != old {
            new = new.join(old.file_name().unwrap_or_default());
        }
        if new == old {
            self.prompt = None;
            return;
        }
        if new.starts_with(old) {
            self.message = Some("A folder cannot go into itself".into());
            return;
        }
        if self.taken(&new) {
            return;
        }
        if self.language_before_file("Rename",old,Some(&new),super::languages::file_events::Resume::Rename(old.into(),input.into())) { return; }
        let was_folder = old.is_dir();
        if let Err(e) = make_dirs(&new).and_then(|()| move_file(old, &new)) {
            self.message = Some(format!("Cannot rename: {e}"));
            return;
        }
        self.prompt = None;
        let done = if new.parent() == old.parent() { "Renamed to" } else { "Moved to" };
        self.message = Some(format!("{done} {}", tools::short(&self.root, &new)));
        self.followed(old, &new);
        self.language_after_file("Rename",old,Some(&new),was_folder);
        self.tree.refresh();
        self.offer_link_updates(old, &new);
    }

    /// Parts with a file or folder of the folder view, to the trash or for
    /// good. An open document that was in it is closed, unless it holds
    /// changes not saved, which are kept to be saved anew.
    pub(super) fn discard_path(&mut self, path: &Path, for_good: bool) {
        if self.doc.path.as_deref() == Some(path) && !self.doc.buf.modified {
            return self.discard(for_good);
        }
        if self.language_before_file("Delete",path,None,super::languages::file_events::Resume::Delete(path.into(),for_good)) { return; }
        let was_folder = path.is_dir();
        let gone = match (for_good, path.is_dir()) {
            (true, true) => std::fs::remove_dir_all(path).map_err(|e| format!("Cannot delete: {e}")),
            (true, false) => std::fs::remove_file(path).map_err(|e| format!("Cannot delete: {e}")),
            (false, _) => trash(path).map_err(|why| format!("{why}. Shift+Enter deletes it for good")),
        };
        if let Err(why) = gone {
            self.message = Some(why);
            return;
        }
        self.language_after_file("Delete",path,None,was_folder);
        self.prompt = None;
        self.remove_deleted_tabs(path);
        let name = tools::short(&self.root, path);
        let mut said = format!("{name} {}", if for_good { "is deleted" } else { "is in the trash" });
        if self.doc.path.as_deref().is_some_and(|d| d.starts_with(path)) {
            if self.doc.buf.modified {
                said += ". The open document keeps its changes, to be saved anew";
            } else {
                self.load(None);
            }
        }
        if self.file_clip.as_ref().is_some_and(|(clip, _)| clip.starts_with(path)) {
            self.file_clip = None;
        }
        self.message = Some(said);
        self.tree.refresh();
    }

    /// Makes a copy of a file or folder beside it.
    pub(super) fn duplicate(&mut self, path: &Path) {
        let copy = copy_beside(path);
        if self.language_before_file("Create",&copy,None,super::languages::file_events::Resume::Duplicate(path.into())) { return; }
        match copy_all(path, &copy) {
            Ok(()) => { self.language_after_file("Create",&copy,None,path.is_dir()); self.message = Some(format!("Copied to {}", tools::short(&self.root, &copy))); }
            Err(e) => self.message = Some(format!("Cannot copy: {e}")),
        }
        self.tree.refresh();
    }

    /// Puts what was copied or cut in the folder view into a folder. A copy
    /// that would land on a name in use gets a name of its own; a file cut
    /// is never written over.
    pub(super) fn paste_files(&mut self, dir: &Path) {
        let Some((from, cut)) = self.file_clip.clone() else { return };
        if from.symlink_metadata().is_err() {
            self.file_clip = None;
            self.message = Some(format!("{} is no longer there", tools::short(&self.root, &from)));
            return self.tree.refresh();
        }
        let mut to = dir.join(from.file_name().unwrap_or_default());
        if cut && to == from {
            self.file_clip = None;
            return;
        }
        if dir.starts_with(&from) {
            self.message = Some("A folder cannot go into itself".into());
            return;
        }
        if !cut && to.symlink_metadata().is_ok() { to = copy_beside(&to); }
        if self.language_before_file(if cut { "Rename" } else { "Create" },if cut { &from } else { &to },if cut { Some(&to) } else { None },super::languages::file_events::Resume::Paste(dir.into(),(from.clone(),cut))) { return; }
        let was_folder = from.is_dir();
        let done = if cut {
            if self.taken(&to) {
                return;
            }
            move_file(&from, &to).map(|()| "Moved to")
        } else {
            if to.symlink_metadata().is_ok() {
                to = copy_beside(&to);
            }
            copy_all(&from, &to).map(|()| "Copied to")
        };
        match done {
            Ok(done) => {
                self.language_after_file(if cut { "Rename" } else { "Create" },if cut { &from } else { &to },if cut { Some(&to) } else { None },was_folder);
                self.message = Some(format!("{done} {}", tools::short(&self.root, &to)));
                if cut {
                    self.followed(&from, &to);
                    self.file_clip = None;
                    self.offer_link_updates(&from, &to);
                }
            }
            Err(e) => self.message = Some(format!("Cannot paste: {e}")),
        }
        self.tree.refresh();
    }

    /// A key pressed while the folder view has the keyboard.
    pub(super) fn tree_key(&mut self, key: Key, mods: Mods) {
        if key == Key::Escape {
            self.focus = Focus::Doc;
        } else if key == Key::Delete {
            if let Some((path, _)) = self.tree.selected().filter(|(p, _)| !self.tree.is_root(p)) {
                self.ask_about(PromptKind::Trash, &path);
            }
        } else if let Some(path) = self.tree.key(key) {
            self.pick(path, mods.ctrl);
        }
    }

    /// Shows a file picked in the folder view: in this window, unless
    /// another was asked for or something here would be lost by it.
    pub(super) fn pick(&mut self, path: PathBuf, apart: bool) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if !path.is_file() {
            self.message = Some(format!("{name} cannot be opened"));
            return self.tree.refresh();
        }
        if !is_text(&path) && !crate::pdf::is_pdf(&path) && !super::is_picture(&path) {
            let ran = std::process::Command::new("xdg-open").arg(&path).spawn();
            let said = if ran.is_ok() {
                format!("{name} is not text, so it opened in its own application")
            } else {
                "Cannot run xdg-open".into()
            };
            self.message = Some(said);
            return;
        }
        if apart {
            spawn_self(Some(&path));
            return;
        }
        self.open_tab(path);
        self.focus = Focus::Doc;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Hit, PromptKind, Tab, Zone};
    use super::*;
    use crate::gfx::Canvas;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
    const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-folder-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/a.md"), "# A").unwrap();
        std::fs::write(dir.join("b.py"), "x = 1").unwrap();
        dir
    }

    fn open(path: PathBuf) -> App {
        let mut a = App::new(Some(path));
        a.resize(900, 600, 1.0);
        a
    }

    fn draw(a: &mut App) {
        let mut px = vec![0u32; 900 * 600];
        a.draw(&mut Canvas::new(&mut px, 900, 600));
    }

    #[test]
    fn the_folder_view_opens_files() {
        let dir = project("open");
        let mut a = open(dir.join("docs/a.md"));
        // Its keys open the sidebar on the folder, unfolded to the file.
        a.key(Key::Char('e'), CTRL_SHIFT, None);
        assert!(a.files_open && a.focus == Focus::Files);
        draw(&mut a);
        assert_eq!(a.tree.listing(), ["docs/", ">  a.md", "b.py"]);
        // The keys go to the tree, not into the document.
        a.key(Key::Char('x'), NONE, Some("x"));
        a.key(Key::Down, NONE, None);
        assert_eq!((a.doc.buf.text.as_str(), a.tree.listing()[2].as_str()), ("# A", ">b.py"));
        // Enter opens the file here and hands the keyboard to it.
        a.key(Key::Enter, NONE, None);
        assert_eq!((a.doc.path.clone(), a.doc.buf.text.as_str()), (Some(dir.join("b.py")), "x = 1"));
        assert!(a.focus == Focus::Doc && a.doc.doc == crate::pane::Doc::Python && a.files_open);
        a.key(Key::Char('y'), NONE, Some("y"));
        assert_eq!(a.doc.buf.text, "yx = 1");

        // A click on a folder unfolds it, and the tree has the keyboard.
        draw(&mut a);
        let (x, y) = a.tree.point("docs");
        assert_eq!(a.zone(x, y, false), Zone::Button);
        a.press(x, y, NONE, 1);
        assert_eq!(a.tree.listing(), [">docs/", "b.py"]);
        assert!(a.focus == Focus::Files && a.doc.path == Some(dir.join("b.py")));
        a.press(x, y, NONE, 1);
        draw(&mut a);
        // Opening another file retains the edited file in its own tab.
        let (x, y) = a.tree.point("a.md");
        a.press(x, y, NONE, 1);
        assert!(a.doc.path == Some(dir.join("docs/a.md")) && a.focus == Focus::Doc);
        assert_eq!(a.file_tabs.len(), 2);
        a.activate_file(0);
        assert_eq!(a.doc.buf.text, "yx = 1");
        a.key(Key::Char('s'), CTRL, None);
        assert_eq!(std::fs::read_to_string(dir.join("b.py")).unwrap(), "yx = 1\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_are_handled_from_the_folder_views_menu() {
        use super::super::context::Target;
        let dir = project("ops");
        let mut a = open(dir.join("b.py"));
        a.key(Key::Char('e'), CTRL_SHIFT, None);
        draw(&mut a);
        let pick = |a: &mut App, path: &Path, key: &str| {
            let target = Target::Tree { path: path.to_path_buf(), folder: path.is_dir() };
            a.context_pick(target, key);
        };
        // Duplicates are named beside the file, each its own.
        pick(&mut a, &dir.join("b.py"), "tree_duplicate");
        pick(&mut a, &dir.join("b.py"), "tree_duplicate");
        assert!(dir.join("b copy.py").is_file() && dir.join("b copy 2.py").is_file());
        // Renaming starts from the path, the name chosen; a name in a
        // folder moves it there.
        pick(&mut a, &dir.join("b copy.py"), "tree_rename");
        let p = a.prompt.as_mut().unwrap();
        assert!(p.kind == PromptKind::Rename && p.text[p.cur..] == *".py");
        (p.text, p.cur) = ("docs/c.py".into(), 9);
        a.key(Key::Enter, NONE, None);
        assert!(dir.join("docs/c.py").is_file() && !dir.join("b copy.py").exists());
        assert_eq!(a.message.as_deref(), Some("Moved to docs/c.py"));
        // The open file, cut and pasted into a folder, goes with its window.
        pick(&mut a, &dir.join("b.py"), "tree_cut");
        pick(&mut a, &dir.join("docs"), "tree_paste");
        assert!(dir.join("docs/b.py").is_file() && a.doc.path == Some(dir.join("docs/b.py")));
        // A copy pasted where its name is taken gets another.
        pick(&mut a, &dir.join("docs/b.py"), "tree_copy");
        pick(&mut a, &dir.join("docs"), "tree_paste");
        assert!(dir.join("docs/b copy.py").is_file() && a.file_clip.is_some());
        // F2 and Delete act on the folder view's choice while it has the keys.
        draw(&mut a);
        assert!(a.focus == Focus::Files);
        a.key(Key::Home, NONE, None);
        a.key(Key::F(2), NONE, None);
        assert_eq!(a.prompt.as_ref().unwrap().subject.as_deref(), Some(dir.join("docs").as_path()));
        a.key(Key::Escape, NONE, None);
        a.key(Key::Delete, NONE, None);
        assert!(a.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::Trash));
        // Deleted for good, the folder takes the open file with it.
        a.key(Key::Enter, Mods { ctrl: false, shift: true, alt: false }, None);
        assert!(!dir.join("docs").exists() && a.doc.path.is_none() && a.file_clip.is_none());
        assert_eq!(a.message.as_deref(), Some("docs is deleted"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_and_agent_open_and_close_independently() {
        let dir = project("tabs");
        let mut a = open(dir.join("b.py"));
        draw(&mut a);
        let at = |a: &App, hit: Hit| {
            let (r, _) = a.hits.iter().find(|(_, h)| *h == hit).unwrap();
            (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
        };
        // Each toggle of the title bar opens its own panel, and closes it again.
        let (x, y) = at(&a, Hit::ToggleFiles);
        a.press(x, y, NONE, 1);
        assert!(a.files_open && a.tree.contains(20.0, 300.0));
        let (x, y) = at(&a, Hit::TogglePanel);
        a.press(x, y, NONE, 1);
        assert!(a.side_open && a.tab == Tab::Agent && a.focus == Focus::Chat);
        assert!(a.tree.contains(20.0, 300.0) && a.ai.side.contains(880.0, 300.0));
        a.press(x, y, NONE, 1);
        assert!(!a.side_open && a.files_open && a.focus == Focus::Doc);

        // The keys go to the folder and back, and from the one to the other.
        a.key(Key::Char('e'), CTRL_SHIFT, None);
        a.key(Key::Char('e'), CTRL_SHIFT, None);
        assert!(a.files_open && a.focus == Focus::Doc);
        a.key(Key::Char('l'), CTRL, None);
        assert!(a.tab == Tab::Agent && a.focus == Focus::Chat);
        a.key(Key::Char('e'), CTRL_SHIFT, None);
        assert!(a.files_open && a.side_open && a.focus == Focus::Files);
        a.key(Key::Char('l'), CTRL_SHIFT, None);
        assert!(!a.side_open && !a.files_open);

        // What is made in the project shows without being asked for.
        a.key(Key::Char('e'), CTRL_SHIFT, None);
        assert_eq!(a.tree.listing(), ["docs/", ">b.py"]);
        a.key(Key::Char('n'), Mods { ctrl: true, shift: false, alt: true }, None);
        let p = a.prompt.as_mut().unwrap();
        (p.text, p.cur) = ("notes/".to_string(), 6);
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.tree.listing(), ["docs/", "notes/", ">b.py"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
