//! Optional LSP features share the navigation client's version checks.
use super::*;
#[derive(Default)]
pub(super) struct Features {
    hide_diagnostics: bool,
    completions: Vec<Value>,
    completion_at: Option<(PathBuf, u64, usize)>,
    actions: Vec<Value>,
    format_range: Option<(usize, usize)>,
    hints: Vec<Value>,
    hints_at: Option<(PathBuf, u64)>,
}
fn label(v: &Value) -> String {
    if let Some(s) = v.as_str() { s.into() } else {
        v.as_array().into_iter().flatten().filter_map(|p| p["value"].as_str()).collect::<String>()
    }
}
impl App {
    pub(in crate::app) fn language_complete(&mut self, force: bool) -> bool {
        // Python retains live interpreter completion alongside type and lint diagnostics.
        if self.nb.is_some() || self.doc.doc == Doc::Python || self.doc.doc == Doc::Latex { return false; }
        self.language_sync();
        if self.languages.key.as_ref().is_none_or(|k| !self.languages.clients.contains_key(k)) { return false; }
        if !force && self.doc.buf.cur > 0 && self.doc.attr_at(self.doc.buf.cur - 1) & (crate::md::DIM | crate::md::SOFT) != 0 { return false; }
        let Some(query) = py::query(&self.doc.buf.text, self.doc.buf.cur) else { return false };
        if !force && query.prefix.is_empty() && !self.doc.buf.text[..self.doc.buf.cur].ends_with('.') { return false; }
        let items = self.snippet_items(&query.prefix);
        self.comp = Some(Completion { query, items, sel: 0, top: 0, seq: 0 });
        self.languages.features.completions.clear();
        let path = self.doc.path.as_ref().unwrap();
        self.language_request("completion", "textDocument/completion", json!({"textDocument":{"uri":lsp::uri(path)},"position":lsp::position(&self.doc.buf.text,self.doc.buf.cur),"context":{"triggerKind":1}}));
        true
    }
    pub(in crate::app) fn language_feature_command(&mut self, action: &str) -> bool {
        if action == "inline_diagnostics" { self.languages.features.hide_diagnostics = !self.languages.features.hide_diagnostics; return true; }
        let Some(path) = &self.doc.path else { return false };
        let mut params = json!({"textDocument":{"uri":lsp::uri(path)},"position":lsp::position(&self.doc.buf.text,self.doc.buf.cur)});
        let method = match action {
            "lenses" => { params = json!({"textDocument":{"uri":lsp::uri(path)}}); "textDocument/codeLens" }
            "signature" => "textDocument/signatureHelp",
            "format_selection" => {
                let (a,b) = self.doc.buf.sel();
                if a == b { self.message = Some("Select text to format".into()); return true; }
                self.languages.features.format_range = Some((a,b));
                params = json!({"textDocument":{"uri":lsp::uri(path)},"range":{"start":lsp::position(&self.doc.buf.text,a),"end":lsp::position(&self.doc.buf.text,b)},"options":{"tabSize":4,"insertSpaces":true}});
                "textDocument/rangeFormatting"
            }
            "format" => {
                params = json!({"textDocument":{"uri":lsp::uri(path)},"options":{"tabSize":4,"insertSpaces":true}});
                "textDocument/formatting"
            }
            "actions" => {
                if !self.language_snapshot() { return true; }
                let (a,b) = self.doc.buf.sel();
                params["range"] = json!({"start":lsp::position(&self.doc.buf.text,a),"end":lsp::position(&self.doc.buf.text,b)});
                params["context"] = json!({"diagnostics":self.languages.diagnostics.get(self.doc.path.as_ref().unwrap()).map(|(_,d)| d.clone()).unwrap_or_default()});
                "textDocument/codeAction"
            }
            "inlays" => {
                if self.languages.features.hints_at.take().is_some() {
                    self.languages.features.hints.clear();
                    return true;
                }
                params["range"] = json!({"start":{"line":0,"character":0},"end":lsp::position(&self.doc.buf.text,self.doc.buf.text.len())});
                "textDocument/inlayHint"
            }
            _ => return false,
        };
        self.language_request(action, method, params);
        true
    }
    pub(in crate::app) fn language_feature_result(&mut self, action: &str, result: &Value) {
        match action {
            "completion" => {
                if self.comp.is_none() { return; }
                let list = result.as_array().or_else(|| result["items"].as_array());
                let mut items = list.cloned().unwrap_or_default();
                for item in &mut items {
                    if item["textEdit"].is_null() && result["itemDefaults"]["editRange"].is_object() {
                        item["textEdit"] = json!({"range":result["itemDefaults"]["editRange"],"newText":item["textEditText"].as_str().or(item["insertText"].as_str()).or(item["label"].as_str()).unwrap_or("")});
                    }
                    if item["insertTextFormat"].is_null() { item["insertTextFormat"] = result["itemDefaults"]["insertTextFormat"].clone(); }
                }
                items.sort_by(|a,b| a["sortText"].as_str().or(a["label"].as_str()).cmp(&b["sortText"].as_str().or(b["label"].as_str())));
                items.truncate(60);
                if let Some(c) = &mut self.comp {
                    for i in &items {
                        if let Some(name) = i["label"].as_str() { c.items.push(Candidate { name: name.into(), kind: "lsp".into() }); }
                    }
                }
                self.languages.features.completion_at = self.doc.path.clone().map(|p| (p,self.doc.buf.version,self.doc.buf.cur));
                self.languages.features.completions = items;
            }
            "signature" => self.language_signature(result),
            "format" | "format_selection" => {
                let changes = result.as_array().cloned().unwrap_or_default();
                if action == "format_selection" {
                    let Some((a,b)) = self.languages.features.format_range.take() else { return };
                    if changes.iter().any(|e| {
                        let start = lsp::offset(&self.doc.buf.text,&e["range"]["start"]);
                        let end = lsp::offset(&self.doc.buf.text,&e["range"]["end"]);
                        start.is_none_or(|s| s < a) || end.is_none_or(|e| e > b)
                    }) { self.message = Some("Server formatting extends outside the selection; refused".into()); return; }
                }
                match lsp::edits(&self.doc.buf.text, &changes) {
                    Ok(out) => { self.doc.edit(0,self.doc.buf.text.len(),&out,EditKind::Other); self.doc.buf.commit(); self.edited(); self.message = Some("Formatted buffer. Review and save.".into()); }
                    Err(e) => self.message = Some(e),
                }
            }
            "lenses" | "actions" => {
                self.languages.features.actions = result.as_array().cloned().unwrap_or_default().into_iter().filter(|v| v["disabled"].is_null()).collect();
                let mut entries: Vec<Entry> = self.languages.features.actions.iter().enumerate().map(|(i,v)| Entry::new(&i.to_string(),v["title"].as_str().or(v["command"]["title"].as_str()).unwrap_or("Code action"),v["kind"].as_str().unwrap_or(""))).collect();
                if action != "lenses" { entries.push(Entry::new("ask_agent", "Ask agent to fix", "Draft diagnostic context for review")); }
                self.menu = Some((MenuKind::LanguageActions,Menu::new(if action == "lenses" { "Code lenses" } else { "Code actions and quick fixes" },entries,"")));
            }
            "action_resolve" | "lens_resolve" => self.language_action_apply(result),
            "inlays" => {
                self.languages.features.hints = result.as_array().cloned().unwrap_or_default();
                self.languages.features.hints_at = self.doc.path.clone().map(|p| (p,self.doc.buf.version));
            }
            _ => {}
        }
    }
    pub(in crate::app) fn language_action_pick(&mut self, key: &str) {
        if key == "ask_agent" { self.language_ask_fix(); return; }
        let Some(v) = key.parse::<usize>().ok().and_then(|i| self.languages.features.actions.get(i)).cloned() else { return };
        if v["range"].is_object() && v["command"].is_null() { self.language_request("lens_resolve","codeLens/resolve",v); }
        else if v["edit"].is_null() && !v["data"].is_null() { self.language_request("action_resolve","codeAction/resolve",v); }
        else { self.language_action_apply(&v); }
    }
    pub(in crate::app) fn language_ask_fix(&mut self) {
        let line = self.doc.buf.line_of(self.doc.buf.cur)+1;
        let problems: Vec<_> = self.language_problems().into_iter().filter(|p| p.path == self.doc.path && p.line == line).collect();
        if problems.is_empty() { self.message = Some("No diagnostic on the cursor line".into()); return; }
        for p in problems { self.ask_problem(&p); }
    }
    fn language_action_apply(&mut self, v: &Value) {
        if v["edit"].is_object() {
            match self.language_apply(&v["edit"]) {
                Ok(n) => self.message = Some(format!("Applied action in {n} buffers. Review and save.")),
                Err(e) => { self.message = Some(e); return; }
            }
        }
        if !v["command"].is_null() {
            self.language_execute(v);
        }
    }
    pub(in crate::app) fn language_completion_take(&mut self, start: usize, name: &str) -> bool {
        let Some(v) = self.languages.features.completions.iter().find(|v| v["label"] == name).cloned() else { return false };
        if self.languages.features.completion_at.as_ref() != self.doc.path.as_ref().map(|p| (p.clone(),self.doc.buf.version,self.doc.buf.cur)).as_ref() {
            self.message = Some("Completion expired. Request it again.".into()); return true;
        }
        let range = v["textEdit"]["range"].as_object().map(|_| v["textEdit"]["range"].clone()).or_else(|| v["textEdit"]["replace"].as_object().map(|_| v["textEdit"]["replace"].clone())).unwrap_or_else(|| json!({"start":lsp::position(&self.doc.buf.text,start),"end":lsp::position(&self.doc.buf.text,self.doc.buf.cur)}));
        let body = v["textEdit"]["newText"].as_str().or(v["insertText"].as_str()).unwrap_or(name);
        let (body, expansion) = if v["insertTextFormat"] == 2 {
            match crate::snippets::expand_full(&crate::snippets::variables(body, &crate::snippets::context(&self.root))) { Ok(e) => (e.text.clone(), Some(e)), Err(e) => { self.message = Some(e); return true; } }
        } else { (body.to_string(),None) };
        let mut changes = v["additionalTextEdits"].as_array().cloned().unwrap_or_default();
        let Some(a) = lsp::offset(&self.doc.buf.text,&range["start"]) else { return true };
        // Additional edits before the insertion shift its stops and caret.
        let mut at = a as isize;
        for e in &changes {
            if let (Some(s),Some(t),Some(body)) = (lsp::offset(&self.doc.buf.text,&e["range"]["start"]),lsp::offset(&self.doc.buf.text,&e["range"]["end"]),e["newText"].as_str()) {
                if t <= a { at += body.len() as isize - (t-s) as isize; }
            }
        }
        changes.push(json!({"range":range,"newText":body}));
        match lsp::edits(&self.doc.buf.text,&changes) {
            Ok(out) => {
                self.doc.edit(0,self.doc.buf.text.len(),&out,EditKind::Other);
                let at = at.max(0) as usize;
                self.doc.select(at+body.len(),at+body.len());
                self.doc.buf.commit();
                if let Some(e) = expansion { self.doc.start_snippet(at, e); }
                self.doc.moved(); self.edited();
                self.snippet_offer_choices();
            }
            Err(e) => self.message = Some(e),
        }
        true
    }
    /// The most severe diagnostic of each line, when the list matches this version.
    fn line_diagnostics(&self) -> std::collections::BTreeMap<usize, (u64, String)> {
        let mut lines = std::collections::BTreeMap::new();
        let Some((version, diagnostics)) = self.doc.path.as_ref().and_then(|p| self.languages.diagnostics.get(p)) else { return lines };
        if *version != self.doc.buf.version || self.languages.features.hide_diagnostics { return lines; }
        for d in diagnostics {
            let Some(at) = lsp::offset(&self.doc.buf.text, &d["range"]["start"]) else { continue };
            let severity = d["severity"].as_u64().unwrap_or(1).clamp(1, 4);
            let message = d["message"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string();
            let entry = lines.entry(self.doc.buf.line_of(at)).or_insert((severity, message.clone()));
            if severity < entry.0 { *entry = (severity, message); }
        }
        lines
    }
    /// Where text after a line's end goes: x, row top and row height.
    /// Kept apart so a horizontally scrolled view can supply its own.
    fn line_end_spot(&mut self, line: usize) -> Option<(f32, f32, f32)> {
        let (_, end) = self.doc.buf.line(line);
        self.doc.spot(end)
    }
    /// Inlay hints, then the line's diagnostic message, after the source.
    pub(in crate::app) fn draw_language_inlays(&mut self, c: &mut Canvas) {
        let mut hints = std::collections::BTreeMap::<usize,Vec<String>>::new();
        if self.languages.features.hints_at.as_ref() == self.doc.path.as_ref().map(|p| (p.clone(),self.doc.buf.version)).as_ref() {
            for h in &self.languages.features.hints {
                if let Some(at) = lsp::offset(&self.doc.buf.text,&h["position"]) { hints.entry(self.doc.buf.line_of(at)).or_default().push(label(&h["label"])); }
            }
        }
        let diagnostics = self.line_diagnostics();
        if hints.is_empty() && diagnostics.is_empty() { return; }
        let mut lines: Vec<usize> = hints.keys().chain(diagnostics.keys()).copied().collect();
        lines.sort_unstable(); lines.dedup();
        let (left,top,width,height) = self.doc.bounds();
        let s = self.scale;
        let size = theme::SMALL * s;
        // Placing a line may lay it out, which takes the fonts, so place them all first.
        let spots: Vec<(usize, (f32, f32, f32))> =
            lines.into_iter().filter_map(|line| self.line_end_spot(line).map(|spot| (line, spot))).collect();
        let fonts = self.fonts.clone();
        let mut fonts = fonts.borrow_mut();
        // Off the gutter, which stays put while the text scrolls sideways.
        let text_left = self.doc.column().0.max(left);
        c.clip(text_left,top,(left + width - text_left).max(0.0),height);
        for (line, (x,y,h)) in spots {
            let base = ui::base(y, h, size);
            let mut x = x + 12.0 * s;
            if let Some(labels) = hints.get(&line) {
                let text = ui::fit(&mut fonts,font::UI,size,&labels.join("  "),(left+width-x-8.0*s).max(0.0));
                ui::text(c,&mut fonts,x,base,&text,font::UI,size,theme::faint());
                x += fonts.width(font::UI,size,&text) + 16.0 * s;
            }
            if let Some((severity, message)) = diagnostics.get(&line) {
                let room = left + width - x - 8.0 * s;
                if room < 40.0 * s { continue; }
                let text = ui::fit(&mut fonts, font::UI, size, message, room);
                let ink = match severity { 1 => theme::danger(), 2 => theme::warning(), _ => theme::dim() };
                ui::text(c, &mut fonts, x, base, &text, font::UI, size, ink);
            }
        }
        c.unclip();
    }
}
