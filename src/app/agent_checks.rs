//! Verification for the agent: the `problems` and `run_tests` tools, and the
//! project's `check` command, run when a turn that changed files ends.
use super::*;
use crate::agent::Item;
use crate::tools::Job;

/// The after-turn check's state for the current agent turn.
#[derive(Default)]
pub(super) struct Checks {
    /// The running check and its command line.
    running: Option<(Job, String)>,
    /// Failures already sent back to the model in this turn.
    sent: u32,
}
impl Checks {
    pub fn busy(&self) -> bool { self.running.is_some() }
    /// A new user message starts a new count.
    pub fn reset(&mut self) { *self = Self::default(); }
}

/// A finished tool job's result as the model receives it.
pub(super) fn job_result(name: &str, out: String, ok: bool) -> String {
    if name == "run_tests" { crate::tools_checks::test_results(&out, ok).to_string() } else { out }
}

const CHECK_OUTPUT: usize = 12_000;

impl App {
    /// The shell command `run_tests` runs. One small adapter, so another
    /// test runner can take over here.
    pub(super) fn agent_test_command(&self, id: &str, file: &str) -> Result<String, String> {
        let (id, file) = (id.trim(), file.trim());
        if self.root.join("Cargo.toml").is_file() {
            let mut cmd = "cargo test".to_string();
            if !file.is_empty() {
                let path = crate::tools::locate(&self.root, file)?;
                let rel = path.strip_prefix(&self.root).unwrap_or(&path);
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if rel.parent() != Some(Path::new("tests")) || path.extension().is_none_or(|e| e != "rs") {
                    return Err("In a Cargo project, `file` must be an integration test in tests/; give `id` for unit tests".into());
                }
                cmd += &format!(" --test {}", terminal::quote(Path::new(stem)));
            }
            if !id.is_empty() { cmd += &format!(" -- {} --color=never", terminal::quote(Path::new(id))); } else { cmd += " -- --color=never"; }
            Ok(cmd)
        } else {
            let python = terminal::quote(Path::new(&self.cfg.python_for(&self.root)));
            if !file.is_empty() { crate::tools::locate(&self.root, file)?; }
            let selection = match (file.is_empty(), id.is_empty()) {
                (true, true) => String::new(),
                (true, false) => id.to_string(),
                (false, true) => file.to_string(),
                (false, false) if id.contains("::") => id.to_string(),
                (false, false) => format!("{file}::{id}"),
            };
            let selection = if selection.is_empty() { String::new() } else { format!(" -- {}", terminal::quote(Path::new(&selection))) };
            Ok(format!("{python} -m pytest -v -rfE --tb=short --color=no -p no:cacheprovider{selection}"))
        }
    }

    /// The `problems` tool's answer.
    pub(super) fn agent_problems(&self) -> String {
        let list: Vec<_> = self.problem_list().into_iter().map(|p| {
            let severity = match p.source {
                "LSP" => match p.path.as_deref().and_then(|path| self.diagnostic_severity(path, p.line, &p.message)) {
                    Some(1) => "error", Some(2) => "warning", Some(3) => "information", Some(4) => "hint", _ => "error",
                },
                "Test" => "failure",
                _ => "error",
            };
            let file = p.path.as_ref().map(|f| crate::tools::short(&self.root, f));
            (p.source.to_string(), file, p.line, severity, p.message)
        }).collect();
        crate::tools_checks::problems(&list).to_string()
    }

    /// Starts the project's check if the turn ending now changed files.
    /// Returns whether it started; the turn then ends when it does.
    pub(super) fn agent_check_start(&mut self) -> bool {
        if self.ai.plan_mode || !self.cfg.agent_check || self.ai.checks.busy() { return false; }
        let Some(command) = self.cfg.check_for(&self.root) else { return false };
        if !self.agent_turn_changed() { return false; }
        // The check is a command the agent's turn sets off, so it needs the
        // permission to run code; it cannot stop to ask at the end of a turn.
        if self.cfg.project(&self.root).run != crate::config::Policy::Allow {
            let keys = self.keys_of("settings");
            self.ai.thread.items.push(Item::Note(format!("The project check did not run: it needs \"Agent may run code\" set to allow for this project ({keys}).")));
            return false;
        }
        if let Some(why) = self.over_limit() {
            self.ai.thread.items.push(Item::Note(format!("The project check did not run. {why}")));
            return false;
        }
        if let Err(why) = self.checkpoint_command() {
            self.ai.thread.items.push(Item::Note(format!("The project check did not run: {why}")));
            return false;
        }
        let attempt = self.ai.checks.sent + 1;
        let total = self.cfg.check_attempts_for(&self.root) + 1;
        self.ai.thread.items.push(Item::Note(format!("Running the project check ({attempt} of at most {total}): {command}")));
        let job = Job::start(self.project_command(&command, 900), self.waker.clone());
        self.ai.checks.running = Some((job, command));
        true
    }

    /// Takes a finished check: the turn ends, or the failure goes back to
    /// the model for another round.
    pub(super) fn agent_check_poll(&mut self) {
        let Some((out, ok)) = self.ai.checks.running.as_ref().and_then(|(job, _)| job.poll()) else { return };
        self.agent_check_done(out, ok);
    }
    /// Waits for a running check. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn agent_check_settle(&mut self) {
        let done = self.ai.checks.running.as_ref().map(|(job, _)| job.wait());
        if let Some(done) = done {
            let (out, ok) = done.unwrap_or(("Timed out".into(), false));
            self.agent_check_done(out, ok);
        }
    }
    pub(super) fn agent_check_done(&mut self, out: String, ok: bool) {
        let Some((_, command)) = self.ai.checks.running.take() else { return };
        self.refresh_agent_changes();
        let output = crate::nb::clip(out.trim(), CHECK_OUTPUT);
        let limit = self.cfg.check_attempts_for(&self.root);
        if ok {
            self.ai.thread.items.push(Item::Note(format!("The project check passed: {command}")));
        } else if self.ai.checks.sent < limit && self.over_limit().is_none() {
            self.ai.checks.sent += 1;
            self.ai.thread.items.push(Item::Note(format!("The project check failed; its output goes back to the model ({} of {limit}).", self.ai.checks.sent)));
            let text = format!("The project check `{command}` failed after your changes. Fix what it reports, then finish.\n\n```\n{output}\n```");
            self.ai.thread.items.push(Item::User { text, context: String::new(), images: Vec::new() });
            self.ai.thread.save(&self.root);
            self.show_thread();
            return self.request();
        } else {
            let why = self.over_limit().map_or(format!("after {limit} attempts"), |why| format!("and no more rounds run. {why}"));
            self.ai.thread.items.push(Item::Note(format!("The project check still fails {why}\n{output}")));
        }
        self.finish_agent_turn();
        self.ai.thread.save(&self.root);
        self.show_thread();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Call, Wire};
    use serde_json::{Value, json};

    fn app(name: &str, settings: Value) -> (PathBuf, App) {
        let root = std::env::temp_dir().join(format!("ld-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join(".lightdown/settings.json"), settings.to_string()).unwrap();
        std::fs::write(root.join("notes.md"), "draft\n").unwrap();
        let mut a = App::new(Some(root.join("notes.md")));
        a.resize(1000, 700, 1.0);
        a.cfg.load_project(&root).unwrap();
        a.cfg.project_mut(&root).edit = crate::config::Policy::Allow;
        a.cfg.project_mut(&root).run = crate::config::Policy::Allow;
        (root, a)
    }
    /// A user message starting a turn, as `send` does. No key is set, so
    /// no request leaves; replies are played with `say`.
    fn user(a: &mut App, text: &str) {
        a.ai.checks.reset();
        a.begin_agent_turn();
        a.ai.thread.items.push(Item::User { text: text.into(), context: String::new(), images: Vec::new() });
    }
    /// Plays one model reply, then waits for its tools and any check.
    fn say(a: &mut App, text: &str, calls: &[(&str, Value)]) {
        a.ai.thread.items.push(Item::Assistant { text: String::new(), reasoning: String::new(), calls: Vec::new(), model: "go/test".into(), blocks: Value::Null });
        a.wire(Wire::Text(text.into()));
        for (i, (name, args)) in calls.iter().enumerate() {
            a.wire(Wire::Call(Call { id: format!("{}-{i}", a.ai.thread.items.len()), name: name.to_string(), args: args.to_string() }));
        }
        a.wire(Wire::Done);
        a.assist_settle();
    }
    fn notes(a: &App) -> Vec<String> {
        a.ai.thread.items.iter().filter_map(|i| if let Item::Note(n) = i { Some(n.clone()) } else { None }).collect()
    }
    fn write(path: &str, content: &str) -> (&'static str, Value) { ("write_file", json!({"path":path,"content":content})) }

    #[test]
    fn failing_check_goes_back_to_the_model_until_it_passes() {
        let check = "grep -qx ok fixed.txt || { echo 'check: fixed.txt is not ok'; exit 1; }";
        let (root, mut a) = app("retry", json!({"check": check}));
        user(&mut a, "make the file");
        say(&mut a, "", &[write("fixed.txt", "broken")]);
        say(&mut a, "Done.", &[]);
        assert!(notes(&a).iter().any(|n| n.starts_with("The project check failed")), "{:?}", notes(&a));
        let sent_back = a.ai.thread.items.iter().rev().find_map(|i| if let Item::User { text, .. } = i { Some(text.clone()) } else { None }).unwrap();
        assert!(sent_back.contains("check: fixed.txt is not ok"), "{sent_back}");
        say(&mut a, "", &[write("fixed.txt", "ok")]);
        say(&mut a, "Fixed.", &[]);
        assert!(notes(&a).last().unwrap().starts_with("The project check passed"), "{:?}", notes(&a));
        assert!(!a.ai.busy());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn check_stops_after_its_attempts_and_skips_turns_without_changes() {
        let (root, mut a) = app("limit", json!({"check": ["false"], "check_attempts": 1}));
        user(&mut a, "write");
        say(&mut a, "", &[write("a.txt", "x")]);
        say(&mut a, "Done.", &[]);
        say(&mut a, "Still done.", &[]);
        let all = notes(&a);
        assert_eq!(all.iter().filter(|n| n.starts_with("Running the project check")).count(), 2, "{all:?}");
        assert!(all.last().unwrap().starts_with("The project check still fails after 1 attempts"), "{all:?}");
        user(&mut a, "look only");
        say(&mut a, "Nothing to change.", &[]);
        assert_eq!(notes(&a).len(), all.len(), "no files changed, so no check");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_cancels_a_running_check_and_the_setting_turns_it_off() {
        let (root, mut a) = app("stop", json!({"check": "sleep 30"}));
        user(&mut a, "write");
        say(&mut a, "", &[write("a.txt", "x")]);
        a.ai.thread.items.push(Item::Assistant { text: String::new(), reasoning: String::new(), calls: Vec::new(), model: "go/test".into(), blocks: Value::Null });
        a.wire(Wire::Text("Done.".into()));
        a.wire(Wire::Done);
        assert!(a.ai.checks.busy() && a.ai.busy());
        let at = std::time::Instant::now();
        a.stop();
        assert!(!a.ai.busy() && at.elapsed().as_secs() < 2);
        a.cfg.agent_check = false;
        user(&mut a, "write again");
        say(&mut a, "", &[write("b.txt", "x")]);
        say(&mut a, "Done.", &[]);
        assert_eq!(notes(&a).iter().filter(|n| n.starts_with("Running the project check")).count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn check_needs_permission_to_run_code() {
        let (root, mut a) = app("permission", json!({"check": "touch ran"}));
        a.cfg.project_mut(&root).run = crate::config::Policy::Ask;
        user(&mut a, "write");
        say(&mut a, "", &[write("a.txt", "x")]);
        say(&mut a, "Done.", &[]);
        assert!(notes(&a).last().unwrap().contains("needs \"Agent may run code\""), "{:?}", notes(&a));
        assert!(!root.join("ran").exists() && !a.ai.busy());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn plan_mode_skips_the_check() {
        let (root, mut a) = app("plan", json!({"check": "false"}));
        user(&mut a, "write");
        say(&mut a, "", &[write("a.txt", "x")]);
        a.ai.plan_mode = true;
        say(&mut a, "A plan.", &[]);
        assert!(!notes(&a).iter().any(|n| n.contains("project check")));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn run_tests_and_problems_answer_in_structure() {
        let (root, mut a) = app("tests", json!({}));
        assert_eq!(crate::tools::class("run_tests"), crate::tools::Class::Run, "needs command permission, refused in plan mode");
        assert_eq!(crate::tools::class("problems"), crate::tools::Class::Read);
        // LD_PYTEST_PYTHON names an interpreter with pytest, when the default lacks it.
        if let Ok(python) = std::env::var("LD_PYTEST_PYTHON") { a.cfg.project_mut(&root).python = python; }
        std::fs::write(root.join("test_sample.py"), "def test_ok():\n    assert True\n\ndef test_bad():\n    assert 1 == 2\n").unwrap();
        user(&mut a, "test");
        say(&mut a, "", &[("problems", json!({})), ("run_tests", json!({"file":"test_sample.py"}))]);
        let results: Vec<String> = a.ai.thread.items.iter().filter_map(|i| if let Item::Tool { result, .. } = i { Some(result.clone()) } else { None }).collect();
        let problems: Value = serde_json::from_str(&results[0]).unwrap();
        assert!(problems["problems"].is_array());
        let tests: Value = serde_json::from_str(&results[1]).unwrap();
        let has_pytest = std::process::Command::new(a.cfg.python_for(&root)).args(["-c", "import pytest"]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        if has_pytest {
            assert_eq!((tests["passed"].as_u64(), tests["failed"].as_u64()), (Some(1), Some(1)), "{tests}");
            assert!(tests["tests"][0]["message"].as_str().unwrap().contains("assert 1 == 2"), "{tests}");
        } else {
            assert!(tests["output"].as_str().unwrap().contains("pytest"), "{tests}");
        }
        a.ai.plan_mode = true;
        say(&mut a, "", &[("run_tests", json!({}))]);
        let last = a.ai.thread.items.iter().rev().find_map(|i| if let Item::Tool { result, .. } = i { Some(result.clone()) } else { None }).unwrap();
        assert!(last.contains("Plan mode"), "{last}");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn test_commands_follow_the_project() {
        let (root, a) = app("commands", json!({}));
        assert!(a.agent_test_command("", "").unwrap().contains("-m pytest -v -rfE"));
        std::fs::write(root.join("test_x.py"), "").unwrap();
        assert!(a.agent_test_command("test_y", "test_x.py").unwrap().contains("test_x.py::test_y"));
        assert!(a.agent_test_command("", "../outside.py").is_err());
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("tests/api.rs"), "").unwrap();
        assert_eq!(a.agent_test_command("a::b", "").unwrap(), "cargo test -- 'a::b' --color=never");
        assert_eq!(a.agent_test_command("", "tests/api.rs").unwrap(), "cargo test --test 'api' -- --color=never");
        assert!(a.agent_test_command("", "notes.md").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
