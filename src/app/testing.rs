//! Tests: discovery and runs for every runner a project uses, one job at a
//! time on a background worker, with gutter marks, failure notes, Problems
//! and comparisons of expected and actual values.
use super::test_runners::{self as runners, Kind, Outcome, Runner};
pub(super) use super::test_runners::Status;
use super::*;
use crate::tools::Job;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

/// The most a run keeps of what its runner prints, report included.
const OUTPUT_LIMIT: usize = 32 << 20;
/// The most of the log kept for Tests: Show output.
const LOG_LIMIT: usize = 256 << 10;
const BEGIN: &str = "@@LD-TEST-REPORT-BEGIN@@";
const END: &str = "@@LD-TEST-REPORT-END@@";

#[derive(Clone, Debug)]
pub(super) struct Test {
    /// Which of `Tests::runners` owns it.
    pub runner: usize,
    pub id: String,
    /// The file, module or package it is listed under.
    pub group: String,
    pub path: Option<PathBuf>,
    /// The declaration's line, from 0.
    pub line: Option<usize>,
    /// It stands for a Jest file whose tests are known once it runs.
    pub whole_file: bool,
    pub status: Status,
    pub duration: Option<f64>,
    pub message: Option<String>,
    /// Where it failed, with the line from 0.
    pub at: Option<(PathBuf, usize)>,
    pub values: Option<(String, String)>,
    /// The file's text when it last ran, so a changed file drops its marks.
    source: Option<String>,
}

impl Test {
    pub(super) fn new(runner: usize, f: runners::Found) -> Test {
        Test {
            runner,
            id: f.id,
            group: f.group,
            path: f.path,
            line: f.line,
            whole_file: f.whole_file,
            status: Status::Idle,
            duration: None,
            message: None,
            at: None,
            values: None,
            source: None,
        }
    }
    /// The name shown in the tree: the ID without its group.
    pub fn name(&self) -> &str {
        if self.whole_file {
            return "All tests in the file";
        }
        self.id.strip_prefix(self.group.as_str()).and_then(|s| s.strip_prefix("::")).filter(|s| !s.is_empty()).unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug)]
struct Step {
    runner: usize,
    discover: bool,
    coverage: bool,
    /// Every test of the runner, so the command needs no filter.
    all: bool,
    ids: Vec<String>,
}

struct Running {
    job: Job,
    step: Step,
    root: PathBuf,
    command: String,
}

#[derive(Default)]
pub(super) struct Tests {
    pub(super) root: PathBuf,
    pub(super) runners: Vec<Runner>,
    pub(super) items: Vec<Test>,
    running: Option<Running>,
    queue: VecDeque<Step>,
    pub(super) output: String,
    /// When the current run started, for its total time.
    started: Option<std::time::Instant>,
    pub(super) elapsed: Option<f64>,
    // The panel.
    pub(super) rect: (f32, f32, f32, f32),
    pub(super) rows: Vec<((f32, f32, f32, f32), super::tests_panel::Press)>,
    pub(super) scroll: f32,
    pub(super) height: f32,
    pub(super) filter: String,
    pub(super) typing: bool,
    pub(super) collapsed: HashSet<String>,
    pub(super) selected: Option<usize>,
}

impl Tests {
    pub(super) fn busy(&self) -> bool {
        self.running.is_some()
    }
    pub(super) fn discovering(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.step.discover) || self.queue.iter().any(|s| s.discover)
    }
    pub(super) fn counts(&self) -> [usize; 3] {
        let n = |s: Status| self.items.iter().filter(|t| t.status == s && !t.whole_file).count();
        [n(Status::Passed), n(Status::Failed), n(Status::Skipped)]
    }
}

/// Wraps a runner's command so its structured report follows its log.
fn wrap(command: &str) -> String {
    format!(
        "ld_report=$(mktemp) || exit 1\ntrap 'rm -f \"$ld_report\"' EXIT\n{command}\nld_exit=$?\nprintf '\\n%s\\n' '{BEGIN}'\nhead -c 33554432 \"$ld_report\"\nprintf '\\n%s\\n' '{END}'\nexit $ld_exit"
    )
}

/// The log and the report of a wrapped command's output.
fn split(out: &str) -> (String, String) {
    match out.rfind(&format!("\n{BEGIN}\n")) {
        Some(at) => {
            let rest = &out[at + BEGIN.len() + 2..];
            let report = rest.rfind(&format!("\n{END}")).map_or(rest, |e| &rest[..e]);
            let after = rest.rfind(&format!("\n{END}")).map_or("", |e| &rest[e + END.len() + 1..]);
            (format!("{}{}", &out[..at], after.trim_end()), report.to_string())
        }
        None => (out.to_string(), String::new()),
    }
}

fn keep_tail(s: &mut String, max: usize) {
    if s.len() > max {
        let mut from = s.len() - max;
        while !s.is_char_boundary(from) {
            from += 1;
        }
        s.replace_range(..from, "…\n");
    }
}

/// Applies a run's outcomes to the tests. Returns how many it changed.
fn apply(items: &mut Vec<Test>, step: &Step, root: &Path, outcomes: Vec<Outcome>, ok: bool) {
    let r = step.runner;
    for o in outcomes {
        let pos = items.iter().position(|t| t.runner == r && t.id == o.id);
        let i = match pos {
            Some(i) => i,
            // A Go package that did not build: its tests that ran all failed.
            None if o.id.ends_with("::") && o.file.is_none() => {
                for t in items.iter_mut().filter(|t| t.runner == r && t.id.starts_with(&o.id) && t.status == Status::Running) {
                    t.status = Status::Failed;
                    t.message = o.message.clone();
                }
                continue;
            }
            // Jest finds a file's tests as it runs them.
            None if o.file.is_some() => {
                let path = o.file.clone().unwrap();
                let group = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                let mut t = Test::new(r, runners::Found { id: o.id.clone(), group: group.clone(), path: Some(path), line: o.line, whole_file: false });
                t.source = items.iter().find(|t| t.runner == r && t.group == group).and_then(|t| t.source.clone());
                let after = items.iter().rposition(|t| t.runner == r && t.group == group).map_or(items.len(), |i| i + 1);
                items.insert(after, t);
                items.retain(|t| !(t.runner == r && t.whole_file && t.id == group));
                match items.iter().position(|t| t.runner == r && t.id == o.id) {
                    Some(i) => i,
                    None => continue,
                }
            }
            None => continue,
        };
        let t = &mut items[i];
        t.status = o.status.unwrap_or(if ok { Status::Skipped } else { Status::Failed });
        t.duration = o.duration;
        t.message = o.message;
        t.at = o.at;
        t.values = o.values;
        if o.line.is_some() {
            t.line = o.line;
        }
    }
    // Tests the runner said nothing of.
    for t in items.iter_mut().filter(|t| t.runner == r && t.status == Status::Running && step.ids.contains(&t.id)) {
        if ok {
            t.status = Status::Skipped;
        } else {
            t.status = Status::Failed;
            t.message = Some("No result was reported; see Tests: Show output".into());
        }
    }
}

impl App {
    pub(super) fn test_problems(&self) -> Vec<super::problems::Problem> {
        if self.tests.root != self.root {
            return Vec::new();
        }
        self.tests
            .items
            .iter()
            .filter(|t| t.status == Status::Failed)
            .map(|t| {
                let (path, line) = match &t.at {
                    Some((p, l)) => (Some(p.clone()), l + 1),
                    None => (t.path.clone(), t.line.map_or(1, |l| l + 1)),
                };
                let message = match &t.message {
                    Some(m) => format!("{}: {m}", t.id),
                    None => t.id.clone(),
                };
                super::problems::Problem { path, line, col: 1, message, source: "Test" }
            })
            .collect()
    }

    /// Finds the project's runners and lists their tests. `force` adds a
    /// runner the project does not declare, as the explicit commands do.
    pub(super) fn tests_refresh(&mut self, force: Option<Kind>) {
        if self.tests.busy() {
            self.message = Some("Tests are running. Stop them first.".into());
            return;
        }
        let python = self.cfg.python_for(&self.root);
        let mut found = runners::detect(&self.root, &python, &runners::on_path);
        if let Some(kind) = force {
            if kind == Kind::Cargo {
                found.retain(|r| r.kind != Kind::Nextest);
            }
            if !found.iter().any(|r| r.kind == kind) {
                found.push(Runner { kind, launch: if kind == Kind::Pytest { python.clone() } else { String::new() } });
            }
        }
        if found.is_empty() {
            self.message = Some("No test runner found: cargo, pytest, Jest, Vitest, go test or CTest".into());
            return;
        }
        let old = std::mem::take(&mut self.tests);
        self.tests = Tests {
            root: self.root.clone(),
            filter: old.filter,
            collapsed: old.collapsed,
            rect: old.rect,
            ..Default::default()
        };
        for i in 0..found.len() {
            self.tests.queue.push_back(Step { runner: i, discover: true, coverage: false, all: true, ids: Vec::new() });
        }
        let names: Vec<&str> = found.iter().map(|r| r.kind.name()).collect();
        self.message = Some(format!("Discovering tests: {}…", names.join(", ")));
        self.tests.runners = found;
        self.tests_next();
    }

    pub(super) fn tests_discover(&mut self, kind: &str) {
        let kind = match kind {
            "cargo" => Kind::Cargo,
            _ => Kind::Pytest,
        };
        self.tests_refresh(Some(kind));
    }

    /// Shows the Tests tab, and lists the project's tests the first time.
    pub(super) fn open_tests(&mut self) {
        self.show_side(Some(Tab::Tests));
        if self.tests.root != self.root || (self.tests.items.is_empty() && !self.tests.busy()) {
            self.tests_refresh(None);
        }
    }

    /// A searchable list of the tests and what can be done with them.
    pub(super) fn tests_filter(&mut self) {
        if self.tests.root != self.root || self.tests.items.is_empty() {
            self.open_tests();
            return;
        }
        let mut entries = vec![
            Entry::new("all", "Run all tests", ""),
            Entry::new("failed", "Rerun failed tests", ""),
            Entry::new("coverage", "Run all with coverage", ""),
            Entry::new("stop", "Stop tests", ""),
            Entry::new("output", "Show test output", ""),
        ];
        let many = self.tests.runners.len() > 1;
        entries.extend(self.tests.items.iter().enumerate().map(|(i, t)| {
            let state = format!("{:?}", t.status);
            let note = if many { format!("{} · {state}", self.tests.runners[t.runner].kind.name()) } else { state };
            Entry::new(&i.to_string(), &t.id, &note)
        }));
        self.menu = Some((MenuKind::Tests, Menu::new("Tests", entries, "")));
    }

    pub(super) fn test_pick(&mut self, key: &str) {
        if let Some(i) = key.strip_prefix("compare:").and_then(|i| i.parse().ok()) {
            return self.tests_compare_one(i);
        }
        if let Some(i) = key.strip_prefix("debug:").and_then(|i| i.parse().ok()) {
            return self.tests_debug(i);
        }
        let selected: Vec<usize> = match key {
            "stop" => return self.tests_stop(),
            "output" => return self.open_output(self.tests.output.clone()),
            "coverage" => return self.tests_coverage(),
            "all" => (0..self.tests.items.len()).collect(),
            "failed" => (0..self.tests.items.len()).filter(|&i| self.tests.items[i].status == Status::Failed).collect(),
            _ => match key.parse::<usize>() {
                Ok(i) if i < self.tests.items.len() => vec![i],
                _ => Vec::new(),
            },
        };
        if key == "failed" && selected.is_empty() {
            self.message = Some("No failed tests".into());
            return;
        }
        self.tests_run(selected);
    }

    /// Why tests cannot run now, if they cannot.
    fn tests_blocked(&self) -> Option<&'static str> {
        if self.tests.root != self.root {
            return Some("Discover tests in this project first");
        }
        if self.tests.busy() {
            return Some("Tests are already running");
        }
        let unsaved = self.file_tabs.iter().enumerate().any(|(i, t)| {
            if i == self.active_file {
                self.doc.buf.modified
            } else {
                t.state.as_ref().is_some_and(|s| s.root == self.root && s.pane.buf.modified)
            }
        });
        unsaved.then_some("Save project edits before running tests")
    }

    /// Runs these tests, one runner after another.
    pub(super) fn tests_run(&mut self, selected: Vec<usize>) {
        if let Some(why) = self.tests_blocked() {
            self.message = Some(why.into());
            return;
        }
        if selected.is_empty() {
            self.message = Some("No matching tests".into());
            return;
        }
        for r in 0..self.tests.runners.len() {
            let ids: Vec<String> = selected.iter().map(|&i| &self.tests.items[i]).filter(|t| t.runner == r).map(|t| t.id.clone()).collect();
            if ids.is_empty() {
                continue;
            }
            let all = ids.len() == self.tests.items.iter().filter(|t| t.runner == r).count();
            self.tests.queue.push_back(Step { runner: r, discover: false, coverage: false, all, ids });
        }
        self.tests_start(&selected);
    }

    /// Runs every runner that has a coverage tool under it.
    fn tests_coverage(&mut self) {
        if let Some(why) = self.tests_blocked() {
            self.message = Some(why.into());
            return;
        }
        let mut selected = Vec::new();
        for (r, runner) in self.tests.runners.iter().enumerate() {
            if runners::coverage_command(runner).is_none() {
                continue;
            }
            let ids: Vec<String> = self.tests.items.iter().filter(|t| t.runner == r).map(|t| t.id.clone()).collect();
            selected.extend(self.tests.items.iter().enumerate().filter(|(_, t)| t.runner == r).map(|(i, _)| i));
            self.tests.queue.push_back(Step { runner: r, discover: false, coverage: true, all: true, ids });
        }
        if self.tests.queue.is_empty() {
            self.message = Some("Coverage runs support cargo, cargo nextest and pytest".into());
            return;
        }
        self.tests_start(&selected);
    }

    fn tests_start(&mut self, selected: &[usize]) {
        for &i in selected {
            let doc_text = self.doc.buf.text.clone();
            let t = &mut self.tests.items[i];
            t.status = Status::Running;
            t.source = t.path.as_ref().and_then(|p| {
                if self.doc.path.as_ref() == Some(p) { Some(doc_text.clone()) } else { std::fs::read_to_string(p).ok().map(|s| s.replace("\r\n", "\n")) }
            });
        }
        self.tests.output.clear();
        self.tests.started = Some(std::time::Instant::now());
        self.tests.elapsed = None;
        self.message = Some("Running tests…".into());
        self.tests_next();
        self.tests_marks();
    }

    /// Starts the next queued step.
    fn tests_next(&mut self) {
        let Some(step) = self.tests.queue.pop_front() else { return };
        let runner = &self.tests.runners[step.runner];
        let command = if step.discover {
            runners::discover_command(runner)
        } else if step.coverage {
            runners::coverage_command(runner).unwrap_or_default()
        } else {
            runners::run_command(runner, &step.ids, step.all)
        };
        let timeout = if step.discover { 300 } else { 3600 };
        let job = Job::start_limited(self.project_command(&wrap(&command), timeout), self.waker.clone(), OUTPUT_LIMIT);
        self.tests.running = Some(Running { job, step, root: self.root.clone(), command });
    }

    pub(super) fn tests_poll(&mut self) {
        let Some((out, ok)) = self.tests.running.as_ref().and_then(|r| r.job.poll()) else {
            return;
        };
        let run = self.tests.running.take().unwrap();
        let (log, report) = split(&out);
        self.tests.output.push_str(&format!("$ {}\n{}\n\n", run.command, log.trim_end()));
        keep_tail(&mut self.tests.output, LOG_LIMIT);
        let step = run.step;
        let runner = self.tests.runners[step.runner].clone();
        if step.discover {
            let found = runners::parse_discovery(&runner, &run.root, &log, &report);
            let mut tests: Vec<Test> = found.into_iter().map(|f| Test::new(step.runner, f)).collect();
            if matches!(runner.kind, Kind::Cargo | Kind::Nextest) {
                let ids: Vec<String> = tests.iter().map(|t| t.id.clone()).collect();
                let places = runners::locate_rust(&run.root, &search::files(&run.root), &ids);
                for t in &mut tests {
                    if let Some((p, l)) = places.get(&t.id) {
                        (t.path, t.line) = (Some(p.clone()), Some(*l));
                    }
                }
            }
            if tests.is_empty() && !ok {
                self.message = Some(format!("{} could not list tests; see Tests: Show output", runner.kind.name()));
            }
            self.tests.items.retain(|t| t.runner != step.runner);
            self.tests.items.extend(tests);
            self.tests.items.sort_by_key(|t| t.runner);
        } else {
            let outcomes = runners::parse_results(&runner, &run.root, &step.ids, &log, &report);
            apply(&mut self.tests.items, &step, &run.root, outcomes, ok);
        }
        if self.tests.queue.is_empty() {
            self.tests.elapsed = self.tests.started.take().map(|s| s.elapsed().as_secs_f64());
            if self.root == run.root {
                let [passed, failed, skipped] = self.tests.counts();
                self.message = Some(if step.discover {
                    format!("Discovered {} tests", self.tests.items.iter().filter(|t| !t.whole_file).count())
                } else {
                    let mut parts = vec![format!("{passed} passed")];
                    if failed > 0 {
                        parts.push(format!("{failed} failed"));
                    }
                    if skipped > 0 {
                        parts.push(format!("{skipped} skipped"));
                    }
                    format!("Tests: {}", parts.join(", "))
                });
            }
        } else {
            self.tests_next();
        }
        self.tests_marks();
        self.redraw = true;
    }

    /// Waits for test jobs, up to two minutes. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn tests_settle(&mut self) {
        let until = std::time::Instant::now();
        while self.tests.busy() && until.elapsed().as_secs() < 120 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            self.tests_poll();
        }
    }

    /// The gutter's play marks and the notes beside failed lines.
    pub(super) fn tests_marks(&mut self) {
        self.doc.test_marks.clear();
        self.doc.test_notes.clear();
        if self.root != self.tests.root || self.doc.path.is_none() {
            return;
        }
        let stale = |t: &Test, doc: &Pane| doc.buf.modified || t.source.as_ref().is_some_and(|s| s != &doc.buf.text);
        for (i, t) in self.tests.items.iter().enumerate() {
            let fresh = !stale(t, &self.doc);
            if let Some(line) = t.line.filter(|_| t.path.is_some() && t.path == self.doc.path) {
                let status = if fresh { t.status } else { Status::Idle };
                let label = match status {
                    Status::Idle => "▶",
                    Status::Running => "…",
                    Status::Passed => "✓",
                    Status::Failed => "×",
                    Status::Skipped => "−",
                };
                if !self.doc.test_marks.iter().any(|m| m.0 == line) {
                    self.doc.test_marks.push((line, i, label.into()));
                }
            }
            if t.status == Status::Failed
                && fresh
                && let Some((path, line)) = &t.at
                && self.doc.path.as_ref() == Some(path)
                && !self.doc.test_notes.iter().any(|n| n.0 == *line)
            {
                let message = t.message.as_deref().unwrap_or("Failed");
                let message = message.lines().next().unwrap_or("Failed");
                self.doc.test_notes.push((*line, message.chars().take(300).collect()));
            }
        }
        self.doc.test_notes.sort_by_key(|n| n.0);
    }

    pub(super) fn tests_stop(&mut self) {
        self.tests.queue.clear();
        if let Some(run) = self.tests.running.take() {
            run.job.stop();
            for t in self.tests.items.iter_mut().filter(|t| t.status == Status::Running) {
                t.status = Status::Idle;
            }
            self.tests.started = None;
            self.tests_marks();
            self.message = Some("Tests stopped".into());
        }
    }

    /// Opens a test's declaration, or where it failed.
    pub(super) fn tests_reveal(&mut self, i: usize, failure: bool) {
        let Some(t) = self.tests.items.get(i) else { return };
        let place = if failure { t.at.clone() } else { None };
        let Some((path, line)) = place.or_else(|| Some((t.path.clone()?, t.line.unwrap_or(0)))) else {
            self.message = Some(format!("{} has no known source file", t.id));
            return;
        };
        self.remember_jump();
        self.jump_to(path.clone(), 0, 0);
        if self.doc.path.as_ref() == Some(&path) && self.reader.is_none() {
            let (a, _) = self.doc.buf.line(line.min(self.doc.buf.lines().saturating_sub(1)));
            self.doc.select(a, a);
            self.doc.moved();
        }
    }

    /// Debugs one test, where its runner allows it.
    pub(super) fn tests_debug(&mut self, i: usize) {
        let Some(t) = self.tests.items.get(i) else { return };
        let kind = self.tests.runners[t.runner].kind;
        if !runners::debuggable(kind) {
            self.message = Some(format!("Debugging {} tests is not supported; pytest tests can be debugged", kind.name()));
            return;
        }
        let id = t.id.clone();
        self.tests_reveal(i, false);
        self.debugger.options["pytest"] = serde_json::json!(id);
        self.debug_command("start_pytest");
    }

    /// The test the tests' commands act on: chosen in the panel, or
    /// declared or failed at the cursor.
    fn tests_at_cursor(&self) -> Vec<usize> {
        let line = self.doc.buf.line_of(self.doc.buf.cur);
        let here = |p: &Option<PathBuf>| p.is_some() && p.as_ref() == self.doc.path.as_ref();
        (0..self.tests.items.len())
            .filter(|&i| {
                let t = &self.tests.items[i];
                (here(&t.path) && t.line == Some(line)) || t.at.as_ref().is_some_and(|(p, l)| *l == line && Some(p) == self.doc.path.as_ref())
            })
            .collect()
    }

    /// Opens the side-by-side diff of a failed test's expected and actual
    /// values.
    pub(super) fn tests_compare(&mut self) {
        let with = |i: &usize| self.tests.items[*i].values.is_some();
        let chosen = self.tests.selected.filter(with).or_else(|| self.tests_at_cursor().into_iter().find(with));
        if let Some(i) = chosen {
            return self.tests_compare_one(i);
        }
        let all: Vec<usize> = (0..self.tests.items.len()).filter(|i| with(i) && self.tests.items[*i].status == Status::Failed).collect();
        match all.len() {
            0 => self.message = Some("No failed test reported expected and actual values".into()),
            1 => self.tests_compare_one(all[0]),
            _ => {
                let entries = all.iter().map(|&i| Entry::new(&format!("compare:{i}"), &self.tests.items[i].id, "")).collect();
                self.menu = Some((MenuKind::Tests, Menu::new("Compare expected and actual", entries, "")));
            }
        }
    }

    fn tests_compare_one(&mut self, i: usize) {
        let Some(t) = self.tests.items.get(i) else { return };
        let Some((expected, actual)) = t.values.clone() else {
            self.message = Some(format!("{} reported no expected and actual values", t.id));
            return;
        };
        let (a, b) = self.tests.runners[t.runner].kind.value_names();
        let name = format!("{}: {a} and {b}", t.id);
        match super::repository::diff(&format!("{expected}\n"), &format!("{actual}\n"), 10_000) {
            Ok(text) if text.trim().is_empty() => self.message = Some(format!("The {a} and {b} values print the same")),
            Ok(text) => {
                // Only the hunk: the patch's header names scratch files.
                let text = text.find("@@").map_or(text.clone(), |at| text[at..].to_string());
                self.open_output(text.clone());
                self.doc.diff = true;
                self.doc.comparison = Some(crate::diff::View::parse(&text));
                self.doc.output_name = Some(name);
                self.focus = Focus::Doc;
            }
            Err(e) => self.message = Some(e),
        }
    }

    /// Carries out a Tests command from the palette or the panel's head.
    pub(super) fn tests_command(&mut self, id: &str) {
        match id {
            "tests_list" => self.open_tests(),
            "tests_filter" => self.tests_filter(),
            "tests_filter_focus" => {
                self.open_tests();
                self.tests.typing = true;
            }
            "tests_refresh" => {
                self.show_side(Some(Tab::Tests));
                self.tests_refresh(None);
            }
            "tests_cargo" | "tests_pytest" => {
                self.show_side(Some(Tab::Tests));
                self.tests_discover(&id[6..]);
            }
            "tests_all" => self.test_pick("all"),
            "tests_failed" => self.test_pick("failed"),
            "tests_coverage" => self.test_pick("coverage"),
            "tests_output" => self.test_pick("output"),
            "tests_stop" => self.tests_stop(),
            "tests_compare" => self.tests_compare(),
            "tests_file" => {
                let here: Vec<usize> = (0..self.tests.items.len())
                    .filter(|&i| self.tests.items[i].path.is_some() && self.tests.items[i].path == self.doc.path)
                    .collect();
                if here.is_empty() {
                    self.message = Some("No discovered tests are in this file".into());
                } else {
                    self.tests_run(here);
                }
            }
            "tests_cursor" | "tests_debug" => match self.test_near_cursor() {
                Some(i) if id == "tests_debug" => self.tests_debug(i),
                Some(i) => self.tests_run(vec![i]),
                None => self.message = Some("No discovered test is declared at or above the cursor".into()),
            },
            _ => {}
        }
    }

    /// The test declared at the cursor or nearest above it in this file.
    fn test_near_cursor(&self) -> Option<usize> {
        let line = self.doc.buf.line_of(self.doc.buf.cur);
        self.tests_at_cursor().into_iter().next().or_else(|| {
            (0..self.tests.items.len())
                .filter(|&i| {
                    let t = &self.tests.items[i];
                    t.path.is_some() && t.path == self.doc.path && t.line.is_some_and(|l| l <= line)
                })
                .max_by_key(|&i| self.tests.items[i].line)
        })
    }

    /// Every discovered test: its runner, ID and state. For tools.
    #[allow(dead_code)] // For the agent's run_tests tool.
    pub(super) fn tests_listed(&self) -> Vec<(&'static str, String, Status)> {
        self.tests.items.iter().map(|t| (self.tests.runners[t.runner].kind.name(), t.id.clone(), t.status)).collect()
    }

    /// Runs the tests with these IDs in the Tests panel. The results arrive
    /// in `tests_listed` and Problems when the run ends.
    #[allow(dead_code)] // For the agent's run_tests tool.
    pub(super) fn tests_run_ids(&mut self, ids: &[String]) -> Result<(), String> {
        if let Some(why) = self.tests_blocked() {
            return Err(why.into());
        }
        let selected: Vec<usize> = (0..self.tests.items.len()).filter(|&i| ids.contains(&self.tests.items[i].id)).collect();
        if selected.is_empty() {
            return Err("No discovered test has these IDs".into());
        }
        self.tests_run(selected);
        Ok(())
    }

    /// Prepares a blocking run of the tests with these IDs, for a worker
    /// thread: `TestPlan::run` returns each test's result. Tests must be
    /// discovered and project files saved.
    #[allow(dead_code)] // For the agent's run_tests tool.
    pub(super) fn tests_plan(&self, ids: &[String]) -> Result<TestPlan, String> {
        if let Some(why) = self.tests_blocked().filter(|w| !w.contains("already")) {
            return Err(why.into());
        }
        let mut steps = Vec::new();
        for (r, runner) in self.tests.runners.iter().enumerate() {
            let chosen: Vec<String> = self.tests.items.iter().filter(|t| t.runner == r && ids.contains(&t.id)).map(|t| t.id.clone()).collect();
            if chosen.is_empty() {
                continue;
            }
            let command = self.project_command(&wrap(&runners::run_command(runner, &chosen, false)), 3600);
            steps.push((runner.clone(), chosen, command));
        }
        if steps.is_empty() {
            return Err("No discovered test has these IDs".into());
        }
        Ok(TestPlan { root: self.root.clone(), steps })
    }
}

/// A blocking run of chosen tests; see `App::tests_plan`.
#[allow(dead_code)] // For the agent's run_tests tool.
pub(super) struct TestPlan {
    root: PathBuf,
    steps: Vec<(Runner, Vec<String>, std::process::Command)>,
}

/// One test's result from a `TestPlan`.
#[derive(Clone, Debug)]
#[allow(dead_code)] // For the agent's run_tests tool.
pub(super) struct TestResult {
    pub runner: &'static str,
    pub id: String,
    pub status: Status,
    pub duration: Option<f64>,
    pub message: Option<String>,
    /// Where it failed, with the line from 1.
    pub at: Option<(PathBuf, usize)>,
    pub values: Option<(String, String)>,
}

#[allow(dead_code)] // For the agent's run_tests tool.
impl TestPlan {
    /// Runs each runner's tests in turn and stops early when `cancel` is set.
    pub(super) fn run(self, cancel: &AtomicBool) -> Vec<TestResult> {
        let mut out = Vec::new();
        for (runner, ids, command) in self.steps {
            let job = Job::start_limited(command, std::sync::Arc::new(|| {}), OUTPUT_LIMIT);
            let (text, ok) = loop {
                if let Some(done) = job.poll() {
                    break done;
                }
                if cancel.load(Ordering::SeqCst) {
                    job.stop();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            };
            let (log, report) = split(&text);
            let mut items: Vec<Test> = ids.iter().map(|id| {
                let mut t = Test::new(0, runners::Found { id: id.clone(), group: String::new(), path: None, line: None, whole_file: false });
                t.status = Status::Running;
                t
            }).collect();
            let step = Step { runner: 0, discover: false, coverage: false, all: false, ids };
            apply(&mut items, &step, &self.root, runners::parse_results(&runner, &self.root, &step.ids, &log, &report), ok);
            out.extend(items.into_iter().map(|t| TestResult {
                runner: runner.kind.name(),
                id: t.id,
                status: t.status,
                duration: t.duration,
                message: t.message,
                at: t.at.map(|(p, l)| (p, l + 1)),
                values: t.values,
            }));
            if cancel.load(Ordering::SeqCst) {
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_are_split_from_the_log() {
        let out = format!("compiling\nlog line\n{BEGIN}\n{{\"a\":1}}\n{END}\n[exit status 1]");
        let (log, report) = split(&out);
        assert_eq!(report, "{\"a\":1}");
        assert_eq!(log, "compiling\nlog line\n[exit status 1]");
        assert_eq!(split("plain").0, "plain");
    }
    #[test]
    fn names_drop_their_group() {
        let t = |id: &str, group: &str| Test::new(0, runners::Found { id: id.into(), group: group.into(), path: None, line: None, whole_file: false });
        assert_eq!(t("app::tests::one", "app::tests").name(), "one");
        assert_eq!(t("test_a.py::TestA::test_x[1]", "test_a.py").name(), "TestA::test_x[1]");
        assert_eq!(t("a.test.ts::suite works", "a.test.ts").name(), "suite works");
        assert_eq!(t("m/calc::TestSum", "m/calc").name(), "TestSum");
    }
}

#[cfg(test)]
mod execution_tests {
    use super::*;
    fn wait(a: &mut App) {
        let until = std::time::Instant::now();
        while a.tests.busy() && until.elapsed().as_secs() < 120 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            a.tests_poll();
        }
        assert!(!a.tests.busy(), "test job timed out");
    }
    #[test]
    fn coverage_writes_python_report_after_test_failure() {
        let root = std::env::temp_dir().join(format!("ld-coverage-runner-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let source = root.join("test_sample.py");
        std::fs::write(&source, "def test_failure(): pass\n").unwrap();
        // A deterministic module fixture checks the two-process orchestration,
        // including report generation after a failing test process.
        std::fs::write(root.join("coverage.py"), "import sys, pathlib\nif sys.argv[1] == 'run':\n print('test_sample.py::test_failure FAILED')\n sys.exit(1)\npathlib.Path('coverage.json').write_text('{}')\n").unwrap();
        let mut a = App::new(Some(source));
        a.tests.root = root.clone();
        a.tests.runners = vec![Runner { kind: Kind::Pytest, launch: a.cfg.python_for(&root) }];
        a.tests.items = vec![Test::new(0, runners::Found { id: "test_sample.py::test_failure".into(), group: "test_sample.py".into(), path: Some(root.join("test_sample.py")), line: Some(0), whole_file: false })];
        a.test_pick("coverage");
        wait(&mut a);
        assert!(root.join("coverage.json").is_file(), "{}", a.tests.output);
        assert_eq!(a.tests.items[0].status, Status::Failed);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cargo_tests_are_discovered_run_individually_and_failures_rerun() {
        let root = std::env::temp_dir().join(format!("ld-runner-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname='lightdown_test_fixture'\nversion='0.1.0'\nedition='2021'\n").unwrap();
        let file = root.join("src/lib.rs");
        std::fs::write(
            &file,
            "#[test]\nfn passes() { assert_eq!(2+2,4); }\n#[test]\nfn fails() {\n    assert_eq!(vec![1, 2], vec![1, 3]);\n}\n",
        )
        .unwrap();
        let mut a = App::new(Some(file.clone()));
        a.tests_discover("cargo");
        wait(&mut a);
        assert_eq!(a.tests.items.len(), 2, "{}", a.tests.output);
        assert_eq!(a.doc.test_marks.len(), 2);
        a.test_pick("all");
        wait(&mut a);
        let failed = a.tests.items.iter().position(|t| t.id == "fails").unwrap();
        assert!(a.tests.items.iter().any(|t| t.id == "passes" && t.status == Status::Passed));
        assert_eq!(a.tests.items[failed].status, Status::Failed);
        assert_eq!(a.tests.items[failed].at, Some((file.clone(), 4)), "{}", a.tests.output);
        assert_eq!(a.tests.items[failed].values, Some(("[1, 2]".into(), "[1, 3]".into())));
        assert_eq!(a.doc.test_notes.len(), 1);
        assert!(a.test_problems().iter().any(|p| p.line == 5 && p.message.contains("left == right")));
        a.test_pick("failed");
        wait(&mut a);
        assert!(a.tests.output.contains("1 filtered out"), "{}", a.tests.output);
        assert_eq!(a.tests.items[failed].status, Status::Failed);
        a.tests.selected = Some(failed);
        a.tests_compare();
        assert!(a.doc.comparison.is_some());
        assert!(a.doc.buf.text.contains("[1, 3]"));
        // A blocking run for tools gives the same result.
        let plan = a.tests_plan(&["fails".into()]).unwrap();
        let results = plan.run(&AtomicBool::new(false));
        assert_eq!((results.len(), results[0].status, results[0].at.as_ref().map(|a| a.1)), (1, Status::Failed, Some(5)));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "needs cmake and ctest"]
    fn ctest_tests_are_discovered_and_run() {
        let root = std::env::temp_dir().join(format!("ld-ctest-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let lists = root.join("CMakeLists.txt");
        std::fs::write(&lists, "cmake_minimum_required(VERSION 3.20)\nproject(fixture NONE)\nenable_testing()\nadd_test(NAME passes COMMAND sh -c \"exit 0\")\nadd_test(NAME fails.one COMMAND sh -c \"echo expected 4; exit 1\")\n").unwrap();
        let ok = std::process::Command::new("cmake").args(["-S", ".", "-B", "build"]).current_dir(&root).output().unwrap().status.success();
        assert!(ok);
        let mut a = App::new(Some(lists.clone()));
        a.tests_refresh(None);
        wait(&mut a);
        assert_eq!(a.tests.runners[0].kind, Kind::Ctest);
        assert_eq!(a.tests.items.len(), 2, "{}", a.tests.output);
        assert_eq!((a.tests.items[1].path.as_ref(), a.tests.items[1].line), (Some(&lists), Some(4)));
        a.test_pick("1");
        wait(&mut a);
        assert_eq!(a.tests.items[1].status, Status::Failed, "{}", a.tests.output);
        assert_eq!(a.tests.items[1].message.as_deref(), Some("Failed: expected 4"));
        assert_eq!(a.tests.items[0].status, Status::Idle);
        a.test_pick("all");
        wait(&mut a);
        assert_eq!(a.tests.items[0].status, Status::Passed);
        assert!(a.tests.items[0].duration.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unsaved_edits_keep_tests_from_running() {
        let root = std::env::temp_dir().join(format!("ld-runner-unsaved-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let file = root.join("test_a.py");
        std::fs::write(&file, "def test_a():\n    pass\n").unwrap();
        let mut a = App::new(Some(file.clone()));
        a.tests.root = a.root.clone();
        a.tests.runners = vec![Runner { kind: Kind::Pytest, launch: "python3".into() }];
        a.tests.items = vec![Test::new(0, runners::Found { id: "test_a.py::test_a".into(), group: "test_a.py".into(), path: Some(file.clone()), line: Some(0), whole_file: false })];
        a.tests_marks();
        assert_eq!(a.doc.test_marks.len(), 1);
        a.doc.insert("# unsaved\n", EditKind::Other);
        a.tests_marks();
        assert!(a.doc.test_marks.iter().all(|(_, _, s)| s == "▶"));
        a.test_pick("all");
        assert!(!a.tests.busy());
        std::fs::remove_dir_all(root).unwrap();
    }
}
