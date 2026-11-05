//! The folders of a window. A window opened on folders has them as its
//! roots, each listed in the explorer and searched by the project-wide
//! tools; the root holding the active file is the project that settings,
//! git, tasks and the agent work in. A window without folders has the
//! active file's own project as its one root.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{App, Entry, Focus, Menu, MenuKind, Tab};
use crate::chooser::Chooser;

/// The end of a workspace file's name.
pub(crate) const WORKSPACE_EXT: &str = "lightdown-workspace";

/// The most folders a window holds.
const MOST: usize = 64;

/// What a window keeps about its folders besides the list of them.
#[derive(Default)]
pub(super) struct Window {
    /// The workspace file the window was opened from; it is kept up to
    /// date as folders are added and removed.
    pub workspace: Option<PathBuf>,
    /// Names given to folders by the workspace file.
    pub names: Vec<(PathBuf, String)>,
    /// The folder chosen for the agent; without one it works in the
    /// active file's.
    pub agent: Option<PathBuf>,
    /// The agent's project for the turn under way.
    pub turn: Option<PathBuf>,
    /// The file chooser is up to add a folder.
    pub adding: bool,
    /// A terminal is being started in the root chosen for it.
    pub terminal_chosen: bool,
}

/// Whether a path names a workspace file.
pub(crate) fn is_workspace(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == WORKSPACE_EXT) && path.is_file()
}

/// The folders a workspace file lists, each with the name it gives, if
/// any. Relative paths are taken from the file's own folder.
pub(crate) fn read_workspace(path: &Path) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let base = path.parent().unwrap_or(Path::new("/"));
    let mut out: Vec<(PathBuf, Option<String>)> = Vec::new();
    for item in value["folders"].as_array().ok_or("Expected a folders array")? {
        let given =
            item["path"].as_str().or(item.as_str()).filter(|p| !p.is_empty()).ok_or("Each folder needs a path")?;
        let dir = match given.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
            None => base.join(given),
        };
        let dir = dir.canonicalize().map_err(|e| format!("{given}: {e}"))?;
        if !dir.is_dir() {
            return Err(format!("{given} is not a folder"));
        }
        if !out.iter().any(|(d, _)| *d == dir) {
            out.push((dir, item["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string)));
        }
    }
    if out.is_empty() {
        return Err("The workspace lists no folders".into());
    }
    out.truncate(MOST);
    Ok(out)
}

/// The text of a workspace file at `path` listing `folders`, relative to
/// the file where they are below its folder.
pub(crate) fn workspace_text(path: &Path, folders: &[PathBuf], names: &[(PathBuf, String)]) -> String {
    let base = path.parent().unwrap_or(Path::new("/"));
    let items: Vec<Value> = folders
        .iter()
        .map(|dir| {
            let shown = match dir.strip_prefix(base) {
                Ok(rest) if rest.as_os_str().is_empty() => ".".to_string(),
                Ok(rest) => rest.to_string_lossy().into_owned(),
                Err(_) => dir.to_string_lossy().into_owned(),
            };
            match names.iter().find(|(d, _)| d == dir) {
                Some((_, name)) => json!({ "path": shown, "name": name }),
                None => json!({ "path": shown }),
            }
        })
        .collect();
    serde_json::to_string_pretty(&json!({ "folders": items })).unwrap() + "\n"
}

/// A folder's own name.
pub(crate) fn name_of(dir: &Path) -> String {
    dir.file_name().map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

/// The folder of `roots` holding `path`, the deepest if they nest.
pub(crate) fn holding<'a>(roots: &'a [PathBuf], path: &Path) -> Option<&'a PathBuf> {
    roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.as_os_str().len())
}

/// A path within the folder of `roots` that holds it.
pub(crate) fn short_in(roots: &[PathBuf], path: &Path) -> String {
    match holding(roots, path) {
        Some(root) => crate::tools::short(root, path),
        None => path.to_string_lossy().into_owned(),
    }
}

/// The files worth searching in every folder, as `search::files` lists
/// them for one.
pub(crate) fn files_in(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        // A folder inside another is listed by the outer one.
        if roots.iter().any(|r| r != root && root.starts_with(r)) {
            continue;
        }
        out.extend(super::search::files(root));
    }
    out
}

impl App {
    /// A window with several folders, as `lightdown a b` opens.
    pub fn with_folders(dirs: Vec<PathBuf>) -> App {
        let mut app = App::new(None);
        app.show_folders(dirs);
        app
    }

    /// The window's roots: its folders, or else the active project.
    pub(super) fn roots(&self) -> Vec<PathBuf> {
        if !self.folders.is_empty() {
            self.folders.clone()
        } else if self.root.as_os_str().is_empty() {
            Vec::new()
        } else {
            vec![self.root.clone()]
        }
    }

    /// What the title bar calls a window of several folders: the active
    /// file's folder, or the first and how many more.
    pub(super) fn roots_label(&self) -> Option<String> {
        if !self.multi_root() {
            return None;
        }
        if let Some(root) = self.doc.path.as_deref().and_then(|p| self.root_of(p)) {
            return Some(self.root_name(&root));
        }
        Some(format!("{} +{}", self.root_name(&self.folders[0]), self.folders.len() - 1))
    }

    /// Whether the window has more than one folder.
    pub(super) fn multi_root(&self) -> bool {
        self.folders.len() > 1
    }

    /// The window's folder holding a path.
    pub(super) fn root_of(&self, path: &Path) -> Option<PathBuf> {
        holding(&self.folders, path).cloned()
    }

    /// Whether a path lies in one of the window's roots.
    pub(super) fn in_roots(&self, path: &Path) -> bool {
        holding(&self.roots(), path).is_some()
    }

    /// What a root is called: the name the workspace gives it, or its own,
    /// with its parent's when two roots share a name.
    pub(super) fn root_name(&self, root: &Path) -> String {
        if let Some((_, name)) = self.window.names.iter().find(|(d, _)| d == root) {
            return name.clone();
        }
        let name = name_of(root);
        let twins = self.folders.iter().filter(|f| name_of(f) == name).count();
        match root.parent().filter(|_| twins > 1) {
            Some(parent) => format!("{name} ({})", name_of(parent)),
            None => name,
        }
    }

    /// A path as lists show it: within its root, after the root's name when
    /// the window has several.
    pub(super) fn root_short(&self, path: &Path) -> String {
        match holding(&self.folders, path).filter(|_| self.multi_root()) {
            Some(root) => format!("{}/{}", self.root_name(root), crate::tools::short(root, path)),
            None => crate::tools::short(&self.root, path),
        }
    }

    /// Runs `f` with another root as the active project, then puts the
    /// active file's back. What `f` opens keeps its own project.
    pub(super) fn with_root<T>(&mut self, root: &Path, f: impl FnOnce(&mut App) -> T) -> T {
        let (saved, doc) = (std::mem::replace(&mut self.root, root.to_path_buf()), self.doc.path.clone());
        let out = f(self);
        if self.doc.path == doc {
            self.root = saved;
        }
        out
    }

    /// The project the agent works in: the turn's, the folder chosen for
    /// it, or the active file's.
    pub(super) fn agent_root(&self) -> PathBuf {
        let window = |r: &&PathBuf| self.folders.contains(r);
        let turn = self.window.turn.as_ref().filter(|_| self.ai.busy());
        turn.or(self.window.agent.as_ref().filter(window)).filter(window).cloned().unwrap_or_else(|| self.root.clone())
    }

    /// Runs agent work in the agent's project.
    pub(super) fn as_agent<T>(&mut self, f: impl FnOnce(&mut App) -> T) -> T {
        let root = self.agent_root();
        if root == self.root { f(self) } else { self.with_root(&root, f) }
    }

    /// A path the agent named for reading: in its project, or by its full
    /// path in another of the window's roots.
    pub(super) fn agent_read_path(&self, path: &str) -> Result<PathBuf, String> {
        let own = crate::tools::locate(&self.root, path);
        // A path that is there in the project is the project's.
        if own.as_ref().is_ok_and(|p| p.exists()) || !self.multi_root() {
            return own;
        }
        for root in self.folders.iter().filter(|r| **r != self.root) {
            let named = format!("{}/", self.root_name(root));
            let rest = path.strip_prefix(&named).map(str::to_string);
            let tried = match rest {
                Some(rest) => crate::tools::locate(root, &rest),
                None if Path::new(path).starts_with(root) => crate::tools::locate(root, path),
                None => continue,
            };
            if tried.as_ref().is_ok_and(|p| p.exists()) || own.is_err() && tried.is_ok() {
                return tried;
            }
        }
        own
    }

    /// The other roots, for the agent's system prompt.
    pub(super) fn agent_other_roots(&self) -> String {
        let others: Vec<String> = self
            .folders
            .iter()
            .filter(|r| **r != self.root)
            .map(|r| format!("- {} ({})", self.root_name(r), r.display()))
            .collect();
        if others.is_empty() {
            return String::new();
        }
        format!(
            "\nOther folders open in this window, which read_file, list_dir and search can read by full path or as \
             `name/path`; changes stay in the project root:\n{}",
            others.join("\n")
        )
    }

    /// Makes folders the window's roots, with nothing open, and shows them.
    pub(super) fn show_folders(&mut self, dirs: Vec<PathBuf>) {
        let mut folders: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            let dir = dir.canonicalize().unwrap_or(dir);
            if !folders.contains(&dir) {
                self.remember_recent(&dir);
                folders.push(dir);
            }
        }
        folders.truncate(MOST);
        let Some(first) = folders.first().cloned() else { return };
        self.folders = folders;
        self.name_tree_roots();
        self.root = PathBuf::new();
        self.load(None);
        self.show_side(Some(Tab::Files));
        self.focus = Focus::Files;
        self.restore_session(&first);
    }

    /// Opens a workspace file: its folders, as one window.
    pub(super) fn open_workspace(&mut self, path: &Path) {
        match read_workspace(path) {
            Ok(folders) => {
                let path = path.canonicalize().unwrap_or(path.to_path_buf());
                self.window.names = folders.iter().filter_map(|(d, n)| Some((d.clone(), n.clone()?))).collect();
                self.window.workspace = Some(path.clone());
                self.show_folders(folders.into_iter().map(|(d, _)| d).collect());
                self.remember_recent(&path);
            }
            Err(e) => {
                self.load(None);
                self.message = Some(format!("{}: {e}", name_of(path)));
            }
        }
    }

    /// Adds a folder to the window, listed after the others.
    pub(super) fn add_folder(&mut self, dir: PathBuf) {
        let dir = dir.canonicalize().unwrap_or(dir);
        if !dir.is_dir() {
            self.message = Some(format!("{} is not a folder", dir.display()));
            return;
        }
        if self.folders.is_empty() && !self.root.as_os_str().is_empty() {
            // The project the window was showing stays, as its first folder.
            self.folders.push(self.root.clone());
        }
        if self.folders.contains(&dir) {
            self.message = Some(format!("{} is already in this window", name_of(&dir)));
            return;
        }
        if self.folders.len() >= MOST {
            self.message = Some(format!("A window holds at most {MOST} folders"));
            return;
        }
        self.remember_recent(&dir);
        self.folders.push(dir.clone());
        if self.doc.path.is_none() && self.root.as_os_str().is_empty() {
            self.root = dir.clone();
        }
        self.roots_changed();
        self.message = Some(format!("Added {} to the window", self.root_name(&dir)));
        if !self.files_open {
            self.show_side(Some(Tab::Files));
        }
    }

    /// Takes a folder out of the window. Its open tabs stay, each in its
    /// own project.
    pub(super) fn remove_folder(&mut self, dir: &Path) {
        let Some(i) = self.folders.iter().position(|f| f == dir) else { return };
        let name = self.root_name(dir);
        self.folders.remove(i);
        self.window.names.retain(|(d, _)| d != dir);
        if self.window.agent.as_deref() == Some(dir) {
            self.window.agent = None;
        }
        if self.root == dir {
            let path = self.doc.path.clone();
            self.root = match &path {
                Some(_) => self.root_for(path.as_deref()),
                None => self.folders.first().cloned().unwrap_or_else(|| dir.to_path_buf()),
            };
        }
        self.roots_changed();
        self.message = Some(format!("Removed {name} from the window"));
    }

    /// After the folders changed: the explorer, the indexes and the
    /// workspace file follow.
    fn roots_changed(&mut self) {
        self.name_tree_roots();
        self.tree.show_roots(&self.roots(), self.doc.path.as_deref());
        self.tree.refresh();
        self.index_files(true);
        self.git_stale();
        self.retitle = true;
        self.redraw = true;
        if let Some(path) = self.window.workspace.clone() {
            let text = workspace_text(&path, &self.folders, &self.window.names);
            if let Err(e) = std::fs::write(&path, text) {
                self.message = Some(format!("Cannot update {}: {e}", name_of(&path)));
            }
        }
    }

    /// Gives the explorer the names of the window's folders.
    pub(super) fn name_tree_roots(&mut self) {
        self.tree.names = self.folders.iter().map(|r| (r.clone(), self.root_name(r))).collect();
    }

    /// Writes the window's folders to a workspace file: the one it came
    /// from, or one beside its first folder.
    fn save_workspace(&mut self) {
        let folders = self.roots();
        let Some(first) = folders.first().cloned() else {
            self.message = Some("Open a folder first".into());
            return;
        };
        let path = self.window.workspace.clone().unwrap_or_else(|| {
            let dir = first.parent().unwrap_or(&first);
            dir.join(format!("{}.{WORKSPACE_EXT}", name_of(&first)))
        });
        match std::fs::write(&path, workspace_text(&path, &folders, &self.window.names)) {
            Ok(()) => {
                if self.folders.is_empty() {
                    self.folders = folders;
                }
                self.window.workspace = Some(path.clone());
                self.remember_recent(&path);
                self.message = Some(format!("Saved the folders to {}", path.display()));
            }
            Err(e) => self.message = Some(format!("Cannot save {}: {e}", path.display())),
        }
    }

    /// A list of the window's roots to choose one from, for `purpose`.
    fn roots_menu(&mut self, purpose: &str, title: &str, extra: Option<Entry>) {
        let mut entries: Vec<Entry> = self
            .roots()
            .iter()
            .enumerate()
            .map(|(i, r)| Entry::new(&format!("{purpose}\n{i}"), &self.root_name(r), &r.to_string_lossy()))
            .collect();
        entries.extend(extra);
        self.menu = Some((MenuKind::Roots, Menu::new(title, entries, "")));
        self.redraw = true;
    }

    /// The palette's commands about the window's folders.
    pub(super) fn roots_command(&mut self, id: &str) {
        match id {
            "folder_add" => {
                let dir = self.root.parent().map_or_else(|| self.root.clone(), Path::to_path_buf);
                self.chooser = Some(Chooser::new(&dir, None));
                self.window.adding = true;
                self.message = Some("Choose a folder to add to the window".into());
            }
            "folder_remove" => match self.folders.len() {
                0 | 1 => self.message = Some("The window has only one folder".into()),
                _ => self.roots_menu("remove", "Remove a folder from the window", None),
            },
            "workspace_save" => self.save_workspace(),
            "agent_root" if !self.multi_root() => {
                self.message = Some("The agent works in this window's only folder".into())
            }
            "agent_root" => {
                let follow = Entry::new("agent\nfollow", "Follow the active file", "The agent works in its folder");
                self.roots_menu("agent", "The agent's project", Some(follow));
            }
            _ => {}
        }
    }

    /// Asks which root a new terminal starts in, when the window has
    /// several and the active file is in none of them. True if asked.
    pub(super) fn ask_terminal_root(&mut self) -> bool {
        let known = self.doc.path.as_deref().is_some_and(|p| self.root_of(p).is_some());
        if !self.multi_root() || known || self.window.terminal_chosen {
            return false;
        }
        self.roots_menu("terminal", "Start the terminal in", None);
        true
    }

    /// Carries out the choice of a root.
    pub(super) fn roots_pick(&mut self, key: &str) {
        let Some((purpose, which)) = key.split_once('\n') else { return };
        if (purpose, which) == ("agent", "follow") {
            self.window.agent = None;
            self.message = Some("The agent works in the active file's folder".into());
            return;
        }
        let Some(root) = which.parse::<usize>().ok().and_then(|i| self.roots().get(i).cloned()) else { return };
        match purpose {
            "remove" => self.remove_folder(&root),
            "terminal" => {
                self.window.terminal_chosen = true;
                self.with_root(&root, |a| a.add_term());
                self.window.terminal_chosen = false;
            }
            "agent" => {
                self.message = Some(format!("The agent works in {}", self.root_name(&root)));
                self.window.agent = Some(root);
            }
            _ => {}
        }
    }

    /// A folder chosen in the file chooser while adding one.
    pub(super) fn roots_chosen(&mut self, dir: PathBuf) {
        self.window.adding = false;
        self.chooser = None;
        self.add_folder(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ld-roots-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn workspace_files_round_trip_relative_to_themselves() {
        let base = dir("ws");
        std::fs::create_dir_all(base.join("app")).unwrap();
        let lib = dir("ws-lib");
        let file = base.join("w.lightdown-workspace");
        let names = vec![(lib.clone(), "library".to_string())];
        let text = workspace_text(&file, &[base.join("app"), lib.clone()], &names);
        assert!(text.contains("\"path\": \"app\"") && text.contains("\"name\": \"library\""), "{text}");
        std::fs::write(&file, &text).unwrap();
        assert!(is_workspace(&file));
        let read = read_workspace(&file).unwrap();
        assert_eq!(read, [(base.join("app"), None), (lib.clone(), Some("library".into()))]);
        std::fs::write(&file, r#"{"folders": [{"path": "nowhere"}]}"#).unwrap();
        assert!(read_workspace(&file).unwrap_err().starts_with("nowhere"));
        std::fs::write(&file, r#"{"folders": []}"#).unwrap();
        assert!(read_workspace(&file).is_err());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&lib);
    }

    /// Two git repositories side by side, with a file of the same name in
    /// each, and a Makefile in the second.
    fn two_repos(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = dir(name);
        for (repo, file, text) in [("app", "a.md", "# A needle\n"), ("lib", "b.py", "needle = 1\n")] {
            std::fs::create_dir_all(base.join(repo)).unwrap();
            let _ = std::process::Command::new("git").args(["init", "-q"]).current_dir(base.join(repo)).status();
            std::fs::write(base.join(repo).join(file), text).unwrap();
            std::fs::write(base.join(repo).join("notes.txt"), "same name\n").unwrap();
        }
        std::fs::write(base.join("lib/Makefile"), "check:\n\ttrue\n").unwrap();
        (base.clone(), base.join("app"), base.join("lib"))
    }

    fn window(dirs: Vec<PathBuf>) -> App {
        let mut a = App::with_folders(dirs);
        a.resize(900, 600, 1.0);
        let mut px = vec![0u32; 900 * 600];
        a.draw(&mut crate::gfx::Canvas::new(&mut px, 900, 600));
        a
    }

    #[test]
    fn a_window_lists_searches_and_works_in_each_of_its_folders() {
        let (base, app, lib) = two_repos("multi");
        let mut a = window(vec![app.clone(), lib.clone()]);
        assert_eq!(a.roots(), [app.clone(), lib.clone()]);
        // Each folder heads its own files in the explorer.
        assert_eq!(a.tree.listing(), [">app/", "  a.md", "  notes.txt", "lib/", "  b.py", "  Makefile", "  notes.txt"]);
        assert_eq!(a.project_name(), "app +1");
        assert!(a.title().contains("app +1"));
        // The finder lists both, naming the folder where a path is in both.
        a.index_files(true);
        a.index_settle();
        let entries = a.file_entries();
        let shown: Vec<(String, String)> = entries.iter().map(|e| (e.label.clone(), e.detail.clone())).collect();
        assert!(
            shown.contains(&("a.md".into(), String::new())) && shown.contains(&("b.py".into(), String::new())),
            "{shown:?}"
        );
        assert!(
            shown.contains(&("notes.txt".into(), "app".into())) && shown.contains(&("notes.txt".into(), "lib".into())),
            "{shown:?}"
        );
        assert_eq!(files_in(&a.roots()).len(), 5);

        // A file of the second folder makes it the project, and the agent's
        // thread stays with the window.
        a.ai.thread.title = "kept".into();
        a.pick(lib.join("b.py"), false);
        assert_eq!((a.root.clone(), a.folders.len()), (lib.clone(), 2));
        assert_eq!((a.project_name(), a.ai.thread.title.as_str()), ("lib".to_string(), "kept"));
        // Its tasks run there; the first folder's are listed too, after its name.
        a.open_tasks();
        let Some((MenuKind::Tasks, menu)) = &a.menu else { panic!("no tasks menu") };
        assert!(menu.entries.iter().any(|e| e.label == "make check" && e.detail.starts_with("lib")));
        a.menu = None;

        // The agent reads across the folders but by full path or folder name.
        a.root = app.clone();
        assert_eq!(a.agent_read_path("lib/b.py").unwrap(), lib.join("b.py"));
        assert_eq!(a.agent_read_path(&lib.join("b.py").to_string_lossy()).unwrap(), lib.join("b.py"));
        assert_eq!(a.agent_read_path("a.md").unwrap(), app.join("a.md"));
        assert!(a.agent_read_path("/etc/passwd").is_err() && a.agent_read_path("../lib/b.py").is_err());
        assert!(a.agent_other_roots().contains("lib"));
        // A chosen folder holds the agent's project, whatever file is active.
        a.roots_pick("agent\n1");
        assert_eq!(a.agent_root(), lib);
        a.roots_pick("agent\nfollow");
        assert_eq!(a.agent_root(), app);

        // Git is asked about every repository.
        a.root = lib.clone();
        a.git_stale();
        a.git_tick();
        a.git_settle();
        assert!(a.git.status.is_some() && a.git.others.len() == 1 && a.git.others[0].0 == app);

        // A folder is taken out and put back, leaving the rest as they were.
        a.remove_folder(&app);
        assert_eq!((a.folders.clone(), a.multi_root()), (vec![lib.clone()], false));
        a.add_folder(app.clone());
        assert_eq!(a.folders, [lib.clone(), app.clone()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn folders_come_back_from_workspace_files_and_sessions() {
        let (base, app, lib) = two_repos("again");
        let mut a = window(vec![app.clone(), lib.clone()]);
        a.roots_command("workspace_save");
        let file = base.join("app.lightdown-workspace");
        assert!(file.is_file(), "{:?}", a.message);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"app\"") && text.contains("\"lib\""), "{text}");
        assert!(crate::config_schema::validate(&text, &crate::config_schema::schema(&file).unwrap()).is_empty());
        let b = App::new(Some(file.clone()));
        assert_eq!(b.folders, [app.clone(), lib.clone()]);
        // A workspace window keeps its file up to date.
        let mut b = App::new(Some(file.clone()));
        b.remove_folder(&lib);
        assert!(!std::fs::read_to_string(&file).unwrap().contains("lib"));

        // A folder opened alone brings back those it was left with.
        window(vec![app.clone(), lib.clone()]).save_session();
        let c = App::new(Some(app.clone()));
        assert_eq!(c.folders, [app.clone(), lib.clone()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn paths_belong_to_the_deepest_root() {
        let roots = [PathBuf::from("/a"), PathBuf::from("/a/b"), PathBuf::from("/c")];
        assert_eq!(holding(&roots, Path::new("/a/b/x.rs")), Some(&roots[1]));
        assert_eq!(holding(&roots, Path::new("/a/x.rs")), Some(&roots[0]));
        assert_eq!(holding(&roots, Path::new("/d/x.rs")), None);
        assert_eq!(short_in(&roots, Path::new("/c/src/m.rs")), "src/m.rs");
    }
}
