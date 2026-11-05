//! Context expressions, chord resolution and the user keymap document.
use super::{ACTIONS, Chord, Scope};
use crate::app::{Key, Mods};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const BASES: &[&str] = &["lightdown", "vscode", "jetbrains", "sublime", "emacs"];

#[derive(Clone, Debug, PartialEq)]
pub struct Sequence(pub Vec<Chord>);
impl Sequence {
    pub fn parse(code: &str) -> Option<Self> {
        let keys: Option<Vec<_>> = code.split_whitespace().map(stroke).collect();
        keys.filter(|k| !k.is_empty()).map(Self)
    }
    pub fn code(&self) -> String {
        self.0.iter().map(|key| key.write("ctrl-", "shift-", "alt-", false)).collect::<Vec<_>>().join(" ")
    }
    pub fn name(&self) -> String {
        self.0.iter().map(Chord::name).collect::<Vec<_>>().join(" ")
    }
}
fn stroke(code: &str) -> Option<Chord> {
    let mut rest = code;
    let mut prefix = String::new();
    loop {
        let lower = rest.to_ascii_lowercase();
        let found = [
            ("ctrl", "ctrl+"),
            ("control", "ctrl+"),
            ("c", "ctrl+"),
            ("alt", "alt+"),
            ("meta", "alt+"),
            ("m", "alt+"),
            ("shift", "shift+"),
        ]
        .iter()
        .find_map(|(alias, value)| {
            ['-', '+']
                .into_iter()
                .find_map(|sep| lower.strip_prefix(&format!("{alias}{sep}")).map(|r| (*value, r.len())))
        });
        let Some((value, len)) = found else { break };
        if len == 0 {
            break;
        }
        prefix.push_str(value);
        rest = &rest[rest.len() - len..];
    }
    let lower = rest.to_ascii_lowercase();
    let special = match lower.as_str() {
        "escape" | "esc" => Some(Key::Escape),
        "backspace" => Some(Key::Backspace),
        "delete" => Some(Key::Delete),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        _ => None,
    };
    if let Some(key) = special {
        let mods =
            Mods { ctrl: prefix.contains("ctrl+"), alt: prefix.contains("alt+"), shift: prefix.contains("shift+") };
        return Some(Chord::new(key, mods));
    }
    Chord::parse(&format!("{prefix}{lower}"))
}

#[derive(Clone, Debug)]
enum Expr {
    Atom(String),
    Eq(String, String, bool),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}
#[derive(Clone, Debug)]
pub struct Context {
    pub code: String,
    expr: Expr,
}
#[derive(Clone, Default, Debug)]
pub struct State {
    pub flags: Vec<String>,
    pub values: BTreeMap<String, String>,
    pub typing: bool,
}
impl State {
    pub fn flag(&mut self, name: &str) {
        self.flags.push(name.into());
    }
    pub fn value(&mut self, name: &str, value: &str) {
        self.values.insert(name.into(), value.into());
    }
    fn atom(&self, name: &str) -> bool {
        name == "true" || self.flags.iter().any(|s| s == name)
    }
}
impl Expr {
    fn score(&self, state: &State) -> Option<usize> {
        match self {
            Self::Atom(a) => state.atom(a).then_some(usize::from(a != "true")),
            Self::Eq(k, v, equal) => ((state.values.get(k).map(String::as_str) == Some(v)) == *equal).then_some(1),
            Self::Not(e) => e.score(state).is_none().then_some(1),
            Self::And(a, b) => Some(a.score(state)? + b.score(state)?),
            Self::Or(a, b) => a.score(state).into_iter().chain(b.score(state)).max(),
        }
    }
}
impl Context {
    pub fn parse(code: &str) -> Result<Self, String> {
        let source = if code.trim().is_empty() { "true" } else { code.trim() };
        let mut tokens = Vec::new();
        let mut start = 0;
        let bytes = source.as_bytes();
        while start < bytes.len() {
            if bytes[start].is_ascii_whitespace() {
                start += 1;
                continue;
            }
            let end = if matches!(bytes[start], b'(' | b')') {
                start + 1
            } else if start + 1 < bytes.len()
                && [b"&&", b"||", b"==", b"!="].iter().any(|op| op.as_slice() == &bytes[start..start + 2])
            {
                start + 2
            } else if bytes[start] == b'!' {
                start + 1
            } else {
                let mut end = start;
                while end < bytes.len() && !bytes[end].is_ascii_whitespace() && !b"()&|=!".contains(&bytes[end]) {
                    end += 1;
                }
                end
            };
            if end == start {
                return Err(format!("Invalid context: {code}"));
            }
            tokens.push(source[start..end].to_string());
            start = end;
        }
        fn primary(t: &[String], at: &mut usize) -> Option<Expr> {
            let token = t.get(*at)?.clone();
            *at += 1;
            if token == "!" {
                return Some(Expr::Not(Box::new(primary(t, at)?)));
            }
            if token == "(" {
                let e = expression(t, at, false)?;
                if t.get(*at)? != ")" {
                    return None;
                }
                *at += 1;
                return Some(e);
            }
            if [")", "&&", "||", "==", "!="].contains(&token.as_str()) {
                return None;
            }
            if t.get(*at).is_some_and(|s| s == "==" || s == "!=") {
                let equal = t[*at] == "==";
                *at += 1;
                let value = t.get(*at)?.trim_matches(['\'', '"']).to_string();
                *at += 1;
                return Some(Expr::Eq(token, value, equal));
            }
            Some(Expr::Atom(token))
        }
        fn expression(t: &[String], at: &mut usize, and: bool) -> Option<Expr> {
            let mut out = if and { primary(t, at)? } else { expression(t, at, true)? };
            let op = if and { "&&" } else { "||" };
            while t.get(*at).is_some_and(|s| s == op) {
                *at += 1;
                let rhs = if and { primary(t, at)? } else { expression(t, at, true)? };
                out =
                    if and { Expr::And(Box::new(out), Box::new(rhs)) } else { Expr::Or(Box::new(out), Box::new(rhs)) };
            }
            Some(out)
        }
        let mut at = 0;
        let expr = expression(&tokens, &mut at, false)
            .filter(|_| at == tokens.len())
            .ok_or_else(|| format!("Invalid context: {code}"))?;
        Ok(Self { code: code.into(), expr })
    }
    pub fn score(&self, state: &State) -> Option<usize> {
        self.expr.score(state)
    }
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub context: Context,
    pub sequence: Sequence,
    pub command: Option<String>,
    pub layer: u8,
}
#[derive(Default)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
    pub pending: Vec<Chord>,
    pending_state: String,
    pub hint: Option<String>,
}
#[derive(Debug, PartialEq)]
pub enum Match {
    Pass,
    Consumed,
    Command(String),
}

pub fn scope_context(scope: Scope) -> &'static str {
    match scope {
        Scope::Always => "",
        Scope::Main => "Main",
        Scope::Doc => "Document",
        Scope::Notebook => "Notebook",
        Scope::Chat => "AgentComposer",
    }
}

pub fn parse(text: &str) -> Result<Vec<Binding>, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let blocks = value.as_array().ok_or("Keymap must be an array")?;
    let mut out = Vec::new();
    for block in blocks {
        let context = match block.get("context") {
            None => "",
            Some(Value::String(s)) => s,
            _ => return Err("Context must be a string".into()),
        };
        let context = Context::parse(context)?;
        let binds = block.get("bindings").and_then(Value::as_object).ok_or("Each keymap block needs bindings")?;
        for (code, command) in binds {
            let sequence = Sequence::parse(code).ok_or_else(|| format!("Invalid key sequence: {code}"))?;
            let command = if command.is_null() {
                None
            } else {
                let id = command
                    .as_str()
                    .filter(|s| crate::app::keymap_command_known(s))
                    .ok_or_else(|| format!("Unknown keymap command: {command}"))?;
                Some(id.to_string())
            };
            out.push(Binding { context: context.clone(), sequence, command, layer: 2 });
        }
    }
    Ok(out)
}
impl Keymap {
    pub fn build(base: &str, legacy: &BTreeMap<String, Vec<String>>, user: Vec<Binding>) -> Self {
        let mut map = Self::default();
        for &(context, code, command) in super::bases::bindings(base) {
            map.add(context, code, Some(command), 0);
        }
        // Moving a command to a base's keys suppresses its old defaults in that context.
        let moved = map.bindings.clone();
        for binding in moved {
            if let Some(action) = ACTIONS.iter().find(|a| Some(a.id) == binding.command.as_deref()) {
                for code in action.keys {
                    map.add(&binding.context.code, code, None, 0);
                }
            }
        }
        // Put live base bindings after their tombstones, including self assignments.
        for &(context, code, command) in super::bases::bindings(base) {
            map.add(context, code, Some(command), 0);
        }
        for (id, codes) in legacy {
            let Some(action) = ACTIONS.iter().find(|a| a.id == id) else { continue };
            let context = scope_context(action.scope);
            for old in action.keys {
                map.add(context, old, None, 1);
            }
            for code in codes {
                map.add(context, code, Some(id), 1);
            }
        }
        map.bindings.extend(user);
        map
    }
    fn add(&mut self, context: &str, code: &str, command: Option<&str>, layer: u8) {
        if let (Ok(context), Some(sequence)) = (Context::parse(context), Sequence::parse(code)) {
            self.bindings.push(Binding { context, sequence, command: command.map(str::to_string), layer });
        }
    }
    pub fn cancel(&mut self) {
        self.pending.clear();
        self.hint = None;
        self.pending_state.clear();
    }
    pub fn feed(&mut self, key: Chord, state: &State) -> Match {
        let signature = format!("{:?}{:?}", state.flags, state.values);
        if !self.pending.is_empty() && (self.pending_state != signature || key.key == Key::Escape) {
            self.cancel();
            return Match::Consumed;
        }
        let was_pending = !self.pending.is_empty();
        self.pending.push(key);
        let mut effective: Vec<(usize, &Binding, (usize, u8, usize))> = Vec::new();
        for (index, b) in self.bindings.iter().enumerate() {
            let Some(score) = b.context.score(state) else { continue };
            // A plain first stroke must never eat text in an input field.
            let first = b.sequence.0[0];
            if state.typing && !first.assignable() && !first.mods.ctrl && !first.mods.alt {
                continue;
            }
            if !b.sequence.0.starts_with(&self.pending) {
                continue;
            }
            let rank = (score, b.layer, index);
            if let Some(existing) = effective.iter_mut().find(|(_, old, _)| old.sequence == b.sequence) {
                if rank > existing.2 {
                    *existing = (index, b, rank);
                }
            } else {
                effective.push((index, b, rank));
            }
        }
        let exact = effective
            .iter()
            .filter(|(_, b, _)| b.sequence.0.len() == self.pending.len())
            .max_by_key(|(_, _, rank)| *rank);
        let longer: Vec<_> = effective
            .iter()
            .filter(|(_, b, rank)| {
                b.command.is_some()
                    && b.sequence.0.len() > self.pending.len()
                    && exact.is_none_or(|(_, b, r)| b.command.is_none() || (rank.0, rank.1) >= (r.0, r.1))
            })
            .collect();
        if !longer.is_empty() {
            self.pending_state = signature;
            self.hint = Some(format!(
                "{}  →  {}",
                Sequence(self.pending.clone()).name(),
                longer
                    .iter()
                    .map(|(_, b, _)| format!(
                        "{}: {}",
                        Sequence(b.sequence.0[self.pending.len()..].to_vec()).name(),
                        b.command.as_deref().unwrap()
                    ))
                    .collect::<Vec<_>>()
                    .join(" · ")
            ));
            return Match::Consumed;
        }
        let result = exact
            .map(|(_, b, _)| b.command.clone().map_or(Match::Consumed, Match::Command))
            .unwrap_or(if was_pending { Match::Consumed } else { Match::Pass });
        self.cancel();
        result
    }
}

pub fn edit_document(
    text: &str,
    context: &str,
    command: &str,
    old: &[Sequence],
    replacement: Option<&Sequence>,
    reset: bool,
) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let blocks = value.as_array_mut().ok_or("Keymap must be an array")?;
    for block in blocks.iter_mut().filter(|b| b["context"].as_str().unwrap_or("") == context) {
        if let Some(binds) = block["bindings"].as_object_mut() {
            binds.retain(|code, id| {
                id.as_str() != Some(command)
                    && !(reset && id.is_null() && Sequence::parse(code).is_some_and(|s| old.contains(&s)))
            });
        }
    }
    if !reset {
        let mut binds = serde_json::Map::new();
        for sequence in old {
            binds.insert(sequence.code(), Value::Null);
        }
        if let Some(sequence) = replacement {
            binds.insert(sequence.code(), json!(command));
        }
        blocks.push(json!({"context":context,"bindings":binds}));
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&value).unwrap()))
}

/// The focus and modal states in which a binding is allowed to start with a plain key.
pub fn plain_context(context: &str) -> bool {
    let Ok(context) = Context::parse(context) else { return false };
    states().iter().any(|s| !s.typing && context.score(s).is_some())
        && !states().iter().any(|s| s.typing && context.score(s).is_some())
}
pub fn contexts_overlap(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (Context::parse(a), Context::parse(b)) else { return false };
    states().iter().any(|s| a.score(s).is_some() && b.score(s).is_some())
}
fn states() -> &'static [State] {
    static STATES: std::sync::OnceLock<Vec<State>> = std::sync::OnceLock::new();
    STATES.get_or_init(|| {
        let mut out = Vec::new();
        for flag in [
            "Editor",
            "Terminal",
            "Palette",
            "Explorer",
            "AgentComposer",
            "Dialog",
            "SourceControl",
            "SearchPanel",
            "Panel",
        ] {
            for mode in ["source", "markdown"] {
                for modal in ["off", "normal", "insert", "visual"] {
                    for kind in ["vim_mode", "helix_mode"] {
                        for typing in [false, true] {
                            let mut state = State::default();
                            state.flag(flag);
                            if !matches!(flag, "Palette" | "Dialog") {
                                state.flag("Main");
                            }
                            if flag == "Editor" {
                                state.flag("Document");
                                state.flag("Notebook");
                            }
                            if ["Explorer", "SourceControl", "SearchPanel"].contains(&flag) {
                                state.flag("Panel");
                            }
                            if flag == "SearchPanel" {
                                state.flag("Search");
                            }
                            state.value("mode", mode);
                            state.value(kind, modal);
                            state.value("modal_mode", modal);
                            state.typing = match flag {
                                "Editor" => matches!(modal, "off" | "insert"),
                                "Explorer" | "Panel" => false,
                                "SourceControl" | "SearchPanel" => typing,
                                _ => true,
                            };
                            if flag == "Editor" {
                                let mut plain = state.clone();
                                plain.flags.retain(|f| f != "Notebook");
                                out.push(plain);
                            }
                            out.push(state);
                        }
                    }
                }
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn editor() -> State {
        let mut s = State::default();
        s.flag("Main");
        s.flag("Editor");
        s.flag("Document");
        s.typing = true;
        s.value("mode", "source");
        s.value("vim_mode", "off");
        s
    }
    fn make_map(text: &str) -> Keymap {
        Keymap::build("lightdown", &BTreeMap::new(), parse(text).unwrap())
    }
    fn feed(map: &mut Keymap, code: &str, state: &State) -> Match {
        map.feed(Sequence::parse(code).unwrap().0[0], state)
    }
    #[test]
    fn sequences_accept_zed_legacy_and_emacs_spellings() {
        for code in ["ctrl-k ctrl-s", "ctrl+k ctrl+s", "C-k C-s"] {
            assert_eq!(Sequence::parse(code).unwrap().code(), "ctrl-k ctrl-s");
        }
        for code in ["ctrl-+", "ctrl--", "alt-\\", "ctrl-space", "escape", "shift-enter", "home", "delete", "ctrl-x b"]
        {
            let sequence = Sequence::parse(code).unwrap();
            assert_eq!(Sequence::parse(&sequence.code()), Some(sequence));
        }
        assert_eq!(Sequence::parse("M-x").unwrap().code(), "alt-x");
        for code in ["", "ctrl-k invalid", "f13"] {
            assert!(Sequence::parse(code).is_none(), "{code}");
        }
    }
    #[test]
    fn contexts_parse_boolean_expressions_and_rank_the_matching_branch() {
        let mut s = editor();
        s.value("vim_mode", "normal");
        assert_eq!(
            Context::parse("Editor && (mode == source || mode == markdown) && vim_mode != insert").unwrap().score(&s),
            Some(3)
        );
        assert!(Context::parse("Explorer || Editor && mode == markdown").unwrap().score(&s).is_none());
        assert_eq!(Context::parse("!Terminal && Editor").unwrap().score(&s), Some(2));
        for code in ["Editor &&", "(Editor", "Editor = source", "Editor || || Explorer", "Editor && )"] {
            assert!(Context::parse(code).is_err(), "{code}");
        }
        assert!(Context::parse("Éditor").is_ok());
    }
    #[test]
    fn specificity_wins_then_the_later_user_binding_and_null_unbinds() {
        let mut map = make_map(
            r#"[
          {"context":"Editor && mode == source", "bindings":{"ctrl-s":"save_as"}},
          {"bindings":{"ctrl-s":"open"}},
          {"context":"Editor", "bindings":{"ctrl-s":"save"}}
        ]"#,
        );
        let s = editor();
        assert_eq!(feed(&mut map, "ctrl-s", &s), Match::Command("save_as".into()));
        let mut s = s;
        s.value("mode", "markdown");
        assert_eq!(feed(&mut map, "ctrl-s", &s), Match::Command("save".into()));
        let mut map = map_from_null();
        assert_eq!(feed(&mut map, "ctrl-s", &s), Match::Consumed);
        fn map_from_null() -> Keymap {
            make_map(
                r#"[{"context":"Editor","bindings":{"ctrl-s":"save"}},{"context":"Editor","bindings":{"ctrl-s":null}}]"#,
            )
        }
    }
    #[test]
    fn chords_show_completions_finish_and_cancel_without_typing() {
        let mut map = make_map(
            r#"[{"context":"Editor","bindings":{"ctrl-k ctrl-s":"save","ctrl-k ctrl-f":"open","ctrl-x ctrl-f ctrl-s":"save_as"}}]"#,
        );
        let s = editor();
        assert_eq!(feed(&mut map, "ctrl-k", &s), Match::Consumed);
        let hint = map.hint.as_ref().unwrap();
        assert!(hint.contains("Ctrl+K") && hint.contains("Ctrl+S: save") && hint.contains("Ctrl+F: open"));
        assert_eq!(feed(&mut map, "ctrl-s", &s), Match::Command("save".into()));
        assert!(map.hint.is_none());
        feed(&mut map, "ctrl-k", &s);
        assert_eq!(feed(&mut map, "escape", &s), Match::Consumed);
        assert!(map.pending.is_empty());
        feed(&mut map, "ctrl-k", &s);
        assert_eq!(feed(&mut map, "z", &s), Match::Consumed);
        assert!(map.pending.is_empty());
        feed(&mut map, "ctrl-x", &s);
        feed(&mut map, "ctrl-f", &s);
        assert_eq!(feed(&mut map, "ctrl-s", &s), Match::Command("save_as".into()));
        feed(&mut map, "ctrl-k", &s);
        let mut explorer = State::default();
        explorer.flag("Explorer");
        assert_eq!(feed(&mut map, "ctrl-s", &explorer), Match::Consumed);
    }
    #[test]
    fn plain_bindings_cannot_consume_input_but_chord_suffixes_can() {
        let mut map = make_map(
            r#"[{"bindings":{"x":"save","ctrl-x b":"quick_open"}},{"context":"Explorer","bindings":{"r":"rename"}}]"#,
        );
        let s = editor();
        assert_eq!(feed(&mut map, "x", &s), Match::Pass);
        feed(&mut map, "ctrl-x", &s);
        assert_eq!(feed(&mut map, "b", &s), Match::Command("quick_open".into()));
        let mut explorer = State::default();
        explorer.flag("Explorer");
        assert_eq!(feed(&mut map, "r", &explorer), Match::Command("rename".into()));
        assert!(plain_context("Explorer"));
        assert!(plain_context("Editor && vim_mode == normal"));
        assert!(!plain_context("Editor"));
        assert!(!contexts_overlap("Explorer", "Editor"));
        assert!(contexts_overlap("Editor && !Notebook", "Editor"));
        assert!(contexts_overlap("Editor", "Editor && mode == source"));
    }
    #[test]
    fn base_tables_have_known_commands_and_reachable_chords() {
        for base in BASES {
            for &(context, code, command) in super::super::bases::bindings(base) {
                assert!(crate::app::keymap_command_known(command), "{base}: {command}");
                assert!(Context::parse(context).is_ok() && Sequence::parse(code).is_some());
            }
        }
        let mut map = Keymap::build("emacs", &BTreeMap::new(), Vec::new());
        let s = editor();
        assert_eq!(feed(&mut map, "ctrl-x", &s), Match::Consumed);
        assert!(map.hint.is_some());
        assert_eq!(feed(&mut map, "ctrl-s", &s), Match::Command("save".into()));
        feed(&mut map, "ctrl-x", &s);
        assert_eq!(feed(&mut map, "b", &s), Match::Command("quick_open".into()));
        assert_eq!(feed(&mut map, "ctrl-a", &s), Match::Command("move_line_start".into()));
    }
    #[test]
    fn user_and_legacy_layers_override_the_base() {
        let legacy = BTreeMap::from([("save".into(), vec!["ctrl-u ctrl-s".into()])]);
        let user = parse(r#"[{"context":"Main","bindings":{"ctrl-u ctrl-s":"save_as"}}]"#).unwrap();
        let mut map = Keymap::build("emacs", &legacy, user);
        let s = editor();
        feed(&mut map, "ctrl-u", &s);
        assert_eq!(feed(&mut map, "ctrl-s", &s), Match::Command("save_as".into()));
    }
    #[test]
    fn invalid_documents_are_rejected_and_edits_preserve_other_contexts() {
        for text in [
            "{}",
            r#"[{"context":3,"bindings":{}}]"#,
            r#"[{"bindings":{"ctrl-k":"unknown"}}]"#,
            r#"[{"bindings":{"oops":"save"}}]"#,
            r#"[{"bindings":{"ctrl-s":3}}]"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        let original =
            r#"[{"context":"Explorer","bindings":{"s":"save"}},{"context":"Editor","bindings":{"ctrl-f":"find"}}]"#;
        let old = vec![Sequence::parse("ctrl-s").unwrap()];
        let sequence = Sequence::parse("ctrl-k ctrl-s").unwrap();
        let changed = edit_document(original, "Editor", "save", &old, Some(&sequence), false).unwrap();
        let binds = parse(&changed).unwrap();
        assert_eq!(binds.len(), 4);
        assert!(binds.iter().any(|b| b.context.code == "Explorer" && b.command.as_deref() == Some("save")));
        let reset = edit_document(&changed, "Editor", "save", &old, None, true).unwrap();
        assert_eq!(parse(&reset).unwrap().len(), 2);
    }
}

#[cfg(test)]
mod defaults_regression {
    use super::*;
    #[test]
    fn lightdown_adds_no_overrides_to_existing_default_keys() {
        let mut map = Keymap::build("lightdown", &BTreeMap::new(), Vec::new());
        for state in states() {
            for action in ACTIONS {
                for code in action.keys {
                    let key = Sequence::parse(code).unwrap().0[0];
                    assert_eq!(map.feed(key, state), Match::Pass, "{} in {:?}", action.id, state);
                }
            }
        }
    }
}
