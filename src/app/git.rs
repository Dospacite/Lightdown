//! What git says of the project: the branch, and which files have changed.
//! Asked on another thread, now and then while the folder view is open.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::App;

/// How long what git said is taken as true.
const FRESH: Duration = Duration::from_secs(2);

/// A change to a file, as git counts it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Change {
    Modified,
    Added,
    /// Not yet known to git.
    Untracked,
    Renamed,
    Deleted,
    /// Both sides of a merge changed it.
    Conflict,
}

impl Change {
    /// The letter shown beside the file.
    pub fn letter(self) -> &'static str {
        match self {
            Change::Modified => "M",
            Change::Added => "A",
            Change::Untracked => "U",
            Change::Renamed => "R",
            Change::Deleted => "D",
            Change::Conflict => "!",
        }
    }
}

/// The state of a repository.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Status {
    pub branch: Option<String>,
    pub ahead_behind: String,
    pub operation: Option<String>,
    pub staged: HashSet<PathBuf>,
    pub unstaged: HashSet<PathBuf>,
    pub ignored: HashSet<PathBuf>,
    pub renamed_from: HashMap<PathBuf, PathBuf>,
    /// Each changed file, by its full path.
    pub changes: HashMap<PathBuf, Change>,
}

/// Reads `git status --porcelain -z -b` output; paths in it are relative
/// to `top`, the repository's own folder.
pub fn parse(top: &Path, out: &[u8]) -> Status {
    let mut status = Status::default();
    let mut records = out.split(|&b| b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        let rec = String::from_utf8_lossy(rec);
        if let Some(head) = rec.strip_prefix("## ") {
            status.ahead_behind = head.split_once('[').map_or(String::new(), |(_, s)| s.trim_end_matches(']').into());
            let name = head.strip_prefix("No commits yet on ").unwrap_or(head);
            let name = name.split("...").next().unwrap_or(name).split(' ').next().unwrap_or("");
            status.branch = (!name.is_empty() && name != "HEAD").then(|| name.to_string());
            if head.starts_with("HEAD (no branch)") {
                status.branch = Some("detached".into());
            }
            continue;
        }
        if rec.len() < 4 {
            continue;
        }
        let (x, y) = (rec.as_bytes()[0], rec.as_bytes()[1]);
        let path = top.join(rec[3..].trim_end_matches('/'));
        if x == b'!' {
            status.ignored.insert(path);
            continue;
        }
        if !matches!(x, b' ' | b'?') {
            status.staged.insert(path.clone());
        }
        if y != b' ' || x == b'?' {
            status.unstaged.insert(path.clone());
        }
        let change = match (x, y) {
            (b'?', _) => Change::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => Change::Conflict,
            (b'R', _) | (b'C', _) => Change::Renamed,
            (b'A', _) => Change::Added,
            (b'D', _) | (_, b'D') => Change::Deleted,
            _ => Change::Modified,
        };
        // A rename names the old path next, which is gone.
        if matches!(x, b'R' | b'C')
            && let Some(old) = records.next()
        {
            status.renamed_from.insert(path.clone(), top.join(String::from_utf8_lossy(old).as_ref()));
        }
        let path = rec[3..].trim_end_matches('/');
        status.changes.insert(top.join(path), change);
    }
    status
}

/// Asks git about the repository a folder is in. None when it is not in one.
pub fn ask(dir: &Path) -> Option<Status> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
    };
    let top = git(&["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    let out = git(&["status", "--porcelain=v1", "-z", "-b", "--untracked-files=all", "--ignored=matching"])?;
    let mut status = parse(&top, &out.stdout);
    status.operation = super::integration::state(&top).map(String::from);
    Some(status)
}

/// What git said, and of which folder; and of the window's other folders.
type Said = Arc<Mutex<Option<(PathBuf, Option<Status>, Vec<(PathBuf, Option<Status>)>)>>>;

#[derive(Default)]
pub(super) struct Git {
    asked: Option<(Instant, PathBuf)>,
    done: Said,
    busy: bool,
    pub status: Option<Status>,
    /// In a window of several folders, the state of each other one.
    pub others: Vec<(PathBuf, Option<Status>)>,
}

impl App {
    /// Asks git again if what it said is old, or was about another folder.
    pub(super) fn git_tick(&mut self) {
        let fresh = self.git.asked.as_ref().is_some_and(|(at, dir)| at.elapsed() < FRESH && *dir == self.root);
        if fresh || self.git.busy {
            return;
        }
        self.git.asked = Some((Instant::now(), self.root.clone()));
        self.git.busy = true;
        let (dir, done, waker) = (self.root.clone(), self.git.done.clone(), self.waker.clone());
        let others: Vec<PathBuf> = self.folders.iter().filter(|r| **r != self.root && self.multi_root()).cloned().collect();
        std::thread::spawn(move || {
            let status = ask(&dir);
            let others = others.into_iter().map(|d| (d.clone(), ask(&d))).collect();
            *done.lock().unwrap() = Some((dir, status, others));
            waker();
        });
    }

    /// Takes in what git said.
    pub(super) fn git_poll(&mut self) {
        let Some((dir, status, others)) = self.git.done.lock().unwrap().take() else { return };
        self.git.busy = false;
        if dir != self.root || (status == self.git.status && others == self.git.others) {
            return;
        }
        let all = || status.iter().chain(others.iter().filter_map(|(_, s)| s.as_ref()));
        self.tree.set_changes(all().flat_map(|s| s.changes.clone()).collect());
        self.tree.set_ignored(all().flat_map(|s| s.ignored.clone()).collect());
        self.git.status = status;
        self.git.others = others;
        self.redraw = true;
    }

    /// Has git asked again at the next chance, after the files changed.
    pub(super) fn git_stale(&mut self) {
        self.git.asked = None;
    }

    /// Waits for git's answer. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn git_settle(&mut self) {
        let start = Instant::now();
        while self.git.busy && start.elapsed().as_secs() < 10 {
            std::thread::sleep(Duration::from_millis(5));
            self.git_poll();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_is_read() {
        let top = Path::new("/repo");
        let out = b"## main...origin/main [ahead 1]\0 M src/a.rs\0?? notes/new.md\0A  b.txt\0R  c2.txt\0c.txt\0UU d.txt\0 D gone.txt\0";
        let s = parse(top, out);
        assert_eq!(s.branch.as_deref(), Some("main"));
        let at = |p: &str| s.changes.get(&top.join(p)).copied();
        assert_eq!(at("src/a.rs"), Some(Change::Modified));
        assert_eq!(at("notes/new.md"), Some(Change::Untracked));
        assert_eq!(at("b.txt"), Some(Change::Added));
        assert_eq!(at("c2.txt"), Some(Change::Renamed));
        assert_eq!(at("c.txt"), None);
        assert_eq!(at("d.txt"), Some(Change::Conflict));
        assert_eq!(at("gone.txt"), Some(Change::Deleted));
        assert_eq!(parse(top, b"## No commits yet on master\0").branch.as_deref(), Some("master"));
    }

    #[test]
    fn a_repository_is_asked_about() {
        let dir = std::env::temp_dir().join(format!("ld-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let ok = Command::new("git")
            .args(["init", "-q", "-b", "trunk"])
            .current_dir(&dir)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        std::fs::write(dir.join("sub/new.md"), "x").unwrap();
        let s = ask(&dir.join("sub")).unwrap();
        let top = dir.canonicalize().unwrap();
        assert_eq!(s.branch.as_deref(), Some("trunk"));
        assert_eq!(s.changes.get(&top.join("sub/new.md")), Some(&Change::Untracked));
        assert!(ask(&std::env::temp_dir()).is_none_or(|s| s.branch.is_some()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
