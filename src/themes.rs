//! Colour themes as files: the two built-in themes, the bundled ones, and
//! JSON files in `~/.config/lightdown/themes` and a project's
//! `.lightdown/themes`. A file holds one theme, or a family under `themes`:
//!
//! ```json
//! {"name": "Nord", "appearance": "dark", "colors": {"bg": "#2e3440", "fg": "#d8dee9"}}
//! ```
//!
//! Colours are named by the roles of `theme::ROLES`; a role left out falls
//! back as that table says, so a theme can be as short as `bg`, `fg` and
//! `primary`. Also here: converting Zed and VS Code themes, and asking the
//! desktop whether it prefers dark or light.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::theme::{self, Palette};

/// Where a theme comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    BuiltIn,
    Bundled,
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct ThemeDef {
    pub name: String,
    pub light: bool,
    /// The colours the theme gives, by role, already laid over its ground
    /// where they had an alpha.
    pub colors: BTreeMap<String, u32>,
    pub origin: Origin,
}

pub const DARK_NAME: &str = "Lightdown Dark";
pub const LIGHT_NAME: &str = "Lightdown Light";

static BUNDLED: &[&str] = &[
    include_str!("../assets/themes/one.json"),
    include_str!("../assets/themes/solarized.json"),
    include_str!("../assets/themes/gruvbox.json"),
    include_str!("../assets/themes/nord.json"),
    include_str!("../assets/themes/catppuccin.json"),
    include_str!("../assets/themes/tokyo-night.json"),
];

/// Reads `#rgb`, `#rrggbb` or `#rrggbbaa`, laying a colour with alpha over
/// `under`.
pub fn parse_colour(s: &str, under: u32) -> Option<u32> {
    let hex = s.trim().strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        3 => Some(((n >> 8 & 15) * 0x110000) | ((n >> 4 & 15) * 0x1100) | ((n & 15) * 0x11)),
        4 => parse_colour(&format!("#{}", hex.chars().flat_map(|c| [c, c]).collect::<String>()), under),
        6 => Some(n),
        8 => Some(theme::mix(under, n >> 8, n & 255)),
        _ => None,
    }
}

impl ThemeDef {
    fn built_in(light: bool) -> ThemeDef {
        let name = if light { LIGHT_NAME } else { DARK_NAME };
        ThemeDef { name: name.into(), light, colors: BTreeMap::new(), origin: Origin::BuiltIn }
    }

    /// One theme from its JSON object.
    fn from_value(v: &Value, origin: Origin) -> Result<ThemeDef, String> {
        let name = v["name"].as_str().filter(|n| !n.trim().is_empty()).ok_or("a theme needs a `name`")?;
        let light = match v["appearance"].as_str() {
            Some("light") => true,
            Some("dark") | None => false,
            Some(other) => return Err(format!("`appearance` of {name} is {other}; use dark or light")),
        };
        let base = if light { &theme::LIGHT } else { &theme::DARK };
        let mut colors = BTreeMap::new();
        let given = v["colors"].as_object().ok_or_else(|| format!("{name} has no `colors`"))?;
        let bg = given.get("bg").and_then(Value::as_str).and_then(|s| parse_colour(s, base.bg)).unwrap_or(base.bg);
        for (role, value) in given {
            let colour = value.as_str().and_then(|s| parse_colour(s, bg));
            let colour = colour.ok_or_else(|| format!("{name}: `{role}` is not a colour like #1e1e2e"))?;
            colors.insert(role.clone(), colour);
        }
        Ok(ThemeDef { name: name.trim().to_string(), light, colors, origin })
    }

    /// The palette, with `overrides` (settings' `theme_overrides`) on top.
    pub fn palette(&self, overrides: &Value) -> Palette {
        let mut over = BTreeMap::new();
        let bg = self.colors.get("bg").copied().unwrap_or(if self.light { theme::LIGHT.bg } else { theme::DARK.bg });
        let mut take = |m: &Map<String, Value>| {
            for (role, v) in m {
                if let Some(c) = v.as_str().and_then(|s| parse_colour(s, bg)) {
                    over.insert(role.clone(), c);
                }
            }
        };
        if let Some(all) = overrides.as_object() {
            take(all);
            if let Some(mine) = all.get(&self.name).and_then(Value::as_object) {
                take(mine);
            }
        }
        let base = if self.light { &theme::LIGHT } else { &theme::DARK };
        let built_in = self.origin == Origin::BuiltIn;
        Palette::build(self.light, &|role| {
            let own = if built_in { base.role(role) } else { self.colors.get(role).copied() };
            over.get(role).copied().or(own)
        })
    }
}

/// Every theme in one file, or the reason the file cannot be read.
pub fn parse(text: &str, origin: Origin) -> Result<Vec<ThemeDef>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    match v["themes"].as_array() {
        Some(list) => list.iter().map(|t| ThemeDef::from_value(t, origin.clone())).collect(),
        None => Ok(vec![ThemeDef::from_value(&v, origin)?]),
    }
}

/// The folders themes are read from, the user's first.
pub fn dirs(root: &Path) -> Vec<PathBuf> {
    vec![crate::config::config_dir().join("themes"), root.join(".lightdown").join("themes")]
}

/// Every theme: built-in, bundled, then the user's and the project's, a
/// later theme replacing an earlier one of the same name. Files that cannot
/// be read are named in the second list.
pub fn all(root: &Path) -> (Vec<ThemeDef>, Vec<String>) {
    let mut out = vec![ThemeDef::built_in(false), ThemeDef::built_in(true)];
    for text in BUNDLED {
        out.extend(parse(text, Origin::Bundled).expect("a bundled theme is valid"));
    }
    let mut errors = Vec::new();
    for dir in dirs(root) {
        for path in json_files(&dir) {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            match parse(&text, Origin::File(path.clone())) {
                Ok(themes) => {
                    for t in themes {
                        out.retain(|o| o.name != t.name);
                        out.push(t);
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }
    }
    (out, errors)
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .take(500)
        .collect();
    files.sort();
    files
}

/// A theme by name. `dark` and `light` name the built-in pair.
pub fn find<'a>(themes: &'a [ThemeDef], name: &str) -> Option<&'a ThemeDef> {
    let name = match name.trim() {
        "dark" | "" => DARK_NAME,
        "light" => LIGHT_NAME,
        n => n,
    };
    themes.iter().find(|t| t.name == name).or_else(|| themes.iter().find(|t| t.name.eq_ignore_ascii_case(name)))
}

/// A number that changes when any theme file is added, removed or written.
pub fn stamp(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for dir in dirs(root) {
        for path in json_files(&dir) {
            path.hash(&mut h);
            if let Ok(m) = std::fs::metadata(&path) {
                m.len().hash(&mut h);
                m.modified().ok().hash(&mut h);
            }
        }
    }
    h.finish()
}

// ---- the desktop's preference ------------------------------------------

/// Whether the desktop prefers light (`Some(true)`) or dark colours, by the
/// portal's `org.freedesktop.appearance color-scheme`; `None` when it has
/// no preference or cannot be asked.
pub fn desktop_prefers_light() -> Option<bool> {
    let ask = |program: &str, args: &[&str]| {
        let out = std::process::Command::new(program).args(args).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let busctl = [
        "--user",
        "call",
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
        "ReadOne",
        "ss",
        "org.freedesktop.appearance",
        "color-scheme",
    ];
    let gdbus = [
        "call",
        "--session",
        "--timeout",
        "2",
        "--dest",
        "org.freedesktop.portal.Desktop",
        "--object-path",
        "/org/freedesktop/portal/desktop",
        "--method",
        "org.freedesktop.portal.Settings.ReadOne",
        "org.freedesktop.appearance",
        "color-scheme",
    ];
    let said = ask("busctl", &busctl).or_else(|| ask("gdbus", &gdbus))?;
    scheme_of(&said)
}

/// The colour scheme in busctl's `v u 1` or gdbus's `(<uint32 1>,)`.
fn scheme_of(said: &str) -> Option<bool> {
    let n = said.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).last()?;
    match n {
        "1" => Some(false),
        "2" => Some(true),
        _ => None,
    }
}

// ---- importing Zed and VS Code themes ------------------------------------

/// Removes `//` and `/* */` comments and trailing commas, as VS Code's
/// JSON with comments allows.
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                while chars.peek().is_some_and(|&n| n != '\n') {
                    chars.next();
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for n in chars.by_ref() {
                    if last == '*' && n == '/' {
                        break;
                    }
                    last = n;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas, outside strings.
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == '\\' && i + 1 < bytes.len() {
                cleaned.push(c);
                cleaned.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            in_string = c != '"';
        } else if c == '"' {
            in_string = true;
        } else if c == ',' {
            let next = bytes[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                i += 1;
                continue;
            }
        }
        cleaned.push(c);
        i += 1;
    }
    cleaned
}

/// Lightdown roles and the Zed style keys they are read from, best first.
/// Syntax entries come from `style.syntax`, ANSI colours from `terminal.ansi.*`.
const ZED: &[(&str, &[&str])] = &[
    ("bg", &["editor.background", "background"]),
    ("fg", &["editor.foreground", "text"]),
    ("raised", &["elevated_surface.background", "surface.background"]),
    ("inset", &["surface.background", "panel.background"]),
    ("tint", &["element.background", "ghost_element.selected"]),
    ("bright", &["text"]),
    ("dim", &["text.muted"]),
    ("faint", &["text.placeholder", "editor.line_number"]),
    ("icon", &["icon.muted", "icon"]),
    ("quote", &["text.muted"]),
    ("line", &["border.variant", "border"]),
    ("edge", &["border"]),
    ("field", &["border.focused", "border"]),
    ("hover", &["element.hover", "ghost_element.hover"]),
    ("active", &["element.selected", "element.active", "ghost_element.selected"]),
    ("primary", &["text.accent", "border.focused"]),
    ("link", &["link_text.hover", "text.accent"]),
    ("found", &["search.match_background"]),
    ("success", &["created", "success"]),
    ("warning", &["modified", "warning"]),
    ("danger", &["deleted", "error"]),
    ("info", &["info"]),
    ("term_blue", &["terminal.ansi.blue"]),
];

/// Syntax roles and the Zed `syntax` names they are read from.
const ZED_SYNTAX: &[(&str, &[&str])] = &[
    ("keyword", &["keyword"]),
    ("string", &["string"]),
    ("number", &["number", "constant"]),
    ("function", &["function", "function.method"]),
    ("special", &["type", "constant", "variable.special"]),
    ("parameter", &["variable.parameter"]),
    ("property", &["property"]),
];

/// Lightdown roles and the VS Code workbench colours they are read from.
const VSCODE: &[(&str, &[&str])] = &[
    ("bg", &["editor.background"]),
    ("fg", &["editor.foreground", "foreground"]),
    ("raised", &["editorWidget.background", "quickInput.background", "dropdown.background"]),
    ("inset", &["textCodeBlock.background", "sideBar.background", "editorGroupHeader.tabsBackground"]),
    ("tint", &["textCodeBlock.background", "badge.background"]),
    ("dim", &["descriptionForeground", "sideBar.foreground"]),
    ("faint", &["editorLineNumber.foreground", "input.placeholderForeground"]),
    ("icon", &["icon.foreground", "editorLineNumber.foreground"]),
    ("quote", &["textBlockQuote.foreground", "descriptionForeground"]),
    ("line", &["panel.border", "editorGroup.border", "sideBar.border", "contrastBorder"]),
    ("edge", &["editorWidget.border", "widget.border", "panel.border"]),
    ("field", &["input.border", "dropdown.border", "focusBorder"]),
    ("hover", &["list.hoverBackground", "toolbar.hoverBackground"]),
    ("active", &["list.activeSelectionBackground", "list.inactiveSelectionBackground"]),
    ("primary", &["button.background", "focusBorder"]),
    ("primary_hover", &["button.hoverBackground"]),
    ("on_primary", &["button.foreground"]),
    ("selection", &["editor.selectionBackground"]),
    ("found", &["editor.findMatchHighlightBackground"]),
    ("caret", &["editorCursor.foreground"]),
    ("link", &["textLink.foreground", "textLink.activeForeground"]),
    ("success", &["gitDecoration.addedResourceForeground", "terminal.ansiGreen"]),
    ("warning", &["editorWarning.foreground", "list.warningForeground"]),
    ("danger", &["errorForeground", "editorError.foreground"]),
    ("info", &["editorInfo.foreground"]),
    ("term_blue", &["terminal.ansiBlue"]),
];

/// Syntax roles and the TextMate scopes they are read from, best first.
/// Roles past the first five are carried for themes that name them.
const SCOPES: &[(&str, &[&str])] = &[
    ("keyword", &["keyword.control", "keyword", "storage.modifier", "storage"]),
    ("string", &["string.quoted", "string"]),
    ("number", &["constant.numeric", "constant"]),
    ("function", &["entity.name.function", "support.function", "meta.function-call"]),
    ("special", &["entity.name.type", "support.type", "entity.name.class", "storage.type"]),
    ("comment", &["comment"]),
    ("type", &["entity.name.type", "support.type", "entity.name.class"]),
    ("constant", &["constant.language", "constant"]),
    ("parameter", &["variable.parameter"]),
    ("property", &["variable.other.property", "support.type.property-name", "meta.object-literal.key"]),
    ("builtin", &["support.function.builtin", "support.function", "variable.language"]),
    ("variable", &["variable.other", "variable"]),
    ("operator", &["keyword.operator"]),
    ("punctuation", &["punctuation"]),
    ("tag", &["entity.name.tag"]),
    ("attribute", &["entity.other.attribute-name"]),
];

const ANSI_ZED: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
const ANSI_VSCODE: [&str; 8] = ["Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White"];

fn hex(c: u32) -> Value {
    Value::String(format!("#{c:06x}"))
}

/// Picks colours from a flat map by `table`, laid over `bg`.
fn pick(colors: &mut Map<String, Value>, from: &dyn Fn(&str) -> Option<String>, table: &[(&str, &[&str])], bg: u32) {
    for (role, keys) in table {
        if colors.contains_key(*role) {
            continue;
        }
        if let Some(c) = keys.iter().find_map(|k| from(k).and_then(|s| parse_colour(&s, bg))) {
            colors.insert(role.to_string(), hex(c));
        }
    }
}

fn appearance_of(v: &Value, bg: u32) -> &'static str {
    match v.as_str() {
        Some("light" | "hc-light") => "light",
        Some(_) => "dark",
        None if theme::luminance(bg) > 0.4 => "light",
        None => "dark",
    }
}

fn zed_theme(t: &Value) -> Option<Value> {
    let style = &t["style"];
    let get = |k: &str| style[k].as_str().map(str::to_string);
    let bg = ["editor.background", "background"].iter().find_map(|k| get(k).and_then(|s| parse_colour(&s, 0)))?;
    let mut colors = Map::new();
    colors.insert("bg".into(), hex(bg));
    pick(&mut colors, &get, ZED, bg);
    let player = &style["players"][0];
    for (role, key) in [("caret", "cursor"), ("selection", "selection")] {
        if let Some(c) = player[key].as_str().and_then(|s| parse_colour(s, bg)) {
            colors.insert(role.into(), hex(c));
        }
    }
    let syntax = |k: &str| style["syntax"][k]["color"].as_str().map(str::to_string);
    pick(&mut colors, &syntax, ZED_SYNTAX, bg);
    for (name, entry) in style["syntax"].as_object().into_iter().flatten() {
        let role = name.replace(['.', '-'], "_");
        if let Some(c) = entry["color"].as_str().and_then(|s| parse_colour(s, bg)) {
            colors.entry(role).or_insert_with(|| hex(c));
        }
    }
    for (i, n) in ANSI_ZED.iter().enumerate() {
        for (bright, offset) in [("", 0), ("bright_", 8)] {
            if let Some(c) = get(&format!("terminal.ansi.{bright}{n}")).and_then(|s| parse_colour(&s, bg)) {
                colors.insert(theme::ANSI_NAMES[i + offset].into(), hex(c));
            }
        }
    }
    Some(json!({
        "name": t["name"].as_str().unwrap_or("Imported"),
        "appearance": appearance_of(&t["appearance"], bg),
        "colors": colors,
    }))
}

/// The foreground TextMate rules give the most specific selector of `scope`.
fn scope_colour(rules: &[Value], scope: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for rule in rules {
        let Some(fg) = rule["settings"]["foreground"].as_str() else { continue };
        let selectors: Vec<String> = match &rule["scope"] {
            Value::String(s) => s.split(',').map(|s| s.trim().to_string()).collect(),
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).collect(),
            _ => continue,
        };
        for sel in selectors {
            if sel.contains(' ') || sel.is_empty() {
                continue;
            }
            let fits = scope == sel || scope.starts_with(&format!("{sel}."));
            if fits && best.as_ref().is_none_or(|(len, _)| sel.len() > *len) {
                best = Some((sel.len(), fg.to_string()));
            }
        }
    }
    best.map(|b| b.1)
}

fn vscode_theme(v: &Value, fallback_name: &str) -> Option<Value> {
    let workbench = &v["colors"];
    let get = |k: &str| workbench[k].as_str().map(str::to_string);
    let rules: Vec<Value> = v["tokenColors"].as_array().cloned().unwrap_or_default();
    let base_fg = rules
        .iter()
        .find(|r| r["scope"].is_null())
        .and_then(|r| r["settings"]["foreground"].as_str().map(str::to_string));
    let default_bg = rules
        .iter()
        .find(|r| r["scope"].is_null())
        .and_then(|r| r["settings"]["background"].as_str().and_then(|s| parse_colour(s, 0)));
    let bg = get("editor.background").and_then(|s| parse_colour(&s, 0)).or(default_bg)?;
    let mut colors = Map::new();
    colors.insert("bg".into(), hex(bg));
    if let Some(fg) = get("editor.foreground").or(base_fg).and_then(|s| parse_colour(&s, bg)) {
        colors.insert("fg".into(), hex(fg));
    }
    pick(&mut colors, &get, VSCODE, bg);
    for (role, scopes) in SCOPES {
        if let Some(c) = scopes.iter().find_map(|s| scope_colour(&rules, s)).and_then(|s| parse_colour(&s, bg)) {
            colors.entry(role.to_string()).or_insert_with(|| hex(c));
        }
    }
    for (i, n) in ANSI_VSCODE.iter().enumerate() {
        for (bright, offset) in [("", 0), ("Bright", 8)] {
            if let Some(c) = get(&format!("terminal.ansi{bright}{n}")).and_then(|s| parse_colour(&s, bg)) {
                colors.insert(theme::ANSI_NAMES[i + offset].into(), hex(c));
            }
        }
    }
    Some(json!({
        "name": v["name"].as_str().unwrap_or(fallback_name),
        "appearance": appearance_of(&v["type"], bg),
        "colors": colors,
    }))
}

/// Converts a Zed theme family or a VS Code colour theme into a Lightdown
/// theme file, and gives the names of the themes in it. `stem` names a VS
/// Code theme that does not name itself.
pub fn import(text: &str, stem: &str) -> Result<(Value, Vec<String>), String> {
    let v: Value = serde_json::from_str(&strip_jsonc(text)).map_err(|e| format!("not JSON: {e}"))?;
    let themes: Vec<Value> = if let Some(list) =
        v["themes"].as_array().filter(|l| l.iter().any(|t| t["style"].is_object()))
    {
        list.iter().filter_map(zed_theme).collect()
    } else if v["colors"].is_object() || v["tokenColors"].is_array() {
        if v["colors"]["bg"].is_string() {
            return Err("this is a Lightdown theme already".into());
        }
        vscode_theme(&v, stem).into_iter().collect()
    } else {
        return Err("neither a Zed theme (`themes` with `style`) nor a VS Code theme (`colors`, `tokenColors`)".into());
    };
    if themes.is_empty() {
        return Err("no theme in it has an editor background".into());
    }
    let names: Vec<String> = themes.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
    let mut out = json!({
        "name": v["name"].as_str().unwrap_or(names.first().map_or(stem, |n| n.as_str())),
        "themes": themes,
    });
    if let Some(author) = v["author"].as_str() {
        out["author"] = json!(author);
    }
    Ok((out, names))
}

/// A file name for an imported theme, from its name.
pub fn file_name(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("{}.json", if slug.is_empty() { "imported" } else { &slug })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::contrast;

    #[test]
    fn colours_parse_with_and_without_alpha() {
        assert_eq!(parse_colour("#abc", 0), Some(0xaabbcc));
        assert_eq!(parse_colour("#1e1e2e", 0), Some(0x1e1e2e));
        assert_eq!(parse_colour("#ffffff80", 0), Some(0x808080));
        assert_eq!(parse_colour("#ffff", 0), Some(0xffffff));
        assert_eq!(parse_colour("1e1e2e", 0), None);
        assert_eq!(parse_colour("#12345", 0), None);
        assert_eq!(parse_colour("#zzzzzz", 0), None);
    }

    #[test]
    fn every_bundled_theme_loads_and_names_its_appearance() {
        let (themes, errors) = all(Path::new("/nonexistent"));
        assert!(errors.is_empty());
        let names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
        for want in [DARK_NAME, LIGHT_NAME, "One Dark", "One Light", "Solarized Light", "Gruvbox Dark", "Nord"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        assert!(themes.iter().filter(|t| t.origin == Origin::Bundled).count() >= 10);
        assert!(find(&themes, "Catppuccin Latte").unwrap().light);
        assert!(!find(&themes, "tokyo night").unwrap().light);
        assert_eq!(find(&themes, "dark").unwrap().name, DARK_NAME);
        assert_eq!(find(&themes, "light").unwrap().name, LIGHT_NAME);
    }

    #[test]
    fn the_built_in_themes_are_the_built_in_palettes() {
        let (themes, _) = all(Path::new("/nonexistent"));
        assert_eq!(find(&themes, DARK_NAME).unwrap().palette(&Value::Null), theme::DARK);
        assert_eq!(find(&themes, LIGHT_NAME).unwrap().palette(&Value::Null), theme::LIGHT);
    }

    /// Bundled themes keep their authors' syntax colours, which are not all
    /// as strong as Lightdown's own; the interface roles were chosen to
    /// read well. Lightdown's built-in pair meets the stricter test in
    /// `theme.rs`. Exceptions, from the built-in rule (syntax at 4.5 on
    /// every surface): syntax colours are held to 3.0 on bg, raised and
    /// inset; text on hover and active rows to 3.5; faint and icon to 3.0.
    #[test]
    fn bundled_themes_keep_text_and_focus_readable() {
        let (themes, _) = all(Path::new("/nonexistent"));
        for t in themes.iter().filter(|t| t.origin == Origin::Bundled) {
            let p = t.palette(&Value::Null);
            let n = &t.name;
            let check = |ink: u32, ground: u32, least: f64, what: &str| {
                let c = contrast(ink, ground);
                assert!(c >= least, "{n}: {what} {ink:06x} on {ground:06x} is {c:.2}, under {least}");
            };
            for ground in [p.bg, p.raised] {
                for ink in [p.fg, p.bright, p.dim, p.quote, p.link, p.success, p.warning, p.danger] {
                    check(ink, ground, 4.5, "text");
                }
                check(p.faint, ground, 3.0, "faint");
                check(p.icon, ground, 3.0, "icon");
                check(p.primary, ground, 3.0, "focus ring");
                check(p.caret, ground, 3.0, "caret");
            }
            for ground in [p.inset, p.hover, p.active] {
                for ink in [p.fg, p.bright, p.dim, p.link, p.success, p.warning, p.danger] {
                    check(ink, ground, 3.5, "text on a row");
                }
            }
            for ground in [p.bg, p.raised, p.inset] {
                for ink in [p.keyword, p.string, p.number, p.function, p.special] {
                    check(ink, ground, 3.0, "syntax");
                }
            }
            for ground in [p.selection, p.found, p.tint] {
                check(p.fg, ground, 4.5, "text in a selection");
            }
            check(p.on_primary, p.primary, 4.5, "button text");
        }
    }

    #[test]
    fn left_out_roles_fall_back_and_overrides_win() {
        let t = &parse(r##"{"name": "Mini", "appearance": "light", "colors": {"bg": "#ffffff", "fg": "#000000", "primary": "#0000ff", "keyword": "#880088"}}"##, Origin::Bundled).unwrap()[0];
        let p = t.palette(&Value::Null);
        assert_eq!((p.bg, p.fg, p.bright, p.caret, p.link), (0xffffff, 0, 0, 0xff, 0xff));
        assert_eq!(p.special, 0x880088, "special falls back to keyword");
        assert_eq!(p.ansi[5], 0x880088, "ANSI magenta to keyword");
        assert_eq!(p.on_primary, 0xffffff);
        assert_eq!(p.success, theme::LIGHT.success, "state colours from the built-in light theme");
        let over = json!({"fg": "#111111", "Mini": {"keyword": "#ff0000"}, "Other": {"bg": "#000000"}});
        let p = t.palette(&over);
        assert_eq!((p.fg, p.keyword, p.bg, p.special), (0x111111, 0xff0000, 0xffffff, 0xff0000));
        assert!(parse(r#"{"name": "X", "appearance": "dim", "colors": {}}"#, Origin::Bundled).is_err());
        assert!(parse(r#"{"name": "X", "colors": {"bg": "red"}}"#, Origin::Bundled).is_err());
        // Roles a newer Lightdown reads are kept, not refused.
        assert!(parse(r##"{"name": "X", "colors": {"parameter": "#ff0000"}}"##, Origin::Bundled).is_ok());
    }

    #[test]
    fn every_fallback_names_a_role_above_it() {
        for (i, r) in theme::ROLES.iter().enumerate() {
            let names: Vec<&str> = match r.fallback {
                theme::Fallback::Base => vec![],
                theme::Fallback::Role(a) | theme::Fallback::On(a) => vec![a],
                theme::Fallback::Mix(a, b, _) => vec![a, b],
            };
            for n in names {
                let at = theme::ROLES.iter().position(|o| o.name == n);
                assert!(at.is_some_and(|at| at < i), "{} falls back to {n}, which is not above it", r.name);
            }
        }
    }

    #[test]
    fn user_and_project_files_are_read_and_replace_by_name() {
        let root = std::env::temp_dir().join(format!("ld-themes-{}", std::process::id()));
        let dir = root.join(".lightdown/themes");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nord.json"), r##"{"name": "Nord", "colors": {"bg": "#000000"}}"##).unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let before = stamp(&root);
        let (themes, errors) = all(&root);
        assert_eq!(themes.iter().filter(|t| t.name == "Nord").count(), 1);
        assert_eq!(find(&themes, "Nord").unwrap().colors["bg"], 0);
        assert!(errors.len() == 1 && errors[0].contains("broken.json"));
        std::fs::write(dir.join("mine.json"), r##"{"name": "Mine", "colors": {}}"##).unwrap();
        assert_ne!(stamp(&root), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_portal_answer_is_read_from_busctl_and_gdbus() {
        assert_eq!(scheme_of("v u 1\n"), Some(false));
        assert_eq!(scheme_of("v u 2\n"), Some(true));
        assert_eq!(scheme_of("(<uint32 2>,)\n"), Some(true));
        assert_eq!(scheme_of("(<<uint32 0>>,)"), None);
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_are_removed() {
        let text = "{\n // a comment\n \"a\": \"http://x\", /* b */ \"c\": [1, 2,],\n}";
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(v, json!({"a": "http://x", "c": [1, 2]}));
    }

    #[test]
    fn a_zed_theme_family_is_converted() {
        let zed = r##"{"name": "Fam", "author": "A", "themes": [{"name": "Fam Dark", "appearance": "dark", "style": {
            "editor.background": "#101010", "text": "#e0e0e0", "text.muted": "#a0a0a0", "border": "#303030",
            "element.hover": "#ffffff10", "text.accent": "#5599ff", "error": "#ff5555",
            "terminal.ansi.red": "#ee0000", "terminal.ansi.bright_cyan": "#00eeee",
            "players": [{"cursor": "#ffcc00", "selection": "#5599ff40"}],
            "syntax": {"keyword": {"color": "#cc88ff"}, "string": {"color": "#88cc88"}, "variable.parameter": {"color": "#ffaa77"}}
        }}, {"name": "Fam Light", "appearance": "light", "style": {"background": "#fafafa", "text": "#202020"}}]}"##;
        let (file, names) = import(zed, "fam").unwrap();
        assert_eq!(names, ["Fam Dark", "Fam Light"]);
        let themes = parse(&file.to_string(), Origin::Bundled).unwrap();
        let p = themes[0].palette(&Value::Null);
        assert_eq!(
            (p.bg, p.fg, p.dim, p.keyword, p.string, p.caret, p.danger),
            (0x101010, 0xe0e0e0, 0xa0a0a0, 0xcc88ff, 0x88cc88, 0xffcc00, 0xff5555)
        );
        assert_eq!(p.hover, theme::mix(0x101010, 0xffffff, 0x10));
        assert_eq!(p.selection, theme::mix(0x101010, 0x5599ff, 0x40));
        assert_eq!((p.ansi[1], p.ansi[14]), (0xee0000, 0x00eeee));
        assert_eq!(themes[0].colors["variable_parameter"], 0xffaa77);
        assert!(themes[1].light);
        assert_eq!(file["author"], "A");
    }

    #[test]
    fn a_vscode_theme_is_converted_from_workbench_colours_and_token_scopes() {
        let vs = r##"{
            // VS Code allows comments
            "name": "Code Theme", "type": "light",
            "colors": {"editor.background": "#ffffff", "editor.foreground": "#333333", "button.background": "#0066bf",
                       "editorCursor.foreground": "#000000", "terminal.ansiBrightBlack": "#666666",},
            "tokenColors": [
                {"scope": ["keyword", "storage"], "settings": {"foreground": "#0000ff"}},
                {"scope": "keyword.control", "settings": {"foreground": "#af00db"}},
                {"scope": "string, string.quoted", "settings": {"foreground": "#a31515"}},
                {"scope": "constant.numeric", "settings": {"foreground": "#098658"}},
                {"scope": "entity.name.function", "settings": {"foreground": "#795e26"}},
                {"scope": "source.python keyword", "settings": {"foreground": "#ff0000"}}
            ]
        }"##;
        let (file, names) = import(vs, "code").unwrap();
        assert_eq!(names, ["Code Theme"]);
        let t = &parse(&file.to_string(), Origin::Bundled).unwrap()[0];
        assert!(t.light);
        let p = t.palette(&Value::Null);
        assert_eq!(
            (p.bg, p.fg, p.primary, p.keyword, p.string, p.number, p.function),
            (0xffffff, 0x333333, 0x0066bf, 0xaf00db, 0xa31515, 0x098658, 0x795e26)
        );
        assert_eq!(p.ansi[8], 0x666666);
        assert!(import("{\"x\": 1}", "x").is_err());
        assert!(import(include_str!("../assets/themes/nord.json"), "nord").is_err());
        assert_eq!(file_name("Tokyo Night (Storm)"), "tokyo-night-storm.json");
    }
}
