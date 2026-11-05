//! The settings: a dialog of rows in groups, each a choice to switch or
//! cycle, a value to type, or a list to pick from, with what each does
//! written under it. Typing narrows the rows.

use super::assist::MenuKind;
use super::{App, Key, Mods};
use crate::config::Policy;
use crate::font;
use crate::gfx::Canvas;
use crate::icons::{self, Icon};
use crate::ui::{Rect, px};
use crate::{agent, editor, tex, theme, ui};

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Provider,
    ProviderUrl,
    ProviderKey,
    ProviderModel,
    ProviderProtocol,
    ProviderImages,
    ProviderPrice,
    Theme,
    Prose,
    FontSize,
    Vim,
    AutoSave,
    Minimap,
    MinimapWidth,
    Autocomplete,
    Spell,
    SpellDict,
    Math,
    ShellIntegration,
    Python,
    Keys,
    BaseKeymap,
    Engine,
    ApiKey,
    Model,
    Reasoning,
    Context,
    ShowReasoning,
    AgentCheck,
    SuggestModel,
    SuggestReasoning,
    AutoSuggest,
    PredictionProvider,
    PredictionDelay,
    Limit(usize),
    Edit,
    Run,
    Web,
    Instructions,
    ProjectLimit,
}

enum Kind {
    Choice(&'static [&'static str]),
    Text,
    /// Opens a list to pick from.
    Pick,
}

/// The rows, top to bottom: a field with its name and what it does, or
/// without a field, the name of a group.
const ROWS: &[(Option<Field>, &str, &str)] = &[
    (None, "Appearance", ""),
    (Some(Field::Theme), "Theme", "Dark, light, or as the desktop prefers; Theme: Select theme lists them all"),
    (Some(Field::Prose), "Reading font", "Markdown and plain text in Inter (sans) or Libron (serif)"),
    (Some(Field::FontSize), "Font size", "Of documents, in pixels; Ctrl+Plus and Ctrl+Minus zoom one"),
    (None, "Editor", ""),
    (Some(Field::Vim), "Modal editing", "Choose none, Vim or Helix keys in editor panes"),
    (Some(Field::Minimap), "Minimap", "Source overview: auto shows it when the file is longer than the view"),
    (Some(Field::MinimapWidth), "Minimap width", "Overview width in pixels, from 32 to 160"),
    (Some(Field::AutoSave), "Auto-save", "Save the active file on focus loss or one second after editing"),
    (Some(Field::Autocomplete), "Autocomplete", "Offer names as you type in Python and LaTeX"),
    (Some(Field::Spell), "Spell checking", "Underline misspelt words in prose"),
    (
        Some(Field::SpellDict),
        "Spelling dictionary",
        "A hunspell dictionary, such as en_GB; empty for the session's language",
    ),
    (Some(Field::Math), "Draw mathematics", "Typeset $\u{2026}$ in markdown with TeX"),
    (Some(Field::BaseKeymap), "Base keymap", "Lightdown, VS Code, JetBrains, Sublime or Emacs shortcuts"),
    (Some(Field::Keys), "Keybinds", "Every shortcut, and the keys it is on"),
    (None, "Languages", ""),
    (Some(Field::Python), "Python interpreter", "For notebooks and completion; empty picks .venv or python3"),
    (Some(Field::Engine), "TeX engine", "Builds PDFs of markdown and LaTeX"),
    (
        Some(Field::ShellIntegration),
        "Shell integration",
        "bash, zsh and fish in the terminal mark their commands and exit statuses",
    ),
    (None, "Agent", ""),
    (Some(Field::ApiKey), "OpenCode API key", "For the agent and suggestions, from opencode.ai"),
    (Some(Field::Provider), "Configure provider", "OpenAI, Anthropic, OpenRouter, Ollama or a custom server"),
    (Some(Field::ProviderUrl), "Provider base URL", "API root including /v1 where required"),
    (Some(Field::ProviderKey), "Provider API key", "Stored privately; empty uses PROVIDER_API_KEY"),
    (Some(Field::ProviderModel), "Provider model ID", "Adds this model to the model picker and selects it here"),
    (Some(Field::ProviderProtocol), "Provider request format", "chat, responses or messages"),
    (Some(Field::ProviderImages), "Provider accepts screenshots", "Enable only for a model that accepts image input"),
    (
        Some(Field::ProviderPrice),
        "Provider token prices",
        "Dollars per million: input, output, cache read, cache write",
    ),
    (Some(Field::Model), "Default model", "For projects that have not chosen one"),
    (Some(Field::Reasoning), "Default reasoning", "How hard the model thinks"),
    (Some(Field::Context), "Send IDE context", "The open file, cursor and selection go with each message"),
    (Some(Field::ShowReasoning), "Show reasoning", "Show what the model thought, above its answer"),
    (Some(Field::AgentCheck), "Project check", "After agent edits, run the check command in .lightdown/settings.json"),
    (Some(Field::SuggestModel), "Suggestion model", "Completes text at the cursor on Alt+\\"),
    (Some(Field::AutoSuggest), "Edit predictions", "Predict edits after a pause; Tab jumps to a nearby edit, then accepts"),
    (Some(Field::PredictionProvider), "Edit prediction provider", "Use the suggestion model or GitHub Copilot"),
    (Some(Field::PredictionDelay), "Edit prediction delay", "Pause before requesting, in milliseconds (100 to 5000)"),
    (Some(Field::SuggestReasoning), "Suggestion reasoning", "Off keeps suggestions quick"),
    (None, "Spending limits", ""),
    (Some(Field::Limit(0)), "Every 5 hours", "The agent stops at this much; empty for no limit"),
    (Some(Field::Limit(1)), "Every week", "The agent stops at this much; empty for no limit"),
    (Some(Field::Limit(2)), "Every 30 days", "The agent stops at this much; empty for no limit"),
    (None, "This project", ""),
    (Some(Field::Edit), "Agent may edit files", "Ask each time, allow, or deny"),
    (Some(Field::Web), "Agent may access the web", "Ask each time, allow, or deny fetch and search"),
    (Some(Field::Run), "Agent may run commands", "Ask each time, allow, or deny"),
    (Some(Field::Instructions), "Give the agent AGENTS.md", "The project's instructions for agents"),
    (Some(Field::ProjectLimit), "Spending limit, 30 days", "For this project alone; empty for no limit"),
];

const ON_OFF: &[&str] = &["on", "off"];
const EFFORTS: &[&str] = &["default", "off", "low", "medium", "high", "xhigh", "max"];

pub(super) struct Settings {
    /// The row chosen, by its place in `ROWS`.
    sel: usize,
    /// The value being typed and the cursor in it.
    typing: Option<(String, usize)>,
    /// What narrows the rows.
    filter: String,
    panel: Rect,
    /// Where each row shown was last drawn: its place in `ROWS`, top and
    /// height.
    places: Vec<(usize, f32, f32)>,
    /// The top and bottom of the scrolling part.
    viewport: (f32, f32),
    scroll: f32,
    tall: f32,
    reveal: bool,
    hover: Option<(f32, f32)>,
    /// Where the close button was last drawn.
    close: Rect,
}

impl Settings {
    pub(super) fn input_text(&mut self) -> (u64,&mut String,usize) {
        if let Some((t,c)) = &mut self.typing { (51,t,*c) } else { let c = self.filter.len(); (50,&mut self.filter,c) }
    }
    pub(super) fn input_changed(&mut self, cur: usize) {
        if let Some((_,c)) = &mut self.typing { *c = cur; } else { self.settle(); }
    }

    pub fn new() -> Settings {
        Settings {
            sel: 1,
            typing: None,
            filter: String::new(),
            panel: (0.0, 0.0, 0.0, 0.0),
            places: Vec::new(),
            viewport: (0.0, 0.0),
            scroll: 0.0,
            tall: 0.0,
            reveal: true,
            hover: None,
            close: (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// The rows the filter leaves, by their place in `ROWS`, with the
    /// name of each group that keeps a row.
    fn shown(&self) -> Vec<usize> {
        let q = self.filter.trim().to_lowercase();
        let fits = |i: usize| {
            let (_, label, about) = ROWS[i];
            q.is_empty() || format!("{label} {about}").to_lowercase().contains(&q)
        };
        let mut out = Vec::new();
        let mut group = None;
        for (i, row) in ROWS.iter().enumerate() {
            if row.0.is_none() {
                group = Some(i);
                if q.is_empty() {
                    out.push(i);
                }
            } else if fits(i) {
                if let Some(g) = group.take().filter(|_| !q.is_empty()) {
                    out.push(g);
                }
                out.push(i);
            }
        }
        out
    }

    /// The next row with a field, up or down from the chosen one, among
    /// those shown.
    fn step(&self, down: bool) -> usize {
        let fields: Vec<usize> = self.shown().into_iter().filter(|&i| ROWS[i].0.is_some()).collect();
        if fields.is_empty() {
            return self.sel;
        }
        let at = fields.iter().position(|&i| i == self.sel);
        let n = fields.len();
        let next = match at {
            Some(a) if down => (a + 1) % n,
            Some(a) => (a + n - 1) % n,
            None => 0,
        };
        fields[next]
    }

    /// Keeps the chosen row among those shown.
    fn settle(&mut self) {
        let fields: Vec<usize> = self.shown().into_iter().filter(|&i| ROWS[i].0.is_some()).collect();
        if !fields.contains(&self.sel)
            && let Some(&first) = fields.first()
        {
            self.sel = first;
        }
        self.reveal = true;
    }

    /// The row under a point, by its place in `ROWS`.
    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let (px, _, pw, _) = self.panel;
        if x < px || x >= px + pw || y < self.viewport.0 || y >= self.viewport.1 {
            return None;
        }
        self.places.iter().find(|(_, top, h)| y >= *top && y < top + h).map(|(i, ..)| *i)
    }

    /// Where a row was last drawn, for tests.
    #[cfg(test)]
    fn place(&self, row: usize) -> Option<(f32, f32)> {
        self.places.iter().find(|(i, ..)| *i == row).map(|(_, top, h)| (*top, *h))
    }
}

fn kind(field: Field) -> Kind {
    match field {
        Field::Provider => Kind::Choice(&["openai", "anthropic", "openrouter", "ollama", "custom"]),
        Field::ProviderProtocol => Kind::Choice(&["chat", "responses", "messages"]),
        Field::ProviderUrl | Field::ProviderKey | Field::ProviderModel | Field::ProviderPrice => Kind::Text,
        Field::Vim => Kind::Choice(&["none", "vim", "helix"]),
        Field::Minimap => Kind::Choice(&["auto", "on", "off"]),
        Field::AutoSave => Kind::Choice(&["off", "on_focus_loss", "after_delay"]),
        Field::Theme => Kind::Choice(&["dark", "light", "system"]),
        Field::Prose => Kind::Choice(&["sans", "serif"]),
        Field::ProviderImages

        | Field::Autocomplete
        | Field::Spell
        | Field::Math
        | Field::ShellIntegration
        | Field::Context
        | Field::ShowReasoning
        | Field::AgentCheck
        | Field::Instructions => Kind::Choice(ON_OFF),
        Field::BaseKeymap => Kind::Choice(crate::keys::contextual::BASES),
        Field::Reasoning | Field::SuggestReasoning => Kind::Choice(EFFORTS),
        Field::AutoSuggest => Kind::Choice(ON_OFF),
        Field::PredictionProvider => Kind::Choice(&["model", "copilot"]),
        Field::PredictionDelay => Kind::Text,
        Field::Edit | Field::Run | Field::Web => Kind::Choice(&Policy::NAMES),
        Field::Model | Field::SuggestModel | Field::Keys | Field::Engine => Kind::Pick,
        Field::MinimapWidth | Field::FontSize | Field::Python | Field::SpellDict | Field::ApiKey | Field::Limit(_) | Field::ProjectLimit => {
            Kind::Text
        }
    }
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

fn dollars(limit: f64) -> String {
    if limit > 0.0 { format!("${limit:.2}") } else { "none".to_string() }
}

fn parse_dollars(s: &str) -> f64 {
    s.trim().trim_start_matches('$').parse::<f64>().unwrap_or(0.0).max(0.0)
}

impl App {
    /// A setting's value as its row shows it.
    fn setting(&self, field: Field) -> String {
        let (cfg, project) = (&self.cfg, self.cfg.project(&self.root));
        match field {
            Field::ProviderPrice => {
                cfg.providers[&cfg.editing_provider].cost.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
            }
            Field::Provider => cfg.editing_provider.clone(),
            Field::ProviderUrl => cfg.providers[&cfg.editing_provider].api.clone(),
            Field::ProviderKey => {
                if cfg.providers[&cfg.editing_provider].key.is_empty() {
                    "from environment or not set".into()
                } else {
                    "••••••••".into()
                }
            }
            Field::ProviderModel => cfg.providers[&cfg.editing_provider].model.clone(),
            Field::ProviderProtocol => cfg.providers[&cfg.editing_provider].protocol.clone(),
            Field::ProviderImages => on_off(cfg.providers[&cfg.editing_provider].images),
            Field::Theme if cfg.looks.theme_mode == "system" => "system".to_string(),
            Field::Theme => if cfg.light { "light" } else { "dark" }.to_string(),
            Field::Prose => if cfg.serif { "serif" } else { "sans" }.to_string(),
            Field::FontSize => format!("{}", cfg.font_size),
            Field::Vim => cfg.modal_editing.clone(),
            Field::Minimap => cfg.minimap.clone(),
            Field::MinimapWidth => format!("{}", cfg.minimap_width),
            Field::AutoSave => cfg.auto_save.clone(),
            Field::Autocomplete => on_off(cfg.autocomplete),
            Field::Spell => on_off(cfg.spell),
            Field::Math => on_off(cfg.math),
            Field::ShellIntegration => on_off(cfg.shell_integration),
            Field::SpellDict => {
                let dict = crate::spell::dictionary(&cfg.spell_dict);
                let missing = self.speller.as_ref().is_some_and(|s| s.dict == dict && s.failed());
                let auto = if cfg.spell_dict.trim().is_empty() { "auto: " } else { "" };
                format!("{auto}{dict}{}", if missing { " (not found)" } else { "" })
            }
            Field::Python if cfg.python.is_empty() => format!("auto ({})", cfg.python_for(&self.root)),
            Field::Python => cfg.python.clone(),
            Field::BaseKeymap => cfg.base_keymap.clone(),
            Field::Keys => {
                let count = cfg.keys.len() + self.key_bindings.user.len();
                if count == 0 { format!("{} defaults", cfg.base_keymap) } else { format!("{count} overrides") }
            }
            Field::ApiKey => match (cfg.api_key.trim(), cfg.key()) {
                ("", Some(_)) => "from OPENCODE_API_KEY".to_string(),
                ("", None) => "not set".to_string(),
                (key, _) => {
                    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                    format!("\u{2022}\u{2022}\u{2022}\u{2022}{tail}")
                }
            },
            Field::Engine => match (cfg.latex.as_str(), self.typeset.busy()) {
                (_, true) if tex::find(&cfg.latex).is_none() => "setting up\u{2026}".to_string(),
                ("auto", _) => format!("auto ({})", tex::describe("auto")),
                (name, _) => format!("{name} ({})", tex::describe(name)),
            },
            Field::Model => self.ai.catalog.get(&cfg.model).name,
            Field::Reasoning => cfg.reasoning.clone(),
            Field::Context => on_off(cfg.context),
            Field::ShowReasoning => on_off(cfg.show_reasoning),
            Field::AgentCheck => on_off(cfg.agent_check),
            Field::SuggestModel => self.ai.catalog.get(&cfg.suggest_model).name,
            Field::SuggestReasoning => cfg.suggest_reasoning.clone(),
            Field::AutoSuggest => on_off(cfg.auto_suggest),
            Field::PredictionProvider => cfg.edit_prediction_provider.clone(),
            Field::PredictionDelay => cfg.edit_prediction_delay_ms.to_string(),
            Field::Limit(i) => dollars(cfg.limits[i]),
            Field::Edit => project.edit.name().to_string(),
            Field::Run => project.run.name().to_string(),
            Field::Web => project.web.name().to_string(),
            Field::Instructions => on_off(project.instructions),
            Field::ProjectLimit => dollars(project.limit_month),
        }
    }

    /// What typing starts from: the raw value, never the key.
    fn setting_draft(&self, field: Field) -> String {
        match field {
            Field::Python => self.cfg.python.clone(),
            Field::SpellDict => self.cfg.spell_dict.clone(),
            Field::ApiKey | Field::ProviderKey => String::new(),
            Field::Limit(i) if self.cfg.limits[i] > 0.0 => self.cfg.limits[i].to_string(),
            Field::ProjectLimit if self.cfg.project(&self.root).limit_month > 0.0 => {
                self.cfg.project(&self.root).limit_month.to_string()
            }
            Field::Limit(_) | Field::ProjectLimit => String::new(),
            _ => self.setting(field),
        }
    }

    fn set_setting(&mut self, field: Field, value: &str) {
        let on = value == "on";
        let root = self.root.clone();
        match field {
            Field::BaseKeymap => { self.cfg.base_keymap = value.into(); self.reload_keymap(); }
            Field::ProviderPrice => {
                let parsed: Vec<f64> = value
                    .split(',')
                    .filter_map(|s| s.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0))
                    .collect();
                let Ok(cost) = <[f64; 4]>::try_from(parsed) else {
                    self.message = Some("Enter four non-negative prices separated by commas".into());
                    return;
                };
                self.cfg.providers.get_mut(&self.cfg.editing_provider).unwrap().cost = cost;
            }
            Field::ProviderImages => self.cfg.providers.get_mut(&self.cfg.editing_provider).unwrap().images = on,
            Field::Provider => self.cfg.editing_provider = value.into(),
            Field::ProviderUrl => {
                if !(value.starts_with("https://") || value.starts_with("http://")) || value.contains(['\r', '\n']) {
                    self.message = Some("Use an http:// or https:// API URL".into());
                    return;
                }
                self.cfg.providers.get_mut(&self.cfg.editing_provider).unwrap().api =
                    value.trim().trim_end_matches('/').into();
            }
            Field::ProviderKey => {
                if value.contains(['\r', '\n']) {
                    self.message = Some("API keys must be one line".into());
                    return;
                }
                self.cfg.providers.get_mut(&self.cfg.editing_provider).unwrap().key = value.trim().into();
            }
            Field::ProviderModel => {
                let name = self.cfg.editing_provider.clone();
                self.cfg.providers.get_mut(&name).unwrap().model = value.trim().into();
                if !value.trim().is_empty() {
                    self.cfg.project_mut(&root).model = format!("{name}/{}", value.trim());
                }
            }
            Field::ProviderProtocol => {
                self.cfg.providers.get_mut(&self.cfg.editing_provider).unwrap().protocol = value.into()
            }
            Field::Theme => {
                let value = value.to_string();
                self.set_theme_mode(&value);
            }
            Field::Prose => self.cfg.serif = value == "serif",
            Field::FontSize => {
                self.cfg.font_size = value.trim().parse().map_or(self.cfg.font_size, |n: f32| n.clamp(8.0, 40.0))
            }
            Field::Vim => { self.cfg.modal_editing = value.into(); self.doc.set_modal_editing(value); },
            Field::Minimap => self.cfg.minimap = value.into(),
            Field::MinimapWidth => { if let Ok(v) = value.parse::<f32>() { self.cfg.minimap_width = v.clamp(32.0, 160.0); } }
            Field::AutoSave => self.cfg.auto_save = value.into(),
            Field::Autocomplete => self.cfg.autocomplete = on,
            Field::Spell => self.cfg.spell = on,
            Field::Math => self.cfg.math = on,
            Field::ShellIntegration => self.cfg.shell_integration = on,
            Field::SpellDict => self.cfg.spell_dict = value.trim().to_string(),
            Field::Python => {
                self.cfg.python = value.trim().to_string();
                self.helper = None;
            }
            Field::ApiKey => self.cfg.api_key = value.trim().to_string(),
            Field::Model | Field::SuggestModel | Field::Keys | Field::Engine => {}
            Field::SuggestReasoning => self.cfg.suggest_reasoning = value.to_string(),
            Field::AutoSuggest => { self.cfg.auto_suggest = on; self.unsuggest(); },
            Field::PredictionProvider => { self.cfg.edit_prediction_provider = value.into(); self.unsuggest(); },
            Field::PredictionDelay => self.cfg.edit_prediction_delay_ms = value.parse::<u64>().unwrap_or(300).clamp(100, 5000),
            Field::Reasoning => self.cfg.reasoning = value.to_string(),
            Field::Context => self.cfg.context = on,
            Field::AgentCheck => self.cfg.agent_check = on,
            Field::ShowReasoning => {
                self.cfg.show_reasoning = on;
                self.show_thread();
            }
            Field::Limit(i) => self.cfg.limits[i] = parse_dollars(value),
            Field::Edit => self.cfg.project_mut(&root).edit = Policy::parse(value),
            Field::Run => self.cfg.project_mut(&root).run = Policy::parse(value),
            Field::Web => self.cfg.project_mut(&root).web = Policy::parse(value),
            Field::Instructions => self.cfg.project_mut(&root).instructions = on,
            Field::ProjectLimit => self.cfg.project_mut(&root).limit_month = parse_dollars(value),
        }
        if let Err(e) = self.cfg.save() {
            self.message = Some(format!("Cannot save the settings: {e}"));
        }
    }

    /// Acts on the chosen row: the next choice, or a value to type.
    fn activate(&mut self, back: bool) {
        let Some(sel) = self.settings.as_ref().map(|s| s.sel) else { return };
        let Some(field) = ROWS[sel].0 else { return };
        match kind(field) {
            Kind::Choice(options) => {
                let now = self.setting(field);
                let at = options.iter().position(|o| *o == now).unwrap_or(0);
                let step = if back { options.len() - 1 } else { 1 };
                self.set_setting(field, options[(at + step) % options.len()]);
            }
            Kind::Text => {
                let draft = self.setting_draft(field);
                self.settings.as_mut().unwrap().typing = Some((draft.clone(), draft.len()));
            }
            Kind::Pick if field == Field::SuggestModel => self.open_menu(MenuKind::SuggestModel),
            Kind::Pick if field == Field::Keys => self.open_menu(MenuKind::Keys),
            Kind::Pick if field == Field::Engine => self.open_menu(MenuKind::Engine),
            Kind::Pick => self.open_menu(MenuKind::DefaultModel),
        }
    }

    pub(super) fn settings_key(&mut self, key: Key, mods: Mods, text: Option<&str>) {
        let Some(s) = &mut self.settings else { return };
        if let Some((draft, cur)) = &mut s.typing {
            match key {
                Key::Escape => s.typing = None,
                Key::Enter => {
                    let (value, field) = (draft.clone(), ROWS[s.sel].0);
                    s.typing = None;
                    if let Some(field) = field {
                        self.set_setting(field, &value);
                    }
                }
                Key::Left => *cur = crate::bidi::visual_move(draft, *cur, false),
                Key::Right => *cur = crate::bidi::visual_move(draft, *cur, true),
                Key::Home => *cur = 0,
                Key::End => *cur = draft.len(),
                Key::Backspace => {
                    let from = editor::prev(draft, *cur);
                    draft.replace_range(from..*cur, "");
                    *cur = from;
                }
                Key::Delete => {
                    let to = editor::next(draft, *cur);
                    draft.replace_range(*cur..to, "");
                }
                Key::Char('v') if mods.ctrl => self.paste = true,
                _ => {
                    let typed = text.filter(|t| !mods.ctrl && !mods.alt && !t.chars().any(char::is_control));
                    if let Some(t) = typed {
                        draft.insert_str(*cur, t);
                        *cur += t.len();
                    }
                }
            }
            return;
        }
        s.reveal = true;
        match key {
            Key::Escape if !s.filter.is_empty() => {
                s.filter.clear();
                s.settle();
            }
            Key::Escape => self.settings = None,
            Key::Char(',') if mods.ctrl => self.settings = None,
            Key::Up => s.sel = s.step(false),
            Key::Tab => s.sel = s.step(!mods.shift),
            Key::Down => s.sel = s.step(true),
            Key::Home => {
                s.sel = s.shown().into_iter().find(|&i| ROWS[i].0.is_some()).unwrap_or(s.sel);
            }
            Key::End => {
                s.sel = s.shown().into_iter().rev().find(|&i| ROWS[i].0.is_some()).unwrap_or(s.sel);
            }
            Key::PageDown | Key::PageUp => {
                for _ in 0..6 {
                    s.sel = s.step(key == Key::PageDown);
                }
            }
            Key::Enter | Key::Right => self.activate(false),
            Key::Char(' ') if s.filter.is_empty() => self.activate(false),
            Key::Left if ROWS[s.sel].0.is_some_and(|f| matches!(kind(f), Kind::Choice(_))) => self.activate(true),
            Key::Backspace => {
                s.filter.pop();
                s.settle();
            }
            _ => {
                let typed = text.filter(|t| !mods.ctrl && !mods.alt && !t.chars().any(char::is_control));
                if let Some(t) = typed {
                    s.filter += t;
                    s.settle();
                }
            }
        }
    }

    /// Text pasted while a value is being typed. Returns whether it was taken.
    pub(super) fn settings_paste(&mut self, text: &str) -> bool {
        let Some(s) = self.settings.as_mut() else { return false };
        let line = text.lines().next().unwrap_or("").trim();
        match s.typing.as_mut() {
            Some((draft, cur)) => {
                draft.insert_str(*cur, line);
                *cur += line.len();
            }
            None => {
                s.filter += line;
                s.settle();
            }
        }
        true
    }

    pub(super) fn settings_press(&mut self, x: f32, y: f32) {
        let Some(s) = &mut self.settings else { return };
        let (px, py, pw, ph) = s.panel;
        if x < px || x >= px + pw || y < py || y >= py + ph || ui::inside(s.close, Some((x, y))) {
            self.settings = None;
            return;
        }
        if let Some(row) = s.row_at(x, y).filter(|&r| ROWS[r].0.is_some()) {
            if s.typing.is_some() && row == s.sel {
                return;
            }
            (s.sel, s.typing) = (row, None);
            self.activate(false);
        }
    }

    /// The row under the pointer becomes the chosen one, unless a value is
    /// being typed.
    pub(super) fn settings_motion(&mut self, x: f32, y: f32) {
        let Some(s) = self.settings.as_mut() else { return };
        s.hover = Some((x, y));
        if s.typing.is_some() {
            return;
        }
        if let Some(row) = s.row_at(x, y).filter(|&r| ROWS[r].0.is_some()) {
            s.sel = row;
        }
    }

    /// A right click on a row with choices takes the one before.
    pub(super) fn settings_back(&mut self, x: f32, y: f32) {
        self.settings_motion(x, y);
        let Some(s) = self.settings.as_ref().filter(|s| s.typing.is_none()) else { return };
        if s.row_at(x, y) == Some(s.sel) && ROWS[s.sel].0.is_some_and(|f| matches!(kind(f), Kind::Choice(_))) {
            self.activate(true);
        }
    }

    pub(super) fn settings_scroll(&mut self, dy: f32) {
        if let Some(s) = &mut self.settings {
            let room = s.viewport.1 - s.viewport.0;
            s.scroll = (s.scroll + dy).clamp(0.0, (s.tall - room).max(0.0));
            s.reveal = false;
        }
    }

    pub(super) fn draw_settings(&mut self, c: &mut Canvas) {
        if self.settings.is_none() {
            return;
        }
        let sc = self.scale;
        let size = px(theme::UI_TEXT, sc);
        let small = px(theme::SMALL, sc);
        let pad = px(20.0, sc);
        let head = px(60.0, sc);
        let foot = px(44.0, sc);
        let field_h = px(52.0, sc);
        let group_h = px(36.0, sc);
        let pw = px(680.0, sc).min(self.w - px(24.0, sc)).max(1.0).round();
        let ph = px(720.0, sc).min(self.h - px(48.0, sc)).max(px(120.0, sc)).round();
        let (x, y) = (((self.w - pw) / 2.0).round(), ((self.h - ph) / 2.0).max(px(8.0, sc)).round());
        let spent = agent::WINDOWS.map(|(window, _)| self.ai_spent(window));
        let values: Vec<String> = ROWS.iter().map(|(f, ..)| f.map_or(String::new(), |f| self.setting(f))).collect();
        let project = self.root.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let close_keys = self.keys_of("settings");
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let Some(s) = &mut self.settings else { return };
        s.panel = (x, y, pw, ph);
        ui::surface(c, s.panel, px(theme::RADIUS_2XL, sc), sc);

        // The head: the title, the filter and the way out.
        let large = px(17.0, sc);
        ui::text(c, fonts, x + pad, ui::base(y, head, large), "Settings", font::UI_BOLD, large, theme::fg());
        let btn = px(theme::CONTROL, sc);
        s.close = (x + pw - pad - btn + px(6.0, sc), y + (head - btn) / 2.0, btn, btn);
        ui::icon_button(c, s.close, Icon::X, ui::inside(s.close, s.hover), false, sc);
        ui::tip(s.close, "Close", &format!("Esc, {close_keys}"));
        let fw = px(240.0, sc).min(pw - px(160.0, sc));
        let fr = (s.close.0 - px(8.0, sc) - fw, y + (head - px(32.0, sc)) / 2.0, fw, px(32.0, sc));
        if fw > px(80.0, sc) {
            ui::input(c, fr, s.typing.is_none(), sc);
            let isz = px(theme::SMALL_ICON, sc);
            icons::draw(c, Icon::Search, fr.0 + px(10.0, sc), (fr.1 + (fr.3 - isz) / 2.0).round(), isz, theme::icon());
            let tx = fr.0 + px(30.0, sc);
            if s.filter.is_empty() {
                ui::text(c, fonts, tx, ui::base(fr.1, fr.3, size), "Search settings", font::UI, size, theme::faint());
                if s.typing.is_none() { ui::caret(c,tx,fr.1+fr.3/2.0,size*1.3,sc); }
            } else {
                let shown = ui::fit_start(fonts, font::UI, size, &s.filter, fr.2 - px(40.0, sc));
                ui::text(c, fonts, tx, ui::base(fr.1, fr.3, size), &shown, font::UI, size, theme::fg());
                if s.typing.is_none() {
                    let caret_x = crate::bidi::caret_x(fonts,font::UI,size,&shown,shown.len());
                    ui::caret(c, tx + caret_x + 1.0, fr.1 + fr.3 / 2.0, size * 1.3, sc);
                }
            }
        }
        c.line(x, y + head, pw, 1.0, theme::line());

        // The rows, scrolling between the head and the foot.
        let (vtop, vbottom) = (y + head + 1.0, (y + ph - foot).max(y + head + 1.0));
        s.viewport = (vtop, vbottom);
        let shown = s.shown();
        let height = |i: usize| if ROWS[i].0.is_none() { group_h } else { field_h };
        s.tall = shown.iter().map(|&i| height(i)).sum::<f32>() + px(12.0, sc);
        let room = vbottom - vtop;
        if std::mem::take(&mut s.reveal) {
            let mut at = px(6.0, sc);
            for &i in &shown {
                if i == s.sel {
                    break;
                }
                at += height(i);
            }
            s.scroll = s.scroll.max(at + field_h - room).min(at);
        }
        s.scroll = s.scroll.clamp(0.0, (s.tall - room).max(0.0));
        c.clip(x + 1.0, vtop, pw - 2.0, room);
        s.places.clear();
        let mut ry = vtop + px(6.0, sc) - s.scroll;
        let inset = px(8.0, sc);
        for &i in &shown {
            let (field, label, about) = ROWS[i];
            let h = height(i);
            s.places.push((i, ry, h));
            if ry + h <= vtop || ry >= vbottom {
                ry += h;
                continue;
            }
            let Some(field) = field else {
                // A group's name, with the project's own after it where the
                // group belongs to the project.
                let gx = x + pad;
                let b = ui::base(ry + px(8.0, sc), h - px(8.0, sc), small);
                let lw = ui::text(c, fonts, gx, b, label, font::UI_MEDIUM, small, theme::dim());
                if label == "This project" && !project.is_empty() {
                    let name = ui::fit(fonts, font::UI, small, &project, pw - pad * 2.0 - lw);
                    ui::text(c, fonts, gx + lw + px(6.0, sc), b, &name, font::UI, small, theme::faint());
                }
                ry += h;
                continue;
            };
            let on = i == s.sel;
            let r = (x + inset, ry + px(2.0, sc), pw - inset * 2.0, h - px(4.0, sc));
            ui::row_ground(c, r, on, false, false, sc);
            let value = &values[i];
            let lx = x + pad;
            // The control at the right, and how wide it is.
            let right = x + pw - pad;
            let mid = ry + h / 2.0;
            let control_w = match (kind(field), s.typing.as_ref().filter(|_| on)) {
                (_, Some((draft, cur))) => {
                    let fw = (pw * 0.42).round();
                    let fr = (right - fw, mid - px(15.0, sc), fw, px(30.0, sc));
                    ui::input(c, fr, true, sc);
                    let masked = "\u{2022}".repeat(draft.chars().count());
                    let (shown, before) = if matches!(field, Field::ApiKey | Field::ProviderKey) {
                        (&masked, &masked[..draft[..*cur].chars().count() * '\u{2022}'.len_utf8()])
                    } else {
                        (draft, &draft[..*cur])
                    };
                    let fpad = px(10.0, sc);
                    let bw = crate::bidi::caret_x(fonts,font::UI,size,shown,before.len());
                    let offset = (bw - (fw - fpad * 2.0) + px(4.0, sc)).max(0.0);
                    c.clip(fr.0 + 1.0, fr.1, fr.2 - 2.0, fr.3);
                    ui::text(
                        c,
                        fonts,
                        fr.0 + fpad - offset,
                        ui::base(fr.1, fr.3, size),
                        shown,
                        font::UI,
                        size,
                        theme::fg(),
                    );
                    if !matches!(field,Field::ApiKey | Field::ProviderKey) { ui::remember_input(51,shown,font::UI,size,fr,fr.0+fpad-offset); }
                    ui::caret(c, fr.0 + fpad - offset + bw, mid, size * 1.3, sc);
                    c.clip(x + 1.0, vtop, pw - 2.0, room);
                    fw
                }
                (Kind::Choice(ON_OFF), None) => {
                    let (sw, sh) = ui::switch_size(sc);
                    ui::switch(c, right - sw, (mid - sh / 2.0).round(), value == "on", sc);
                    sw
                }
                (kind, None) => {
                    let face = font::UI_MEDIUM;
                    let isz = px(12.0, sc);
                    let cap = (pw * 0.42).round();
                    let text = ui::fit(fonts, face, size, value, cap - isz - px(30.0, sc));
                    let tw = fonts.width(face, size, &text);
                    let cw = (tw + isz + px(26.0, sc)).round();
                    let cr = (right - cw, mid - px(14.0, sc), cw, px(28.0, sc));
                    let ink = if matches!(kind, Kind::Text) && (value == "none" || value == "not set") {
                        theme::faint()
                    } else {
                        theme::fg()
                    };
                    c.round_rect(cr.0, cr.1, cr.2, cr.3, px(theme::RADIUS, sc), theme::bg());
                    c.round_ring(cr.0, cr.1, cr.2, cr.3, px(theme::RADIUS, sc), 1.0, theme::field());
                    ui::text(c, fonts, cr.0 + px(10.0, sc), ui::base(cr.1, cr.3, size), &text, face, size, ink);
                    let icon = match kind {
                        Kind::Choice(_) => Icon::ChevronDown,
                        Kind::Pick => Icon::ChevronRight,
                        Kind::Text => Icon::SquarePen,
                    };
                    icons::draw(
                        c,
                        icon,
                        cr.0 + cr.2 - px(8.0, sc) - isz,
                        (mid - isz / 2.0).round(),
                        isz,
                        theme::icon(),
                    );
                    cw
                }
            };
            let room = right - control_w - px(16.0, sc) - lx;
            let label = ui::fit(fonts, font::UI_MEDIUM, size, label, room);
            ui::text(c, fonts, lx, (ry + h / 2.0 - px(3.0, sc)).round(), &label, font::UI_MEDIUM, size, theme::fg());
            let about = ui::fit(fonts, font::UI, small, about, room);
            ui::text(c, fonts, lx, (ry + h / 2.0 + small + px(1.0, sc)).round(), &about, font::UI, small, theme::dim());
            ry += h;
        }
        if shown.iter().all(|&i| ROWS[i].0.is_none()) {
            let said = "No settings match";
            let sw = fonts.width(font::UI, size, said);
            ui::text(c, fonts, x + (pw - sw) / 2.0, vtop + px(40.0, sc), said, font::UI, size, theme::dim());
        }
        c.unclip();
        if s.tall > room {
            ui::scrollbar(c, (x, vtop, pw - px(2.0, sc), room), s.scroll / s.tall, room / s.tall, false, sc);
        }

        // The foot: what the agent has cost, and the keys.
        let fy = y + ph - foot;
        c.line(x, fy, pw, 1.0, theme::line());
        let mut fx = x + pad;
        let b = ui::base(fy, foot, small);
        fx += ui::text(c, fonts, fx, b, "Spent", font::UI_MEDIUM, small, theme::fg()) + px(8.0, sc);
        for (amount, window) in spent.iter().zip(["5 h", "week", "30 days"]) {
            let part = format!("${:.2} {window}", amount + 0.0);
            if fx + fonts.width(font::UI, small, &part) > x + pw * 0.55 {
                break;
            }
            fx += ui::text(c, fonts, fx, b, &part, font::UI, small, theme::dim()) + px(12.0, sc);
        }
        let keys: [(&str, &str); 3] = [("Up", "choose"), ("Enter", "change"), ("Esc", "close")];
        let mut widths = Vec::new();
        for (k, d) in keys {
            widths.push(ui::kbd_width(fonts, k, sc) + px(5.0, sc) + fonts.width(font::UI, small, d));
        }
        let total: f32 = widths.iter().sum::<f32>() + px(12.0, sc) * 2.0;
        let mut kx = x + pw - pad - total;
        if kx > fx + px(12.0, sc) {
            for ((k, d), w) in keys.iter().zip(widths) {
                let used = ui::kbd(c, fonts, kx, fy + foot / 2.0, k, sc) + px(5.0, sc);
                ui::text(c, fonts, kx + used, b, d, font::UI, small, theme::faint());
                kx += w + px(12.0, sc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(app: &mut App) {
        let mut px = vec![0; (app.w * app.h) as usize];
        app.draw(&mut Canvas::new(&mut px, app.w as i32, app.h as i32));
    }

    #[test]
    fn settings_keep_the_keyboard_choice_visible_in_small_windows() {
        let mut app = App::new(None);
        app.resize(320, 240, 1.0);
        app.settings = Some(Settings::new());
        draw(&mut app);
        app.settings_key(Key::End, Mods::default(), None);
        draw(&mut app);
        let s = app.settings.as_ref().unwrap();
        let (top, h) = s.place(s.sel).unwrap();
        assert!(s.panel.1 >= 0.0 && s.panel.1 + s.panel.3 <= app.h);
        assert!(top >= s.viewport.0 && top + h <= s.viewport.1 + 0.5, "{top} {h} {:?}", s.viewport);
        let row = s.sel;
        app.settings_key(Key::Tab, Mods { shift: true, ..Mods::default() }, None);
        assert!(app.settings.as_ref().unwrap().sel < row);
        app.settings_key(Key::Home, Mods::default(), None);
        draw(&mut app);
        assert_eq!(app.settings.as_ref().unwrap().sel, 1);
        app.settings_key(Key::Escape, Mods::default(), None);
        assert!(app.settings.is_none());
    }

    #[test]
    fn typing_narrows_the_settings() {
        let mut app = App::new(None);
        app.resize(900, 700, 1.0);
        app.settings = Some(Settings::new());
        for ch in "python".chars() {
            app.settings_key(Key::Char(ch), Mods::default(), Some(&ch.to_string()));
        }
        let s = app.settings.as_ref().unwrap();
        let fields: Vec<&str> = s.shown().into_iter().filter(|&i| ROWS[i].0.is_some()).map(|i| ROWS[i].1).collect();
        assert!(fields.contains(&"Python interpreter") && !fields.contains(&"Theme"), "{fields:?}");
        assert_eq!(ROWS[s.sel].1, fields[0]);
        // Esc clears the filter first, then closes.
        app.settings_key(Key::Escape, Mods::default(), None);
        assert!(app.settings.as_ref().is_some_and(|s| s.filter.is_empty()));
        // Enter on a switch flips it.
        let on = app.cfg.autocomplete;
        for ch in "autocomplete".chars() {
            app.settings_key(Key::Char(ch), Mods::default(), Some(&ch.to_string()));
        }
        app.settings_key(Key::Enter, Mods::default(), None);
        assert_eq!(app.cfg.autocomplete, !on);
    }
}
