//! Themes, icon themes and fonts from the palette: choosing them with a
//! live preview, following the desktop's dark or light preference,
//! reloading edited theme files and settings, and importing Zed and VS Code
//! themes.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use super::*;
use crate::config::Looks;
use crate::icon_theme::{self, IconTheme};
use crate::icons::Icon;
use crate::themes::{self, ThemeDef};

/// How often the desktop's preference, theme files and settings are looked at.
const CHECK: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct State {
    themes: Vec<ThemeDef>,
    stamp: Option<u64>,
    /// The project the themes were read for.
    root: PathBuf,
    checked: Option<Instant>,
    /// The desktop prefers light, by the portal, once asked.
    desktop: Option<bool>,
    asking: Option<mpsc::Receiver<Option<bool>>>,
    /// The settings file as last read, to see it written by hand.
    settings_time: Option<SystemTime>,
    /// The looks and document size last applied to the fonts.
    fonts_applied: Option<(Looks, u32)>,
    ui_family: String,
    /// While a list is open: what it previews, and what to go back to.
    preview: Option<(String, String)>,
    /// The theme in use and whether it is light.
    pub shown: String,
}

/// What a looks list chooses, by the prefix of its entries' keys.
fn split(key: &str) -> (&str, &str) {
    key.split_once(':').unwrap_or(("", key))
}

impl App {
    /// Reads the themes and puts the settings' looks in place.
    pub(super) fn looks_init(&mut self) {
        self.looks.settings_time = settings_time();
        self.reload_themes();
        self.apply_fonts();
        self.apply_icon_theme(&self.cfg.looks.icon_theme.clone());
    }

    fn reload_themes(&mut self) {
        let (list, errors) = themes::all(&self.root);
        self.looks.themes = list;
        self.looks.stamp = Some(themes::stamp(&self.root));
        self.looks.root = self.root.clone();
        if let Some(e) = errors.first() {
            self.message = Some(format!("Theme not loaded: {e}"));
        }
        self.apply_theme();
    }

    /// Shows the theme the settings choose, by the mode and the desktop.
    pub(super) fn apply_theme(&mut self) {
        let name = self.cfg.looks.theme_for(self.looks.desktop).to_string();
        let found = themes::find(&self.looks.themes, &name).cloned();
        let theme = match found {
            Some(t) => t,
            None => {
                self.message = Some(format!("No theme named {name}; showing {}", themes::DARK_NAME));
                themes::find(&self.looks.themes, "dark").cloned().expect("the built-in theme")
            }
        };
        self.show_theme(&theme);
    }

    fn show_theme(&mut self, t: &ThemeDef) {
        let palette = t.palette(&self.cfg.looks.theme_overrides);
        let same = self.looks.shown == t.name && theme::palette() == &palette;
        theme::set_palette(&palette, t.light);
        self.cfg.light = t.light;
        self.looks.shown = t.name.clone();
        if !same {
            self.pics.borrow_mut().forget();
            self.redraw = true;
        }
    }

    /// Puts the settings' fonts in place, when they or the document size
    /// have changed since last time.
    pub(super) fn apply_fonts(&mut self) {
        let now = (self.cfg.looks.clone(), self.cfg.font_size.to_bits());
        if self.looks.fonts_applied.as_ref() == Some(&now) {
            return;
        }
        let l = &self.cfg.looks;
        theme::set_code_metrics(code_scale(l, self.cfg.font_size), l.buffer_line_height);
        {
            let mut fonts = self.fonts.borrow_mut();
            fonts.set_mono_family(&l.buffer_font_family);
            fonts.set_mono_features(&l.buffer_font_features);
        }
        if l.ui_font_family != self.looks.ui_family {
            let family = l.ui_font_family.clone();
            if let Err(e) = self.fonts.borrow_mut().set_ui_family(&family) {
                self.message = Some(e);
            }
            self.looks.ui_family = family;
        }
        self.looks.fonts_applied = Some(now);
        self.forget_text();
    }

    /// Lays out text anew after a font change.
    fn forget_text(&mut self) {
        for t in &mut self.terms {
            t.forget_glyphs();
        }
        self.redraw = true;
    }

    fn apply_icon_theme(&mut self, name: &str) {
        let (list, _) = icon_theme::all();
        let wanted = if name.is_empty() { icon_theme::DEFAULT } else { name };
        match list.into_iter().find(|t| t.name.eq_ignore_ascii_case(wanted)) {
            Some(t) => icon_theme::set(t),
            None => {
                self.message = Some(format!("No icon theme named {name}"));
                icon_theme::set(IconTheme::built_in(false));
            }
        }
        self.redraw = true;
    }

    /// Looks, at most every two seconds, at the desktop's preference, the
    /// theme files and the settings file, and puts changes in place.
    pub(super) fn looks_tick(&mut self) {
        if let Some(rx) = &self.looks.asking {
            match rx.try_recv() {
                Ok(light) => {
                    self.looks.asking = None;
                    if light != self.looks.desktop {
                        self.looks.desktop = light;
                        if self.looks.preview.is_none() {
                            self.apply_theme();
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.looks.asking = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.looks.checked.is_some_and(|at| at.elapsed() < CHECK) || cfg!(test) {
            return;
        }
        self.looks.checked = Some(Instant::now());
        if self.cfg.looks.theme_mode == "system" && self.looks.asking.is_none() {
            let (tx, rx) = mpsc::channel();
            let waker = self.waker.clone();
            std::thread::spawn(move || {
                let _ = tx.send(themes::desktop_prefers_light());
                waker();
            });
            self.looks.asking = Some(rx);
        }
        let time = settings_time();
        if time != self.looks.settings_time {
            self.looks.settings_time = time;
            self.reload_looks();
        }
        if self.looks.root != self.root || self.looks.stamp != Some(themes::stamp(&self.root)) {
            if self.looks.preview.is_none() {
                self.reload_themes();
            }
        }
        self.apply_fonts();
    }

    /// Takes the looks from the settings file, written by hand.
    fn reload_looks(&mut self) {
        let path = crate::config::config_dir().join("settings.json");
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return };
        let mut looks = Looks::default();
        looks.load(&v);
        if looks != self.cfg.looks {
            let icons = looks.icon_theme != self.cfg.looks.icon_theme;
            self.cfg.looks = looks;
            self.apply_theme();
            self.apply_fonts();
            if icons {
                self.apply_icon_theme(&self.cfg.looks.icon_theme.clone());
            }
        }
    }

    fn save_looks(&mut self) {
        let _ = self.cfg.save();
        self.looks.settings_time = settings_time();
    }

    /// Switches between the dark and the light theme of the settings.
    pub(super) fn toggle_theme(&mut self) {
        let light = !self.cfg.light;
        let l = &mut self.cfg.looks;
        l.theme_mode = if light { "light" } else { "dark" }.into();
        l.theme = if light { l.light_theme.clone() } else { l.dark_theme.clone() };
        self.apply_theme();
        self.save_looks();
        self.message = Some(format!("Theme: {}", self.looks.shown));
    }

    /// Sets the theme mode: empty, `system`, `dark` or `light`.
    pub(super) fn set_theme_mode(&mut self, mode: &str) {
        self.cfg.looks.theme_mode = mode.into();
        self.looks.checked = None;
        self.apply_theme();
        self.save_looks();
    }

    // ---- the lists ------------------------------------------------------

    pub(super) fn open_themes(&mut self) {
        if self.looks.root != self.root {
            self.reload_themes();
        }
        let entries = self
            .looks
            .themes
            .iter()
            .map(|t| {
                let from = match &t.origin {
                    themes::Origin::BuiltIn => "built in".to_string(),
                    themes::Origin::Bundled => "bundled".to_string(),
                    themes::Origin::File(p) => crate::tools::short(&self.root, p),
                };
                let detail = format!("{} · {from}", if t.light { "light" } else { "dark" });
                Entry::new(&format!("theme:{}", t.name), &t.name, &detail).icon(if t.light {
                    Icon::Sun
                } else {
                    Icon::Moon
                })
            })
            .collect();
        let current = format!("theme:{}", self.looks.shown);
        self.open_looks_menu("Select theme", entries, &current);
    }

    pub(super) fn open_icon_themes(&mut self) {
        let (list, errors) = icon_theme::all();
        if let Some(e) = errors.first() {
            self.message = Some(format!("Icon theme not loaded: {e}"));
        }
        let entries = list
            .iter()
            .map(|t| {
                let detail = match t.name.as_str() {
                    icon_theme::DEFAULT => "built in · line icons in the finder",
                    icon_theme::LINES => "built in · line icons everywhere",
                    _ => "SVG icons",
                };
                Entry::new(&format!("icons:{}", t.name), &t.name, detail).icon(Icon::FileCode)
            })
            .collect();
        let current = format!("icons:{}", icon_theme::current().name);
        self.open_looks_menu("Select icon theme", entries, &current);
    }

    /// Lists font families, monospace ones for code.
    pub(super) fn open_fonts(&mut self, code: bool) {
        let mut entries = vec![Entry::new(
            if code { "mono:" } else { "ui:" },
            if code { "System monospace" } else { "Inter" },
            "default",
        )];
        for family in crate::font::families(code) {
            let key = format!("{}:{family}", if code { "mono" } else { "ui" });
            entries.push(Entry::new(&key, &family, "").icon(Icon::Code));
        }
        let (title, current) = if code {
            ("Select buffer font", format!("mono:{}", self.cfg.looks.buffer_font_family))
        } else {
            ("Select interface font", format!("ui:{}", self.cfg.looks.ui_font_family))
        };
        if entries.len() == 1 {
            self.message = Some("fontconfig lists no fonts; is fc-list installed?".into());
        }
        self.open_looks_menu(title, entries, &current);
    }

    fn open_looks_menu(&mut self, title: &str, entries: Vec<Entry>, current: &str) {
        self.comp = None;
        let mut menu = Menu::new(title, entries, current);
        menu.tick = true;
        menu.footer = vec![("\u{2191}\u{2193}".into(), "preview".into()), ("Esc".into(), "restore".into())];
        self.looks.preview = Some((current.to_string(), current.to_string()));
        self.menu = Some((MenuKind::Looks, menu));
        self.redraw = true;
    }

    /// Shows what the open list has chosen, or goes back to what was
    /// there once the list closes without a pick. Runs before each frame.
    pub(super) fn looks_preview(&mut self) {
        let Some((original, shown)) = self.looks.preview.clone() else { return };
        let chosen = match &mut self.menu {
            Some((MenuKind::Looks, menu)) => menu.chosen().map(|e| e.key.clone()),
            _ => {
                self.looks.preview = None;
                if shown != original {
                    self.try_look(&original);
                }
                return;
            }
        };
        if let Some(key) = chosen.filter(|k| *k != shown) {
            self.try_look(&key);
            self.looks.preview = Some((original, key));
        }
    }

    /// Shows a theme, icon theme or font without keeping it.
    fn try_look(&mut self, key: &str) {
        match split(key) {
            ("theme", name) => {
                if let Some(t) = themes::find(&self.looks.themes, name).cloned() {
                    self.show_theme(&t);
                }
            }
            ("icons", name) => self.apply_icon_theme(name),
            ("mono", family) => {
                self.fonts.borrow_mut().set_mono_family(family);
                self.forget_text();
            }
            ("ui", family) => {
                if self.fonts.borrow_mut().set_ui_family(family).is_ok() {
                    self.looks.ui_family = family.to_string();
                }
                self.forget_text();
            }
            _ => {}
        }
    }

    /// Keeps what a looks list picked.
    pub(super) fn looks_pick(&mut self, key: &str) {
        self.looks.preview = None;
        self.try_look(key);
        match split(key) {
            ("theme", name) => {
                let light = themes::find(&self.looks.themes, name).is_some_and(|t| t.light);
                self.cfg.looks.choose(name, light);
                let showing = self.cfg.looks.theme_for(self.looks.desktop) == name;
                self.message = Some(if showing {
                    format!("Theme: {name}")
                } else {
                    format!("{name} is used while the desktop prefers {}", if light { "light" } else { "dark" })
                });
                self.apply_theme();
            }
            ("icons", name) => {
                self.cfg.looks.icon_theme = if name == icon_theme::DEFAULT { String::new() } else { name.to_string() };
            }
            ("mono", family) => self.cfg.looks.buffer_font_family = family.to_string(),
            ("ui", family) => self.cfg.looks.ui_font_family = family.to_string(),
            _ => return,
        }
        self.save_looks();
        self.apply_fonts();
    }

    // ---- importing --------------------------------------------------------

    /// Converts a Zed or VS Code theme file into the user's themes folder.
    pub(super) fn import_theme(&mut self, input: &str) {
        let path = self.resolve(input.trim());
        self.prompt = None;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return self.message = Some(format!("Could not read {}: {e}", path.display())),
        };
        let stem = path.file_stem().map_or("Imported".into(), |s| s.to_string_lossy().into_owned());
        let (file, names) = match themes::import(&text, &stem) {
            Ok(done) => done,
            Err(e) => return self.message = Some(format!("Not imported: {e}")),
        };
        let dir = crate::config::config_dir().join("themes");
        let out = dir.join(themes::file_name(file["name"].as_str().unwrap_or(&stem)));
        let text = serde_json::to_string_pretty(&file).unwrap_or_default() + "\n";
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&out, text)) {
            return self.message = Some(format!("Could not write {}: {e}", out.display()));
        }
        self.reload_themes();
        self.message = Some(format!("Imported {} into {}", names.join(", "), out.display()));
        self.open_themes();
    }
}

/// Code's size as a fraction of the document's.
fn code_scale(l: &Looks, font_size: f32) -> f32 {
    if l.buffer_font_size > 0.0 { l.buffer_font_size / font_size.max(1.0) } else { theme::MONO_SCALE }
}

fn settings_time() -> Option<SystemTime> {
    std::fs::metadata(crate::config::config_dir().join("settings.json")).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(a: &mut App, key: Key) {
        a.key(key, Mods::default(), None);
        a.looks_preview();
    }

    #[test]
    fn moving_through_themes_previews_them_and_escape_restores() {
        let mut a = App::new(None);
        a.looks_init();
        assert_eq!(a.looks.shown, themes::DARK_NAME);
        a.run_command("theme_select");
        let Some((MenuKind::Looks, menu)) = &mut a.menu else { panic!("no theme list") };
        assert_eq!(menu.chosen().unwrap().key, format!("theme:{}", themes::DARK_NAME));
        press(&mut a, Key::Down);
        assert_eq!(a.looks.shown, themes::LIGHT_NAME);
        assert!(a.cfg.light);
        press(&mut a, Key::Down);
        assert_eq!(a.looks.shown, "One Dark");
        press(&mut a, Key::Escape);
        assert!(a.menu.is_none());
        assert_eq!(a.looks.shown, themes::DARK_NAME);
        assert!(!a.cfg.light);
        // Enter keeps it, and the toggle goes to the light theme and back.
        a.run_command("theme_select");
        press(&mut a, Key::Down);
        press(&mut a, Key::Down);
        press(&mut a, Key::Enter);
        assert_eq!((a.looks.shown.as_str(), a.cfg.looks.theme.as_str()), ("One Dark", "One Dark"));
        assert_eq!(a.cfg.looks.dark_theme, "One Dark");
        a.toggle_theme();
        assert!(a.cfg.light && a.looks.shown == themes::LIGHT_NAME);
        a.toggle_theme();
        assert_eq!(a.looks.shown, "One Dark");
        theme::set_light(false);
    }

    #[test]
    fn overrides_and_the_system_mode_choose_the_palette() {
        let mut a = App::new(None);
        a.cfg.looks.theme_overrides = serde_json::json!({"bg": "#123456"});
        a.looks_init();
        let shown = themes::find(&a.looks.themes, &a.looks.shown).unwrap().palette(&a.cfg.looks.theme_overrides);
        assert_eq!(shown.bg, 0x123456);
        a.cfg.looks.theme_mode = "system".into();
        a.cfg.looks.light_theme = "Nord".into();
        a.looks.desktop = Some(true);
        a.cfg.looks.theme_overrides = serde_json::json!({});
        a.apply_theme();
        assert_eq!(a.looks.shown, "Nord");
        a.cfg.looks.theme = "No such theme".into();
        a.cfg.looks.theme_mode = String::new();
        a.apply_theme();
        assert_eq!(a.looks.shown, themes::DARK_NAME);
        assert!(a.message.as_deref().unwrap().contains("No theme named"));
        theme::set_light(false);
    }

    #[test]
    fn a_vscode_theme_is_imported_into_the_themes_folder() {
        let dir = std::env::temp_dir().join(format!("ld-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mine-color-theme.json");
        std::fs::write(&path, r##"{"name": "Mine Imported", "type": "dark", "colors": {"editor.background": "#202020"}, "tokenColors": []}"##).unwrap();
        let mut a = App::new(None);
        a.looks_init();
        a.import_theme(&path.to_string_lossy());
        let written = crate::config::config_dir().join("themes/mine-imported.json");
        assert!(written.is_file(), "{:?}", a.message);
        assert!(matches!(a.menu, Some((MenuKind::Looks, _))));
        assert!(themes::find(&a.looks.themes, "Mine Imported").is_some());
        std::fs::remove_file(written).unwrap();
        a.import_theme("/nonexistent/theme.json");
        assert!(a.message.as_deref().unwrap().starts_with("Could not read"));
        std::fs::remove_dir_all(dir).unwrap();
        theme::set_light(false);
    }

    #[test]
    fn code_size_follows_the_settings_or_the_document() {
        let mut l = Looks::default();
        assert_eq!(code_scale(&l, 16.0), theme::MONO_SCALE);
        l.buffer_font_size = 20.0;
        assert_eq!(code_scale(&l, 16.0), 1.25);
    }
}
