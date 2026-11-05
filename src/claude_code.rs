//! Claude Code as an agent: the `claude` program run with the stream-JSON
//! protocol of the Claude Agent SDK. Messages go in as JSON lines on stdin;
//! the session's messages, streamed deltas, permission requests and the
//! turn's result come back on stdout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::outside_agent::{Event, line, short};
use crate::tools::Class;

/// How a Claude Code process is started.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub model: String,
    /// An effort level, or empty for the model's own.
    pub effort: String,
    /// `default`, `acceptEdits` or `plan`. Never `bypassPermissions`.
    pub mode: &'static str,
    /// The session to resume, or a new session's id.
    pub session: String,
    pub resume: bool,
    /// Tools the project's settings forbid outright.
    pub disallowed: Vec<&'static str>,
    pub adaptive: bool,
    pub cwd: PathBuf,
}

impl Launch {
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
        ]
        .map(String::from)
        .to_vec();
        a.extend(["--model".into(), self.model.clone()]);
        if !self.effort.is_empty() {
            a.extend(["--effort".into(), self.effort.clone()]);
        }
        if self.adaptive {
            a.extend(["--thinking", "adaptive", "--thinking-display", "summarized"].map(String::from));
        }
        a.extend(["--permission-mode".into(), self.mode.into()]);
        a.extend([if self.resume { "--resume" } else { "--session-id" }.into(), self.session.clone()]);
        if !self.disallowed.is_empty() {
            a.push("--disallowedTools".into());
            a.extend(self.disallowed.iter().map(|t| t.to_string()));
        }
        a.extend(["--append-system-prompt".into(), crate::outside_agent::NOTE.into()]);
        a
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new("claude");
        cmd.args(self.args()).current_dir(&self.cwd).env("CLAUDE_CODE_ENTRYPOINT", "sdk-ts");
        cmd
    }
}

/// The tools a class covers, to forbid when the project denies it.
pub fn tools_of(class: Class) -> &'static [&'static str] {
    match class {
        Class::Edit => &["Edit", "MultiEdit", "Write", "NotebookEdit"],
        Class::Run => &["Bash"],
        Class::Web => &["WebFetch", "WebSearch"],
        Class::Read => &[],
    }
}

/// Which of the project's permissions a tool needs.
pub fn class_of(tool: &str) -> Class {
    match tool {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => Class::Edit,
        "WebFetch" | "WebSearch" => Class::Web,
        "Read" | "Glob" | "Grep" | "LS" | "TodoWrite" | "ExitPlanMode" | "EnterPlanMode" | "ToolSearch" => Class::Read,
        _ => Class::Run,
    }
}

/// What a tool call does, in a few words.
pub fn describe(cwd: &Path, tool: &str, input: &Value) -> String {
    let text = |key: &str| input[key].as_str().unwrap_or("").to_string();
    let path = |key: &str| short(cwd, &text(key));
    match tool {
        "Bash" => line(&format!("run {}", text("command"))),
        "Read" => format!("read {}", path("file_path")),
        "Edit" | "MultiEdit" => format!("edit {}", path("file_path")),
        "Write" => format!("write {}", path("file_path")),
        "NotebookEdit" => format!("edit notebook {}", path("notebook_path")),
        "Glob" => line(&format!("find files {}", text("pattern"))),
        "Grep" => line(&format!("search {}", text("pattern"))),
        "LS" => format!("list {}", path("path")),
        "WebFetch" => line(&format!("fetch {}", text("url"))),
        "WebSearch" => line(&format!("web search {}", text("query"))),
        "Task" | "Agent" => line(&format!("agent: {}", text("description"))),
        "TodoWrite" => "update the to-do list".into(),
        "ExitPlanMode" => "propose the plan".into(),
        other => match other.strip_prefix("mcp__").and_then(|t| t.split_once("__")) {
            Some((server, name)) => format!("{server}: {name}"),
            None => other.to_string(),
        },
    }
}

/// The text of a tool result, whichever form it takes.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p["type"].as_str() {
                Some("text") => p["text"].as_str().unwrap_or("").to_string(),
                Some("image") => "[image]".into(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ---- what is sent ---------------------------------------------------------

pub fn initialize(id: &str) -> Value {
    json!({"type": "control_request", "request_id": id, "request": {"subtype": "initialize"}})
}

pub fn interrupt(id: &str) -> Value {
    json!({"type": "control_request", "request_id": id, "request": {"subtype": "interrupt"}})
}

/// A user message, with screenshots as PNG images.
pub fn user(text: &str, images: &[crate::images::Image], session: &str) -> Value {
    let mut content = vec![json!({"type": "text", "text": text})];
    for image in images {
        content.push(
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": image.data}}),
        );
    }
    json!({
        "type": "user", "session_id": session, "parent_tool_use_id": null,
        "message": {"role": "user", "content": content},
    })
}

/// The answer to a permission request.
pub fn decide(request: &Value, allow: bool, input: &Value, why: &str) -> Value {
    let response = if allow {
        json!({"behavior": "allow", "updatedInput": input})
    } else {
        json!({"behavior": "deny", "message": why})
    };
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request, "response": response}})
}

fn refuse(request: &Value, why: &str) -> Value {
    json!({"type": "control_response", "response": {"subtype": "error", "request_id": request, "error": why}})
}

// ---- what comes back ------------------------------------------------------

#[derive(Default)]
struct Block {
    tool: Option<(String, String)>,
    json: String,
}

/// Reads one Claude Code process's messages.
pub struct Parser {
    cwd: PathBuf,
    /// Content blocks of the message streaming in, by index.
    blocks: HashMap<u64, Block>,
    /// Tools called, by id: name and what they do.
    tools: HashMap<String, (String, String)>,
    /// What the process has cost so far; its results report the sum.
    spent: f64,
    /// Models listed in the answer to `initialize`.
    pub models: Option<Value>,
}

impl Parser {
    pub fn new(cwd: &Path) -> Parser {
        Parser { cwd: cwd.into(), blocks: HashMap::new(), tools: HashMap::new(), spent: 0.0, models: None }
    }

    pub fn line(&mut self, v: &Value) -> Vec<Event> {
        let mut out = Vec::new();
        // What a subagent does is its own business.
        let nested = !v["parent_tool_use_id"].is_null();
        match v["type"].as_str().unwrap_or("") {
            "system" if v["subtype"] == "init" => {
                out.extend(v["session_id"].as_str().map(|s| Event::Session(s.into())));
            }
            "stream_event" if !nested => self.stream(&v["event"], &mut out),
            "assistant" if !nested => {
                if let Some(e) = v["error"].as_str() {
                    let text =
                        v["message"]["content"].as_array().and_then(|c| c.first()).map(|c| result_text(&json!([c])));
                    out.push(Event::Note(
                        text.filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Claude Code: {e}")),
                    ));
                }
                // Tool calls the stream did not show, as when partial
                // messages are not streamed.
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    let Some(id) = block["id"].as_str().filter(|_| block["type"] == "tool_use") else { continue };
                    if !self.tools.contains_key(id) {
                        let name = block["name"].as_str().unwrap_or("");
                        self.tool(id, name, &block["input"], &mut out);
                    }
                }
            }
            "user" if !nested => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    let Some(id) = block["tool_use_id"].as_str().filter(|_| block["type"] == "tool_result") else {
                        continue;
                    };
                    let (name, what) = self.tools.get(id).cloned().unwrap_or_default();
                    let class = class_of(&name);
                    out.push(Event::ToolDone {
                        id: id.into(),
                        what,
                        result: result_text(&block["content"]),
                        ok: block["is_error"] != true,
                        changes: matches!(class, Class::Edit | Class::Run),
                    });
                }
            }
            "result" => {
                let total = v["total_cost_usd"].as_f64().unwrap_or(self.spent);
                let cost = (total - self.spent).max(0.0);
                self.spent = total;
                let u = &v["usage"];
                let n = |u: &Value, k: &str| u[k].as_u64().unwrap_or(0);
                let sum = |u: &Value| {
                    n(u, "input_tokens") + n(u, "cache_read_input_tokens") + n(u, "cache_creation_input_tokens")
                };
                let last = u["iterations"].as_array().and_then(|i| i.last());
                let context = v["modelUsage"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(_, m)| m["contextWindow"].as_u64())
                    .max();
                out.push(Event::Usage {
                    cost,
                    tokens: sum(u) + n(u, "output_tokens"),
                    filled: last.map_or(0, sum),
                    context: context.unwrap_or(0),
                });
                let failed = v["is_error"] == true || v["subtype"].as_str().is_some_and(|s| s != "success");
                let why = || {
                    let errors: Vec<&str> =
                        v["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    match (errors.is_empty(), v["result"].as_str()) {
                        (false, _) => errors.join("\n"),
                        (true, Some(r)) if !r.is_empty() => r.to_string(),
                        _ => format!("Claude Code ended the turn: {}", v["subtype"].as_str().unwrap_or("error")),
                    }
                };
                out.push(Event::Done(failed.then(why)));
            }
            "control_request" => {
                let request = &v["request"];
                match request["subtype"].as_str().unwrap_or("") {
                    "can_use_tool" => {
                        let tool = request["tool_name"].as_str().unwrap_or("");
                        let input = request["input"].clone();
                        let id = request["tool_use_id"].as_str().unwrap_or("").to_string();
                        if tool == "ExitPlanMode" {
                            let plan = input["plan"].as_str().unwrap_or("").to_string();
                            out.push(Event::Plan(plan));
                            let why = "The user reviews the plan in Lightdown and approves it there. Stop here.";
                            out.push(Event::Send(decide(&v["request_id"], false, &input, why)));
                        } else {
                            let what = describe(&self.cwd, tool, &input);
                            out.push(Event::Ask {
                                request: v["request_id"].clone(),
                                id,
                                what,
                                class: class_of(tool),
                                input,
                            });
                        }
                    }
                    other => {
                        out.push(Event::Send(refuse(&v["request_id"], &format!("Lightdown does not handle {other}"))))
                    }
                }
            }
            "control_response" => {
                let r = &v["response"]["response"];
                if r["models"].is_array() {
                    self.models = Some(r.clone());
                }
            }
            _ => {}
        }
        out
    }

    fn tool(&mut self, id: &str, name: &str, input: &Value, out: &mut Vec<Event>) {
        let what = describe(&self.cwd, name, input);
        self.tools.insert(id.into(), (name.into(), what.clone()));
        out.push(Event::Tool { id: id.into(), what });
    }

    fn stream(&mut self, e: &Value, out: &mut Vec<Event>) {
        let index = e["index"].as_u64().unwrap_or(0);
        match e["type"].as_str().unwrap_or("") {
            "message_start" => self.blocks.clear(),
            "content_block_start" => {
                let b = &e["content_block"];
                let tool = (b["type"] == "tool_use").then(|| {
                    (b["id"].as_str().unwrap_or("").to_string(), b["name"].as_str().unwrap_or("").to_string())
                });
                self.blocks.insert(index, Block { tool, json: String::new() });
            }
            "content_block_delta" => {
                let d = &e["delta"];
                let text = |k: &str| d[k].as_str().filter(|t| !t.is_empty()).map(String::from);
                match d["type"].as_str().unwrap_or("") {
                    "text_delta" => out.extend(text("text").map(Event::Text)),
                    "thinking_delta" => out.extend(text("thinking").map(Event::Reasoning)),
                    "input_json_delta" => {
                        if let Some(b) = self.blocks.get_mut(&index) {
                            b.json += d["partial_json"].as_str().unwrap_or("");
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(Block { tool: Some((id, name)), json }) = self.blocks.remove(&index)
                    && !self.tools.contains_key(&id)
                {
                    let input = serde_json::from_str(&json).unwrap_or_else(|_| json!({}));
                    self.tool(&id, &name, &input, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CWD: &str = "/tmp/project";

    /// Plays a recorded session, as `claude` wrote it, through the parser.
    fn play(fixture: &str) -> Vec<Event> {
        let mut p = Parser::new(Path::new(CWD));
        fixture
            .lines()
            .filter(|l| !l.trim().is_empty())
            .flat_map(|l| p.line(&serde_json::from_str(l).unwrap()))
            .collect()
    }

    #[test]
    fn launch_never_bypasses_permissions_and_resumes_sessions() {
        let mut l = Launch {
            model: "claude-haiku-5-5".into(),
            effort: "low".into(),
            mode: "plan",
            session: "abc".into(),
            resume: false,
            disallowed: tools_of(Class::Web).to_vec(),
            adaptive: true,
            cwd: CWD.into(),
        };
        let a = l.args().join(" ");
        assert!(a.contains("--permission-mode plan"));
        assert!(a.contains("--session-id abc"));
        assert!(a.contains("--effort low"));
        assert!(a.contains("--permission-prompt-tool stdio"));
        assert!(a.contains("--disallowedTools WebFetch WebSearch"));
        assert!(!a.contains("bypass"));
        (l.resume, l.effort) = (true, String::new());
        let a = l.args().join(" ");
        assert!(a.contains("--resume abc") && !a.contains("--effort"));
    }

    #[test]
    fn a_recorded_turn_reads_as_text_tools_and_usage() {
        let events = play(include_str!("../tests/fixtures/claude_turn.jsonl"));
        assert_eq!(events[0], Event::Session("11387e89-83c0-4056-bc80-c69155b7cb9d".into()));
        let tools: Vec<_> = events
            .iter()
            .filter_map(|e| if let Event::Tool { what, .. } = e { Some(what.as_str()) } else { None })
            .collect();
        assert_eq!(tools, ["read notes.txt", "edit notes.txt", "run wc -l notes.txt"]);
        let done: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let Event::ToolDone { what, ok, changes, .. } = e {
                    Some((what.as_str(), *ok, *changes))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            done,
            [("read notes.txt", true, false), ("edit notes.txt", true, true), ("run wc -l notes.txt", true, true)]
        );
        let ask = events
            .iter()
            .find_map(|e| if let Event::Ask { what, class, .. } = e { Some((what.clone(), *class)) } else { None });
        assert_eq!(ask, Some(("edit notes.txt".into(), Class::Edit)));
        let text: String =
            events.iter().filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
        assert!(text.starts_with("`notes.txt` had"));
        let usage = events.iter().find(|e| matches!(e, Event::Usage { .. })).unwrap();
        let Event::Usage { cost, filled, context, .. } = usage else { unreachable!() };
        assert!((cost - 0.00771918).abs() < 1e-6);
        assert_eq!((*filled, *context), (2 + 55170 + 665, 1_000_000));
        assert_eq!(events.last(), Some(&Event::Done(None)));
    }

    #[test]
    fn permission_answers_and_plans() {
        let req = json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool",
            "tool_name": "Bash", "input": {"command": "rm -rf build"}, "tool_use_id": "t1"}});
        let mut p = Parser::new(Path::new(CWD));
        let events = p.line(&req);
        let Event::Ask { request, class, what, input, .. } = &events[0] else { panic!() };
        assert_eq!((class, what.as_str()), (&Class::Run, "run rm -rf build"));
        let yes = decide(request, true, input, "");
        assert_eq!(yes["response"]["response"]["updatedInput"]["command"], "rm -rf build");
        assert_eq!(yes["response"]["request_id"], "r1");
        let no = decide(request, false, input, "declined");
        assert_eq!(no["response"]["response"], json!({"behavior": "deny", "message": "declined"}));
        let plan = json!({"type": "control_request", "request_id": "r2", "request": {"subtype": "can_use_tool",
            "tool_name": "ExitPlanMode", "input": {"plan": "1. Do it"}, "tool_use_id": "t2"}});
        let events = p.line(&plan);
        assert_eq!(events[0], Event::Plan("1. Do it".into()));
        assert!(matches!(&events[1], Event::Send(v) if v["response"]["response"]["behavior"] == "deny"));
        let odd = json!({"type": "control_request", "request_id": "r3", "request": {"subtype": "elicitation"}});
        assert!(matches!(&p.line(&odd)[0], Event::Send(v) if v["response"]["subtype"] == "error"));
    }

    #[test]
    fn failures_and_costs_across_turns() {
        let mut p = Parser::new(Path::new(CWD));
        let first = p.line(&json!({"type": "result", "subtype": "success", "total_cost_usd": 0.5, "usage": {}}));
        assert!(matches!(first[0], Event::Usage { cost, .. } if (cost - 0.5).abs() < 1e-9));
        let second = p.line(&json!({"type": "result", "subtype": "error_during_execution", "is_error": true,
            "total_cost_usd": 0.75, "errors": ["Interrupted"], "usage": {}}));
        assert!(matches!(second[0], Event::Usage { cost, .. } if (cost - 0.25).abs() < 1e-9));
        assert_eq!(second[1], Event::Done(Some("Interrupted".into())));
        let nested = json!({"type": "stream_event", "parent_tool_use_id": "x",
            "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hidden"}}});
        assert!(p.line(&nested).is_empty());
        let msg = user("hi", &[crate::images::Image { data: "QUJD".into() }], "s");
        assert_eq!(msg["message"]["content"][1]["source"]["data"], "QUJD");
        assert_eq!(describe(Path::new(CWD), "mcp__lightdown__problems", &json!({})), "lightdown: problems");
    }
}
