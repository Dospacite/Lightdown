//! Built-in schemas for Lightdown's JSON config files, matched by path. They
//! give completion of keys and values, hover descriptions, and inline
//! errors while a config file is edited. Validation only reads the text.
//!
//! Adding a config file is one entry in `FILES`, with its schema in
//! `schemas/`. Adding a key is one entry in that schema's `properties`.
//! The schemas are a small subset of JSON Schema: `type` (a name or a
//! list), `description`, `properties`, `additionalProperties` (a schema,
//! or `true` to allow any key), `required`, `items`, `enum`, `minimum`,
//! `maximum`, `minItems`, `maxItems`, `maxProperties`, `minLength`,
//! `pattern` (with `patternMessage` to say it plainly), `propertyNames` and
//! `anyOf`. A few extensions match what the loaders check:
//! - `"x-shortcuts": true` fills an object's properties with the shortcut
//!   ids of `src/keys.rs`.
//! - `"x-unique": "name"` on an array rejects two items with the same key.
//! - `"x-one-of": ["command", "url"]` on an object needs one of the keys.
//! - `"x-snippet": true` on a string checks it as a snippet body.
//! - `"examples"` on a value are offered as completions, without limiting it.
//! - `"x-lenient": true` at the top says the loader ignores bad values
//!   instead of refusing the file, so its marks are warnings.

mod json;

use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;

/// Where a config file lives.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Place {
    /// In the user's config folder, `~/.config/lightdown`.
    User,
    /// In a project's `.lightdown` folder.
    Project,
    /// Anywhere, known by the end of its name.
    Anywhere,
}

pub struct ConfigFile {
    pub name: &'static str,
    pub place: Place,
    /// The schema file: `{"description", "schema", "example"}`. The example
    /// is what **Preferences: Create project config file** writes.
    source: &'static str,
}

const fn file(name: &'static str, place: Place, source: &'static str) -> ConfigFile {
    ConfigFile { name, place, source }
}

/// Every config file with a schema.
pub static FILES: &[ConfigFile] = &[
    file("settings.json", Place::User, include_str!("schemas/settings.json")),
    file("settings.json", Place::Project, include_str!("schemas/project_settings.json")),
    file("keymap.json", Place::User, include_str!("schemas/keymap.json")),
    file("snippets.json", Place::User, include_str!("schemas/snippets.json")),
    file("snippets.json", Place::Project, include_str!("schemas/snippets.json")),
    file("lsp.json", Place::Project, include_str!("schemas/lsp.json")),
    file("run.json", Place::Project, include_str!("schemas/run.json")),
    file("debug.json", Place::Project, include_str!("schemas/debug.json")),
    file("commands.json", Place::Project, include_str!("schemas/commands.json")),
    file("mcp.json", Place::Project, include_str!("schemas/mcp.json")),
    file("remote.json", Place::Project, include_str!("schemas/remote.json")),
    file("grammar.json", Place::Project, include_str!("schemas/grammar.json")),
    file("tasks.json", Place::Project, include_str!("schemas/tasks.json")),
    file("container.json", Place::Project, include_str!("schemas/container.json")),
    file("*.lightdown-workspace", Place::Anywhere, include_str!("schemas/workspace.json")),
];

impl ConfigFile {
    /// The parsed schema file, read once.
    fn parsed(&self) -> &'static Value {
        static PARSED: OnceLock<Vec<Value>> = OnceLock::new();
        let all = PARSED.get_or_init(|| {
            FILES.iter().map(|f| serde_json::from_str(f.source).expect("a built-in config schema is valid JSON")).collect()
        });
        let i = FILES.iter().position(|f| std::ptr::eq(f, self)).unwrap_or(0);
        &all[i]
    }

    pub fn schema(&self) -> Value {
        let mut s = self.parsed()["schema"].clone();
        inline_refs(&mut s, &self.parsed()["definitions"], 0);
        fill_shortcuts(&mut s);
        s
    }

    /// What the file is for, in a line.
    pub fn description(&self) -> &'static str {
        self.parsed()["description"].as_str().unwrap_or("")
    }

    /// The example, as the file should be written.
    pub fn example(&self) -> String {
        let mut text = serde_json::to_string_pretty(&self.parsed()["example"]).unwrap_or_default();
        text.push('\n');
        text
    }
}

/// Puts each `{"$ref": "name"}` in place from the file's `definitions`.
fn inline_refs(s: &mut Value, defs: &Value, depth: usize) {
    if let Some(name) = s["$ref"].as_str() {
        *s = defs[name].clone();
    }
    if depth > 16 {
        return;
    }
    match s {
        Value::Object(m) => m.values_mut().for_each(|v| inline_refs(v, defs, depth + 1)),
        Value::Array(a) => a.iter_mut().for_each(|v| inline_refs(v, defs, depth + 1)),
        _ => {}
    }
}

/// Fills `x-shortcuts` objects with the shortcut ids and their names.
fn fill_shortcuts(s: &mut Value) {
    if s["x-shortcuts"] == true {
        let item = s["additionalProperties"].clone();
        s["properties"] = Value::Object(
            crate::keys::ACTIONS
                .iter()
                .map(|a| {
                    let mut p = item.clone();
                    p["description"] = Value::String(format!("{}. Default: {}", a.name, a.keys.join(", ")));
                    (a.id.to_string(), p)
                })
                .collect(),
        );
        // Ids not in the list are ignored by the keymap, so they warn.
        if let Some(m) = s.as_object_mut() {
            m.remove("additionalProperties");
        }
    }
    match s {
        Value::Object(m) => m.values_mut().for_each(fill_shortcuts),
        Value::Array(a) => a.iter_mut().for_each(fill_shortcuts),
        _ => {}
    }
}

/// The config file at `path`, if it is one: by its name, in the user's
/// config folder or in a `.lightdown` folder.
pub fn file_for(path: &Path) -> Option<&'static ConfigFile> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".lightdown-workspace") {
        return FILES.iter().find(|f| f.place == Place::Anywhere);
    }
    let parent = path.parent()?;
    let place = if parent == crate::config::config_dir() {
        Place::User
    } else if parent.file_name()? == ".lightdown" {
        Place::Project
    } else {
        return None;
    };
    FILES.iter().find(|f| f.name == name && f.place == place)
}

/// The schema of the config file at `path`.
pub fn schema(path: &Path) -> Option<Value> {
    file_for(path).map(ConfigFile::schema)
}

/// A problem found in a config file: where, how bad, and what.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub warning: bool,
    pub message: String,
}

/// Files larger than this are not checked, so typing never waits.
const LARGEST: usize = 256 * 1024;
const MOST_MARKS: usize = 200;

/// Checks `text` against `schema`: its syntax first, then types, ranges,
/// enums, required and unknown keys.
pub fn validate(text: &str, schema: &Value) -> Vec<Mark> {
    if text.len() > LARGEST {
        return Vec::new();
    }
    if let Err(e) = serde_json::from_str::<Value>(text) {
        let start: usize = text.split_inclusive('\n').take(e.line().saturating_sub(1)).map(str::len).sum();
        let at = crate::editor::floor(text, (start + e.column().saturating_sub(1)).min(text.len()));
        let end = text[at..].chars().next().map_or(at, |c| at + c.len_utf8());
        let message = e.to_string();
        let message = message.split(" at line ").next().unwrap_or(&message);
        let mut m = message.to_string();
        if let Some(first) = m.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        return vec![Mark { start: at, end, warning: false, message: format!("JSON: {m}") }];
    }
    let Some(root) = json::parse(text) else { return Vec::new() };
    let mut out = Vec::new();
    check(&root, schema, "", None, &mut out, text);
    if schema["x-keymap"] == true && out.iter().all(|m| m.warning) {
        if let Err(message) = crate::keys::contextual::parse(text) {
            out.push(Mark { start: 0, end: text.len().min(1), warning: false, message });
        }
    }
    out.truncate(MOST_MARKS);
    if schema["x-lenient"] == true {
        for m in out.iter_mut().filter(|m| !m.warning) {
            m.warning = true;
            m.message += "; Lightdown ignores it, or clamps a number into range";
        }
    }
    out
}

/// Whether `s` matches a schema's pattern; a bad pattern matches anything.
fn pattern_ok(s: &str, schema: &Value) -> bool {
    let Some(p) = schema["pattern"].as_str() else { return true };
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<regex::Regex>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    let re = cache.entry(p.to_string()).or_insert_with(|| regex::Regex::new(p).ok());
    re.as_ref().is_none_or(|re| re.is_match(s))
}

fn pattern_message(label: &str, schema: &Value) -> String {
    match schema["patternMessage"].as_str() {
        Some(m) => format!("{label}: {m}"),
        None => format!("{label} does not match `{}`", schema["pattern"].as_str().unwrap_or("")),
    }
}

/// What a value is expected to be, as a phrase.
fn expected(s: &Value) -> String {
    let one = |t: &str| match t {
        "object" => "an object",
        "array" => "an array",
        "string" => "a string",
        "boolean" => "true or false",
        "integer" => "a whole number",
        "number" => "a number",
        "null" => "null",
        _ => "a value",
    };
    if let Some(list) = s["anyOf"].as_array() {
        let mut parts: Vec<String> = list.iter().map(expected).collect();
        parts.dedup();
        return join_or(&parts);
    }
    match &s["type"] {
        Value::String(t) => one(t).into(),
        Value::Array(ts) => join_or(&ts.iter().filter_map(Value::as_str).map(|t| one(t).to_string()).collect::<Vec<_>>()),
        _ => "a value".into(),
    }
}

fn join_or(parts: &[String]) -> String {
    match parts {
        [] => "a value".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

fn type_ok(node: &json::Node, s: &Value) -> bool {
    let ok = |t: &str| match (t, node.type_name()) {
        ("number", "integer") => true,
        (t, n) => t == n,
    };
    match &s["type"] {
        Value::String(t) => ok(t),
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).any(ok),
        _ => true,
    }
}

/// Where a mark about a value goes: the value itself, but only its first
/// line, so an object's mark does not cover the whole file.
fn value_span(node: &json::Node, text: &str) -> (usize, usize) {
    let (a, b) = node.span;
    let b = text[a..b].find('\n').map_or(b, |n| a + n);
    (a, b.max(a + 1).min(text.len()))
}

fn check(node: &json::Node, s: &Value, name: &str, key: Option<(usize, usize)>, out: &mut Vec<Mark>, text: &str) {
    if out.len() >= MOST_MARKS {
        return;
    }
    let (a, b) = value_span(node, text);
    let error = |out: &mut Vec<Mark>, message: String| out.push(Mark { start: a, end: b, warning: false, message });
    let label = if name.is_empty() { "The file".to_string() } else { format!("`{name}`") };
    if let Some(list) = s["anyOf"].as_array() {
        // The first branch of the right type decides; else say what fits.
        match list.iter().find(|branch| type_ok(node, branch)) {
            Some(branch) => check(node, branch, name, key, out, text),
            None => error(out, format!("{label} must be {}", expected(s))),
        }
        return;
    }
    if !type_ok(node, s) {
        return error(out, format!("{label} must be {}", expected(s)));
    }
    if let Some(choices) = s["enum"].as_array() {
        let v = node.value();
        let same = |c: &Value| match (c.as_f64(), v.as_f64()) {
            (Some(x), Some(y)) => x == y,
            _ => *c == v,
        };
        if !choices.iter().any(same) {
            let list: Vec<String> = choices.iter().map(|c| c.to_string()).collect();
            error(out, format!("{label} must be one of {}", list.join(", ")));
        }
    }
    match &node.kind {
        json::Kind::Num(n, _) => {
            if let Some(min) = s["minimum"].as_f64().filter(|m| n < m) {
                error(out, format!("{label} must be at least {min}"));
            }
            if let Some(max) = s["maximum"].as_f64().filter(|m| n > m) {
                error(out, format!("{label} must be at most {max}"));
            }
        }
        json::Kind::Str(v) => {
            if s["minLength"].as_u64().is_some_and(|n| (v.chars().count() as u64) < n) {
                error(out, format!("{label} must not be empty"));
            } else if !pattern_ok(v, s) {
                error(out, pattern_message(&label, s));
            }
            if s["x-key-sequence"] == true && crate::keys::contextual::Sequence::parse(v).is_none() {
                error(out, format!("{label} must be a key sequence such as ctrl-k ctrl-s"));
            }
            if s["x-snippet"] == true {
                if let Err(e) = crate::snippets::expand(&crate::snippets::variables(v, &[])) {
                    error(out, format!("{label}: {e}"));
                }
            }
        }
        json::Kind::Array(items) => {
            if s["minItems"].as_u64().is_some_and(|n| (items.len() as u64) < n) {
                error(out, format!("{label} needs at least {} item{}", s["minItems"], if s["minItems"] == 1 { "" } else { "s" }));
            }
            if s["maxItems"].as_u64().is_some_and(|n| (items.len() as u64) > n) {
                error(out, format!("{label} may hold at most {} items", s["maxItems"]));
            }
            if let Some(field) = s["x-unique"].as_str() {
                let mut seen = std::collections::HashSet::new();
                for item in items {
                    let json::Kind::Object(m) = &item.kind else { continue };
                    if let Some((_, span, v)) = m.iter().find(|(k, _, _)| k == field) {
                        let v = v.value().to_string();
                        if !seen.insert(v.clone()) {
                            out.push(Mark { start: span.0, end: span.1, warning: false, message: format!("Two items have {field} {v}") });
                        }
                    }
                }
            }
            if s["items"].is_object() {
                for (i, item) in items.iter().enumerate() {
                    let child = if name.is_empty() { format!("[{i}]") } else { format!("{name}[{i}]") };
                    check(item, &s["items"], &child, None, out, text);
                }
            }
        }
        json::Kind::Object(members) => {
            let props = s["properties"].as_object();
            let extra = &s["additionalProperties"];
            if s["maxProperties"].as_u64().is_some_and(|n| (members.len() as u64) > n) {
                error(out, format!("{label} may hold at most {} entries", s["maxProperties"]));
            }
            if let Some(keys) = s["x-one-of"].as_array().filter(|_| !members.iter().any(|(k, _, v)| k == "disabled" && matches!(v.kind, json::Kind::Bool(true)))) {
                let names: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
                if !members.iter().any(|(k, _, _)| names.contains(&k.as_str())) {
                    let (ka, kb) = key.unwrap_or((a, (a + 1).min(text.len())));
                    let list: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
                    out.push(Mark { start: ka, end: kb, warning: false, message: format!("{label} needs {}", join_or(&list)) });
                }
            }
            if s["propertyNames"].is_object() {
                let names = &s["propertyNames"];
                for (k, span, _) in members {
                    if !pattern_ok(k, names) {
                        out.push(Mark { start: span.0, end: span.1, warning: false, message: pattern_message(&format!("`{k}`"), names) });
                    }
                }
            }
            for required in s["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if !members.iter().any(|(k, _, _)| k == required) {
                    let (ka, kb) = key.unwrap_or((a, (a + 1).min(text.len())));
                    out.push(Mark { start: ka, end: kb, warning: false, message: format!("{label} needs `{required}`") });
                }
            }
            let mut seen = std::collections::HashSet::new();
            for (k, span, v) in members {
                let child = if name.is_empty() { k.clone() } else { format!("{name}.{k}") };
                if !seen.insert(k.as_str()) {
                    out.push(Mark { start: span.0, end: span.1, warning: true, message: format!("`{child}` is repeated; the last one counts") });
                }
                match (props.and_then(|p| p.get(k)), extra) {
                    (Some(p), _) => check(v, p, &child, Some(*span), out, text),
                    (None, Value::Object(_)) => check(v, extra, &child, Some(*span), out, text),
                    (None, Value::Bool(true)) => {}
                    (None, _) if props.is_some() => {
                        let near = props.and_then(|p| closest(k, p.keys()));
                        let hint = near.map_or(String::new(), |n| format!("; did you mean `{n}`?"));
                        out.push(Mark { start: span.0, end: span.1, warning: true, message: format!("Unknown key `{child}`, ignored{hint}") });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// The known key closest in spelling to `key`, if it is close.
fn closest<'a>(key: &str, known: impl Iterator<Item = &'a String>) -> Option<&'a String> {
    let d = |a: &str, b: &str| {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut prev = row[0];
            row[0] = i;
            for j in 1..=b.len() {
                let cur = row[j];
                row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1]));
                prev = cur;
            }
        }
        row[b.len()]
    };
    known.map(|k| (d(key, k), k)).filter(|(n, _)| *n <= 2.max(key.len() / 4)).min_by_key(|(n, _)| *n).map(|(_, k)| k)
}

/// A step from a value into one inside it.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Key(String),
    Index,
}

/// What the text before the cursor is in the middle of typing.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    /// The way from the top to the object or array the cursor is in.
    pub path: Vec<Step>,
    /// A key of that object, or else a value: for an object, the value of
    /// `key`; for an array, an item.
    pub on_key: bool,
    pub key: Option<String>,
    /// Where the word being typed starts, and whether it is inside quotes.
    pub start: usize,
    pub quoted: bool,
    /// The keys already written in the object, before the cursor.
    pub present: Vec<String>,
}

/// Works out what is being typed at `cur`, reading only the text before
/// it, which need not be valid JSON yet.
pub fn spot(text: &str, cur: usize) -> Option<Spot> {
    if cur > LARGEST {
        return None;
    }
    struct Frame {
        object: bool,
        key: Option<String>,
        expect_key: bool,
        present: Vec<String>,
    }
    let mut frames: Vec<Frame> = Vec::new();
    let mut path = Vec::new();
    let before = &text[..cur];
    let mut it = before.char_indices().peekable();
    let mut open: Option<usize> = None;
    while let Some((i, c)) = it.next() {
        match c {
            '"' => {
                let start = i + 1;
                let mut end = None;
                while let Some((j, d)) = it.next() {
                    if d == '\\' {
                        it.next();
                    } else if d == '"' {
                        end = Some(j);
                        break;
                    }
                }
                let Some(end) = end else {
                    open = Some(start);
                    break;
                };
                let s: String = serde_json::from_str(&before[start - 1..=end]).unwrap_or_default();
                if let Some(f) = frames.last_mut().filter(|f| f.object && f.expect_key) {
                    f.present.push(s.clone());
                    f.key = Some(s);
                    f.expect_key = false;
                }
            }
            '{' | '[' => {
                if let Some(f) = frames.last() {
                    path.push(if f.object { Step::Key(f.key.clone().unwrap_or_default()) } else { Step::Index });
                }
                frames.push(Frame { object: c == '{', key: None, expect_key: c == '{', present: Vec::new() });
            }
            '}' | ']' => {
                frames.pop();
                path.pop();
            }
            ',' => {
                if let Some(f) = frames.last_mut().filter(|f| f.object) {
                    f.expect_key = true;
                    f.key = None;
                }
            }
            _ => {}
        }
    }
    let f = frames.last()?;
    let (start, quoted) = match open {
        Some(s) => (s, true),
        None => {
            let word = before.len() - before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.').len();
            (cur - word, false)
        }
    };
    Some(Spot { path, on_key: f.object && f.expect_key, key: f.key.clone(), start, quoted, present: f.present.clone() })
}

/// The schema for the value one step inside `s`.
fn step<'a>(s: &'a Value, step: &Step) -> Option<&'a Value> {
    if let Some(list) = s["anyOf"].as_array() {
        return list.iter().find_map(|b| step_one(b, step));
    }
    step_one(s, step)
}

fn step_one<'a>(s: &'a Value, step: &Step) -> Option<&'a Value> {
    match step {
        Step::Key(k) => s["properties"].get(k).or_else(|| s.get("additionalProperties").filter(|a| a.is_object())),
        Step::Index => s.get("items").filter(|i| i.is_object()),
    }
}

/// The schema at the end of a path.
pub fn at<'a>(s: &'a Value, path: &[Step]) -> Option<&'a Value> {
    path.iter().try_fold(s, |s, p| step(s, p))
}

/// A completion: what is shown, what is inserted, and its description.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    pub insert: String,
    pub detail: String,
}

/// The keys of an object schema, merging the branches of `anyOf`.
fn keys_of(s: &Value) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    let mut add = |s: &Value| {
        for (k, v) in s["properties"].as_object().into_iter().flatten() {
            if !out.iter().any(|(o, _)| o == k) {
                out.push((k.clone(), v.clone()));
            }
        }
    };
    add(s);
    s["anyOf"].as_array().into_iter().flatten().for_each(&mut add);
    out
}

/// A short description for a list: the description's first sentence, or
/// what the value must be.
fn detail(s: &Value) -> String {
    match s["description"].as_str() {
        Some(d) => d.split(". ").next().unwrap_or(d).trim_end_matches('.').to_string(),
        None => expected(s),
    }
}

/// What can be typed at the spot: keys not yet written, or the values an
/// enum or a boolean allows.
pub fn complete(schema: &Value, spot: &Spot, prefix: &str) -> Vec<Item> {
    let Some(here) = at(schema, &spot.path) else { return Vec::new() };
    let wrap = |s: &str| if spot.quoted { s.to_string() } else { format!("\"{s}\"") };
    if spot.on_key {
        let mut items: Vec<Item> = keys_of(here)
            .into_iter()
            .filter(|(k, _)| k.starts_with(prefix) && !spot.present.iter().any(|p| p == k && p != prefix))
            .map(|(k, s)| Item { insert: wrap(&k), detail: detail(&s), label: k })
            .collect();
        items.sort_by(|a, b| a.label.cmp(&b.label));
        return items;
    }
    let value = match (&spot.key, here["anyOf"].is_array() || here["properties"].is_object() || here["additionalProperties"].is_object()) {
        (Some(k), true) => step(here, &Step::Key(k.clone())),
        _ if !spot.path.is_empty() || spot.key.is_none() => step(here, &Step::Index).or(Some(here)),
        _ => None,
    };
    let Some(value) = value else { return Vec::new() };
    let mut values: Vec<Value> = Vec::new();
    let mut gather = |s: &Value| {
        values.extend(s["enum"].as_array().cloned().unwrap_or_default());
        values.extend(s["examples"].as_array().cloned().unwrap_or_default());
        let types: Vec<&str> = match &s["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if types.contains(&"boolean") && s["enum"].is_null() {
            values.extend([Value::Bool(true), Value::Bool(false)]);
        }
    };
    gather(value);
    value["anyOf"].as_array().into_iter().flatten().for_each(&mut gather);
    let d = detail(value);
    values
        .into_iter()
        .filter_map(|v| {
            let label = v.as_str().map_or_else(|| v.to_string(), str::to_string);
            if !label.starts_with(prefix) {
                return None;
            }
            let insert = if v.is_string() { wrap(&label) } else { label.clone() };
            Some(Item { label, insert, detail: d.clone() })
        })
        .collect()
}

/// The key under `at`, and its schema: for hover descriptions.
pub fn key_at(schema: &Value, text: &str, at: usize) -> Option<(String, Value)> {
    // The key's string: the quotes around `at` on its line.
    let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |n| at + n);
    let line = &text[line_start..line_end];
    let mut quotes = Vec::new();
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            quotes.push(line_start + i);
        }
    }
    let pair = quotes.chunks(2).find(|p| p.len() == 2 && p[0] < at && at <= p[1] + 1)?;
    let (a, b) = (pair[0], pair[1]);
    // A key is followed by a colon.
    if !text[b + 1..].trim_start().starts_with(':') {
        return None;
    }
    let s = spot(text, a + 1)?;
    let key: String = serde_json::from_str(&text[a..=b]).ok()?;
    let here = crate::config_schema::at(schema, &s.path)?;
    let found = step(here, &Step::Key(key.clone()))?.clone();
    Some((key, found))
}

/// A description of a key for hover: what it does and what it takes.
pub fn describe(s: &Value) -> String {
    let mut out = s["description"].as_str().unwrap_or("").to_string();
    let mut add = |line: String| {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&line);
    };
    if let Some(e) = s["enum"].as_array() {
        add(format!("One of: {}", e.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")));
    } else {
        let mut t = format!("Takes {}", expected(s));
        match (s["minimum"].as_f64(), s["maximum"].as_f64()) {
            (Some(a), Some(b)) => t += &format!(", from {a} to {b}"),
            (Some(a), None) => t += &format!(", at least {a}"),
            (None, Some(b)) => t += &format!(", at most {b}"),
            _ => {}
        }
        add(t);
    }
    if let Some(d) = s.get("default") {
        add(format!("Default: {d}"));
    }
    out
}

#[cfg(test)]
mod tests;
