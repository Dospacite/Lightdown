//! Explicit worktree creation, review and fast-forward merge. Each window owns its agent.
use super::*;
use std::sync::mpsc::{self, Receiver};
use super::repository as repo;
#[derive(Default)]
pub(super) struct Worktrees {
    pending: Option<(PathBuf, Receiver<Result<Done, String>>)>,
    entries: Vec<(PathBuf, String)>,
    status: Option<(PathBuf, String)>,
}
enum Done { Overview(Vec<(PathBuf, String)>, Vec<String>), Created(PathBuf), Listed(Vec<(PathBuf, String)>), Diff(String), Merged }
fn storage(root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new(); root.hash(&mut hash);
    crate::config::data_dir().join("worktrees").join(format!("{:016x}", hash.finish()))
}
fn create(root: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.len() > 60 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { return Err("Use 1 to 60 letters, digits, hyphens or underscores".into()); }
    let repository = repo::Repo::open(root)?;
    let parent = storage(&repository.top);
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let path = parent.join(name);
    repo::run(&repository.top, ["worktree", "add", "-b", &format!("lightdown/{name}"), path.to_str().ok_or("Non-UTF-8 worktree path")?, "HEAD"])?;
    let base = repo::run(&repository.top, ["rev-parse", "HEAD"])?;
    repo::run(&path, ["config", &format!("branch.lightdown/{name}.lightdownBase"), base.trim()])?;
    Ok(path)
}
fn list(root: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let repository = repo::Repo::open(root)?;
    let output = repo::run(root, ["worktree", "list", "--porcelain"])?;
    let parent = storage(&repository.top);
    let mut entries = Vec::new();
    for block in output.split("\n\n") {
        let path = block.lines().find_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from);
        let branch = block.lines().find_map(|l| l.strip_prefix("branch refs/heads/"));
        if let (Some(p), Some(b)) = (path, branch) {
            if p.starts_with(&parent) && b.starts_with("lightdown/") { entries.push((p, b.to_string())); }
        }
    }
    Ok(entries)
}
fn overview(root: &Path) -> Result<Done, String> {
    let entries = list(root)?;
    let current = repo::run(root, ["rev-parse", "HEAD"])?;
    let details = entries.iter().map(|(path, branch)| {
        let base = repo::run(path, ["config", "--get", &format!("branch.{branch}.lightdownBase")]).unwrap_or_else(|_| repo::run(path, ["merge-base", current.trim(), "HEAD"]).unwrap_or_else(|_| "HEAD".into()));
        let ahead = repo::run(path, ["rev-list", "--count", &format!("{}..HEAD", base.trim())])?;
        let changed = repo::run(path, ["diff", "--name-only", &format!("{}...HEAD", base.trim()), "--"])?;
        let dirty = repo::run(path, ["status", "--porcelain"])?;
        Ok(format!("{} ahead · {} files changed · {} uncommitted · {}", ahead.trim(), changed.lines().count(), dirty.lines().count(), super::worktree_overview::windows(path)))
    }).collect::<Result<Vec<_>, String>>()?;
    Ok(Done::Overview(entries, details))
}
impl App {
    fn worktree_job(&mut self, task: impl FnOnce(PathBuf) -> Result<Done, String> + Send + 'static) {
        if self.worktrees.pending.is_some() || self.ai.busy() { self.message = Some("Finish the current agent or worktree operation first".into()); return; }
        let root = self.root.clone(); let dir = root.clone(); let wake = self.waker.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || { let _ = tx.send(task(dir)); wake(); });
        self.worktrees.pending = Some((root, rx));
        self.message = Some("Working on agent worktrees…".into());
    }
    pub(super) fn new_agent_worktree(&mut self, name: &str) {
        let name = name.to_string();
        self.worktree_job(move |root| create(&root, &name).map(Done::Created));
    }
    pub(super) fn list_agent_worktrees(&mut self) { self.worktree_job(|root| list(&root).map(Done::Listed)); }
    pub(super) fn worktree_overview(&mut self) {
        self.worktree_job(|root| overview(&root));
    }
    pub(super) fn worktrees_poll(&mut self) {
        let value = serde_json::json!({"path":self.root,"pid":std::process::id(),"busy":self.ai.busy(),"cost":self.ai.thread.cost}).to_string();
        if self.worktrees.status.as_ref().is_none_or(|(_, old)| old != &value) {
            let path = super::worktree_overview::publish(&value);
            self.worktrees.status = path.map(|p| (p, value));
        }
        let Some((root, rx)) = &self.worktrees.pending else { return };
        let Ok(done) = rx.try_recv() else { return };
        let same = *root == self.root; self.worktrees.pending = None;
        if !same { return; }
        match done {
            Err(e) => self.message = Some(e),
            Ok(Done::Created(path)) => { spawn_self(Some(&path)); self.message = Some(format!("Created {}. Its agent runs in a separate window", path.display())); }
            Ok(Done::Overview(entries, details)) => {
                let rows = entries.iter().zip(details).enumerate().map(|(i, ((_, branch), detail))| Entry::new(&format!("pick:{i}"), branch, &detail)).collect();
                self.worktrees.entries = entries;
                self.menu = Some((MenuKind::AgentWorktrees, Menu::new("Agent worktree overview", rows, "")));
            }
            Ok(Done::Listed(entries)) => {
                let rows = entries.iter().enumerate().map(|(i, (path, branch))| Entry::new(&format!("pick:{i}"), branch, &path.to_string_lossy())).collect();
                self.worktrees.entries = entries;
                self.menu = Some((MenuKind::AgentWorktrees, Menu::new("Agent worktrees", rows, "")));
            }
            Ok(Done::Diff(text)) => { self.open_output(text); self.doc.diff = true; self.doc.output_name = Some("Worktree review".into()); }
            Ok(Done::Merged) => { self.disk_poll(true); self.tree.refresh(); self.message = Some("Worktree branch merged by fast-forward".into()); }
        }
    }
    pub(super) fn pick_agent_worktree(&mut self, key: &str) {
        let Some((action, number)) = key.split_once(':') else { return };
        let Some((path, branch)) = number.parse::<usize>().ok().and_then(|i| self.worktrees.entries.get(i)).cloned() else { return };
        if action == "pick" {
            self.menu = Some((MenuKind::AgentWorktrees, Menu::new(&branch, vec![Entry::new(&format!("open:{number}"), "Open worktree window", "Independent agent, permissions and terminal"), Entry::new(&format!("review:{number}"), "Review committed and uncommitted changes", "Shows differences in the editor"), Entry::new(&format!("merge:{number}"), "Merge committed branch", "Requires saved files and a clean worktree; fast-forward only")], "")));
        } else if action == "open" { spawn_self(Some(&path)); }
        else if action == "review" {
            self.worktree_job(move |root| {
                let repository = repo::Repo::open(&root)?;
                let mut text = repo::run(&repository.top, ["diff", "--no-ext-diff", "--no-textconv", &format!("HEAD...{branch}"), "--"])?;
                text += "\nUncommitted worktree changes:\n";
                text += &repo::run(&path, ["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--"])?;
                text += "\nWorktree status, including untracked files:\n";
                text += &repo::run(&path, ["status", "--short"])?;
                Ok(Done::Diff(text))
            });
        } else if action == "merge" {
            if self.doc.buf.modified || self.file_tabs.iter().filter_map(|t| t.state.as_ref()).any(|s| s.pane.buf.modified) { self.message = Some("Save editor changes before merging".into()); return; }
            self.worktree_job(move |root| {
                if !repo::run(&root, ["status", "--porcelain"])?.is_empty() || !repo::run(&path, ["status", "--porcelain"])?.is_empty() { return Err("Both projects must be clean. Commit the agent worktree before merging".into()); }
                repo::run(&root, ["merge", "--ff-only", &branch])?;
                Ok(Done::Merged)
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_worktree_branch_can_be_reviewed_and_fast_forwarded() {
        let root = std::env::temp_dir().join(format!("ld-worktree-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        repo::run(&root, ["init", "-q"]).unwrap();
        repo::run(&root, ["config", "user.email", "test@example.invalid"]).unwrap();
        repo::run(&root, ["config", "user.name", "Test"]).unwrap();
        std::fs::write(root.join("file"), "before").unwrap();
        repo::run(&root, ["add", "file"]).unwrap(); repo::run(&root, ["commit", "-qm", "initial"]).unwrap();
        assert!(create(&root, "../bad").is_err());
        let path = create(&root, "fixture").unwrap();
        std::fs::write(path.join("file"), "after").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "before");
        repo::run(&path, ["commit", "-am", "agent change"]).unwrap();
        std::fs::write(path.join("new"), "untracked").unwrap();
        assert_eq!(list(&root).unwrap().len(), 1);
        // The overview counts from the recorded base: one commit, one file,
        // one untracked file, and no window has it open.
        let Done::Overview(_, details) = overview(&root).unwrap() else { panic!() };
        assert!(details[0].starts_with("1 ahead · 1 files changed · 1 uncommitted · no window"), "{}", details[0]);
        std::fs::remove_file(path.join("new")).unwrap();
        repo::run(&root, ["merge", "--ff-only", "lightdown/fixture"]).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "after");
        repo::run(&root, ["worktree", "remove", path.to_str().unwrap()]).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl Drop for Worktrees {
    fn drop(&mut self) { if let Some((path, _)) = &self.status { let _ = std::fs::remove_file(path); } }
}
