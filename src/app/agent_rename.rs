//! The agent's lsp_rename: the server's workspace edit, validated as a whole,
//! then applied file by file through the same checkpoints and buffers as edit_file.
use super::*;
impl App {
    /// Remembers the text the server is asked about, to refuse a stale reply.
    pub(in crate::app) fn agent_rename_started(&mut self, path: &Path, text: &str) {
        self.languages.agent_rename = Some((path.to_path_buf(), text.to_string()));
    }
    /// The open tab holding a file of this project, if any.
    fn agent_rename_tab(&self, path: &Path) -> Option<usize> {
        if self.doc.path.as_deref() == Some(path) { return Some(self.active_file); }
        self.file_tabs.iter().position(|t| t.state.as_ref().is_some_and(|s| s.root == self.root && s.pane.path.as_deref() == Some(path)))
    }
    fn agent_rename_text(&self, path: &Path) -> Result<(String, bool), String> {
        let name = path.strip_prefix(&self.root).unwrap_or(path).display().to_string();
        if let Some(i) = self.agent_rename_tab(path) {
            let pane = if i == self.active_file { &self.doc } else { &self.file_tabs[i].state.as_ref().unwrap().pane };
            if pane.read_only || pane.doc == Doc::Cells { return Err(format!("{name} is not editable source")); }
            return Ok((pane.buf.text.clone(), pane.buf.modified));
        }
        if std::fs::metadata(path).map_or(true, |m| m.len() > 2 << 20) { return Err(format!("{name} is missing or over 2 MB")); }
        let text = std::fs::read_to_string(path).map_err(|e| format!("Cannot read {name}: {e}"))?;
        Ok((text.replace("\r\n", "\n"), false))
    }
    /// Turns the tool's server reply into edits. Returns what the model is told and whether it worked.
    pub(in crate::app) fn agent_rename_apply(&mut self, out: &str) -> (String, bool) {
        match self.agent_rename_edit(out) {
            Ok(said) => (said, true),
            Err(why) => (format!("Rename not applied: {why}"), false),
        }
    }
    fn agent_rename_edit(&mut self, out: &str) -> Result<String, String> {
        let (asked, sent) = self.languages.agent_rename.take().ok_or("no rename was pending")?;
        let reply: Value = serde_json::from_str(out).map_err(|e| e.to_string())?;
        let edit = &reply["result"];
        if edit.is_null() { return Err("the server found no symbol to rename at that position".into()); }
        if edit["documentChanges"].as_array().is_some_and(|a| a.iter().any(|v| v.get("kind").is_some())) {
            if self.agent_rename_text(&asked)?.0 != sent { return Err("File changed while the server worked".into()); }
            let n = self.language_resource_apply(edit)?;
            return Ok(format!("Applied rename across {n} paths; checkpoint available for review"));
        }
        let mut grouped = std::collections::BTreeMap::<PathBuf, Vec<Value>>::new();
        for (uri, edits) in edit["changes"].as_object().into_iter().flatten() {
            grouped.entry(lsp::path(uri).ok_or("unsupported URI")?).or_default().extend(edits.as_array().ok_or("invalid edits")?.clone());
        }
        for d in edit["documentChanges"].as_array().into_iter().flatten() {
            let path = d["textDocument"]["uri"].as_str().and_then(lsp::path).ok_or("invalid target")?;
            grouped.entry(path).or_default().extend(d["edits"].as_array().ok_or("missing edits")?.clone());
        }
        if grouped.is_empty() { return Err("the server returned no changes".into()); }
        let root = self.root.canonicalize().map_err(|e| e.to_string())?;
        let asked = asked.canonicalize().unwrap_or(asked);
        // Every file is checked and every new text computed before anything changes.
        let mut plan = Vec::new();
        let mut count = 0;
        for (path, edits) in grouped {
            let path = path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?;
            if !path.starts_with(&root) { return Err(format!("{} is outside the project", path.display())); }
            if doc_for(Some(&path)) == Doc::Cells || pdf::is_pdf(&path) || is_picture(&path) { return Err(format!("{} is not editable source", path.display())); }
            let (text, unsaved) = self.agent_rename_text(&path)?;
            let name = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
            if path == asked {
                if text != sent { return Err(format!("{name} changed while the server worked; ask again")); }
            } else if unsaved {
                return Err(format!("{name} has unsaved changes the server may not have seen; save it and ask again"));
            }
            let new = lsp::edits(&text, &edits).map_err(|e| format!("{name}: {e}"))?;
            count += edits.len();
            if new != text { plan.push((path, name, text, new)); }
        }
        let original = self.active_file;
        let mut done = Vec::new();
        for (path, name, old, new) in plan {
            self.checkpoint_file(&path)?;
            let said = if let Some(i) = self.agent_rename_tab(&path) {
                self.activate_file(i);
                if self.doc.path.as_deref() != Some(&path) || self.doc.buf.text != old { return Err(format!("{name} changed while opening; {} files were renamed before it", done.len())); }
                self.rewrite_open(&old, &new)?
            } else {
                super::write_file(&path, new.as_bytes()).map_err(|e| format!("Cannot write {name}: {e}"))?;
                String::new()
            };
            done.push(format!("{name}{said}"));
        }
        if original < self.file_tabs.len() { self.activate_file(original); }
        self.tree.refresh();
        Ok(format!("Renamed {count} occurrences in {} files:\n{}", done.len(), done.join("\n")))
    }
}
