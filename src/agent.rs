//! The model side of the agent: which models there are, what a conversation
//! holds, how it is put to the OpenCode Zen and Go APIs in each of their
//! three dialects, and what comes back.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config;
use crate::kernel::Waker;

const CATALOG_URL: &str = "https://models.dev/api.json";
const ZEN: &str = "https://opencode.ai/inference/v1";
const GO: &str = "https://opencode.ai/inference/go/v1";
const USER_AGENT: &str = concat!("lightdown/", env!("CARGO_PKG_VERSION"));

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ---- models ---------------------------------------------------------------

/// The dialect a model is spoken to in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Protocol {
    /// OpenAI chat completions.
    Chat,
    /// Anthropic messages.
    Messages,
    /// OpenAI responses.
    Responses,
}

#[derive(Clone, Debug)]
pub struct Model {
    /// Provider and id, as in `go/kimi-k3`. This is what settings store.
    pub key: String,
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    /// Base URL of the provider's API.
    pub api: String,
    pub context: u64,
    pub output: u64,
    /// Dollars per million tokens: input, output, cache read, cache write.
    pub cost: [f64; 4],
    /// Reasoning efforts the model accepts, if it takes any.
    pub efforts: Vec<String>,
    /// Reasoning can be switched off.
    pub toggle: bool,
    /// Reasoning is sized by a token budget.
    pub budget: bool,
    /// Reasoning must be sent back along with tool calls.
    pub interleaved: bool,
    pub images: bool,
}

impl Model {
    /// A model the catalogue does not list, described by its name alone.
    fn guess(key: &str) -> Model {
        let (provider, id) = key.split_once('/').unwrap_or(("zen", key));
        let go = provider == "go";
        let starts = |list: &[&str]| list.iter().any(|p| id.starts_with(p));
        let protocol = if starts(&["gpt-", "grok-", "muse-"]) {
            Protocol::Responses
        } else if starts(&["claude-"]) || (go && starts(&["minimax-", "qwen"])) {
            Protocol::Messages
        } else {
            Protocol::Chat
        };
        Model {
            key: key.to_string(),
            id: id.to_string(),
            name: id.to_string(),
            protocol,
            api: if go { GO } else { ZEN }.to_string(),
            context: 200_000,
            output: 32_000,
            cost: [0.0; 4],
            efforts: Vec::new(),
            toggle: false,
            budget: false,
            interleaved: protocol == Protocol::Chat,
            images: false,
        }
    }

    fn from_json(v: &Value) -> Option<Model> {
        let protocol = match v["protocol"].as_str()? {
            "chat" => Protocol::Chat,
            "messages" => Protocol::Messages,
            "responses" => Protocol::Responses,
            _ => return None,
        };
        let text = |key: &str| v[key].as_str().unwrap_or("").to_string();
        let mut cost = [0.0; 4];
        for (c, n) in cost.iter_mut().zip(v["cost"].as_array().into_iter().flatten()) {
            *c = n.as_f64().unwrap_or(0.0);
        }
        Some(Model {
            key: text("key"),
            id: text("id"),
            name: text("name"),
            protocol,
            api: text("api"),
            context: v["context"].as_u64().unwrap_or(200_000),
            output: v["output"].as_u64().unwrap_or(32_000),
            cost,
            efforts: v["efforts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect(),
            toggle: v["toggle"] == true,
            budget: v["budget"] == true,
            interleaved: v["interleaved"] == true,
            images: v["images"] == true,
        })
    }

    fn to_json(&self) -> Value {
        let protocol = match self.protocol {
            Protocol::Chat => "chat",
            Protocol::Messages => "messages",
            Protocol::Responses => "responses",
        };
        json!({
            "key": self.key, "id": self.id, "name": self.name, "protocol": protocol, "api": self.api,
            "context": self.context, "output": self.output, "cost": self.cost, "efforts": self.efforts,
            "toggle": self.toggle, "budget": self.budget, "interleaved": self.interleaved, "images":self.images,
        })
    }

    /// The reasoning settings this model can be given.
    pub fn reasoning_options(&self) -> Vec<String> {
        let mut out = vec!["default".to_string()];
        if self.toggle {
            out.push("off".into());
        }
        if !self.efforts.is_empty() {
            out.extend(self.efforts.iter().filter(|e| *e != "none").cloned());
        } else if self.budget {
            out.extend(["low", "medium", "high"].map(String::from));
        }
        out
    }

    /// What a request costs, in dollars.
    pub fn price(&self, u: &Usage) -> f64 {
        let [input, output, read, write] = self.cost;
        let read = if read > 0.0 { read } else { input };
        let write = if write > 0.0 { write } else { input };
        (u.input as f64 * input + u.output as f64 * output + u.cached as f64 * read + u.written as f64 * write) / 1e6
    }

    /// A short note on the price, for the model list.
    pub fn price_note(&self) -> String {
        match self.cost {
            [0.0, 0.0, ..] if self.key.starts_with("ollama/") => "local".into(),
            [0.0, 0.0, ..] if !self.key.starts_with("go/") && !self.key.starts_with("zen/") => {
                "price not configured".into()
            }
            [0.0, 0.0, ..] => "free".into(),
            [input, output, ..] => format!("${input} / ${output}"),
        }
    }
}

/// The models of both providers, as listed by models.dev.
fn extract(api: &Value) -> Vec<Model> {
    let mut models = Vec::new();
    for (provider, prefix, fallback) in [("opencode", "zen", ZEN), ("opencode-go", "go", GO)] {
        let p = &api[provider];
        let api_url = p["api"].as_str().unwrap_or(fallback).trim_end_matches('/');
        for (id, m) in p["models"].as_object().into_iter().flatten() {
            let npm = m["provider"]["npm"].as_str().or(p["npm"].as_str()).unwrap_or("");
            let protocol = match npm {
                "@ai-sdk/openai-compatible" => Protocol::Chat,
                "@ai-sdk/anthropic" => Protocol::Messages,
                "@ai-sdk/openai" => Protocol::Responses,
                _ => continue,
            };
            if m["status"] == "deprecated" || m["tool_call"] == false {
                continue;
            }
            let options = || m["reasoning_options"].as_array().into_iter().flatten();
            let has = |kind: &str| options().any(|o| o["type"] == kind);
            let efforts = options().find(|o| o["type"] == "effort").map(|o| &o["values"]);
            let cost = &m["cost"];
            let price = |key: &str| cost[key].as_f64().unwrap_or(0.0);
            models.push(Model {
                key: format!("{prefix}/{id}"),
                id: id.clone(),
                name: m["name"].as_str().unwrap_or(id).to_string(),
                protocol,
                api: api_url.to_string(),
                context: m["limit"]["context"].as_u64().unwrap_or(200_000),
                output: m["limit"]["output"].as_u64().unwrap_or(32_000),
                cost: [price("input"), price("output"), price("cache_read"), price("cache_write")],
                efforts: efforts
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect(),
                toggle: has("toggle"),
                budget: has("budget_tokens"),
                interleaved: m["interleaved"].is_object() || m["interleaved"] == true,
                images: m["modalities"]["input"].as_array().is_some_and(|a| a.iter().any(|v| v == "image")),
            });
        }
    }
    models.sort_by(|a, b| a.key.cmp(&b.key));
    models
}

pub struct Catalog {
    pub models: Vec<Model>,
    /// When the list was last fetched.
    fetched: u64,
    refresh: Option<Receiver<Vec<Model>>>,
}

impl Catalog {
    fn file() -> PathBuf {
        config::cache_dir().join("models.json")
    }

    /// Reads the list as it was last fetched.
    pub fn load() -> Catalog {
        let mut catalog = Catalog { models: Vec::new(), fetched: 0, refresh: None };
        if cfg!(test) {
            return catalog;
        }
        let cached: Option<Value> =
            std::fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok());
        if let Some(v) = cached {
            catalog.fetched = v["fetched"].as_u64().unwrap_or(0);
            catalog.models = v["models"].as_array().into_iter().flatten().filter_map(Model::from_json).collect();
        }
        catalog
    }

    /// Fetches the list in the background if it is missing or more than a
    /// day old. Called when the agent is first put to use, so that nothing
    /// goes over the network for someone who only edits.
    pub fn refresh(&mut self, wake: &Waker) {
        let fresh = !self.models.is_empty() && now().saturating_sub(self.fetched) < 86_400;
        if fresh || self.refresh.is_some() || cfg!(test) {
            return;
        }
        let (tx, rx) = channel();
        let wake = wake.clone();
        std::thread::spawn(move || {
            let out = Command::new("curl").args(["-sS", "-m", "40", "-A", USER_AGENT, CATALOG_URL]).output();
            let api = out.ok().and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok());
            let _ = tx.send(api.map_or(Vec::new(), |api| extract(&api)));
            wake();
        });
        self.refresh = Some(rx);
    }

    /// Takes in a freshly fetched list, if one has arrived.
    pub fn poll(&mut self) {
        let Some(models) = self.refresh.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.refresh = None;
        // Whether or not it worked, do not ask again for a day.
        self.fetched = now();
        if models.is_empty() {
            return;
        }
        let list: Vec<Value> = models.iter().map(Model::to_json).collect();
        let text = json!({"fetched": self.fetched, "models": list}).to_string();
        let _ = config::write_private(&Self::file(), text.as_bytes());
        self.models = models;
    }

    /// Blocks until a list being fetched has arrived. For tests and
    /// headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn wait(&mut self) {
        if let Some(models) =
            self.refresh.take().and_then(|rx| rx.recv_timeout(std::time::Duration::from_secs(45)).ok())
        {
            let (tx, rx) = channel();
            let _ = tx.send(models);
            self.refresh = Some(rx);
            self.poll();
        }
    }

    pub fn get(&self, key: &str) -> Model {
        self.models.iter().find(|m| m.key == key).cloned().unwrap_or_else(|| Model::guess(key))
    }
}

// ---- conversations --------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    /// The arguments, as the JSON text the model wrote.
    pub args: String,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Usage {
    /// Input tokens that were not read from the cache.
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub written: u64,
}

#[derive(Clone)]
pub enum Item {
    User {
        text: String,
        /// What the editor showed when this was sent, for the model only.
        context: String,
        images: Vec<crate::images::Image>,
    },
    Assistant {
        text: String,
        reasoning: String,
        calls: Vec<Call>,
        model: String,
        /// The reply as Anthropic's API gave it, to be sent back unchanged.
        blocks: Value,
    },
    Tool {
        id: String,
        /// One line for the transcript: what was done.
        summary: String,
        result: String,
        ok: bool,
    },
    /// Something the program tells the user; the model does not see it.
    Note(String),
}

pub struct Thread {
    pub id: String,
    pub title: String,
    pub updated: u64,
    pub items: Vec<Item>,
    pub cost: f64,
    pub tokens: u64,
    /// How much of the model's context the last request filled.
    pub filled: u64,
    pub summary: String,
    pub summarized: usize,
    /// The outside agent the thread is bound to (`claude-code`, `codex`),
    /// or empty for the built-in one.
    pub backend: String,
    /// That agent's own id for the conversation, to resume it.
    pub session: String,
}

fn threads_dir(root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut h);
    config::data_dir().join("threads").join(format!("{:016x}", h.finish()))
}

impl Thread {
    pub fn new() -> Thread {
        let id = format!("{:x}-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(), std::process::id());
        Thread {
            id,
            title: String::new(),
            updated: now(),
            items: Vec::new(),
            cost: 0.0,
            tokens: 0,
            filled: 0,
            summary: String::new(),
            summarized: 0,
            backend: String::new(),
            session: String::new(),
        }
    }

    fn to_json(&self) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|item| match item {
                Item::User { text, context, images } => json!({"type": "user", "text": text, "context": context, "images":images.iter().map(crate::images::Image::to_json).collect::<Vec<_>>()}),
                Item::Assistant { text, reasoning, calls, model, blocks } => {
                    let calls: Vec<Value> =
                        calls.iter().map(|c| json!({"id": c.id, "name": c.name, "args": c.args})).collect();
                    json!({"type": "assistant", "text": text, "reasoning": reasoning, "calls": calls, "model": model, "blocks": blocks})
                }
                Item::Tool { id, summary, result, ok } => {
                    json!({"type": "tool", "id": id, "summary": summary, "result": result, "ok": ok})
                }
                Item::Note(text) => json!({"type": "note", "text": text}),
            })
            .collect();
        json!({
            "id": self.id, "title": self.title, "updated": self.updated, "items": items,
            "cost": self.cost, "tokens": self.tokens, "filled": self.filled, "summary":self.summary,"summarized":self.summarized,
            "backend": self.backend, "session": self.session,
        })
    }

    fn from_json(v: &Value) -> Option<Thread> {
        let text = |v: &Value, key: &str| v[key].as_str().unwrap_or("").to_string();
        let item = |i: &Value| {
            Some(match i["type"].as_str()? {
                "user" => Item::User {
                    text: text(i, "text"),
                    context: text(i, "context"),
                    images: i["images"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(crate::images::Image::from_json)
                        .take(4)
                        .collect(),
                },
                "assistant" => Item::Assistant {
                    text: text(i, "text"),
                    reasoning: text(i, "reasoning"),
                    calls: i["calls"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|c| Call { id: text(c, "id"), name: text(c, "name"), args: text(c, "args") })
                        .collect(),
                    model: text(i, "model"),
                    blocks: i["blocks"].clone(),
                },
                "tool" => Item::Tool {
                    id: text(i, "id"),
                    summary: text(i, "summary"),
                    result: text(i, "result"),
                    ok: i["ok"] == true,
                },
                "note" => Item::Note(text(i, "text")),
                _ => return None,
            })
        };
        Some(Thread {
            id: v["id"].as_str()?.to_string(),
            title: text(v, "title"),
            updated: v["updated"].as_u64().unwrap_or(0),
            items: v["items"].as_array()?.iter().filter_map(item).collect(),
            cost: v["cost"].as_f64().unwrap_or(0.0),
            tokens: v["tokens"].as_u64().unwrap_or(0),
            filled: v["filled"].as_u64().unwrap_or(0),
            summary: text(v, "summary"),
            summarized: v["summarized"].as_u64().unwrap_or(0) as usize,
            backend: text(v, "backend"),
            session: text(v, "session"),
        })
    }

    pub fn save(&mut self, root: &Path) {
        if cfg!(test) || self.items.is_empty() {
            return;
        }
        self.updated = now();
        let path = threads_dir(root).join(format!("{}.json", self.id));
        let _ = config::write_private(&path, self.to_json().to_string().as_bytes());
    }

    pub fn open(root: &Path, id: &str) -> Option<Thread> {
        let text = std::fs::read_to_string(threads_dir(root).join(format!("{id}.json"))).ok()?;
        Thread::from_json(&serde_json::from_str(&text).ok()?)
    }

    pub fn delete(root: &Path, id: &str) {
        let _ = std::fs::remove_file(threads_dir(root).join(format!("{id}.json")));
    }

    /// The project's saved threads, newest first: id, title, last change.
    pub fn list(root: &Path) -> Vec<(String, String, u64)> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(threads_dir(root)).into_iter().flatten().flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            if let Some(id) = v["id"].as_str() {
                out.push((
                    id.to_string(),
                    v["title"].as_str().unwrap_or("").to_string(),
                    v["updated"].as_u64().unwrap_or(0),
                ));
            }
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.2));
        out
    }

    /// What the project's saved threads cost together, if any is saved.
    pub fn recorded_cost(root: &Path) -> Option<f64> {
        let costs: Vec<f64> = std::fs::read_dir(threads_dir(root)).into_iter().flatten().flatten()
            .filter_map(|e| serde_json::from_str::<Value>(&std::fs::read_to_string(e.path()).ok()?).ok())
            .filter_map(|v| v["cost"].as_f64()).collect();
        (!costs.is_empty()).then(|| costs.iter().sum())
    }

    /// Makes the thread fit to send again after it was cut short: every
    /// tool call must have its result.
    pub fn close_calls(&mut self, why: &str) {
        let mut open: Vec<String> = Vec::new();
        for item in &self.items {
            match item {
                Item::Assistant { calls, .. } => open.extend(calls.iter().map(|c| c.id.clone())),
                Item::Tool { id, .. } => open.retain(|o| o != id),
                _ => {}
            }
        }
        for id in open {
            self.items.push(Item::Tool { id, summary: why.to_string(), result: why.to_string(), ok: false });
        }
    }
}

// ---- spending -------------------------------------------------------------

/// What has been spent, by the hour, so that limits over the last five
/// hours, week and month can be kept.
pub struct Ledger {
    /// Hour, model, project and dollars.
    entries: Vec<(u64, String, String, f64)>,
}

pub const WINDOWS: [(u64, &str); 3] = [(5 * 3600, "5-hour"), (7 * 86_400, "weekly"), (30 * 86_400, "monthly")];

impl Ledger {
    fn file() -> PathBuf {
        config::data_dir().join("usage.json")
    }

    pub fn load() -> Ledger {
        let mut ledger = Ledger { entries: Vec::new() };
        if cfg!(test) {
            return ledger;
        }
        let v: Option<Value> = std::fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok());
        for e in v.as_ref().and_then(Value::as_array).into_iter().flatten() {
            let text = |i: usize| e[i].as_str().unwrap_or("").to_string();
            ledger.entries.push((e[0].as_u64().unwrap_or(0), text(1), text(2), e[3].as_f64().unwrap_or(0.0)));
        }
        ledger
    }

    pub fn add(&mut self, model: &str, project: &str, cost: f64) {
        let hour = now() / 3600 * 3600;
        match self.entries.iter_mut().find(|e| e.0 == hour && e.1 == model && e.2 == project) {
            Some(e) => e.3 += cost,
            None => self.entries.push((hour, model.to_string(), project.to_string(), cost)),
        }
        if cfg!(test) {
            return;
        }
        let keep = now().saturating_sub(35 * 86_400);
        self.entries.retain(|e| e.0 >= keep);
        let list: Vec<Value> = self.entries.iter().map(|e| json!([e.0, e.1, e.2, e.3])).collect();
        let _ = config::write_private(&Self::file(), Value::Array(list).to_string().as_bytes());
    }

    /// Dollars spent in the last `window` seconds, in one project or in all.
    pub fn spent(&self, window: u64, project: Option<&str>) -> f64 {
        // An hour's spending is recorded at its start.
        let since = now().saturating_sub(window + 3600);
        self.entries.iter().filter(|e| e.0 > since && project.is_none_or(|p| e.2 == p)).map(|e| e.3).sum()
    }
}

// ---- requests -------------------------------------------------------------

/// A tool as the model is told of it.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The JSON schema of its arguments.
    pub schema: &'static str,
}

pub struct Request {
    pub url: String,
    pub headers: Vec<String>,
    pub body: Value,
    pub protocol: Protocol,
}

fn budget_for(effort: &str) -> u64 {
    match effort {
        "low" | "minimal" => 2048,
        "medium" => 8192,
        "high" => 16_384,
        _ => 24_576,
    }
}

/// The reasoning setting to use with a model: the one asked for if the
/// model takes it, or else its default.
fn reasoning_for<'a>(model: &Model, wanted: &'a str) -> &'a str {
    if model.reasoning_options().iter().any(|o| o == wanted) { wanted } else { "default" }
}

fn user_text(text: &str, context: &str) -> String {
    if context.is_empty() { text.to_string() } else { format!("{text}\n\n<ide_context>\n{context}\n</ide_context>") }
}

/// Puts a thread to a model. Only the newest message carries what the
/// editor showed; older snapshots would be stale and costly.
pub fn request(
    model: &Model,
    key: &str,
    thread: &Thread,
    system: &str,
    tools: &[ToolSpec],
    reasoning: &str,
) -> Request {
    let system = if thread.summary.is_empty() {
        system.to_string()
    } else {
        format!("{system}\n\nPrevious conversation summary, for context:\n{}", thread.summary)
    };
    let system = system.as_str();
    let reasoning = reasoning_for(model, reasoning);
    let effort = model.efforts.iter().any(|e| e == reasoning);
    let last_user = thread.items.iter().rposition(|i| matches!(i, Item::User { .. }));
    let schema = |t: &ToolSpec| serde_json::from_str::<Value>(t.schema).unwrap_or_else(|_| json!({"type": "object"}));
    let args =
        |c: &Call| serde_json::from_str::<Value>(&c.args).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
    let mut headers = vec![
        "Content-Type: application/json".to_string(),
        format!("Authorization: Bearer {key}"),
        format!("User-Agent: {USER_AGENT}"),
        format!("x-opencode-session: {}", thread.id),
    ];
    if !model.key.starts_with("go/") && !model.key.starts_with("zen/") {
        headers.retain(|h| !h.starts_with("x-opencode-session:"));
    }
    if key.is_empty() || model.key.starts_with("anthropic/") {
        headers.retain(|h| !h.starts_with("Authorization:"));
    }
    let mut msgs: Vec<Value> = Vec::new();
    let body = match model.protocol {
        Protocol::Chat => {
            msgs.push(json!({"role": "system", "content": system}));
            for (i, item) in thread.items.iter().enumerate().skip(thread.active_start()) {
                match item {
                    Item::User { text, context, images } => {
                        let context = if Some(i) == last_user { context.as_str() } else { "" };
                        msgs.push(json!({"role": "user", "content": crate::images::content(user_text(text, context), images, model.protocol)}));
                    }
                    Item::Assistant { text, reasoning, calls, .. } => {
                        let mut m = json!({"role": "assistant", "content": text});
                        if !calls.is_empty() {
                            let list = calls.iter().map(|c| {
                                json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args}})
                            });
                            m["tool_calls"] = list.collect();
                        }
                        if model.interleaved && !reasoning.is_empty() {
                            m["reasoning_content"] = reasoning.as_str().into();
                        }
                        msgs.push(m);
                    }
                    Item::Tool { id, result, .. } => {
                        msgs.push(json!({"role": "tool", "tool_call_id": id, "content": result}));
                    }
                    Item::Note(_) => {}
                }
            }
            let list = tools.iter().map(|t| {
                json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": schema(t)}})
            });
            let mut body = json!({
                "model": model.id, "stream": true, "stream_options": {"include_usage": true},
                "messages": msgs, "tools": list.collect::<Vec<_>>(),
            });
            if effort {
                body["reasoning_effort"] = reasoning.into();
            } else if reasoning == "off" {
                body["thinking"] = json!({"type": "disabled"});
            }
            body
        }
        Protocol::Messages => {
            headers.push(format!("x-api-key: {key}"));
            headers.push("anthropic-version: 2023-06-01".to_string());
            // Results of tool calls go back in the user's turn, together.
            let mut results: Vec<Value> = Vec::new();
            let flush = |results: &mut Vec<Value>, msgs: &mut Vec<Value>| {
                if !results.is_empty() {
                    msgs.push(json!({"role": "user", "content": std::mem::take(results)}));
                }
            };
            for (i, item) in thread.items.iter().enumerate().skip(thread.active_start()) {
                match item {
                    Item::Tool { id, result, ok, .. } => {
                        results.push(
                            json!({"type": "tool_result", "tool_use_id": id, "content": result, "is_error": !ok}),
                        );
                    }
                    Item::User { text, context, images } => {
                        flush(&mut results, &mut msgs);
                        let context = if Some(i) == last_user { context.as_str() } else { "" };
                        msgs.push(json!({"role": "user", "content": crate::images::content(user_text(text, context), images, model.protocol)}));
                    }
                    Item::Assistant { text, calls, blocks, .. } => {
                        flush(&mut results, &mut msgs);
                        let content = match blocks.as_array().filter(|b| !b.is_empty()) {
                            Some(blocks) => blocks.clone(),
                            None => {
                                let mut content = Vec::new();
                                if !text.is_empty() {
                                    content.push(json!({"type": "text", "text": text}));
                                }
                                for c in calls {
                                    content.push(
                                        json!({"type": "tool_use", "id": c.id, "name": c.name, "input": args(c)}),
                                    );
                                }
                                content
                            }
                        };
                        if !content.is_empty() {
                            msgs.push(json!({"role": "assistant", "content": content}));
                        }
                    }
                    Item::Note(_) => {}
                }
            }
            flush(&mut results, &mut msgs);
            let list =
                tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": schema(t)}));
            let max_tokens = model.output.clamp(1024, 32_000);
            let mut body = json!({
                "model": model.id, "stream": true, "max_tokens": max_tokens, "system": system,
                "messages": msgs, "tools": list.collect::<Vec<_>>(),
            });
            if reasoning == "off" {
                body["thinking"] = json!({"type": "disabled"});
            } else if effort && !model.budget {
                body["thinking"] = json!({"type": "adaptive"});
                body["output_config"] = json!({"effort": reasoning});
            } else if reasoning != "default" {
                body["thinking"] =
                    json!({"type": "enabled", "budget_tokens": budget_for(reasoning).min(max_tokens / 2)});
            }
            body
        }
        Protocol::Responses => {
            for (i, item) in thread.items.iter().enumerate().skip(thread.active_start()) {
                match item {
                    Item::User { text, context, images } => {
                        let context = if Some(i) == last_user { context.as_str() } else { "" };
                        msgs.push(json!({"role": "user", "content": crate::images::content(user_text(text, context), images, model.protocol)}));
                    }
                    Item::Assistant { text, calls, .. } => {
                        if !text.is_empty() {
                            msgs.push(json!({"role": "assistant", "content": text}));
                        }
                        for c in calls {
                            msgs.push(
                                json!({"type": "function_call", "call_id": c.id, "name": c.name, "arguments": c.args}),
                            );
                        }
                    }
                    Item::Tool { id, result, .. } => {
                        msgs.push(json!({"type": "function_call_output", "call_id": id, "output": result}));
                    }
                    Item::Note(_) => {}
                }
            }
            let list = tools.iter().map(
                |t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": schema(t)}),
            );
            let mut body = json!({
                "model": model.id, "stream": true, "store": false, "instructions": system,
                "input": msgs, "tools": list.collect::<Vec<_>>(),
            });
            if effort {
                body["reasoning"] = json!({"effort": reasoning, "summary": "auto"});
            } else if reasoning == "off" && model.efforts.iter().any(|e| e == "none") {
                body["reasoning"] = json!({"effort": "none"});
            }
            body
        }
    };
    let path = match model.protocol {
        Protocol::Chat => "chat/completions",
        Protocol::Messages => "messages",
        Protocol::Responses => "responses",
    };
    Request { url: format!("{}/{path}", model.api), headers, body, protocol: model.protocol }
}

/// Asks a model for one short answer to one question, without tools: what
/// an inline suggestion needs. `reasoning` "off" is insisted on, whatever
/// the catalogue says the model offers, since a suggestion has to be quick.
pub fn completion(model: &Model, key: &str, session: &str, system: &str, user: &str, reasoning: &str) -> Request {
    let mut thread = Thread::new();
    thread.id = session.to_string();
    thread.items.push(Item::User { text: user.to_string(), context: String::new(), images: Vec::new() });
    let mut req = request(model, key, &thread, system, &[], reasoning);
    let off = reasoning == "off";
    let limit = if off { 512 } else { 4096 };
    let body = &mut req.body;
    if let Some(fields) = body.as_object_mut() {
        fields.remove("tools");
    }
    match model.protocol {
        Protocol::Chat => {
            body["max_tokens"] = limit.into();
            body["temperature"] = 0.2.into();
            if off {
                body["thinking"] = json!({"type": "disabled"});
            }
        }
        // With thinking on, the request already allows room for it.
        Protocol::Messages if off => {
            body["max_tokens"] = limit.into();
            body["thinking"] = json!({"type": "disabled"});
        }
        Protocol::Messages => {}
        Protocol::Responses => {
            body["max_output_tokens"] = limit.into();
            if let Some(least) = model.efforts.first().filter(|_| off) {
                body["reasoning"] = json!({"effort": least});
            }
        }
    }
    req
}

// ---- replies --------------------------------------------------------------

/// A piece of a reply as it streams in.
#[derive(Debug, PartialEq)]
pub enum Wire {
    Text(String),
    Reasoning(String),
    Call(Call),
    /// The whole reply in Anthropic's form.
    Blocks(Value),
    Usage(Usage),
    Done,
    Fail(String),
}

/// Reads one dialect's stream of server-sent events.
pub struct Parser {
    protocol: Protocol,
    /// Tool calls being assembled (chat), or content blocks (messages).
    calls: Vec<Call>,
    blocks: Vec<Value>,
    partial: Vec<String>,
    usage: Usage,
    done: bool,
}

fn error_text(v: &Value) -> Option<String> {
    let e = v.get("error")?;
    let message = e["message"].as_str().or(e.as_str()).unwrap_or("The request failed");
    Some(match e["type"].as_str() {
        Some(kind) => format!("{message} ({kind})"),
        None => message.to_string(),
    })
}

impl Parser {
    pub fn new(protocol: Protocol) -> Parser {
        Parser {
            protocol,
            calls: Vec::new(),
            blocks: Vec::new(),
            partial: Vec::new(),
            usage: Usage::default(),
            done: false,
        }
    }

    /// Takes the payload of one `data:` line.
    pub fn data(&mut self, data: &str, out: &mut Vec<Wire>) {
        if data == "[DONE]" {
            return self.finish(out);
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return };
        if let Some(e) = error_text(&v).filter(|_| v["type"] == "error" || v.get("choices").is_none()) {
            self.done = true;
            return out.push(Wire::Fail(e));
        }
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        match self.protocol {
            Protocol::Chat => {
                for choice in v["choices"].as_array().into_iter().flatten() {
                    let delta = &choice["delta"];
                    if let Some(t) = text(&delta["reasoning_content"]).or_else(|| text(&delta["reasoning"])) {
                        out.push(Wire::Reasoning(t));
                    }
                    if let Some(t) = text(&delta["content"]) {
                        out.push(Wire::Text(t));
                    }
                    for tc in delta["tool_calls"].as_array().into_iter().flatten() {
                        let i = tc["index"].as_u64().unwrap_or(0) as usize;
                        while self.calls.len() <= i {
                            self.calls.push(Call { id: String::new(), name: String::new(), args: String::new() });
                        }
                        let call = &mut self.calls[i];
                        call.id += tc["id"].as_str().unwrap_or("");
                        call.name += tc["function"]["name"].as_str().unwrap_or("");
                        call.args += tc["function"]["arguments"].as_str().unwrap_or("");
                    }
                }
                let u = &v["usage"];
                if u.is_object() {
                    let cached = u["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
                    self.usage = Usage {
                        input: u["prompt_tokens"].as_u64().unwrap_or(0).saturating_sub(cached),
                        output: u["completion_tokens"].as_u64().unwrap_or(0),
                        cached,
                        written: 0,
                    };
                }
            }
            Protocol::Messages => match v["type"].as_str().unwrap_or("") {
                "message_start" => {
                    let u = &v["message"]["usage"];
                    self.usage.input = u["input_tokens"].as_u64().unwrap_or(0);
                    self.usage.cached = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                    self.usage.written = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                }
                "content_block_start" => {
                    self.blocks.push(v["content_block"].clone());
                    self.partial.push(String::new());
                }
                "content_block_delta" => {
                    let (Some(block), Some(partial)) = (self.blocks.last_mut(), self.partial.last_mut()) else {
                        return;
                    };
                    let delta = &v["delta"];
                    let append = |block: &mut Value, key: &str, more: &str| {
                        block[key] = format!("{}{more}", block[key].as_str().unwrap_or("")).into();
                    };
                    match delta["type"].as_str().unwrap_or("") {
                        "text_delta" => {
                            let t = delta["text"].as_str().unwrap_or("");
                            append(block, "text", t);
                            out.push(Wire::Text(t.to_string()));
                        }
                        "thinking_delta" => {
                            let t = delta["thinking"].as_str().unwrap_or("");
                            append(block, "thinking", t);
                            out.push(Wire::Reasoning(t.to_string()));
                        }
                        "signature_delta" => append(block, "signature", delta["signature"].as_str().unwrap_or("")),
                        "input_json_delta" => *partial += delta["partial_json"].as_str().unwrap_or(""),
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let (Some(block), Some(partial)) = (self.blocks.last_mut(), self.partial.last()) else { return };
                    if block["type"] == "tool_use" {
                        let args = if partial.trim().is_empty() { "{}" } else { partial.as_str() };
                        block["input"] = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                        let name = |key: &str| block[key].as_str().unwrap_or("").to_string();
                        out.push(Wire::Call(Call { id: name("id"), name: name("name"), args: args.to_string() }));
                    }
                }
                "message_delta" => {
                    self.usage.output = v["usage"]["output_tokens"].as_u64().unwrap_or(self.usage.output);
                }
                "message_stop" => self.finish(out),
                _ => {}
            },
            Protocol::Responses => match v["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => out.extend(text(&v["delta"]).map(Wire::Text)),
                "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                    out.extend(text(&v["delta"]).map(Wire::Reasoning));
                }
                "response.output_item.done" if v["item"]["type"] == "function_call" => {
                    let field = |key: &str| v["item"][key].as_str().unwrap_or("").to_string();
                    out.push(Wire::Call(Call { id: field("call_id"), name: field("name"), args: field("arguments") }));
                }
                "response.completed" | "response.incomplete" => {
                    let u = &v["response"]["usage"];
                    let cached = u["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
                    self.usage = Usage {
                        input: u["input_tokens"].as_u64().unwrap_or(0).saturating_sub(cached),
                        output: u["output_tokens"].as_u64().unwrap_or(0),
                        cached,
                        written: 0,
                    };
                    self.finish(out);
                }
                "response.failed" => {
                    self.done = true;
                    let e = error_text(&v["response"]).unwrap_or_else(|| "The request failed".into());
                    out.push(Wire::Fail(e));
                }
                _ => {}
            },
        }
    }

    /// Ends the reply, once: by the stream's own last word or by its end.
    pub fn finish(&mut self, out: &mut Vec<Wire>) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        out.extend(self.calls.drain(..).filter(|c| !c.name.is_empty()).map(Wire::Call));
        if !self.blocks.is_empty() {
            out.push(Wire::Blocks(Value::Array(std::mem::take(&mut self.blocks))));
        }
        out.push(Wire::Usage(self.usage));
        out.push(Wire::Done);
    }
}

/// A request in flight: `curl` does the talking, on its own thread.
pub struct Stream {
    replies: Receiver<Wire>,
    child: Arc<Mutex<Option<Child>>>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

/// A string as curl's config files quote one.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Stream {
    pub fn start(req: Request, wake: Waker) -> Stream {
        let (tx, replies) = channel();
        let child = Arc::new(Mutex::new(None));
        let slot = child.clone();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Err(e) = converse(&req, &slot, &stop, &tx, &wake) {
                out.push(Wire::Fail(e));
            }
            // However it ended, the reader is told that it did.
            out.push(Wire::Done);
            for w in out {
                let _ = tx.send(w);
            }
            wake();
        });
        Stream { replies, child, stopped }
    }

    pub fn poll(&self) -> Vec<Wire> {
        self.replies.try_iter().collect()
    }

    /// Blocks for the next piece. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn wait(&self) -> Option<Wire> {
        self.replies.recv_timeout(std::time::Duration::from_secs(180)).ok()
    }

    pub fn stop(&self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(child) = self.child.lock().ok().as_deref_mut().and_then(Option::as_mut) {
            let _ = child.kill();
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn converse(req: &Request, slot: &Mutex<Option<Child>>, stopped: &std::sync::atomic::AtomicBool, tx: &Sender<Wire>, wake: &Waker) -> Result<(), String> {
    if stopped.load(std::sync::atomic::Ordering::SeqCst) { return Ok(()); }
    // The key goes to curl on its standard input and the body in a private
    // file, so neither shows in the process list.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(config::cache_dir, PathBuf::from);
    let body = dir.join(format!("lightdown-{}-{n}.json", std::process::id()));
    config::write_private(&body, req.body.to_string().as_bytes())
        .map_err(|e| format!("Cannot write the request: {e}"))?;
    let mut script = format!("url = {}\n", quoted(&req.url));
    for h in &req.headers {
        script += &format!("header = {}\n", quoted(h));
    }
    script += &format!("data-binary = {}\n", quoted(&format!("@{}", body.display())));
    script += "write-out = \"\\nHTTP_STATUS:%{http_code}\\n\"\n";

    let spawned = Command::new("curl")
        .args(["-sS", "-N", "--connect-timeout", "20", "--max-time", "900", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&body);
            return Err(format!("Cannot run curl: {e}"));
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    *slot.lock().unwrap() = Some(child);
    // A dropped stream may have raced the worker publishing its process.
    if stopped.load(std::sync::atomic::Ordering::SeqCst) {
        if let Some(child) = slot.lock().unwrap().as_mut() { let _ = child.kill(); }
    }

    let mut parser = Parser::new(req.protocol);
    let (mut other, mut status) = (String::new(), 0);
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let mut out = Vec::new();
        if let Some(data) = line.strip_prefix("data:") {
            parser.data(data.trim(), &mut out);
        } else if let Some(code) = line.strip_prefix("HTTP_STATUS:") {
            status = code.trim().parse().unwrap_or(0);
        } else if !line.starts_with("event:") && !line.starts_with(':') && other.len() < 4000 {
            other += line.trim();
        }
        if !out.is_empty() {
            for w in out {
                let _ = tx.send(w);
            }
            wake();
        }
    }
    let mut complaint = String::new();
    let _ = stderr.read_to_string(&mut complaint);
    let killed = slot.lock().unwrap().take().is_none_or(|mut c| !c.wait().is_ok_and(|s| s.success()));
    let _ = std::fs::remove_file(&body);
    if status != 200 {
        let said = serde_json::from_str::<Value>(&other).ok().and_then(|v| error_text(&v));
        let why = said.or(Some(other).filter(|o| !o.is_empty())).or(Some(complaint.trim().to_string()));
        return Err(match (why.filter(|w| !w.is_empty()), status) {
            (Some(why), 0) => why,
            (Some(why), code) => format!("{why} (HTTP {code})"),
            (None, _) if killed => "The request was cut short".into(),
            (None, code) => format!("The request failed (HTTP {code})"),
        });
    }
    let mut out = Vec::new();
    parser.finish(&mut out);
    for w in out {
        let _ = tx.send(w);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(protocol: Protocol, lines: &[&str]) -> Vec<Wire> {
        let (mut p, mut out) = (Parser::new(protocol), Vec::new());
        for l in lines {
            p.data(l, &mut out);
        }
        p.finish(&mut out);
        out
    }

    fn call(id: &str, name: &str, args: &str) -> Wire {
        Wire::Call(Call { id: id.into(), name: name.into(), args: args.into() })
    }

    #[test]
    fn chat_streams_are_read() {
        let out = read(
            Protocol::Chat,
            &[
                r#"{"choices":[{"delta":{"role":"assistant","reasoning_content":"hm"}}]}"#,
                r#"{"choices":[{"delta":{"content":"Hi"}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]}}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":60}}}"#,
                "[DONE]",
            ],
        );
        let usage = Usage { input: 40, output: 7, cached: 60, written: 0 };
        assert_eq!(
            out,
            [
                Wire::Reasoning("hm".into()),
                Wire::Text("Hi".into()),
                call("c1", "read_file", "{\"path\":\"a\"}"),
                Wire::Usage(usage),
                Wire::Done
            ]
        );
        let out = read(Protocol::Chat, &[r#"{"type":"error","error":{"type":"Limit","message":"No more"}}"#]);
        assert_eq!(out, [Wire::Fail("No more (Limit)".into())]);
    }

    #[test]
    fn message_streams_are_read() {
        let out = read(
            Protocol::Messages,
            &[
                r#"{"type":"message_start","message":{"usage":{"input_tokens":12,"cache_read_input_tokens":3}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"why"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"list_dir","input":{}}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\".\"}"}}"#,
                r#"{"type":"content_block_stop","index":1}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#,
                r#"{"type":"message_stop"}"#,
            ],
        );
        let blocks = json!([
            {"type": "thinking", "thinking": "why", "signature": "sig"},
            {"type": "tool_use", "id": "t1", "name": "list_dir", "input": {"path": "."}},
        ]);
        assert_eq!(
            out,
            [
                Wire::Reasoning("why".into()),
                call("t1", "list_dir", "{\"path\":\".\"}"),
                Wire::Blocks(blocks),
                Wire::Usage(Usage { input: 12, output: 9, cached: 3, written: 0 }),
                Wire::Done
            ]
        );
    }

    #[test]
    fn response_streams_are_read() {
        let out = read(
            Protocol::Responses,
            &[
                r#"{"type":"response.reasoning_summary_text.delta","delta":"plan"}"#,
                r#"{"type":"response.output_text.delta","delta":"Done"}"#,
                r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"f1","name":"search","arguments":"{}"}}"#,
                r#"{"type":"response.completed","response":{"usage":{"input_tokens":50,"output_tokens":5,"input_tokens_details":{"cached_tokens":10}}}}"#,
            ],
        );
        assert_eq!(
            out,
            [
                Wire::Reasoning("plan".into()),
                Wire::Text("Done".into()),
                call("f1", "search", "{}"),
                Wire::Usage(Usage { input: 40, output: 5, cached: 10, written: 0 }),
                Wire::Done
            ]
        );
    }

    fn thread() -> Thread {
        let mut t = Thread::new();
        t.items.push(Item::User { text: "old".into(), context: "stale".into(), images: Vec::new() });
        t.items.push(Item::Assistant {
            text: "Looking".into(),
            reasoning: "think".into(),
            calls: vec![Call { id: "c1".into(), name: "read_file".into(), args: "{\"path\":\"a\"}".into() }],
            model: String::new(),
            blocks: Value::Null,
        });
        t.items.push(Item::Tool { id: "c1".into(), summary: String::new(), result: "text".into(), ok: true });
        t.items.push(Item::Note("not for the model".into()));
        t.items.push(Item::User { text: "new".into(), context: "fresh".into(), images: Vec::new() });
        t
    }

    const TOOL: [ToolSpec; 1] = [ToolSpec { name: "read_file", description: "Reads", schema: r#"{"type":"object"}"# }];

    #[test]
    fn requests_follow_each_dialect() {
        let t = thread();
        let mut model = Model::guess("go/deepseek-v4.1-flash");
        model.efforts = vec!["low".into(), "high".into()];
        let r = request(&model, "KEY", &t, "sys", &TOOL, "high");
        assert_eq!(r.url, "https://opencode.ai/inference/go/v1/chat/completions");
        assert!(r.headers.contains(&"Authorization: Bearer KEY".to_string()));
        assert!(r.headers.iter().any(|h| h.starts_with("x-opencode-session: ")));
        let m = r.body["messages"].as_array().unwrap();
        assert_eq!(m.len(), 5);
        assert_eq!((m[1]["content"].as_str(), m[2]["reasoning_content"].as_str()), (Some("old"), Some("think")));
        assert_eq!(m[2]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(m[3]["tool_call_id"], "c1");
        assert!(m[4]["content"].as_str().unwrap().contains("<ide_context>\nfresh"));
        assert_eq!(r.body["reasoning_effort"], "high");
        // A setting the model does not take is left out.
        assert!(request(&model, "KEY", &t, "sys", &TOOL, "medium").body.get("reasoning_effort").is_none());

        let r = request(&Model::guess("zen/claude-opus-5-5"), "KEY", &t, "sys", &TOOL, "default");
        assert!(r.url.ends_with("/v1/messages") && r.body["system"] == "sys");
        let m = r.body["messages"].as_array().unwrap();
        assert_eq!(m[1]["content"][1]["type"], "tool_use");
        assert_eq!(m[2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(m[3]["role"], "user");

        let r = request(&Model::guess("zen/gpt-5.5"), "KEY", &t, "sys", &TOOL, "default");
        let input = r.body["input"].as_array().unwrap();
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[3]["output"], "text");
    }

    #[test]
    fn catalogue_is_read_and_threads_are_mended() {
        let api = json!({"opencode-go": {"npm": "@ai-sdk/openai-compatible", "api": "https://x/go/v1", "models": {
            "a": {"name": "A", "cost": {"input": 1.0, "output": 2.0, "cache_read": 0.5}, "limit": {"context": 1000, "output": 10},
                  "reasoning_options": [{"type": "toggle"}, {"type": "effort", "values": ["low", "high"]}], "interleaved": {"field": "reasoning_content"}},
            "b": {"name": "B", "provider": {"npm": "@ai-sdk/anthropic"}, "status": "deprecated"},
            "c": {"name": "C", "provider": {"npm": "@ai-sdk/google"}},
        }}});
        let models = extract(&api);
        assert_eq!(models.len(), 1);
        let m = Model::from_json(&models[0].to_json()).unwrap();
        assert_eq!(
            (m.key.as_str(), m.api.as_str(), m.protocol, m.interleaved),
            ("go/a", "https://x/go/v1", Protocol::Chat, true)
        );
        assert_eq!(m.reasoning_options(), ["default", "off", "low", "high"]);
        let cost = m.price(&Usage { input: 1_000_000, output: 500_000, cached: 2_000_000, written: 0 });
        assert!((cost - 3.0).abs() < 1e-9);

        let mut t = thread();
        t.items.truncate(2);
        t.close_calls("Stopped");
        assert!(matches!(&t.items[2], Item::Tool { id, ok: false, .. } if id == "c1"));
        let again = Thread::from_json(&t.to_json()).unwrap();
        assert_eq!(again.items.len(), 3);
    }
}

impl Thread {
    pub fn active_start(&self) -> usize {
        if !self.summary.is_empty()
            && self.summarized < self.items.len()
            && matches!(self.items[self.summarized], Item::User { .. })
        {
            self.summarized
        } else {
            0
        }
    }
    pub fn summary_cut(&self) -> Option<usize> {
        let users: Vec<_> =
            self.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::User { .. })).map(|(n, _)| n).collect();
        users.len().checked_sub(2).and_then(|n| users.get(n)).copied().filter(|n| *n > self.active_start())
    }
    pub fn summary_source(&self, until: usize) -> String {
        let mut out = self.summary.clone();
        for item in &self.items[self.active_start()..until] {
            match item {
                Item::User { text, .. } => out += &format!("\nUser: {text}"),
                Item::Assistant { text, calls, .. } => {
                    out += &format!("\nAssistant: {text}");
                    for c in calls {
                        out += &format!("\nTool call {}: {}", c.name, c.args);
                    }
                }
                Item::Tool { summary, result, ok, .. } => out += &format!("\nTool {ok}: {summary}\n{result}"),
                Item::Note(_) => {}
            }
        }
        out
    }
    pub fn request_bytes(&self) -> usize {
        self.summary.len()
            + self.items[self.active_start()..]
                .iter()
                .map(|i| match i {
                    Item::User { text, context, images } => {
                        text.len() + context.len() + images.iter().map(|i| i.data.len()).sum::<usize>()
                    }
                    Item::Assistant { text, reasoning, calls, .. } => {
                        text.len() + reasoning.len() + calls.iter().map(|c| c.args.len()).sum::<usize>()
                    }
                    Item::Tool { result, .. } => result.len(),
                    _ => 0,
                })
                .sum::<usize>()
    }
}
#[cfg(test)]
mod summary_tests {
    use super::*;
    #[test]
    fn compacted_requests_preserve_recent_tools_in_every_protocol_and_roundtrip() {
        let mut t = Thread::new();
        t.items.push(Item::User {
            text: "old secret payload".into(),
            context: "stale editor context".into(),
            images: Vec::new(),
        });
        t.items.push(Item::Assistant {
            text: "old answer".into(),
            reasoning: String::new(),
            calls: Vec::new(),
            model: String::new(),
            blocks: Value::Null,
        });
        t.items.push(Item::User { text: "recent turn".into(), context: String::new(), images: Vec::new() });
        t.items.push(Item::Assistant {
            text: String::new(),
            reasoning: String::new(),
            calls: vec![Call { id: "c1".into(), name: "read_file".into(), args: "{}".into() }],
            model: String::new(),
            blocks: Value::Null,
        });
        t.items.push(Item::Tool { id: "c1".into(), summary: "read".into(), result: "fresh result".into(), ok: true });
        t.items.push(Item::User {
            text: "latest turn".into(),
            context: "fresh editor context".into(),
            images: Vec::new(),
        });
        assert_eq!(t.summary_cut(), Some(2));
        t.summary = "Earlier decision: keep the interface stable".into();
        t.summarized = 2;
        let loaded = Thread::from_json(&t.to_json()).unwrap();
        assert_eq!(loaded.summary, t.summary);
        assert_eq!(loaded.items.len(), 6);
        for protocol in [Protocol::Chat, Protocol::Messages, Protocol::Responses] {
            let mut m = Model::guess("test/model");
            m.protocol = protocol;
            let req = request(&m, "key", &loaded, "system", &[], "off");
            let body = req.body.to_string();
            assert!(body.contains("Earlier decision"));
            assert!(body.contains("fresh result"));
            assert!(body.contains("fresh editor context"));
            assert!(!body.contains("old secret payload"));
            assert!(!body.contains("old answer"));
        }
        let before = t.items.len();
        t.close_calls("stop");
        assert_eq!(t.items.len(), before);
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    #[test]
    fn screenshot_messages_roundtrip_and_reach_all_provider_dialects() {
        let mut t = Thread::new();
        t.items.push(Item::User {
            text: "Inspect screenshot".into(),
            context: "cursor".into(),
            images: vec![crate::images::Image { data: "YWJj".into() }],
        });
        let restored = Thread::from_json(&t.to_json()).unwrap();
        for protocol in [Protocol::Chat, Protocol::Messages, Protocol::Responses] {
            let mut model = Model::guess("test/model");
            model.protocol = protocol;
            model.images = true;
            let r = request(&model, "", &restored, "system", &[], "default");
            let content = if protocol == Protocol::Responses {
                &r.body["input"][0]["content"]
            } else if protocol == Protocol::Chat {
                &r.body["messages"][1]["content"]
            } else {
                &r.body["messages"][0]["content"]
            };
            assert_eq!(content.as_array().unwrap().len(), 2);
            assert!(content[0]["text"].as_str().unwrap().contains("Inspect screenshot"));
            match protocol {
                Protocol::Chat => assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,YWJj"),
                Protocol::Messages => assert_eq!(content[1]["source"]["data"], "YWJj"),
                Protocol::Responses => assert_eq!(content[1]["image_url"], "data:image/png;base64,YWJj"),
            }
        }
    }
}
