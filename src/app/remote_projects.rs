//! SSH mode is enabled only by an explicit connect command in this window.
use super::*;
use crate::remote::{self, Remote};
#[derive(Default)]
pub(super) struct RemoteProjects {
    pending: Option<(PathBuf, Operation, tools::Job)>,
    /// Saved files waiting to be pushed, the project, and when to push.
    queued: std::collections::BTreeSet<PathBuf>,
    queued_root: PathBuf,
    push_due: Option<std::time::Instant>,
    pushing: Option<(PathBuf, usize, tools::Job)>,
    pub(super) ports: super::port_list::PortList,
    pub(super) containers: super::containers::Containers,
}
enum Operation { Connect(Remote), Sync(bool) }
use crate::tools;
impl App {
    pub(super) fn remote_action(&mut self, action: &str) {
        match action {
            "forward" => return self.forward_menu(),
            "push_on_save" => return self.toggle_push_on_save(),
            _ => {}
        }
        if action == "stop" {
            if let Some((root, op, _)) = self.remote_projects.pending.take() {
                if root == self.root && matches!(op, Operation::Sync(true)) { self.disk_poll(true); self.refresh_agent_changes(); self.finish_agent_turn(); }
            }
            self.message = Some("Remote operation stopped".into()); return;
        }
        if self.ai.busy() || self.remote_projects.pending.is_some() { self.message = Some("Finish the current agent or remote operation first".into()); return; }
        if action == "disconnect" {
            if remote::active(&self.root).is_some_and(|r| r.container.is_some()) { return self.container_action("stop"); }
            remote::activate(&self.root, None); self.languages = Default::default();
            let root = self.root.clone();
            self.stop_forwards(&root);
            self.remote_projects.queued.clear();
            self.message = Some("Remote execution disconnected. Existing terminals keep their connection".into()); return;
        }
        let result = (|| {
            if action == "connect" {
                if remote::active(&self.root).is_some_and(|r| r.container.is_some()) { return Err("Stop the container first".into()); }
                let r = Remote::read(&self.root)?;
                let cmd = r.command("pwd", false);
                self.remote_projects.pending = Some((self.root.clone(), Operation::Connect(r), tools::Job::start(cmd, self.waker.clone())));
            } else {
                if self.doc.buf.modified || self.file_tabs.iter().filter_map(|t| t.state.as_ref()).any(|s| s.pane.buf.modified) { return Err("Save local editor changes before synchronizing".into()); }
                let r = remote::active(&self.root).ok_or("Connect the remote project first")?;
                if r.container.is_some() { return Err("A container shares the project folder; nothing to sync".into()); }
                let pull = action == "pull";
                if pull { self.begin_agent_turn(); self.checkpoint_command()?; }
                self.remote_projects.pending = Some((self.root.clone(), Operation::Sync(pull), tools::Job::start(r.sync(&self.root, pull), self.waker.clone())));
            }
            self.message = Some("Remote operation running…".into());
            Ok::<_, String>(())
        })();
        if let Err(e) = result { self.message = Some(e); }
    }
    pub(super) fn remote_poll(&mut self) {
        self.ports_poll();
        self.containers_poll();
        self.push_poll();
        let Some((_, _, job)) = &self.remote_projects.pending else { return };
        let Some((out, ok)) = job.poll() else { return };
        let (root, op, _) = self.remote_projects.pending.take().unwrap();
        if root != self.root { return; }
        if let Operation::Sync(true) = op { self.disk_poll(true); self.refresh_agent_changes(); self.finish_agent_turn(); self.tree.refresh(); }
        if !ok { self.message = Some(format!("Remote operation failed: {}", tools::clip_line(&out, 240))); return; }
        match op {
            Operation::Connect(r) => {
                let push = if r.push_on_save { "; saves are pushed" } else { "" };
                self.message = Some(format!("Remote execution: {}:{}{push}", r.host, r.path.display()));
                remote::activate(&self.root, Some(r)); self.languages = Default::default();
            }
            Operation::Sync(pull) => self.message = Some(if pull { "Pulled remote files. Review changes with Agent: Review changed files" } else { "Pushed saved local files to the remote project" }.into()),
        }
    }
}
impl App {
    fn toggle_push_on_save(&mut self) {
        let Some(mut r) = remote::active(&self.root).filter(|r| r.container.is_none()) else {
            self.message = Some("Connect the SSH project first".into());
            return;
        };
        r.push_on_save = !r.push_on_save;
        self.message = Some(if r.push_on_save { "Saved files are pushed to the remote" } else { "Push on save off" }.into());
        remote::activate(&self.root, Some(r));
    }
    /// Queues a saved file for pushing, when the SSH project asks for that.
    pub(super) fn remote_saved(&mut self, path: &Path) {
        let Some(r) = remote::active(&self.root).filter(|r| r.container.is_none() && r.push_on_save) else { return };
        let Ok(rel) = path.strip_prefix(&self.root) else { return };
        if rel.starts_with(".git") || rel == Path::new(".lightdown/remote.json") || r.host.is_empty() {
            return;
        }
        let state = &mut self.remote_projects;
        if state.queued_root != self.root {
            state.queued.clear();
            state.queued_root = self.root.clone();
        }
        if state.queued.len() < 256 {
            state.queued.insert(rel.to_path_buf());
        }
        state.push_due = Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
    }
    /// Pushes queued files once saves settle, one rsync at a time.
    pub(super) fn remote_tick(&mut self) {
        self.forwards_tick();
        self.containers_tick();
        let state = &mut self.remote_projects;
        if state.pushing.is_some() || state.queued.is_empty() || !state.push_due.is_some_and(|d| d <= std::time::Instant::now()) {
            return;
        }
        let root = state.queued_root.clone();
        let Some(r) = remote::active(&root).filter(|r| r.container.is_none() && r.push_on_save) else {
            state.queued.clear();
            return;
        };
        let files: Vec<PathBuf> = std::mem::take(&mut state.queued).into_iter().filter(|f| root.join(f).is_file()).collect();
        state.push_due = None;
        if files.is_empty() {
            return;
        }
        let job = tools::Job::start(r.push_files(&root, &files), self.waker.clone());
        self.remote_projects.pushing = Some((root, files.len(), job));
    }
    fn push_poll(&mut self) {
        let Some((_, _, job)) = &self.remote_projects.pushing else { return };
        let Some((out, ok)) = job.poll() else { return };
        let (_, count, _) = self.remote_projects.pushing.take().unwrap();
        self.message = Some(if ok {
            format!("Pushed {count} saved file{} to the remote", if count == 1 { "" } else { "s" })
        } else {
            format!("Push on save failed: {}", tools::clip_line(&out, 240))
        });
        self.redraw = true;
    }
}
#[cfg(test)]
mod push_tests {
    use super::*;
    #[test]
    fn saves_queue_for_pushing_only_when_enabled() {
        let root = std::env::temp_dir().join(format!("ld-push-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let mut a = App::new(Some(root.join("a.txt")));
        let mut r = Remote::parse(&serde_json::json!({"host":"example.invalid","path":"/srv/x"})).unwrap();
        remote::activate(&root, Some(r.clone()));
        a.remote_saved(&root.join("a.txt"));
        assert!(a.remote_projects.queued.is_empty());
        a.remote_action("push_on_save");
        assert!(remote::active(&root).unwrap().push_on_save);
        a.remote_saved(&root.join("a.txt"));
        a.remote_saved(&root.join(".git/config"));
        a.remote_saved(&root.join("a.txt"));
        assert_eq!(a.remote_projects.queued.len(), 1);
        assert!(a.remote_projects.push_due.is_some());
        r.push_on_save = false;
        remote::activate(&root, Some(r));
        a.remote_projects.push_due = Some(std::time::Instant::now());
        a.remote_tick();
        assert!(a.remote_projects.pushing.is_none() && a.remote_projects.queued.is_empty());
        remote::activate(&root, None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn settle(a: &mut App) {
        let start = std::time::Instant::now();
        while a.remote_projects.pending.is_some() && start.elapsed().as_secs() < 30 { a.remote_poll(); std::thread::sleep(std::time::Duration::from_millis(10)); }
        assert!(a.remote_projects.pending.is_none(), "remote operation did not finish");
    }
    /// Set LIGHTDOWN_SSH_TEST_HOST to an ssh host alias with key login and a
    /// trusted host key. The remote folder is made under /tmp on that host.
    #[test]
    #[ignore = "requires LIGHTDOWN_SSH_TEST_HOST with key-based login"]
    fn connects_pushes_runs_pulls_and_opens_a_remote_shell() {
        let host = std::env::var("LIGHTDOWN_SSH_TEST_HOST").unwrap();
        let id = std::process::id();
        let root = std::env::temp_dir().join(format!("ld-remote-mirror-{id}"));
        let remote = format!("/tmp/ld-remote-target-{id}");
        std::fs::create_dir_all(root.join(".git")).unwrap(); std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join(".lightdown/remote.json"), serde_json::json!({"host":host,"path":remote}).to_string()).unwrap();
        let file = root.join("hello.txt"); std::fs::write(&file, "from the mirror\n").unwrap();
        let mut setup = Remote::parse(&serde_json::json!({"host":host,"path":"/tmp"})).unwrap().command(&format!("mkdir -p {}", remote::quote(&remote)), false);
        assert!(setup.status().unwrap().success());
        let mut a = App::new(Some(file.clone())); a.resize(1200, 800, 1.0);
        assert_eq!(a.root, root);
        a.remote_action("connect"); settle(&mut a);
        assert!(remote::active(&root).is_some(), "{:?}", a.message);
        a.remote_action("push"); settle(&mut a);
        let out = a.project_command("pwd; cat hello.txt; echo 'made remotely' > made.txt", 30).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{remote}\nfrom the mirror\n"));
        a.remote_action("pull"); settle(&mut a);
        assert_eq!(std::fs::read_to_string(root.join("made.txt")).unwrap(), "made remotely\n", "{:?}", a.message);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        let pty = crate::pty::Pty::spawn("sh", &root, &root, (80, 24), move |b| { sink.lock().unwrap().extend_from_slice(b); Vec::new() }, || {}).unwrap();
        pty.write(b"pwd; exit\n");
        let start = std::time::Instant::now();
        while !String::from_utf8_lossy(&seen.lock().unwrap()).contains(&remote) && start.elapsed().as_secs() < 15 { std::thread::sleep(std::time::Duration::from_millis(20)); }
        assert!(String::from_utf8_lossy(&seen.lock().unwrap()).contains(&remote), "{}", String::from_utf8_lossy(&seen.lock().unwrap()));
        a.remote_action("disconnect");
        assert!(remote::active(&root).is_none());
        let _ = Remote::parse(&serde_json::json!({"host":host,"path":"/tmp"})).unwrap().command(&format!("rm -rf {}", remote::quote(&remote)), false).status();
        std::fs::remove_dir_all(root).unwrap();
    }
}
