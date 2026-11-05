//! Keymap loading and the boundary between focus, commands and modal editing.
use super::*;
use crate::keys::contextual::{self, Binding, Keymap as ContextMap, Match, Sequence, State};
use std::time::{Duration, Instant};

pub(super) const COMMANDS: &[(&str, &str)] = &[
    ("keymap_open", "Preferences: Open keymap file"),
    ("keymap_reload", "Preferences: Reload keymap file"),
    ("keymap_mark", "Edit: Set or clear selection mark"),
    ("keymap_cancel", "Edit: Cancel chord or selection"),
    ("move_line_start", "Edit: Move to line start"),
    ("move_line_end", "Edit: Move to line end"),
    ("move_left", "Edit: Move left"),
    ("move_right", "Edit: Move right"),
    ("move_up", "Edit: Move up"),
    ("move_down", "Edit: Move down"),
    ("move_word_left", "Edit: Move word left"),
    ("move_word_right", "Edit: Move word right"),
    ("select_line", "Edit: Select line"),
    ("menu_next", "Menu: Select next item"),
    ("menu_previous", "Menu: Select previous item"),
    ("menu_confirm", "Menu: Confirm selected item"),
];
pub(super) fn commands() -> Vec<(&'static str, &'static str, crate::keys::Scope)> {
    ACTIONS
        .iter()
        .map(|a| (a.id, a.name, a.scope))
        .chain(
            super::palette::EXTRA
                .iter()
                .chain(COMMANDS)
                .filter(|(id, _)| !ACTIONS.iter().any(|a| a.id == *id))
                .map(|(id, name)| (*id, *name, crate::keys::Scope::Main)),
        )
        .collect()
}
pub(super) fn path() -> PathBuf {
    config::config_dir().join("keymap.json")
}
#[derive(Default)]
pub(super) struct KeyBindings {
    pub map: ContextMap,
    pub user: Vec<Binding>,
    pub recording: Vec<Chord>,
    pub context: String,
    pub mark: bool,
    stamp: Option<std::time::SystemTime>,
    checked: Option<Instant>,
    pub error: Option<String>,
}
impl KeyBindings {
    pub fn load(cfg: &config::Config) -> Self {
        let mut state = Self::default();
        if cfg!(test) {
            state.map = ContextMap::build(&cfg.base_keymap, &cfg.keys, Vec::new());
            return state;
        }
        if let Err(error) = state.reload(cfg) {
            state.error = Some(error);
            state.map = ContextMap::build(&cfg.base_keymap, &cfg.keys, Vec::new());
        }
        state
    }
    pub fn reload(&mut self, cfg: &config::Config) -> Result<(), String> {
        let text = match std::fs::read_to_string(path()) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "[]".into(),
            Err(e) => return Err(e.to_string()),
        };
        let user = contextual::parse(&text)?;
        self.map = ContextMap::build(&cfg.base_keymap, &cfg.keys, user.clone());
        self.user = user;
        self.stamp = std::fs::metadata(path()).and_then(|m| m.modified()).ok();
        self.error = None;
        Ok(())
    }
}
impl App {
    pub(super) fn keymap_state(&self) -> State {
        let mut state = State::default();
        let main = self.prompt.is_none() && !self.overlaid();
        if main {
            state.flag("Main");
        }
        if self.popup.is_some() || self.prompt_has_keys() || self.settings.is_some() || self.chooser.is_some() {
            state.flag("Dialog");
            state.typing = true;
        } else if self.menu.is_some() {
            state.flag("Palette");
            state.typing = true;
        } else if self.search_panel_has_keys() {
            state.flag("Search");
            state.flag("SearchPanel");
            state.flag("Panel");
            state.typing = self.search.panel.field != 2;
        } else if self.term_has_keys() {
            state.flag("Terminal");
            state.typing = true;
        } else if self.focus == Focus::Files {
            state.flag("Panel");
            if self.source_shown() {
                state.flag("SourceControl");
                state.typing = self.source.typing;
            } else {
                state.flag("Explorer");
            }
        } else if self.focus == Focus::Chat {
            state.flag("AgentComposer");
            state.typing = true;
        } else if self.in_side() {
            state.flag("Panel");
        } else {
            state.flag("Editor");
            state.flag("Document");
            if self.nb.is_some() {
                state.flag("Notebook");
            }
            state.value(
                "mode",
                if self.doc.doc == Doc::Markdown && !self.doc.edit_mode { "markdown" } else { "source" },
            );
            state.value(
                "vim_mode",
                match self.doc.vim_label() {
                    Some("INSERT" | "REPLACE") => "insert",
                    Some("VISUAL" | "V-LINE" | "V-BLOCK") => "visual",
                    Some(_) => "normal",
                    None => "off",
                },
            );
            // The modal implementation owns interpretation. Labels provide a read-only context hook.
            if let Some(label) = self.doc.vim_label() {
                state.value(
                    "modal_mode",
                    if label.contains("INSERT") {
                        "insert"
                    } else if label.contains("VISUAL") || label.starts_with("V-") || label.contains("SELECT") {
                        "visual"
                    } else {
                        "normal"
                    },
                );
            }
            if let Some(label) = self.doc.vim_label().filter(|s| s.contains("HELIX") || s.starts_with("HX")) {
                state.value("vim_mode", "off");
                state.value(
                    "helix_mode",
                    if label.contains("INSERT") {
                        "insert"
                    } else if label.contains("SELECT") || label.contains("VISUAL") {
                        "visual"
                    } else {
                        "normal"
                    },
                );
            }
            state.typing = self.doc.vim_typing();
        }
        state
    }
    pub(super) fn contextual_key(&mut self, key: Key, mods: Mods) -> bool {
        let state = self.keymap_state();
        match self.key_bindings.map.feed(Chord::new(key, mods), &state) {
            Match::Pass => false,
            Match::Consumed => true,
            Match::Command(id) => {
                if matches!(id.as_str(), "cut" | "copy" | "paste") {
                    self.key_bindings.mark = false;
                }
                if !self.keymap_command(&id) {
                    self.run_command(&id);
                }
                true
            }
        }
    }
    pub(super) fn keymap_poll(&mut self) {
        if cfg!(test) {
            return;
        }
        if self.key_bindings.checked.is_some_and(|t| t.elapsed() < Duration::from_millis(400)) {
            return;
        }
        self.key_bindings.checked = Some(Instant::now());
        let stamp = std::fs::metadata(path()).and_then(|m| m.modified()).ok();
        if stamp != self.key_bindings.stamp {
            // Remember invalid saves as well, so one bad file reports once.
            self.key_bindings.stamp = stamp;
            self.reload_keymap();
            self.redraw = true;
        }
    }
    pub(super) fn reload_keymap(&mut self) {
        if let Err(e) = self.key_bindings.reload(&self.cfg) {
            self.message = Some(format!("Cannot reload keymap: {e}"));
        }
    }
    pub(super) fn keymap_command(&mut self, id: &str) -> bool {
        match id {
            "save" => self.save(),
            "save_as" => self.ask(PromptKind::SaveAs),
            "menu_next" | "menu_previous" | "menu_confirm" => {
                let key = match id {
                    "menu_next" => Key::Down,
                    "menu_previous" => Key::Up,
                    _ => Key::Enter,
                };
                self.dispatch(key, Mods::default(), None);
            }
            "keymap_open" => {
                if !path().exists()
                    && let Err(e) = config::write_private(&path(), b"[]\n")
                {
                    self.message = Some(format!("Cannot create keymap: {e}"));
                    return true;
                }
                self.open_tab(path());
            }
            "keymap_reload" => self.reload_keymap(),
            "keymap_cancel" => {
                self.key_bindings.map.cancel();
                self.key_bindings.mark = false;
                let cur = self.doc.buf.cur;
                self.doc.select(cur, cur);
                self.dispatch(Key::Escape, Mods::default(), None);
            }
            "keymap_mark" => {
                self.key_bindings.mark = !self.key_bindings.mark;
                self.message = Some(if self.key_bindings.mark { "Mark set" } else { "Mark cleared" }.into());
            }
            "select_line" => {
                self.doc.key(Key::Home, Mods::default(), None);
                self.doc.key(Key::End, Mods { shift: true, ..Mods::default() }, None);
            }
            "move_line_start" | "move_line_end" => {
                let (start, end) = self.doc.buf.line(self.doc.buf.line_of(self.doc.buf.cur));
                let to = if id == "move_line_start" { start } else { end };
                let anchor = if self.key_bindings.mark { self.doc.buf.anchor } else { to };
                self.doc.select(anchor, to);
                self.doc.moved();
            }
            "move_left" | "move_right" | "move_up" | "move_down" | "move_word_left" | "move_word_right" => {
                let key = match id {
                    "move_left" | "move_word_left" => Key::Left,
                    "move_right" | "move_word_right" => Key::Right,
                    "move_up" => Key::Up,
                    _ => Key::Down,
                };
                self.doc.key(key, Mods { ctrl: id.contains("word"), shift: self.key_bindings.mark, alt: false }, None);
            }
            _ => return false,
        }
        self.collect();
        self.redraw = true;
        true
    }
    pub(super) fn binding_sequences(&self, id: &str, context: &str) -> Vec<Sequence> {
        let mut out = Vec::new();
        if let Some(a) = ACTIONS.iter().find(|a| a.id == id) {
            for key in a.keys {
                if let Some(seq) = Sequence::parse(key) {
                    out.push(seq);
                }
            }
        }
        for b in &self.key_bindings.map.bindings {
            if b.context.code == context {
                out.retain(|seq| *seq != b.sequence);
                if b.command.as_deref() == Some(id) {
                    out.push(b.sequence.clone());
                }
            }
        }
        out
    }
    pub(super) fn binding_label(&self, id: &str, context: &str) -> String {
        self.binding_sequences(id, context).iter().map(Sequence::name).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
    fn bind(app: &mut App, text: &str) {
        app.key_bindings.map = ContextMap::build(&app.cfg.base_keymap, &app.cfg.keys, contextual::parse(text).unwrap());
    }
    #[test]
    fn chords_dispatch_commands_and_cancel_without_editing() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("one two");
        bind(&mut a, r#"[{"context":"Editor","bindings":{"ctrl-k ctrl-a":"select_all"}}]"#);
        a.key(Key::Char('k'), CTRL, None);
        assert!(a.key_bindings.map.hint.as_ref().unwrap().contains("Ctrl+A"));
        a.key(Key::Char('a'), CTRL, None);
        assert_eq!(a.doc.buf.sel(), (0, 7));
        assert!(a.key_bindings.map.pending.is_empty());
        a.doc.select(7, 7);
        a.key(Key::Char('k'), CTRL, None);
        a.key(Key::Char('q'), NONE, Some("q"));
        assert_eq!(a.doc.buf.text, "one two");
        a.key(Key::Char('k'), CTRL, None);
        a.key(Key::Escape, NONE, None);
        assert!(a.key_bindings.map.pending.is_empty());
    }
    #[test]
    fn focus_source_and_modal_contexts_are_read_only() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        assert!(a.keymap_state().flags.contains(&"Editor".into()));
        a.doc.vim.enabled = true;
        assert_eq!(a.keymap_state().values["vim_mode"], "normal");
        assert!(!a.keymap_state().typing);
        a.doc.vim.enabled = false;
        a.open_palette(">");
        assert!(a.keymap_state().flags.contains(&"Palette".into()));
        a.menu = None;
        a.files_open = true;
        a.focus = Focus::Files;
        assert!(a.keymap_state().flags.contains(&"Explorer".into()));
        assert!(!a.keymap_state().typing);
        bind(&mut a, r#"[{"context":"Explorer","bindings":{"x":"palette"}}]"#);
        a.key(Key::Char('x'), NONE, Some("x"));
        assert!(a.menu.is_some());
    }
    #[test]
    fn emacs_mark_movement_copy_cut_and_yank_reach_editor_services() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("one two\nthree");
        a.cfg.base_keymap = "emacs".into();
        a.key_bindings.map = ContextMap::build("emacs", &a.cfg.keys, Vec::new());
        a.doc.select(0, 0);
        a.key(Key::Char(' '), CTRL, None);
        assert!(a.key_bindings.mark);
        a.key(Key::Char('f'), CTRL, None);
        a.key(Key::Char('f'), CTRL, None);
        assert_eq!(a.doc.buf.sel(), (0, 2));
        a.key(Key::Char('w'), Mods { alt: true, ..NONE }, None);
        assert_eq!(a.copy.as_deref(), Some("on"));
        assert!(!a.key_bindings.mark);
        a.key(Key::Char('w'), CTRL, None);
        assert_eq!(a.doc.buf.text, "e two\nthree");
        a.key(Key::Char('y'), CTRL, None);
        assert!(a.paste);
        a.key(Key::Char('x'), CTRL, None);
        a.key(Key::Char('f'), CTRL, None);
        assert!(a.chooser.is_some());
    }
}
