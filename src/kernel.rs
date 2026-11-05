//! The Python helper process: it runs notebook cells and answers completion
//! and documentation requests. The script is embedded and started with whichever interpreter
//! the settings name, so nothing has to be installed beside Python itself.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use serde_json::{Map, Value, json};

static SCRIPT: &str = include_str!("../assets/kernel.py");
static BRIDGE: &str = include_str!("../assets/jupyter_bridge.py");

pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub enum Event {
    /// The interpreter's version, and for a Jupyter kernel its `language_info`.
    Ready(String, Option<Value>),
    Variables(Vec<(String, String, String)>),
    Timing {
        id: u64,
        elapsed_ms: f64,
        ran_at: f64,
    },
    Stream {
        id: u64,
        err: bool,
        text: String,
    },
    /// A value shown under a cell: the result of its last expression, or
    /// something displayed along the way.
    Show {
        id: u64,
        result: bool,
        /// The output's forms by MIME type, as Jupyter gives them.
        data: Map<String, Value>,
        metadata: Value,
        /// A display that later updates may replace.
        display_id: Option<String>,
        /// This replaces the outputs shown under `display_id`.
        update: bool,
    },
    /// The cell's output is to be cleared, now or (`wait`) when the next arrives.
    Clear {
        id: u64,
        wait: bool,
    },
    /// The cell asks for a line of input, as `input()` does.
    Input {
        id: u64,
        prompt: String,
        password: bool,
    },
    /// Jupyter kernels cannot be started: `jupyter_client` is missing.
    Missing(String),
    Error {
        id: u64,
        name: String,
        value: String,
        trace: Vec<String>,
    },
    Done {
        id: u64,
        count: u64,
        ok: bool,
    },
    Matches {
        id: u64,
        items: Vec<(String, String)>,
    },
    /// What is known of a name: `None` when nothing is.
    Info {
        id: u64,
        info: Option<Info>,
    },
    /// The process ended; the text is whatever it last said on stderr.
    Dead(String),
}

/// A function's, class's or value's signature and documentation.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Info {
    pub name: String,
    /// Function, class, module, or a value's type.
    pub kind: String,
    /// How it is called, as `name(a, b=1)`, or what it is.
    pub signature: String,
    /// Each parameter, and the byte range it takes in the signature.
    pub params: Vec<(String, Option<(usize, usize)>)>,
    pub doc: String,
    /// The module it comes from, where that is worth saying.
    pub module: String,
}

/// The helper counts in characters; the editor in bytes.
fn byte_range(s: &str, from: i64, to: i64) -> Option<(usize, usize)> {
    let at = |n: i64| s.char_indices().map(|(i, _)| i).chain([s.len()]).nth(usize::try_from(n).ok()?);
    Some((at(from)?, at(to)?))
}

fn info(v: &Value) -> Option<Info> {
    let text = |key: &str| v[key].as_str().unwrap_or("").to_string();
    let signature = text("signature");
    let param = |p: &Value| {
        let range = byte_range(&signature, p[1].as_i64()?, p[2].as_i64()?);
        Some((p[0].as_str()?.to_string(), range))
    };
    let params = v["params"].as_array().into_iter().flatten().filter_map(param).collect();
    let info =
        Info { name: text("name"), kind: text("kind"), params, doc: text("doc"), module: text("module"), signature };
    Some(info)
}

pub struct Kernel {
    child: Option<Child>,
    stdin: ChildStdin,
    events: Receiver<Event>,
    /// Started through the Jupyter bridge rather than the built-in helper.
    jupyter: bool,
}

/// The plain-text form of an output's data.
pub fn plain(data: &Map<String, Value>) -> String {
    match data.get("text/plain") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

fn event(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line).ok()?;
    let id = v["id"].as_u64().unwrap_or(0);
    let text = |key: &str| v[key].as_str().unwrap_or("").to_string();
    Some(match v["type"].as_str()? {
        "ready" => Event::Ready(text("version"), v.get("language_info").cloned()),
        "missing" => Event::Missing(text("text")),
        "fatal" => Event::Dead(text("text")),
        "clear" => Event::Clear { id, wait: v["wait"] == true },
        "input" => Event::Input { id, prompt: text("prompt"), password: v["password"] == true },
        "variables" => Event::Variables(
            v["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| Some((v[0].as_str()?.into(), v[1].as_str()?.into(), v[2].as_str()?.into())))
                .collect(),
        ),
        "timing" => Event::Timing {
            id,
            elapsed_ms: v["elapsed_ms"].as_f64().unwrap_or_default(),
            ran_at: v["ran_at"].as_f64().unwrap_or_default(),
        },
        "stream" => Event::Stream { id, err: v["name"] == "stderr", text: text("text") },
        kind @ ("result" | "display") => {
            // The built-in helper names its forms; the bridge passes Jupyter's on.
            let mut data = v["data"].as_object().cloned().unwrap_or_default();
            if v.get("data").is_none() {
                data.insert("text/plain".into(), text("text").into());
                for (key, mime) in [("png", "image/png"), ("html", "text/html")] {
                    if let Some(s) = v[key].as_str() {
                        data.insert(mime.into(), s.into());
                    }
                }
            }
            Event::Show {
                id,
                result: kind == "result",
                data,
                metadata: v.get("metadata").cloned().unwrap_or(json!({})),
                display_id: v["display_id"].as_str().map(str::to_string),
                update: v["update"] == true,
            }
        }
        "error" => {
            let trace = v["trace"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(str::to_string));
            Event::Error { id, name: text("name"), value: text("value"), trace: trace.collect() }
        }
        "done" => Event::Done { id, count: v["count"].as_u64().unwrap_or(0), ok: v["ok"] == true },
        "matches" => {
            let pair = |i: &Value| Some((i[0].as_str()?.to_string(), i[1].as_str().unwrap_or("").to_string()));
            Event::Matches { id, items: v["items"].as_array().into_iter().flatten().filter_map(pair).collect() }
        }
        "info" => Event::Info { id, info: v["info"].is_object().then(|| info(&v["info"])).flatten() },
        _ => return None,
    })
}

impl Kernel {
    /// Starts the helper in `dir`. `wake` is called, from another thread,
    /// whenever there are events to `poll`.
    #[cfg(test)]
    pub fn spawn(python: &str, dir: &Path, wake: Waker) -> std::io::Result<Kernel> {
        Self::spawn_in(python, dir, &crate::config::project_root(Some(&dir.join("__kernel__"))), wake)
    }
    pub fn spawn_in(python: &str, dir: &Path, root: &Path, wake: Waker) -> std::io::Result<Kernel> {
        let mut cmd = Command::new(python);
        cmd.args(["-u", "-c", SCRIPT]);
        Self::start(cmd, python, dir, root, wake, false)
    }

    /// Starts a Jupyter kernel through the bridge, which runs on `python`
    /// and needs its `jupyter_client`.
    pub fn spawn_jupyter(
        python: &str,
        spec: &crate::kernelspec::Spec,
        dir: &Path,
        root: &Path,
        wake: Waker,
    ) -> std::io::Result<Kernel> {
        let cwd = if dir.is_dir() { dir } else { root };
        let config = json!({"spec": spec.json, "name": spec.name, "cwd": cwd,
            "resource_dir": spec.dir.as_ref().map(|d| d.to_string_lossy().into_owned())});
        let mut cmd = Command::new(python);
        cmd.args(["-u", "-c", BRIDGE, &config.to_string()]);
        Self::start(cmd, python, dir, root, wake, true)
    }

    fn start(mut cmd: Command, python: &str, dir: &Path, root: &Path, wake: Waker, jupyter: bool) -> std::io::Result<Kernel> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        crate::environment::apply(&mut cmd, root, Some(python));
        if dir.is_dir() {
            cmd.current_dir(dir);
        }
        if std::env::var_os("MPLBACKEND").is_none() {
            cmd.env("MPLBACKEND", "Agg");
        }
        let mut child = cmd.env("PYTHONIOENCODING", "utf-8").spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let (stdout, mut stderr) = (child.stdout.take().expect("piped stdout"), child.stderr.take().expect("piped"));
        let (tx, events) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(ev) = event(&line) {
                    // The process says why it ends; its end then needs no word.
                    let last = matches!(ev, Event::Dead(_) | Event::Missing(_));
                    if tx.send(ev).is_err() {
                        return;
                    }
                    wake();
                    if last {
                        return;
                    }
                }
            }
            let mut last = String::new();
            let _ = stderr.read_to_string(&mut last);
            let _ = tx.send(Event::Dead(last.trim().lines().last().unwrap_or("").to_string()));
            wake();
        });
        Ok(Kernel { child: Some(child), stdin, events, jupyter })
    }

    fn send(&mut self, msg: Value) {
        let _ = writeln!(self.stdin, "{msg}").and_then(|()| self.stdin.flush());
    }

    pub fn execute(&mut self, id: u64, code: &str) {
        self.send(json!({"op": "execute", "id": id, "code": code}));
    }

    /// Asks for completions at a 1-based line and a column counted in
    /// characters. `live` completes against what the cells have defined.
    pub fn complete(&mut self, id: u64, code: &str, line: usize, col: usize, live: bool, path: Option<&Path>) {
        let path = path.map(|p| p.to_string_lossy().into_owned());
        self.send(
            json!({"op": "complete", "id": id, "code": code, "line": line, "col": col, "live": live, "path": path}),
        );
    }

    /// Asks what is known of the name that ends at a place, given as for
    /// `complete`. For a call, `paren` is the column just inside its bracket.
    #[allow(clippy::too_many_arguments)]
    pub fn info(
        &mut self,
        id: u64,
        code: &str,
        line: usize,
        col: usize,
        live: bool,
        path: Option<&Path>,
        paren: Option<usize>,
    ) {
        let path = path.map(|p| p.to_string_lossy().into_owned());
        self.send(json!({"op": "info", "id": id, "code": code, "line": line, "col": col, "live": live, "path": path,
            "paren": paren}));
    }

    /// Stops the running cell, as Ctrl+C would.
    pub fn interrupt(&mut self) {
        if self.jupyter {
            self.send(json!({"op": "interrupt"}));
        } else if let Some(child) = &self.child {
            let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
        }
    }

    /// Answers the cell's request for input.
    pub fn input(&mut self, value: &str) {
        self.send(json!({"op": "input", "value": value}));
    }

    pub fn is_jupyter(&self) -> bool {
        self.jupyter
    }

    pub fn poll(&mut self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Blocks until the next event. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn wait(&mut self) -> Option<Event> {
        self.events.recv_timeout(std::time::Duration::from_secs(30)).ok()
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        if !self.jupyter {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        // A Jupyter kernel is asked to shut down, so it does not outlive
        // the bridge; the bridge is killed if it takes too long.
        self.send(json!({"op": "shutdown"}));
        std::thread::spawn(move || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
            while std::time::Instant::now() < until {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn until_done(k: &mut Kernel, id: u64) -> Vec<Event> {
        let mut seen = Vec::new();
        while let Some(ev) = k.wait() {
            let done = matches!(ev, Event::Done { id: i, .. } | Event::Matches { id: i, .. } | Event::Info { id: i, .. } if i == id);
            seen.push(ev);
            if done {
                break;
            }
        }
        seen
    }

    #[test]
    fn cells_run_and_names_complete() {
        let Ok(mut k) = Kernel::spawn("python3", Path::new("."), Arc::new(|| {})) else { return };
        k.execute(1, "import math\nx = 6\nprint('hi')\nx * 7");
        let seen = until_done(&mut k, 1);
        assert!(seen.iter().any(|e| matches!(e, Event::Stream { text, err: false, .. } if text.contains("hi"))));
        assert!(seen.iter().any(|e| matches!(e, Event::Show { data, result: true, .. } if plain(data) == "42")));
        assert!(matches!(seen.last(), Some(Event::Done { count: 1, ok: true, .. })));

        k.execute(3, "class Frame:\n    def _repr_html_(self): return '<table><tr><td>42</td></tr></table>'\nFrame()");
        let seen = until_done(&mut k, 3);
        assert!(seen.iter().any(|e| matches!(e, Event::Show { data, .. } if data["text/html"].as_str().is_some_and(|h| h.contains("<td>42</td>")))));
        k.execute(2, "def f():\n    return 1 / 0\nf()");
        let seen = until_done(&mut k, 2);
        let trace = seen.iter().find_map(|e| match e {
            Event::Error { name, trace, .. } if name == "ZeroDivisionError" => Some(trace.join("\n")),
            _ => None,
        });
        assert!(trace.is_some_and(|t| t.contains("return 1 / 0") && !t.contains("<string>")));

        k.complete(3, "math.sq", 1, 7, true, None);
        let seen = until_done(&mut k, 3);
        assert!(matches!(seen.last(), Some(Event::Matches { items, .. }) if items.iter().any(|i| i.0 == "sqrt")));
        // Without a live namespace, the file's own imports are followed.
        k.complete(4, "import os.path as p\np.jo", 2, 4, false, None);
        let seen = until_done(&mut k, 4);
        assert!(matches!(seen.last(), Some(Event::Matches { items, .. }) if items.iter().any(|i| i.0 == "join")));
    }

    fn info_of(k: &mut Kernel, id: u64, code: &str, col: usize, live: bool, call: bool) -> Option<Info> {
        let line = code.matches('\n').count() + 1;
        k.info(id, code, line, col, live, None, call.then_some(col + 1));
        match until_done(k, id).pop() {
            Some(Event::Info { info, .. }) => info,
            _ => None,
        }
    }

    #[test]
    fn names_are_documented() {
        let Ok(mut k) = Kernel::spawn("python3", Path::new("."), Arc::new(|| {})) else { return };
        // A builtin, with each parameter's place in the signature.
        let i = info_of(&mut k, 1, "len", 3, false, false).unwrap();
        assert!(i.signature.starts_with("len(") && i.doc.contains("number of items"));
        let (p, range) = &i.params[0];
        assert_eq!(&i.signature[range.unwrap().0..range.unwrap().1], p);
        // A library function, through the file's imports.
        let i = info_of(&mut k, 2, "import os.path as op\nop.join", 7, false, false).unwrap();
        assert!(i.signature.starts_with("join(") && i.module == "posixpath");
        // The file's own definitions, though they are never run.
        let code = "def add(x, y=1):\n    \"\"\"Adds them.\"\"\"\nadd(";
        let i = info_of(&mut k, 3, code, 3, false, true).unwrap();
        assert_eq!((i.signature.as_str(), i.doc.as_str()), ("add(x, y=1)", "Adds them."));
        assert_eq!(i.params.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["x", "y=1"]);
        // Keywords and unknown names have nothing to say.
        assert_eq!(info_of(&mut k, 4, "for", 3, false, false), None);
        assert_eq!(info_of(&mut k, 5, "nowhere", 7, false, false), None);
        // In a notebook, what the cells defined.
        k.execute(6, "z = 5");
        until_done(&mut k, 6);
        assert_eq!(info_of(&mut k, 7, "z", 1, true, false).unwrap().signature, "z: int");
    }
}

#[cfg(test)]
mod project_execution_tests {
    use super::*;
    #[test]
    fn project_env_variables_and_cell_timing_work_from_subfolders() {
        let root = std::env::temp_dir().join(format!("ld-kernel-env-{}", std::process::id()));
        std::fs::create_dir_all(root.join("notebooks")).unwrap();
        std::fs::write(root.join(".env"), "LD_KERNEL_FIXTURE=from_project\n").unwrap();
        let mut k = Kernel::spawn_in("python3", &root.join("notebooks"), &root, Arc::new(|| {})).unwrap();
        k.execute(42, "import os\nvalues = [1, 2, 3]\nprint(os.environ['LD_KERNEL_FIXTURE'])");
        let mut variables = None;
        let mut timing = None;
        let mut output = String::new();
        loop {
            match k.wait().expect("kernel event") {
                Event::Stream { text, .. } => output += &text,
                Event::Variables(items) => variables = Some(items),
                Event::Timing { id: 42, elapsed_ms, ran_at } => timing = Some((elapsed_ms, ran_at)),
                Event::Done { id: 42, ok, .. } => {
                    assert!(ok);
                    break;
                }
                _ => {}
            }
        }
        assert!(output.contains("from_project"));
        assert!(variables.unwrap().contains(&("values".into(), "list".into(), "length 3".into())));
        assert!(timing.is_some_and(|(ms, at)| ms >= 0.0 && at > 0.0));
        drop(k);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
pub(crate) mod jupyter_tests {
    use super::*;

    /// An interpreter with jupyter_client and ipykernel: the one
    /// `LIGHTDOWN_TEST_JUPYTER_PYTHON` names, or `python3` if it has them.
    pub fn jupyter_python() -> Option<String> {
        let python = std::env::var("LIGHTDOWN_TEST_JUPYTER_PYTHON").unwrap_or_else(|_| "python3".into());
        let ok = crate::kernelspec::has_package(&python, "jupyter_client")
            && crate::kernelspec::has_package(&python, "ipykernel");
        ok.then_some(python)
    }

    #[test]
    fn bridge_lines_become_events() {
        let show = event(r#"{"type":"display","id":3,"data":{"text/plain":"x","text/markdown":"**x**"},"metadata":{},"display_id":"d1","update":true}"#);
        match show {
            Some(Event::Show { id: 3, result: false, data, display_id: Some(d), update: true, .. }) => {
                assert_eq!((plain(&data).as_str(), d.as_str()), ("x", "d1"));
                assert_eq!(data["text/markdown"], "**x**");
            }
            _ => panic!("not a display"),
        }
        // The built-in helper's older form still reads.
        match event(r#"{"type":"result","id":1,"text":"42","png":"AAAA"}"#) {
            Some(Event::Show { result: true, data, display_id: None, update: false, .. }) => {
                assert_eq!((data["text/plain"].as_str(), data["image/png"].as_str()), (Some("42"), Some("AAAA")));
            }
            _ => panic!("not a result"),
        }
        assert!(matches!(event(r#"{"type":"clear","id":2,"wait":true}"#), Some(Event::Clear { id: 2, wait: true })));
        assert!(matches!(event(r#"{"type":"input","id":2,"prompt":"Name? ","password":false}"#),
            Some(Event::Input { id: 2, prompt, password: false }) if prompt == "Name? "));
        assert!(matches!(event(r#"{"type":"missing","text":"no jupyter_client"}"#), Some(Event::Missing(t)) if t.contains("jupyter")));
        assert!(matches!(event(r#"{"type":"fatal","text":"The kernel died"}"#), Some(Event::Dead(t)) if t == "The kernel died"));
        assert!(matches!(event(r#"{"type":"ready","version":"3.12","language_info":{"name":"python"}}"#),
            Some(Event::Ready(v, Some(info))) if v == "3.12" && info["name"] == "python"));
    }

    fn until(k: &mut Kernel, mut stop: impl FnMut(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        while let Some(ev) = k.wait() {
            let done = stop(&ev);
            seen.push(ev);
            if done {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_jupyter_kernel_runs_cells_asks_for_input_and_completes() {
        let Some(python) = jupyter_python() else { return };
        let spec = crate::kernelspec::ipykernel(&python);
        let dir = std::env::temp_dir();
        let mut k = Kernel::spawn_jupyter(&python, &spec, &dir, &dir, Arc::new(|| {})).unwrap();
        let done = |id| move |e: &Event| matches!(e, Event::Done { id: i, .. } if *i == id);
        k.execute(1, "from IPython.display import display, Markdown\nh = display(Markdown('**a**'), display_id=True)\nh.update(Markdown('b'))\nprint('hi')\n6 * 7");
        let seen = until(&mut k, done(1));
        assert!(seen.iter().any(|e| matches!(e, Event::Ready(_, Some(info)) if info["name"] == "python")));
        assert!(seen.iter().any(|e| matches!(e, Event::Show { update: true, data, display_id: Some(_), .. } if data["text/markdown"] == "b")));
        assert!(seen.iter().any(|e| matches!(e, Event::Stream { text, .. } if text == "hi\n")));
        assert!(seen.iter().any(|e| matches!(e, Event::Show { result: true, data, .. } if plain(data) == "42")));
        assert!(seen.iter().any(|e| matches!(e, Event::Variables(v) if v.iter().any(|v| v.0 == "h"))));
        assert!(matches!(seen.last(), Some(Event::Done { count: 1, ok: true, .. })));

        k.execute(2, "name = input('Who? ')\nname.upper()");
        let seen = until(&mut k, |e| matches!(e, Event::Input { .. }));
        assert!(matches!(seen.last(), Some(Event::Input { id: 2, prompt, .. }) if prompt == "Who? "));
        k.input("ada");
        let seen = until(&mut k, done(2));
        assert!(seen.iter().any(|e| matches!(e, Event::Show { data, .. } if plain(data) == "'ADA'")));

        k.execute(3, "1 / 0");
        let seen = until(&mut k, done(3));
        let trace = seen.iter().find_map(|e| match e {
            Event::Error { name, trace, .. } if name == "ZeroDivisionError" => Some(trace.join("\n")),
            _ => None,
        });
        assert!(trace.is_some_and(|t| t.contains('\u{1b}')), "tracebacks keep their colours");
        assert!(matches!(seen.last(), Some(Event::Done { ok: false, .. })));

        k.execute(4, "import time\ntime.sleep(30)");
        std::thread::sleep(std::time::Duration::from_millis(800));
        k.interrupt();
        let seen = until(&mut k, done(4));
        assert!(seen.iter().any(|e| matches!(e, Event::Error { name, .. } if name == "KeyboardInterrupt")));

        k.complete(5, "import math\nmath.sq", 2, 7, true, None);
        let seen = until(&mut k, |e| matches!(e, Event::Matches { id: 5, .. }));
        assert!(matches!(seen.last(), Some(Event::Matches { items, .. }) if items.iter().any(|i| i.0 == "sqrt")));
        k.info(6, "len", 1, 3, true, None, None);
        let seen = until(&mut k, |e| matches!(e, Event::Info { id: 6, .. }));
        assert!(matches!(seen.last(), Some(Event::Info { info: Some(i), .. }) if i.signature.starts_with("len(") && !i.doc.is_empty()));
    }

    #[test]
    fn a_missing_jupyter_client_is_reported() {
        // An interpreter without jupyter_client: the system one, isolated from user packages.
        if crate::kernelspec::has_package("python3", "jupyter_client") {
            return;
        }
        let spec = crate::kernelspec::ipykernel("python3");
        let dir = std::env::temp_dir();
        let Ok(mut k) = Kernel::spawn_jupyter("python3", &spec, &dir, &dir, Arc::new(|| {})) else { return };
        assert!(matches!(k.wait(), Some(Event::Missing(t)) if t.contains("jupyter_client")));
    }
}
