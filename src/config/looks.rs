//! Settings for how Lightdown looks: the colour theme and how it follows the
//! desktop, single colours overridden, the icon theme and the fonts.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// How the theme is chosen.
pub const THEME_MODES: [&str; 4] = ["", "system", "dark", "light"];

#[derive(Clone, Debug, PartialEq)]
pub struct Looks {
    /// The theme by name; `dark` and `light` are the built-in pair.
    pub theme: String,
    /// Empty to use `theme`; `system` to follow the desktop's colour
    /// scheme, `dark` or `light` to use `dark_theme` or `light_theme`.
    pub theme_mode: String,
    pub dark_theme: String,
    pub light_theme: String,
    /// Roles to colours, for every theme, or under a theme's name for one.
    pub theme_overrides: Value,
    /// The icon theme by name; empty for the built-in one.
    pub icon_theme: String,
    /// A fontconfig family for code; empty for the system's monospace.
    pub buffer_font_family: String,
    /// Code's size in pixels; 0 to follow the document size.
    pub buffer_font_size: f32,
    /// Rows of code as a multiple of its size.
    pub buffer_line_height: f32,
    /// OpenType features for code, as `{"liga": false, "ss01": true}`.
    pub buffer_font_features: BTreeMap<String, u16>,
    /// A fontconfig family for the interface; empty for Inter.
    pub ui_font_family: String,
}

pub const LINE_HEIGHT: f32 = 1.5;

impl Default for Looks {
    fn default() -> Looks {
        Looks {
            theme: "dark".into(),
            theme_mode: String::new(),
            dark_theme: "dark".into(),
            light_theme: "light".into(),
            theme_overrides: Value::Object(Map::new()),
            icon_theme: String::new(),
            buffer_font_family: String::new(),
            buffer_font_size: 0.0,
            buffer_line_height: LINE_HEIGHT,
            buffer_font_features: BTreeMap::new(),
            ui_font_family: String::new(),
        }
    }
}

/// An OpenType feature tag: four printable characters.
pub fn feature_tag(tag: &str) -> bool {
    tag.len() == 4 && tag.bytes().all(|b| b.is_ascii_graphic())
}

impl Looks {
    pub fn load(&mut self, v: &Value) {
        let text = |key: &str, into: &mut String| {
            if let Some(s) = v[key].as_str() {
                *into = s.trim().to_string();
            }
        };
        text("theme", &mut self.theme);
        text("dark_theme", &mut self.dark_theme);
        text("light_theme", &mut self.light_theme);
        text("icon_theme", &mut self.icon_theme);
        text("buffer_font_family", &mut self.buffer_font_family);
        text("ui_font_family", &mut self.ui_font_family);
        if let Some(mode) = v["theme_mode"].as_str().filter(|m| THEME_MODES.contains(m)) {
            self.theme_mode = mode.into();
        }
        if v["theme_overrides"].is_object() {
            self.theme_overrides = v["theme_overrides"].clone();
        }
        if let Some(n) = v["buffer_font_size"].as_f64() {
            self.buffer_font_size = if n <= 0.0 { 0.0 } else { (n as f32).clamp(6.0, 60.0) };
        }
        if let Some(n) = v["buffer_line_height"].as_f64() {
            self.buffer_line_height = (n as f32).clamp(1.0, 3.0);
        }
        if let Some(features) = v["buffer_font_features"].as_object() {
            self.buffer_font_features = features
                .iter()
                .filter(|(tag, _)| feature_tag(tag))
                .filter_map(|(tag, on)| {
                    let n = on.as_bool().map(u16::from).or_else(|| on.as_u64().map(|n| n.min(u16::MAX as u64) as u16));
                    Some((tag.clone(), n?))
                })
                .take(32)
                .collect();
        }
    }

    /// Writes these settings into the object `save` writes.
    pub fn save(&self, into: &mut Value) {
        let features: Map<String, Value> = self
            .buffer_font_features
            .iter()
            .map(|(tag, &n)| (tag.clone(), if n <= 1 { json!(n == 1) } else { json!(n) }))
            .collect();
        for (key, value) in [
            ("theme", json!(self.theme)),
            ("theme_mode", json!(self.theme_mode)),
            ("dark_theme", json!(self.dark_theme)),
            ("light_theme", json!(self.light_theme)),
            ("theme_overrides", self.theme_overrides.clone()),
            ("icon_theme", json!(self.icon_theme)),
            ("buffer_font_family", json!(self.buffer_font_family)),
            ("buffer_font_size", json!(self.buffer_font_size)),
            ("buffer_line_height", json!(self.buffer_line_height)),
            ("buffer_font_features", Value::Object(features)),
            ("ui_font_family", json!(self.ui_font_family)),
        ] {
            into[key] = value;
        }
    }

    /// The theme to show, given whether the desktop prefers light.
    pub fn theme_for(&self, desktop_light: Option<bool>) -> &str {
        match self.theme_mode.as_str() {
            "system" if desktop_light == Some(true) => &self.light_theme,
            "system" => &self.dark_theme,
            "dark" => &self.dark_theme,
            "light" => &self.light_theme,
            _ => &self.theme,
        }
    }

    /// Keeps a chosen theme: as `theme`, and for its appearance when the
    /// mode picks between a dark and a light one.
    pub fn choose(&mut self, name: &str, light: bool) {
        self.theme = name.into();
        if light {
            self.light_theme = name.into();
        } else {
            self.dark_theme = name.into();
        }
        if matches!(self.theme_mode.as_str(), "dark" | "light") {
            self.theme_mode = if light { "light" } else { "dark" }.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_load_clamp_and_round_trip() {
        let mut l = Looks::default();
        let v = json!({"theme": "Nord", "theme_mode": "system", "light_theme": "One Light", "buffer_font_size": 99,
            "buffer_line_height": 0.2, "buffer_font_features": {"calt": true, "liga": false, "cv01": 2, "toolong": true},
            "theme_overrides": {"bg": "#000000"}, "buffer_font_family": " Fira Code "});
        l.load(&v);
        assert_eq!(
            (l.theme.as_str(), l.theme_mode.as_str(), l.buffer_font_family.as_str()),
            ("Nord", "system", "Fira Code")
        );
        assert_eq!((l.buffer_font_size, l.buffer_line_height), (60.0, 1.0));
        assert_eq!(
            l.buffer_font_features.iter().map(|(k, &v)| (k.as_str(), v)).collect::<Vec<_>>(),
            [("calt", 1), ("cv01", 2), ("liga", 0)]
        );
        let mut out = json!({});
        l.save(&mut out);
        let mut again = Looks::default();
        again.load(&out);
        assert_eq!(again, l);
        l.load(&json!({"theme_mode": "sometimes"}));
        assert_eq!(l.theme_mode, "system");
    }

    #[test]
    fn the_mode_picks_the_theme_and_choosing_keeps_both_sides() {
        let mut l = Looks::default();
        assert_eq!(l.theme_for(Some(true)), "dark");
        l.theme_mode = "system".into();
        l.dark_theme = "Nord".into();
        l.light_theme = "One Light".into();
        assert_eq!(
            (l.theme_for(Some(true)), l.theme_for(Some(false)), l.theme_for(None)),
            ("One Light", "Nord", "Nord")
        );
        l.choose("Gruvbox Light", true);
        assert_eq!((l.light_theme.as_str(), l.theme_mode.as_str()), ("Gruvbox Light", "system"));
        l.theme_mode = "dark".into();
        l.choose("Solarized Light", true);
        assert_eq!((l.theme_for(None), l.theme_mode.as_str()), ("Solarized Light", "light"));
    }
}
