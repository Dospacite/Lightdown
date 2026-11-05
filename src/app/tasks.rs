//! Discover project commands and retain their output and source locations.
use super::*;
use super::job_terminal::Job;
use std::process::Command;

#[derive(Clone, Debug, Default)]
struct Task {
    label: String,
    command: String,
    /// Background tasks that must be ready first.
    needs: Vec<String>,
    /// Globs whose saves rerun this task once it has been started.
    globs: Vec<String>,
    /// Set for a background task: what its output says once it is ready.
    ready: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Diagnostic {
    pub(super) path: PathBuf,
    pub(super) line: usize,
    pub(super) col: usize,
    pub(super) message: String,
}
/// A job running in a terminal tab.
struct Running {
    name: String,
    root: PathBuf,
    job: Job,
    /// A background task's ready pattern, until it has matched.
    ready: Option<regex::Regex>,
    seen: u64,
    background: bool,
}
/// Reruns a task when files are saved.
struct Watch {
    root: PathBuf,
    task: Task,
    /// Empty for any saved file.
    globs: Vec<String>,
    due: Option<std::time::Instant>,
    passed: Option<bool>,
}
#[derive(Default)]
pub(super) struct Tasks {
    listed: Vec<Task>,
    running: Vec<Running>,
    /// Tasks waiting for background tasks to say they are ready.
    waiting: Vec<(PathBuf, Task)>,
    /// Compound runs, as the terminal ids of their members.
    groups: std::collections::BTreeMap<u64, Vec<u64>>,
    next_group: u64,
    logs: Vec<(String, String)>,
    compounds: std::collections::BTreeMap<String, Vec<(String, String)>>,
    errors: Vec<Diagnostic>,
    selected: Option<usize>,
    output: String,
    last: Option<Task>,
    last_root: PathBuf,
    watches: Vec<Watch>,
    /// How each task last ended, or `true` once a background task is ready.
    results: std::collections::BTreeMap<String, bool>,
}
fn discover(root: &Path, python: &str) -> Vec<Task> {
    let mut out = Vec::new();
    if root.join("Cargo.toml").is_file() {
        for command in ["cargo check", "cargo build", "cargo test", "cargo clippy", "cargo fmt"] {
            out.push(Task { label: command.into(), command: command.into(), ..Default::default() });
        }
    }
    for name in ["GNUmakefile", "makefile", "Makefile"] {
        if let Ok(text) = std::fs::read_to_string(root.join(name)) {
            for line in text.lines().filter(|l| !l.starts_with(['\t', ' ', '#'])) {
                let Some((targets, rest)) = line.split_once(':') else { continue };
                if rest.starts_with('=') {
                    continue;
                }
                for target in targets
                    .split_whitespace()
                    .filter(|t| !t.starts_with('.') && t.chars().all(|c| c.is_alphanumeric() || "_-/".contains(c)))
                {
                    out.push(Task {
                        label: format!("make {target}"),
                        command: format!("make -- {}", terminal::quote(Path::new(target))),
                        ..Default::default()
                    });
                }
            }
            break;
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
    {
        let runner = if root.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if root.join("yarn.lock").is_file() {
            "yarn"
        } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
            "bun"
        } else {
            "npm"
        };
        for name in value["scripts"].as_object().into_iter().flatten().filter(|(_, v)| v.is_string()).map(|(k, _)| k) {
            out.push(Task {
                label: format!("{runner} run {name}"),
                command: format!("{runner} run {}", terminal::quote(Path::new(name))),
                ..Default::default()
            });
        }
    }
    if root.join("pyproject.toml").is_file() {
        let py = terminal::quote(Path::new(python));
        out.push(Task { label: "pytest".into(), command: format!("{py} -m pytest"), ..Default::default() });
        // Python is already required for notebooks. tomllib ships with Python 3.11.
        let result = Command::new(python).args(["-I", "-c", "import tomllib,json; p=tomllib.load(open('pyproject.toml','rb')); print(json.dumps(p.get('project',{}).get('scripts',{})))"])
            .current_dir(root).output();
        if let Ok(result) = result
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&result.stdout)
        {
            for (name, target) in value.as_object().into_iter().flatten() {
                let Some(target) = target.as_str() else { continue };
                let script = String::from(
                    "import importlib,sys; m,f=sys.argv[1].split(':',1); obj=importlib.import_module(m);\nfor part in f.split('.'): obj=getattr(obj,part)\nsys.exit(obj())",
                );
                out.push(Task {
                    label: format!("Python: {name}"),
                    command: format!(
                        "{py} -c {} {}",
                        terminal::quote(Path::new(&script)),
                        terminal::quote(Path::new(target))
                    ),
                    ..Default::default()
                });
            }
        }
    }
    if let Ok(tasks) = super::task_config::load_defs(root) {
        out.extend(tasks.into_iter().map(|t| Task { label: t.name, command: t.command, needs: t.needs, globs: t.watch, ready: t.ready }));
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out.dedup_by(|a, b| a.label == b.label);
    out
}
fn plain(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}
pub(super) fn diagnostics(root: &Path, output: &str) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for line in plain(output).lines() {
        let text = line.trim().trim_start_matches("--> ").trim_start_matches("::: ");
        let python = text
            .strip_prefix("File \"")
            .and_then(|s| s.split_once("\", line "))
            .and_then(|(p, n)| Some((p, n.split(',').next()?.parse().ok()?, 1)));
        let typescript = text.split_once('(').and_then(|(p, rest)| {
            let (nums, _) = rest.split_once(')')?;
            let (line, col) = nums.split_once(',')?;
            Some((p, line.parse().ok()?, col.parse().ok()?))
        });
        let mut location = python.or(typescript);
        if location.is_none() {
            // Try each colon so filenames containing a colon remain usable.
            for (i, _) in text.match_indices(':') {
                let rest = &text[i + 1..];
                let mut parts = rest.split(':');
                let Some(line) = parts.next().and_then(|s| s.parse::<usize>().ok()) else { continue };
                let col = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                location = Some((&text[..i], line, col));
                break;
            }
        }
        let Some((name, line, col)) = location else { continue };
        let path = root.join(name.trim());
        if line == 0 || !path.is_file() {
            continue;
        }
        let path = path.canonicalize().unwrap_or(path);
        let error = Diagnostic { path, line, col: col.max(1), message: text.into() };
        if !found
            .iter()
            .any(|e: &Diagnostic| (e.path.as_path(), e.line, e.col) == (error.path.as_path(), error.line, error.col))
        {
            found.push(error);
        }
    }
    found
}
impl Running {
    fn state(&self) -> &'static str {
        match (self.background, self.ready.is_some()) {
            (true, true) => "Starting; waiting for its ready pattern",
            (true, false) => "Ready; running in the background",
            _ => "Running",
        }
    }
}
impl App {
    pub(super) fn task_problems(&self) -> Vec<super::problems::Problem> {
        if self.task_state.last_root != self.root { return Vec::new(); }
        self.task_state.errors.iter().map(|p| super::problems::Problem { path:Some(p.path.clone()),line:p.line,col:p.col,message:p.message.clone(),source:"Task" }).collect()
    }
    pub(super) fn task_output(&self) -> String {
        self.task_state.output.clone()
    }
    fn job_running(&self, root: &Path, name: &str) -> Option<usize> {
        self.task_state.running.iter().position(|r| r.root == root && r.name == name)
    }
    /// Whether background task `name` has said it is ready.
    fn task_ready(&self, name: &str) -> bool {
        self.job_running(&self.root, name).is_some_and(|i| {
            let r = &self.task_state.running[i];
            r.background && r.ready.is_none()
        })
    }
    pub(super) fn task_status(&self) -> Option<String> {
        let jobs = match self.task_state.running.len() + self.task_state.waiting.len() {
            0 => String::new(),
            1 => "1 job running".into(),
            n => format!("{n} jobs running"),
        };
        if let Some(w) = self.task_state.watches.iter().find(|w| w.root == self.root) {
            let status = if self.job_running(&w.root, &w.task.label).is_some() {
                "running"
            } else if w.due.is_some() {
                "pending"
            } else {
                match w.passed {
                    Some(true) => "passed",
                    Some(false) => "failed",
                    None => "ready",
                }
            };
            let more = if jobs.is_empty() { String::new() } else { format!("; {jobs}") };
            return Some(format!("Watch {}: {status}{more}", w.task.label));
        }
        (!jobs.is_empty()).then_some(jobs)
    }
    pub(super) fn watch_badge(&self) -> Option<bool> {
        self.task_state.watches.iter().find(|w| w.root == self.root).and_then(|w| w.passed)
    }
    fn arm_watch(&mut self, task: Task) {
        let root = self.root.clone();
        self.task_state.watches.retain(|w| !(w.root == root && w.task.label == task.label));
        if self.task_state.watches.len() >= 16 {
            self.task_state.watches.remove(0);
        }
        self.task_state.watches.push(Watch { root, globs: task.globs.clone(), task, due: None, passed: None });
    }
    pub(super) fn toggle_watch(&mut self) {
        let root = self.root.clone();
        if self.task_state.watches.iter().any(|w| w.root == root) {
            self.task_state.watches.retain(|w| w.root != root);
            self.message = Some("Watch mode off".into());
            return;
        }
        let task = self
            .task_state
            .last
            .clone()
            .filter(|t| {
                self.task_state.last_root == self.root
                    && (!t.globs.is_empty()
                        || t.command.starts_with("cargo check")
                        || t.command.starts_with("cargo test")
                        || t.command.contains(" -m pytest"))
            })
            .or_else(|| {
                if self.root.join("Cargo.toml").is_file() {
                    Some(Task { label: "cargo check".into(), command: "cargo check".into(), ..Default::default() })
                } else if self.root.join("pyproject.toml").is_file() {
                    Some(Task {
                        label: "pytest".into(),
                        command: format!("{} -m pytest", terminal::quote(Path::new(&self.cfg.python_for(&self.root)))),
                        ..Default::default()
                    })
                } else {
                    None
                }
            });
        let Some(task) = task else {
            self.message = Some("Run cargo check, cargo test, pytest or a task with watch globs before enabling watch".into());
            return;
        };
        self.arm_watch(task);
        self.task_state.watches.last_mut().unwrap().due = Some(std::time::Instant::now());
        self.watch_tick();
    }
    /// Schedules the watched tasks that `path` concerns.
    pub(super) fn watch_saved(&mut self, path: &Path) {
        let due = std::time::Instant::now() + std::time::Duration::from_millis(300);
        for w in self.task_state.watches.iter_mut().filter(|w| w.root == self.root) {
            let Ok(rel) = path.strip_prefix(&w.root) else { continue };
            let rel = rel.to_string_lossy();
            if w.globs.is_empty() || w.globs.iter().any(|g| super::saving::glob(g, &rel)) {
                w.due = Some(due);
            }
        }
    }
    /// Starts watched tasks whose saves have settled. One that is still
    /// running keeps its rerun queued until it ends; a background one is
    /// restarted.
    pub(super) fn watch_tick(&mut self) {
        let now = std::time::Instant::now();
        let mut start = Vec::new();
        for i in 0..self.task_state.watches.len() {
            let w = &self.task_state.watches[i];
            if w.root != self.root || !w.due.is_some_and(|d| d <= now) || self.task_state.waiting.iter().any(|(_, t)| t.label == w.task.label) {
                continue;
            }
            if let Some(r) = self.job_running(&w.root, &w.task.label) {
                if !self.task_state.running[r].background {
                    continue;
                }
                self.task_state.running.remove(r).job.stop();
            }
            self.task_state.watches[i].due = None;
            start.push(self.task_state.watches[i].task.clone());
        }
        for task in start {
            self.run_task(task);
        }
    }
    pub(super) fn project_tasks(&self) -> Vec<(String, String)> {
        discover(&self.root, &self.cfg.python_for(&self.root)).into_iter().map(|t| (t.label, t.command)).collect()
    }
    /// Starts a project task by its F6 name in its own terminal tab, after
    /// the background tasks it needs are ready. `task_result` gives the
    /// outcome once it has ended. The debugger's pre-launch task uses these.
    pub(super) fn start_named_task(&mut self, name: &str) -> Result<(), String> {
        super::task_config::load_defs(&self.root).map_err(|e| format!(".lightdown/tasks.json: {e}"))?;
        let task = discover(&self.root, &self.cfg.python_for(&self.root))
            .into_iter()
            .find(|t| t.label == name)
            .ok_or_else(|| format!("No task named {name}"))?;
        if self.task_state.running.len() >= 16 {
            return Err("At most 16 jobs can run at once".into());
        }
        self.task_state.results.remove(name);
        self.run_task(task);
        Ok(())
    }
    /// How task `name` last ended: `None` while it runs or waits, and `true`
    /// once a background task is ready.
    pub(super) fn task_result(&self, name: &str) -> Option<bool> {
        if self.task_state.waiting.iter().any(|(_, t)| t.label == name) {
            return None;
        }
        if let Some(i) = self.job_running(&self.root, name) {
            return self.task_ready(name).then_some(true).filter(|_| self.task_state.running[i].background);
        }
        self.task_state.results.get(name).copied()
    }
    /// Whether task `name` runs or waits for the tasks it depends on.
    #[cfg(test)]
    pub(super) fn task_running(&self, name: &str) -> bool {
        self.task_state.waiting.iter().any(|(_, t)| t.label == name) || self.job_running(&self.root, name).is_some()
    }
    /// Stops task `name` if it runs, and drops it if it waits, as cancelling
    /// a debug launch does with its pre-launch task.
    pub(super) fn stop_named_task(&mut self, name: &str) {
        self.task_state.waiting.retain(|(_, t)| t.label != name);
        let root = self.root.clone();
        if let Some(i) = self.job_running(&root, name) {
            let term = self.task_state.running[i].job.term;
            self.stop_job(term);
        }
    }
    pub(super) fn set_run_compounds(&mut self, compounds: std::collections::BTreeMap<String, Vec<(String, String)>>) {
        self.task_state.compounds = compounds;
    }
    pub(super) fn offer_run_configs(&mut self, entries: Vec<(String, String)>, default: Option<String>) {
        self.task_state.listed = entries.into_iter().map(|(label, command)| Task { label, command, ..Default::default() }).collect();
        if let Some(default) = default {
            if let Some(i) = self.task_state.listed.iter().position(|t| t.label == default) { self.start_task(&i.to_string()); }
            else { self.message = Some("Set a valid default name in .lightdown/run.json".into()); }
        } else {
            let detail = if self.multi_root() { format!("Run configuration \u{b7} {}", self.root_name(&self.root)) } else { "Run configuration".into() };
            let entries = self.task_state.listed.iter().enumerate().map(|(i, t)| Entry::new(&i.to_string(), &t.label, &detail)).collect();
            self.menu = Some((MenuKind::Tasks, Menu::new("Run configurations", entries, "")));
        }
    }
    /// Menu entries for the jobs running and waiting, to stop them.
    fn job_entries(&self) -> Vec<Entry> {
        let mut entries: Vec<_> = self
            .task_state
            .running
            .iter()
            .map(|r| Entry::new(&format!("stop:{}", r.job.term), &r.name, &format!("{}; select to stop", r.state())))
            .collect();
        entries.extend(self.task_state.waiting.iter().map(|(_, t)| {
            Entry::new(&format!("wait:{}", t.label), &t.label, &format!("Waiting for {}; select to cancel", t.needs.join(", ")))
        }));
        entries
    }
    pub(super) fn open_tasks(&mut self) {
        if let Err(e) = super::task_config::load_defs(&self.root) {
            self.message = Some(format!(".lightdown/tasks.json: {e}"));
            return;
        }
        self.task_state.listed = discover(&self.root, &self.cfg.python_for(&self.root));
        let mut entries = self.job_entries();
        entries.extend(self.task_state.listed.iter().enumerate().map(|(i, t)| {
            let detail = match (&t.ready, t.globs.is_empty()) {
                (Some(_), _) => format!("Background · {}", t.command),
                (None, false) => format!("Watches {} · {}", t.globs.join(" "), t.command),
                _ => t.command.clone(),
            };
            Entry::new(&i.to_string(), &t.label, &detail)
        }));
        self.other_root_tasks(&mut entries);
        let mut menu = Menu::new("Project tasks", entries, "");
        menu.hint = "Run a task from this project, or stop a running one".into();
        self.menu = Some((MenuKind::Tasks, menu));
    }
    /// In a window of several folders, the tasks of the folders the active
    /// file is not in, each after its folder's name.
    fn other_root_tasks(&self, entries: &mut Vec<Entry>) {
        if !self.multi_root() {
            return;
        }
        let here = self.root_name(&self.root);
        for e in entries.iter_mut().filter(|e| e.key.parse::<usize>().is_ok()) {
            e.detail = format!("{here} \u{b7} {}", e.detail);
        }
        for (r, root) in self.folders.iter().enumerate().filter(|(_, r)| **r != self.root) {
            let name = self.root_name(root);
            for t in discover(root, &self.cfg.python_for(root)) {
                entries.push(Entry::new(&format!("root:{r}\n{}", t.label), &t.label, &format!("{name} \u{b7} {}", t.command)));
            }
        }
    }
    /// Starts a task of another of the window's folders, in that folder.
    fn start_root_task(&mut self, key: &str) {
        let Some((r, label)) = key.split_once('\n') else { return };
        let Some(root) = r.parse::<usize>().ok().and_then(|r| self.folders.get(r)).cloned() else { return };
        self.with_root(&root, |a| {
            a.task_state.listed = discover(&root, &a.cfg.python_for(&root));
            if let Some(i) = a.task_state.listed.iter().position(|t| t.label == label) {
                a.start_task(&i.to_string());
            }
        });
    }
    /// Stops one job, by its terminal id, and its watch.
    fn stop_job(&mut self, term: u64) {
        let Some(i) = self.task_state.running.iter().position(|r| r.job.term == term) else { return };
        let r = self.task_state.running.remove(i);
        self.task_state.watches.retain(|w| !(w.root == r.root && w.task.label == r.name));
        r.job.stop();
        self.message = Some(format!("Stopped {}", r.name));
    }
    pub(super) fn start_task(&mut self, key: &str) {
        if let Some(rest) = key.strip_prefix("root:") {
            return self.start_root_task(rest);
        }
        if let Some(rest) = key.strip_prefix("port:") {
            return self.port_pick(rest);
        }
        if let Some(index) = key.strip_prefix("log:").and_then(|s| s.parse::<usize>().ok()) {
            if let Some((_, text)) = self.task_state.logs.get(index) { self.open_output(text.clone()); }
            return;
        }
        if let Some(term) = key.strip_prefix("stop:").and_then(|s| s.parse::<u64>().ok()) {
            return self.stop_job(term);
        }
        if let Some(label) = key.strip_prefix("wait:") {
            self.task_state.waiting.retain(|(_, t)| t.label != label);
            self.task_state.watches.retain(|w| w.task.label != label);
            self.message = Some(format!("Cancelled {label}"));
            return;
        }
        if let Some(group) = key.strip_prefix("group:").and_then(|s| s.parse::<u64>().ok()) {
            if let Some(terms) = self.task_state.groups.remove(&group) {
                for term in terms {
                    self.stop_job(term);
                }
                self.message = Some("Stopped the compound's jobs".into());
            }
            return;
        }
        if self.task_state.running.len() >= 16 {
            self.message = Some("At most 16 jobs can run at once".into());
            return;
        }
        let Some(task) = key.parse::<usize>().ok().and_then(|i| self.task_state.listed.get(i)).cloned() else { return };
        if let Some(members) = self.task_state.compounds.get(&task.label).cloned().filter(|_| task.command.is_empty()) {
            if self.task_state.running.len() + members.len() > 16 {
                self.message = Some("Compound would exceed the 16 job limit".into());
                return;
            }
            let first = self.task_state.running.len();
            for (label, command) in members {
                self.task_state.listed.push(Task { label, command, ..Default::default() });
                self.start_task(&(self.task_state.listed.len() - 1).to_string());
            }
            self.task_state.next_group += 1;
            let terms = self.task_state.running[first..].iter().map(|r| r.job.term).collect();
            self.task_state.groups.insert(self.task_state.next_group, terms);
            self.task_state.last = Some(task);
            return;
        }
        if self.task_state.last_root != self.root { self.task_state.errors.clear(); }
        self.task_state.last_root = self.root.clone();
        self.task_state.last = Some(task.clone());
        if !task.globs.is_empty() {
            self.arm_watch(task.clone());
        }
        self.run_task(task);
        if self.task_state.watches.iter().any(|w| w.root == self.root && !w.globs.is_empty() && self.task_state.last.as_ref().is_some_and(|t| t.label == w.task.label)) {
            self.message = Some(format!("{}; saves rerun it until Task: Stop", self.message.clone().unwrap_or_default()));
        }
    }
    /// Starts the background tasks `task` needs, then `task` once they are ready.
    fn run_task(&mut self, task: Task) {
        if task.needs.is_empty() {
            return self.launch(task);
        }
        let defs = discover(&self.root, &self.cfg.python_for(&self.root));
        for need in &task.needs {
            let started = self.job_running(&self.root, need).is_some() || self.task_state.waiting.iter().any(|(_, t)| &t.label == need);
            if !started && let Some(def) = defs.iter().find(|t| &t.label == need) {
                self.run_task(def.clone());
            }
        }
        if task.needs.iter().all(|n| self.task_ready(n)) {
            self.launch(task);
        } else {
            self.message = Some(format!("{} waits for {}", task.label, task.needs.join(", ")));
            self.task_state.waiting.push((self.root.clone(), task));
        }
    }
    /// The command a job runs on its terminal. Foreground jobs end after an
    /// hour; background ones run until stopped.
    fn job_command(&self, command: &str, background: bool) -> std::process::Command {
        if let Some(remote) = crate::remote::active(&self.root) {
            return remote.command(command, true);
        }
        let mut cmd = std::process::Command::new(if background { "bash" } else { "timeout" });
        if !background {
            cmd.args(["--foreground", "-k", "2", "3600", "bash"]);
        }
        cmd.args(["-c", command]).current_dir(&self.root);
        crate::environment::apply(&mut cmd, &self.root, Some(&self.cfg.python_for(&self.root)));
        cmd
    }
    fn launch(&mut self, task: Task) {
        if self.task_state.running.len() >= 16 {
            self.message = Some("At most 16 jobs can run at once".into());
            return;
        }
        if self.task_state.last_root != self.root { self.task_state.errors.clear(); }
        self.task_state.last_root = self.root.clone();
        self.task_state.selected = None;
        self.task_state.results.remove(&task.label);
        self.task_state.output = format!("$ {}\nRunning…", task.command);
        let ready = task.ready.as_deref().and_then(|p| super::task_config::ready_pattern(p).ok());
        let command = self.job_command(&task.command, ready.is_some());
        let job = match Job::start(self, &task.label, command) {
            Ok(job) => job,
            Err(e) => { self.message = Some(format!("Cannot start task: {e}")); return; }
        };
        self.message = Some(format!("Running {}", task.label));
        let background = ready.is_some();
        self.task_state.running.push(Running { name: task.label, root: self.root.clone(), job, ready, seen: 0, background });
    }
    pub(super) fn tasks_poll(&mut self) {
        for r in &mut self.task_state.running {
            if let Some(pattern) = &r.ready
                && r.job.seen(pattern, &mut r.seen)
            {
                r.ready = None;
                self.task_state.results.insert(r.name.clone(), true);
                self.message = Some(format!("{} is ready", r.name));
                self.redraw = true;
            }
        }
        let mut index = 0;
        while index < self.task_state.running.len() {
            let Some((out, code)) = self.task_state.running[index].job.poll() else { index += 1; continue };
            let Running { name, root, .. } = self.task_state.running.remove(index);
            let ok = code == Some(0);
            self.task_state.results.insert(name.clone(), ok);
            for w in self.task_state.watches.iter_mut().filter(|w| w.root == root && w.task.label == name) {
                w.passed = Some(ok);
            }
            if root == self.root {
                self.task_state.errors.extend(diagnostics(&root, &out));
                for (file, line, col, message) in super::task_config::locations(&root, &name, &plain(&out)) {
                    let path = root.join(file);
                    if path.is_file() {
                        self.task_state.errors.push(Diagnostic { path: path.canonicalize().unwrap_or(path), line, col, message });
                    }
                }
                if self.task_state.errors.len() > 512 { self.task_state.errors.drain(..self.task_state.errors.len() - 512); } }
            let ending = match code {
                Some(0) => "Passed".to_string(),
                Some(n) => format!("Failed with status {n}"),
                None => "Stopped by a signal".to_string(),
            };
            self.task_state.output = format!("$ {name}\n{}\n\n{ending}", plain(&out));
            self.task_state.logs.push((name.clone(), self.task_state.output.clone()));
            if self.task_state.logs.len() > 32 { self.task_state.logs.remove(0); }
            self.message = Some(format!("{name}: {}. Task: Show output opens logs.", ending.to_lowercase()));
            self.disk_poll(true);
            self.redraw = true;
        }
        let mut i = 0;
        while i < self.task_state.waiting.len() {
            let (root, task) = &self.task_state.waiting[i];
            if root != &self.root {
                i += 1;
                continue;
            }
            if task.needs.iter().all(|n| self.task_ready(n)) {
                let (_, task) = self.task_state.waiting.remove(i);
                self.launch(task);
            } else if let Some(gone) = task.needs.iter().find(|n| {
                self.job_running(&self.root, n).is_none() && !self.task_state.waiting.iter().any(|(_, t)| &t.label == *n)
            }) {
                self.message = Some(format!("{} not started: {gone} ended before it was ready", task.label));
                let (_, task) = self.task_state.waiting.remove(i);
                self.task_state.results.insert(task.label, false);
            } else {
                i += 1;
            }
        }
        self.watch_tick();
    }
    pub(super) fn rerun_task(&mut self) {
        if self.task_state.last_root != self.root {
            self.message = Some("Last task belongs to another project".into());
            return;
        }
        if let Some(task) = self.task_state.last.clone() {
            self.task_state.listed.push(task);
            self.start_task(&(self.task_state.listed.len() - 1).to_string());
        }
    }
    pub(super) fn stop_job_menu(&mut self) {
        let mut entries = self.job_entries();
        for (group, terms) in &self.task_state.groups {
            let names: Vec<_> = self.task_state.running.iter().filter(|r| terms.contains(&r.job.term)).map(|r| r.name.as_str()).collect();
            if !names.is_empty() {
                entries.push(Entry::new(&format!("group:{group}"), &format!("Compound: {}", names.join(", ")), "Stop all remaining members"));
            }
        }
        if entries.is_empty() {
            self.message = Some("No jobs are running".into());
            return;
        }
        self.menu = Some((MenuKind::Tasks, Menu::new("Stop job", entries, "")));
    }
    pub(super) fn stop_task(&mut self) {
        self.task_state.watches.clear();
        self.task_state.waiting.clear();
        for r in self.task_state.running.drain(..) {
            r.job.stop();
            self.message = Some(format!("Stopped {}", r.name));
        }
    }
    pub(super) fn show_task_output(&mut self) {
        let mut entries = self.job_entries();
        entries.extend(self.task_state.logs.iter().enumerate().rev().map(|(i, (name, text))|
            Entry::new(&format!("log:{i}"), name, text.lines().last().unwrap_or("Finished"))));
        if self.task_state.logs.len() == 1 && self.task_state.running.is_empty() && self.task_state.waiting.is_empty() {
            self.open_output(self.task_state.logs[0].1.clone());
        } else {
            self.menu = Some((MenuKind::Tasks, Menu::new("Job output and Stop", entries, "")));
        }
    }
    pub(super) fn open_errors(&mut self) {
        let entries = self
            .task_state
            .errors
            .iter()
            .enumerate()
            .map(|(i, e)| {
                Entry::new(
                    &i.to_string(),
                    &e.message,
                    &format!("{}:{}:{}", crate::tools::short(&self.root, &e.path), e.line, e.col),
                )
            })
            .collect();
        self.menu = Some((MenuKind::Errors, Menu::new("Task errors", entries, "")));
    }
    pub(super) fn pick_error(&mut self, key: &str) {
        let Some(i) = key.parse::<usize>().ok().filter(|&i| i < self.task_state.errors.len()) else { return };
        self.task_state.selected = Some(i);
        let e = self.task_state.errors[i].clone();
        self.remember_jump();
        // Locate in the current buffer, including unsaved tabs.
        self.jump_to(e.path.clone(), 0, 0);
        if self.doc.path.as_ref() != Some(&e.path) || self.reader.is_some() {
            return;
        }
        let (s, end) = self.doc.buf.line(e.line.saturating_sub(1).min(self.doc.buf.lines() - 1));
        let at = self.doc.buf.text[s..end].char_indices().nth(e.col - 1).map_or(end, |(n, _)| s + n);
        self.doc.select(at, at);
        self.doc.moved();
        self.message = Some(format!("{} of {}: {}", i + 1, self.task_state.errors.len(), e.message));
    }
    pub(super) fn next_error(&mut self, back: bool) {
        self.next_problem(back);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_tasks_run_and_errors_navigate_to_sources() {
        let dir = std::env::temp_dir().join(format!("ld-tasks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname='test'\nversion='0.1.0'").unwrap();
        std::fs::write(dir.join("Makefile"), ".PHONY: build\nbuild test:\n\techo hello\nVAR := a\n").unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"lint":"true"}}"#).unwrap();
        std::fs::write(dir.join("pyproject.toml"), "[project.scripts]\nhello = 'example:main'\n").unwrap();
        let listed = discover(&dir, "python3");
        assert!(listed.iter().any(|t| t.label == "cargo check"));
        assert!(listed.iter().any(|t| t.label == "make test"));
        assert!(listed.iter().any(|t| t.label == "npm run lint"));
        assert!(listed.iter().any(|t| t.label == "Python: hello"));
        assert!(!listed.iter().any(|t| t.label.contains("VAR")));
        let errors = diagnostics(
            &dir,
            "\x1b[31merror\x1b[0m\n  --> a.py:2:2\n  File \"a.py\", line 3, in main\na.py(1,2): error TS100\na.py:2:2: error duplicate",
        );
        assert_eq!(errors.len(), 3);
        assert_eq!((errors[0].line, errors[0].col), (2, 2));
        let mut app = App::new(Some(dir.join("a.py")));
        app.task_state
            .listed
            .push(Task { label: "fixture".into(), command: "printf 'a.py:2:2: error: fixture\\n'; exit 1".into(), ..Default::default() });
        app.start_task("0");
        let until = std::time::Instant::now();
        while !app.task_state.running.is_empty() && until.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.tasks_poll();
        }
        assert_eq!(app.task_state.errors.len(), 1);
        assert!(app.task_state.output.contains("Failed"));
        app.next_error(false);
        assert_eq!(app.doc.buf.cur, 5);
        app.show_task_output();
        assert!(app.doc.read_only);
        assert!(app.doc.buf.text.contains("fixture"));
        app.next_error(true);
        assert_eq!(app.doc.buf.cur, 5);
        app.navigate(false);
        assert!(app.doc.read_only);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod watch_tests {
    use super::*;
    #[test]
    fn saves_coalesce_and_never_overlap_running_tasks() {
        let dir = std::env::temp_dir().join(format!("ld-watch-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.py"), "source\n").unwrap();
        std::fs::write(dir.join(".env"), "LD_WATCH_FIXTURE=from_dotenv\n").unwrap();
        let mut app = App::new(Some(dir.join("a.py")));
        let task =
            Task { label: "fixture".into(), command: "printf '%s\\n' \"$LD_WATCH_FIXTURE\" >> runs; sleep 0.1".into(), ..Default::default() };
        app.arm_watch(task);
        app.task_state.watches[0].due = Some(std::time::Instant::now());
        app.watch_tick();
        assert!(!app.task_state.running.is_empty());
        app.save();
        app.save();
        assert!(app.task_state.watches[0].due.is_some());
        app.task_state.watches[0].due = Some(std::time::Instant::now());
        app.watch_tick();
        let until = std::time::Instant::now();
        while !app.task_state.running.is_empty() && until.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.tasks_poll();
        }
        // The queued rerun started as soon as the first run ended.
        assert_eq!(std::fs::read_to_string(dir.join("runs")).unwrap(), "from_dotenv\nfrom_dotenv\n");
        assert_eq!(app.watch_badge(), Some(true));
        app.stop_task();
        assert!(app.task_state.watches.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod concurrent_tests {
    use super::*;
    #[test]
    fn jobs_retain_separate_logs_and_compounds_start_together() {
        let root = std::env::temp_dir().join(format!("ld-concurrent-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("main.py"), "one\ntwo\n").unwrap();
        std::fs::write(root.join(".lightdown/run.json"), r#"{"default":"both","configurations":[{"name":"one","command":"sh","args":["-c","sleep 0.1; printf 'main.py:1:1: first\\n'"]},{"name":"two","command":"sh","args":["-c","sleep 0.1; printf 'main.py:2:1: second\\n'"]},{"name":"both","compound":["one","two"]}]}"#).unwrap();
        let mut app = App::new(Some(root.join("main.py")));
        app.run_configs(true);
        assert_eq!(app.task_state.running.len(), 2);
        assert!(app.task_status().unwrap().contains("2 jobs"));
        let start = std::time::Instant::now();
        while !app.task_state.running.is_empty() && start.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.tasks_poll();
        }
        assert_eq!(app.task_state.logs.len(), 2);
        assert_eq!(app.task_state.errors.len(), 2);
        app.rerun_task();
        assert_eq!(app.task_state.running.len(), 2);
        let first = app.task_state.running[0].job.term;
        app.start_task(&format!("stop:{first}"));
        assert_eq!(app.task_state.running.len(), 1);
        app.stop_task();
        assert!(app.task_state.running.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod background_tests {
    use super::*;
    fn settle(app: &mut App, until: impl Fn(&App) -> bool) {
        let start = std::time::Instant::now();
        while !until(app) && start.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            app.term_poll();
            app.tasks_poll();
        }
        assert!(until(app), "{:?}", app.message);
    }
    #[test]
    fn background_tasks_start_dependents_when_ready_and_watches_rerun() {
        let root = std::env::temp_dir().join(format!("ld-background-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("notes.md"), "notes\n").unwrap();
        std::fs::write(root.join(".lightdown/tasks.json"), r#"{"tasks":[
            {"name":"server","shell":"sleep 0.3; echo 'Listening on http://localhost:4567'; sleep 30","background":{"readyPattern":"Listening on"}},
            {"name":"client","shell":"echo client >> runs","dependsOn":["server"],"watch":["src/**/*.rs"]}]}"#).unwrap();
        let mut app = App::new(Some(root.join("src/a.rs")));
        app.resize(900, 700, 1.0);
        app.start_named_task("client").unwrap();
        assert_eq!(app.task_state.running.len(), 1);
        assert_eq!(app.task_state.waiting.len(), 1);
        assert_eq!(app.task_result("client"), None);
        assert!(app.task_status().unwrap().contains("2 jobs"));
        settle(&mut app, |a| a.task_result("client") == Some(true));
        assert_eq!(app.task_result("server"), Some(true));
        assert_eq!(std::fs::read_to_string(root.join("runs")).unwrap(), "client\n");
        // The job terminals are named after their tasks and stay open.
        assert!(app.terms.iter().any(|t| t.title() == "client"));
        // Agents cannot reach job terminals.
        let id = app.task_state.running[0].job.term;
        assert!(app.agent_terminal("read_terminal", &serde_json::json!({"id": id})).is_err());
        // Starting from F6 arms the watch; only matching saves rerun it.
        app.open_tasks();
        let i = app.task_state.listed.iter().position(|t| t.label == "client").unwrap();
        app.start_task(&i.to_string());
        settle(&mut app, |a| a.task_result("client") == Some(true));
        app.watch_saved(&root.join("notes.md"));
        assert!(app.task_state.watches[0].due.is_none());
        app.watch_saved(&root.join("src/a.rs"));
        app.task_state.watches[0].due = Some(std::time::Instant::now());
        app.tasks_poll();
        settle(&mut app, |a| a.task_result("client") == Some(true));
        assert_eq!(std::fs::read_to_string(root.join("runs")).unwrap(), "client\nclient\nclient\n");
        // Stopping the server leaves nothing waiting or running.
        let server = app.task_state.running.iter().find(|r| r.name == "server").unwrap().job.term;
        app.start_task(&format!("stop:{server}"));
        assert!(app.task_state.running.is_empty());
        assert!(app.terms.iter().any(|t| t.title() == "server"));
        app.stop_task();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_background_task_that_ends_early_fails_its_dependents() {
        let root = std::env::temp_dir().join(format!("ld-background-fail-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join(".lightdown/tasks.json"), r#"{"tasks":[
            {"name":"server","shell":"echo crashed; exit 3","background":{"readyPattern":"never"}},
            {"name":"client","shell":"echo client > ran","dependsOn":["server"]}]}"#).unwrap();
        let mut app = App::new(Some(root.join("a.txt")));
        app.resize(900, 700, 1.0);
        app.start_named_task("client").unwrap();
        settle(&mut app, |a| a.task_result("client") == Some(false));
        assert_eq!(app.task_result("server"), Some(false));
        assert!(!root.join("ran").exists());
        assert!(app.task_state.logs.iter().any(|(n, t)| n == "server" && t.ends_with("Failed with status 3")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
