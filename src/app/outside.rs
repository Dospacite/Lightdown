//! Claude Code and Codex at work in the agent sidebar, beside the built-in
//! agent. Each runs as its own process under the user's own sign-in and
//! acts on the project's files itself; Lightdown shows what it does, answers
//! its permission requests by the project's settings or the user's word,
//! and takes a checkpoint before each turn so its changes can be reviewed
//! and rolled back.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::*;
use crate::agent::{Call, Item, Model};
use crate::claude_code::{self, Launch};
use crate::codex_agent::{self, Client};
use crate::config::Policy;
use crate::icons::Icon;
use crate::images::Image;
use crate::nb::clip;
use crate::outside_agent::{Backend, Event, Line, Models, Proc, user_text, uuid};
use crate::tools::Class;

const PLAN: &str = "PLAN MODE: investigate with read-only commands only. Do not change files. Finish with a concrete \
implementation plan for the user to approve or edit in the editor.";

struct ClaudeRun {
    proc: Proc,
    parser: claude_code::Parser,
    launch: Launch,
    /// The Lightdown thread it serves.
    thread: String,
}

struct CodexRun {
    proc: Proc,
    client: Client,
    /// The Lightdown thread whose Codex thread is loaded, and that thread.
    loaded: String,
    thread: String,
}

/// A permission request waiting for the user.
struct Ask {
    backend: Backend,
    request: Value,
    class: Class,
    input: Value,
}

/// A Codex turn to start once its thread is ready.
struct Queued {
    text: String,
    images: Vec<Image>,
    model: String,
    effort: String,
    policy: codex_agent::Policy,
}

pub(super) struct Outside {
    pub models: Models,
    claude: Option<ClaudeRun>,
    codex: Option<CodexRun>,
    /// The backend whose turn is in progress.
    turn: Option<Backend>,
    ask: Option<Ask>,
    queued: Option<Queued>,
    /// A stopped Claude Code turn whose result has not come yet, and since
    /// when.
    draining: Option<Instant>,
    /// A plan proposed in this turn.
    plan: Option<String>,
    /// Context window sizes the agents reported, by model key.
    context: HashMap<String, u64>,
}

impl Outside {
    pub fn new() -> Outside {
        Outside {
            models: Models::load(),
            claude: None,
            codex: None,
            turn: None,
            ask: None,
            queued: None,
            draining: None,
            plan: None,
            context: HashMap::new(),
        }
    }

    pub fn busy(&self) -> bool {
        self.turn.is_some()
    }
}

/// Whether two launches can share a process: everything but the session
/// must match.
fn same(a: &Launch, b: &Launch) -> bool {
    (&a.model, &a.effort, a.mode, &a.disallowed, a.adaptive, &a.cwd)
        == (&b.model, &b.effort, b.mode, &b.disallowed, b.adaptive, &b.cwd)
}

impl App {
    /// The outside agent the project's model names, if one does.
    pub(super) fn outside_backend(&self) -> Option<Backend> {
        Backend::of(&self.cfg.model_for(&self.root).0)
    }

    /// An outside model, described as the sidebar shows models.
    pub(super) fn outside_model(&self, key: &str) -> Option<Model> {
        let (backend, m) = self.ai.outside.models.get(key)?;
        let mut model = self.ai.catalog.get(key);
        model.id = m.id;
        model.name = format!("{} \u{b7} {}", backend.name(), m.name);
        model.efforts = m.efforts;
        (model.toggle, model.budget, model.images) = (false, false, m.images);
        model.context = self.ai.outside.context.get(key).copied().unwrap_or(0);
        model.cost = [0.0; 4];
        Some(model)
    }

    /// The outside models for the model list, for the agents that are
    /// installed.
    pub(super) fn outside_entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for b in Backend::ALL.into_iter().filter(|b| b.installed()) {
            for m in self.ai.outside.models.list(b) {
                let key = format!("{}/{}", b.prefix(), m.id);
                out.push(Entry::new(&key, &m.name, &format!("{}  your sign-in", b.name())).icon(Icon::Bot));
            }
        }
        out
    }

    /// Why the thread cannot go on with the agent chosen, if it cannot: a
    /// thread stays with the agent it began with.
    pub(super) fn outside_mismatch(&self) -> Option<String> {
        let thread = &self.ai.thread;
        let want = self.outside_backend().map_or("", Backend::prefix);
        if thread.backend == want || thread.items.iter().all(|i| matches!(i, Item::Note(_))) {
            return None;
        }
        let name = |p: &str| Backend::of(p).map_or("the built-in agent", Backend::name);
        Some(format!(
            "This thread runs on {}. Start a new thread ({}) to talk to {}.",
            name(&thread.backend),
            self.keys_of("new_thread"),
            name(want)
        ))
    }

    /// A model was picked: a thread bound to another agent gives way to a
    /// new one.
    pub(super) fn outside_model_picked(&mut self, key: &str) {
        let want = Backend::of(key).map_or("", Backend::prefix);
        let thread = &self.ai.thread;
        if thread.backend != want && !thread.items.iter().all(|i| matches!(i, Item::Note(_))) {
            self.new_thread();
            self.message =
                Some(format!("New thread for {}", Backend::of(key).map_or("the built-in agent", Backend::name)));
        }
    }

    /// A saved thread was opened: the model goes back to its agent's.
    pub(super) fn outside_thread_opened(&mut self) {
        let have = self.outside_backend().map_or("", Backend::prefix);
        let bound = self.ai.thread.backend.clone();
        if bound == have {
            return;
        }
        let last = self.ai.thread.items.iter().rev().find_map(|i| match i {
            Item::Assistant { model, .. } if Backend::of(model).map_or("", Backend::prefix) == bound => {
                Some(model.clone())
            }
            _ => None,
        });
        if let Some(key) = last {
            self.cfg.project_mut(&self.root).model = key;
            let _ = self.cfg.save();
        }
    }

    /// The name of an outside model, for the transcript.
    pub(super) fn outside_name(&self, key: &str) -> Option<String> {
        self.outside_model(key).map(|m| m.name)
    }

    // ---- sending ---------------------------------------------------------

    /// Puts the newest messages to the outside agent, when one is chosen.
    /// Returns false when the built-in agent is to answer.
    pub(super) fn outside_request(&mut self) -> bool {
        let Some(backend) = self.outside_backend() else { return false };
        if crate::remote::active(&self.root).is_some() {
            self.note(&format!("{} works only in local projects.", backend.name()));
            return true;
        }
        if !backend.installed() {
            let program = backend.program();
            self.note(&format!("{} is not installed: `{program}` is not on PATH.", backend.name()));
            return true;
        }
        // The messages not yet answered: the last, and any follow-ups
        // queued with it.
        let items = &self.ai.thread.items;
        let mut start = items.len();
        for (i, item) in items.iter().enumerate().rev() {
            match item {
                Item::User { .. } => start = i,
                Item::Note(_) => {}
                _ => break,
            }
        }
        let (mut texts, mut images) = (Vec::new(), Vec::new());
        for item in &items[start..] {
            if let Item::User { text, context, images: shots } = item {
                texts.push(user_text(text, context));
                images.extend(shots.iter().cloned());
            }
        }
        if texts.is_empty() {
            return true;
        }
        let mut text = texts.join("\n\n");
        // A thread that has lost its session, as one forked or edited, is
        // told what came before.
        if self.ai.thread.session.is_empty() && start > 0 {
            let earlier = clip(self.ai.thread.summary_source(start).trim(), 30_000);
            if !earlier.is_empty() {
                text = format!("<earlier_conversation>\n{earlier}\n</earlier_conversation>\n\n{text}");
            }
        }
        if let Err(why) = self.checkpoint_command() {
            self.message = Some(format!("This turn's changes cannot be reviewed: {why}"));
        }
        let key = self.cfg.model_for(&self.root).0;
        self.ai.thread.backend = backend.prefix().into();
        self.ai.thread.items.push(Item::Assistant {
            text: String::new(),
            reasoning: String::new(),
            calls: Vec::new(),
            model: key.clone(),
            blocks: Value::Null,
        });
        self.ai.outside.plan = None;
        self.ai.outside.turn = Some(backend);
        let started = match backend {
            Backend::Claude => self.claude_turn(&key, &text, &images),
            Backend::Codex => self.codex_turn(&key, text, images),
        };
        if let Err(why) = started {
            self.outside_done(Some(why));
        }
        self.show_thread();
        true
    }

    /// The effort to ask for: the one chosen if the model takes it.
    fn outside_effort(&self, efforts: &[String]) -> String {
        let chosen = self.cfg.model_for(&self.root).1;
        if efforts.contains(&chosen) { chosen } else { String::new() }
    }

    fn claude_turn(&mut self, key: &str, text: &str, images: &[Image]) -> Result<(), String> {
        let (_, m) = self.ai.outside.models.get(key).ok_or("Unknown model")?;
        let project = self.cfg.project(&self.root);
        let mode = match (self.ai.plan_mode, project.edit) {
            (true, _) => "plan",
            (false, Policy::Allow) => "acceptEdits",
            _ => "default",
        };
        let mut disallowed = Vec::new();
        for (class, policy) in [(Class::Edit, project.edit), (Class::Run, project.run), (Class::Web, project.web)] {
            if policy == Policy::Deny {
                disallowed.extend(claude_code::tools_of(class));
            }
        }
        let resume = !self.ai.thread.session.is_empty();
        let launch = Launch {
            effort: self.outside_effort(&m.efforts),
            model: m.id,
            mode,
            session: if resume { self.ai.thread.session.clone() } else { uuid() },
            resume,
            disallowed,
            adaptive: m.adaptive,
            cwd: self.root.clone(),
        };
        // A turn stopped before its end leaves the process unsure; a fresh
        // one resumes the session.
        if self.ai.outside.draining.take().is_some() {
            self.ai.outside.claude = None;
        }
        let thread = self.ai.thread.id.clone();
        let reuse = self.ai.outside.claude.as_ref().is_some_and(|r| {
            r.thread == thread && same(&r.launch, &launch) && (!resume || r.launch.session == launch.session)
        });
        if !reuse {
            self.ai.outside.claude = None;
            let proc = Proc::start(launch.command(), self.waker.clone())?;
            proc.send(&claude_code::initialize("init"))?;
            let parser = claude_code::Parser::new(&self.root);
            self.ai.outside.claude = Some(ClaudeRun { proc, parser, launch, thread });
        }
        let run = self.ai.outside.claude.as_ref().unwrap();
        run.proc.send(&claude_code::user(text, images, &run.launch.session))
    }

    fn codex_policy(&self) -> codex_agent::Policy {
        let project = self.cfg.project(&self.root);
        codex_agent::Policy {
            plan: self.ai.plan_mode,
            edit_free: project.edit == Policy::Allow,
            run_free: project.run == Policy::Allow,
            edit_denied: project.edit == Policy::Deny,
            web_denied: project.web == Policy::Deny,
        }
    }

    fn codex_turn(&mut self, key: &str, text: String, images: Vec<Image>) -> Result<(), String> {
        let (_, m) = self.ai.outside.models.get(key).ok_or("Unknown model")?;
        let policy = self.codex_policy();
        let text = if policy.plan { format!("{PLAN}\n\n{text}") } else { text };
        let queued = Queued { text, images, effort: self.outside_effort(&m.efforts), model: m.id, policy };
        if self.ai.outside.codex.as_ref().is_none_or(|r| r.client.cwd() != self.root) {
            self.ai.outside.codex = None;
            let proc = Proc::start(codex_agent::command(&self.root), self.waker.clone())?;
            let mut client = Client::new(&self.root);
            for v in client.hello() {
                proc.send(&v)?;
            }
            self.ai.outside.codex = Some(CodexRun { proc, client, loaded: String::new(), thread: String::new() });
        }
        let (id, session, root) = (self.ai.thread.id.clone(), self.ai.thread.session.clone(), self.root.clone());
        let run = self.ai.outside.codex.as_mut().unwrap();
        if run.loaded == id && !run.thread.is_empty() {
            let q = queued;
            let v = run.client.start_turn(&run.thread, &q.text, &q.images, &q.model, &q.effort, &q.policy);
            return run.proc.send(&v);
        }
        let v = if session.is_empty() {
            run.client.start_thread(&root, &queued.policy)
        } else {
            run.client.resume_thread(&session, &root, &queued.policy)
        };
        (run.loaded, run.thread) = (id, String::new());
        self.ai.outside.queued = Some(queued);
        run.proc.send(&v)
    }

    // ---- what comes back -------------------------------------------------

    /// Takes in what the outside agents have written.
    pub(super) fn outside_poll(&mut self) {
        self.ai.outside.models.poll();
        if self.ai.outside.draining.is_some_and(|t| t.elapsed() > Duration::from_secs(10)) {
            self.ai.outside.draining = None;
            self.ai.outside.claude = None;
        }
        let mut lines: Vec<(Backend, Line)> = Vec::new();
        if let Some(r) = &self.ai.outside.claude {
            lines.extend(r.proc.poll().into_iter().map(|l| (Backend::Claude, l)));
        }
        if let Some(r) = &self.ai.outside.codex {
            lines.extend(r.proc.poll().into_iter().map(|l| (Backend::Codex, l)));
        }
        for (backend, line) in lines {
            self.outside_line(backend, line);
        }
    }

    fn outside_line(&mut self, backend: Backend, line: Line) {
        let o = &mut self.ai.outside;
        let events = match (line, backend) {
            (Line::Json(v), Backend::Claude) => {
                let Some(run) = o.claude.as_mut() else { return };
                let events = run.parser.line(&v);
                if let Some(m) = run.parser.models.take() {
                    o.models.set(backend, crate::outside_agent::claude_models(&m));
                }
                events
            }
            (Line::Json(v), Backend::Codex) => {
                let Some(run) = o.codex.as_mut() else { return };
                let events = run.client.line(&v);
                if let Some(m) = run.client.models.take() {
                    o.models.set(backend, crate::outside_agent::codex_models(&m));
                }
                events
            }
            (Line::Exit(said), _) => {
                match backend {
                    Backend::Claude => o.claude = None,
                    Backend::Codex => o.codex = None,
                }
                if backend == Backend::Claude && said.contains("No conversation found") {
                    self.ai.thread.session.clear();
                }
                if o.turn != Some(backend) {
                    return;
                }
                let tail: String =
                    said.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                let why = if tail.is_empty() {
                    format!("{} ended unexpectedly.", backend.name())
                } else {
                    format!("{} ended: {}", backend.name(), clip(&tail, 1500))
                };
                vec![Event::Done(Some(why))]
            }
        };
        for e in events {
            self.outside_event(backend, e);
        }
    }

    fn outside_send(&mut self, backend: Backend, v: &Value) {
        let sent = match backend {
            Backend::Claude => self.ai.outside.claude.as_ref().map(|r| r.proc.send(v)),
            Backend::Codex => self.ai.outside.codex.as_ref().map(|r| r.proc.send(v)),
        };
        if let Some(Err(why)) = sent {
            self.message = Some(why);
        }
    }

    /// Answers a permission request.
    fn outside_answer(&mut self, backend: Backend, request: &Value, input: &Value, allow: bool, why: &str) {
        let v = match backend {
            Backend::Claude => claude_code::decide(request, allow, input, why),
            Backend::Codex => Client::decide(request, allow, false),
        };
        self.outside_send(backend, &v);
    }

    /// The assistant message being written, started anew after tools.
    fn outside_assistant(&mut self) -> &mut Item {
        let items = &mut self.ai.thread.items;
        if !matches!(items.last(), Some(Item::Assistant { .. })) {
            let model = items.iter().rev().find_map(|i| match i {
                Item::Assistant { model, .. } => Some(model.clone()),
                _ => None,
            });
            items.push(Item::Assistant {
                text: String::new(),
                reasoning: String::new(),
                calls: Vec::new(),
                model: model.unwrap_or_default(),
                blocks: Value::Null,
            });
        }
        items.last_mut().unwrap()
    }

    fn outside_event(&mut self, backend: Backend, e: Event) {
        // What a stopped Claude Code turn still says is dropped, but its
        // cost is counted and its questions refused.
        if backend == Backend::Claude && self.ai.outside.draining.is_some() {
            match e {
                Event::Done(_) => self.ai.outside.draining = None,
                Event::Usage { cost, tokens, .. } => {
                    self.ai.thread.cost += cost;
                    self.ai.thread.tokens += tokens;
                }
                Event::Ask { request, input, .. } => {
                    self.outside_answer(backend, &request, &input, false, "Stopped by the user.")
                }
                Event::Send(v) => self.outside_send(backend, &v),
                Event::Session(id) => self.ai.thread.session = id,
                _ => {}
            }
            return;
        }
        if self.ai.outside.turn != Some(backend) {
            match e {
                Event::Send(v) => self.outside_send(backend, &v),
                Event::Ask { request, input, .. } => {
                    self.outside_answer(backend, &request, &input, false, "No turn is running.")
                }
                _ => {}
            }
            return;
        }
        match e {
            Event::Session(id) => {
                self.ai.thread.session = id.clone();
                self.ai.thread.save(&self.root);
                if let Some(run) = self.ai.outside.codex.as_mut().filter(|_| backend == Backend::Codex) {
                    run.thread = id.clone();
                    if let Some(q) = self.ai.outside.queued.take() {
                        let v = run.client.start_turn(&id, &q.text, &q.images, &q.model, &q.effort, &q.policy);
                        self.outside_send(backend, &v);
                    }
                }
            }
            Event::Text(t) => {
                if let Item::Assistant { text, .. } = self.outside_assistant() {
                    *text += &t;
                }
            }
            Event::Reasoning(t) => {
                if let Item::Assistant { reasoning, .. } = self.outside_assistant() {
                    *reasoning += &t;
                }
            }
            Event::Tool { id, what } => {
                if let Item::Assistant { calls, .. } = self.outside_assistant() {
                    calls.push(Call { id, name: what, args: "{}".into() });
                }
            }
            Event::ToolDone { id, what, result, ok, changes } => {
                let summary = if ok { what } else { format!("{what} (failed)") };
                let result = if result.trim().is_empty() { "(no output)".to_string() } else { clip(&result, 40_000) };
                self.ai.thread.items.push(Item::Tool { id, summary, result, ok });
                if changes {
                    self.refresh_agent_changes();
                    self.disk_poll(true);
                    self.tree.refresh();
                }
            }
            Event::Ask { request, id, what, class, input } => {
                self.outside_ask(backend, request, id, what, class, input)
            }
            Event::Usage { cost, tokens, filled, context } => {
                self.ai.thread.cost += cost;
                self.ai.thread.tokens += tokens;
                if filled > 0 {
                    self.ai.thread.filled = filled;
                }
                if context > 0 {
                    self.ai.outside.context.insert(self.cfg.model_for(&self.root).0, context);
                }
            }
            Event::Plan(plan) => self.ai.outside.plan = Some(plan).filter(|p| !p.trim().is_empty()),
            Event::Turn(_) => {}
            Event::Lost(why) => {
                self.ai
                    .thread
                    .items
                    .push(Item::Note(format!("The Codex thread could not be resumed ({why}); a new one is started.")));
                self.ai.thread.session.clear();
                let policy = self.codex_policy();
                let root = self.root.clone();
                if let Some(run) = self.ai.outside.codex.as_mut() {
                    let v = run.client.start_thread(&root, &policy);
                    self.outside_send(backend, &v);
                }
            }
            Event::Send(v) => self.outside_send(backend, &v),
            Event::Done(why) => return self.outside_done(why),
            Event::Note(text) => self.ai.thread.items.push(Item::Note(text)),
        }
        self.show_thread();
    }

    fn outside_ask(&mut self, backend: Backend, request: Value, id: String, what: String, class: Class, input: Value) {
        let project = self.cfg.project(&self.root);
        let policy = match class {
            Class::Read => Policy::Allow,
            Class::Edit => project.edit,
            Class::Run => project.run,
            Class::Web => project.web,
        };
        if self.ai.plan_mode && matches!(class, Class::Edit | Class::Run) {
            let why = "Plan mode is read-only. Propose a plan and wait for the user to approve it in the editor.";
            return self.outside_answer(backend, &request, &input, false, why);
        }
        match policy {
            Policy::Allow => self.outside_answer(backend, &request, &input, true, ""),
            Policy::Deny => {
                let why = "This project's settings do not allow the agent to do this.";
                self.outside_answer(backend, &request, &input, false, why);
            }
            Policy::Ask => {
                self.ai.outside.ask = Some(Ask { backend, request, class, input });
                self.ai.approval = Some(Call { id, name: what, args: "{}".into() });
                self.focus = Focus::Chat;
                self.show_side(Some(Tab::Agent));
            }
        }
    }

    /// Answers the question on the approval card, if it is an outside
    /// agent's. Returns whether it was.
    pub(super) fn outside_approve(&mut self, allow: bool, always: bool) -> bool {
        let Some(ask) = self.ai.outside.ask.take() else { return false };
        self.ai.approval = None;
        if allow && always {
            let project = self.cfg.project_mut(&self.root);
            match ask.class {
                Class::Edit => project.edit = Policy::Allow,
                Class::Web => project.web = Policy::Allow,
                _ => project.run = Policy::Allow,
            }
            let _ = self.cfg.save();
        }
        let why = "The user declined this. Do not try it again without asking.";
        self.outside_answer(ask.backend, &ask.request, &ask.input, allow, why);
        self.show_thread();
        true
    }

    /// The turn is over: what it changed is taken in, and what comes after
    /// a turn of the built-in agent follows.
    fn outside_done(&mut self, why: Option<String>) {
        self.ai.outside.turn = None;
        self.ai.outside.queued = None;
        if self.ai.outside.ask.take().is_some() {
            self.ai.approval = None;
        }
        let hollow = |i: &Item| matches!(i, Item::Assistant { text, reasoning, calls, .. } if text.is_empty() && reasoning.is_empty() && calls.is_empty());
        if self.ai.thread.items.last().is_some_and(hollow) {
            self.ai.thread.items.pop();
        }
        self.ai.thread.close_calls("Not finished");
        let failed = why.is_some();
        if let Some(why) = why {
            self.ai.thread.items.push(Item::Note(why));
        }
        self.refresh_agent_changes();
        self.disk_poll(true);
        self.tree.refresh();
        if self.deliver_followups() {
            return self.request();
        }
        if self.ai.plan_mode && !failed {
            let last = self.ai.thread.items.iter().rev().find_map(|i| match i {
                Item::Assistant { text, .. } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            });
            if let Some(plan) = self.ai.outside.plan.take().or(last) {
                self.ai.proposed_plan = Some(plan);
                self.message = Some("Plan ready. Use Agent: Approve plan or Edit proposed plan".into());
            }
        }
        if !failed && self.agent_check_start() {
            self.ai.thread.save(&self.root);
            return self.show_thread();
        }
        self.finish_agent_turn();
        self.ai.thread.save(&self.root);
        self.show_thread();
    }

    /// Stops the outside agent's turn: Claude Code is interrupted, Codex's
    /// turn cancelled. Called by `stop`.
    pub(super) fn outside_stop(&mut self) {
        // A turn that has just ended needs no stopping.
        self.outside_poll();
        let Some(backend) = self.ai.outside.turn.take() else { return };
        self.ai.outside.queued = None;
        if let Some(ask) = self.ai.outside.ask.take() {
            self.ai.approval = None;
            let v = match backend {
                Backend::Claude => claude_code::decide(&ask.request, false, &ask.input, "Stopped by the user."),
                Backend::Codex => Client::decide(&ask.request, false, true),
            };
            self.outside_send(backend, &v);
        }
        match backend {
            Backend::Claude => {
                self.outside_send(backend, &claude_code::interrupt("stop"));
                self.ai.outside.draining = Some(Instant::now());
            }
            Backend::Codex => {
                if let Some(run) = self.ai.outside.codex.as_mut() {
                    let thread = run.thread.clone();
                    if let Some(v) = run.client.interrupt(&thread) {
                        self.outside_send(backend, &v);
                    }
                }
            }
        }
        self.refresh_agent_changes();
        self.disk_poll(true);
        self.tree.refresh();
    }

    /// Waits for the outside agent's turn to end, or to need the user. For
    /// tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn outside_settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(300);
        while let Some(backend) = self.ai.outside.turn.filter(|_| self.ai.outside.ask.is_none()) {
            if Instant::now() > deadline {
                break;
            }
            let line = match backend {
                Backend::Claude => self.ai.outside.claude.as_ref().map(|r| r.proc.wait(Duration::from_secs(1))),
                Backend::Codex => self.ai.outside.codex.as_ref().map(|r| r.proc.wait(Duration::from_secs(1))),
            };
            match line {
                Some(Some(line)) => self.outside_line(backend, line),
                Some(None) => {}
                None => self.outside_done(Some(format!("{} is not running.", backend.name()))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-outside-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }

    /// Plays a recorded turn as if the agent were running.
    fn play(a: &mut App, backend: Backend, key: &str, events: Vec<Event>) {
        a.cfg.project_mut(&a.root.clone()).model = key.into();
        a.ai.thread.items.push(Item::User { text: "go".into(), context: String::new(), images: Vec::new() });
        a.ai.thread.items.push(Item::Assistant {
            text: String::new(),
            reasoning: String::new(),
            calls: Vec::new(),
            model: key.into(),
            blocks: Value::Null,
        });
        a.ai.thread.backend = backend.prefix().into();
        a.ai.outside.turn = Some(backend);
        for e in events {
            a.outside_event(backend, e);
        }
    }

    fn claude_events(cwd: &Path) -> Vec<Event> {
        let fixture =
            include_str!("../../tests/fixtures/claude_turn.jsonl").replace("/tmp/project", &cwd.to_string_lossy());
        let mut p = claude_code::Parser::new(cwd);
        fixture.lines().flat_map(|l| p.line(&serde_json::from_str(l).unwrap())).collect()
    }

    #[test]
    fn a_claude_turn_fills_the_transcript_asks_and_ends() {
        let dir = project("claude");
        let mut a = App::new(Some(dir.join("notes.txt")));
        a.resize(1000, 700, 1.0);
        a.cfg.project_mut(&dir).edit = Policy::Ask;
        let events = claude_events(&dir);
        let ask = events.iter().position(|e| matches!(e, Event::Ask { .. })).unwrap();
        play(&mut a, Backend::Claude, "claude-code/claude-haiku-5-5", events[..=ask].to_vec());
        // The edit waits on the approval card.
        assert!(a.ai.waiting() && a.ai.busy());
        assert_eq!(a.side_view().approval.as_deref(), Some("edit notes.txt"));
        assert!(a.outside_approve(true, false));
        assert!(!a.ai.waiting());
        for e in events[ask + 1..].to_vec() {
            a.outside_event(Backend::Claude, e);
        }
        assert!(!a.ai.busy());
        assert_eq!(a.ai.thread.session, "11387e89-83c0-4056-bc80-c69155b7cb9d");
        let tools: Vec<_> =
            a.ai.thread
                .items
                .iter()
                .filter_map(|i| if let Item::Tool { summary, .. } = i { Some(summary.as_str()) } else { None })
                .collect();
        assert_eq!(tools, ["read notes.txt", "edit notes.txt", "run wc -l notes.txt"]);
        let shown = a.ai.side.transcript.buf.text.clone();
        assert!(shown.contains("Claude Code \u{b7} Haiku 5.5"), "{shown}");
        assert!(shown.contains("reports 2."));
        assert!(a.ai.thread.cost > 0.007);
        assert!(a.side_view().usage.contains("$0.008"), "{}", a.side_view().usage);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn policies_answer_without_asking_and_threads_stay_with_their_agent() {
        let dir = project("policy");
        let mut a = App::new(Some(dir.join("notes.txt")));
        a.cfg.project_mut(&dir).edit = Policy::Deny;
        a.cfg.project_mut(&dir).run = Policy::Allow;
        let ask =
            |class| Event::Ask { request: "r".into(), id: "t".into(), what: "x".into(), class, input: Value::Null };
        play(&mut a, Backend::Codex, "codex/gpt-6-luna", vec![ask(Class::Edit), ask(Class::Run)]);
        assert!(!a.ai.waiting());
        // A Codex thread refuses the built-in agent, and picking another
        // agent starts a new thread.
        a.cfg.project_mut(&dir).model = "go/kimi-k3".into();
        assert!(a.outside_mismatch().unwrap().contains("runs on Codex"));
        a.cfg.project_mut(&dir).model = "codex/gpt-6-luna".into();
        assert!(a.outside_mismatch().is_none());
        a.outside_event(Backend::Codex, Event::Done(Some("boom".into())));
        assert!(matches!(a.ai.thread.items.last(), Some(Item::Note(n)) if n == "boom"));
        a.outside_model_picked("claude-code/claude-haiku-5-5");
        assert!(a.ai.thread.items.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
