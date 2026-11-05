//! Language server integration and validated, unsaved workspace edits.
use super::*;
use crate::icons::Icon;
use crate::lsp;
use serde_json::{Value, json};
use std::collections::HashMap;
#[path = "language_features.rs"]
mod features;
#[path = "language_install.rs"]
mod install;
#[path = "language_workspace.rs"]
mod workspace_edits;
#[path = "language_files.rs"]
pub(in crate::app) mod file_events;
#[path = "hierarchy.rs"]
mod hierarchy;
#[path = "language_status.rs"]
mod status;
#[path = "peek.rs"]
mod peek;
#[path = "lightbulb.rs"]
mod lightbulb;
#[path = "semantic.rs"]
mod semantic;
#[path = "agent_rename.rs"]
mod agent_rename;
/// Requests the editor makes by itself; they never speak in the status bar.
const BACKGROUND: [&str; 6] = ["document_symbols", "pull_diagnostics", "workspace_symbols", "hierarchy_children", "semantic_tokens", "lightbulb"];
struct Pending {
    action: String,
    path: PathBuf,
    version: u64,
    since: std::time::Instant,
    hover_at: Option<usize>,
    cursor: usize,
}
/// Project symbols from the server, for one project and the query last asked.
#[derive(Default)]
struct Workspace {
    root: Option<PathBuf>,
    asked: Option<String>,
    wanted: String,
    symbols: Option<Vec<Value>>,
}
#[derive(Default)]
pub(super) struct Languages {
    hierarchy: hierarchy::Hierarchy,
    installs: install::Installs,
    files: file_events::Files,
    key: Option<(PathBuf, String)>,
    clients: HashMap<(PathBuf, String), lsp::Client>,
    failed: std::collections::HashSet<(PathBuf,String)>,
    opened: HashMap<PathBuf, (u64, String)>,
    next: u64,
    pending: HashMap<u64, Pending>,
    pub(super) diagnostics: HashMap<PathBuf, (u64, Vec<Value>)>,
    capabilities: HashMap<(PathBuf, String), Value>,
    status: status::Status,
    peek: Option<peek::Peek>,
    bulb: lightbulb::Bulb,
    semantic: semantic::Semantic,
    agent_rename: Option<(PathBuf, String)>,
    workspace: Workspace,
    symbols: HashMap<PathBuf, (u64, Vec<crate::outline::Item>)>,
    pub(super) locations: Vec<Value>,
    rename_snapshot: HashMap<PathBuf, (String, String)>,
    hover_at: Option<usize>,
    features: features::Features,
}
impl App {
    /// The LSP severity (1 error to 4 hint) of a diagnostic the Problems list shows.
    pub(super) fn diagnostic_severity(&self, path: &Path, line: usize, message: &str) -> Option<u64> {
        let (_, items) = self.languages.diagnostics.get(path)?;
        items.iter().find(|d| d["range"]["start"]["line"].as_u64() == Some(line.saturating_sub(1) as u64) && d["message"] == message)?["severity"].as_u64()
    }
    pub(super) fn language_problems(&self) -> Vec<super::problems::Problem> {
        self.languages.diagnostics.iter().filter(|(p,(version,_))| {
            if self.doc.path.as_ref() == Some(p) { return self.doc.buf.version == *version; }
            self.file_tabs.iter().filter_map(|t| t.state.as_ref()).find(|s| s.pane.path.as_ref() == Some(p)).is_none_or(|s| s.pane.buf.version == *version)
        }).flat_map(|(path,(_,items))| items.iter().map(|d| super::problems::Problem { path:Some(path.clone()),line:d["range"]["start"]["line"].as_u64().unwrap_or(0) as usize+1,col:d["range"]["start"]["character"].as_u64().unwrap_or(0) as usize+1,message:d["message"].as_str().unwrap_or("").into(),source:"LSP" })).collect()
    }
    pub(super) fn language_sync(&mut self) {
        if self.inline_preview.is_some()
            || self.doc.ghost().is_some()
            || self.nb.is_some()
            || self.reader.is_some()
            || self.doc.read_only
        {
            return;
        }
        let Some(path) = self.doc.path.clone() else { return };
        let (id,cmd) = match lsp::server(&self.root,&path) {
            Ok(Some(v)) => v,
            Ok(None) => { self.languages.key = None; self.doc.diagnostics.clear(); return; }
            Err(e) => { self.message = Some(e); return; }
        };
        // A window of several folders keeps a server for each of them.
        if self.languages.clients.keys().any(|(root,_)| root != &self.root && !(self.multi_root() && self.folders.contains(root) && self.folders.contains(&self.root))) { self.languages = Default::default(); }
        let key = (self.root.clone(), id.clone());
        self.languages.key = Some(key.clone());
        if self.languages.failed.contains(&key) { return; }
        if !self.languages.clients.contains_key(&key) {
            if self.language_install_missing(&key, &cmd) { return; }
            self.language_server_started(&key, &cmd);
            let client = lsp::Client::start_for_language(cmd.clone(),self.root.clone(),self.waker.clone(),&id);
            client.share(&self.root, &cmd);
            self.languages.clients.insert(key.clone(),client);
        }
        let Some(client) = self.languages.clients.get(&key) else { return };
        let version = self.doc.buf.version;
        let old = self.languages.opened.get(&path);
        if old.is_some_and(|(v, _)| *v == version) {
            self.semantic_apply();
            return;
        }
        if old.is_none() {
            client.notify("textDocument/didOpen",json!({"textDocument":{"uri":lsp::uri(&path),"languageId":id,"version":version,"text":self.doc.buf.text}}));
        } else {
            client.notify("textDocument/didChange",json!({"textDocument":{"uri":lsp::uri(&path),"version":version},"contentChanges":[{"text":self.doc.buf.text}]}));
        }
        let pull = self.languages.capabilities.get(&key).is_some_and(|c| c["diagnosticProvider"].is_object());
        self.languages.opened.insert(path, (version, self.doc.buf.text.clone()));
        self.doc.diagnostics.clear();
        if pull { self.language_pull_diagnostics(); }
        self.language_request_symbols();
        self.semantic_request();
    }
    /// Asks the running server for project symbols matching `query`, one request at a time.
    /// Until it answers, or when it cannot, the palette keeps the local index.
    pub(super) fn language_workspace_symbols(&mut self, query: &str) {
        self.language_sync();
        let supported = self.languages.key.as_ref().filter(|k| self.languages.clients.contains_key(*k)).and_then(|k| self.languages.capabilities.get(k)).is_some_and(|c| c["workspaceSymbolProvider"] == true || c["workspaceSymbolProvider"].is_object());
        let w = &mut self.languages.workspace;
        if !supported || w.root.as_ref() != Some(&self.root) {
            *w = Workspace { root: Some(self.root.clone()), ..Default::default() };
            if !supported { return; }
        }
        w.wanted = query.to_string();
        if w.asked.as_deref() == Some(query) || self.languages.pending.values().any(|p| p.action == "workspace_symbols") { return; }
        w.asked = Some(query.to_string());
        self.language_request("workspace_symbols", "workspace/symbol", json!({"query":query}));
    }
    fn language_workspace_result(&mut self, result: &Value) {
        let w = &mut self.languages.workspace;
        w.symbols = Some(result.as_array().into_iter().flatten().filter(|s| s["location"]["range"].is_object()).take(10000).cloned().collect());
        if let Some((MenuKind::Palette, menu)) = &self.menu
            && let Some(typed) = menu.typed().strip_prefix('#').map(str::to_string)
        {
            self.palette_stale = true;
            self.palette_update();
            // The query moved on while the server worked; ask for the newest one.
            if self.languages.workspace.wanted != self.languages.workspace.asked.clone().unwrap_or_default() { self.language_workspace_symbols(&typed); }
        }
    }
    pub(super) fn language_workspace_entries(&self) -> Option<Vec<Entry>> {
        let w = &self.languages.workspace;
        if w.root.as_ref() != Some(&self.root) { return None; }
        w.symbols.as_ref().map(|symbols| symbols.iter().enumerate().filter_map(|(i,s)| {
            let path = lsp::path(s["location"]["uri"].as_str()?)?;
            let line = s.pointer("/location/range/start/line").and_then(Value::as_u64).unwrap_or(0) + 1;
            let shown = path.strip_prefix(&self.root).unwrap_or(&path).display().to_string();
            Some(Entry::new(&format!("w\n{i}"), s["name"].as_str().unwrap_or("Symbol"), &format!("{shown}:{line}")).icon(Icon::Braces))
        }).collect())
    }
    pub(super) fn language_workspace_pick(&mut self, key: &str) {
        let Some(symbol) = key.parse::<usize>().ok().and_then(|i| self.languages.workspace.symbols.as_ref()?.get(i)).cloned() else { return };
        self.languages.locations = vec![symbol["location"].clone()];
        self.language_pick("0");
    }
    fn language_request_symbols(&mut self) {
        if !self.languages.key.as_ref().and_then(|k| self.languages.capabilities.get(k)).is_some_and(|c| c["documentSymbolProvider"] == true || c["documentSymbolProvider"].is_object()) { return; }
        let Some(path) = &self.doc.path else { return };
        self.language_request("document_symbols", "textDocument/documentSymbol", json!({"textDocument":{"uri":lsp::uri(path)}}));
    }
    pub(super) fn language_symbols(&self) -> Option<Vec<crate::outline::Item>> {
        self.doc.path.as_ref().and_then(|p| self.languages.symbols.get(p)).filter(|(v,_)| *v == self.doc.buf.version).map(|(_,s)| s.clone())
    }
    fn language_pull_diagnostics(&mut self) {
        let Some(path) = &self.doc.path else { return };
        self.language_request("pull_diagnostics", "textDocument/diagnostic", json!({"textDocument":{"uri":lsp::uri(path)}}));
    }
    pub(super) fn language_command(&mut self, action: &str) {
        if let Some(direction) = action.strip_prefix("hierarchy_") { self.hierarchy_start(direction); return; }
        if action == "update" { self.language_updates(); return; }
        if let Some(name) = action.strip_prefix("install:") { self.language_install_start(name); return; }
        if action == "server_log" { self.language_server_log(); return; }
        if action == "stop" {
            if let Some(key) = self.languages.key.clone() {
                self.languages.clients.remove(&key);
                self.languages.failed.insert(key.clone());
                self.languages.opened.retain(|p,_| lsp::server(&self.root,p).ok().flatten().is_none_or(|(id,_)| id != key.1));
                self.language_server_stopped(&key);
                self.message = Some(format!("Stopped {}", key.1));
            }
            return;
        }
        if action == "restart" {
            // Logs survive a restart so the reason for it can still be read.
            let status = std::mem::take(&mut self.languages.status);
            self.languages = Default::default();
            self.languages.status = status;
        }
        self.language_sync();
        if action == "restart" {
            return;
        }
        let Some(path) = self.doc.path.clone().filter(|_| self.languages.key.as_ref().is_some_and(|k| self.languages.clients.contains_key(k))) else {
            self.message = Some("No language server for this file".into());
            return;
        };
        if action == "ask_fix" { self.language_ask_fix(); return; }
        if action == "diagnostics" {
            let entries = self
                .doc
                .diagnostics
                .iter()
                .map(|(a, _, text)| Entry::new(&a.to_string(), text, &format!("Line {}", self.doc.buf.line_of(*a) + 1)))
                .collect();
            self.menu = Some((MenuKind::LanguageDiagnostics, Menu::new("Language diagnostics", entries, "")));
            return;
        }
        if action == "rename" {
            self.ask(PromptKind::LspRename);
            return;
        }
        if self.language_feature_command(action) { return; }
        let (method, extra) = match action {
            "definition" => ("textDocument/definition", Value::Null),
            "implementation" => ("textDocument/implementation", Value::Null),
            "type_definition" => ("textDocument/typeDefinition", Value::Null),
            "declaration" => ("textDocument/declaration", Value::Null),
            "references" | "references_editor" => ("textDocument/references", json!({"includeDeclaration":true})),
            "peek_definition" => ("textDocument/definition", Value::Null),
            "peek_references" => ("textDocument/references", json!({"includeDeclaration":true})),
            "hover" => ("textDocument/hover", Value::Null),
            _ => return,
        };
        let mut params = json!({"textDocument":{"uri":lsp::uri(&path)},"position":lsp::position(&self.doc.buf.text,self.doc.buf.cur)});
        if !extra.is_null() {
            params["context"] = extra;
        }
        self.language_request(action, method, params);
    }
    fn language_request(&mut self, action: &str, method: &str, params: Value) {
        let Some(client) = self.languages.key.as_ref().and_then(|k| self.languages.clients.get(k)) else { return };
        let Some(path) = self.doc.path.clone() else { return };
        if action == "format_selection" {
            self.languages.pending.retain(|_,p| p.action != "format_selection");
        }
        self.languages.next += 1;
        let n = self.languages.next;
        self.languages.pending.insert(
            n,
            Pending {
                action: action.into(),
                path,
                version: self.doc.buf.version,
                since: std::time::Instant::now(),
                hover_at: self.languages.hover_at.take(),
                cursor: self.doc.buf.cur,
            },
        );
        client.request(n, method, params);
        if !BACKGROUND.contains(&action) { self.message = Some(format!("Language server: {action}…")); }
    }
    pub(super) fn language_rename(&mut self, name: &str) {
        self.prompt = None;
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            self.message = Some("Enter a symbol name without spaces".into());
            return;
        }
        if !self.language_snapshot() { return; }
        let Some(path) = &self.doc.path else { return };
        self.language_request("rename","textDocument/rename",json!({"textDocument":{"uri":lsp::uri(path)},"position":lsp::position(&self.doc.buf.text,self.doc.buf.cur),"newName":name}));
    }
    fn language_snapshot(&mut self) -> bool {
        let mut snapshot = HashMap::new();
        let mut size = 0;
        for path in search::files(&self.root) {
            if std::fs::metadata(&path).map_or(true, |m| m.len() > 2 << 20) {
                continue;
            }
            let Ok(disk) = std::fs::read_to_string(&path) else { continue };
            if disk.contains('\0') {
                continue;
            }
            size += disk.len();
            if size > 32 << 20 {
                self.message = Some("Project is too large for a safe rename snapshot".into());
                return false;
            }
            let mut text = disk.replace("\r\n", "\n");
            for (i, t) in self.file_tabs.iter().enumerate() {
                let pane = if i == self.active_file { Some(&self.doc) } else { t.state.as_ref().map(|s| &s.pane) };
                if let Some(p) = pane.filter(|p| p.path.as_ref() == Some(&path)) {
                    text = p.buf.text.clone();
                }
            }
            snapshot.insert(path, (disk, text));
        }
        self.languages.rename_snapshot = snapshot;
        true
    }
    pub(super) fn language_selection_request(&mut self,at:usize) {
        self.language_sync();
        let Some(path) = &self.doc.path else { return };
        self.language_request("selection","textDocument/selectionRange",json!({"textDocument":{"uri":lsp::uri(path)},"positions":[lsp::position(&self.doc.buf.text,at)]}));
    }
    pub(super) fn language_pointer_hover(&mut self, at: usize) {
        if let Some((_,_,message)) = self.doc.diagnostics.iter().find(|(a,b,_)| *a <= at && at <= *b) {
            self.language_hover_at(message.clone(), Some(at));
            return;
        }
        self.language_sync();
        let Some(path) = &self.doc.path else { return };
        self.languages.hover_at = Some(at);
        self.language_request(
            "hover",
            "textDocument/hover",
            json!({"textDocument":{"uri":lsp::uri(path)},"position":lsp::position(&self.doc.buf.text,at)}),
        );
    }
    /// Waits up to five seconds for the servers' replies. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn language_settle(&mut self) {
        let start = std::time::Instant::now();
        loop {
            self.language_poll();
            self.lightbulb_tick();
            let starting = self.languages.key.as_ref().is_some_and(|k| self.languages.clients.contains_key(k) && !self.languages.capabilities.contains_key(k));
            if (self.languages.pending.is_empty() && !starting) || start.elapsed().as_secs() >= 5 { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    pub(super) fn language_poll(&mut self) {
        self.language_install_poll();
        self.language_files_tick();
        let messages: Vec<_> = self.languages.clients.iter().flat_map(|(k,c)| c.poll().into_iter().map(|v| (k.clone(),v))).collect();
        for (key,v) in messages {
            if self.language_file_event(&key,&v) { continue; }
            if self.language_status_event(&key, &v) { continue; }
            if v["method"] == "workspace/applyEdit" {
                // applyEdit carries its own document versions. A snapshot from an older
                // user refactor must not veto an unrelated server request.
                let snapshot = std::mem::take(&mut self.languages.rename_snapshot);
                let result = self.language_resource_apply(&v["params"]["edit"]);
                self.languages.rename_snapshot = snapshot;
                if let Some(client) = self.languages.clients.get(&key) {
                    client.respond(v["id"].clone(),match result { Ok(_)=>json!({"applied":true}),Err(e)=>json!({"applied":false,"failureReason":e}) });
                }
                continue;
            }
            if v["method"] == "workspace/semanticTokens/refresh" && self.languages.key.as_ref() == Some(&key) {
                self.semantic_request();
                continue;
            }
            if v["method"] == "workspace/diagnostic/refresh" && self.languages.key.as_ref() == Some(&key) {
                self.language_pull_diagnostics();
                continue;
            }
            if v["clientCapabilities"].is_object() {
                let pull = v["clientCapabilities"]["diagnosticProvider"].is_object();
                self.languages.capabilities.insert(key.clone(), v["clientCapabilities"].clone());
                if self.languages.key.as_ref() == Some(&key) {
                    if pull { self.language_pull_diagnostics(); }
                    self.language_request_symbols();
                    self.semantic_request();
                }
                continue;
            }
            if let Some(why) = v["clientError"].as_str() {
                self.message = Some(why.into());
                self.languages.clients.remove(&key);
                self.languages.failed.insert(key.clone());
                self.languages.opened.retain(|p,_| lsp::server(&self.root,p).ok().flatten().is_none_or(|(id,_)| id != key.1));
                continue;
            }
            if v["method"] == "textDocument/publishDiagnostics" {
                if let Some(path) = v["params"]["uri"].as_str().and_then(lsp::path)
                    && (path.starts_with(&self.root) || self.multi_root() && self.in_roots(&path))
                    && v["params"]["version"].as_u64().is_none_or(|n| self.languages.opened.get(&path).is_none_or(|(v,_)| n == *v))
                {
                    self.languages
                        .diagnostics
                        .insert(path.clone(), (self.languages.opened.get(&path).map_or(0,|(v,_)| *v), v["params"]["diagnostics"].as_array().cloned().unwrap_or_default()));
                }
                continue;
            }
            let Some(Pending { action, path, version, hover_at, cursor, .. }) =
                v["id"].as_u64().and_then(|n| self.languages.pending.remove(&n))
            else {
                continue;
            };
            if action != "execute" && (self.doc.path.as_ref() != Some(&path) || self.doc.buf.version != version) {
                if !BACKGROUND.contains(&action.as_str()) { self.message = Some("Discarded stale language server result".into()); }
                continue;
            }
            if !v["error"].is_null() {
                self.message = Some(format!("Language server: {}", v["error"]["message"]));
                continue;
            }
            // The "working" note goes once its answer is in.
            if self.message.as_deref() == Some(format!("Language server: {action}…").as_str()) { self.message = None; }
            let result = &v["result"];
            match action.as_str() {
                "execute" => { self.message = Some("Server command completed".into()); }
                "lenses" | "lens_resolve" => self.language_feature_result(&action,result),
                "hierarchy_prepare" | "hierarchy_children" => self.hierarchy_result(&action, result),
                "workspace_symbols" => self.language_workspace_result(result),
                "document_symbols" => {
                    self.languages.symbols.insert(path, (version, lsp::document_symbols(&self.doc.buf.text, result)));
                    if matches!(self.menu, Some((MenuKind::Palette, _))) { self.palette_mode = None; self.palette_update(); }
                }
                "pull_diagnostics" => {
                    if result["kind"] == "full" {
                        self.languages.diagnostics.insert(path, (version, result["items"].as_array().cloned().unwrap_or_default()));
                    }
                }
                "selection" => self.selection_ranges(result),
                "lightbulb" => self.lightbulb_result(path, version, cursor, result),
                "semantic_tokens" => self.semantic_result(path, version, result),
                "peek_definition" | "peek_references" => {
                    if cursor == self.doc.buf.cur { self.peek_show(&action, result); }
                }
                "hover" => {
                    fn content(v: &Value) -> String {
                        if let Some(s) = v.as_str() {
                            s.into()
                        } else if let Some(a) = v.as_array() {
                            a.iter().map(content).collect::<Vec<_>>().join("\n\n")
                        } else {
                            v["value"].as_str().unwrap_or("").into()
                        }
                    }
                    let text = content(&result["contents"]);
                    if hover_at.is_none() && cursor != self.doc.buf.cur {
                        continue;
                    }
                    self.language_hover_at(text, hover_at);
                }
                "rename" => match self.language_apply(result) {
                    Ok(n) => self.message = Some(format!("Renamed in {n} files. Review and save the open tabs.")),
                    Err(e) => self.message = Some(e),
                },
                "completion" | "signature" | "format" | "format_selection" | "actions" | "action_resolve" | "inlays" => {
                    if cursor == self.doc.buf.cur || matches!(action.as_str(), "format" | "format_selection" | "inlays") {
                        self.language_feature_result(&action, result);
                    }
                }
                _ => {
                    self.languages.locations = if let Some(a) = result.as_array() {
                        a.clone()
                    } else if result.is_object() {
                        vec![result.clone()]
                    } else {
                        Vec::new()
                    };
                    if self.languages.locations.is_empty() {
                        self.message = Some("No locations found".into());
                    } else if action == "references_editor" {
                        self.multibuffer_locations("References");
                    } else if self.languages.locations.len() == 1 {
                        self.language_pick("0");
                    } else {
                        let mut entries: Vec<Entry> = self
                            .languages
                            .locations
                            .iter()
                            .enumerate()
                            .map(|(i, v)| {
                                let uri = v["uri"].as_str().or(v["targetUri"].as_str()).unwrap_or("");
                                Entry::new(
                                    &i.to_string(),
                                    uri,
                                    &format!(
                                        "Line {}",
                                        v.pointer("/range/start/line")
                                            .or(v.pointer("/targetSelectionRange/start/line"))
                                            .and_then(Value::as_u64)
                                            .unwrap_or(0)
                                            + 1
                                    ),
                                )
                            })
                            .collect();
                        entries.insert(0, Entry::new("editor", "Open all in editor", "Excerpts of every location in one tab"));
                        self.menu = Some((MenuKind::Language, Menu::new("Language server locations", entries, "")));
                    }
                }
            }
        }
        self.languages.pending.retain(|_, p| p.since.elapsed().as_secs() < 30);
        if !self.languages.pending.values().any(|p| p.action.starts_with("hierarchy")) { self.hierarchy_settle(); }
        self.semantic_apply();
        self.doc.diagnostics.clear();
        if let Some(path) = &self.doc.path
            && let Some((version, list)) = self.languages.diagnostics.get(path)
            && *version == self.doc.buf.version
        {
            for d in list {
                if let (Some(a), Some(b)) = (
                    lsp::offset(&self.doc.buf.text, &d["range"]["start"]),
                    lsp::offset(&self.doc.buf.text, &d["range"]["end"]),
                ) {
                    self.doc.diagnostics.push((a, b, d["message"].as_str().unwrap_or("").into()));
                }
            }
        }
    }
    pub(super) fn language_pick(&mut self, key: &str) {
        if let Some(action) = key.strip_prefix("server:") { self.menu = None; self.language_command(action); return; }
        if key == "editor" { self.menu = None; return self.multibuffer_locations("Locations"); }
        let Some(v) = key.parse::<usize>().ok().and_then(|n| self.languages.locations.get(n)).cloned() else { return };
        let Some(path) = v["uri"].as_str().or(v["targetUri"].as_str()).and_then(lsp::path) else { return };
        self.jump_to(path.clone(), 0, 0);
        if self.doc.path.as_ref() != Some(&path) {
            return;
        }
        let range = if v["range"].is_object() { &v["range"] } else { &v["targetSelectionRange"] };
        if let (Some(a), Some(b)) =
            (lsp::offset(&self.doc.buf.text, &range["start"]), lsp::offset(&self.doc.buf.text, &range["end"]))
        {
            self.doc.select(a, b);
            self.doc.moved();
        }
    }
    fn language_apply(&mut self, v: &Value) -> Result<usize, String> {
        if v["documentChanges"].as_array().is_some_and(|a| a.iter().any(|v| v.get("kind").is_some())) { return self.language_resource_apply(v); }
        self.language_text_apply(v)
    }

}
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) const SERVER: &str = r#"import sys,json
uri=None
version=0
rng={'start':{'line':0,'character':0},'end':{'line':0,'character':3}}
def send(v):
 b=json.dumps(v).encode();sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(b)).encode()+b);sys.stdout.buffer.flush()
while True:
 headers={}
 while True:
  line=sys.stdin.buffer.readline()
  if not line:sys.exit()
  if line in (b'\r\n',b'\n'):break
  k,v=line.decode().split(':',1);headers[k.lower()]=v.strip()
 m=json.loads(sys.stdin.buffer.read(int(headers['content-length'])))
 method=m.get('method');p=m.get('params',{})
 if method=='initialize':send({'jsonrpc':'2.0','id':m['id'],'result':{'capabilities':{'textDocumentSync':1,'documentSymbolProvider':True,'diagnosticProvider':{},'callHierarchyProvider':True,'codeActionProvider':True,'workspaceSymbolProvider':True,'semanticTokensProvider':{'legend':{'tokenTypes':['function'],'tokenModifiers':[]},'full':{'delta':True}}}}})
 elif method in ('textDocument/didOpen','textDocument/didChange'):
  uri=p['textDocument']['uri'];version=p['textDocument']['version']
  send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':uri,'version':version,'diagnostics':[{'range':rng,'message':'fixture diagnostic','severity':1}]}})
 elif method=='textDocument/diagnostic':send({'id':m['id'],'result':{'kind':'full','items':[{'range':rng,'message':'fixture diagnostic','severity':1}]}})
 elif method=='textDocument/documentSymbol':send({'id':m['id'],'result':[{'name':'foo','selectionRange':rng,'range':rng,'kind':12}]})
 elif method in ('textDocument/definition','textDocument/declaration'):send({'jsonrpc':'2.0','id':m['id'],'result':{'uri':uri,'range':rng}})
 elif method=='textDocument/implementation':send({'id':m['id'],'result':[{'targetUri':uri,'targetRange':rng,'targetSelectionRange':rng}]})
 elif method=='textDocument/typeDefinition':send({'id':m['id'],'result':[{'uri':uri,'range':rng},{'uri':uri,'range':rng}]})
 elif method=='textDocument/references':send({'jsonrpc':'2.0','id':m['id'],'result':[{'uri':uri,'range':rng},{'uri':uri,'range':rng}]})
 elif method=='textDocument/prepareCallHierarchy':send({'id':m['id'],'result':[{'name':'foo','kind':12,'uri':uri,'range':rng,'selectionRange':rng}]})
 elif method=='callHierarchy/outgoingCalls':send({'id':m['id'],'result':[]})
 elif method=='callHierarchy/incomingCalls':send({'id':m['id'],'result':[{'from':{'name':'caller','kind':12,'uri':uri,'range':rng,'selectionRange':rng},'fromRanges':[rng]}]})
 elif method=='textDocument/semanticTokens/full':send({'id':m['id'],'result':{'resultId':'1','data':[0,0,3,0,0]}})
 elif method=='textDocument/semanticTokens/full/delta':send({'id':m['id'],'result':{'resultId':'2','edits':[{'start':2,'deleteCount':1,'data':[2]}]}})
 elif method=='workspace/symbol':send({'id':m['id'],'result':[{'name':'sym_'+p['query'],'kind':12,'location':{'uri':uri,'range':rng}}]})
 elif method=='textDocument/hover':send({'jsonrpc':'2.0','id':m['id'],'result':{'contents':{'kind':'markdown','value':'Fixture documentation'}}})
 elif method=='textDocument/completion':send({'id':m['id'],'result':{'items':[{'label':'food','textEdit':{'range':rng,'newText':'food'}}]}})
 elif method=='textDocument/signatureHelp':send({'id':m['id'],'result':{'signatures':[{'label':'foo(x: int)','parameters':[{'label':'x: int'}]}],'activeParameter':0}})
 elif method in ('textDocument/formatting','textDocument/rangeFormatting'):send({'id':m['id'],'result':[{'range':rng,'newText':'FOO'}]})
 elif method=='textDocument/inlayHint':send({'id':m['id'],'result':[{'position':rng['end'],'label':': int'}]})
 elif method=='textDocument/codeAction' and p['context'].get('only'):send({'id':m['id'],'result':[{'title':'Organize','kind':'source.organizeImports','edit':{'documentChanges':[{'textDocument':{'uri':uri,'version':version},'edits':[{'range':rng,'newText':'use'}]}]}}]})
 elif method=='textDocument/codeAction':send({'id':m['id'],'result':[{'title':'Fix foo','kind':'quickfix','edit':{'changes':{uri:[{'range':rng,'newText':'bar'}]}}}]})
 elif method=='textDocument/rename':send({'jsonrpc':'2.0','id':m['id'],'result':{'changes':{uri:[{'range':rng,'newText':p['newName']}]}}})
"#;
    fn wait(a: &mut App) {
        let start = std::time::Instant::now();
        while !a.languages.pending.is_empty() && start.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            a.language_poll();
        }
        assert!(a.languages.pending.is_empty(), "LSP request did not finish");
    }
    #[test]
    fn a_stdio_server_drives_navigation_diagnostics_hover_and_undoable_rename() {
        let root = std::env::temp_dir().join(format!("ld-language-fixture-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(
            root.join(".lightdown/lsp.json"),
            json!({"rust":["python3",root.join("server.py")]}).to_string(),
        )
        .unwrap();
        let file = root.join("a #é.rs");
        std::fs::write(&file, "foo\n").unwrap();
        let mut a = App::new(Some(file.clone()));
        a.resize(900, 700, 1.0);
        a.language_command("definition");
        wait(&mut a);
        assert_eq!(a.doc.buf.sel(), (0, 3));
        for action in ["implementation", "declaration"] {
            a.doc.select(0,0); a.language_command(action); wait(&mut a);
            assert_eq!(a.doc.buf.sel(), (0,3), "{action}");
        }
        a.language_command("type_definition"); wait(&mut a);
        assert!(matches!(a.menu, Some((MenuKind::Language, _))));
        a.menu = None;
        assert_eq!(a.doc.diagnostics[0].2, "fixture diagnostic");
        assert_eq!(a.language_symbols().unwrap()[0].title, "foo");
        let command = lsp::server(&root, &file).unwrap().unwrap().1;
        let shared = lsp::shared_query(&root, &command, &lsp::uri(&file), &a.doc.buf.text, "textDocument/hover", json!({"textDocument":{"uri":lsp::uri(&file)},"position":{"line":0,"character":0}}), &std::sync::atomic::AtomicBool::new(false));
        assert!(shared.unwrap().unwrap()["contents"]["value"].as_str().unwrap().contains("Fixture"));
        assert!(lsp::shared_query(&root, &command, &lsp::uri(&file), "different unsaved text", "textDocument/hover", json!({}), &std::sync::atomic::AtomicBool::new(false)).is_none());
        a.language_command("hover");
        wait(&mut a);
        assert!(a.docs_escape());
        a.language_command("references");
        wait(&mut a);
        assert!(matches!(a.menu, Some((MenuKind::Language, _))));
        a.menu = None;
        a.language_rename("bar");
        wait(&mut a);
        assert_eq!(a.doc.buf.text, "bar\n");
        assert!(a.doc.buf.modified);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "foo\n");
        a.doc.key(Key::Char('z'), Mods { ctrl: true, ..Default::default() }, None);
        assert_eq!(a.doc.buf.text, "foo\n");
        a.language_command("definition");
        a.doc.insert("new", EditKind::Other);
        wait(&mut a);
        assert!(a.message.as_deref().unwrap().contains("stale"));
        a.languages = Default::default();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn server_completion_actions_formatting_signature_and_inlays() {
        let root = std::env::temp_dir().join(format!("ld-language-features-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rust":["python3",root.join("server.py")]}).to_string()).unwrap();
        let file = root.join("a.rs");
        std::fs::write(&file,"foo\n").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(900,700,1.0);
        a.doc.select(3,3); a.complete(true); wait(&mut a);
        assert_eq!(a.comp.as_ref().unwrap().items[0].name,"food");
        a.completion_key(Key::Tab,Mods::default());
        assert_eq!(a.doc.buf.text,"food\n");
        a.doc.key(Key::Char('z'),Mods { ctrl:true,..Default::default() },None);
        a.language_command("signature"); wait(&mut a); assert!(a.docs_escape());
        a.language_command("inlays"); wait(&mut a);
        let mut pixels = vec![0;900*700]; a.draw(&mut Canvas::new(&mut pixels,900,700));
        a.doc.select(0,3); a.language_command("format_selection"); wait(&mut a);
        assert_eq!(a.doc.buf.text,"FOO\n");
        a.doc.key(Key::Char('z'),Mods { ctrl:true,..Default::default() },None);
        assert_eq!(a.doc.buf.text,"foo\n");
        a.language_command("format"); wait(&mut a); assert_eq!(a.doc.buf.text,"FOO\n");
        a.language_command("actions"); wait(&mut a); a.menu = None; a.language_action_pick("0");
        assert_eq!(a.doc.buf.text,"bar\n");
        assert_eq!(std::fs::read_to_string(file).unwrap(),"foo\n");
        a.doc.select(1,3); a.language_command("format_selection"); wait(&mut a);
        assert_eq!(a.doc.buf.text,"bar\n");
        assert!(a.message.as_deref().unwrap().contains("outside the selection"));
        a.doc.select(0,0); a.language_command("actions"); wait(&mut a);
        a.menu = None; a.language_action_pick("ask_agent");
        assert!(a.ai.side.input.buf.text.contains("fixture diagnostic"));
        assert!(a.ai.side.input.buf.text.contains("Range, zero-based UTF-16"));
        assert!(a.ai.side.input.buf.text.contains("1: bar"));
        a.languages = Default::default(); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn peek_lists_references_and_opens_the_chosen_one() {
        let root = std::env::temp_dir().join(format!("ld-peek-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rust":["python3",root.join("server.py")]}).to_string()).unwrap();
        let file = root.join("a.rs");
        std::fs::write(&file,"foo\nsecond\n").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(1100,700,1.0);
        a.doc.select(5,5);
        a.language_command("peek_references"); wait(&mut a);
        assert_eq!(a.languages.peek.as_ref().unwrap().items.len(), 2);
        let mut pixels = vec![0;1100*700]; a.draw(&mut Canvas::new(&mut pixels,1100,700));
        assert!(a.hits.iter().any(|(_, h)| *h == Hit::Peek(1)));
        assert!(a.peek_key(Key::Down, Mods::default()));
        assert_eq!(a.languages.peek.as_ref().unwrap().sel, 1);
        assert!(a.peek_key(Key::Enter, Mods::default()));
        assert!(a.languages.peek.is_none());
        assert_eq!(a.doc.buf.sel(), (0,3));
        a.language_command("peek_definition"); wait(&mut a);
        assert!(a.peek_key(Key::Escape, Mods::default()));
        assert!(a.languages.peek.is_none());
        assert!(!a.peek_key(Key::Escape, Mods::default()));
        a.languages = Default::default(); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lightbulb_marks_the_cursor_line_and_opens_actions() {
        let root = std::env::temp_dir().join(format!("ld-bulb-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rust":["python3",root.join("server.py")]}).to_string()).unwrap();
        let file = root.join("a.rs");
        std::fs::write(&file,"foo\n").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(1100,700,1.0);
        a.language_sync(); a.language_settle();
        a.lightbulb_tick(); wait(&mut a);
        let mut pixels = vec![0;1100*700]; a.draw(&mut Canvas::new(&mut pixels,1100,700));
        let (r, _) = *a.hits.iter().find(|(_, h)| *h == Hit::Lightbulb).expect("lightbulb drawn");
        assert!(r.0 < a.doc.column().0 - a.doc.gutter());
        a.lightbulb_tick();
        assert!(a.languages.pending.is_empty(), "the same line is not asked twice");
        a.doc.insert("x", EditKind::Other);
        a.draw(&mut Canvas::new(&mut pixels,1100,700));
        assert!(!a.hits.iter().any(|(_, h)| *h == Hit::Lightbulb));
        a.languages = Default::default(); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn semantic_tokens_overlay_and_follow_deltas() {
        let root = std::env::temp_dir().join(format!("ld-semantic-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rust":["python3",root.join("server.py")]}).to_string()).unwrap();
        let file = root.join("a.rs");
        std::fs::write(&file,"foo\n").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(900,500,1.0);
        assert_eq!(a.doc.attr_at(2) & crate::md::FUNCTION, 0);
        a.language_sync(); a.language_settle();
        assert_ne!(a.doc.attr_at(2) & crate::md::FUNCTION, 0);
        a.doc.select(4,4); a.doc.insert("bar", EditKind::Other);
        assert_eq!(a.doc.attr_at(2) & crate::md::FUNCTION, 0, "stale tokens are not shown");
        a.language_sync(); a.language_settle();
        assert_ne!(a.doc.attr_at(1) & crate::md::FUNCTION, 0);
        assert_eq!(a.doc.attr_at(2) & crate::md::FUNCTION, 0, "the delta shortened the token to two units");
        a.languages = Default::default(); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn save_actions_use_the_running_server_in_one_undo_step() {
        let root = std::env::temp_dir().join(format!("ld-save-actions-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rust":["python3",root.join("server.py")]}).to_string()).unwrap();
        std::fs::write(root.join(".lightdown/settings.json"),json!({"code_actions_on_save":["source.organizeImports"]}).to_string()).unwrap();
        let file = root.join("a.rs");
        std::fs::write(&file,"foo\n").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(900,500,1.0);
        a.language_sync(); a.language_settle();
        a.doc.select(4,4); a.doc.insert("x", EditKind::Other);
        a.save();
        let start = std::time::Instant::now();
        while a.doc.buf.modified && start.elapsed().as_secs() < 10 { std::thread::sleep(std::time::Duration::from_millis(10)); a.saving_poll(); }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "use\nx\n", "{:?}", a.message);
        a.doc.key(Key::Char('z'), Mods { ctrl: true, ..Default::default() }, None);
        assert_eq!(a.doc.buf.text, "foo\nx");
        a.languages = Default::default(); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn agent_rename_checkpoints_and_edits_every_file_or_none() {
        let root = std::env::temp_dir().join(format!("ld-agent-rename-{}", std::process::id())).canonicalize().unwrap_or_else(|_| {
            let r = std::env::temp_dir().join(format!("ld-agent-rename-{}", std::process::id())); std::fs::create_dir_all(&r).unwrap(); r.canonicalize().unwrap()
        });
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let (a_path, b_path) = (root.join("a.rs"), root.join("b.rs"));
        std::fs::write(&a_path, "fn foo() {}\n").unwrap();
        std::fs::write(&b_path, "use foo;\n").unwrap();
        let mut a = App::new(Some(a_path.clone())); a.resize(900,500,1.0);
        let at = |line: u64, from: u64| json!({"range":{"start":{"line":line,"character":from},"end":{"line":line,"character":from+3}},"newText":"bar"});
        let reply = json!({"uri":lsp::uri(&a_path),"result":{"changes":{lsp::uri(&a_path):[at(0,3)],lsp::uri(&b_path):[at(0,4)]}}}).to_string();
        // A stale request is refused before anything changes.
        a.agent_rename_started(&a_path, "fn foo() {}\nold\n");
        let (said, ok) = a.agent_rename_apply(&reply);
        assert!(!ok && said.contains("changed"), "{said}");
        assert_eq!(std::fs::read_to_string(&b_path).unwrap(), "use foo;\n");
        a.agent_rename_started(&a_path, "fn foo() {}\n");
        let (said, ok) = a.agent_rename_apply(&reply);
        assert!(ok, "{said}");
        assert!(said.contains("2 occurrences in 2 files"), "{said}");
        assert_eq!(a.doc.buf.text, "fn bar() {}\n");
        assert_eq!(std::fs::read_to_string(&a_path).unwrap(), "fn bar() {}\n", "a saved open file stays saved");
        assert_eq!(std::fs::read_to_string(&b_path).unwrap(), "use bar;\n");
        let outside = json!({"result":{"changes":{"file:///etc/hostname":[at(0,0)]}}}).to_string();
        a.agent_rename_started(&a_path, &a.doc.buf.text.clone());
        assert!(!a.agent_rename_apply(&outside).1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn project_symbols_follow_the_typed_query() {
        let root = std::env::temp_dir().join(format!("ld-workspace-symbols-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rust":["python3",root.join("server.py")]}).to_string()).unwrap();
        let file = root.join("a.rs");
        std::fs::write(&file,"foo\n").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(900,500,1.0);
        a.language_sync(); a.language_settle();
        a.open_palette("#"); a.language_settle();
        for c in "ab".chars() { a.key(Key::Char(c), Mods::default(), Some(&c.to_string())); }
        a.language_settle();
        let Some((MenuKind::Palette, menu)) = &a.menu else { panic!("palette closed") };
        assert!(menu.entries.iter().any(|e| e.label == "sym_ab"), "{:?}", menu.entries.iter().map(|e| e.label.clone()).collect::<Vec<_>>());
        a.languages = Default::default(); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn call_hierarchy_expands_lazily_and_collapses() {
        let root = std::env::temp_dir().join(format!("ld-hierarchy-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rust":["python3",root.join("server.py")]}).to_string()).unwrap();
        let file = root.join("a.rs");
        std::fs::write(&file,"foo\n").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(1100,700,1.0);
        a.key(Key::Char('h'), Mods { ctrl: true, alt: true, shift: true, ..Default::default() }, None); wait(&mut a);
        a.hierarchy_rows();
        assert_eq!(a.outline.items[0].title, "Outgoing calls", "Ctrl+Alt+Shift+H is not the git hunk key");
        a.key(Key::Char('h'), Mods { alt: true, shift: true, ..Default::default() }, None); wait(&mut a);
        assert_eq!(a.tab, Tab::Hierarchy);
        a.hierarchy_rows();
        let titles: Vec<_> = a.outline.items.iter().map(|i| i.title.clone()).collect();
        assert_eq!(titles[0], "Incoming calls");
        assert!(titles[1].starts_with("▾ foo  a.rs:1"), "{titles:?}");
        assert!(titles[2].starts_with("▸ caller"), "{titles:?}");
        assert_eq!(a.outline.items[2].depth, 1);
        a.hierarchy_pick(0);
        a.hierarchy_rows();
        assert_eq!(a.outline.items.len(), 2);
        a.hierarchy_pick(0); wait(&mut a);
        a.hierarchy_rows();
        assert_eq!(a.outline.items.len(), 3);
        let mut pixels = vec![0;1100*700]; a.draw(&mut Canvas::new(&mut pixels,1100,700));
        a.languages = Default::default(); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn problems_group_files_skip_stale_diagnostics_and_navigate_utf16() {
        let root = std::env::temp_dir().join(format!("ld-ab9-problems-{}",std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let file = root.join("a.rs"); std::fs::write(&file,"a😀b\nsecond").unwrap();
        let mut a = App::new(Some(file.clone())); a.resize(1100,700,1.0);
        let d = json!({"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":4}},"message":"fixture"});
        a.languages.diagnostics.insert(file.clone(),(a.doc.buf.version,vec![d]));
        assert_eq!(a.problem_list().len(),1); a.next_problem(false); assert_eq!(a.doc.buf.cur,5);
        a.show_side(Some(Tab::Problems));
        let mut pixels = vec![0;1100*700]; a.draw(&mut Canvas::new(&mut pixels,1100,700));
        assert!(a.problems.rect.2 > 0.0);
        a.doc.insert("new",EditKind::Other); assert!(a.problem_list().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rename_validates_every_target_before_editing_any() {
        let root = std::env::temp_dir().join(format!("ld-rename-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let a_path = root.join("a.rs");
        let b_path = root.join("b.rs");
        std::fs::write(&a_path, "foo").unwrap();
        std::fs::write(&b_path, "foo").unwrap();
        let mut a = App::new(Some(a_path.clone()));
        a.languages.rename_snapshot.insert(a_path.clone(), ("foo".into(), "foo".into()));
        a.languages.rename_snapshot.insert(b_path.clone(), ("foo".into(), "foo".into()));
        let edit = json!([{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"bar"}]);
        let request = json!({"changes":{lsp::uri(&a_path):edit.clone(),lsp::uri(&b_path):edit}});
        std::fs::write(&b_path, "later").unwrap();
        assert!(a.language_apply(&request).is_err());
        assert_eq!(a.doc.buf.text, "foo");
        assert!(!a.doc.buf.modified);
        std::fs::remove_dir_all(root).unwrap();
    }
}
