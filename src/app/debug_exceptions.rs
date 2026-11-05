//! Stopping on exceptions: the Python bridge's raised and uncaught switches,
//! saved per project, and the filters a DAP adapter advertises, switched
//! during a session. The exception stopped on heads the Debug tab and is
//! written at the end of its line.
use super::*;
use crate::icons::Icon;
use serde_json::{Value, json};

/// The filters an adapter turns on unless told otherwise.
pub(super) fn default_filters(filters: &Value) -> Value {
    filters.as_array().into_iter().flatten().filter(|f| f["default"] == true).filter_map(|f| f["filter"].as_str().map(|s| json!(s))).collect()
}

/// The exception an `exceptionInfo` answer describes.
pub(super) fn from_info(body: &Value) -> Value {
    let short = |v: &Value| v.as_str().map(|s| s.chars().take(500).collect::<String>());
    let kind = short(&body["details"]["typeName"]).or(short(&body["exceptionId"])).unwrap_or_else(|| "Exception".into());
    let message = short(&body["description"]).or(short(&body["details"]["message"])).unwrap_or_default();
    json!({"type":kind,"message":message})
}

/// "Type: message" for the exception a snapshot stopped on.
pub(super) fn summary(snapshot: &Value) -> Option<String> {
    let e = snapshot["exception"].as_object()?;
    let kind = e.get("type").and_then(Value::as_str).unwrap_or("Exception");
    let message = e.get("message").and_then(Value::as_str).unwrap_or("").replace('\n', " ");
    let text = if message.is_empty() { kind.to_string() } else { format!("{kind}: {message}") };
    Some(text.chars().take(500).collect())
}

impl App {
    /// Debug: Select exception breakpoints: the adapter's filters in a
    /// native session, otherwise the Python bridge's two switches.
    pub(super) fn debug_exceptions_menu(&mut self) {
        if self.debugger.dap.is_some() { return self.native_filters(None); }
        let o = &self.debugger.options;
        let entries = [("raised", "Break on raised exceptions"), ("uncaught", "Break on uncaught exceptions")].iter().map(|(key, label)| {
            let on = o[*key] == true;
            let entry = Entry::new(key, label, if on { "on" } else { "off" });
            if on { entry.icon(Icon::Check) } else { entry }
        }).collect();
        self.menu = Some((MenuKind::DebugExceptions, Menu::new("Python exception breakpoints", entries, "")));
    }

    pub(super) fn debug_exception_pick(&mut self, key: &str) {
        if self.debugger.dap.is_some() { return self.native_filters(Some(key)); }
        if matches!(key, "raised" | "uncaught") {
            self.debug_command(key);
            self.debug_exceptions_menu();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_defaults_info_and_summary() {
        let filters = json!([{"filter":"raised","label":"Raised"},{"filter":"uncaught","label":"Uncaught","default":true}]);
        assert_eq!(default_filters(&filters), json!(["uncaught"]));
        let info = from_info(&json!({"exceptionId":"E","description":"bad\nthing","details":{"typeName":"ValueError"}}));
        assert_eq!(summary(&json!({"exception":info})).as_deref(), Some("ValueError: bad thing"));
        assert_eq!(summary(&json!({"exception":{"type":"KeyError","message":""}})).as_deref(), Some("KeyError"));
        assert_eq!(summary(&json!({"exception":null})), None);
    }

    fn wait(a: &mut App, what: &str, predicate: impl Fn(&App) -> bool) {
        let start = std::time::Instant::now();
        while !predicate(a) && start.elapsed().as_secs() < 5 {
            a.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(predicate(a), "{what}: {}\n{}", a.debugger.output, a.debugger.view.buf.text);
    }

    #[test]
    fn python_stops_on_raised_and_uncaught_exceptions_in_project_code() {
        let dir = std::env::temp_dir().join(format!("ld-debug-exc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let path = dir.join("main.py");
        std::fs::write(&path, "import json\ndef parse(text):\n    return int(text)\ntry:\n    parse('x')\nexcept ValueError:\n    pass\ntry:\n    json.loads('{')\nexcept ValueError:\n    pass\nitems = {}\nitems['missing']\n").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(1200, 800, 1.0);
        a.debug_settings();
        a.debugger.options = json!({"args":[],"stdin":"","watches":[]});
        a.debug_exceptions_menu();
        a.debug_exception_pick("raised");
        a.debug_exception_pick("uncaught");
        assert!(a.debugger.options["raised"] == true && a.debugger.options["uncaught"] == true);
        a.debug_command("start");
        wait(&mut a, "started", |a| a.debugger.paused);
        a.debug_command("continue");
        wait(&mut a, "raised", |a| a.debugger.paused);
        assert_eq!(summary(&a.debugger.snapshot).as_deref(), Some("ValueError: invalid literal for int() with base 10: 'x'"));
        assert_eq!(a.debugger.snapshot["stack"][0]["line"], 3);
        assert!(a.debugger.view.buf.text.contains("Exception: ValueError: invalid literal"));
        a.debug_marks();
        assert!(a.doc.debug_exception.as_ref().is_some_and(|(line, text)| *line == 2 && text.starts_with("ValueError")));
        // The exception stops once, not again in each caller; json's own
        // raise stops in the project frame that called it.
        a.debug_command("continue");
        wait(&mut a, "library", |a| a.debugger.paused);
        assert_eq!(a.debugger.snapshot["stack"][0]["line"], 9);
        assert!(summary(&a.debugger.snapshot).unwrap().starts_with("JSONDecodeError"));
        a.debug_command("raised");
        a.debug_command("continue");
        wait(&mut a, "uncaught", |a| a.debugger.paused);
        assert_eq!(summary(&a.debugger.snapshot).as_deref(), Some("KeyError: 'missing'"));
        assert_eq!(a.debugger.snapshot["stack"][0]["line"], 13);
        assert!(a.debugger.snapshot["variables"].as_array().unwrap().iter().any(|v| v["name"] == "items"));
        a.debug_command("continue");
        wait(&mut a, "finished", |a| a.debugger.process.is_none());
        assert!(a.debugger.output.contains("KeyError"));
        let mut b = App::new(Some(path)); b.debug_settings();
        assert!(b.debugger.options["raised"] == false && b.debugger.options["uncaught"] == true);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
