//! Semantic tokens laid over the base highlighting, kept per file and version.
use super::*;
#[derive(Default)]
pub(super) struct Semantic {
    /// File: the version the tokens describe, the result id for deltas, and the raw data.
    tokens: HashMap<PathBuf, (u64, Option<String>, Vec<u32>)>,
    /// File, version and span count of what the pane holds, so a reparse is noticed.
    applied: Option<(PathBuf, u64, usize)>,
}
impl App {
    fn semantic_provider(&self) -> Option<Value> {
        let caps = self.languages.capabilities.get(self.languages.key.as_ref()?)?;
        let p = &caps["semanticTokensProvider"];
        p["legend"]["tokenTypes"].is_array().then(|| p.clone())
    }
    /// Asks for the current file's tokens: a delta when the server keeps them, else full, else the whole range.
    pub(in crate::app) fn semantic_request(&mut self) {
        let Some(provider) = self.semantic_provider() else { return };
        let Some(path) = self.doc.path.clone() else { return };
        if self.languages.pending.values().any(|p| p.action == "semantic_tokens" && p.path == path && p.version == self.doc.buf.version) { return; }
        let uri = lsp::uri(&path);
        let previous = self.languages.semantic.tokens.get(&path).and_then(|(_, id, _)| id.clone());
        let full = &provider["full"];
        let (method, params) = if let (Some(id), true) = (previous, full["delta"] == true) {
            ("textDocument/semanticTokens/full/delta", json!({"textDocument":{"uri":uri},"previousResultId":id}))
        } else if full == true || full.is_object() {
            ("textDocument/semanticTokens/full", json!({"textDocument":{"uri":uri}}))
        } else if provider["range"] == true || provider["range"].is_object() {
            let text = &self.doc.buf.text;
            ("textDocument/semanticTokens/range", json!({"textDocument":{"uri":uri},"range":{"start":lsp::position(text,0),"end":lsp::position(text,text.len())}}))
        } else {
            return;
        };
        self.language_request("semantic_tokens", method, params);
    }
    pub(in crate::app) fn semantic_result(&mut self, path: PathBuf, version: u64, result: &Value) {
        let old = self.languages.semantic.tokens.get(&path).map(|(_, _, d)| d.clone()).unwrap_or_default();
        let data = if result["edits"].is_array() {
            match lsp::semantic::apply_delta(&old, &result["edits"]) {
                Some(d) => d,
                None => {
                    // A delta that does not fit is dropped and the whole set asked for again.
                    self.languages.semantic.tokens.remove(&path);
                    return self.semantic_request();
                }
            }
        } else if result.is_null() {
            return;
        } else {
            lsp::semantic::data(result)
        };
        let id = result["resultId"].as_str().map(str::to_string);
        self.languages.semantic.tokens.insert(path, (version, id, data));
        self.languages.semantic.applied = None;
        self.semantic_apply();
    }
    /// Lays the stored tokens over the pane unless they are stale or already there.
    pub(in crate::app) fn semantic_apply(&mut self) {
        let Some(path) = self.doc.path.clone() else { return };
        let Some((version, _, data)) = self.languages.semantic.tokens.get(&path) else { return };
        if *version != self.doc.buf.version || self.languages.semantic.applied == Some((path.clone(), *version, self.doc.span_count())) { return; }
        let Some(roles) = self.languages.key.as_ref().and_then(|k| self.languages.capabilities.get(k)).and_then(lsp::semantic::legend) else { return };
        let spans = lsp::semantic::spans(&self.doc.buf.text, data, &roles);
        let version = *version;
        if self.doc.overlay_spans(&spans) {
            self.languages.semantic.applied = Some((path, version, self.doc.span_count()));
        }
    }
}
