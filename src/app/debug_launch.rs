//! Work a debug configuration needs before its adapter starts: a named F6
//! task (`preLaunchTask`), then a Cargo build whose executable becomes the
//! program (`cargo`). Each step runs in the background; Debug: Stop cancels.
use super::*;
use crate::tools::Job;
use serde_json::{Value, json};

pub(super) enum Prep {
    /// Waiting for the preLaunchTask to finish.
    Task { config: Value, root: PathBuf, name: String },
    /// Waiting for `cargo build --message-format=json`.
    Cargo { config: Value, root: PathBuf, job: Job },
}

impl App {
    /// Runs whatever the configuration needs first, then launches it.
    pub(super) fn debug_prepare(&mut self, mut config: Value) {
        if self.debug_prep.is_some() {
            self.message = Some("Debug: a launch is already being prepared; use Debug: Stop to cancel it".into());
            return;
        }
        if let Some(task) = config.as_object_mut().and_then(|c| c.remove("preLaunchTask")) {
            let Some(name) = task.as_str().filter(|s| !s.is_empty()).map(str::to_string) else {
                self.message = Some("Native debug: preLaunchTask must name an F6 task, such as \"cargo build\"".into());
                return;
            };
            match self.start_named_task(&name) {
                Ok(()) => {
                    self.message = Some(format!("Debug: running {name} before launch"));
                    self.debug_prep = Some(Prep::Task { config, root: self.root.clone(), name });
                }
                Err(e) => self.message = Some(format!("Native debug: preLaunchTask: {e}")),
            }
            return;
        }
        if let Some(cargo) = config.as_object_mut().and_then(|c| c.remove("cargo")) {
            let argv = match super::debug_config::cargo_command(&cargo) {
                Ok(argv) => argv,
                Err(e) => { self.message = Some(format!("Native debug: {e}")); return; }
            };
            let command = argv.iter().map(|a| terminal::quote(Path::new(a))).collect::<Vec<_>>().join(" ");
            let job = Job::start(self.project_command(&command, 3600), self.waker.clone());
            self.debugger.output = format!("$ {}\n", argv.join(" "));
            self.debugger.update();
            self.message = Some(format!("Debug: building with {}", argv[..argv.len() - 1].join(" ")));
            self.debug_prep = Some(Prep::Cargo { config, root: self.root.clone(), job });
            return;
        }
        self.native_launch(config);
    }

    /// Moves a launch being prepared on when its step finishes.
    pub(super) fn debug_prep_poll(&mut self) {
        let root = match &self.debug_prep { Some(Prep::Task { root, .. } | Prep::Cargo { root, .. }) => root.clone(), None => return };
        if root != self.root {
            self.debug_prep_cancel();
            return;
        }
        match &self.debug_prep {
            Some(Prep::Task { name, .. }) => {
                let name = name.clone();
                let Some(ok) = self.task_result(&name) else { return };
                let Some(Prep::Task { config, name, .. }) = self.debug_prep.take() else { return };
                if ok {
                    self.debug_prepare(config);
                } else {
                    // The task's errors explain why; the launch does not go ahead.
                    if self.task_problems().is_empty() { self.show_task_output(); } else { self.open_errors(); }
                    self.message = Some(format!("Debug: {name} failed, so the debugger was not started"));
                }
            }
            Some(Prep::Cargo { job, .. }) => {
                let Some((out, ok)) = job.poll() else { return };
                let Some(Prep::Cargo { mut config, .. }) = self.debug_prep.take() else { return };
                match super::debug_config::cargo_executable(&out, ok) {
                    Ok(program) => {
                        if !config["launch"].is_object() { config["launch"] = json!({}); }
                        self.debugger.output += &format!("Built {program}\n");
                        config["launch"]["program"] = json!(program);
                        self.native_launch(config);
                    }
                    Err(e) => {
                        self.debugger.output += &e;
                        self.debugger.update();
                        self.show_side(Some(Tab::Debug));
                        self.message = Some(format!("Debug: {}", e.lines().next().unwrap_or("Cargo build failed")));
                    }
                }
            }
            None => {}
        }
        self.redraw = true;
    }

    /// Cancels a launch being prepared. Returns whether there was one.
    pub(super) fn debug_prep_cancel(&mut self) -> bool {
        match self.debug_prep.take() {
            Some(Prep::Task { name, .. }) => self.stop_named_task(&name),
            Some(Prep::Cargo { job, .. }) => job.stop(),
            None => return false,
        }
        self.message = Some("Debug: launch cancelled".into());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(a: &mut App, done: impl Fn(&App) -> bool) {
        let start = std::time::Instant::now();
        while !done(a) && start.elapsed().as_secs() < 60 {
            a.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(done(a), "{:?} {}", a.message, a.debugger.output);
    }
    fn project(name: &str, makefile: &str) -> (PathBuf, App) {
        let root = std::env::temp_dir().join(format!("ld-prep-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("Makefile"), makefile).unwrap();
        let source = root.join("main.rs");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        std::fs::write(root.join("adapter.py"), include_str!("../../assets/test_dap.py")).unwrap();
        let mut a = App::new(Some(source));
        a.resize(1200, 800, 1.0);
        (root, a)
    }
    fn config(root: &Path, extra: Value) -> Value {
        let mut c = json!({"adapter":["python3", root.join("adapter.py"), root.join("main.rs")],"transport":"stdio","launch":{"program":"${file}"}});
        for (k, v) in extra.as_object().unwrap() { c[k] = v.clone(); }
        c
    }
    #[test]
    fn pre_launch_task_runs_first_and_its_failure_stops_the_launch() {
        let (root, mut a) = project("task", "build:\n\ttouch built\nbroken:\n\t@echo 'main.rs:1:1: error: broken'; false\n");
        std::fs::write(root.join(".lightdown/debug.json"), config(&root, json!({"preLaunchTask":"make build"})).to_string()).unwrap();
        a.debug_command("native");
        assert!(a.debugger.dap.is_none(), "the adapter waits for the task");
        wait(&mut a, |a| a.debugger.paused);
        assert!(root.join("built").is_file());
        a.debug_command("stop");

        std::fs::write(root.join(".lightdown/debug.json"), config(&root, json!({"preLaunchTask":"make broken"})).to_string()).unwrap();
        a.debug_command("native");
        wait(&mut a, |a| a.debug_prep.is_none());
        assert!(a.debugger.dap.is_none());
        assert!(a.message.as_deref().is_some_and(|m| m.contains("make broken failed")), "{:?}", a.message);
        assert!(matches!(a.menu, Some((MenuKind::Errors, _))), "the task's errors are shown");

        std::fs::write(root.join(".lightdown/debug.json"), config(&root, json!({"preLaunchTask":"make missing"})).to_string()).unwrap();
        a.debug_command("native");
        assert!(a.message.as_deref().is_some_and(|m| m.contains("No task named make missing")), "{:?}", a.message);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_cancels_a_pre_launch_task() {
        let (root, mut a) = project("cancel", "slow:\n\tsleep 30\n");
        std::fs::write(root.join(".lightdown/debug.json"), config(&root, json!({"preLaunchTask":"make slow"})).to_string()).unwrap();
        a.debug_command("native");
        assert!(a.debug_prep.is_some());
        a.debug_command("stop");
        assert!(a.debug_prep.is_none() && !a.task_running("make slow"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cargo_build_supplies_the_program() {
        let (root, mut a) = project("cargo", "");
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"prepapp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[[bin]]\nname = \"prepapp\"\npath = \"main.rs\"\n").unwrap();
        // The fake adapter stops only when launched with the program it expects.
        let built = root.join("target/debug/prepapp");
        std::fs::write(root.join(".lightdown/debug.json"), json!({"adapter":["python3", root.join("adapter.py"), built],"cargo":{"args":["build","--bin","prepapp"]},"launch":{}}).to_string()).unwrap();
        a.debug_command("native");
        wait(&mut a, |a| a.debugger.paused);
        a.debug_command("stop");
        std::fs::write(root.join("main.rs"), "fn main() { let x: u8 = \"no\"; }\n").unwrap();
        a.debug_command("native");
        wait(&mut a, |a| a.debug_prep.is_none());
        assert!(a.debugger.dap.is_none());
        assert!(a.debugger.output.contains("mismatched types"), "{}", a.debugger.output);
        std::fs::remove_dir_all(root).unwrap();
    }
}
