//! A bounded, explicit set of project overrides. Never serialized globally.
use super::*;
impl Config {
    pub fn load_project(&mut self, root: &Path) -> Result<(), String> {
        let path = root.join(".lightdown/settings.json");
        let read = || -> Result<Value, String> {
            if !path.exists() { return Ok(json!({})); }
            if path.metadata().map_err(|e| e.to_string())?.len() > 65536 {
                return Err("project settings exceed 64 KB".into());
            }
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("project settings: {e}"))?;
            let Some(map) = v.as_object() else { return Err("project settings must be an object".into()) };
            for (key, value) in map {
                let valid = match key.as_str() {
                    "font_size" => value.as_f64().is_some_and(|n| (8.0..=40.0).contains(&n)),
                    "ruler" => value.as_u64().is_some_and(|n| [0, 80, 100].contains(&n)),
                    "auto_save" => value.as_str().is_some_and(|s| ["off", "on_focus_loss", "after_delay"].contains(&s)),
                    "code_actions_on_save" => value.as_array().is_some_and(|a| a.len() <= 32 && a.iter().all(Value::is_string)),
                    "format_on_save" => value.is_boolean(),
                    "formatters" => value.as_object().is_some_and(|v| v.values().all(Value::is_string)),
                    "spell_dictionary" | "latex_engine" | "model" => value.is_string(),
                    "check" => value.as_str().is_some_and(|s| !s.trim().is_empty())
                        || value.as_array().is_some_and(|a| !a.is_empty() && a.len() <= 64 && a.iter().all(Value::is_string)),
                    "check_attempts" => value.as_u64().is_some_and(|n| n <= 10),
                    _ => true, // Other agents and future versions may add keys.
                };
                if !valid { return Err(format!("Invalid project setting: {key}")); }
            }
            Ok(v)
        };
        let value = read()?;
        self.local.insert(root.to_path_buf(), value);
        Ok(())
    }
    pub fn ensure_project(&mut self, root: &Path) -> Result<(), String> {
        if self.local.contains_key(root) { Ok(()) } else { self.load_project(root) }
    }
    fn local(&self, root: &Path) -> &Value {
        self.local.get(root).unwrap_or(&Value::Null)
    }
    pub fn font_size_for(&self, root: &Path) -> f32 {
        self.local(root)["font_size"].as_f64().map_or(self.font_size, |n| n as f32)
    }
    pub fn ruler_for(&self, root: &Path) -> usize {
        self.local(root)["ruler"].as_u64().map_or(self.ruler, |n| n as usize)
    }
    pub fn auto_save_for(&self, root: &Path) -> &str {
        self.local(root)["auto_save"].as_str().unwrap_or(&self.auto_save)
    }

    pub fn code_actions_on_save_for(&self, root: &Path) -> Vec<String> {
        self.local(root)["code_actions_on_save"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_else(|| self.code_actions_on_save.clone())
    }
    pub fn format_on_save_for(&self, root: &Path) -> bool {
        self.local(root)["format_on_save"].as_bool().unwrap_or(self.format_on_save)
    }
    pub fn formatter_for(&self, root: &Path, lang: &str) -> Option<&str> {
        self.local(root)["formatters"][lang].as_str().or_else(|| self.formatters.get(lang).map(String::as_str))
    }
    pub fn dictionary_for(&self, root: &Path) -> &str {
        self.local(root)["spell_dictionary"].as_str().unwrap_or(&self.spell_dict)
    }
    pub fn latex_for(&self, root: &Path) -> &str {
        self.local(root)["latex_engine"].as_str().unwrap_or(&self.latex)
    }
    /// The project's check command as a shell command line; an argv array is quoted.
    pub fn check_for(&self, root: &Path) -> Option<String> {
        let check = &self.local(root)["check"];
        if let Some(s) = check.as_str() { return Some(s.to_string()); }
        let argv = check.as_array()?;
        Some(argv.iter().filter_map(Value::as_str).map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" "))
    }
    /// How many times a failing check goes back to the model in one turn.
    pub fn check_attempts_for(&self, root: &Path) -> u32 {
        self.local(root)["check_attempts"].as_u64().map_or(2, |n| n as u32)
    }
    pub(super) fn default_model_for(&self, root: &Path) -> &str {
        self.local(root)["model"].as_str().filter(|s| !s.is_empty()).unwrap_or(&self.model)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_values_fall_back_without_mutating_global_preferences() {
        let root = std::env::temp_dir().join(format!("ld-local-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join(".lightdown/settings.json"), r#"{
            "font_size": 22, "ruler": 100, "formatters": {"rs": "cat", "py": ""},
            "format_on_save": true, "spell_dictionary": "en_GB", "latex_engine": "xelatex", "model": "custom/local"
        }"#).unwrap();
        let mut c = Config::default();
        c.load_project(&root).unwrap();
        assert_eq!(c.font_size_for(&root), 22.0);
        assert_eq!(c.ruler_for(&root), 100);
        assert_eq!(c.formatter_for(&root, "rs"), Some("cat"));
        assert_eq!(c.formatter_for(&root, "py"), Some(""));
        assert!(c.formatter_for(&root, "go").is_some());
        assert!(c.format_on_save_for(&root));
        assert_eq!(c.dictionary_for(&root), "en_GB");
        assert_eq!(c.latex_for(&root), "xelatex");
        assert_eq!(c.model_for(&root).0, "custom/local");
        assert_eq!(c.font_size, theme_size());
        assert_eq!(c.latex, "auto");
        assert_eq!(c.ruler_for(Path::new("elsewhere")), 80);
        std::fs::write(root.join(".lightdown/settings.json"), "{\"font_size\": 999}").unwrap();
        assert!(c.load_project(&root).is_err());
        assert_eq!(c.font_size_for(&root), 22.0);
        std::fs::remove_file(root.join(".lightdown/settings.json")).unwrap();
        c.load_project(&root).unwrap();
        assert_eq!(c.font_size_for(&root), c.font_size);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn check_commands_accept_strings_and_argv() {
        let root = std::env::temp_dir().join(format!("ld-check-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let mut c = Config::default();
        let load = |c: &mut Config, text: &str| { std::fs::write(root.join(".lightdown/settings.json"), text).unwrap(); c.load_project(&root) };
        load(&mut c, "{}").unwrap();
        assert_eq!((c.check_for(&root), c.check_attempts_for(&root)), (None, 2));
        load(&mut c, r#"{"check": "cargo check && cargo test", "check_attempts": 3}"#).unwrap();
        assert_eq!((c.check_for(&root).as_deref(), c.check_attempts_for(&root)), (Some("cargo check && cargo test"), 3));
        load(&mut c, r#"{"check": ["cargo", "clippy", "it's"]}"#).unwrap();
        assert_eq!(c.check_for(&root).as_deref(), Some("'cargo' 'clippy' 'it'\\''s'"));
        for bad in [r#"{"check": ""}"#, r#"{"check": []}"#, r#"{"check": [1]}"#, r#"{"check_attempts": 11}"#] { assert!(load(&mut c, bad).is_err(), "{bad}"); }
        std::fs::remove_dir_all(root).unwrap();
    }
    fn theme_size() -> f32 { crate::theme::BODY }
}
