//! Explicit message attachments, project prompts, and commands that draft questions.
use super::*;
use std::collections::BTreeSet;

fn mentions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '@' || i > 0 && !text[..i].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = &text[i + 1..];
        if let Some(rest) = rest.strip_prefix('{') {
            if let Some(end) = rest.find('}') {
                out.push(rest[..end].to_string());
                for _ in 0..=rest[..end].chars().count() {
                    chars.next();
                }
            }
        } else {
            let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}
fn inside(root: &Path, name: &str) -> Result<PathBuf, String> {
    let path = crate::tools::locate(root, name)?;
    let real = path.canonicalize().map_err(|e| format!("Attachment {name}: {e}"))?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(root) {
        return Err("Attachment is outside the project".into());
    }
    Ok(path)
}
impl App {
    pub(super) fn draft_agent_question(&mut self, label: &str, text: &str) {
        if text.trim().is_empty() {
            self.message = Some("No output to attach".into());
            return;
        }
        let text = if text.len() > 20000 { &text[editor::ceil(text, text.len() - 20000)..] } else { text };
        let draft = format!("Help me understand and fix this {label}.\n\n```text\n{text}\n```\n");
        let pane = &mut self.ai.side.input;
        let prefix = if pane.buf.text.is_empty() { "" } else { "\n\n" };
        let at = pane.buf.text.len();
        pane.edit(at, at, &format!("{prefix}{draft}"), EditKind::Other);
        self.show_side(Some(Tab::Agent));
        self.focus = Focus::Chat;
    }
    pub(super) fn ask_terminal(&mut self) {
        let text = self.term().map(|t| t.selected().unwrap_or_else(|| t.recent_text(20000))).unwrap_or_default();
        self.draft_agent_question("terminal output", &text);
    }
    pub(super) fn ask_task(&mut self) {
        let text = self.task_output();
        self.draft_agent_question("task failure", &text);
    }
    pub(super) fn ask_build(&mut self) {
        let text = self
            .typeset
            .view
            .problems
            .iter()
            .map(|p| format!("{}:{}: {}", p.file, p.line.unwrap_or(1), p.text))
            .collect::<Vec<_>>()
            .join("\n");
        self.draft_agent_question("PDF build failure", &text);
    }
    pub(super) fn open_attachments(&mut self) {
        self.index_files(false);
        self.attachments_menu("");
    }
    pub(super) fn attachments_menu(&mut self, typed: &str) {
        let mut paths: BTreeSet<PathBuf> = self.file_index.paths.iter().cloned().collect();
        paths.extend(self.tab_files().into_iter().flatten().map(|(p, _)| p).filter(|p| p.starts_with(&self.root)));
        let mut folders = BTreeSet::new();
        let mut entries = Vec::new();
        for path in &paths {
            let name = crate::tools::short(&self.root, path);
            entries.push(Entry::new(&name, &name, "File"));
            for parent in path
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .take_while(|p| p.starts_with(&self.root) && *p != self.root)
            {
                folders.insert(parent.to_path_buf());
            }
        }
        for path in folders {
            let name = format!("{}/", crate::tools::short(&self.root, &path));
            entries.push(Entry::new(&name, &name, "Folder"));
        }
        for (path, item) in &self.file_index.symbols {
            let name = format!("{}::{}", crate::tools::short(&self.root, path), item.title);
            entries.push(Entry::new(&name, &item.title, &crate::tools::short(&self.root, path)));
        }
        // Unsaved declarations replace the disk index for every open file.
        for (i, tab) in self.file_tabs.iter().enumerate() {
            let p = if i == self.active_file { &self.doc } else { &tab.state.as_ref().unwrap().pane };
            let Some(path) = &p.path else { continue };
            let name = crate::tools::short(&self.root, path);
            entries.retain(|e| !e.key.starts_with(&format!("{name}::")));
            for item in crate::outline::items(&p.buf.text, p.doc) {
                let key = format!("{name}::{}", item.title);
                entries.push(Entry::new(&key, &item.title, &name));
            }
        }
        let mut menu = Menu::new("Attach file, folder or symbol", entries, "");
        menu.set_typed(typed);
        self.menu = Some((MenuKind::Attach, menu));
    }
    pub(super) fn pick_attachment(&mut self, key: &str) {
        let pane = &mut self.ai.side.input;
        let at = pane.buf.cur;
        pane.edit(at, at, &format!("@{{{key}}} "), EditKind::Other);
        self.show_side(Some(Tab::Agent));
        self.focus = Focus::Chat;
    }
    fn attachment_text(&self, path: &Path) -> Result<String, String> {
        inside(&self.root, &path.to_string_lossy())?;
        let p = if self.doc.path.as_deref() == Some(path) {
            Some(&self.doc)
        } else {
            self.file_tabs
                .iter()
                .filter_map(|t| t.state.as_ref())
                .find(|s| s.pane.path.as_deref() == Some(path))
                .map(|s| &s.pane)
        };
        if let Some(pane) = p.filter(|p| !p.read_only) {
            return Ok(pane.buf.text.clone());
        }
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > 64000 {
            return Err(format!("Attachment {} exceeds 64 KB", path.display()));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("Cannot attach {}: {e}", path.display()))?;
        if text.contains('\0') {
            return Err("Cannot attach binary files".into());
        }
        if super::doc_for(Some(path)) == Doc::Cells {
            return Notebook::open(&text, self.pics.clone()).map(|(_, source, _)| source);
        }
        Ok(text)
    }
    pub(super) fn message_attachments(&self, text: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut seen = BTreeSet::new();
        for name in mentions(text) {
            if !seen.insert(name.clone()) {
                continue;
            }
            let (file, symbol) = name.split_once("::").map_or((name.as_str(), None), |(f, s)| (f, Some(s)));
            let path = inside(&self.root, file)?;
            if path.is_dir() {
                let mut files: Vec<_> =
                    super::search::files(&self.root).into_iter().filter(|p| p.starts_with(&path)).collect();
                files.sort();
                out += &format!("\nFolder {file}: {} files. Attaching up to 20 text files.\n", files.len());
                for file in files.iter().take(20) {
                    match self.attachment_text(file) {
                        Ok(text) => {
                            out += &format!(
                                "\nFile {}:\n{}\n",
                                crate::tools::short(&self.root, file),
                                crate::nb::clip(&text.replace(crate::cells::MARK, "# %% "), 4000)
                            )
                        }
                        Err(e) => out += &format!("{e}\n"),
                    }
                }
            } else {
                let text = self.attachment_text(&path)?;
                let text = if let Some(symbol) = symbol {
                    let kind = super::doc_for(Some(&path));
                    let items = crate::outline::items(&text, kind);
                    let i = items
                        .iter()
                        .position(|i| i.title == symbol)
                        .ok_or_else(|| format!("Symbol {symbol} no longer exists in {file}"))?;
                    let start = items[i].at;
                    let end = items.get(i + 1).map_or(text.len(), |i| i.at);
                    crate::nb::clip(&text[start..end], 12000)
                } else {
                    text
                };
                let text = text.replace(crate::cells::MARK, "# %% ");
                out += &format!("\nAttachment {name}:\n{text}\n");
            }
            if out.len() > 64000 {
                return Err("Attachments exceed 64 KB. Attach fewer files or individual symbols.".into());
            }
        }
        Ok(out)
    }
    pub(super) fn open_saved_prompts(&mut self) {
        let dir = self.root.join(".lightdown/prompts");
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.extension().is_none_or(|e| e != "md") {
                continue;
            }
            if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                entries.push(Entry::new(name, &format!("/{name}"), "Project prompt"));
            }
        }
        entries.sort_by(|a, b| a.label.cmp(&b.label));
        let mut menu = Menu::new("Saved prompts", entries, "");
        menu.hint = "Markdown files in .lightdown/prompts".into();
        self.menu = Some((MenuKind::SavedPrompts, menu));
    }
    pub(super) fn pick_saved_prompt(&mut self, key: &str) {
        let pane = &mut self.ai.side.input;
        let at = pane.buf.cur;
        pane.edit(at, at, &format!("/{key} "), EditKind::Other);
        self.show_side(Some(Tab::Agent));
        self.focus = Focus::Chat;
    }
    pub(super) fn expand_prompt(&self, text: &str) -> Result<String, String> {
        let Some(rest) = text.strip_prefix('/') else { return Ok(text.into()) };
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || "_-".contains(c)) {
            return Err("Prompt names use letters, digits, _ or -".into());
        }
        let path = inside(&self.root, &format!(".lightdown/prompts/{name}.md"))?;
        let source = self.attachment_text(&path)?;
        Ok(if source.contains("$ARGUMENTS") {
            source.replace("$ARGUMENTS", args.trim())
        } else if args.trim().is_empty() {
            source
        } else {
            format!("{source}\n\n{}", args.trim())
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mentions_ignore_emails_and_preserve_paths_with_spaces() {
        assert_eq!(mentions("See @{src/a b.rs} @src/c.rs user@example.com"), vec!["src/a b.rs", "src/c.rs"]);
    }
    #[test]
    fn prompts_attachments_and_output_drafts_use_project_contents() {
        let root = std::env::temp_dir().join(format!("ld-agent-context-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown/prompts")).unwrap();
        std::fs::write(root.join(".lightdown/prompts/test.md"), "Test $ARGUMENTS").unwrap();
        std::fs::write(root.join("a.py"), "def original(): pass\n").unwrap();
        let mut app = App::new(Some(root.join("a.py")));
        app.doc.edit(0, app.doc.buf.text.len(), "def changed(): pass\n", EditKind::Other);
        assert_eq!(app.expand_prompt("/test selected code").unwrap(), "Test selected code");
        assert!(app.message_attachments("@{a.py::changed()}").unwrap().contains("def changed"));
        assert!(app.message_attachments("@../outside").is_err());
        std::fs::write(
            root.join("n.ipynb"),
            r#"{"cells":[{"cell_type":"code","source":"def item(): pass\n"}],"nbformat":4,"nbformat_minor":5}"#,
        )
        .unwrap();
        assert!(app.message_attachments("@{n.ipynb::item()}").unwrap().contains("def item"));
        app.ai.side.input.set_text("Keep this question".into(), Doc::Text);
        app.draft_agent_question("task failure", "test failed");
        assert!(app.ai.side.input.buf.text.contains("Keep this question"));
        assert!(app.ai.side.input.buf.text.contains("test failed"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
