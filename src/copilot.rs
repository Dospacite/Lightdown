//! Copilot owns its authentication storage. Lightdown only speaks JSON-RPC.
use crate::{config, kernel::Waker, lsp, rpc};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn directory() -> PathBuf {
    config::data_dir().join("servers/copilot")
}
pub fn executable() -> PathBuf {
    directory().join("node_modules/.bin/copilot-language-server")
}
/// Called only after the installation confirmation in the editor.
pub fn install() -> Result<(), String> {
    let dir = directory();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let status = Command::new("npm")
        .args(["install", "--prefix"])
        .arg(&dir)
        .args(["--no-audit", "--no-fund", "--ignore-scripts", "@github/copilot-language-server"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Cannot run npm: {e}"))?;
    if !status.success() || !executable().is_file() {
        return Err(format!("Copilot installation failed ({status}); npm and Node.js are required"));
    }
    Ok(())
}
#[derive(Debug)]
pub enum Event {
    Message(String),
    Code(String, String),
    OpenUrl(String),
    Completion(u64, Value),
    Failed(String),
}
pub struct Client {
    process: rpc::Process,
    next: u64,
    ready: bool,
    waiting: Vec<Value>,
    pending: HashMap<u64, String>,
    documents: HashMap<String, (u64, String)>,
    pub status: String,
}
impl Client {
    pub fn start(root: &Path, wake: Waker) -> Result<Self, String> {
        let mut cmd = Command::new(executable());
        cmd.arg("--stdio").current_dir(root);
        let process = rpc::Process::start_framed(cmd, wake)?;
        let mut c = Self {
            process,
            next: 2,
            ready: false,
            waiting: Vec::new(),
            pending: HashMap::from([(1, "initialize".into())]),
            documents: HashMap::new(),
            status: "starting".into(),
        };
        c.process.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "processId":std::process::id(),"rootUri":lsp::uri(root),"workspaceFolders":[{"uri":lsp::uri(root),"name":"Lightdown"}],
            "capabilities":{"workspace":{"workspaceFolders":true},"window":{"showDocument":{"support":true}}},
            "initializationOptions":{"editorInfo":{"name":"Lightdown","version":env!("CARGO_PKG_VERSION")},
                "editorPluginInfo":{"name":"Lightdown","version":env!("CARGO_PKG_VERSION")}}}}));
        c.status = "starting".into();
        Ok(c)
    }
    fn send(&mut self, v: Value) {
        if self.ready {
            self.process.send(v);
        } else {
            self.waiting.push(v);
        }
    }
    pub fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        self.pending.insert(id, method.into());
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        id
    }
    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }
    pub fn cancel(&mut self, id: u64) {
        self.pending.remove(&id);
        self.waiting.retain(|v| v["id"].as_u64() != Some(id));
        self.notify("$/cancelRequest", json!({"id":id}));
    }
    pub fn sync(&mut self, uri: &str, language: &str, version: u64, text: &str) {
        match self.documents.get(uri) {
            None => self.notify(
                "textDocument/didOpen",
                json!({"textDocument":{"uri":uri,"languageId":language,"version":version,"text":text}}),
            ),
            Some((old_version, old)) if *old_version != version || old != text => {
                // The server requires incremental sync; replacing the whole old range is one valid incremental edit.
                let end = lsp::position(old, old.len());
                self.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"range":{"start":{"line":0,"character":0},"end":end},"text":text}]}));
            }
            _ => {}
        }
        self.documents.insert(uri.into(), (version, text.into()));
        self.notify("textDocument/didFocus", json!({"textDocument":{"uri":uri}}));
    }
    pub fn close_except(&mut self, uris: &[String]) {
        let closed: Vec<_> = self.documents.keys().filter(|uri| !uris.contains(uri)).cloned().collect();
        for uri in closed {
            self.documents.remove(&uri);
            self.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
        }
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        for value in self.process.poll() {
            match value {
                Ok(v) => self.receive(v, &mut out),
                Err(e) => {
                    self.status = "stopped".into();
                    out.push(Event::Failed(e));
                    break;
                }
            }
        }
        out
    }
    fn receive(&mut self, v: Value, out: &mut Vec<Event>) {
        if let Some(method) = v["method"].as_str() {
            let params = &v["params"];
            let result = match method {
                "didChangeStatus" => {
                    let msg = params["message"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| params["kind"].as_str().unwrap_or("ready"));
                    self.status = msg.chars().take(120).collect();
                    Value::Null
                }
                "window/showDocument" => {
                    if let Some(url) = params["uri"].as_str().filter(|s| s.starts_with("https://github.com/")) {
                        out.push(Event::OpenUrl(url.into()));
                        json!({"success":true})
                    } else {
                        json!({"success":false})
                    }
                }
                "workspace/configuration" => {
                    json!(params["items"].as_array().map(|a| vec![Value::Null; a.len()]).unwrap_or_default())
                }
                "window/showMessage" => {
                    if let Some(msg) = params["message"].as_str() {
                        out.push(Event::Message(msg.into()));
                    }
                    Value::Null
                }
                _ => Value::Null,
            };
            if let Some(id) = v.get("id") {
                self.process.send(json!({"jsonrpc":"2.0","id":id,"result":result}));
            }
            return;
        }
        let Some(id) = v["id"].as_u64() else { return };
        let Some(method) = self.pending.remove(&id) else { return };
        if let Some(error) = v.get("error") {
            let message = error["message"].as_str().unwrap_or("Copilot request failed");
            if method == "initialize" {
                out.push(Event::Failed(format!("Copilot initialize: {message}")));
                return;
            }
            out.push(Event::Message(format!("Copilot: {message}")));
            if method == "textDocument/inlineCompletion" {
                out.push(Event::Completion(id, Value::Null));
            }
            return;
        }
        let result = &v["result"];
        match method.as_str() {
            "initialize" => {
                self.process.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
                self.process.send(json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"telemetry":{"telemetryLevel":"off"}}}}));
                self.ready = true;
                self.status = "ready; sign in if needed".into();
                for queued in self.waiting.drain(..) {
                    self.process.send(queued);
                }
            }
            "signIn" => {
                if let Some(code) = result["userCode"].as_str() {
                    let url = result["verificationUri"].as_str().unwrap_or("https://github.com/login/device");
                    self.status = format!("sign-in code {code}");
                    out.push(Event::Code(code.into(), url.into()));
                    if result["command"]["command"].is_string() {
                        self.request("workspace/executeCommand", result["command"].clone());
                    } else {
                        out.push(Event::OpenUrl(url.into()));
                    }
                } else {
                    self.status = "signed in".into();
                    out.push(Event::Message("Copilot signed in".into()));
                }
            }
            "signOut" => {
                self.status = "signed out".into();
                out.push(Event::Message("Copilot signed out".into()));
            }
            "textDocument/inlineCompletion" => out.push(Event::Completion(id, result.clone())),
            _ => {}
        }
    }
}
/// Copilot ranges use UTF-16, including astral characters.
pub fn edit(text: &str, cur: usize, value: &Value) -> Option<(usize, usize, String, Value)> {
    let items = value["items"].as_array().or_else(|| value.as_array())?;
    let item = items.first()?;
    let insert = item["insertText"].as_str().or_else(|| item["insertText"]["value"].as_str())?.replace("\r\n", "\n");
    let (a, b) = if item.get("range").is_some() {
        (lsp::offset(text, &item["range"]["start"])?, lsp::offset(text, &item["range"]["end"])?)
    } else {
        (cur, cur)
    };
    if a > b || insert.len() > 16000 || insert.contains(crate::cells::MARK) {
        return None;
    }
    Some((a, b, insert, item.clone()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Downloads the Copilot server and starts device sign-in; run explicitly with scratch HOME/XDG directories"]
    fn live_install_and_start_sign_in() {
        install().expect("install Copilot");
        let root = std::env::temp_dir();
        let mut client = Client::start(&root, std::sync::Arc::new(|| {})).expect("start Copilot");
        client.request("signIn", json!({}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            for event in client.poll() {
                match event {
                    Event::Code(code, url) => {
                        assert!(!code.is_empty());
                        assert!(url.starts_with("https://github.com/"));
                        return;
                    }
                    Event::Failed(e) => panic!("Copilot stopped: {e}"),
                    Event::Message(e) => eprintln!("{e}"),
                    _ => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("Copilot did not start device sign-in within 60 seconds");
    }
    #[test]
    fn sync_cancel_and_device_flow_follow_the_protocol() {
        let process = rpc::Process::start_framed(Command::new("cat"), std::sync::Arc::new(|| {})).unwrap();
        let mut c = Client {
            process,
            next: 2,
            ready: false,
            waiting: Vec::new(),
            pending: HashMap::from([(1, "initialize".into())]),
            documents: HashMap::new(),
            status: String::new(),
        };
        c.sync("file:///a.py", "python", 1, "🦀x");
        c.sync("file:///a.py", "python", 2, "🦀y");
        assert_eq!(c.waiting.iter().filter(|v| v["method"] == "textDocument/didOpen").count(), 1);
        let change = c.waiting.iter().find(|v| v["method"] == "textDocument/didChange").unwrap();
        assert_eq!(change["params"]["contentChanges"][0]["range"]["end"]["character"], 3);
        c.close_except(&[]);
        assert!(c.documents.is_empty());
        let id = c.request("textDocument/inlineCompletion", json!({}));
        c.cancel(id);
        let mut events = Vec::new();
        c.receive(json!({"id":id,"result":{"items":[]}}), &mut events);
        assert!(events.is_empty());
        c.receive(json!({"id":1,"result":{"capabilities":{}}}), &mut events);
        assert!(c.ready && c.waiting.is_empty());
        let sign_in = c.request("signIn", json!({}));
        c.receive(json!({"id":sign_in,"result":{"userCode":"TEST-CODE","command":{"command":"github.copilot.finishDeviceFlow","arguments":[]}}}),&mut events);
        assert!(events.iter().any(|e| matches!(e,Event::Code(code,_) if code=="TEST-CODE")));
        assert!(c.pending.values().any(|m| m == "workspace/executeCommand"));
        c.receive(json!({"method":"didChangeStatus","params":{"kind":"Normal","message":"Ready"}}), &mut events);
        assert_eq!(c.status, "Ready");
    }
    #[test]
    fn completion_ranges_use_utf16_and_reject_invalid_ranges() {
        let v = json!({"items":[{"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":3}},"insertText":"é"}]});
        assert_eq!(edit("🦀x", 0, &v).map(|(a, b, t, _)| (a, b, t)), Some((4, 5, "é".into())));
        assert!(edit("🦀", 0, &v).is_none());
        assert!(edit("", 0, &Value::Null).is_none());
    }
}
