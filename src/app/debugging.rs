//! Python stepping and inspection use bdb through a private JSON pipe.
use super::*;
use crate::rpc::Process;
use serde_json::{Value, json};

pub(super) struct Debugging {
    pub view: Pane,
    pub(super) process: Option<Process>,
    pub(super) dap: Option<super::native_debug::Session>,
    pub root: PathBuf,
    pub points: Vec<(PathBuf, usize)>,
    settings_root: PathBuf,
    pub(super) options: Value,
    point_options: Value,
    pub(super) input_action: String,
    pub(super) paused: bool,
    pub(super) snapshot: Value,
    pub(super) output: String,
    pub(super) tree: super::debug_tree::Tree,
    pub(super) hover: super::debug_hover::Hover,
}
impl Debugging {
    pub fn new(fonts: Rc<RefCell<Fonts>>, pics: Rc<RefCell<Pics>>) -> Self {
        let mut view = Pane::new(fonts, pics);
        view.set_text(
            "Python debugger\n\nUse Debug commands in the palette to start, step and inspect.\n".into(),
            Doc::Code,
        );
        view.read_only = true;
        Self {
            view,
            process: None,
            dap: None,
            root: PathBuf::new(),
            points: Vec::new(),
            settings_root: PathBuf::new(),
            options: json!({"args":[],"stdin":"","watches":[]}),
            point_options: json!({}),
            input_action: String::new(),
            paused: false,
            snapshot: Value::Null,
            output: String::new(),
            tree: Default::default(),
            hover: Default::default(),
        }
    }
    pub(super) fn points(&self) -> Value {
        self.points.iter().map(|(p, l)| {
            let key = format!("{}:{l}", p.display());
            json!({"path":p,"line":l,"condition":self.point_options[&key]["condition"],"log":self.point_options[&key]["log"]})
        }).collect()
    }
    pub(super) fn update(&mut self) {
        let mut text = format!(
            "Debugger: {}\n\n",
            if self.process.is_none() && self.dap.is_none() {
                "stopped"
            } else if self.paused {
                "paused"
            } else {
                "running"
            }
        );
        if let Some(e) = super::debug_exceptions::summary(&self.snapshot).filter(|_| self.paused) { text += &format!("Exception: {e}\n\n"); }
        text += "Palette: Continue / Step over / Step into / Step out / Stop\n";
        if self.dap.is_none() {
            let on = |k: &str| if self.options[k] == true { "on" } else { "off" };
            text += &format!("Break on raised exceptions: {}, uncaught: {}\n", on("raised"), on("uncaught"));
        }
        text += "\nBreakpoints\n";
        for (p, l) in &self.points {
            let key = format!("{}:{l}", p.display());
            text += &format!("{}:{l} {} {}\n", p.display(), self.point_options[&key]["condition"].as_str().unwrap_or(""), self.point_options[&key]["log"].as_str().unwrap_or(""));
        }
        if self.paused {
            text += &format!("\n{}\n\nCall stack\n", self.snapshot["reason"].as_str().unwrap_or(""));
            let selected = self.snapshot["selected"].as_u64().unwrap_or(0) as usize;
            for (i, f) in self.snapshot["stack"].as_array().into_iter().flatten().enumerate() {
                text += &format!(
                    "{} {i}: {} at {}:{}\n",
                    if selected == i { ">" } else { " " },
                    f["name"].as_str().unwrap_or(""),
                    f["path"].as_str().unwrap_or(""),
                    f["line"]
                );
            }
            text += "\nWatch expressions\n";
            for v in self.snapshot["watches"].as_array().into_iter().flatten() {
                text += &format!("{} = {}\n", v["expression"].as_str().unwrap_or(""), v["value"].as_str().unwrap_or(""));
            }
            text += "\nVariables in selected frame (click, or Right and Left, to open)\n";
            let line = text.matches('\n').count();
            self.tree.rows = super::debug_tree::render(&mut text, line, &self.snapshot["variables"], &self.snapshot["expanded"]);
            text += "\nThreads (Debug: Select thread by ID)\n";
            for t in self.snapshot["threads"].as_array().into_iter().flatten() {
                text += &format!("{}: {}\n", t["id"], t["name"].as_str().unwrap_or(""));
            }
        }
        if !self.paused { self.tree.rows.clear(); self.tree.focus = false; }
        text += &format!("\nOutput\n{}", self.output);
        self.view.sync(&text);
        if !self.tree.focus && self.view.buf.has_sel() { self.view.select(0, 0); }
    }
}
impl App {
    pub(super) fn debug_command(&mut self, action: &str) {
        self.debug_settings();
        if matches!(action, "raised" | "uncaught" | "inline_values") {
            // Inline values start on; the exception stops start off.
            let on = if action == "inline_values" { self.debugger.options[action] != false } else { self.debugger.options[action] == true };
            self.debugger.options[action] = json!(!on);
            if let Some(p) = &self.debugger.process { p.send(json!({"action":"exceptions","raised":self.debugger.options["raised"],"uncaught":self.debugger.options["uncaught"]})); }
            self.debug_save_settings(); self.debugger.update(); return;
        }
        if action == "exceptions" { self.debug_exceptions_menu(); return; }
        if matches!(action, "attach" | "template" | "debugpy") { self.debug_extra_command(action); return; }
        if action == "stop" && self.debug_prep_cancel() { return; }
        if action == "native" { self.native_start(); return; }
        if action == "variables" { self.debugger.update(); self.debug_tree_focus(); return; }
        if self.debugger.dap.is_some() && matches!(action, "continue" | "next" | "step" | "out" | "stop") { self.native_command(action); return; }
        if action == "show" { self.debugger.update(); self.show_side(Some(Tab::Debug)); return; }
        if matches!(action, "condition" | "logpoint" | "watch" | "evaluate" | "args" | "stdin" | "pytest" | "configuration" | "thread") {
            if matches!(action, "condition" | "logpoint") && (self.doc.path.is_none() || !matches!(self.doc.doc, Doc::Python | Doc::Code)) {
                self.message = Some("Open a saved source file first".into()); return;
            }
            if action == "evaluate" && !self.debugger.paused { self.message = Some("Pause the debugger before evaluating".into()); return; }
            self.debugger.input_action = action.into();
            self.ask(PromptKind::DebugInput);
            return;
        }
        if action == "start_pytest" { self.debugger.input_action = "pytest_run".into(); }

        if action == "breakpoint" {
            if !matches!(self.doc.doc, Doc::Python | Doc::Code) || self.reader.is_some() {
                self.message = Some("Open a source file to set a breakpoint".into());
                return;
            }
            let Some(path) = self.doc.path.clone() else {
                self.message = Some("Save the Python file first".into());
                return;
            };
            if (self.debugger.process.is_some() || self.debugger.dap.is_some()) && !self.debugger.paused {
                self.message = Some("Pause at a breakpoint or stop before changing breakpoints".into());
                return;
            }
            let point = (path, self.doc.buf.line_of(self.doc.buf.cur) + 1);
            if let Some(i) = self.debugger.points.iter().position(|p| *p == point) {
                self.debugger.points.remove(i);
            } else {
                self.debugger.points.push(point);
            }
            if let Some(p) = &self.debugger.process {
                p.send(json!({"action":"breakpoints","points":self.debugger.points()}));
            }
            if self.debugger.dap.is_some() { self.native_points(); }
            self.debug_save_settings();
        } else if action == "start" || action == "start_pytest" {
            if self.doc.doc != Doc::Python || self.nb.is_some() || self.reader.is_some() {
                self.message = Some("Open a saved Python script to debug".into());
                return;
            }
            if self.file_tabs.iter().enumerate().any(|(i, t)| {
                if i == self.active_file {
                    self.doc.buf.modified
                } else {
                    t.state.as_ref().is_some_and(|s| s.pane.buf.modified)
                }
            }) {
                self.message = Some("Save project files before debugging".into());
                return;
            }
            let Some(path) = &self.doc.path else { return };
            let python = self.cfg.python_for(&self.root);
            let mut cmd = std::process::Command::new(&python);
            cmd.args(["-u", "-c", include_str!("../../assets/debugger.py")]).current_dir(&self.root);
            crate::environment::apply(&mut cmd, &self.root, Some(&python));
            self.debugger.process = None;
            self.debugger.dap = None;
            match Process::start(cmd, self.waker.clone()) {
                Ok(p) => {
                    let pytest = action == "start_pytest";
                    let args = if pytest { json!(["-s", self.debugger.options["pytest"].as_str().unwrap_or("")]) } else { self.debugger.options["args"].clone() };
                    p.send(json!({"path":path,"points":self.debugger.points(),"args":args,"stdin":self.debugger.options["stdin"].as_str().unwrap_or(""),"watches":self.debugger.options["watches"],"root":self.root,"raised":self.debugger.options["raised"] == true,"uncaught":self.debugger.options["uncaught"] == true,"module":if pytest { "pytest" } else { "" }}));
                    self.debugger.process = Some(p);
                    self.debugger.root = self.root.clone();
                    self.debugger.output.clear();
                    self.debugger.paused = false;
                    self.debugger.snapshot = Value::Null;
                }
                Err(e) => self.message = Some(format!("Cannot start debugger: {e}")),
            }
        } else if action == "stop" {
            self.debugger.process = None;
            self.debugger.paused = false;
            self.debugger.snapshot = Value::Null;
        } else if action == "frames" {
            let entries = self.debugger.snapshot["stack"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(i, f)| {
                    Entry::new(
                        &i.to_string(),
                        f["name"].as_str().unwrap_or("frame"),
                        &format!("{}:{}", f["path"].as_str().unwrap_or(""), f["line"]),
                    )
                })
                .collect();
            self.menu = Some((MenuKind::DebugFrames, Menu::new("Inspect stack frame", entries, "")));
            return;
        } else if self.debugger.paused {
            if let Some(p) = &self.debugger.process {
                p.send(json!({"action":action}));
            }
            self.debugger.paused = false;
        } else {
            self.message = Some("Debugger must be paused to step or continue".into());
        }
        self.debugger.update();
        self.show_side(Some(Tab::Debug));
    }
    pub(super) fn debug_settings(&mut self) {
        if self.debugger.settings_root == self.root { return; }
        let all: Value = std::fs::read(config::data_dir().join("debug.json")).ok()
            .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
        let data = &all[self.root.to_string_lossy().as_ref()];
        self.debugger.points = data["points"].as_array().into_iter().flatten().filter_map(|v| {
            let p = PathBuf::from(v["path"].as_str()?); let n = v["line"].as_u64()? as usize;
            (p.starts_with(&self.root) && n > 0).then_some((p, n))
        }).collect();
        self.debugger.point_options = data["point_options"].as_object().map(|m| Value::Object(m.clone())).unwrap_or(json!({}));
        self.debugger.options = if data["options"].is_object() { data["options"].clone() } else { json!({"args":[],"stdin":"","watches":[]}) };
        self.debugger.settings_root = self.root.clone();
    }
    fn debug_save_settings(&mut self) {
        let file = config::data_dir().join("debug.json");
        let mut all: Value = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).filter(Value::is_object).unwrap_or(json!({}));
        all[self.root.to_string_lossy().as_ref()] = json!({"points":self.debugger.points(),"point_options":self.debugger.point_options,"options":self.debugger.options});
        if let Err(e) = config::write_private(&file, all.to_string().as_bytes()) { self.message = Some(format!("Cannot save debug settings: {e}")); }
    }
    pub(super) fn debug_input(&mut self, text: &str) {
        let action = self.debugger.input_action.clone();
        let result: Result<(), String> = (|| {
            match action.as_str() {
                "thread" => {
                    if !self.debugger.paused { return Err("Pause a native adapter session first".into()); }
                    let id: i64 = text.trim().parse().map_err(|_| "Enter the numeric ID shown in the Debug tab")?;
                    self.native_thread(id)?;
                }
                "configuration" => { self.debugger.options["configuration"] = if text.trim().is_empty() { Value::Null } else { json!(text.trim()) }; }
                "condition" | "logpoint" => {
                    if (self.debugger.process.is_some() || self.debugger.dap.is_some()) && !self.debugger.paused { return Err("Stop or pause before changing breakpoints".into()); }
                    let Some(path) = self.doc.path.clone() else { return Err("Open a Python file".into()); };
                    let line = self.doc.buf.line_of(self.doc.buf.cur) + 1;
                    let point = (path.clone(), line);
                    if !self.debugger.points.contains(&point) { self.debugger.points.push(point); }
                    let key = format!("{}:{line}", path.display());
                    if !self.debugger.point_options[&key].is_object() { self.debugger.point_options[&key] = json!({}); }
                    self.debugger.point_options[&key][if action == "condition" { "condition" } else { "log" }] = json!(text);
                    if let Some(p) = &self.debugger.process { p.send(json!({"action":"breakpoints","points":self.debugger.points()})); }
                }
                "watch" => {
                    let mut watches = self.debugger.options["watches"].as_array().cloned().unwrap_or_default();
                    if let Some(i) = watches.iter().position(|v| v == text) { watches.remove(i); }
                    else if !text.is_empty() && watches.len() < 100 { watches.push(json!(text)); }
                    self.debugger.options["watches"] = json!(watches);
                    if let Some(p) = &self.debugger.process { p.send(json!({"action":"watches","watches":watches})); }
                    if self.debugger.dap.is_some() && self.debugger.paused {
                        let frame = self.debugger.snapshot["selected"].as_u64().unwrap_or(0) as usize;
                        self.native_watches(frame);
                    }
                }
                "evaluate" => {
                    if !self.debugger.paused { return Err("Pause before evaluating".into()); }
                    if let Some(p) = &self.debugger.process { p.send(json!({"action":"evaluate","expression":text})); }
                    if self.debugger.dap.is_some() { self.native_evaluate(text); }
                }
                "args" => {
                    let args: Vec<String> = serde_json::from_str(text).map_err(|e| format!("Use a JSON string array: {e}"))?;
                    self.debugger.options["args"] = json!(args);
                }
                "stdin" => {
                    let input: String = serde_json::from_str(text).map_err(|e| format!("Use a JSON string, with escaped newlines: {e}"))?;
                    self.debugger.options["stdin"] = json!(input);
                }
                "pytest" => { self.debugger.options["pytest"] = json!(text); }
                _ => {}
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.prompt = None;
                self.debug_save_settings();
                self.debugger.update();
                self.show_side(Some(Tab::Debug));
                if action == "pytest" { self.debug_command("start_pytest"); }
            }
            Err(e) => self.message = Some(e),
        }
    }
    pub(super) fn debug_frame(&mut self, key: &str) {
        if self.debugger.dap.is_some() && self.debugger.paused { if let Ok(i) = key.parse() { self.native_frame(i); } return; }
        if self.debugger.paused
            && let (Some(p), Ok(i)) = (&self.debugger.process, key.parse::<usize>())
        {
            p.send(json!({"action":"frame","index":i}));
        }
    }
    pub(super) fn debug_poll(&mut self) {
        if (self.debugger.process.is_some() || self.debugger.dap.is_some()) && self.debugger.root != self.root {
            self.debug_command("stop");
        }
        let mut events = self.debugger.process.as_ref().map_or(Vec::new(), Process::poll);
        events.extend(self.native_events());
        for event in events {
            match event {
                Ok(v) => match v["event"].as_str() {
                    Some("paused") => {
                        self.debugger.paused = true;
                        let selected = v["selected"].as_u64().unwrap_or(0) as usize;
                        let frame = &v["stack"][selected];
                        if let (Some(path), Some(line)) = (frame["path"].as_str(), frame["line"].as_u64()) {
                            let path = PathBuf::from(path);
                            if path.starts_with(&self.debugger.root) && path.is_file() {
                                self.open_tab(path);
                                let li = (line as usize).saturating_sub(1).min(self.doc.buf.lines() - 1);
                                self.go_to(self.doc.buf.starts[li]);
                            }
                        }
                        self.debugger.snapshot = v;
                        self.show_side(Some(Tab::Debug));
                    }
                    Some("hover") => self.debug_hover_answer(&v),
                    Some("children") => {
                        let key = v["reference"].as_str().unwrap_or("").to_string();
                        self.debug_children(&key, v["start"].as_u64().unwrap_or(0) as usize, &v["variables"]);
                    }
                    Some("output") => {
                        self.debugger.output += v["text"].as_str().unwrap_or("");
                        if self.debugger.output.len() > 60_000 {
                            self.debugger.output = self.debugger.output
                                [editor::ceil(&self.debugger.output, self.debugger.output.len() - 60_000)..]
                                .to_string();
                        }
                    }
                    Some("finished") => {
                        self.debugger.process = None;
                        self.debugger.dap = None;
                        self.debugger.paused = false;
                        self.debugger.snapshot = Value::Null;
                    }
                    _ => {}
                },
                Err(e) => {
                    if self.debugger.process.take().is_some() || self.debugger.dap.take().is_some() {
                        self.debugger.output += &format!("\n{e}");
                    }
                    self.debugger.paused = false;
                    self.debugger.snapshot = Value::Null;
                }
            }
            self.debugger.update();
            self.debug_tree_mark();
        }
    }
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn debug_settle(&mut self) {
        let start = std::time::Instant::now();
        while (self.debugger.process.is_some() || self.debugger.dap.is_some()) && !self.debugger.paused && start.elapsed().as_secs() < 5 {
            self.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Children and hover values asked for while paused come in shortly.
        for _ in 0..20 {
            if !self.debugger.paused { break; }
            self.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    pub(super) fn debug_marks(&mut self) {
        let mark = |pane: &mut Pane, d: &Debugging| {
            pane.breakpoints =
                d.points.iter().filter(|(p, _)| Some(p) == pane.path.as_ref()).map(|(_, l)| l - 1).collect();
            pane.debug_values.clear();
            pane.debug_exception = None;
            if d.paused && !pane.buf.modified {
                let f = &d.snapshot["stack"][d.snapshot["selected"].as_u64().unwrap_or(0) as usize];
                if pane.path.as_deref() == f["path"].as_str().map(Path::new) {
                    let stop = f["line"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
                    if d.options["inline_values"] != false {
                        let start = f["start"].as_u64().map_or_else(|| super::inline_values::function_start(&pane.buf.text, stop), |l| l.saturating_sub(1) as usize);
                        let python = pane.doc == Doc::Python || pane.path.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e == "py"));
                        pane.debug_values = super::inline_values::labels(&pane.buf.text, start, stop, &d.snapshot["variables"], python);
                    }
                    pane.debug_exception = super::debug_exceptions::summary(&d.snapshot).map(|e| (stop, e));
                }
            }
            pane.debug_line = if d.paused {
                let f = &d.snapshot["stack"][d.snapshot["selected"].as_u64().unwrap_or(0) as usize];
                (pane.path.as_deref() == f["path"].as_str().map(Path::new))
                    .then(|| f["line"].as_u64().unwrap_or(1) as usize - 1)
            } else {
                None
            };
        };
        mark(&mut self.doc, &self.debugger);
        for i in self.split_others() {
            mark(&mut self.file_tabs[i].state.as_mut().unwrap().pane, &self.debugger);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(a: &mut App, predicate: impl Fn(&App) -> bool) {
        let start = std::time::Instant::now();
        while !predicate(a) && start.elapsed().as_secs() < 5 {
            a.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(predicate(a), "debugger output: {}", a.debugger.output);
    }
    #[test]
    fn pytest_launcher_passes_one_node_to_a_fixture_module() {
        let dir = std::env::temp_dir().join(format!("ld-debug-pytest-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("pytest")).unwrap();
        std::fs::write(dir.join("pytest/__init__.py"), "").unwrap();
        std::fs::write(dir.join("pytest/__main__.py"), "import sys\nprint('pytest node', sys.argv[2])\n").unwrap();
        let path = dir.join("test_sample.py"); std::fs::write(&path, "def test_sample(): pass\n").unwrap();
        let mut a = App::new(Some(path)); a.resize(1200, 800, 1.0); a.debug_settings();
        a.debugger.options["pytest"] = json!("test_sample.py::test_sample");
        a.debug_command("start_pytest"); wait(&mut a, |a| a.debugger.paused);
        a.debug_command("continue"); wait(&mut a, |a| a.debugger.process.is_none());
        assert!(a.debugger.output.contains("pytest node test_sample.py::test_sample"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn conditions_logpoints_watches_evaluation_arguments_input_and_persistence() {
        let dir = std::env::temp_dir().join(format!("ld-debug-upgrades-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let path = dir.join("main.py");
        std::fs::write(&path, "import sys\nvalue = input()\nfor i in range(3):\n    x = i * 2\n    print('loop', i)\nprint(sys.argv[1], value)\n").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(1200, 800, 1.0);
        a.debug_settings();
        a.debugger.points = vec![(path.clone(), 4), (path.clone(), 5)];
        a.debugger.point_options = json!({format!("{}:4", path.display()): {"condition":"i == 2"}, format!("{}:5", path.display()): {"log":"i"}});
        a.debugger.options = json!({"args":["hello world"],"stdin":"answer\n","watches":["i", "missing_name"]});
        a.debug_save_settings();
        a.debug_command("start"); wait(&mut a, |a| a.debugger.paused);
        a.debug_command("continue"); wait(&mut a, |a| a.debugger.paused);
        assert_eq!(a.debugger.snapshot["stack"][0]["line"], 4);
        assert_eq!(a.debugger.snapshot["watches"][0]["value"], "2");
        assert!(a.debugger.snapshot["watches"][1]["value"].as_str().unwrap().contains("NameError"));
        a.debugger.input_action = "evaluate".into(); a.debug_input("i + 10");
        wait(&mut a, |a| a.debugger.output.contains("i + 10 = 12"));
        assert!(a.debugger.paused);
        assert!(a.debugger.output.contains("Logpoint: 0") && a.debugger.output.contains("Logpoint: 1"));
        a.debug_command("continue"); wait(&mut a, |a| a.debugger.process.is_none());
        assert!(a.debugger.output.contains("hello world answer"));
        let mut b = App::new(Some(path)); b.debug_settings();
        assert_eq!(b.debugger.points.len(), 2);
        assert_eq!(b.debugger.options["args"][0], "hello world");
        assert_eq!(b.debugger.options["watches"][0], "i");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn breakpoints_steps_variables_frames_and_stop_use_real_python() {
        let dir = std::env::temp_dir().join(format!("ld-debug-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let path = dir.join("main.py");
        std::fs::write(
            &path,
            "def double(v):\n    result = v * 2\n    return result\nx = 3\ny = double(x)\nprint(y)\n",
        )
        .unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(1200, 800, 1.0);
        a.debug_settings();
        a.debugger.points.push((path.canonicalize().unwrap(), 5));
        a.debug_command("start");
        wait(&mut a, |a| a.debugger.paused);
        a.debug_command("continue");
        wait(&mut a, |a| a.debugger.paused);
        assert_eq!(a.debugger.snapshot["stack"][0]["line"], 5);
        assert!(a.debugger.view.buf.text.contains("x: int = 3"));
        a.debug_command("step");
        wait(&mut a, |a| a.debugger.paused);
        assert_eq!(a.debugger.snapshot["stack"][0]["name"], "double");
        assert!(a.debugger.view.buf.text.contains("v: int = 3"));
        a.debug_command("next");
        wait(&mut a, |a| a.debugger.paused);
        assert!(a.debugger.view.buf.text.contains("result: int = 6"));
        a.debug_frame("1");
        wait(&mut a, |a| a.debugger.snapshot["selected"] == 1);
        assert!(a.debugger.view.buf.text.contains("x: int = 3"));
        a.debug_command("out");
        wait(&mut a, |a| a.debugger.paused);
        a.debug_command("continue");
        wait(&mut a, |a| a.debugger.process.is_none());
        assert!(a.debugger.output.contains('6'));
        a.debug_command("start");
        wait(&mut a, |a| a.debugger.paused);
        a.debug_command("stop");
        assert!(a.debugger.process.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
