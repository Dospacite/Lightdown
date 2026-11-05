//! OpenAI Codex as an agent: `codex app-server`, spoken to in JSON-RPC over
//! stdio. A thread is started or resumed, each message starts a turn, and
//! the turn's items (messages, reasoning, commands, file changes, MCP calls)
//! stream back as notifications. Codex asks before commands and file
//! changes its sandbox does not allow by itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::outside_agent::{Event, line, short};
use crate::tools::Class;

pub fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new("codex");
    cmd.arg("app-server").current_dir(cwd);
    cmd
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn initialize(id: u64) -> Value {
    let info = json!({"name": "lightdown", "title": "Lightdown", "version": env!("CARGO_PKG_VERSION")});
    request(id, "initialize", json!({"clientInfo": info, "capabilities": {"experimentalApi": true}}))
}

pub fn initialized() -> Value {
    json!({"jsonrpc": "2.0", "method": "initialized"})
}

pub fn model_list(id: u64) -> Value {
    request(id, "model/list", json!({}))
}

/// How a turn may act, from the project's permissions and plan mode.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub plan: bool,
    /// The project lets the agent edit without asking.
    pub edit_free: bool,
    pub run_free: bool,
    pub edit_denied: bool,
    pub web_denied: bool,
}

impl Policy {
    /// The sandbox: read-only in plan mode or when editing is denied, else
    /// writes inside the project.
    pub fn sandbox(&self) -> Value {
        if self.plan || self.edit_denied { json!({"type": "readOnly"}) } else { json!({"type": "workspaceWrite"}) }
    }

    /// When Codex asks: never in plan mode, where nothing may be escalated;
    /// for anything not known safe unless the project allows it all.
    pub fn approval(&self) -> &'static str {
        match self {
            Policy { plan: true, .. } => "never",
            Policy { edit_free: true, run_free: true, .. } => "on-request",
            _ => "untrusted",
        }
    }

    fn config(&self) -> Value {
        if self.web_denied { json!({"web_search": "disabled"}) } else { Value::Null }
    }
}

/// One `codex app-server` and what Lightdown has asked of it.
pub struct Client {
    next: u64,
    /// Requests sent and not yet answered, by id: their method.
    pending: HashMap<u64, String>,
    cwd: PathBuf,
    /// The turn in progress; notifications of others are stale.
    pub turn: String,
    stopped: HashSet<String>,
    /// A thread whose turn was stopped before Codex named the turn.
    cancel: Option<String>,
    /// Agent messages that have streamed, so their completion is not
    /// shown twice.
    streamed: HashSet<String>,
    /// What each item does, by id.
    items: HashMap<String, (String, Class)>,
    /// Token totals so far, to report each update's share.
    total: u64,
    pub models: Option<Value>,
}

impl Client {
    pub fn new(cwd: &Path) -> Client {
        Client {
            next: 1,
            pending: HashMap::new(),
            cwd: cwd.into(),
            turn: String::new(),
            stopped: HashSet::new(),
            cancel: None,
            streamed: HashSet::new(),
            items: HashMap::new(),
            total: 0,
            models: None,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.pending.insert(id, method.into());
        request(id, method, params)
    }

    /// What starts the conversation: the handshake and the model list.
    pub fn hello(&mut self) -> Vec<Value> {
        vec![
            self.call("initialize", initialize(0)["params"].clone()),
            initialized(),
            self.call("model/list", json!({})),
        ]
    }

    pub fn start_thread(&mut self, cwd: &Path, policy: &Policy) -> Value {
        let mut p = json!({
            "cwd": cwd, "sandbox": if policy.plan || policy.edit_denied { "read-only" } else { "workspace-write" },
            "approvalPolicy": policy.approval(), "developerInstructions": crate::outside_agent::NOTE,
        });
        if !policy.config().is_null() {
            p["config"] = policy.config();
        }
        self.call("thread/start", p)
    }

    pub fn resume_thread(&mut self, id: &str, cwd: &Path, policy: &Policy) -> Value {
        let mut p = json!({
            "threadId": id, "cwd": cwd, "approvalPolicy": policy.approval(), "excludeTurns": true,
            "developerInstructions": crate::outside_agent::NOTE,
        });
        if !policy.config().is_null() {
            p["config"] = policy.config();
        }
        self.call("thread/resume", p)
    }

    pub fn start_turn(
        &mut self,
        thread: &str,
        text: &str,
        images: &[crate::images::Image],
        model: &str,
        effort: &str,
        policy: &Policy,
    ) -> Value {
        let mut input = vec![json!({"type": "text", "text": text, "text_elements": []})];
        for image in images {
            input.push(json!({"type": "image", "url": format!("data:image/png;base64,{}", image.data)}));
        }
        let mut p = json!({
            "threadId": thread, "input": input, "model": model, "cwd": self.cwd, "summary": "detailed",
            "approvalPolicy": policy.approval(), "sandboxPolicy": policy.sandbox(),
        });
        if !effort.is_empty() {
            p["effort"] = effort.into();
        }
        self.turn.clear();
        self.cancel = None;
        self.call("turn/start", p)
    }

    /// Stops the turn in progress. What it still reports is dropped.
    pub fn interrupt(&mut self, thread: &str) -> Option<Value> {
        let turn = std::mem::take(&mut self.turn);
        if turn.is_empty() {
            self.cancel = Some(thread.into());
            return None;
        }
        self.stopped.insert(turn.clone());
        Some(self.call("turn/interrupt", json!({"threadId": thread, "turnId": turn})))
    }

    /// The answer to one of Codex's requests.
    pub fn decide(request: &Value, allow: bool, cancel: bool) -> Value {
        let decision = match (allow, cancel) {
            (true, _) => "accept",
            (false, true) => "cancel",
            (false, false) => "decline",
        };
        json!({"jsonrpc": "2.0", "id": request, "result": {"decision": decision}})
    }

    fn describe(&self, item: &Value) -> Option<(String, Class)> {
        let text = |k: &str| item[k].as_str().unwrap_or("").to_string();
        Some(match item["type"].as_str()? {
            "commandExecution" => {
                let shown = item["commandActions"]
                    .as_array()
                    .filter(|a| a.len() == 1)
                    .and_then(|a| a[0]["command"].as_str())
                    .map_or_else(|| text("command"), String::from);
                (line(&format!("run {shown}")), Class::Run)
            }
            "fileChange" => {
                let paths: Vec<String> = item["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| short(&self.cwd, c["path"].as_str().unwrap_or("")))
                    .collect();
                (
                    line(&format!("edit {}", if paths.is_empty() { "files".into() } else { paths.join(", ") })),
                    Class::Edit,
                )
            }
            "mcpToolCall" => (format!("{}: {}", text("server"), text("tool")), Class::Run),
            "dynamicToolCall" => (text("tool"), Class::Run),
            "webSearch" => (line(&format!("web search {}", text("query"))), Class::Web),
            "imageView" => (format!("view {}", short(&self.cwd, &text("path"))), Class::Read),
            _ => return None,
        })
    }

    /// What one line from Codex means.
    pub fn line(&mut self, v: &Value) -> Vec<Event> {
        let mut out = Vec::new();
        let method = v["method"].as_str().unwrap_or("");
        let p = &v["params"];
        if v.get("id").is_some() && !method.is_empty() {
            self.request(v, method, &mut out);
        } else if let Some(id) = v["id"].as_u64() {
            self.response(id, v, &mut out);
        } else {
            if let Some(turn) = p["turnId"].as_str().or(p["turn"]["id"].as_str()) {
                if self.stopped.contains(turn) || (!self.turn.is_empty() && turn != self.turn) {
                    return out;
                }
            }
            self.notification(method, p, &mut out);
        }
        out
    }

    fn request(&mut self, v: &Value, method: &str, out: &mut Vec<Event>) {
        let p = &v["params"];
        let id = p["itemId"].as_str().unwrap_or("").to_string();
        let known = self.items.get(&id).cloned();
        let ask = match method {
            "item/commandExecution/requestApproval" => {
                let what = p["command"].as_str().map(|c| line(&format!("run {c}")));
                Some((what.or(known.map(|k| k.0)).unwrap_or_else(|| "run a command".into()), Class::Run))
            }
            "item/fileChange/requestApproval" => {
                Some((known.map_or_else(|| "change files".into(), |k| k.0), Class::Edit))
            }
            "item/permissions/requestApproval" => {
                let why = p["reason"].as_str().unwrap_or("more access");
                Some((line(&format!("grant {why}")), Class::Run))
            }
            _ => None,
        };
        match ask {
            Some((what, class)) => out.push(Event::Ask { request: v["id"].clone(), id, what, class, input: p.clone() }),
            None => out.push(Event::Send(json!({"jsonrpc": "2.0", "id": v["id"],
                "error": {"code": -32601, "message": format!("Lightdown does not handle {method}")}}))),
        }
    }

    fn response(&mut self, id: u64, v: &Value, out: &mut Vec<Event>) {
        let Some(method) = self.pending.remove(&id) else { return };
        let error = v["error"]["message"].as_str().map(String::from);
        match (method.as_str(), error) {
            ("thread/start" | "thread/resume", None) => {
                out.extend(v["result"]["thread"]["id"].as_str().map(|t| Event::Session(t.into())));
            }
            ("thread/resume", Some(e)) => out.push(Event::Lost(e)),
            ("thread/start", Some(e)) => out.push(Event::Done(Some(format!("Codex could not start a thread: {e}")))),
            ("turn/start", None) => {
                if let Some(t) = v["result"]["turn"]["id"].as_str() {
                    if let Some(thread) = self.cancel.take() {
                        self.stopped.insert(t.into());
                        let stop = self.call("turn/interrupt", json!({"threadId": thread, "turnId": t}));
                        return out.push(Event::Send(stop));
                    }
                    self.turn = t.into();
                    out.push(Event::Turn(t.into()));
                }
            }
            ("turn/start", Some(e)) => out.push(Event::Done(Some(format!("Codex could not start the turn: {e}")))),
            ("model/list", None) => self.models = Some(v["result"].clone()),
            ("initialize", Some(e)) => out.push(Event::Done(Some(format!("Codex did not start: {e}")))),
            _ => {}
        }
    }

    fn notification(&mut self, method: &str, p: &Value, out: &mut Vec<Event>) {
        let item = &p["item"];
        let item_id = item["id"].as_str().unwrap_or("").to_string();
        match method {
            "item/started" => {
                if let Some((what, class)) = self.describe(item) {
                    self.items.insert(item_id.clone(), (what.clone(), class));
                    out.push(Event::Tool { id: item_id, what });
                }
            }
            "item/agentMessage/delta" => {
                if let Some(d) = p["delta"].as_str().filter(|d| !d.is_empty()) {
                    self.streamed.insert(p["itemId"].as_str().unwrap_or("").into());
                    out.push(Event::Text(d.into()));
                }
            }
            "item/reasoning/summaryTextDelta" => {
                out.extend(p["delta"].as_str().filter(|d| !d.is_empty()).map(|d| Event::Reasoning(d.into())));
            }
            "item/reasoning/summaryPartAdded" => out.push(Event::Reasoning("\n\n".into())),
            "item/completed" => match item["type"].as_str().unwrap_or("") {
                "agentMessage" if !self.streamed.contains(&item_id) => {
                    out.extend(item["text"].as_str().filter(|t| !t.is_empty()).map(|t| Event::Text(t.into())));
                }
                _ => {
                    let Some((what, class)) = self.describe(item) else { return };
                    let status = item["status"].as_str().unwrap_or("completed");
                    let (result, ok) = match item["type"].as_str().unwrap_or("") {
                        "commandExecution" => {
                            let code = item["exitCode"].as_i64();
                            let mut r = item["aggregatedOutput"].as_str().unwrap_or("").to_string();
                            if let Some(c) = code.filter(|c| *c != 0) {
                                r += &format!("\n(exit code {c})");
                            }
                            (r, status == "completed" && code.is_none_or(|c| c == 0))
                        }
                        "fileChange" => {
                            let diffs: Vec<String> = item["changes"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|c| {
                                    format!(
                                        "{}\n{}",
                                        c["path"].as_str().unwrap_or(""),
                                        c["diff"].as_str().unwrap_or("")
                                    )
                                })
                                .collect();
                            (diffs.join("\n"), status == "completed")
                        }
                        "mcpToolCall" => match item["error"]["message"].as_str() {
                            Some(e) => (e.to_string(), false),
                            None => (item["result"].to_string(), status == "completed"),
                        },
                        _ => (String::new(), status != "failed" && status != "declined"),
                    };
                    let changes = matches!(class, Class::Edit | Class::Run);
                    out.push(Event::ToolDone { id: item_id, what, result, ok, changes });
                }
            },
            "thread/tokenUsage/updated" => {
                let u = &p["tokenUsage"];
                let total = u["total"]["totalTokens"].as_u64().unwrap_or(0);
                let tokens = total.checked_sub(self.total).unwrap_or(total);
                self.total = total;
                out.push(Event::Usage {
                    cost: 0.0,
                    tokens,
                    filled: u["last"]["totalTokens"].as_u64().unwrap_or(0),
                    context: u["modelContextWindow"].as_u64().unwrap_or(0),
                });
            }
            "turn/completed" => {
                let turn = &p["turn"];
                let failed = turn["status"] == "failed";
                let why = turn["error"]["message"].as_str().unwrap_or("The turn failed").to_string();
                self.turn.clear();
                out.push(Event::Done(failed.then_some(why)));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CWD: &str = "/tmp/project";

    #[test]
    fn a_recorded_turn_reads_as_text_tools_and_usage() {
        let mut c = Client::new(Path::new(CWD));
        let hello = c.hello();
        assert_eq!(hello[0]["method"], "initialize");
        let policy = Policy { plan: false, edit_free: false, run_free: false, edit_denied: false, web_denied: false };
        let start = c.start_thread(Path::new(CWD), &policy);
        assert_eq!(start["id"], 3);
        let turn = c.start_turn("t", "hi", &[], "gpt-6-luna", "low", &policy);
        assert_eq!(turn["id"], 4);
        let mut events = Vec::new();
        for l in include_str!("../tests/fixtures/codex_turn.jsonl").lines().filter(|l| !l.trim().is_empty()) {
            events.extend(c.line(&serde_json::from_str(l).unwrap()));
        }
        assert_eq!(events[0], Event::Session("01a1ad38-cfc7-7d31-bfdd-5ea640f63d4f".into()));
        assert_eq!(events[1], Event::Turn("01a1ad38-d007-7c93-b456-228798a30a25".into()));
        let what = "run cat notes.txt && printf '\\nthird line\\n' >> notes.txt && wc -l notes.txt";
        assert!(matches!(&events[2], Event::Tool { what: w, .. } if w == what));
        let Event::Ask { request, class, .. } = &events[3] else { panic!("{:?}", events[3]) };
        assert_eq!((request, class), (&json!(0), &Class::Run));
        assert_eq!(Client::decide(request, true, false)["result"]["decision"], "accept");
        assert!(
            matches!(&events[4], Event::ToolDone { ok: true, changes: true, result, .. } if result.contains("4 notes.txt"))
        );
        let text: String =
            events.iter().filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(text, "Appended `third line`. `wc -l notes.txt` reports **4** lines.");
        let usage: Vec<_> = events.iter().filter(|e| matches!(e, Event::Usage { .. })).collect();
        assert_eq!(usage.len(), 2);
        assert!(matches!(usage[1], Event::Usage { tokens: 15222, filled: 15222, context: 258400, .. }));
        assert_eq!(events.last(), Some(&Event::Done(None)));
    }

    #[test]
    fn stopped_turns_are_dropped_and_requests_answered() {
        let mut c = Client::new(Path::new(CWD));
        c.turn = "a".into();
        let stop = c.interrupt("t").unwrap();
        assert_eq!(stop["params"], json!({"threadId": "t", "turnId": "a"}));
        let late = json!({"method": "item/agentMessage/delta", "params": {"turnId": "a", "itemId": "m", "delta": "x"}});
        assert!(c.line(&late).is_empty());
        let odd = json!({"id": 9, "method": "item/tool/requestUserInput", "params": {}});
        assert!(matches!(&c.line(&odd)[0], Event::Send(v) if v["error"]["code"] == -32601));
        let patch = json!({"method": "item/started", "params": {"item": {"type": "fileChange", "id": "f",
            "changes": [{"path": "/tmp/project/a.py", "diff": "+x", "kind": {"type": "update"}}], "status": "inProgress"}}});
        assert_eq!(c.line(&patch), [Event::Tool { id: "f".into(), what: "edit a.py".into() }]);
        let ask = json!({"id": 5, "method": "item/fileChange/requestApproval", "params": {"itemId": "f"}});
        assert!(matches!(&c.line(&ask)[0], Event::Ask { what, class: Class::Edit, .. } if what == "edit a.py"));
        assert_eq!(Client::decide(&json!(5), false, true)["result"]["decision"], "cancel");
        let plan = Policy { plan: true, edit_free: true, run_free: true, edit_denied: false, web_denied: true };
        assert_eq!((plan.approval(), plan.sandbox()["type"].as_str()), ("never", Some("readOnly")));
        assert_eq!(c.start_thread(Path::new(CWD), &plan)["params"]["config"]["web_search"], "disabled");
        let free = Policy { plan: false, ..plan };
        assert_eq!((free.approval(), free.sandbox()["type"].as_str()), ("on-request", Some("workspaceWrite")));
        let bad = json!({"id": c.next - 1, "error": {"message": "no rollout"}});
        assert!(matches!(&c.line(&bad)[0], Event::Done(Some(_))));
    }
}
