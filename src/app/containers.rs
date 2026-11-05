//! Dev container execution, started only by an explicit command. Steps run
//! on a thread and write to a bounded log; the container is stopped when it
//! is disconnected and when the window closes.
use super::*;
use crate::container::{self, Config, Target};
use std::collections::BTreeSet;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Connected {
    root: PathBuf,
    target: Target,
    workspace: PathBuf,
    forward: Vec<u16>,
    host_network: bool,
}

type Steps = (PathBuf, Receiver<Result<(Connected, Option<String>), String>>, Arc<AtomicBool>);

#[derive(Default)]
pub(super) struct Containers {
    log: Arc<Mutex<String>>,
    pending: Option<Steps>,
    connected: Option<Connected>,
    /// Tags of container commands issued here, and when.
    issued: Vec<(String, Instant)>,
    checked: Option<Instant>,
    sweeping: Option<Child>,
}

impl Drop for Containers {
    fn drop(&mut self) {
        if let Some(c) = &self.connected {
            crate::remote::activate(&c.root, None);
            let _ = container::stop(&c.target.engine, &c.target.name).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        }
        if let Some(mut child) = self.sweeping.take() {
            let _ = child.wait();
        }
    }
}

fn append(log: &Mutex<String>, text: &str) {
    let mut log = log.lock().unwrap();
    log.push_str(text);
    if log.len() > 256_000 {
        let mut at = log.len() - 256_000;
        while !log.is_char_boundary(at) {
            at += 1;
        }
        log.drain(..at);
    }
}

fn shown(cmd: &Command) -> String {
    std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
}

/// Runs one step, its output going to the log, within `secs`.
fn step(log: &Mutex<String>, cancel: &AtomicBool, cmd: Command, secs: u64) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("Stopped".into());
    }
    append(log, &format!("$ {}\n", shown(&cmd)));
    let mut timed = Command::new("timeout");
    timed.args(["-k", "5", &secs.to_string()]).arg(cmd.get_program()).args(cmd.get_args());
    if let Some(dir) = cmd.get_current_dir() {
        timed.current_dir(dir);
    }
    let (mut reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    let err = writer.try_clone().map_err(|e| e.to_string())?;
    let mut child = timed.stdin(Stdio::null()).stdout(writer).stderr(err).process_group(0).spawn().map_err(|e| format!("{}: {e}", shown(&cmd)))?;
    drop(timed);
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => append(log, &String::from_utf8_lossy(&buf[..n])),
        }
        if cancel.load(Ordering::SeqCst) {
            // SAFETY: the process group this step started.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if cancel.load(Ordering::SeqCst) {
        return Err("Stopped".into());
    }
    match status.code() {
        Some(0) => Ok(()),
        Some(124) => Err(format!("Timed out after {secs} s: {}", shown(&cmd))),
        _ => Err(format!("Failed: {}", shown(&cmd))),
    }
}

/// Builds or pulls the image and starts or reuses the container. Returns
/// the connection and any warning.
fn up(engine: &str, root: &Path, cfg: &Config, rebuild: bool, log: &Mutex<String>, cancel: &AtomicBool) -> Result<(Connected, Option<String>), String> {
    let name = container::name(root);
    append(log, &format!("Container {name} for {} from {}\n", root.display(), cfg.source.display()));
    for key in &cfg.ignored {
        append(log, &format!("Not supported, ignored: {key}\n"));
    }
    if rebuild {
        let _ = step(log, cancel, container::remove(engine, &name), 60);
    }
    let found = container::inspect(engine, &name).stdin(Stdio::null()).output().map_err(|e| format!("{engine}: {e}"))?;
    let state = String::from_utf8_lossy(&found.stdout).trim().to_string();
    let fingerprint = cfg.fingerprint(root);
    let mut created = false;
    match state.split_once(' ') {
        Some((running, label)) if found.status.success() && label == fingerprint => {
            append(log, "Reusing the existing container\n");
            if running != "true" {
                step(log, cancel, container::start(engine, &name), 120)?;
            }
        }
        _ => {
            if found.status.success() {
                append(log, "The configuration changed; recreating the container\n");
                step(log, cancel, container::remove(engine, &name), 60)?;
            }
            let image = if cfg.build.is_some() {
                step(log, cancel, container::build(engine, cfg, root), 3600)?;
                cfg.tag(root)
            } else {
                let image = cfg.image.clone().unwrap_or_default();
                let have = container::image_exists(engine, &image).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
                if !have {
                    step(log, cancel, container::pull(engine, &image), 1800)?;
                }
                image
            };
            step(log, cancel, container::run(engine, cfg, root, &image), 120)?;
            created = true;
        }
    }
    let target = Target { engine: engine.into(), name, user: cfg.user.clone() };
    let mut warning = None;
    if created && let Some(argv) = &cfg.post_create {
        let command = argv.iter().map(|a| crate::remote::quote(a)).collect::<Vec<_>>().join(" ");
        if let Err(e) = step(log, cancel, target.exec(&cfg.workspace, &command, false), 3600) {
            append(log, &format!("postCreateCommand: {e}\n"));
            warning = Some("postCreateCommand failed; Container: Show log has its output".to_string());
        }
    }
    append(log, "Connected\n");
    let connected = Connected { root: root.into(), target, workspace: cfg.workspace.clone(), forward: cfg.forward.clone(), host_network: cfg.host_network };
    Ok((connected, warning))
}

impl App {
    /// The palette filter the status bar's execution item opens.
    pub(super) fn execution_commands(&self) -> &'static str {
        if crate::remote::active(&self.root).is_some_and(|r| r.container.is_some()) { ">Container:" } else { ">Remote:" }
    }
    /// The container's forwarded ports, and whether it uses host networking.
    pub(super) fn container_ports(&self) -> Option<(Vec<u16>, bool)> {
        let c = self.remote_projects.containers.connected.as_ref().filter(|c| c.root == self.root)?;
        Some((c.forward.clone(), c.host_network))
    }
    pub(super) fn container_action(&mut self, action: &str) {
        let state = &mut self.remote_projects.containers;
        match action {
            "log" => {
                let text = state.log.lock().unwrap().clone();
                if text.is_empty() {
                    self.message = Some("No container log yet".into());
                } else {
                    self.open_output(text);
                }
            }
            "stop" => {
                if let Some((_, _, cancel)) = &state.pending {
                    cancel.store(true, Ordering::SeqCst);
                    self.message = Some("Stopping the container start…".into());
                    return;
                }
                let Some(c) = state.connected.take_if(|c| c.root == self.root) else {
                    self.message = Some("No container is connected for this project".into());
                    return;
                };
                crate::remote::activate(&c.root, None);
                self.languages = Default::default();
                let log = state.log.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                self.message = Some(format!("Stopping container {}; new commands run on this machine", c.target.name));
                std::thread::spawn(move || {
                    let _ = step(&log, &cancel, container::stop(&c.target.engine, &c.target.name), 60);
                });
            }
            "start" | "rebuild" => {
                if state.pending.is_some() {
                    self.message = Some("The container is already starting".into());
                    return;
                }
                if crate::remote::active(&self.root).is_some_and(|r| r.container.is_none()) {
                    self.message = Some("Disconnect the SSH project first".into());
                    return;
                }
                let Some(engine) = container::engine() else {
                    self.message = Some("Install docker or podman to use containers".into());
                    return;
                };
                let cfg = match container::read(&self.root) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        self.message = Some(format!("Container: {e}"));
                        return;
                    }
                };
                if action == "rebuild"
                    && let Some(c) = state.connected.take_if(|c| c.root == self.root)
                {
                    crate::remote::activate(&c.root, None);
                    self.languages = Default::default();
                }
                let state = &mut self.remote_projects.containers;
                *state.log.lock().unwrap() = String::new();
                let (tx, rx) = channel();
                let cancel = Arc::new(AtomicBool::new(false));
                let (log, stop, wake, root, rebuild) = (state.log.clone(), cancel.clone(), self.waker.clone(), self.root.clone(), action == "rebuild");
                std::thread::spawn(move || {
                    let result = up(&engine, &root, &cfg, rebuild, &log, &stop);
                    if let Err(e) = &result {
                        append(&log, &format!("{e}\n"));
                    }
                    let _ = tx.send(result);
                    wake();
                });
                state.pending = Some((self.root.clone(), rx, cancel));
                self.message = Some("Starting the container… Container: Show log follows it".into());
            }
            _ => {}
        }
    }
    pub(super) fn containers_poll(&mut self) {
        let state = &mut self.remote_projects.containers;
        let Some((_, rx, _)) = &state.pending else { return };
        let Ok(result) = rx.try_recv() else { return };
        let (root, _, _) = state.pending.take().unwrap();
        match result {
            Ok((c, warning)) => {
                // A new connection replaces one for another project in this window.
                if let Some(old) = state.connected.take() {
                    crate::remote::activate(&old.root, None);
                }
                crate::remote::activate(&root, Some(crate::remote::Remote::container(c.target.clone(), c.workspace.clone())));
                self.message = Some(warning.unwrap_or_else(|| format!("Container {} connected; new terminals, tasks and language servers run inside it", c.target.name)));
                state.connected = Some(c);
                self.languages = Default::default();
            }
            Err(e) => self.message = Some(format!("Container: {}. Container: Show log has details", crate::tools::clip_line(&e, 200))),
        }
        self.redraw = true;
    }
    /// Ends container processes whose clients here have gone, every two seconds.
    pub(super) fn containers_tick(&mut self) {
        let state = &mut self.remote_projects.containers;
        if let Some(child) = &mut state.sweeping {
            if !matches!(child.try_wait(), Ok(None)) {
                state.sweeping = None;
            }
            return;
        }
        let Some(c) = &state.connected else { return };
        if state.checked.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        state.checked = Some(Instant::now());
        state.issued.extend(container::take_issued().into_iter().map(|t| (t, Instant::now())));
        let live = container::live_tags();
        let mut gone = BTreeSet::new();
        // A tag is only gone once its client has had time to start.
        state.issued.retain(|(tag, at)| {
            let dead = at.elapsed() > Duration::from_secs(3) && !live.contains(tag);
            if dead {
                gone.insert(tag.clone());
            }
            !dead
        });
        if gone.is_empty() {
            return;
        }
        state.sweeping = c.target.sweep(&gone).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settle(a: &mut App, until: impl Fn(&App) -> bool, secs: u64) {
        let start = Instant::now();
        while !until(a) && start.elapsed().as_secs() < secs {
            std::thread::sleep(Duration::from_millis(20));
            a.remote_poll();
            a.term_poll();
            a.tasks_poll();
            a.remote_tick();
        }
        assert!(until(a), "{:?}\n{}", a.message, a.remote_projects.containers.log.lock().unwrap());
    }
    #[test]
    fn without_a_config_nothing_starts() {
        let root = std::env::temp_dir().join(format!("ld-container-none-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let mut a = App::new(Some(root.join("a.txt")));
        a.container_action("start");
        assert!(a.remote_projects.containers.pending.is_none());
        assert!(crate::remote::active(&root).is_none());
        a.container_action("stop");
        assert!(a.message.as_deref().unwrap().contains("No container"));
        std::fs::remove_dir_all(root).unwrap();
    }
    /// Needs docker or podman and the `alpine` image (pulled if missing).
    #[test]
    #[ignore = "requires docker or podman"]
    fn starts_runs_commands_inside_and_stops() {
        let root = std::env::temp_dir().join(format!("ld-container-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join(".lightdown/container.json"), r#"{
            // Comments are allowed.
            "image": "alpine", "workspaceFolder": "/work", "containerEnv": {"GREETING": "hi there"},
            "forwardPorts": [8123], "runArgs": ["--network=host"],
            "postCreateCommand": "touch /work/post-created",
        }"#).unwrap();
        std::fs::write(root.join(".lightdown/tasks.json"), r#"{"tasks":[{"name":"where","shell":"echo \"$GREETING in $(pwd)\"; cat /etc/alpine-release"},{"name":"long","shell":"sleep 300"}]}"#).unwrap();
        let mut a = App::new(Some(root.join("a.txt")));
        a.resize(900, 700, 1.0);
        a.container_action("start");
        settle(&mut a, |_| crate::remote::active(&root).is_some(), 300);
        assert!(root.join("post-created").exists());
        let out = a.project_command("printf '%s|' \"$GREETING\" \"$(pwd)\"", 30).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hi there|/work|");
        assert_eq!(a.execution_commands(), ">Container:");
        assert_eq!(a.container_ports(), Some((vec![8123], true)));
        a.start_named_task("where").unwrap();
        settle(&mut a, |a| a.task_result("where").is_some(), 30);
        assert_eq!(a.task_result("where"), Some(true));
        // A stopped job leaves nothing behind in the container.
        a.start_named_task("long").unwrap();
        let name = container::name(&root);
        let count = || {
            let out = Command::new("docker").args(["exec", &name, "sh", "-c", "ps -o args | grep -c '^sleep 300'"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().parse::<u32>().unwrap_or(0)
        };
        let start = Instant::now();
        while count() == 0 && start.elapsed().as_secs() < 10 {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(count(), 1);
        a.stop_task();
        let start = Instant::now();
        while count() > 0 && start.elapsed().as_secs() < 15 {
            std::thread::sleep(Duration::from_millis(200));
            a.remote_tick();
        }
        assert_eq!(count(), 0);
        // A second start reuses the container without rerunning postCreateCommand.
        std::fs::remove_file(root.join("post-created")).unwrap();
        a.container_action("stop");
        assert!(crate::remote::active(&root).is_none());
        a.container_action("start");
        settle(&mut a, |_| crate::remote::active(&root).is_some(), 120);
        assert!(!root.join("post-created").exists());
        assert!(a.remote_projects.containers.log.lock().unwrap().contains("Reusing"));
        drop(a);
        let _ = Command::new("docker").args(["rm", "-f", &name]).output();
        std::fs::remove_dir_all(root).unwrap();
    }
}
