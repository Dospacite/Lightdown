//! Discarding unstaged changes. What is discarded is read when the user is
//! asked, and the discard is refused if the file or index changed since.
use super::git_actions::{menu, Action, Answer, Choice};
use super::repository::{self as repo, Repo};
use super::*;

#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    path: PathBuf,
    /// The file's bytes, a link's target, or None when it is gone.
    bytes: Option<Vec<u8>>,
    /// The file's entries in the index; empty for an untracked file.
    index: String,
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(m) if m.is_dir() => Err(format!("{} is a folder", path.display())),
        Ok(m) if m.is_symlink() => {
            std::fs::read_link(path).map(|t| Some(t.into_os_string().into_encoded_bytes())).map_err(|e| e.to_string())
        }
        Ok(_) => std::fs::read(path).map(Some).map_err(|e| e.to_string()),
    }
}

fn index(r: &Repo, path: &Path) -> Result<String, String> {
    repo::run(&r.top, ["ls-files", "--stage", "--", r.relative(path)?])
}

pub(super) fn capture(r: &Repo, paths: Vec<PathBuf>) -> Result<Vec<Snapshot>, String> {
    paths
        .into_iter()
        .map(|path| {
            let index = index(r, &path)?;
            // Entries past stage 0 are a conflict's sides.
            if index.lines().any(|l| l.split_whitespace().nth(2).is_some_and(|stage| stage != "0")) {
                return Err(format!("Resolve the conflict in {} first", r.relative(&path)?));
            }
            Ok(Snapshot { bytes: read(&path)?, path, index })
        })
        .collect()
}

/// The question asked before discarding.
pub(super) fn confirm(r: &Repo, snapshots: Vec<Snapshot>) -> Answer {
    let untracked = snapshots.iter().filter(|s| s.index.is_empty()).count();
    let tracked = snapshots.len() - untracked;
    let name = |s: &Snapshot| r.relative(&s.path).unwrap_or("file").to_string();
    let title = match (snapshots.len(), untracked) {
        (1, 1) => format!("Delete untracked file {}? This cannot be undone", name(&snapshots[0])),
        (1, _) => format!("Discard unstaged changes to {}? This cannot be undone", name(&snapshots[0])),
        (n, 0) => format!("Discard unstaged changes to {n} files? This cannot be undone"),
        (n, u) if u == n => format!("Delete {n} untracked files? This cannot be undone"),
        (_, u) => format!("Discard changes to {tracked} files and delete {u} untracked files? This cannot be undone"),
    };
    let names: Vec<_> = snapshots.iter().take(6).map(name).collect();
    let more = snapshots.len().saturating_sub(names.len());
    let detail = format!("{}{}", names.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() });
    let label = if tracked == 0 { "Delete files" } else { "Discard changes" };
    menu(&title, vec![Choice::new(label, detail, Action::Discard(snapshots))])
}

pub(super) fn apply(r: &Repo, snapshots: Vec<Snapshot>) -> Result<(), String> {
    for s in &snapshots {
        if read(&s.path)? != s.bytes || index(r, &s.path)? != s.index {
            return Err("File or index changed. Request the discard again.".into());
        }
    }
    let (untracked, tracked): (Vec<_>, Vec<_>) = snapshots.into_iter().partition(|s| s.index.is_empty());
    for s in untracked {
        if s.bytes.is_some() {
            std::fs::remove_file(&s.path).map_err(|e| e.to_string())?;
        }
    }
    if !tracked.is_empty() {
        let mut args = vec!["restore".to_string(), "--worktree".into(), "--".into()];
        for s in &tracked {
            args.push(r.relative(&s.path)?.to_string());
        }
        repo::run(&r.top, args)?;
    }
    Ok(())
}
