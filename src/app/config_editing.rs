//! Editing Lightdown's own config files: completion of keys and values,
//! hover descriptions, marks and Problems entries from the built-in
//! schemas, and scaffolding a missing project config file.
use super::*;
use crate::config_schema::{self, Mark, Place};

/// Marks worked out for a file, by its path and buffer version, so a
/// frame never validates the same text twice.
pub(super) type ConfigChecks = std::cell::RefCell<std::collections::HashMap<PathBuf, (u64, Vec<Mark>)>>;

impl App {
    /// The marks of a pane holding a config file, from the cache if its text
    /// has not changed.
    fn config_marks_of(&self, pane: &crate::pane::Pane) -> Vec<Mark> {
        let Some(path) = pane.path.as_deref() else { return Vec::new() };
        let Some(file) = config_schema::file_for(path) else { return Vec::new() };
        let mut cache = self.config_checks.borrow_mut();
        if let Some((version, marks)) = cache.get(path).filter(|(v, _)| *v == pane.buf.version) {
            let _ = version;
            return marks.clone();
        }
        let marks = config_schema::validate(&pane.buf.text, &file.schema());
        cache.retain(|p, _| p.exists());
        cache.insert(path.to_path_buf(), (pane.buf.version, marks.clone()));
        marks
    }

    /// The document's schema marks, as diagnostics to underline.
    pub(super) fn config_marks(&self) -> Vec<(usize, usize, String)> {
        self.config_marks_of(&self.doc)
            .into_iter()
            .map(|m| (m.start, m.end, if m.warning { format!("Warning: {}", m.message) } else { m.message }))
            .collect()
    }

    /// Underlines the schema marks of a config file, before each frame. No
    /// language server reports on these files, so the marks are all its own.
    pub(super) fn config_refresh(&mut self) {
        if self.doc.path.as_deref().and_then(config_schema::file_for).is_some() {
            self.doc.diagnostics = self.config_marks();
        }
    }

    /// Problems entries for every open config file.
    pub(super) fn config_problems(&self) -> Vec<super::problems::Problem> {
        let mut out = Vec::new();
        for (i, tab) in self.file_tabs.iter().enumerate() {
            let pane = if i == self.active_file { Some(&self.doc) } else { tab.state.as_ref().map(|s| &s.pane) };
            let Some(pane) = pane else { continue };
            for m in self.config_marks_of(pane) {
                let line = pane.buf.line_of(m.start.min(pane.buf.text.len()));
                let (a, _) = pane.buf.line(line);
                let col = pane.buf.text[a..m.start.max(a)].chars().count() + 1;
                let message = if m.warning { format!("Warning: {}", m.message) } else { m.message };
                out.push(super::problems::Problem { path: pane.path.clone(), line: line + 1, col, message, source: "Config" });
            }
        }
        out
    }

    /// The description of the config key at `at`, for hover and Ctrl+K.
    pub(super) fn config_info(&self, at: usize) -> Option<crate::kernel::Info> {
        let schema = config_schema::schema(self.doc.path.as_deref()?)?;
        let (key, s) = config_schema::key_at(&schema, &self.doc.buf.text, at)?;
        Some(crate::kernel::Info { name: key, kind: "Setting".into(), doc: config_schema::describe(&s), ..Default::default() })
    }

    /// Completion in a config file: keys at the cursor's level, or the
    /// values the key allows. Returns whether the document is a config
    /// file, so other completion stays out of it.
    pub(super) fn config_complete(&mut self) -> bool {
        let Some(schema) = self.doc.path.as_deref().and_then(config_schema::schema) else { return false };
        let cur = self.doc.buf.cur;
        let Some(spot) = config_schema::spot(&self.doc.buf.text, cur) else { return true };
        let prefix = self.doc.buf.text[spot.start..cur].to_string();
        let items: Vec<Candidate> = config_schema::complete(&schema, &spot, &prefix)
            .into_iter()
            .take(200)
            .map(|i| Candidate { name: i.label, kind: short(&i.detail, 40) })
            .collect();
        self.comp = (!items.is_empty()).then(|| Completion {
            query: py::Query { start: spot.start, prefix, base: None },
            items,
            sel: 0,
            top: 0,
            seq: 0,
        });
        true
    }

    /// Takes a config completion: a key gets its quotes and colon, a string
    /// value its quotes. Returns whether it was one.
    pub(super) fn config_completion_take(&mut self, start: usize, name: &str) -> bool {
        let Some(schema) = self.doc.path.as_deref().and_then(config_schema::schema) else { return false };
        let text = &self.doc.buf.text;
        let cur = self.doc.buf.cur;
        let Some(spot) = config_schema::spot(text, cur).filter(|s| s.start == start) else { return false };
        let Some(item) = config_schema::complete(&schema, &spot, name).into_iter().find(|i| i.label == name) else {
            return false;
        };
        // A closing quote already there is stepped over, not doubled.
        let mut end = cur;
        let mut insert = item.insert.clone();
        if spot.quoted {
            if text[cur..].starts_with('"') {
                end += 1;
            }
            insert.push('"');
        }
        if spot.on_key && !text[end..].trim_start().starts_with(':') {
            insert.push_str(": ");
        }
        if self.doc.edit(start, end, &insert, EditKind::Other) {
            self.doc.buf.commit();
            let at = start + insert.len();
            self.doc.select(at, at);
            self.doc.moved();
        }
        self.edited();
        true
    }

    /// Lists the project config files not written yet, to scaffold one.
    pub(super) fn config_create_list(&mut self) {
        let dir = self.root.join(".lightdown");
        let entries: Vec<Entry> = config_schema::FILES
            .iter()
            .filter(|f| f.place == Place::Project && !dir.join(f.name).exists())
            .map(|f| Entry::new(&format!("c\nconfig_create:{}", f.name), f.name, f.description()))
            .collect();
        if entries.is_empty() {
            self.message = Some("Every project config file exists already".into());
            return;
        }
        self.menu = Some((MenuKind::Palette, Menu::new("Create project config file", entries, "")));
    }

    /// Writes the chosen file with its example and opens it. An existing
    /// file is never overwritten.
    pub(super) fn config_create(&mut self, name: &str) {
        let Some(file) = config_schema::FILES.iter().find(|f| f.name == name && f.place == Place::Project) else { return };
        let path = self.root.join(".lightdown").join(name);
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
            out.write_all(file.example().as_bytes())
        })();
        match result {
            Ok(()) => {
                self.open_tab(path);
                self.message = Some(format!("Created .lightdown/{name} from its example"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.message = Some(format!(".lightdown/{name} exists already; opening it"));
                self.open_tab(path);
            }
            Err(e) => self.message = Some(format!("Could not create .lightdown/{name}: {e}")),
        }
    }
}

/// A description cut to fit beside a name in the completion list; hover
/// and Ctrl+K show all of it.
fn short(s: &str, most: usize) -> String {
    if s.chars().count() <= most {
        return s.to_string();
    }
    let cut: String = s.chars().take(most).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a).trim_end_matches([',', ':', ';']);
    format!("{cut}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ld-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        root
    }

    fn schema(name: &str) -> serde_json::Value {
        config_schema::FILES.iter().find(|f| f.name == name && f.place == Place::Project).unwrap().schema()
    }

    fn valid(name: &str, text: &str) -> bool {
        !config_schema::validate(text, &schema(name)).iter().any(|m| !m.warning)
    }

    #[test]
    fn completion_inserts_keys_and_values_with_their_quotes() {
        let root = temp("complete");
        let path = root.join(".lightdown/settings.json");
        std::fs::write(&path, "{\"font_").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.doc.select(7, 7);
        assert!(a.config_complete());
        assert_eq!(a.comp.as_ref().unwrap().items[0].name, "font_size");
        a.key(Key::Tab, Mods::default(), None);
        assert_eq!(a.doc.buf.text, "{\"font_size\": ");
        a.doc.set_text("{\"auto_save\": ".into(), Doc::Code);
        a.doc.select(14, 14);
        a.config_complete();
        let names: Vec<_> = a.comp.as_ref().unwrap().items.iter().map(|i| i.name.clone()).collect();
        assert_eq!(names, ["off", "on_focus_loss", "after_delay"]);
        a.key(Key::Down, Mods::default(), None);
        a.key(Key::Enter, Mods::default(), None);
        assert_eq!(a.doc.buf.text, "{\"auto_save\": \"on_focus_loss\"");
        // Marks, Problems and hover.
        a.doc.set_text("{\"ruler\": 81}".into(), Doc::Code);
        assert_eq!(a.config_problems()[0].message, "`ruler` must be one of 0, 80, 100");
        assert_eq!(a.config_marks()[0].0, 10);
        assert!(a.config_info(3).unwrap().doc.contains("ruler"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn create_lists_missing_files_writes_the_example_and_never_overwrites() {
        let root = temp("create");
        std::fs::write(root.join(".lightdown/lsp.json"), "{}").unwrap();
        let mut a = App::new(Some(root.join("a.txt")));
        a.config_create_list();
        let Some((_, menu)) = &a.menu else { panic!() };
        assert!(menu.entries.iter().any(|e| e.label == "grammar.json"));
        assert!(!menu.entries.iter().any(|e| e.label == "lsp.json"));
        a.menu = None;
        a.config_create("grammar.json");
        let text = std::fs::read_to_string(root.join(".lightdown/grammar.json")).unwrap();
        assert!(text.contains("127.0.0.1:8081"));
        assert_eq!(a.doc.path.as_deref(), Some(root.join(".lightdown/grammar.json").as_path()));
        std::fs::write(root.join(".lightdown/grammar.json"), "{\"mine\": 1}").unwrap();
        a.config_create("grammar.json");
        assert_eq!(std::fs::read_to_string(root.join(".lightdown/grammar.json")).unwrap(), "{\"mine\": 1}");
        std::fs::remove_dir_all(root).unwrap();
    }

    // The examples in the README pass both the schema and the loader, and
    // broken files fail both.

    #[test]
    fn lsp_json_agrees_with_its_loader() {
        let root = temp("lsp");
        let file = config_schema::FILES.iter().find(|f| f.name == "lsp.json").unwrap();
        for (text, source, ok) in [
            (file.example(), "a.py", true),
            (file.example(), "a.rust", true),
            ("{\"rs\": true}".to_string(), "a.rs", false),
            ("{\"rs\": []}".to_string(), "a.rs", false),
            ("{\"rs\": [\"x\", 3]}".to_string(), "a.rs", false),
            ("{\"rs\": {\"languageId\": \"rust\"}}".to_string(), "a.rs", false),
            ("{\"rs\": false}".to_string(), "a.rs", true),
            ("{\"rs\": [\"ra\"],}".to_string(), "a.rs", false),
        ] {
            std::fs::write(root.join(".lightdown/lsp.json"), &text).unwrap();
            assert_eq!(crate::lsp::server(&root, &root.join(source)).is_ok(), ok, "loader: {text}");
            assert_eq!(valid("lsp.json", &text), ok, "schema: {text}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commands_json_agrees_with_its_loader() {
        let root = temp("commands");
        let file = config_schema::FILES.iter().find(|f| f.name == "commands.json").unwrap();
        for (text, ok) in [
            (file.example(), true),
            (r#"{"commands": [{"name": "a", "command": "x"}]}"#.to_string(), true),
            (r#"[{"name": "a"}]"#.to_string(), false),
            (r#"[{"name": " ", "command": "x"}]"#.to_string(), false),
            (r#"[{"name": "a", "command": "x", "output": "print"}]"#.to_string(), false),
            (r#"[{"name": "a", "command": "x", "key": "k"}]"#.to_string(), false),
            (r#"[{"name": "a", "command": "x", "key": "ctrl+k"}, {"name": "a", "command": "y"}]"#.to_string(), false),
            (r#"{"name": "a"}"#.to_string(), false),
        ] {
            std::fs::write(root.join(".lightdown/commands.json"), &text).unwrap();
            assert_eq!(super::super::user_commands::loads(&root).is_ok(), ok, "loader: {text}");
            assert_eq!(valid("commands.json", &text), ok, "schema: {text}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_json_agrees_with_its_loader() {
        let root = temp("run");
        let tasks = vec![("make build".to_string(), "make build".to_string())];
        let check = |text: &str| -> bool {
            let v: serde_json::Value = serde_json::from_str(text).unwrap();
            v["configurations"].as_array().is_some_and(|list| {
                list.iter().all(|c| super::super::run_configs::compile(&root, c, &tasks).is_ok())
            })
        };
        let file = config_schema::FILES.iter().find(|f| f.name == "run.json").unwrap();
        for (text, ok) in [
            (file.example(), true),
            (r#"{"configurations": [{"name": "a"}]}"#.to_string(), false),
            (r#"{"configurations": [{"name": "a", "command": "x", "args": "-v"}]}"#.to_string(), false),
            (r#"{"configurations": [{"name": "a", "command": "x", "env": {"A=B": "c"}}]}"#.to_string(), false),
            (r#"{"configurations": [{"name": "a", "command": "x", "env": {"A": 1}}]}"#.to_string(), false),
            (r#"{"configurations": {}}"#.to_string(), false),
        ] {
            assert_eq!(check(&text), ok, "loader: {text}");
            assert_eq!(valid("run.json", &text), ok, "schema: {text}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_settings_agree_with_their_loader() {
        let root = temp("settings");
        let file = config_schema::FILES.iter().find(|f| f.name == "settings.json" && f.place == Place::Project).unwrap();
        for (text, ok) in [
            (file.example(), true),
            (r#"{"auto_save": "after_delay", "ruler": 0}"#.to_string(), true),
            (r#"{"font_size": 41}"#.to_string(), false),
            (r#"{"ruler": 80.0}"#.to_string(), false),
            (r#"{"auto_save": "always"}"#.to_string(), false),
            (r#"{"formatters": {"py": 1}}"#.to_string(), false),
            (r#"[]"#.to_string(), false),
        ] {
            std::fs::write(root.join(".lightdown/settings.json"), &text).unwrap();
            let mut cfg = crate::config::Config::default();
            assert_eq!(cfg.load_project(&root).is_ok(), ok, "loader: {text}");
            assert_eq!(valid("settings.json", &text), ok, "schema: {text}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mcp_grammar_and_snippets_agree_with_their_loaders() {
        let root = temp("mcp");
        let file = config_schema::FILES.iter().find(|f| f.name == "mcp.json").unwrap();
        for (text, ok) in [
            (file.example(), true),
            (r#"{"servers": {}}"#.to_string(), false),
            (format!("{{\"mcpServers\": {{{}}}}}", (0..17).map(|i| format!("\"s{i}\": {{\"command\": \"x\"}}")).collect::<Vec<_>>().join(",")), false),
        ] {
            std::fs::write(root.join(".lightdown/mcp.json"), &text).unwrap();
            let waker: crate::kernel::Waker = std::sync::Arc::new(|| {});
            assert_eq!(crate::mcp::Servers::connect_auto(&root, waker).is_ok(), ok, "loader: {text}");
            assert_eq!(valid("mcp.json", &text), ok, "schema: {text}");
        }
        for url in ["http://127.0.0.1:8081", "http://localhost:80/", "http://[::1]:65535", "https://127.0.0.1:8081", "http://127.0.0.1:0", "http://127.0.0.1:65536", "http://example.com:80", "http://127.0.0.1:80/v2"] {
            let text = serde_json::json!({ "url": url }).to_string();
            assert_eq!(super::super::grammar::endpoint(url.trim_end_matches('/')), valid("grammar.json", &text), "{url}");
        }
        let file = config_schema::FILES.iter().find(|f| f.name == "snippets.json" && f.place == Place::Project).unwrap();
        std::fs::write(root.join(".lightdown/snippets.json"), file.example()).unwrap();
        assert!(crate::snippets::load(&root, "py").iter().any(|s| s.prefix == "defn"));
        let broken = r#"{"py": [{"prefix": 3, "body": "x"}]}"#;
        std::fs::write(root.join(".lightdown/snippets.json"), broken).unwrap();
        assert!(!crate::snippets::load(&root, "py").iter().any(|s| s.body == "x"));
        assert!(!valid("snippets.json", broken));
        std::fs::remove_dir_all(root).unwrap();
    }
}
