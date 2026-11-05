//! Settings, kept as JSON in the XDG config directory, and the places where
//! state and caches go.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

mod looks;
mod project;

pub use looks::Looks;

/// Whether the agent may do something that changes things.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Policy {
    Ask,
    Allow,
    Deny,
}

impl Policy {
    pub const NAMES: [&'static str; 3] = ["ask", "allow", "deny"];

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    pub fn parse(s: &str) -> Policy {
        match s {
            "allow" => Policy::Allow,
            "deny" => Policy::Deny,
            _ => Policy::Ask,
        }
    }
}

/// Settings that apply to one project, found by its root directory.
#[derive(Clone)]
pub struct Project {
    pub python: String,
    /// The model and reasoning last chosen here; empty for the defaults.
    pub model: String,
    pub reasoning: String,
    pub edit: Policy,
    pub run: Policy,
    pub web: Policy,
    /// Give the agent the project's AGENTS.md.
    pub instructions: bool,
    /// Dollars the agent may spend here in thirty days; 0 for no limit.
    pub limit_month: f64,
}

impl Default for Project {
    fn default() -> Project {
        Project {
            python: String::new(),
            model: String::new(),
            reasoning: String::new(),
            edit: Policy::Ask,
            run: Policy::Ask,
            web: Policy::Ask,
            instructions: true,
            limit_month: 0.0,
        }
    }
}

#[derive(Clone)]
pub struct Provider {
    pub api: String,
    pub key: String,
    pub model: String,
    pub protocol: String,
    pub images: bool,
    pub cost: [f64; 4],
}

/// The kinds of file with their own word-wrap setting, as settings.json
/// names them.
pub const WRAP_KINDS: [&str; 3] = ["code", "markdown", "text"];

pub struct Config {
    local: BTreeMap<PathBuf, Value>,
    pub providers: BTreeMap<String, Provider>,
    pub editing_provider: String,
    /// Whether the theme shown is light, as worked out from `looks`.
    pub light: bool,
    pub looks: Looks,
    /// Prose is set in the serif Libron rather than the sans-serif Inter.
    pub serif: bool,
    pub font_size: f32,
    pub autocomplete: bool,
    /// Mark misspelt words in prose, and the hunspell dictionary to use;
    /// empty for the session's language.
    pub spell: bool,
    pub spell_dict: String,
    /// Draw markdown's mathematics, typeset by TeX, in place of its source.
    pub math: bool,
    /// The interpreter for notebooks and completion; empty to pick the
    /// project's virtual environment or `python3`.
    pub python: String,
    pub sidebar: bool,
    /// The file browser opens independently on the left.
    pub files_sidebar: bool,
    pub source_control: bool,
    pub files_width: f32,
    pub sidebar_width: f32,
    /// How tall the terminal panel is, in logical pixels.
    pub terminal_height: f32,
    pub api_key: String,
    /// A provider and model, as in `go/deepseek-v4.1-flash`.
    pub model: String,
    pub reasoning: String,
    pub context: bool,
    pub show_reasoning: bool,
    /// Run the project's `check` command after agent turns that change files.
    pub agent_check: bool,
    /// The model that suggests code at the cursor, and how hard it thinks.
    pub suggest_model: String,
    pub suggest_reasoning: String,
    pub auto_suggest: bool,
    pub edit_prediction_provider: String,
    pub edit_prediction_delay_ms: u64,
    /// Dollars the agent may spend in five hours, a week and thirty days.
    pub limits: [f64; 3],
    /// The TeX engine to build with, one of `tex::CHOICES`.
    pub latex: String,
    /// Shortcuts moved off their default keys: the keys each has instead.
    pub keys: BTreeMap<String, Vec<String>>,
    pub base_keymap: String,
    pub projects: BTreeMap<String, Project>,
    pub auto_save: String,
    pub code_actions_on_save: Vec<String>,
    pub format_on_save: bool,
    pub lsp_auto_install: String,
    /// Whether lines wrap, for code, markdown and plain text.
    pub word_wrap: [bool; 3],
    pub modal_editing: String,
    pub minimap: String,
    pub minimap_width: f32,
    pub indent_guides: bool,
    /// Shells started in the terminal mark their prompts and commands.
    pub shell_integration: bool,
    pub ruler: usize,
    pub formatters: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            local: BTreeMap::new(),
            providers: [
                ("openai", "https://api.openai.com/v1", "responses"),
                ("anthropic", "https://api.anthropic.com/v1", "messages"),
                ("openrouter", "https://openrouter.ai/api/v1", "chat"),
                ("ollama", "http://localhost:11434/v1", "chat"),
                ("custom", "http://localhost:8080/v1", "chat"),
            ]
            .into_iter()
            .map(|(name, api, protocol)| {
                (
                    name.into(),
                    Provider {
                        api: api.into(),
                        key: String::new(),
                        model: String::new(),
                        protocol: protocol.into(),
                        images: false,
                        cost: [0.0; 4],
                    },
                )
            })
            .collect(),
            editing_provider: "openai".into(),
            light: false,
            looks: Looks::default(),
            serif: false,
            font_size: crate::theme::BODY,
            autocomplete: true,
            spell: true,
            spell_dict: String::new(),
            math: true,
            python: String::new(),
            sidebar: false,
            files_sidebar: false,
            source_control: false,
            files_width: 260.0,
            sidebar_width: 440.0,
            terminal_height: 280.0,
            api_key: String::new(),
            model: "go/deepseek-v4.1-flash".into(),
            reasoning: "default".into(),
            context: true,
            show_reasoning: true,
            agent_check: true,
            suggest_model: "go/deepseek-v4.1-flash".into(),
            suggest_reasoning: "off".into(),
            auto_suggest: false,
            edit_prediction_provider: "model".into(),
            edit_prediction_delay_ms: 300,
            limits: [0.0; 3],
            latex: "auto".into(),
            keys: BTreeMap::new(),
            base_keymap: "lightdown".into(),
            projects: BTreeMap::new(),
            auto_save: "off".into(),
            code_actions_on_save: Vec::new(),
            format_on_save: false,
            lsp_auto_install: "ask".into(),
            word_wrap: [true; 3],
            modal_editing: "none".into(),
            minimap: "auto".into(),
            minimap_width: 72.0,
            indent_guides: true,
            shell_integration: true,
            ruler: 80,
            formatters: [
                ("py", "ruff format -"),
                ("rs", "rustfmt --emit stdout"),
                ("js", "prettier --parser babel"),
                ("ts", "prettier --parser typescript"),
                ("tsx", "prettier --parser typescript"),
                ("jsx", "prettier --parser babel"),
                ("json", "prettier --parser json"),
                ("go", "gofmt"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        }
    }
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    // Tests keep what they write away from the user's own folders.
    if cfg!(test) {
        return std::env::temp_dir().join(format!("lightdown-xdg-{}", std::process::id())).join(fallback);
    }
    let base = std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback);
    base.unwrap_or_else(home).join("lightdown")
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// Where threads and the usage record are kept.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// The directory a file's project lives in: the nearest one holding a
/// `.git`, or else the file's own.
pub fn project_root(file: Option<&Path>) -> PathBuf {
    let cwd = || std::env::current_dir().unwrap_or_default();
    let dir = file.and_then(Path::parent).filter(|d| !d.as_os_str().is_empty()).map_or_else(cwd, Path::to_path_buf);
    dir.ancestors().find(|d| d.join(".git").exists() || d.join(".lightdown/settings.json").is_file()).unwrap_or(&dir).to_path_buf()
}

/// Writes a file only its owner can read, creating its directory.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    std::fs::rename(&tmp, path)
}

impl Config {
    fn load_sidebars(&mut self, v: &Value) {
        self.source_control = v["source_control"].as_bool().unwrap_or(false);
        let legacy_files = v["sidebar_view"] == "files";
        self.files_sidebar = v["files_sidebar"].as_bool().unwrap_or(self.sidebar && legacy_files);
        self.files_width = v["files_width"]
            .as_f64()
            .map(|n| (n as f32).clamp(160.0, 1200.0))
            .unwrap_or(if legacy_files { self.sidebar_width } else { self.files_width });
        if legacy_files && v["files_sidebar"].is_null() {
            self.sidebar = false;
        }
    }

    pub fn load() -> Config {
        let mut c = Config::default();
        if cfg!(test) {
            return c;
        }
        let Ok(text) = std::fs::read_to_string(config_dir().join("settings.json")) else { return c };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { return c };
        let text = |v: &Value, key: &str, into: &mut String| {
            if let Some(s) = v[key].as_str() {
                *into = s.to_string();
            }
        };
        let flag = |v: &Value, key: &str, into: &mut bool| *into = v[key].as_bool().unwrap_or(*into);
        for (name, p) in v["providers"].as_object().into_iter().flatten() {
            if let Some(provider) = c.providers.get_mut(name) {
                text(p, "api", &mut provider.api);
                text(p, "key", &mut provider.key);
                text(p, "model", &mut provider.model);
                text(p, "protocol", &mut provider.protocol);
                flag(p, "images", &mut provider.images);
                for (cost, n) in provider.cost.iter_mut().zip(p["cost"].as_array().into_iter().flatten()) {
                    *cost = n.as_f64().filter(|n| n.is_finite()).unwrap_or(0.0).max(0.0);
                }
            }
        }
        text(&v, "editing_provider", &mut c.editing_provider);
        if !c.providers.contains_key(&c.editing_provider) {
            c.editing_provider = "openai".into();
        }
        c.light = v["theme"] == "light";
        c.looks.load(&v);
        c.serif = v["reading_font"] == "serif";

        c.font_size = v["font_size"].as_f64().map_or(c.font_size, |n| (n as f32).clamp(8.0, 40.0));
        c.sidebar_width = v["sidebar_width"].as_f64().map_or(c.sidebar_width, |n| (n as f32).clamp(240.0, 1200.0));
        c.terminal_height = v["terminal_height"].as_f64().map_or(c.terminal_height, |n| (n as f32).clamp(80.0, 2000.0));
        if let Some(mode) = v["auto_save"].as_str().filter(|s| ["off", "on_focus_loss", "after_delay"].contains(s)) { c.auto_save = mode.into(); }
        c.code_actions_on_save = v["code_actions_on_save"].as_array().into_iter().flatten().filter_map(Value::as_str).take(32).map(str::to_string).collect();
        if let Some(mode @ ("ask" | "always" | "never")) = v["lsp_auto_install"].as_str() { c.lsp_auto_install = mode.into(); }
        flag(&v, "format_on_save", &mut c.format_on_save);
        for (i, kind) in WRAP_KINDS.iter().enumerate() {
            flag(&v["word_wrap"], kind, &mut c.word_wrap[i]);
        }
        c.modal_editing = modal_editing_setting(&v).into();
        if let Some(mode) = v["minimap"].as_str().filter(|s| ["off", "on", "auto"].contains(s)) { c.minimap = mode.into(); }
        c.minimap_width = v["minimap_width"].as_f64().map_or(72.0, |v| (v as f32).clamp(32.0, 160.0));
        flag(&v, "indent_guides", &mut c.indent_guides);
        flag(&v, "shell_integration", &mut c.shell_integration);
        c.ruler = v["ruler"].as_u64().filter(|n| [0, 80, 100].contains(n)).unwrap_or(80) as usize;
        for (lang, command) in v["formatters"].as_object().into_iter().flatten() {
            if let Some(cmd) = command.as_str() {
                c.formatters.insert(lang.clone(), cmd.into());
            }
        }
        flag(&v, "autocomplete", &mut c.autocomplete);
        flag(&v, "spell", &mut c.spell);
        flag(&v, "draw_math", &mut c.math);
        text(&v, "spell_dictionary", &mut c.spell_dict);
        flag(&v, "sidebar", &mut c.sidebar);
        c.load_sidebars(&v);
        flag(&v, "ide_context", &mut c.context);
        flag(&v, "show_reasoning", &mut c.show_reasoning);
        flag(&v, "agent_check", &mut c.agent_check);
        text(&v, "python", &mut c.python);
        text(&v, "api_key", &mut c.api_key);
        text(&v, "model", &mut c.model);
        text(&v, "reasoning", &mut c.reasoning);
        text(&v, "suggest_model", &mut c.suggest_model);
        text(&v, "suggest_reasoning", &mut c.suggest_reasoning);
        c.load_predictions(&v);
        text(&v, "latex_engine", &mut c.latex);
        for (limit, key) in c.limits.iter_mut().zip(["limit_5h", "limit_week", "limit_month"]) {
            *limit = v[key].as_f64().unwrap_or(0.0).max(0.0);
        }
        if let Some(base) = v["base_keymap"].as_str().filter(|b| crate::keys::contextual::BASES.contains(b)) { c.base_keymap = base.into(); }
        for (id, chords) in v["keys"].as_object().into_iter().flatten() {
            let codes = chords.as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string));
            c.keys.insert(id.clone(), codes.collect());
        }
        for (root, p) in v["projects"].as_object().into_iter().flatten() {
            let mut project = Project::default();
            text(p, "python", &mut project.python);
            text(p, "model", &mut project.model);
            text(p, "reasoning", &mut project.reasoning);
            project.edit = Policy::parse(p["edit"].as_str().unwrap_or(""));
            project.run = Policy::parse(p["run"].as_str().unwrap_or(""));
            project.web = Policy::parse(p["web"].as_str().unwrap_or(""));
            flag(p, "instructions", &mut project.instructions);
            project.limit_month = p["limit_month"].as_f64().unwrap_or(0.0).max(0.0);
            c.projects.insert(root.clone(), project);
        }
        c
    }

    pub fn save(&self) -> std::io::Result<()> {
        if cfg!(test) {
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&self.saved()).unwrap_or_default() + "\n";
        write_private(&config_dir().join("settings.json"), text.as_bytes())
    }

    /// The settings as `save` writes them.
    pub(crate) fn saved(&self) -> Value {
        let mut projects = Map::new();
        for (root, p) in &self.projects {
            projects.insert(
                root.clone(),
                json!({
                    "python": p.python,
                    "model": p.model,
                    "reasoning": p.reasoning,
                    "edit": p.edit.name(),
                    "run": p.run.name(),
                    "web": p.web.name(),
                    "instructions": p.instructions,
                    "limit_month": p.limit_month,
                }),
            );
        }
        let providers: Map<String, Value> = self
            .providers
            .iter()
            .map(|(name, p)| {
                (
                    name.clone(),
                    json!({"api": p.api, "key": p.key, "model": p.model, "protocol": p.protocol, "images":p.images, "cost": p.cost}),
                )
            })
            .collect();
        let mut value = json!({
            "providers": providers,
            "editing_provider": self.editing_provider,
            "reading_font": if self.serif { "serif" } else { "sans" },
            "font_size": self.font_size,
            "autocomplete": self.autocomplete,
            "spell": self.spell,
            "draw_math": self.math,
            "spell_dictionary": self.spell_dict,
            "python": self.python,
            "sidebar": self.sidebar,
            "files_sidebar": self.files_sidebar,
            "source_control": self.source_control,
            "files_width": self.files_width,
            "sidebar_width": self.sidebar_width,
            "terminal_height": self.terminal_height,
            "api_key": self.api_key,
            "model": self.model,
            "reasoning": self.reasoning,
            "ide_context": self.context,
            "show_reasoning": self.show_reasoning,
            "agent_check": self.agent_check,
            "suggest_model": self.suggest_model,
            "suggest_reasoning": self.suggest_reasoning,
            "edit_predictions": if self.auto_suggest { "on" } else { "off" },
            "limit_5h": self.limits[0],
            "limit_week": self.limits[1],
            "limit_month": self.limits[2],
            "latex_engine": self.latex,
            "code_actions_on_save": self.code_actions_on_save, "auto_save": self.auto_save, "format_on_save": self.format_on_save, "word_wrap": WRAP_KINDS.iter().zip(self.word_wrap).map(|(k, on)| (k.to_string(), json!(on))).collect::<serde_json::Map<_, _>>(),"indent_guides":self.indent_guides,"ruler":self.ruler,
            "shell_integration": self.shell_integration,
            "formatters": self.formatters,
            "keys": self.keys,
            "base_keymap": self.base_keymap,
            "projects": projects,
        });
        value["modal_editing"] = json!(self.modal_editing);
        value["lsp_auto_install"] = json!(self.lsp_auto_install);
        value["minimap"] = json!(self.minimap);
        value["minimap_width"] = json!(self.minimap_width);
        value["edit_prediction_provider"] = json!(self.edit_prediction_provider);
        value["edit_prediction_delay_ms"] = json!(self.edit_prediction_delay_ms);
        self.looks.save(&mut value);
        value
    }

    /// The API key from the settings, or else from the environment.
    pub fn key(&self) -> Option<String> {
        let env = || std::env::var("OPENCODE_API_KEY").ok().filter(|_| !cfg!(test));
        Some(self.api_key.trim().to_string()).filter(|k| !k.is_empty()).or_else(env).filter(|k| !k.is_empty())
    }

    pub fn key_for(&self, model: &str) -> Option<String> {
        let provider = model.split_once('/').map(|p| p.0).unwrap_or("");
        let Some(p) = self.providers.get(provider) else { return self.key() };
        let env = format!("{}_API_KEY", provider.to_uppercase());
        let key = if p.key.trim().is_empty() {
            std::env::var(env).ok().filter(|_| !cfg!(test)).unwrap_or_default()
        } else {
            p.key.trim().into()
        };
        if key.is_empty() && ["ollama", "custom"].contains(&provider) {
            Some(String::new())
        } else {
            (!key.is_empty() && !key.contains(['\r', '\n'])).then_some(key)
        }
    }

    pub fn agent_model(&self, catalog: &crate::agent::Catalog, key: &str) -> crate::agent::Model {
        let mut model = catalog.get(key);
        if let Some((provider, id)) = key.split_once('/')
            && let Some(p) = self.providers.get(provider)
        {
            model.id = id.into();
            model.name = format!("{provider}/{id}");
            model.api = p.api.trim_end_matches('/').into();
            model.protocol = match p.protocol.as_str() {
                "messages" => crate::agent::Protocol::Messages,
                "responses" => crate::agent::Protocol::Responses,
                _ => crate::agent::Protocol::Chat,
            };
            model.interleaved = false;
            model.images = p.images;
            model.efforts.clear();
            model.toggle = false;
            model.budget = false;
            model.cost = p.cost;
        }
        model
    }

    pub fn project(&self, root: &Path) -> Project {
        self.projects.get(root.to_string_lossy().as_ref()).cloned().unwrap_or_default()
    }

    pub fn project_mut(&mut self, root: &Path) -> &mut Project {
        self.projects.entry(root.to_string_lossy().into_owned()).or_default()
    }

    /// The model and reasoning to use in a project.
    pub fn model_for(&self, root: &Path) -> (String, String) {
        let p = self.project(root);
        let pick = |own: String, default: &str| if own.is_empty() { default.to_string() } else { own };
        (pick(p.model, self.default_model_for(root)), pick(p.reasoning, &self.reasoning))
    }

    /// The interpreter to run for a project: the one set, the project's own
    /// virtual environment, or whatever `python3` is.
    pub fn python_for(&self, root: &Path) -> String {
        let project = self.project(root);
        if !project.python.is_empty() {
            return project.python;
        }
        if !self.python.trim().is_empty() {
            return self.python.trim().to_string();
        }
        let venv = [".venv", "venv"].iter().map(|d| root.join(d).join("bin/python")).find(|p| p.is_file());
        venv.map_or("python3".into(), |p| p.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_preferences_support_both_sides_and_migrate_the_shared_sidebar() {
        let mut cfg = Config::default();
        cfg.sidebar = true;
        cfg.sidebar_width = 360.0;
        cfg.load_sidebars(&json!({"sidebar_view": "files"}));
        assert!(cfg.files_sidebar && !cfg.sidebar);
        assert_eq!(cfg.files_width, 360.0);
        cfg.sidebar = true;
        cfg.load_sidebars(&json!({"files_sidebar": true, "files_width": 300}));
        assert!(cfg.files_sidebar && cfg.sidebar);
        assert_eq!(cfg.files_width, 300.0);
        assert_eq!(cfg.sidebar_width, 360.0);
        cfg.load_sidebars(&json!({"files_sidebar": false, "files_width": 1}));
        assert!(!cfg.files_sidebar && cfg.sidebar);
        assert_eq!(cfg.files_width, 160.0);
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    #[test]
    fn provider_models_route_without_sharing_opencode_credentials() {
        let mut cfg = Config { api_key: "opencode-secret".into(), ..Config::default() };
        let catalog = crate::agent::Catalog::load();
        for (name, protocol) in [
            ("openai", crate::agent::Protocol::Responses),
            ("anthropic", crate::agent::Protocol::Messages),
            ("openrouter", crate::agent::Protocol::Chat),
            ("ollama", crate::agent::Protocol::Chat),
        ] {
            let key = format!("{name}/fixture-model");
            let model = cfg.agent_model(&catalog, &key);
            assert_eq!(model.protocol, protocol);
            assert_eq!(model.id, "fixture-model");
            assert_eq!(cfg.key_for(&key).is_some(), name == "ollama");
            cfg.providers.get_mut(name).unwrap().key = "provider-secret".into();
            assert_eq!(cfg.key_for(&key).as_deref(), Some("provider-secret"));
            let req = crate::agent::request(
                &model,
                "provider-secret",
                &crate::agent::Thread::new(),
                "system",
                &[],
                "default",
            );
            assert!(req.url.starts_with(&cfg.providers[name].api));
            assert!(!req.headers.iter().any(|h| h.contains("opencode-secret") || h.starts_with("x-opencode-session:")));
            if name == "anthropic" {
                assert!(req.headers.contains(&"x-api-key: provider-secret".into()));
                assert!(!req.headers.iter().any(|h| h.starts_with("Authorization:")));
            }
        }
        assert_eq!(cfg.key_for("go/model").as_deref(), Some("opencode-secret"));
    }
    #[test]
    fn interpreter_choice_is_per_project() {
        let mut cfg = Config { python: "global-python".into(), ..Config::default() };
        cfg.project_mut(Path::new("/one")).python = "/one/.venv/bin/python".into();
        assert_eq!(cfg.python_for(Path::new("/one")), "/one/.venv/bin/python");
        assert_eq!(cfg.python_for(Path::new("/two")), "global-python");
    }
}

/// The explicit modal setting takes precedence over the legacy switch.
fn modal_editing_setting(value: &serde_json::Value) -> &str {
    value["modal_editing"].as_str().filter(|s| ["none", "vim", "helix"].contains(s))
        .unwrap_or(if value["vim_mode"] == true { "vim" } else { "none" })
}

#[cfg(test)]
mod modal_settings_tests {
    #[test]
    fn migrates_legacy_switch_and_prefers_explicit_mode() {
        use serde_json::json;
        use super::modal_editing_setting as mode;
        assert_eq!(mode(&json!({})), "none");
        assert_eq!(mode(&json!({"vim_mode":true})), "vim");
        assert_eq!(mode(&json!({"vim_mode":false})), "none");
        assert_eq!(mode(&json!({"vim_mode":true,"modal_editing":"helix"})), "helix");
        assert_eq!(mode(&json!({"vim_mode":true,"modal_editing":"none"})), "none");
    }
}
