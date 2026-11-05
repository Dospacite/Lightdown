//! The terminal panel: a shell on a pseudo-terminal, drawn under the
//! document in the system's monospace face, with ANSI and truecolour output.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::app::{Key, Mods};
use crate::font::Fonts;
use crate::gfx::Canvas;
use crate::kernel::Waker;
use crate::pty::{self, Pty};
use crate::theme;
use crate::vt::{self, Cell, Color, Mouse, Screen};

/// What the reading thread and the window share: the screen, and the
/// parser in the middle of whatever sequence the last read cut short.
struct Shared {
    parser: vte::Parser,
    screen: Screen,
    gone: bool,
}

/// A place in the text: a line, numbered as `Screen::dropped` numbers
/// them, and a column.
type Pos = (u64, usize);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Unit {
    Char,
    Word,
    Line,
}

/// A character's glyphs, each with its face and place in the cell, and
/// how far the character advances.
type Glyphs = (Vec<(u8, u16, f32, f32)>, f32);

/// Something printed in the terminal that can be opened.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Url(String),
    /// A file, with the line and column printed after it, if any.
    File(PathBuf, Option<usize>, Option<usize>),
}

/// The word of `text` at character `at`, as far as spaces, quotes and
/// brackets, with what follows it.
fn word_at(text: &str, at: usize) -> Option<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    let stop = |c: char| c.is_whitespace() || "'\"`<>()[]{}|".contains(c);
    if chars.get(at).is_none_or(|&c| stop(c)) {
        return None;
    }
    let start = chars[..at].iter().rposition(|&c| stop(c)).map_or(0, |i| i + 1);
    let end = chars[at..].iter().position(|&c| stop(c)).map_or(chars.len(), |i| at + i);
    let word: String = chars[start..end].iter().collect();
    let after: String = chars[end..].iter().collect();
    Some((word, after))
}

/// Reads a word printed in a terminal as something to open: a web
/// address, or a file that is there, as a path from `cwd`, perhaps with a
/// line and column after it (`src/a.rs:12:5`), or as Python's tracebacks
/// print it (`File "a.py", line 12`).
pub fn link(word: &str, after: &str, cwd: &Path) -> Option<Link> {
    let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    if word.starts_with("http://") || word.starts_with("https://") {
        return Some(Link::Url(word.to_string()));
    }
    let word = word.strip_prefix("file://").unwrap_or(word);
    // Numbers after colons: a line, and a column.
    let mut parts: Vec<&str> = word.split(':').collect();
    let mut numbers = Vec::new();
    while parts.len() > 1 && parts.last().is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        numbers.insert(0, parts.pop().unwrap().parse::<usize>().ok()?);
    }
    let name = parts.join(":");
    if name.is_empty() {
        return None;
    }
    let path = match name.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None => cwd.join(&name),
    };
    if !path.is_file() {
        return None;
    }
    let mut line = numbers.first().copied();
    if line.is_none()
        && let Some(rest) = after.strip_prefix("\", line ").or_else(|| after.strip_prefix(", line "))
    {
        line = rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok());
    }
    Some(Link::File(path, line, numbers.get(1).copied()))
}

/// What has happened since the terminal was last asked.
#[derive(Default)]
pub struct News {
    /// The shell has ended.
    pub gone: bool,
    /// Text a program put on the clipboard.
    pub clipboard: Option<String>,
}

/// What a job has written, bounded, and how it ended: an exit status, or
/// `None` inside for a signal.
#[derive(Default)]
pub struct JobState {
    pub text: String,
    /// Bytes written in all, including those dropped from `text`.
    pub total: u64,
    pub code: Option<Option<i32>>,
}

impl JobState {
    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        self.text.push_str(&String::from_utf8_lossy(bytes));
        if self.text.len() > 256_000 {
            let mut at = self.text.len() - 256_000;
            while !self.text.is_char_boundary(at) {
                at += 1;
            }
            self.text.drain(..at);
        }
    }
}

pub struct Term {
    pub job_name: Option<String>,
    pub agent_owner: Option<String>,
    pub agent_id: u64,
    shared: Arc<Mutex<Shared>>,
    pty: Option<Pty>,
    /// Where the shell starts.
    pub dir: PathBuf,
    env_root: PathBuf,
    /// The right pane of a split, ruled off from the left one.
    pub divided: bool,
    waker: Waker,
    fonts: Rc<RefCell<Fonts>>,
    rect: (f32, f32, f32, f32),
    scale: f32,
    /// Text size in logical pixels, before zooming.
    pub size: f32,
    pub zoom: f32,
    /// How many lines the view is scrolled back into the history.
    back: usize,
    pub search: String,
    search_at: Option<Pos>,
    /// Wheel movement not yet a whole line.
    wheel: f32,
    /// The selection: where it was started, and where it reaches.
    sel: Option<(Pos, Pos)>,
    drag: Option<Unit>,
    /// The button held down, as the program was told of it.
    held: Option<u8>,
    /// The cell the pointer was last reported in.
    reported: Option<(usize, usize)>,
    glyphs: HashMap<(char, bool), Glyphs>,
    glyph_size: f32,
    pub focused: bool,
    /// Whether the program has been told it has the keyboard.
    told: Option<bool>,
    pub hover: Option<(f32, f32)>,
    grid: (usize, usize),
}

/// The colours of xterm's palette.
fn palette(i: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let i = i - 16;
            let v = |n: u8| if n == 0 { 0 } else { 55 + 40 * n };
            (v(i / 36), v(i / 6 % 6), v(i % 6))
        }
        _ => {
            let g = 8 + 10 * (i - 232);
            (g, g, g)
        }
    }
}

/// The first sixteen ANSI colours follow the theme's terminal roles. Extended
/// indexed and truecolour values retain the colour requested by the program.
fn colour(c: Color) -> Option<u32> {
    match c {
        Color::Default => None,
        Color::Index(i @ 0..=15) => Some(theme::ansi(i as usize)),
        Color::Index(i) => {
            let (r, g, b) = palette(i);
            Some((r as u32) << 16 | (g as u32) << 8 | b as u32)
        }
        Color::Rgb(r, g, b) => Some((r as u32) << 16 | (g as u32) << 8 | b as u32),
    }
}

fn lerp(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let (x, y) = ((a >> shift & 0xff) as f32, (b >> shift & 0xff) as f32);
        ((x + (y - x) * t).round() as u32).min(255) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// Keeps text legible on arbitrary terminal backgrounds, moving toward
/// black or white only as far as the required contrast. Hue is retained.
fn readable(fg: u32, bg: u32) -> u32 {
    if theme::contrast(fg, bg) >= 4.5 {
        return fg;
    }
    let target = if theme::contrast(0, bg) > theme::contrast(0xffffff, bg) { 0 } else { 0xffffff };
    for step in 1..=32 {
        let ink = lerp(fg, target, step as f32 / 32.0);
        if theme::contrast(ink, bg) >= 4.5 {
            return ink;
        }
    }
    target
}

fn inks(cell: &Cell) -> (u32, u32) {
    let mut fg =
        colour(cell.fg).unwrap_or_else(|| if cell.attrs & vt::BOLD != 0 { theme::bright() } else { theme::fg() });
    let mut bg = colour(cell.bg).unwrap_or_else(theme::bg);
    if cell.attrs & vt::INVERSE != 0 {
        if cell.fg == Color::Default {
            fg = theme::fg();
        }
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.attrs & vt::DIM != 0 {
        fg = lerp(fg, bg, 0.25);
    }
    (readable(fg, bg), bg)
}

/// Whether a character belongs to a word, for selecting by words: paths
/// and addresses hold together.
fn wordy(c: char) -> bool {
    !c.is_whitespace() && !"()[]{}<>'\"`|;,".contains(c)
}

/// Draws a box-drawing or block character as rectangles, so that lines
/// join from row to row whatever the font. Returns false for others.
#[allow(clippy::too_many_arguments)]
fn block(c: &mut Canvas, ch: char, x: f32, y: f32, w: f32, h: f32, thin: f32, color: u32) -> bool {
    let (x, y) = (x.round(), y.round());
    let (w, h) = ((x + w).round() - x, (y + h).round() - y);
    match ch {
        '█' => c.rect(x, y, w, h, color),
        '▀' => c.rect(x, y, w, (h / 2.0).round(), color),
        '▄' => c.rect(x, y + (h / 2.0).round(), w, h - (h / 2.0).round(), color),
        '▌' => c.rect(x, y, (w / 2.0).round(), h, color),
        '▐' => c.rect(x + (w / 2.0).round(), y, w - (w / 2.0).round(), h, color),
        '▔' => c.rect(x, y, w, (h / 8.0).round().max(1.0), color),
        '▁'..='▇' => {
            let part = (h * (ch as u32 - '▁' as u32 + 1) as f32 / 8.0).round();
            c.rect(x, y + h - part, w, part, color);
        }
        '▉'..='▏' => {
            let part = (w * ('▏' as u32 - ch as u32 + 1) as f32 / 8.0).round().max(1.0);
            c.rect(x, y, part, h, color);
        }
        '░' | '▒' | '▓' => {
            let alpha = match ch {
                '░' => 64,
                '▒' => 128,
                _ => 192,
            };
            c.shade(x, y, w, h, color, alpha);
        }
        _ => {
            // Arms reaching left, right, up and down: 1 thin, 2 thick, 3 double.
            let arms = match ch {
                '─' => [1, 1, 0, 0],
                '━' => [2, 2, 0, 0],
                '│' => [0, 0, 1, 1],
                '┃' => [0, 0, 2, 2],
                '┌' | '╭' => [0, 1, 0, 1],
                '┐' | '╮' => [1, 0, 0, 1],
                '└' | '╰' => [0, 1, 1, 0],
                '┘' | '╯' => [1, 0, 1, 0],
                '├' => [0, 1, 1, 1],
                '┤' => [1, 0, 1, 1],
                '┬' => [1, 1, 0, 1],
                '┴' => [1, 1, 1, 0],
                '┼' => [1, 1, 1, 1],
                '┏' => [0, 2, 0, 2],
                '┓' => [2, 0, 0, 2],
                '┗' => [0, 2, 2, 0],
                '┛' => [2, 0, 2, 0],
                '┣' => [0, 2, 2, 2],
                '┫' => [2, 0, 2, 2],
                '┳' => [2, 2, 0, 2],
                '┻' => [2, 2, 2, 0],
                '╋' => [2, 2, 2, 2],
                '═' => [3, 3, 0, 0],
                '║' => [0, 0, 3, 3],
                '╔' => [0, 3, 0, 3],
                '╗' => [3, 0, 0, 3],
                '╚' => [0, 3, 3, 0],
                '╝' => [3, 0, 3, 0],
                '╠' => [0, 3, 3, 3],
                '╣' => [3, 0, 3, 3],
                '╦' => [3, 3, 0, 3],
                '╩' => [3, 3, 3, 0],
                '╬' => [3, 3, 3, 3],
                '╴' => [1, 0, 0, 0],
                '╶' => [0, 1, 0, 0],
                '╵' => [0, 0, 1, 0],
                '╷' => [0, 0, 0, 1],
                _ => return false,
            };
            let (mx, my) = (x + ((w - thin) / 2.0).round(), y + ((h - thin) / 2.0).round());
            for (i, &arm) in arms.iter().enumerate() {
                if arm == 0 {
                    continue;
                }
                let t = if arm == 2 { thin * 2.0 } else { thin };
                // A double line is two thin ones either side of the middle.
                let offsets: &[f32] = if arm == 3 { &[-thin, thin] } else { &[0.0] };
                for &o in offsets {
                    let (ax, ay) = (mx - ((t - thin) / 2.0).round(), my - ((t - thin) / 2.0).round());
                    match i {
                        0 => c.rect(x, ay + o, mx - x + t, t, color),
                        1 => c.rect(ax, ay + o, x + w - ax, t, color),
                        2 => c.rect(ax + o, y, t, my - y + t, color),
                        _ => c.rect(ax + o, ay, t, y + h - ay, color),
                    }
                }
            }
        }
    }
    true
}

/// The bytes a key sends to the program.
pub fn encode(key: Key, mods: Mods, text: Option<&str>, app_cursor: bool) -> Vec<u8> {
    let m = 1 + mods.shift as u8 + 2 * mods.alt as u8 + 4 * mods.ctrl as u8;
    let alt = |mut bytes: Vec<u8>| {
        if mods.alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };
    // Cursor keys: `CSI A`, or `SS3 A` in application mode, or with the
    // modifiers held `CSI 1;m A`.
    let cursor = |f: char| match (m, app_cursor) {
        (1, true) => format!("\x1bO{f}").into_bytes(),
        (1, false) => format!("\x1b[{f}").into_bytes(),
        _ => format!("\x1b[1;{m}{f}").into_bytes(),
    };
    let tilde = |n: u8| if m == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{m}~") }.into_bytes();
    match key {
        Key::Up => cursor('A'),
        Key::Down => cursor('B'),
        Key::Right => cursor('C'),
        Key::Left => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Delete => tilde(3),
        Key::F(n @ 1..=4) => {
            let f = (b'P' + n - 1) as char;
            if m == 1 { format!("\x1bO{f}").into_bytes() } else { format!("\x1b[1;{m}{f}").into_bytes() }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        Key::F(_) => Vec::new(),
        Key::Enter => alt(b"\r".to_vec()),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt(b"\t".to_vec()),
        Key::Escape => b"\x1b".to_vec(),
        // Ctrl+Backspace takes back a word, as in the rest of the editor.
        Key::Backspace if mods.ctrl => b"\x17".to_vec(),
        Key::Backspace => alt(b"\x7f".to_vec()),
        Key::Char(c) if mods.ctrl => {
            let code = match c {
                'a'..='z' => Some(c as u8 - b'a' + 1),
                '@' | ' ' | '2' => Some(0),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '-' | '7' | '/' => Some(0x1f),
                '8' | '?' => Some(0x7f),
                _ => None,
            };
            match code {
                Some(b) => alt(vec![b]),
                None => text.map_or(Vec::new(), |t| alt(t.as_bytes().to_vec())),
            }
        }
        Key::Char(c) => alt(text.map_or_else(|| c.to_string(), str::to_string).into_bytes()),
        Key::Other => alt(text.unwrap_or("").as_bytes().to_vec()),
    }
}

impl Term {
    #[cfg(test)]
    pub fn new(dir: &Path, fonts: Rc<RefCell<Fonts>>, waker: Waker) -> Term {
        Self::new_in(dir, dir, fonts, waker)
    }
    pub fn new_in(dir: &Path, root: &Path, fonts: Rc<RefCell<Fonts>>, waker: Waker) -> Term {
        let shared = Shared { parser: vte::Parser::new(), screen: Screen::new(80, 24), gone: false };
        static NEXT_AGENT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Term {
            job_name: None,
            agent_owner: None,
            agent_id: NEXT_AGENT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            divided: false,
            shared: Arc::new(Mutex::new(shared)),
            pty: None,
            dir: dir.to_path_buf(),
            env_root: root.to_path_buf(),
            waker,
            fonts,
            rect: (0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
            size: 13.5,
            zoom: 1.0,
            back: 0,
            search: String::new(),
            search_at: None,
            wheel: 0.0,
            sel: None,
            drag: None,
            held: None,
            reported: None,
            glyphs: HashMap::new(),
            glyph_size: 0.0,
            focused: false,
            told: None,
            hover: None,
            grid: (0, 0),
        }
    }

    pub fn start_job(&mut self, name: &str, command: std::process::Command, state: Arc<Mutex<JobState>>) -> std::io::Result<()> {
        self.job_name = Some(name.into());
        self.grid = (80, 24);
        let shared = self.shared.clone();
        let wake = self.waker.clone();
        let done = state.clone();
        let end_shared = self.shared.clone();
        let end_wake = self.waker.clone();
        let id = self.agent_id;
        self.pty = Some(Pty::spawn_command(Some(command), "sh", &self.dir, &self.env_root, self.grid,
            move |bytes| {
                crate::ports::feed(id, bytes);
                let mut guard = shared.lock().unwrap();
                let Shared { parser, screen, .. } = &mut *guard;
                parser.advance(screen, bytes);
                let reply = std::mem::take(&mut screen.reply);
                state.lock().unwrap().push(bytes);
                drop(guard);
                wake();
                reply
            }, move |code| {
                done.lock().unwrap().code = Some(code);
                let mut guard = end_shared.lock().unwrap();
                let Shared { parser, screen, .. } = &mut *guard;
                let note = match code {
                    Some(0) => "\r\n\x1b[1m[Exited with status 0]\x1b[0m\r\n".to_string(),
                    Some(n) => format!("\r\n\x1b[1;31m[Exited with status {n}]\x1b[0m\r\n"),
                    None => "\r\n\x1b[1;31m[Stopped by a signal]\x1b[0m\r\n".to_string(),
                };
                parser.advance(screen, note.as_bytes());
                guard.gone = true;
                end_wake();
            })?);
        Ok(())
    }

    pub fn job_pid(&self) -> Option<i32> { self.pty.as_ref().map(Pty::pid) }

    /// Starts the shell, once the panel has a size to give it.
    fn start(&mut self) {
        if self.job_name.is_some() || self.pty.is_some() || self.grid.0 == 0 {
            return;
        }
        let (shared, wake, gone_wake) = (self.shared.clone(), self.waker.clone(), self.waker.clone());
        let gone_shared = self.shared.clone();
        let id = self.agent_id;
        let output = move |bytes: &[u8]| {
            crate::ports::feed(id, bytes);
            let mut guard = shared.lock().unwrap();
            let Shared { parser, screen, .. } = &mut *guard;
            parser.advance(screen, bytes);
            let reply = std::mem::take(&mut screen.reply);
            drop(guard);
            wake();
            reply
        };
        let gone = move || {
            gone_shared.lock().unwrap().gone = true;
            gone_wake();
        };
        match Pty::spawn(&pty::shell(), &self.dir, &self.env_root, self.grid, output, gone) {
            Ok(pty) => self.pty = Some(pty),
            Err(e) => {
                let mut guard = self.shared.lock().unwrap();
                let Shared { parser, screen, .. } = &mut *guard;
                parser.advance(screen, format!("Cannot start {}: {e}\r\n", pty::shell()).as_bytes());
            }
        }
    }

    /// Ends the shell and starts another, on a clean screen.
    pub fn restart(&mut self) {
        self.pty = None;
        let (cols, rows) = self.grid;
        let screen = Screen::new(cols.max(2), rows.max(1));
        *self.shared.lock().unwrap() = Shared { parser: vte::Parser::new(), screen, gone: false };
        (self.back, self.sel, self.told) = (0, None, None);
        self.start();
    }

    /// Whether a shell is running.
    pub fn running(&self) -> bool {
        self.pty.is_some()
    }

    /// Whether a selection is being dragged out.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether the shell has the terminal to itself, waiting for a command.
    pub fn idle(&self) -> bool {
        self.pty.as_ref().is_some_and(Pty::idle)
    }

    /// Takes in what the shell has done since the last call.
    pub fn poll(&mut self) -> News {
        let mut guard = self.shared.lock().unwrap();
        let screen = &mut guard.screen;
        let scrolled = std::mem::take(&mut screen.scrolled);
        if self.back > 0 {
            self.back = (self.back + scrolled).min(screen.history.len());
        }
        screen.bell = false;
        let clipboard = screen.clipboard.take();
        let gone = std::mem::take(&mut guard.gone);
        drop(guard);
        if gone {
            self.pty = None;
        }
        News { gone, clipboard }
    }

    /// The folder the shell is in, as it last said, or else where it
    /// started.
    pub fn cwd(&self) -> PathBuf {
        let said = self.shared.lock().unwrap().screen.cwd.clone();
        said.map(PathBuf::from).filter(|d| d.is_dir()).unwrap_or_else(|| self.dir.clone())
    }

    /// The program's own name for what it shows, or else the shell's
    /// folder.
    pub fn title(&self) -> String {
        if let Some(name) = &self.job_name { return name.clone(); }
        let guard = self.shared.lock().unwrap();
        match (&guard.screen.title, &guard.screen.cwd) {
            (t, _) if !t.is_empty() => t.clone(),
            (_, Some(cwd)) => cwd.clone(),
            _ => String::new(),
        }
    }

    // ---- geometry -------------------------------------------------------

    /// Space above the first row of cells.
    fn head(&self) -> f32 {
        (6.0 * self.scale).round()
    }

    fn pad(&self) -> f32 {
        (10.0 * self.scale).round()
    }

    /// The text size in pixels, a cell's width and a row's height.
    fn metrics(&self, fonts: &mut Fonts) -> (f32, f32, f32) {
        let size = (self.size * self.zoom * self.scale).round().max(6.0);
        let mono = fonts.mono();
        (size, fonts.width(mono, size, "M").max(1.0), (size * 1.3).round())
    }

    /// The top left of the grid of cells.
    fn origin(&self) -> (f32, f32) {
        (self.rect.0 + self.pad(), self.rect.1 + self.head())
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) {
        (self.rect, self.scale) = ((x, y, w, h), scale);
        if w < 1.0 || h < 1.0 {
            return;
        }
        let (_, cw, ch) = self.metrics(&mut self.fonts.borrow_mut());
        let (_, top) = self.origin();
        let cols = ((w - self.pad() * 2.0) / cw).floor().max(2.0) as usize;
        let rows = ((y + h - top - self.pad() / 2.0) / ch).floor().max(1.0) as usize;
        if (cols, rows) != self.grid {
            // Lines wrapped anew are numbered anew.
            if cols != self.grid.0 {
                (self.sel, self.back) = (None, 0);
            }
            self.grid = (cols, rows);
            self.shared.lock().unwrap().screen.resize(cols, rows);
            if let Some(pty) = &self.pty {
                pty.resize(cols, rows);
            }
        }
        self.start();
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        rw > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    /// Whether a point is on the panel's top edge, which is dragged to
    /// make it taller or shorter.
    pub fn on_edge(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, _) = self.rect;
        rw > 0.0 && x >= rx && x < rx + rw && y - ry >= -9.0 * self.scale && y - ry <= 3.0 * self.scale
    }

    /// Whether a point is on the text, below the header.
    pub fn on_text(&self, x: f32, y: f32) -> bool {
        self.contains(x, y) && y >= self.rect.1 + self.head()
    }

    /// What printed under a point can be opened, if anything: a word of
    /// the line there, lines the screen broke joined again.
    pub fn link_at(&self, x: f32, y: f32) -> Option<Link> {
        if !self.on_text(x, y) {
            return None;
        }
        let (col, row) = self.cell_at(x, y, false);
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        let first = screen.total() - screen.rows - self.back.min(screen.history.len());
        let at = first + row;
        let mut start = at;
        while start > 0 && screen.row(start - 1).is_some_and(|l| l.wrapped) {
            start -= 1;
        }
        let (mut text, mut offset) = (String::new(), 0);
        let mut i = start;
        while let Some(line) = screen.row(i) {
            if i == at {
                offset = text.chars().count() + line.cells.iter().take(col).filter(|c| c.width > 0).count();
            }
            text += &line.text(0, screen.cols);
            if !line.wrapped || i > at + 8 {
                break;
            }
            i += 1;
        }
        drop(guard);
        let (word, after) = word_at(&text, offset)?;
        link(&word, &after, &self.cwd())
    }

    /// Whether the program takes the mouse for itself, and so a press
    /// is reported to it rather than selecting.
    pub fn grabs_mouse(&self, mods: Mods) -> bool {
        !mods.shift && self.shared.lock().unwrap().screen.mouse != Mouse::Off
    }

    /// The cell under a point, column and row of the screen, kept within
    /// it. With `round`, a point past the middle of a cell counts as the
    /// next, as the end of a selection does.
    fn cell_at(&self, x: f32, y: f32, round: bool) -> (usize, usize) {
        let (cw, ch) = {
            let (_, cw, ch) = self.metrics(&mut self.fonts.borrow_mut());
            (cw, ch)
        };
        let (x0, y0) = self.origin();
        let col = ((x - x0) / cw + if round { 0.5 } else { 0.0 }).floor().max(0.0) as usize;
        let row = ((y - y0) / ch).floor().max(0.0) as usize;
        (col.min(self.grid.0), row.min(self.grid.1.saturating_sub(1)))
    }

    /// The place in the text under a point.
    fn pos_at(&self, screen: &Screen, x: f32, y: f32, round: bool) -> Pos {
        let (col, row) = self.cell_at(x, y, round);
        let first = screen.total() - screen.rows - self.back.min(screen.history.len());
        (screen.dropped + (first + row) as u64, col)
    }

    // ---- input ----------------------------------------------------------

    fn send(&mut self, bytes: &[u8]) {
        if let Some(pty) = &self.pty {
            pty.write(bytes);
        }
    }

    /// A key typed while the terminal has the keyboard. Returns whether it
    /// was sent.
    pub fn key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        let app_cursor = self.shared.lock().unwrap().screen.app_cursor;
        let bytes = encode(key, mods, text, app_cursor);
        if bytes.is_empty() {
            return false;
        }
        (self.back, self.sel) = (0, None);
        self.send(&bytes);
        true
    }

    /// Text pasted in, marked as pasted if the program asked for that, so
    /// that what it holds is not run line by line.
    /// IME commits are typed text, without bracketed-paste framing.
    pub(crate) fn input_commit(&mut self, text: &str) { self.send(text.as_bytes()); }
    pub(crate) fn input_caret(&self) -> (f32,f32,f32,f32) {
        let (_,cw,ch) = self.metrics(&mut self.fonts.borrow_mut());
        let (x,y) = self.origin();
        let shared = self.shared.lock().unwrap();
        let (col,row) = shared.screen.cursor();
        (x+col as f32*cw,y+row as f32*ch,cw,ch)
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bracketed = self.shared.lock().unwrap().screen.bracketed_paste;
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
            // A paste must not end the paste early.
            bytes.extend(text.replace("\x1b[201~", "").bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend(text.bytes());
        }
        (self.back, self.sel) = (0, None);
        self.send(&bytes);
    }

    /// Types a command at the shell's prompt and runs it.
    pub fn run(&mut self, command: &str) {
        (self.back, self.sel) = (0, None);
        self.send(format!("{command}\r").as_bytes());
    }

    /// Tells the program whether it has the keyboard, if it asked to know.
    pub fn set_focus(&mut self, on: bool) {
        self.focused = on;
        if self.told == Some(on) {
            return;
        }
        let wants = self.shared.lock().unwrap().screen.focus_events;
        if wants {
            self.send(if on { b"\x1b[I" } else { b"\x1b[O" });
        }
        self.told = Some(on);
    }

    /// Lets go of the history, and has the shell show its prompt afresh at
    /// the top if it is waiting for a command.
    pub fn clear(&mut self) {
        self.shared.lock().unwrap().screen.clear();
        (self.back, self.sel) = (0, None);
        if self.idle() {
            self.send(b"\x0c");
        }
    }

    pub fn select_all(&mut self) {
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        let last = (0..screen.rows).rev().find(|&r| screen.lines[r].cells.iter().any(|c| c.ch != ' '));
        let end = screen.dropped + (screen.history.len() + last.unwrap_or(0)) as u64;
        self.sel = Some(((screen.dropped, 0), (end, screen.cols)));
    }

    /// Scrolls the prompt before, or after, the top of the view to the
    /// top. Returns whether there was one.
    pub fn command_jump(&mut self, previous: bool) -> bool {
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        let top = screen.total() - screen.rows - self.back.min(screen.history.len());
        let prompts = screen.commands().into_iter().map(|c| c.prompt.0);
        let row = if previous { prompts.filter(|&r| r < top).last() } else { prompts.into_iter().find(|&r| r > top) };
        let Some(row) = row else { return false };
        self.back = (screen.total() - screen.rows).saturating_sub(row).min(screen.history.len());
        true
    }

    /// Selects the output of the last command that has ended, scrolls to
    /// it and gives its text.
    pub fn select_last_output(&mut self) -> Option<String> {
        let (from, to, back) = {
            let guard = self.shared.lock().unwrap();
            let screen = &guard.screen;
            let last = screen.commands().into_iter().rev().find(|c| c.output.is_some() && c.end.is_some())?;
            let (from, to) = (last.output?, last.end?);
            let top = screen.total() - screen.rows - self.back.min(screen.history.len());
            let shown = (top..top + screen.rows).contains(&from.0);
            let back = if shown { self.back } else { (screen.total() - screen.rows).saturating_sub(from.0).min(screen.history.len()) };
            let pos = |(r, c): (usize, usize)| (screen.dropped + r as u64, c);
            (pos(from), pos(to), back)
        };
        self.back = back;
        self.sel = Some((from, to));
        Some(self.selected().unwrap_or_default().trim_end_matches('\n').to_string())
    }

    /// The command line of the last command that was run, as the shell
    /// said it, or else as the screen shows it.
    pub fn last_command_line(&self) -> Option<String> {
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        let last = screen.commands().into_iter().rev().find(|c| c.output.is_some())?;
        if let Some(text) = last.text.filter(|t| !t.trim().is_empty()) {
            return Some(text.trim_end().to_string());
        }
        let ((r0, c0), (r1, c1)) = (last.input?, last.output?);
        let mut out = String::new();
        for r in r0..=r1 {
            let Some(line) = screen.row(r) else { break };
            let from = if r == r0 { c0 } else { 0 };
            let to = if r == r1 { c1 } else { usize::MAX };
            out += &line.text(from, to);
            if !line.wrapped && r != r1 {
                out = out.trim_end().to_string() + "\n";
            }
        }
        let out = out.trim().to_string();
        (!out.is_empty()).then_some(out)
    }

    /// Whether the shell has marked any prompt.
    pub fn has_marks(&self) -> bool {
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        (0..screen.total()).any(|r| screen.row(r).is_some_and(|l| !l.marks.is_empty()))
    }

    pub fn has_selection(&self) -> bool {
        self.sel.is_some_and(|(a, b)| a != b)
    }

    /// The text selected, lines that were wrapped joined again.
    pub fn selected(&self) -> Option<String> {
        let (a, b) = self.sel.filter(|(a, b)| a != b)?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        let mut out = String::new();
        for l in a.0.max(screen.dropped)..=b.0 {
            let Some(line) = screen.row((l - screen.dropped) as usize) else { break };
            let from = if l == a.0 { a.1 } else { 0 };
            let to = if l == b.0 { b.1 } else { usize::MAX };
            let text = line.text(from, to);
            let runs_on = line.wrapped && l != b.0;
            out += if runs_on { &text } else { text.trim_end() };
            if l != b.0 && !line.wrapped {
                out.push('\n');
            }
        }
        Some(out)
    }

    /// The word or line around a place, as the start and end of it.
    fn unit_at(&self, screen: &Screen, at: Pos, unit: Unit) -> (Pos, Pos) {
        let Some(line) = at.0.checked_sub(screen.dropped).and_then(|i| screen.row(i as usize)) else {
            return (at, at);
        };
        match unit {
            Unit::Char => (at, at),
            Unit::Line => ((at.0, 0), (at.0, screen.cols)),
            Unit::Word => {
                let cells = &line.cells;
                let ch = |i: usize| cells.get(i).map_or(' ', |c| c.ch);
                if !wordy(ch(at.1)) {
                    return (at, (at.0, at.1 + 1));
                }
                let mut a = at.1;
                while a > 0 && (wordy(ch(a - 1)) || cells[a - 1].width == 0) {
                    a -= 1;
                }
                let mut b = at.1;
                while b < cells.len() && (wordy(ch(b)) || cells[b].width == 0) {
                    b += 1;
                }
                ((at.0, a), (at.0, b))
            }
        }
    }

    /// The bytes that tell the program of a button or the wheel at a cell.
    fn report(screen: &Screen, code: u8, (col, row): (usize, usize), mods: Mods, release: bool) -> Vec<u8> {
        let code = code + 4 * mods.shift as u8 + 8 * mods.alt as u8 + 16 * mods.ctrl as u8;
        if screen.mouse_sgr {
            let end = if release { 'm' } else { 'M' };
            return format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes();
        }
        let code = if release { code & !3 | 3 } else { code };
        let at = |v: usize| (v + 33).min(255) as u8;
        vec![0x1b, b'[', b'M', 32 + code, at(col), at(row)]
    }

    /// A press on the text, where it is either reported to the program or
    /// starts a selection. `button` is 0, 1 or 2 for left, middle and
    /// right.
    pub fn press(&mut self, x: f32, y: f32, mods: Mods, clicks: u32, button: u8) {
        if !self.on_text(x, y) {
            return;
        }
        let cell = self.cell_at(x, y, false);
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        if screen.mouse != Mouse::Off && !mods.shift {
            let bytes = Self::report(screen, button, cell, mods, false);
            drop(guard);
            self.held = Some(button);
            self.reported = Some(cell);
            self.send(&bytes);
            return;
        }
        if button != 0 {
            return;
        }
        let at = self.pos_at(screen, x, y, clicks == 1);
        let unit = match clicks {
            1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let (a, b) = self.unit_at(screen, at, unit);
        drop(guard);
        self.sel = match (self.sel, mods.shift && clicks == 1) {
            (Some((anchor, _)), true) => Some((anchor, b)),
            _ => Some((a, b)),
        };
        self.drag = Some(unit);
    }

    /// The pointer moved. Returns whether the panel should be drawn again.
    pub fn motion(&mut self, x: f32, y: f32, mods: Mods) -> bool {
        self.hover = Some((x, y));
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        let reporting = matches!((screen.mouse, self.held), (Mouse::Motion, _) | (Mouse::Drag, Some(_)));
        if reporting && self.drag.is_none() && self.on_text(x, y) {
            let cell = self.cell_at(x, y, false);
            if self.reported == Some(cell) {
                return false;
            }
            let code = 32 + self.held.unwrap_or(3);
            let bytes = Self::report(screen, code, cell, mods, false);
            drop(guard);
            self.reported = Some(cell);
            self.send(&bytes);
            return false;
        }
        let Some(unit) = self.drag else { return false };
        let at = self.pos_at(screen, x, y, unit == Unit::Char);
        let Some((anchor, _)) = self.sel else { return false };
        let (a, b) = self.unit_at(screen, at, unit);
        let (anchor_a, anchor_b) = self.unit_at(screen, anchor, unit);
        drop(guard);
        self.sel = Some(if at < anchor { (anchor_b.max(anchor), a) } else { (anchor_a.min(anchor), b) });
        // Dragging past the top or bottom scrolls the history.
        let (_, y0) = self.origin();
        let bottom = self.rect.1 + self.rect.3;
        if y < y0 {
            self.scroll_back(1);
        } else if y >= bottom {
            self.scroll_back(-1);
        }
        true
    }

    pub fn release(&mut self, button: u8, mods: Mods) {
        self.drag = None;
        if self.held.take() == Some(button) {
            let guard = self.shared.lock().unwrap();
            let screen = &guard.screen;
            if matches!(screen.mouse, Mouse::Click | Mouse::Drag | Mouse::Motion) {
                let cell = self.reported.unwrap_or((0, 0));
                let bytes = Self::report(screen, button, cell, mods, true);
                drop(guard);
                self.send(&bytes);
            }
        }
    }

    pub fn leave(&mut self) {
        self.hover = None;
    }

    fn scroll_back(&mut self, lines: i64) {
        let most = self.shared.lock().unwrap().screen.history.len() as i64;
        self.back = (self.back as i64 + lines).clamp(0, most) as usize;
    }

    /// Scrolls by lines, downwards for more than 0: the history, or else
    /// what the program shows, by telling it of the wheel or, on the
    /// alternate screen, by arrow keys.
    pub fn scroll_lines(&mut self, lines: f32, mods: Mods) {
        self.wheel += lines;
        let steps = self.wheel.trunc() as i64;
        self.wheel -= steps as f32;
        if steps == 0 {
            return;
        }
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        if screen.mouse != Mouse::Off && !mods.shift {
            let cell = self.hover.map_or((0, 0), |(x, y)| self.cell_at(x, y, false));
            let code = if steps < 0 { 64 } else { 65 };
            let bytes = Self::report(screen, code, cell, mods, false).repeat(steps.unsigned_abs() as usize);
            drop(guard);
            self.send(&bytes);
        } else if screen.alt {
            let key = if steps < 0 { Key::Up } else { Key::Down };
            let bytes = encode(key, Mods::default(), None, screen.app_cursor).repeat(steps.unsigned_abs() as usize);
            drop(guard);
            self.send(&bytes);
        } else {
            drop(guard);
            self.scroll_back(-steps);
        }
    }

    /// Scrolls by pixels, as a touchpad does.
    pub fn scroll(&mut self, dy: f32, mods: Mods) {
        let (_, _, ch) = self.metrics(&mut self.fonts.borrow_mut());
        self.scroll_lines(dy / ch, mods);
    }

    /// Pages through the history, as Shift+PgUp and Shift+PgDn do.
    pub fn page(&mut self, up: bool) {
        let rows = self.grid.1.max(2) as i64 - 1;
        self.scroll_back(if up { rows } else { -rows });
    }

    pub fn scroll_edge(&mut self, top: bool) {
        self.scroll_back(if top { i64::MAX / 2 } else { i64::MIN / 2 });
    }

    pub fn zoom_by(&mut self, steps: i32) {
        self.zoom = (self.zoom * 1.1f32.powi(steps)).clamp(0.5, 4.0);
    }

    /// Shapes characters anew, after the font changed.
    pub fn forget_glyphs(&mut self) {
        self.glyphs.clear();
    }

    // ---- painting -------------------------------------------------------

    /// The glyphs of a character, shaped once and kept: where each is in
    /// the cell, and how far the character advances.
    fn glyphs(&mut self, fonts: &mut Fonts, ch: char, bold: bool, size: f32) -> &Glyphs {
        if self.glyph_size != size {
            self.glyphs.clear();
            self.glyph_size = size;
        }
        self.glyphs.entry((ch, bold)).or_insert_with(|| {
            let face = if bold { fonts.mono_bold() } else { fonts.mono() };
            let (mut out, mut pen) = (Vec::new(), 0.0);
            fonts.shape(face, size, ch.encode_utf8(&mut [0; 4]), |_, _, adv, shaped| {
                out.extend(shaped.iter().map(|g| (g.face, g.id, pen + g.x, g.y)));
                pen += adv;
            });
            (out, pen)
        })
    }

    /// Draws the panel: the grid of cells, and while scrolled back, how far.
    pub fn draw(&mut self, c: &mut Canvas) {
        let (x, y, w, h) = self.rect;
        if w < 1.0 || h < 1.0 {
            return;
        }
        let s = self.scale;
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let (size, cw, ch) = self.metrics(fonts);
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());
        // Split, the right pane is ruled off from the left one.
        if self.divided {
            c.line(x, y, 1.0, h, theme::line());
        }
        let head = self.head();

        let (x0, y0) = self.origin();
        c.clip(x, y + head, w, h - head);
        let shared = self.shared.clone();
        let guard = shared.lock().unwrap();
        let screen = &guard.screen;
        let rows = screen.rows;
        self.back = self.back.min(screen.history.len());
        let first = screen.total() - rows - self.back;
        let (cx, cy) = screen.cursor();
        let show_cursor = self.back == 0 && (screen.cursor_visible || !self.focused);
        let sel = self.sel.filter(|(a, b)| a != b).map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let mono = fonts.mono();
        let (asc, desc, _) = fonts.metrics(mono, size);
        let baseline = ((ch - (asc + desc)) / 2.0 + asc).round();
        let thin = (size * 0.075).round().max(1.0);
        let mut colours = HashMap::new();
        for r in 0..rows {
            let Some(line) = screen.row(first + r) else { continue };
            let abs = screen.dropped + (first + r) as u64;
            let ry = (y0 + r as f32 * ch).round();
            // Beside a command that has ended, whether it went well.
            if let Some(code) = line.marks.iter().filter(|m| m.kind == 'A').find_map(|m| m.code) {
                let ink = if code == 0 { theme::success() } else { theme::danger() };
                let pad = self.pad();
                c.circle(x0 - pad / 2.0, ry + ch / 2.0, (2.5 * s).max(1.5), ink);
                crate::ui::tip((x0 - pad, ry, pad, ch), &format!("Exit status {code}"), "");
            }
            for col in 0..screen.cols {
                let cell = line.cells.get(col).copied().unwrap_or(Cell::BLANK);
                if cell.width == 0 {
                    continue;
                }
                let span = cell.width as usize;
                let cxp = x0 + col as f32 * cw;
                let (mut fg, mut bg) = *colours.entry((cell.fg, cell.bg, cell.attrs)).or_insert_with(|| inks(&cell));
                let chosen = sel.is_some_and(|(a, b)| (abs, col) >= a && (abs, col) < b);
                let at_cursor = show_cursor && r == cy && col == cx;
                if chosen {
                    bg = theme::selection();
                }
                let block_cursor = at_cursor && self.focused && screen.cursor_shape <= 2;
                if block_cursor {
                    (fg, bg) = (theme::bg(), theme::caret());
                }
                if bg != theme::bg() {
                    c.rect(cxp, ry, cw * span as f32, ch, bg);
                }
                if at_cursor && !block_cursor {
                    let bar = (s * 2.0).round().max(1.0);
                    match screen.cursor_shape {
                        _ if !self.focused => c.frame(cxp.round(), ry, cw.round(), ch, 1.0, theme::dim()),
                        3 | 4 => c.rect(cxp, ry + ch - bar, cw, bar, theme::caret()),
                        _ => c.rect(cxp, ry, bar, ch, theme::caret()),
                    }
                }
                if cell.ch == ' ' || cell.attrs & vt::HIDDEN != 0 {
                    continue;
                }
                if block(c, cell.ch, cxp, ry, cw * span as f32, ch, thin, fg) {
                    continue;
                }
                let bold = cell.attrs & vt::BOLD != 0;
                let room = cw * span as f32;
                let (glyphs, adv) = self.glyphs(fonts, cell.ch, bold, size);
                // A glyph from another face is centred in its cells.
                let dx = if *adv < room { ((room - adv) / 2.0).max(0.0) } else { 0.0 };
                for &(face, id, gx, gy) in glyphs {
                    let px = cxp + dx + gx;
                    let ix = px.floor();
                    let bmp = fonts.glyph(face, id, size, ((px - ix) * 4.0) as u8);
                    c.glyph(ix as i32, (ry + baseline - gy) as i32, bmp, fg);
                }
                if cell.attrs & vt::UNDERLINE != 0 {
                    c.line(cxp, ry + baseline + (desc * 0.5).round().max(1.0), room, thin, fg);
                }
                if cell.attrs & vt::STRIKE != 0 {
                    c.line(cxp, ry + baseline - (size * 0.3).round(), room, thin, fg);
                }
            }
        }
        drop(guard);
        // While scrolled back, a bar says how far.
        if self.back > 0 {
            let total = self.shared.lock().unwrap().screen.total() as f32;
            let (top, tall) = (y + head, h - head);
            let len = (tall * rows as f32 / total).max(24.0 * s).min(tall);
            let pos = (tall - len) * (first as f32 / (total - rows as f32).max(1.0));
            let bw = (4.0 * s).round();
            c.round_rect(x + w - bw - (3.0 * s).round(), top + pos, bw, len, bw / 2.0, theme::faint());
        }
        c.unclip();
    }

    /// Finds literal text across history and screen, wrapping in either direction.
    pub fn find_history(&mut self, query: &str, previous: bool) -> (usize, usize) {
        if self.search != query {
            self.search_at = None;
        }
        self.search = query.to_string();
        if query.is_empty() {
            self.search_at = None;
            self.sel = None;
            return (0, 0);
        }
        let guard = self.shared.lock().unwrap();
        let screen = &guard.screen;
        let mut hits = Vec::new();
        let mut row = 0;
        while row < screen.total() {
            let mut text = String::new();
            let mut columns = Vec::new();
            let mut end = (screen.dropped + row as u64, 0);
            while let Some(line) = screen.row(row) {
                for (col, cell) in line.cells.iter().enumerate().filter(|(_, c)| c.width > 0) {
                    columns.push((text.len(), (screen.dropped + row as u64, col)));
                    text.push(cell.ch);
                }
                end = (screen.dropped + row as u64, line.cells.len());
                row += 1;
                if !line.wrapped || row >= screen.total() {
                    break;
                }
            }
            columns.push((text.len(), end));
            for (at, _) in text.match_indices(query) {
                let start = columns.binary_search_by_key(&at, |p| p.0).ok().map(|i| columns[i].1);
                let end = columns.binary_search_by_key(&(at + query.len()), |p| p.0).ok().map(|i| columns[i].1);
                if let (Some(a), Some(b)) = (start, end) {
                    hits.push(((a, b), (a.0 - screen.dropped) as usize));
                }
                if hits.len() >= 100_000 {
                    break;
                }
            }
            if hits.len() >= 100_000 {
                break;
            }
        }
        let total = hits.len();
        if total == 0 {
            self.search_at = None;
            self.sel = None;
            return (0, 0);
        }
        let selected = if previous {
            self.search_at.and_then(|p| hits.iter().rposition(|(range, _)| range.0 < p)).unwrap_or(total - 1)
        } else {
            self.search_at.and_then(|p| hits.iter().position(|(range, _)| range.0 > p)).unwrap_or(0)
        };
        let (range, row) = hits[selected];
        self.sel = Some(range);
        self.search_at = Some(range.0);
        let top = row.saturating_sub(screen.rows / 2).min(screen.total().saturating_sub(screen.rows));
        self.back = screen.total().saturating_sub(screen.rows + top);
        (selected + 1, total)
    }

    pub fn recent_text(&self, limit: usize) -> String {
        let guard = self.shared.lock().unwrap();
        let text = guard.screen.lines.iter().map(|line| line.text(0, usize::MAX)).collect::<Vec<_>>().join("\n");
        if text.len() > limit { text[crate::editor::ceil(&text, text.len() - limit)..].to_string() } else { text }
    }

    /// The screen's text, for tests.
    #[cfg(test)]
    pub fn screen_text(&self) -> String {
        self.shared.lock().unwrap().screen.text()
    }

    /// Waits until the shell has been quiet for a moment. For tests and
    /// headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub fn settle(&mut self) {
        let quiet = std::time::Duration::from_millis(250);
        let start = std::time::Instant::now();
        let mut last = (String::new(), std::time::Instant::now());
        while start.elapsed() < std::time::Duration::from_secs(5) && self.pty.is_some() {
            let now = {
                let guard = self.shared.lock().unwrap();
                let s = &guard.screen;
                let lines: Vec<String> = s.lines.iter().map(|l| l.text(0, s.cols)).collect();
                format!("{}{lines:?}", s.history.len())
            };
            if now != last.0 {
                last = (now, std::time::Instant::now());
            } else if last.1.elapsed() >= quiet {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    #[test]
    fn keys_become_what_terminals_send() {
        let e = |key, mods, text: Option<&str>| encode(key, mods, text, false);
        assert_eq!(e(Key::Char('a'), NONE, Some("A")), b"A");
        assert_eq!(e(Key::Char('c'), CTRL, Some("\x03")), b"\x03");
        assert_eq!(e(Key::Char('b'), Mods { alt: true, ..NONE }, Some("b")), b"\x1bb");
        assert_eq!(e(Key::Enter, NONE, None), b"\r");
        assert_eq!(e(Key::Backspace, NONE, None), b"\x7f");
        assert_eq!(e(Key::Up, NONE, None), b"\x1b[A");
        assert_eq!(encode(Key::Up, NONE, None, true), b"\x1bOA");
        assert_eq!(e(Key::Right, CTRL, None), b"\x1b[1;5C");
        assert_eq!(e(Key::Tab, Mods { shift: true, ..NONE }, None), b"\x1b[Z");
        assert_eq!(e(Key::F(5), NONE, None), b"\x1b[15~");
        assert_eq!(e(Key::F(1), NONE, None), b"\x1bOP");
        assert_eq!(e(Key::Delete, CTRL, None), b"\x1b[3;5~");
        assert_eq!(e(Key::Char(' '), CTRL, None), b"\x00");
    }

    #[test]
    fn printed_paths_and_addresses_are_links() {
        let dir = std::env::temp_dir().join(format!("ld-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "").unwrap();
        let file = |l, c| Some(Link::File(dir.join("src/a.rs"), l, c));
        let at = |text: &str, i: usize| word_at(text, i).and_then(|(w, a)| link(&w, &a, &dir));
        assert_eq!(at("error at src/a.rs:12:5: oops", 10), file(Some(12), Some(5)));
        assert_eq!(at("see (src/a.rs).", 7), file(None, None));
        assert_eq!(at("  File \"src/a.rs\", line 40, in f", 10), file(Some(40), None));
        assert_eq!(at("go to https://example.com/x.", 8), Some(Link::Url("https://example.com/x".into())));
        assert_eq!(at("src/missing.rs:3", 2), None);
        assert_eq!(at("plain words", 2), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colours_preserve_hue_and_contrast() {
        let cell = Cell { fg: Color::Index(0), bg: Color::Index(2), ..Cell::BLANK };
        let (fg, bg) = inks(&cell);
        assert!(theme::contrast(fg, bg) >= 4.5);
        let red = colour(Color::Index(1)).unwrap();
        let green = colour(Color::Index(2)).unwrap();
        assert_ne!(red, green);
        assert_eq!(colour(Color::Rgb(20, 180, 240)), Some(0x14b4f0));
        for i in 0..=255 {
            for bg in [Color::Default, Color::Index(i), Color::Rgb(100, 90, 170)] {
                for attrs in [0, vt::DIM, vt::INVERSE] {
                    let (fg, bg) = inks(&Cell { fg: Color::Index(i), bg, attrs, ..Cell::BLANK });
                    assert!(theme::contrast(fg, bg) >= 4.5);
                }
            }
        }
        let (fg, bg) = inks(&Cell { attrs: vt::INVERSE, ..Cell::BLANK });
        assert_eq!((fg, bg), (theme::bg(), theme::fg()));
    }

    #[test]
    fn a_shell_runs_in_it() {
        let fonts = Rc::new(RefCell::new(Fonts::new()));
        let mut t = Term::new(&std::env::temp_dir(), fonts, Arc::new(|| {}));
        t.set_rect(0.0, 0.0, 600.0, 300.0, 1.0);
        assert!(t.pty.is_some() && t.grid.0 > 20);
        t.settle();
        t.run("printf 'one\\ntwo  \\n'; tput cols");
        t.settle();
        let text = t.screen_text();
        assert!(text.contains("one\ntwo\n"), "{text}");
        assert!(text.contains(&t.grid.0.to_string()), "{text}");
        // A selection of whole lines is copied as they read.
        t.select_all();
        assert!(t.selected().unwrap().contains("one\ntwo\n"));
        // The shell ending is noticed.
        t.run("exit");
        let start = std::time::Instant::now();
        while !t.poll().gone && start.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(t.pty.is_none());
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    #[test]
    fn history_search_wraps_and_selects_unicode_cells() {
        let fonts = Rc::new(RefCell::new(Fonts::new()));
        let mut term = Term::new(std::env::temp_dir().as_path(), fonts, Arc::new(|| {}));
        {
            let mut guard = term.shared.lock().unwrap();
            let Shared { parser, screen, .. } = &mut *guard;
            for i in 0..60 {
                parser.advance(screen, format!("line {i}: café 界\r\n").as_bytes());
            }
        }
        assert_eq!(term.find_history("café", false), (1, 60));
        assert_eq!(term.selected().unwrap(), "café");
        assert!(term.back > 0);
        assert_eq!(term.find_history("café", true), (60, 60));
        assert_eq!(term.find_history("界", false), (1, 60));
        assert_eq!(term.selected().unwrap(), "界");
        assert_eq!(term.find_history("absent", false), (0, 0));
    }
}

#[cfg(test)]
mod wrapped_search_tests {
    use super::*;
    #[test]
    fn searches_join_soft_wrapped_rows() {
        let mut t = Term::new(std::env::temp_dir().as_path(), Rc::new(RefCell::new(Fonts::new())), Arc::new(|| {}));
        {
            let mut g = t.shared.lock().unwrap();
            g.screen = Screen::new(8, 3);
            let Shared { parser, screen, .. } = &mut *g;
            parser.advance(screen, b"abcdefghXYZ");
        }
        assert_eq!(t.find_history("ghXY", false), (1, 1));
        assert_eq!(t.selected().as_deref(), Some("ghXY"));
    }
}
