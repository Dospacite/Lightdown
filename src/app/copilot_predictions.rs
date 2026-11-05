//! Editor integration for the separate Copilot RPC client.
use super::*;
use crate::copilot::{self, Client, Event};
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub(super) struct State {
    client: Option<Client>,
    install: Option<Receiver<Result<(), String>>>,
    pending: Option<(u64, super::predictions::Context)>,
    pub item: Option<Value>,
    failed: bool,
}
impl App {
    pub(super) fn copilot_action(&mut self, action: &str) {
        match action {
            "install_confirm" => {
                if self.copilot.install.is_some() {
                    return;
                }
                let (tx, rx) = mpsc::channel();
                let wake = self.waker.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(copilot::install());
                    wake();
                });
                self.copilot.install = Some(rx);
                self.message = Some("Installing Copilot language server…".into());
            }
            "install" => {
                let menu = ui::Menu::new(
                    "Install GitHub Copilot language server?",
                    vec![
                        ui::Entry::new(
                            "copilot_install_confirm",
                            "Install with npm",
                            "Downloads @github/copilot-language-server into the Lightdown data directory",
                        ),
                        ui::Entry::new("cancel", "Cancel", ""),
                    ],
                    "",
                );
                self.menu = Some((assist::MenuKind::Copilot, menu));
            }
            "sign_in" => {
                self.prediction_clear_cache();
                if !copilot::executable().is_file() {
                    self.copilot_action("install");
                    return;
                }
                self.copilot.failed = false;
                if self.copilot_start() {
                    self.copilot.client.as_mut().unwrap().request("signIn", json!({}));
                }
            }
            "sign_out" => {
                self.prediction_clear_cache();
                self.unsuggest();
                self.copilot.item = None;
                if copilot::executable().is_file() && self.copilot_start() {
                    self.copilot.client.as_mut().unwrap().request("signOut", json!({}));
                }
            }
            _ => {}
        }
    }
    fn copilot_start(&mut self) -> bool {
        if self.copilot.client.is_some() {
            return true;
        }
        if self.copilot.failed {
            return false;
        }
        match Client::start(&self.root, self.waker.clone()) {
            Ok(client) => {
                self.copilot.client = Some(client);
                true
            }
            Err(e) => {
                self.copilot.failed = true;
                self.message = Some(format!("Copilot: {e}"));
                false
            }
        }
    }
    pub(super) fn copilot_predict(&mut self, cx: super::predictions::Context) {
        if !copilot::executable().is_file() {
            self.message = Some("Copilot: use Copilot: Sign in to install and authenticate".into());
            return;
        }
        if !self.copilot_start() {
            return;
        }
        let path = self.doc.path.clone().unwrap_or_else(|| self.root.join("untitled.txt"));
        let uri = crate::lsp::uri(&path);
        let client = self.copilot.client.as_mut().unwrap();
        let mut uris = vec![uri.clone()];
        // Synchronize related open documents as well, without starting another LSP client.
        for tab in &self.file_tabs {
            if let Some(doc) = &tab.state
                && doc.root == self.root
                && doc.notebook.is_none()
                && doc.pane.buf.text.len() <= 1024 * 1024
                && let Some(path) = &doc.pane.path
            {
                let uri = crate::lsp::uri(path);
                client.sync(&uri, language(path), doc.pane.buf.version, &doc.pane.source_text());
                uris.push(uri);
            }
        }
        client.close_except(&uris);
        client.sync(&uri, language(&path), self.doc.buf.version, &self.doc.buf.text);
        let id = client.request("textDocument/inlineCompletion",json!({"textDocument":{"uri":uri,"version":self.doc.buf.version},
            "position":crate::lsp::position(&self.doc.buf.text,cx.cur),"context":{"triggerKind":2},"formattingOptions":{"tabSize":4,"insertSpaces":true}}));
        self.copilot.pending = Some((id, cx));
    }
    pub(super) fn copilot_pending(&self) -> bool {
        self.copilot.pending.is_some()
    }
    pub(super) fn cancel_copilot_prediction(&mut self) {
        if let Some((id, _)) = self.copilot.pending.take()
            && let Some(client) = &mut self.copilot.client
        {
            client.cancel(id);
        }
        self.copilot.item = None;
    }
    pub(super) fn copilot_accept(&mut self, item: Option<Value>) {
        if let (Some(client), Some(item)) = (&mut self.copilot.client, item)
            && item["command"]["command"].is_string()
        {
            client.request("workspace/executeCommand", item["command"].clone());
        }
    }
    pub(super) fn copilot_poll(&mut self) {
        if let Some(result) = self.copilot.install.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.copilot.install = None;
            match result {
                Ok(()) => self.copilot_action("sign_in"),
                Err(e) => self.message = Some(e),
            }
        }
        let events = self.copilot.client.as_mut().map_or(Vec::new(), Client::poll);
        for event in events {
            match event {
                Event::Message(msg) => self.message = Some(msg),
                Event::Code(code, url) => {
                    self.copy = Some(code.clone());
                    self.message = Some(format!("Copilot code {code} (copied). Finish sign-in at {url}"));
                }
                Event::OpenUrl(url) => open_url(&url),
                Event::Failed(e) => {
                    self.copilot.client = None;
                    self.copilot.pending = None;
                    self.copilot.failed = true;
                    self.message = Some(format!("Copilot: {e}"));
                }
                Event::Completion(id, value) => {
                    if self.copilot.pending.as_ref().is_none_or(|(pending, _)| *pending != id) {
                        continue;
                    }
                    let (_, cx) = self.copilot.pending.take().unwrap();
                    if (cx.tab, cx.version, cx.cur)
                        != (self.file_tabs[self.active_file].id, self.doc.buf.version, self.doc.buf.cur)
                    {
                        continue;
                    }
                    let mut edit = None;
                    if let Some((a, b, text, item)) = copilot::edit(&self.doc.buf.text, cx.cur, &value) {
                        edit = super::predictions::difference(&self.doc.buf.text[a..b], &text, a);
                        if edit.as_ref().is_some_and(|e| {
                            e.text.lines().count() <= 40 && self.doc.buf.text[e.a..e.b].lines().count() <= 40
                        }) {
                            self.copilot.item = Some(item.clone());
                            if let Some(client) = &mut self.copilot.client {
                                client.notify("textDocument/didShowCompletion", json!({"item":item}));
                            }
                        } else {
                            edit = None;
                        }
                    }
                    if !value.is_null() {
                        self.prediction_cache(cx.key, edit.clone());
                    }
                    self.prediction_show(edit);
                }
            }
        }
    }
    pub(super) fn copilot_status(&self) -> Option<String> {
        if self.copilot.install.is_some() {
            return Some("Copilot: installing".into());
        }
        if let Some(client) = &self.copilot.client {
            return Some(format!("Copilot: {}", client.status));
        }
        (self.cfg.edit_prediction_provider == "copilot")
            .then(|| if self.copilot.failed { "Copilot: stopped; sign in to retry" } else { "Copilot: sign in" }.into())
    }
}
fn open_url(url: &str) {
    if url.starts_with("https://github.com/") {
        let _ = std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}
fn language(path: &std::path::Path) -> &str {
    match path.extension().and_then(|s| s.to_str()).unwrap_or("text") {
        "py" => "python",
        "rs" => "rust",
        "md" => "markdown",
        "tex" => "latex",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "jsx" => "javascriptreact",
        ext => ext,
    }
}
