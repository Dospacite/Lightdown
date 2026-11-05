//! Resource edits share a checkpoint and update paths without replacing tab identities.
use super::*;
use crate::lsp_workspace::{Node, Plan};
impl App {
    pub(super) fn language_text_apply(&mut self, edit: &Value) -> Result<usize, String> {
        let mut buffers = std::collections::BTreeMap::new();
        for (i, tab) in self.file_tabs.iter().enumerate() {
            let pane = if i == self.active_file { Some(&self.doc) } else { tab.state.as_ref().map(|s| &s.pane) };
            if let Some(pane) = pane {
                if let Some(path) = &pane.path {
                    buffers.insert(path.clone(), (pane.buf.version, pane.buf.text.clone()));
                }
            }
        }
        let plan = Plan::build(&self.root, edit, &buffers)?;
        let mut replacements = Vec::new();
        for (path, node) in &plan.after {
            let Some(Node::File(bytes, _)) = node else { continue };
            if doc_for(Some(path)) == Doc::Cells || pdf::is_pdf(path) || is_picture(path) {
                return Err("Workspace target must be editable source".into());
            }
            let Some((disk, text)) = self.languages.rename_snapshot.get(path) else {
                return Err("Workspace target was not in the request snapshot".into());
            };
            if std::fs::read(path).map_err(|e| e.to_string())? != disk.as_bytes() {
                return Err("Workspace target changed on disk since the request".into());
            }
            let current = buffers.get(path).map(|(_, text)| text.clone()).unwrap_or_else(|| disk.replace("\r\n", "\n"));
            if &current != text {
                return Err("Workspace target buffer changed since the request".into());
            }
            let out = String::from_utf8(bytes.clone()).map_err(|_| "Workspace target is not UTF-8")?;
            replacements.push((path.clone(), current, out));
        }
        let separate = !self.checkpoint_active();
        if separate {
            self.begin_agent_turn();
        }
        for (path, _, _) in &replacements {
            self.checkpoint_file(path)?;
        }
        let original = self.doc.path.clone();
        let count = replacements.len();
        for (path, text, out) in replacements {
            self.open_tab(path.clone());
            if self.doc.path.as_ref() != Some(&path) || self.doc.read_only || self.doc.buf.text != text {
                return Err("Workspace target could not be opened for editing".into());
            }
            self.doc.edit(0, text.len(), &out, EditKind::Other);
            self.doc.buf.commit();
            self.edited();
        }
        if let Some(path) = original {
            self.open_tab(path);
        }
        self.refresh_agent_changes();
        if separate {
            self.finish_agent_turn();
        }
        Ok(count)
    }

    pub(super) fn language_resource_apply(&mut self, edit: &Value) -> Result<usize, String> {
        let mut buffers = std::collections::BTreeMap::new();
        for (i, tab) in self.file_tabs.iter().enumerate() {
            let pane = if i == self.active_file { Some(&self.doc) } else { tab.state.as_ref().map(|s| &s.pane) };
            if let Some(pane) = pane {
                if let Some(path) = &pane.path {
                    if !pane.read_only && pane.doc != Doc::Cells {
                        buffers.insert(path.clone(), (pane.buf.version, pane.buf.text.clone()));
                    }
                }
            }
        }
        let plan = Plan::build(&self.root, edit, &buffers)?;
        for (path, before) in &plan.before {
            if let Some((disk, text)) = self.languages.rename_snapshot.get(path) {
                if let Some(Node::File(bytes, _)) = before {
                    let current =
                        buffers.get(path).map(|(_, text)| text.clone()).unwrap_or_else(|| disk.replace("\r\n", "\n"));
                    if bytes != disk.as_bytes() || &current != text {
                        return Err("Workspace edit target changed since the request".into());
                    }
                }
            }
        }
        let separate = !self.checkpoint_active();
        if separate {
            self.begin_agent_turn();
        }
        for (path, node) in &plan.before {
            if !matches!(node, Some(Node::Directory)) && !matches!(plan.after.get(path), Some(Some(Node::Directory))) {
                self.checkpoint_file(path)?;
            }
        }
        if let Err(e) = plan.apply() {
            if separate {
                self.finish_agent_turn();
            }
            return Err(e);
        }
        for (old, new) in &plan.moves {
            self.language_follow_path(old, new);
        }
        for path in &plan.deleted {
            for i in (0..self.file_tabs.len()).rev() {
                let pane = if i == self.active_file {
                    Some(&self.doc)
                } else {
                    self.file_tabs[i].state.as_ref().map(|s| &s.pane)
                };
                if pane.and_then(|p| p.path.as_ref()).is_some_and(|p| p.starts_with(path)) {
                    self.tab_close_armed = Some(i);
                    self.close_file_tab(i);
                }
            }
        }
        for i in 0..self.file_tabs.len() {
            let pane = if i == self.active_file {
                Some(&mut self.doc)
            } else {
                self.file_tabs[i].state.as_mut().map(|s| &mut s.pane)
            };
            if let Some(pane) = pane {
                if let Some(Some(Node::File(bytes, _))) = pane.path.as_ref().and_then(|p| plan.after.get(p)) {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        if pane.buf.text != text {
                            pane.edit(0, pane.buf.text.len(), text, EditKind::Other);
                            pane.buf.commit();
                        }
                        pane.buf.modified = false;
                    }
                }
            }
        }
        for path in self.languages.opened.keys() {
            for client in self.languages.clients.values() {
                client.notify("textDocument/didClose", json!({"textDocument":{"uri":lsp::uri(path)}}));
            }
        }
        self.languages.opened.clear();
        self.languages.diagnostics.clear();
        self.disk_remember();
        self.tree.refresh();
        self.refresh_agent_changes();
        if separate {
            self.finish_agent_turn();
        }
        Ok(plan.after.iter().filter(|(p, n)| plan.before.get(*p) != Some(*n)).count())
    }
    pub(in crate::app) fn language_follow_path(&mut self, old: &Path, new: &Path) {
        self.move_stored_paths(old, new);
        if let Some(path) = self.doc.path.clone() {
            if let Ok(rest) = path.strip_prefix(old) {
                self.set_path(if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) });
            }
        }
    }
    pub(super) fn language_execute(&mut self, command: &Value) {
        let command = if command["command"].is_string() { command.clone() } else { command["command"].clone() };
        if let Some(name) = command["command"].as_str() {
            if self.language_snapshot() {
                self.language_request(
                    "execute",
                    "workspace/executeCommand",
                    json!({"command":name,"arguments":command["arguments"].as_array().cloned().unwrap_or_default()}),
                );
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_edits_follow_tabs_and_checkpoint_paths() {
        let root = std::env::temp_dir().join(format!("ld-resource-tabs-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let old = root.join("a.rs");
        let new = root.join("b.rs");
        let extra = root.join("extra.rs");
        std::fs::write(&old, "foo\n").unwrap();
        std::fs::write(&extra, "extra\n").unwrap();
        let mut app = App::new(Some(old.clone()));
        app.open_tab(extra.clone());
        app.open_tab(old.clone());
        let tab = app.file_tabs[app.active_file].id;
        let edit = json!({"documentChanges":[{"kind":"rename","oldUri":lsp::uri(&old),"newUri":lsp::uri(&new)},{"textDocument":{"uri":lsp::uri(&new)},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"bar"}]},{"kind":"delete","uri":lsp::uri(&extra)}]});
        app.language_resource_apply(&edit).unwrap();
        assert_eq!(app.doc.path.as_ref(), Some(&new));
        assert_eq!(app.doc.buf.text, "bar\n");
        assert_eq!(app.file_tabs[app.active_file].id, tab);
        assert!(!old.exists());
        assert!(!extra.exists());
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "bar\n");
        app.refresh_agent_changes();
        app.open_agent_turns();
        assert!(app.menu.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stdio_commands_apply_edit_and_explorer_rename_updates_imports() {
        let root = std::env::temp_dir().join(format!("ld-file-events-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let fixture = super::super::tests::SERVER.replace("'textDocumentSync':1", "'workspace':{'fileOperations':{'willRename':{'filters':[{'pattern':{'glob':'**/*.rs'}}]},'didRename':{'filters':[{'pattern':{'glob':'**/*.rs'}}]}}},'textDocumentSync':1").replace(" elif method=='textDocument/rename':", " elif method=='workspace/willRenameFiles':send({'id':m['id'],'result':{'changes':{uri:[{'range':rng,'newText':'use'}]}}})\n elif method=='workspace/didRenameFiles':open('notified','w').write(p['files'][0]['newUri'])\n elif method=='textDocument/codeLens':send({'id':m['id'],'result':[{'range':rng,'command':{'title':'Run fixture','command':'fixture.run'}}]})\n elif method=='workspace/executeCommand':\n  send({'id':'apply-fixture','method':'workspace/applyEdit','params':{'edit':{'documentChanges':[{'textDocument':{'uri':uri},'edits':[{'range':rng,'newText':'run'}]}]}}})\n  send({'id':m['id'],'result':None})\n elif method=='textDocument/rename':");
        std::fs::write(root.join("server.py"), fixture).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"), json!({"rs":["python3",root.join("server.py")]}).to_string())
            .unwrap();
        let old = root.join("a.rs");
        let new = root.join("b.rs");
        std::fs::write(&old, "foo\n").unwrap();
        let mut app = App::new(Some(old.clone()));
        app.language_sync();
        app.language_settle();
        app.language_command("lenses");
        app.language_settle();
        app.language_action_pick("0");
        app.language_settle();
        assert_eq!(app.doc.buf.text, "run\n");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "run\n");
        app.rename("b.rs");
        let started = std::time::Instant::now();
        while (!new.exists() || !root.join("notified").exists()) && started.elapsed().as_secs() < 5 {
            app.language_poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(new.exists(), "{:?}", app.message);
        assert!(!old.exists());
        assert_eq!(app.doc.path.as_ref(), Some(&new));
        assert_eq!(app.doc.buf.text, "use\n");
        assert_eq!(std::fs::read_to_string(root.join("notified")).unwrap(), lsp::uri(&new));
        app.languages = Default::default();
        std::fs::remove_dir_all(root).unwrap();
    }
}
