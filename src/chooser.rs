//! The file chooser: a panel over the window that lists one folder at a
//! time, to walk the disk in and pick a file or a folder to open, and to
//! make a new folder in. What is typed narrows the list and reads as a
//! path: a slash steps into the folder named.

use std::path::{Path, PathBuf};

use crate::app::{Key, Mods};
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::icons::{self, Icon};
use crate::ui::px;
use crate::{theme, tree, ui};

/// The row that leads out of a folder.
const UP: &str = "..";
/// The row that opens the folder being shown.
const HERE: &str = ".";
/// The row that makes a new folder. No file can have this name.
const NEW: &str = "/new";

struct Row {
    name: String,
    folder: bool,
    /// A file's size, as its row says it.
    detail: String,
}

/// What a key or click did to the chooser.
#[derive(PartialEq, Debug)]
pub enum Chosen {
    None,
    Close,
    File(PathBuf),
    Folder(PathBuf),
    /// A folder was made, and the chooser has stepped into it.
    Made(PathBuf),
}

pub struct Chooser {
    dir: PathBuf,
    rows: Vec<Row>,
    /// The folder holds more than was read.
    more: bool,
    /// What has been typed since the folder was stepped into.
    typed: String,
    /// The name of a new folder being typed, in place of narrowing.
    naming: Option<String>,
    /// Why the new folder could not be made.
    said: Option<String>,
    /// The selection, counted among the rows that are shown.
    sel: usize,
    /// How far down the list is scrolled, in pixels.
    scroll: f32,
    /// The selected row is to be scrolled into view.
    reveal: bool,
    panel: (f32, f32, f32, f32),
    /// Where the list was last drawn: its top, its height and a row's.
    list: (f32, f32, f32),
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// A size in the largest unit that leaves it a number worth reading.
fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let (mut n, mut unit) = (bytes as f64, "B");
    for next in ["kB", "MB", "GB", "TB"] {
        if n < 999.5 {
            break;
        }
        (n, unit) = (n / 1000.0, next);
    }
    if n < 9.95 { format!("{n:.1} {unit}") } else { format!("{n:.0} {unit}") }
}

impl Chooser {
    pub(crate) fn input_text(&mut self) -> &mut String { self.naming.as_mut().unwrap_or(&mut self.typed) }
    pub(crate) fn input_changed(&mut self) {
        if self.naming.is_none() { let text = std::mem::take(&mut self.typed); self.type_text(&text); }
        self.sel = 0; self.reveal = true;
    }

    /// A chooser on `dir`, with `current` selected if it is there.
    pub fn new(dir: &Path, current: Option<&str>) -> Chooser {
        let mut chooser = Chooser {
            dir: PathBuf::new(),
            rows: Vec::new(),
            more: false,
            typed: String::new(),
            naming: None,
            said: None,
            sel: 0,
            scroll: 0.0,
            reveal: false,
            panel: (0.0, 0.0, 0.0, 0.0),
            list: (0.0, 0.0, 0.0),
        };
        chooser.enter(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()), current);
        chooser
    }

    /// Steps into a folder and selects `name` there, or else the first
    /// thing it holds.
    fn enter(&mut self, dir: PathBuf, name: Option<&str>) {
        let (found, more) = tree::entries(&dir);
        let row = |(name, folder): (String, bool)| {
            let bytes = if folder { None } else { std::fs::metadata(dir.join(&name)).ok().map(|m| m.len()) };
            Row { name, folder, detail: bytes.map_or(String::new(), size) }
        };
        let here = Row { name: HERE.into(), folder: true, detail: "Ctrl+Enter".into() };
        let new = Row { name: NEW.into(), folder: false, detail: "Ctrl+Shift+N".into() };
        let up = dir.parent().map(|_| Row { name: UP.into(), folder: true, detail: String::new() });
        self.rows = [here, new].into_iter().chain(up).chain(found.into_iter().map(row)).collect();
        (self.dir, self.more) = (dir, more);
        self.typed.clear();
        let shown = self.shown();
        let named = |n: &str| shown.iter().position(|&i| self.rows[i].name == n);
        // The first thing the folder holds, or else the way out of it.
        let lead = shown.iter().take_while(|&&i| matches!(self.rows[i].name.as_str(), HERE | NEW | UP)).count();
        let first = lead.min(shown.len().saturating_sub(1));
        (self.sel, self.scroll, self.reveal) = (name.and_then(named).unwrap_or(first), 0.0, true);
    }

    /// Steps out to the folder this one is in, with this one selected.
    fn up(&mut self) {
        if let Some(parent) = self.dir.parent() {
            let from = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
            self.enter(parent.to_path_buf(), from.as_deref());
        }
    }

    /// Indexes of the rows that what is typed leaves. Hidden files show
    /// once a dot is typed first, as in a shell.
    fn shown(&self) -> Vec<usize> {
        let want = self.typed.to_lowercase();
        let keep = |r: &Row| match r.name.as_str() {
            HERE | NEW | UP => want.is_empty(),
            name => (want.starts_with('.') || !name.starts_with('.')) && name.to_lowercase().contains(&want),
        };
        (0..self.rows.len()).filter(|&i| keep(&self.rows[i])).collect()
    }

    /// Puts the selection on what the typing means: the first name that
    /// starts with it, or else the first that holds it.
    fn aim(&mut self) {
        let want = self.typed.to_lowercase();
        let starts = |&i: &usize| self.rows[i].name.to_lowercase().starts_with(&want);
        self.sel = self.shown().iter().position(starts).unwrap_or(0);
        self.reveal = true;
    }

    /// Takes in text as it is typed or pasted. A slash ends the name of a
    /// folder, which is stepped into, so that a whole path leads to its file.
    pub fn type_text(&mut self, text: &str) {
        if let Some(name) = &mut self.naming {
            name.push_str(text);
            self.said = None;
            return;
        }
        for ch in text.chars() {
            if ch == '/' {
                self.slash();
            } else {
                self.typed.push(ch);
                self.aim();
            }
        }
    }

    fn slash(&mut self) {
        let shown = self.shown();
        let exact = self.rows.iter().position(|r| r.folder && r.name == self.typed);
        match self.typed.as_str() {
            "" => self.enter(PathBuf::from("/"), None),
            "~" => self.enter(home(), None),
            "." => self.typed.clear(),
            UP => self.up(),
            _ => {
                let folder = exact.or(shown.get(self.sel).copied()).filter(|&i| self.rows[i].folder);
                if let Some(i) = folder {
                    self.enter(self.dir.join(&self.rows[i].name), None);
                }
            }
        }
    }

    /// Acts on a row: a folder is stepped into, a file is the one chosen.
    /// The row of the folder shown opens it.
    fn take(&mut self, row: usize) -> Chosen {
        let Row { name, folder, .. } = &self.rows[row];
        let path = self.dir.join(name);
        match (name.as_str(), *folder) {
            (HERE, _) => return Chosen::Folder(self.dir.clone()),
            (NEW, _) => self.name_new(),
            (UP, _) => self.up(),
            (_, true) => self.enter(path, None),
            (_, false) => return Chosen::File(path),
        }
        Chosen::None
    }

    /// Opens the folder a row stands for, rather than stepping into it: the
    /// row's own if it is one, or else the folder shown.
    fn take_folder(&self, row: Option<usize>) -> Chosen {
        let folder = row.map(|i| &self.rows[i]).filter(|r| r.folder);
        Chosen::Folder(match folder.map(|r| r.name.as_str()) {
            Some(UP) => self.dir.parent().unwrap_or(&self.dir).to_path_buf(),
            Some(name) if name != HERE => self.dir.join(name),
            _ => self.dir.clone(),
        })
    }

    /// Starts typing the name of a new folder, from what was typed to
    /// narrow the list.
    fn name_new(&mut self) {
        self.naming = Some(std::mem::take(&mut self.typed));
        self.said = None;
    }

    /// Makes the folder named, in the one shown, and steps into it, onto
    /// the row that opens it.
    fn make(&mut self) -> Chosen {
        let name = self.naming.as_deref().unwrap_or("").trim().trim_end_matches('/').to_string();
        let plain = Path::new(&name).components().all(|c| matches!(c, std::path::Component::Normal(_)));
        let path = self.dir.join(&name);
        let why = match () {
            _ if name.is_empty() => Some("Type the new folder's name".to_string()),
            _ if !plain => Some("A name for a folder in this one, not a path out of it".to_string()),
            _ if path.symlink_metadata().is_ok() => Some(format!("{name} already exists")),
            _ => std::fs::create_dir_all(&path).err().map(|e| format!("Cannot make it: {e}")),
        };
        if why.is_some() {
            self.said = why;
            return Chosen::None;
        }
        self.naming = None;
        self.enter(path.clone(), None);
        self.sel = 0;
        Chosen::Made(path)
    }

    /// A key pressed while the name of a new folder is typed.
    fn naming_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> Chosen {
        let Some(name) = &mut self.naming else { return Chosen::None };
        self.said = None;
        match key {
            Key::Escape => self.naming = None,
            Key::Enter => return self.make(),
            Key::Backspace if mods.ctrl => name.clear(),
            Key::Backspace => {
                name.pop();
            }
            _ => {
                if let Some(s) = text.filter(|s| !mods.ctrl && !mods.alt && !s.chars().any(char::is_control)) {
                    name.push_str(s);
                }
            }
        }
        Chosen::None
    }

    pub fn key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> Chosen {
        if self.naming.is_some() {
            return self.naming_key(key, mods, text);
        }
        let shown = self.shown();
        let last = shown.len().saturating_sub(1);
        let sel = self.sel.min(last);
        let row = shown.get(sel).copied();
        let (_, tall, rh) = self.list;
        let page = if rh > 0.0 { ((tall / rh) as usize).saturating_sub(1).max(1) } else { 1 };
        self.reveal = true;
        match key {
            Key::Escape => return Chosen::Close,
            Key::Char('n') if mods.ctrl && mods.shift => self.name_new(),
            Key::Enter if mods.ctrl => return self.take_folder(row),
            Key::Enter => return row.map_or(Chosen::None, |i| self.take(i)),
            Key::Right => {
                let into = row.filter(|&i| self.rows[i].folder && self.rows[i].name != HERE);
                return into.map_or(Chosen::None, |i| self.take(i));
            }
            Key::Left => self.up(),
            Key::Backspace if self.typed.is_empty() => self.up(),
            Key::Backspace => {
                if mods.ctrl {
                    self.typed.clear();
                } else {
                    self.typed.pop();
                }
                self.aim();
            }
            Key::Up => self.sel = sel.saturating_sub(1),
            Key::Down => self.sel = (sel + 1).min(last),
            Key::PageUp => self.sel = sel.saturating_sub(page),
            Key::PageDown => self.sel = (sel + page).min(last),
            Key::Home => self.sel = 0,
            Key::End => self.sel = last,
            _ => {
                let typed = text.filter(|s| !mods.ctrl && !mods.alt && !s.chars().any(char::is_control));
                if let Some(s) = typed {
                    self.type_text(s);
                }
            }
        }
        Chosen::None
    }

    pub fn scroll(&mut self, dy: f32) {
        self.scroll += dy;
        self.reveal = false;
    }

    pub fn scroll_lines(&mut self, lines: f32) {
        self.scroll(lines * self.list.2);
    }

    /// The row under the pointer becomes the chosen one.
    pub fn motion(&mut self, x: f32, y: f32) {
        let (px, _, pw, _) = self.panel;
        let (top, tall, rh) = self.list;
        if rh <= 0.0 || x < px || x >= px + pw || y < top || y >= top + tall || self.naming.is_some() {
            return;
        }
        let at = ((y - top + self.scroll) / rh) as usize;
        if at < self.shown().len() {
            self.sel = at;
        }
    }

    /// A click acts on the row under it, as Enter would, or as Ctrl+Enter
    /// with `ctrl` held; one outside the panel closes it.
    pub fn press(&mut self, x: f32, y: f32, ctrl: bool) -> Chosen {
        let (px, py, pw, ph) = self.panel;
        if x < px || x >= px + pw || y < py || y >= py + ph {
            return Chosen::Close;
        }
        // A click in the list leaves the new folder unmade.
        let (top, tall, rh) = self.list;
        if y >= top {
            (self.naming, self.said) = (None, None);
        }
        if rh <= 0.0 || y < top || y >= top + tall {
            return Chosen::None;
        }
        let at = ((y - top + self.scroll) / rh) as usize;
        match self.shown().get(at) {
            Some(&i) if ctrl => {
                self.sel = at;
                self.take_folder(Some(i))
            }
            Some(&i) => {
                self.sel = at;
                self.take(i)
            }
            None => Chosen::None,
        }
    }

    /// The folder as the top of the panel names it, from the home folder
    /// where it is in there.
    fn place(&self) -> String {
        let place = match self.dir.strip_prefix(home()) {
            Ok(rest) if !home().as_os_str().is_empty() => Path::new("~").join(rest),
            _ => self.dir.clone(),
        };
        let place = place.to_string_lossy();
        format!("{}/", place.trim_end_matches('/'))
    }

    /// Draws the chooser in a `w` by `h` window.
    pub fn draw(&mut self, c: &mut Canvas, fonts: &mut Fonts, w: f32, h: f32, scale: f32) {
        let shown = self.shown();
        self.sel = self.sel.min(shown.len().saturating_sub(1));
        let size = px(theme::LARGE, scale);
        let small = px(theme::SMALL, scale);
        let rh = px(32.0, scale);
        let field_h = px(46.0, scale);
        let foot_h = px(34.0, scale);
        let inset = px(6.0, scale);
        // The panel is as tall in a folder that holds little as in one
        // that holds much, so it stays put while the disk is walked.
        let top_room = px(theme::TITLE + 6.0, scale);
        let count = ((h - top_room - field_h - foot_h - inset * 2.0 - px(16.0, scale)) / rh).floor().clamp(1.0, 13.0);
        let tall = rh * count;
        let pw = px(620.0, scale).min(w - px(24.0, scale)).max(160.0).round();
        let ph = field_h + tall + inset * 2.0 + foot_h;
        let (px_, py) = (((w - pw) / 2.0).round(), top_room.max((h * 0.08).min(px(72.0, scale))).round());
        self.panel = (px_, py, pw, ph);
        ui::surface(c, self.panel, px(theme::RADIUS_2XL, scale), scale);

        // The field: the folder, and after it what is typed, as one path.
        // One too long keeps its end in view.
        let pad = px(14.0, scale);
        let isz = px(theme::ICON, scale);
        let icon = if self.naming.is_some() { Icon::FolderPlus } else { Icon::FolderOpen };
        icons::draw(c, icon, px_ + pad, (py + (field_h - isz) / 2.0).round(), isz, theme::icon());
        let left = px_ + pad + isz + px(10.0, scale);
        let title = if self.naming.is_some() { "New folder" } else { "Open" };
        let title_w = fonts.width(font::UI_MEDIUM, small, title) + pad;
        let room = pw - (left - px_) - pad - title_w;
        let typed = self.naming.as_deref().unwrap_or(&self.typed);
        let place = self.place();
        let (dw, nw) = (fonts.width(font::UI, size, &place), fonts.width(font::UI, size, typed));
        let x = left - (dw + nw + size * 0.5 - room).max(0.0);
        let b = ui::base(py, field_h, size);
        c.clip(left, py, room, field_h);
        ui::text(c, fonts, x, b, &place, font::UI, size, theme::dim());
        ui::text(c, fonts, x + dw, b, typed, font::UI, size, theme::fg());
        let caret_x = crate::bidi::caret_x(fonts,font::UI,size,typed,typed.len());
        ui::caret(c, x + dw + caret_x + 1.0, py + field_h / 2.0, size * 1.25, scale);
        c.unclip();
        let tw = fonts.width(font::UI_MEDIUM, small, title);
        ui::text(
            c,
            fonts,
            px_ + pw - pad - tw,
            ui::base(py, field_h, small),
            title,
            font::UI_MEDIUM,
            small,
            theme::dim(),
        );
        ui::rule(c, px_, py + field_h, pw, 0.0);

        let top = py + field_h + inset;
        self.list = (top, tall, rh);
        if std::mem::take(&mut self.reveal) {
            let at = self.sel as f32 * rh;
            self.scroll = self.scroll.max(at + rh - tall).min(at);
        }
        self.scroll = self.scroll.min(shown.len() as f32 * rh - tall).max(0.0);
        c.clip(px_ + 1.0, top, pw - 2.0, tall);
        for (i, &at) in shown.iter().enumerate().skip((self.scroll / rh) as usize) {
            let y = top + i as f32 * rh - self.scroll;
            if y >= top + tall {
                break;
            }
            let row = &self.rows[at];
            let (label, icon, ink) = match (row.name.as_str(), row.folder) {
                (HERE, _) => ("Open this folder".to_string(), Icon::FolderOpen, theme::fg()),
                (NEW, _) => ("New folder\u{2026}".to_string(), Icon::FolderPlus, theme::fg()),
                (UP, _) => ("..".to_string(), Icon::ChevronLeft, theme::dim()),
                (_, true) => (format!("{}/", row.name), Icon::Folder, theme::fg()),
                (_, false) => (row.name.clone(), Icon::File, theme::fg()),
            };
            let r = (px_ + inset, y, pw - inset * 2.0, rh);
            ui::row_ground(c, r, i == self.sel, false, false, scale);
            let iy = (y + (rh - isz) / 2.0).round();
            icons::draw(c, icon, r.0 + px(10.0, scale), iy, isz, theme::icon());
            let dw = if row.detail.is_empty() { 0.0 } else { fonts.width(font::UI, small, &row.detail) };
            let lx = r.0 + px(36.0, scale);
            let label = ui::fit(
                fonts,
                font::UI,
                px(theme::UI_TEXT + 1.0, scale),
                &label,
                r.0 + r.2 - lx - dw - px(20.0, scale),
            );
            ui::text(
                c,
                fonts,
                lx,
                ui::base(y, rh, px(theme::UI_TEXT + 1.0, scale)),
                &label,
                font::UI,
                px(theme::UI_TEXT + 1.0, scale),
                ink,
            );
            if dw > 0.0 {
                ui::text(
                    c,
                    fonts,
                    r.0 + r.2 - px(10.0, scale) - dw,
                    ui::base(y, rh, small),
                    &row.detail,
                    font::UI,
                    small,
                    theme::dim(),
                );
            }
        }
        if shown.is_empty() {
            let none = "Nothing matches";
            let nw = fonts.width(font::UI, size, none);
            ui::text(c, fonts, px_ + (pw - nw) / 2.0, ui::base(top, rh, size), none, font::UI, size, theme::dim());
        }
        c.unclip();
        let n = shown.len() as f32;
        if n * rh > tall {
            ui::scrollbar(c, (px_, top, pw, tall), self.scroll / (n * rh), tall / (n * rh), false, scale);
        }

        // The foot: what the keys do, or why a folder could not be made.
        let foot = py + ph - foot_h;
        ui::rule(c, px_, foot, pw, 0.0);
        let room = pw - pad * 2.0;
        let fb = ui::base(foot, foot_h, small);
        let said = match (&self.said, self.more) {
            (Some(why), _) => Some((why.clone(), theme::danger())),
            (None, true) if self.naming.is_none() => {
                Some((format!("Only the first {} are listed", tree::MOST), theme::dim()))
            }
            _ => None,
        };
        let keys: &[(&str, &str)] = if self.naming.is_some() {
            &[("Enter", "make it here"), ("Esc", "go back")]
        } else {
            &[("Enter", "open"), ("/", "step in"), ("Ctrl+Enter", "open the folder"), ("Esc", "close")]
        };
        match said {
            Some((said, ink)) => {
                let said = ui::fit(fonts, font::UI, small, &said, room);
                ui::text(c, fonts, px_ + pad, fb, &said, font::UI, small, ink);
            }
            None => {
                let mut x = px_ + pad;
                for (k, d) in keys {
                    let need = ui::kbd_width(fonts, k, scale) + px(6.0, scale) + fonts.width(font::UI, small, d);
                    if x + need > px_ + pw - pad {
                        break;
                    }
                    x += ui::kbd(c, fonts, x, foot + foot_h / 2.0, k, scale) + px(6.0, scale);
                    x += ui::text(c, fonts, x, fb, d, font::UI, small, theme::dim()) + px(16.0, scale);
                }
            }
        }
    }

    /// The rows that are shown, with a `>` where the selection is.
    #[cfg(test)]
    pub fn listing(&self) -> Vec<String> {
        let line = |(i, &at): (usize, &usize)| {
            let row = &self.rows[at];
            format!("{}{}{}", if i == self.sel { ">" } else { "" }, row.name, if row.folder { "/" } else { "" })
        };
        self.shown().iter().enumerate().map(line).collect()
    }

    /// The middle of a row, by its name, for clicking it.
    #[cfg(test)]
    pub fn point(&self, name: &str) -> (f32, f32) {
        let i = self.shown().iter().position(|&at| self.rows[at].name == name).unwrap();
        let (top, _, rh) = self.list;
        (self.panel.0 + self.panel.2 / 2.0, top + (i as f32 + 0.5) * rh - self.scroll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-chooser-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["src/deep", "Docs"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for file in [".env", "b.md", "A.md", "src/main.py", "src/deep/x.py"] {
            std::fs::write(dir.join(file), "text").unwrap();
        }
        dir.canonicalize().unwrap()
    }

    fn draw(ch: &mut Chooser) {
        let mut px = vec![0u32; 900 * 600];
        ch.draw(&mut Canvas::new(&mut px, 900, 600), &mut Fonts::new(), 900.0, 600.0, 1.0);
    }

    #[test]
    fn folders_are_walked_and_a_file_is_chosen() {
        let dir = folder("walk");
        let mut ch = Chooser::new(&dir, Some("b.md"));
        // Folders first, hidden files left out, the open file selected.
        assert_eq!(ch.listing(), ["./", "/new", "../", "Docs/", "src/", "A.md", ">b.md"]);
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::File(dir.join("b.md")));
        // Enter and Right step into a folder; Left and Backspace step out,
        // back onto the folder that was left.
        ch.key(Key::Up, NONE, None);
        ch.key(Key::Up, NONE, None);
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::None);
        assert_eq!(ch.listing(), ["./", "/new", "../", ">deep/", "main.py"]);
        ch.key(Key::Right, NONE, None);
        assert_eq!(ch.listing(), ["./", "/new", "../", ">x.py"]);
        assert_eq!(ch.key(Key::Right, NONE, None), Chosen::None);
        ch.key(Key::Left, NONE, None);
        assert_eq!(ch.listing(), ["./", "/new", "../", ">deep/", "main.py"]);
        ch.key(Key::Backspace, NONE, None);
        assert_eq!(ch.listing(), ["./", "/new", "../", "Docs/", ">src/", "A.md", "b.md"]);

        // A click does what Enter does on its row; one outside closes.
        draw(&mut ch);
        let (x, y) = ch.point("src");
        assert_eq!(ch.press(x, y, false), Chosen::None);
        draw(&mut ch);
        let (x, y) = ch.point("main.py");
        assert_eq!(ch.press(x, y, false), Chosen::File(dir.join("src/main.py")));
        let (x, y) = ch.point(UP);
        ch.press(x, y, false);
        assert_eq!(ch.listing()[4], ">src/");
        let (px, py, ..) = ch.panel;
        assert_eq!((ch.press(px + 5.0, py + 5.0, false), ch.press(2.0, 2.0, false)), (Chosen::None, Chosen::Close));
        assert_eq!(ch.key(Key::Escape, NONE, None), Chosen::Close);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_is_chosen() {
        let dir = folder("pick");
        let ctrl = Mods { ctrl: true, ..NONE };
        let mut ch = Chooser::new(&dir, None);
        // Its own row opens the folder shown; Right does not step into it.
        ch.key(Key::Home, NONE, None);
        assert_eq!(ch.key(Key::Right, NONE, None), Chosen::None);
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::Folder(dir.clone()));
        // Ctrl+Enter opens the folder selected instead of stepping in, the
        // way out's folder on its row, and the folder shown on a file.
        ch.key(Key::Down, NONE, None);
        ch.key(Key::Down, NONE, None);
        ch.key(Key::Down, NONE, None);
        assert_eq!(ch.listing()[3], ">Docs/");
        assert_eq!(ch.key(Key::Enter, ctrl, None), Chosen::Folder(dir.join("Docs")));
        ch.key(Key::Up, NONE, None);
        assert_eq!(ch.key(Key::Enter, ctrl, None), Chosen::Folder(dir.parent().unwrap().to_path_buf()));
        ch.key(Key::End, NONE, None);
        assert_eq!(ch.key(Key::Enter, ctrl, None), Chosen::Folder(dir.clone()));
        // So does a click with Ctrl held, and the row stays hidden while
        // something is typed.
        draw(&mut ch);
        let (x, y) = ch.point("src");
        assert_eq!(ch.press(x, y, true), Chosen::Folder(dir.join("src")));
        let (x, y) = ch.point(HERE);
        assert_eq!(ch.press(x, y, false), Chosen::Folder(dir.clone()));
        ch.type_text("sr");
        assert_eq!(ch.listing(), [">src/"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_is_made() {
        let dir = folder("make");
        let mut ch = Chooser::new(&dir, None);
        // Its row asks for the name, in place of what narrows the list.
        ch.key(Key::Home, NONE, None);
        ch.key(Key::Down, NONE, None);
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::None);
        assert!(ch.naming.is_some());
        // A name that is taken, or leads out of here, is refused and said.
        ch.type_text("src");
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::None);
        assert_eq!(ch.said.as_deref(), Some("src already exists"));
        ch.key(Key::Backspace, Mods { ctrl: true, ..NONE }, None);
        ch.type_text("../out");
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::None);
        assert!(ch.said.is_some() && !dir.parent().unwrap().join("out").exists());
        // Made, it is stepped into, with its row to open it selected.
        ch.key(Key::Backspace, Mods { ctrl: true, ..NONE }, None);
        for c in "notes/2026".chars() {
            ch.key(Key::Char(c), NONE, Some(&c.to_string()));
        }
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::Made(dir.join("notes/2026")));
        assert_eq!(ch.listing(), ["./", "/new", "../"].map(|r| if r == "./" { ">./" } else { r }));
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::Folder(dir.join("notes/2026")));
        // Ctrl+Shift+N starts from what was typed; Esc goes back to the list.
        ch.type_text("dra");
        ch.key(Key::Char('n'), Mods { ctrl: true, shift: true, alt: false }, None);
        assert_eq!(ch.naming.as_deref(), Some("dra"));
        ch.key(Key::Escape, NONE, None);
        assert!(ch.naming.is_none() && ch.key(Key::Escape, NONE, None) == Chosen::Close);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn typing_narrows_the_list_and_reads_as_a_path() {
        let dir = folder("type");
        let mut ch = Chooser::new(&dir, None);
        assert_eq!(ch.listing(), ["./", "/new", "../", ">Docs/", "src/", "A.md", "b.md"]);
        // Whatever the case, and wherever in the name; a dot first shows
        // what is hidden.
        ch.type_text("MD");
        assert_eq!(ch.listing(), [">A.md", "b.md"]);
        ch.key(Key::Backspace, Mods { ctrl: true, ..NONE }, None);
        ch.key(Key::Char('.'), NONE, Some("."));
        assert_eq!(ch.listing(), [">.env", "A.md", "b.md"]);
        ch.key(Key::Backspace, NONE, None);
        // The name that starts with what is typed comes before one that
        // only holds it, and a slash steps into it.
        ch.type_text("d");
        assert_eq!(ch.listing(), [">Docs/", "A.md", "b.md"]);
        ch.key(Key::Backspace, NONE, None);
        ch.type_text("sr/de");
        assert_eq!((ch.listing(), ch.typed.as_str()), (vec![">deep/".to_string()], "de"));
        ch.type_text("/../../");
        assert_eq!(ch.listing(), ["./", "/new", "../", "Docs/", ">src/", "A.md", "b.md"]);
        // A whole path, as when one is pasted, leads to its file.
        ch.type_text(&format!("{}/src/deep/x.py", dir.display()));
        assert_eq!(ch.listing(), [">x.py"]);
        assert_eq!(ch.key(Key::Enter, NONE, None), Chosen::File(dir.join("src/deep/x.py")));
        ch.key(Key::Backspace, Mods { ctrl: true, ..NONE }, None);
        ch.type_text("/");
        assert_eq!((ch.dir.as_path(), ch.place().as_str()), (Path::new("/"), "/"));
        assert!(!ch.listing().contains(&"../".to_string()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sizes_read_in_their_unit() {
        let said = [0, 999, 1000, 9_949, 123_456, 999_499, 999_500, 5_000_000_000].map(size);
        assert_eq!(said, ["0 B", "999 B", "1.0 kB", "9.9 kB", "123 kB", "999 kB", "1.0 MB", "5.0 GB"]);
    }
}
