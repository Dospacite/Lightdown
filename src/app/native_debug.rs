//! DAP sessions feed the same stack, variables, output and source marks as Python.
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub(super) struct Session {
    client: crate::dap::Client, seq: u64, pending: BTreeMap<u64, (String, usize)>,
    started: std::time::Instant,
    launch: Value, request: String, exceptions: Value, panic: bool, reason: String, generation: u64, thread: i64, initialized: bool, configured: bool, done_supported: bool, filters: Value,
    /// The exception stopped on, and whether the adapter can describe it.
    exception: Value, info_supported: bool,
}
impl Session {
    fn request(&mut self, command: &str, arguments: Value, frame: usize) {
        self.request_as(command, command, arguments, frame);
    }
    /// Sends `command`, remembering its reply as `label`'s.
    fn request_as(&mut self, label: &str, command: &str, arguments: Value, frame: usize) {
        self.seq += 1; self.pending.insert(self.seq, (label.into(), frame));
        self.client.send(json!({"seq":self.seq,"type":"request","command":command,"arguments":arguments}));
    }
}
impl App {
    pub(super) fn native_start(&mut self) {
        self.native_start_value(None);
    }
    /// Reads and expands the selected configuration, then prepares and launches it.
    pub(super) fn native_start_value(&mut self, supplied: Option<Value>) {
        let result = (|| -> Result<Value, String> {
            if self.file_tabs.iter().enumerate().any(|(i,t)| if i == self.active_file { self.doc.buf.modified } else { t.state.as_ref().is_some_and(|s| s.pane.buf.modified) }) { return Err("Save source files before debugging".into()); }
            let mut value = if let Some(value) = supplied { value } else {
                let bytes = std::fs::read(self.root.join(".lightdown/debug.json")).map_err(|e| e.to_string())?;
                if bytes.len() > 1024 * 1024 { return Err("Debug configuration exceeds 1 MB".into()); }
                let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                super::debug_config::select(value, self.debugger.options["configuration"].as_str())?
            };
            super::debug_config::expand(&mut value, &self.root, self.doc.path.as_deref())?;
            Ok(value)
        })();
        match result {
            Ok(value) => self.debug_prepare(value),
            Err(e) => self.message = Some(format!("Native debug: {e}")),
        }
    }
    /// Starts the adapter for an expanded configuration whose preparation is done.
    pub(super) fn native_launch(&mut self, mut value: Value) {
        let result = (|| -> Result<Session, String> {
            let request = value["request"].as_str().unwrap_or("launch").to_string();
            if request != "launch" && request != "attach" { return Err("request must be launch or attach".into()); }
            super::debug_config::discover_adapter(&mut value, &self.cfg.python_for(&self.root))?;
            let argv: Vec<String> = value["adapter"].as_array().ok_or("adapter must be an argv array")?.iter().map(|v| v.as_str().map(str::to_string).ok_or("Adapter arguments must be strings".into())).collect::<Result<_, String>>()?;
            if argv.is_empty() || !value["launch"].is_object() { return Err("Provide adapter argv and launch object".into()); }
            if value.get("transport").is_some_and(|v| v != "tcp" && v != "stdio") { return Err("transport must be stdio or tcp".into()); }
            // js-debug resolves launch/attach defaults from the configuration,
            // independently of the DAP command used to deliver it.
            if value["launch"]["type"] == "pwa-node" || value["type"].as_str().is_some_and(|s| ["node", "pwa-node", "javascript", "js-debug"].contains(&s)) {
                value["launch"]["type"] = json!("pwa-node");
                value["launch"]["request"] = json!(request);
                if request == "attach" { node_attach_port(&mut value["launch"])?; }
                // VS Code's extension always passes this; without it js-debug cannot
                // predict where breakpoints in TypeScript and other mapped sources
                // land, and they bind only after the code has run.
                if value["launch"]["__workspaceFolder"].is_null() { value["launch"]["__workspaceFolder"] = json!(self.root); }
            }
            let tcp = value["transport"] == "tcp";
            let client = crate::dap::Client::start(argv, tcp, self.root.clone(), self.waker.clone());
            Ok(Session { started: std::time::Instant::now(), client, seq: 0, pending: BTreeMap::new(), launch: value["launch"].clone(), request, exceptions: value["exceptionFilters"].clone(), panic: value["breakOnPanic"] == true, reason: String::new(), generation: 0, thread: 0, initialized: false, configured: false, done_supported: false, filters: json!([]), exception: Value::Null, info_supported: false })
        })();
        match result {
            Ok(mut s) => {
                s.request("initialize", json!({"clientID":"lightdown","clientName":"Lightdown","adapterID":"native","linesStartAt1":true,"columnsStartAt1":true,"pathFormat":"path","supportsRunInTerminalRequest":false,"supportsVariableType":true}), 0);
                self.debugger.process = None; self.debugger.dap = Some(s); self.debugger.root = self.root.clone();
                self.debugger.output.clear(); self.debugger.snapshot = Value::Null; self.debugger.paused = false;
                self.show_side(Some(Tab::Debug)); self.debugger.update();
            }
            Err(e) => self.message = Some(format!("Native debug: {e}")),
        }
    }
    pub(super) fn native_points(&mut self) {
        let mut by_path: BTreeMap<PathBuf, Vec<Value>> = BTreeMap::new();
        // Conditions and logpoints go to the adapter as DAP has them.
        for p in self.debugger.points().as_array().into_iter().flatten() {
            let Some(path) = p["path"].as_str() else { continue };
            let mut point = json!({"line":p["line"]});
            if let Some(c) = p["condition"].as_str().filter(|c| !c.is_empty()) { point["condition"] = json!(c); }
            if let Some(l) = p["log"].as_str().filter(|l| !l.is_empty()) { point["logMessage"] = json!(l); }
            by_path.entry(PathBuf::from(path)).or_default().push(point);
        }
        // Include the current file even when its last breakpoint was removed.
        if let Some(path) = &self.doc.path { by_path.entry(path.clone()).or_default(); }
        if let Some(s) = &mut self.debugger.dap {
            for (path, points) in by_path { s.request("setBreakpoints", json!({"source":{"path":path},"breakpoints":points}), 0); }
        }
    }
    pub(super) fn native_command(&mut self, command: &str) {
        if command == "stop" { self.debugger.dap = None; self.debugger.paused = false; self.debugger.snapshot = Value::Null; self.debugger.update(); return; }
        if !self.debugger.paused { self.message = Some("Pause the debugger before stepping".into()); return; }
        if let Some(s) = &mut self.debugger.dap {
            let command = match command { "continue" => "continue", "next" => "next", "step" => "stepIn", "out" => "stepOut", _ => return };
            s.generation += 1; s.request(command, json!({"threadId":s.thread}), 0); self.debugger.paused = false;
        }
    }
    pub(super) fn native_frame(&mut self, frame: usize) {
        let id = self.debugger.snapshot["stack"][frame]["id"].as_i64();
        if let (Some(id), Some(s)) = (id, &mut self.debugger.dap) {
            s.generation += 1;
            self.debugger.snapshot["selected"] = json!(frame);
            s.request("scopes", json!({"frameId":id}), frame);
        }
    }
    /// Evaluates the watch expressions in a paused frame; their values
    /// fill in as the adapter answers.
    pub(super) fn native_watches(&mut self, frame: usize) {
        let watches = self.debugger.options["watches"].as_array().cloned().unwrap_or_default();
        let id = self.debugger.snapshot["stack"][frame]["id"].clone();
        self.debugger.snapshot["watches"] = watches.iter().map(|e| json!({"expression":e,"value":"…"})).collect();
        if let Some(s) = &mut self.debugger.dap {
            for (i, e) in watches.iter().enumerate() {
                s.request_as("watch", "evaluate", json!({"expression":e,"frameId":id,"context":"watch"}), i);
            }
        }
    }
    pub(super) fn native_expand(&mut self, reference: i64) -> Result<(), String> {
        if reference <= 0 { return Err("Enter a positive variables reference".into()); }
        if self.debugger.snapshot["expanded"].as_object().is_some_and(|o| o.len() >= 100) { return Err("Variable expansion limit reached; select the frame again".into()); }
        let frame = self.debugger.snapshot["selected"].as_u64().unwrap_or(0) as usize;
        if let Some(s) = &mut self.debugger.dap {
            let start = self.debugger.snapshot["expanded"][reference.to_string()].as_array().map_or(0, Vec::len);
            if start >= 20000 { return Err("Variable page limit reached".into()); }
            s.request_as(&format!("children:{reference}:{}:{start}", s.generation), "variables", json!({"variablesReference":reference,"start":start,"count":super::debug_tree::PAGE}), frame);
        }
        Ok(())
    }
    pub(super) fn native_filters(&mut self, toggle: Option<&str>) {
        let Some(s) = &mut self.debugger.dap else { return };
        if !s.filters.as_array().is_some_and(|f| !f.is_empty()) {
            self.message = Some(if s.initialized { "The adapter offers no exception filters" } else { "The adapter has not started yet" }.into());
            return;
        }
        if let Some(id) = toggle {
            let mut enabled = s.exceptions.as_array().cloned().unwrap_or_default();
            if let Some(i) = enabled.iter().position(|v| v == id) { enabled.remove(i); } else { enabled.push(json!(id)); }
            s.exceptions = json!(enabled);
            s.request("setExceptionBreakpoints", json!({"filters":s.exceptions}), 0);
        }
        let entries = s.filters.as_array().into_iter().flatten().take(100).filter_map(|f| {
            let id = f["filter"].as_str()?;
            let on = s.exceptions.as_array().is_some_and(|a| a.iter().any(|v| v == id));
            let entry = Entry::new(id, f["label"].as_str().unwrap_or(id), if on { "on" } else { "off" });
            Some(if on { entry.icon(crate::icons::Icon::Check) } else { entry })
        }).collect();
        self.menu = Some((MenuKind::DebugExceptions, Menu::new("Toggle exception filter", entries, "")));
    }
    pub(super) fn native_thread(&mut self, id: i64) -> Result<(), String> {
        if !self.debugger.snapshot["threads"].as_array().is_some_and(|ts| ts.iter().any(|t| t["id"] == id)) { return Err("Choose a thread ID shown in the Debug tab".into()); }
        if let Some(s) = &mut self.debugger.dap { s.thread = id; s.request("stackTrace", json!({"threadId":id,"startFrame":0,"levels":100}), 0); }
        Ok(())
    }
    pub(super) fn native_events(&mut self) -> Vec<Result<Value, String>> {
        let incoming = self.debugger.dap.as_ref().map_or(Vec::new(), |s| s.client.poll());
        let mut events = Vec::new();
        let mut watched = false;
        if self.debugger.dap.as_ref().is_some_and(|s| !s.configured && s.started.elapsed().as_secs() > 20) { return vec![Err("DAP initialization timed out".into())]; }
        for v in incoming {
            if let Some(e) = v["clientError"].as_str() { events.push(Err(e.into())); continue; }
            if v["type"] == "request" {
                if let Some(s) = &self.debugger.dap { s.client.send(json!({"seq":0,"type":"response","request_seq":v["seq"],"command":v["command"],"success":false,"message":"Lightdown does not support reverse requests; use internalConsole"})); }
                continue;
            }
            match v["event"].as_str() {
                Some("initialized") => { if let Some(s) = &mut self.debugger.dap { s.initialized = true;
                    // Without configured filters, the adapter's defaults apply.
                    if !s.exceptions.is_array() { s.exceptions = super::debug_exceptions::default_filters(&s.filters); }
                    if let Some(filters) = s.exceptions.as_array() { let filters = filters.clone(); s.request("setExceptionBreakpoints", json!({"filters":filters}), 0); }
                    if s.panic { s.request("setFunctionBreakpoints", json!({"breakpoints":[{"name":"rust_panic"},{"name":"rust_begin_unwind"}]}), 0); }
                } self.native_points(); }
                Some("output") => events.push(Ok(json!({"event":"output","text":v["body"]["output"]}))),
                Some("terminated" | "exited") => { events.push(Ok(json!({"event":"finished"}))); }
                Some("continued") => { self.debugger.paused = false; if let Some(s) = &mut self.debugger.dap { s.generation += 1; } },
                Some("stopped") => {
                    self.debugger.paused = true;
                    if let Some(s) = &mut self.debugger.dap {
                        s.reason = v["body"]["description"].as_str().or(v["body"]["text"].as_str()).or(v["body"]["reason"].as_str()).unwrap_or("stopped").chars().take(2000).collect();
                        s.thread = v["body"]["threadId"].as_i64().unwrap_or(0);
                        s.exception = Value::Null;
                        if v["body"]["reason"] == "exception" {
                            s.exception = json!({"type":"Exception","message":v["body"]["text"].as_str().or(v["body"]["description"].as_str()).unwrap_or("").chars().take(500).collect::<String>()});
                            if s.info_supported { s.request("exceptionInfo", json!({"threadId":s.thread}), 0); }
                        }
                        s.request("threads", json!({}), 0);
                    }
                }
                _ => {}
            }
            let mut paused_at = None;
            if v["type"] == "response" {
                let Some(s) = &mut self.debugger.dap else { break };
                let Some((command, frame)) = v["request_seq"].as_u64().and_then(|id| s.pending.remove(&id)) else { continue };
                if v["success"] == false {
                    let why = v["body"]["error"]["format"].as_str().or(v["message"].as_str()).unwrap_or("adapter error");
                    // An expression that fails is answered, not fatal.
                    match command.as_str() {
                        "watch" => {
                            if let Some(w) = self.debugger.snapshot["watches"].get_mut(frame) { w["value"] = json!(format!("<{why}>")); }
                            watched = true;
                        }
                        // The stop's own text stays when the adapter cannot say more.
                        "exceptionInfo" => {}
                        label if label.starts_with("hover:") => events.push(Ok(json!({"event":"hover","id":label[6..].parse::<u64>().unwrap_or(0),"error":why}))),
                        "setExceptionBreakpoints" if s.configured => events.push(Ok(json!({"event":"output","text":format!("Exception filters: {why}\n")}))),
                        label if label == "evaluate" || label.starts_with("children:") => events.push(Ok(json!({"event":"output","text":format!("{why}\n")}))),
                        _ => events.push(Err(format!("{command}: {why}{}", if command == "attach" { super::debug_process::ptrace_hint() } else { String::new() }))),
                    }
                    continue;
                }
                match command.as_str() {
                    "initialize" => { s.filters = Value::Array(v["body"]["exceptionBreakpointFilters"].as_array().into_iter().flatten().take(100).cloned().collect()); s.info_supported = v["body"]["supportsExceptionInfoRequest"] == true; s.done_supported = v["body"]["supportsConfigurationDoneRequest"] == true; s.request(&s.request.clone(), s.launch.clone(), 0); }
                    "threads" if self.debugger.paused => { self.debugger.snapshot["threads"] = Value::Array(v["body"]["threads"].as_array().into_iter().flatten().take(200).cloned().collect()); if s.thread == 0 { s.thread = v["body"]["threads"][0]["id"].as_i64().unwrap_or(1); } s.request("stackTrace", json!({"threadId":s.thread,"levels":100}), 0); }
                    "stackTrace" if self.debugger.paused => {
                        let stack: Vec<_> = v["body"]["stackFrames"].as_array().into_iter().flatten().take(100).map(|f| json!({"id":f["id"],"name":f["name"],"path":f["source"]["path"],"line":f["line"]})).collect();
                        self.debugger.snapshot = json!({"reason":s.reason,"exception":s.exception,"selected":0,"stack":stack,"variables":[],"expanded":{},"threads":self.debugger.snapshot["threads"]});
                        self.native_frame(0);
                    }
                    "scopes" if self.debugger.paused && self.debugger.snapshot["selected"] == frame => {
                        self.debugger.snapshot["variables"] = json!([]);
                        self.debugger.snapshot["expanded"] = json!({});
                        if let Some(scope) = v["body"]["scopes"].as_array().and_then(|a| a.iter().find(|v| v["expensive"] != true)) {
                            s.request("variables", json!({"variablesReference":scope["variablesReference"],"count":200}), frame);
                        } else { paused_at = Some(frame); }
                    }
                    "variables" if self.debugger.paused && self.debugger.snapshot["selected"] == frame => {
                        self.debugger.snapshot["variables"] = Value::Array(v["body"]["variables"].as_array().into_iter().flatten().take(200).cloned().collect());
                        paused_at = Some(frame);
                    }
                    label if label.starts_with("children:") && self.debugger.paused && self.debugger.snapshot["selected"] == frame => {
                        let mut parts = label[9..].split(':');
                        let (Some(key), Some(generation), Some(start)) = (parts.next(), parts.next(), parts.next()) else { continue };
                        if generation.parse::<u64>().ok() != Some(s.generation) { continue; }
                        let (key, start) = (key.to_string(), start.parse().unwrap_or(0));
                        self.debug_children(&key, start, &v["body"]["variables"]);
                        watched = true;
                    }
                    label if label.starts_with("hover:") => events.push(Ok(json!({"event":"hover","id":label[6..].parse::<u64>().unwrap_or(0),"type":v["body"]["type"],"value":v["body"]["result"]}))),
                    "exceptionInfo" if self.debugger.paused => {
                        s.exception = super::debug_exceptions::from_info(&v["body"]);
                        self.debugger.snapshot["exception"] = s.exception.clone();
                        watched = true;
                    }
                    "evaluate" => events.push(Ok(json!({"event":"output","text":format!("{}\n", v["body"]["result"].as_str().unwrap_or(""))}))),
                    "watch" => {
                        if let Some(w) = self.debugger.snapshot["watches"].get_mut(frame) { w["value"] = v["body"]["result"].clone(); }
                        watched = true;
                    }
                    _ => {}
                }
            }
            if let Some(frame) = paused_at {
                self.native_watches(frame);
                let mut snapshot = self.debugger.snapshot.clone();
                snapshot["event"] = json!("paused");
                events.push(Ok(snapshot));
            }
            if let Some(s) = &mut self.debugger.dap && s.initialized && !s.configured && !s.pending.values().any(|(c, _)| matches!(c.as_str(), "setBreakpoints" | "setExceptionBreakpoints" | "setFunctionBreakpoints")) {
                s.configured = true; if s.done_supported { s.request("configurationDone", json!({}), 0); }
            }
        }
        // Watch values change the snapshot in place; the view is drawn anew.
        if watched { events.push(Ok(json!({"event":"watches"}))); }
        events
    }
    /// Asks for a plain name or dotted chain in the hover context.
    pub(super) fn native_hover(&mut self, id: u64, expression: &str, frame_id: Value, frame: usize) {
        if let Some(s) = &mut self.debugger.dap { s.request_as(&format!("hover:{id}"), "evaluate", json!({"expression":expression,"frameId":frame_id,"context":"hover"}), frame); }
    }
    pub(super) fn native_evaluate(&mut self, text: &str) {
        let frame = self.debugger.snapshot["selected"].as_u64().unwrap_or(0) as usize;
        let id = self.debugger.snapshot["stack"][frame]["id"].clone();
        if let Some(s) = &mut self.debugger.dap { s.request("evaluate", json!({"expression":text,"frameId":id,"context":"watch"}), frame); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn wait(a: &mut App, condition: impl Fn(&App) -> bool) {
        let start = std::time::Instant::now();
        while !condition(a) && start.elapsed().as_secs() < 5 { a.debug_poll(); std::thread::sleep(std::time::Duration::from_millis(5)); }
        assert!(condition(a), "{}", a.debugger.output);
    }
    #[test]
    fn real_protocol_fixture_initializes_steps_inspects_and_evaluates_over_both_transports() {
        for tcp in [false, true] {
            let root = std::env::temp_dir().join(format!("ld-dap-{}-{tcp}", std::process::id()));
            std::fs::create_dir_all(root.join(".git")).unwrap(); std::fs::create_dir_all(root.join(".lightdown")).unwrap();
            let source = root.join("main.rs"); std::fs::write(&source, "fn main() {}\n").unwrap();
            let fake = root.join("adapter.py"); std::fs::write(&fake, include_str!("../../assets/test_dap.py")).unwrap();
            let mut argv = vec!["python3".to_string(), fake.to_string_lossy().into_owned(), source.to_string_lossy().into_owned()];
            if tcp { argv.push("{port}".into()); }
            let config = json!({"name":"fixture","adapter":argv,"transport":if tcp {"tcp"} else {"stdio"},"request":if tcp {"attach"} else {"launch"},"launch":{"program":"${file}"},"exceptionFilters":["all"],"breakOnPanic":true});
            let config = if tcp { json!({"default":"fixture","configurations":[config]}) } else { config };
            std::fs::write(root.join(".lightdown/debug.json"), config.to_string()).unwrap();
            let mut a = App::new(Some(source)); a.resize(1200, 800, 1.0);
            a.debug_command("native");
            wait(&mut a, |a| a.debugger.snapshot["variables"][0]["value"] == "42");
            assert!(a.debugger.view.buf.text.contains("answer: int = 42"));
            assert!(a.debugger.view.buf.text.contains("7: main"));
            a.native_expand(30).unwrap();
            wait(&mut a, |a| a.debugger.view.buf.text.contains("  child: int = 3"));
            assert!(a.native_thread(999).is_err());
            a.native_thread(7).unwrap();
            wait(&mut a, |a| a.debugger.snapshot["expanded"].as_object().is_some_and(|o| o.is_empty()));
            a.debugger.input_action = "evaluate".into(); a.debug_input("answer");
            wait(&mut a, |a| a.debugger.output.contains("evaluated 42"));
            // Hover asks only for plain names, in the hover context.
            a.docs_rest_on((3, 7));
            wait(&mut a, |a| a.docs_text().as_deref() == Some("main: int = hover main"));
            a.debug_command("next"); wait(&mut a, |a| a.debugger.paused);
            // The adapter's advertised filters switch during the session.
            a.debug_command("exceptions");
            assert!(matches!(&a.menu, Some((MenuKind::DebugExceptions, _))));
            a.menu = None;
            a.debug_exception_pick("raised");
            wait(&mut a, |a| a.debugger.output.contains("filters all,raised"));
            a.debug_exception_pick("all");
            wait(&mut a, |a| a.debugger.output.contains("filters raised\n"));
            a.debug_command("step");
            wait(&mut a, |a| a.debugger.paused && a.debugger.snapshot["exception"]["type"] == "core::panic");
            assert!(a.debugger.view.buf.text.contains("Exception: core::panic: index out of bounds"), "{}", a.debugger.view.buf.text);
            a.debug_command("continue"); wait(&mut a, |a| a.debugger.dap.is_none());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn adapter_default_filters_apply_without_configured_ones() {
        let root = std::env::temp_dir().join(format!("ld-dap-defaults-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap(); std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let source = root.join("main.rs"); std::fs::write(&source, "fn main() {}\n").unwrap();
        let fake = root.join("adapter.py"); std::fs::write(&fake, include_str!("../../assets/test_dap.py")).unwrap();
        let argv = vec!["python3".to_string(), fake.to_string_lossy().into_owned(), source.to_string_lossy().into_owned()];
        std::fs::write(root.join(".lightdown/debug.json"), json!({"adapter":argv,"launch":{"program":"${file}"}}).to_string()).unwrap();
        let mut a = App::new(Some(source)); a.resize(1200, 800, 1.0);
        a.debug_command("native");
        wait(&mut a, |a| a.debugger.paused && a.debugger.output.contains("filters uncaught\n"));
        a.debug_command("stop");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires gdb 14 or newer and a C compiler"]
    fn gdb_dap_honours_conditions_logpoints_and_watches() {
        let root = std::env::temp_dir().join(format!("ld-gdb-watch-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap(); std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let source = root.join("loop.c");
        std::fs::write(&source, "int square(int x) {\n    int y = x * x;\n    return y;\n}\nint main(void) {\n    int total = 0;\n    for (int i = 0; i < 5; i++) total += square(i);\n    return total - 30;\n}\n").unwrap();
        let program = root.join("loop");
        assert!(std::process::Command::new("cc").args(["-g", "-O0", "-o"]).arg(&program).arg(&source).status().unwrap().success());
        std::fs::write(root.join(".lightdown/debug.json"), json!({"adapter":["gdb","-q","-i","dap"],"transport":"stdio",
            "launch":{"program":program,"cwd":root}}).to_string()).unwrap();
        let mut a = App::new(Some(source)); a.resize(1200, 800, 1.0);
        let at_line = |a: &mut App, n: usize| { let at = a.doc.buf.starts[n - 1]; a.doc.select(at, at); };
        at_line(&mut a, 3);
        a.debugger.input_action = "condition".into(); a.debug_input("x == 3");
        at_line(&mut a, 2);
        a.debugger.input_action = "logpoint".into(); a.debug_input("x is {x}");
        for w in ["y", "x * 10", "no_such_name"] { a.debugger.input_action = "watch".into(); a.debug_input(w); }
        a.debug_command("native");
        wait(&mut a, |a| a.debugger.paused && a.debugger.snapshot["stack"][0]["line"] == 3);
        wait(&mut a, |a| a.debugger.snapshot["watches"][2]["value"].as_str().is_some_and(|v| v.starts_with('<')));
        let watches = &a.debugger.snapshot["watches"];
        assert_eq!((watches[0]["value"].as_str(), watches[1]["value"].as_str()), (Some("9"), Some("30")), "{watches}");
        assert!(a.debugger.view.buf.text.contains("x * 10 = 30"), "{}", a.debugger.view.buf.text);
        // The logpoint ran for each call up to the stop, without stopping.
        for n in 0..=3 { assert!(a.debugger.output.contains(&format!("x is {n}")), "{}", a.debugger.output); }
        a.debug_command("continue"); wait(&mut a, |a| a.debugger.dap.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires gdb 14 or newer and a C compiler"]
    fn gdb_dap_stops_in_main_inspects_steps_and_finishes() {
        let root = std::env::temp_dir().join(format!("ld-gdb-dap-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap(); std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let source = root.join("main.c");
        std::fs::write(&source, "int square(int x) {\n    int y = x * x;\n    return y;\n}\nint main(void) {\n    int answer = square(6);\n    return answer - 36;\n}\n").unwrap();
        let program = root.join("app");
        assert!(std::process::Command::new("cc").args(["-g", "-O0", "-o"]).arg(&program).arg(&source).status().unwrap().success());
        std::fs::write(root.join(".lightdown/debug.json"), json!({"adapter":["gdb","-q","-i","dap"],"transport":"stdio",
            "launch":{"program":program,"cwd":root,"stopAtBeginningOfMainSubprogram":true}}).to_string()).unwrap();
        let mut a = App::new(Some(source)); a.resize(1200, 800, 1.0);
        a.debug_command("native");
        wait(&mut a, |a| a.debugger.paused && a.debugger.snapshot["stack"][0]["name"].as_str().is_some_and(|n| n.contains("main")));
        wait(&mut a, |a| a.debugger.snapshot["variables"].as_array().is_some_and(|v| v.iter().any(|v| v["name"] == "answer")));
        a.debug_command("next");
        wait(&mut a, |a| a.debugger.paused && a.debugger.snapshot["stack"][0]["line"] == 7);
        a.debugger.input_action = "evaluate".into(); a.debug_input("answer * 2");
        wait(&mut a, |a| a.debugger.output.contains("72"));
        let at = a.doc.buf.text.rfind("answer").unwrap();
        a.docs_rest_on((at, at + 6));
        wait(&mut a, |a| a.docs_text().is_some_and(|t| t.starts_with("answer") && t.ends_with("= 36")));
        a.debug_marks();
        assert!(a.doc.debug_values.iter().any(|(line, text)| *line == 5 && text == "answer = 36"), "{:?}", a.doc.debug_values);
        a.debug_command("continue"); wait(&mut a, |a| a.debugger.dap.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Turns a Node attach by process ID into an attach by inspector port. The
/// standalone js-debug server connects to a port but does not ask the process
/// to open one, so this sends the signal VS Code's extension sends. A
/// `processId` of `pid:port` names a port that is already open.
fn node_attach_port(launch: &mut Value) -> Result<(), String> {
    if !launch["port"].is_null() {
        return Ok(());
    }
    let id = match &launch["processId"] {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return Ok(()),
    };
    let (address, port) = match id.split_once(':') {
        Some((_, port)) => ("127.0.0.1", port.parse::<u16>().map_err(|_| format!("Bad Node process ID {id}"))?),
        None => crate::dap::open_node_inspector(id.parse().map_err(|_| format!("Bad Node process ID {id}"))?)?,
    };
    if let Some(launch) = launch.as_object_mut() {
        launch.remove("processId");
        launch.insert("port".into(), json!(port));
        launch.entry("address").or_insert(json!(address));
    }
    Ok(())
}

#[cfg(test)]
mod node_attach_tests {
    use super::*;

    #[test]
    fn a_process_id_with_a_port_attaches_to_that_port() {
        let mut launch = json!({"type":"pwa-node","processId":"123:9230"});
        node_attach_port(&mut launch).unwrap();
        assert_eq!(launch, json!({"type":"pwa-node","port":9230,"address":"127.0.0.1"}));
        // An explicit port is left alone, and so is an attach without a process.
        let mut launch = json!({"processId":"123","port":9300});
        node_attach_port(&mut launch).unwrap();
        assert_eq!(launch["processId"], "123");
        let mut launch = json!({"port":9229});
        node_attach_port(&mut launch).unwrap();
        assert_eq!(launch, json!({"port":9229}));
        assert!(node_attach_port(&mut json!({"processId":"nope"})).is_err());
    }
}
