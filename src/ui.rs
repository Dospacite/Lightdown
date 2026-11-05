//! The interface's shared pieces: text, buttons, key caps, fields,
//! switches, raised surfaces, tooltips, and the lists that narrow as they
//! are typed in, from the command palette to the menus of each control.

use std::cell::RefCell;

use crate::app::{Key, Mods};
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::icons::{self, Icon};
use crate::{editor, fuzzy, theme};

thread_local! { static INPUT_CARET: RefCell<Option<Rect>> = const { RefCell::new(None) }; }
pub(crate) fn input_caret() -> Option<Rect> { INPUT_CARET.with(|r| *r.borrow()) }
pub(crate) fn clear_input_caret() {
    INPUT_CARET.with(|r| *r.borrow_mut() = None);
    INPUT_FIELD.with(|r| *r.borrow_mut() = None);
}
struct InputField { id: u64, text: String, face: u8, size: f32, rect: Rect, x: f32 }
thread_local! { static INPUT_FIELD: RefCell<Option<InputField>> = const { RefCell::new(None) }; }
pub(crate) fn remember_input(id: u64, text: &str, face: u8, size: f32, rect: Rect, x: f32) {
    INPUT_FIELD.with(|f| *f.borrow_mut() = Some(InputField { id,text:text.into(),face,size,rect,x }));
}
pub(crate) fn input_hit(fonts: &mut Fonts, x: f32, y: f32) -> Option<(u64,usize)> {
    INPUT_FIELD.with(|f| {
        let f = f.borrow(); let f = f.as_ref()?;
        inside(f.rect,Some((x,y))).then(|| (f.id,crate::bidi::hit(fonts,f.face,f.size,&f.text,x-f.x)))
    })
}

pub type Rect = (f32, f32, f32, f32);

/// Draws one line of text with its baseline at `base`. Returns its width.
#[allow(clippy::too_many_arguments)]
pub fn text(c: &mut Canvas, fonts: &mut Fonts, x: f32, base: f32, s: &str, face: u8, size: f32, color: u32) -> f32 {
    text_marked(c, fonts, (x, base), s, face, size, color, &[], color)
}

/// Draws a line of text with the characters at the byte offsets `marks`
/// in another colour. Returns its width.
#[allow(clippy::too_many_arguments)]
pub fn text_marked(
    c: &mut Canvas,
    fonts: &mut Fonts,
    (x, base): (f32, f32),
    s: &str,
    face: u8,
    size: f32,
    color: u32,
    marks: &[usize],
    mark: u32,
) -> f32 {
    let mut pen = x;
    let mut glyphs = Vec::new();
    crate::bidi::shape(fonts, face, size, s, |start, end, adv, shaped| {
        let lit = marks.iter().any(|&m| m >= start && m < end);
        glyphs.extend(shaped.iter().map(|g| (pen + g.x, g.y, g.face, g.id, lit)));
        pen += adv;
    });
    for (gx, gy, face, id, lit) in glyphs {
        let ix = gx.floor();
        let bmp = fonts.glyph(face, id, size, ((gx - ix) * 4.0) as u8);
        c.glyph(ix as i32, (base - gy) as i32, bmp, if lit { mark } else { color });
    }
    pen - x
}

/// The baseline that centres a line of interface text in a row.
pub fn base(y: f32, h: f32, size: f32) -> f32 {
    (y + h / 2.0 + size * 0.36).round()
}

/// Whether a point, if there is one, lies in a rectangle.
pub fn inside(r: Rect, at: Option<(f32, f32)>) -> bool {
    at.is_some_and(|(x, y)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3)
}

/// Shortens a text with an ellipsis until it fits a width.
pub fn fit(fonts: &mut Fonts, face: u8, size: f32, s: &str, max: f32) -> String {
    if fonts.width(face, size, s) <= max {
        return s.to_string();
    }
    let mut end = s.len();
    while end > 0 {
        end = editor::prev(s, end);
        let cut = format!("{}\u{2026}", s[..end].trim_end());
        if fonts.width(face, size, &cut) <= max {
            return cut;
        }
    }
    String::new()
}

/// Shortens a path from its start, keeping the end, which names it.
pub fn fit_start(fonts: &mut Fonts, face: u8, size: f32, s: &str, max: f32) -> String {
    if fonts.width(face, size, s) <= max {
        return s.to_string();
    }
    let mut start = 0;
    while start < s.len() {
        start = editor::next(s, start);
        let cut = format!("\u{2026}{}", &s[start..]);
        if fonts.width(face, size, &cut) <= max {
            return cut;
        }
    }
    String::new()
}

/// Pixels for a length in logical pixels.
pub fn px(v: f32, scale: f32) -> f32 {
    (v * scale).round()
}

// ---- tooltips -----------------------------------------------------------

thread_local! {
    /// What the controls drawn this frame say of themselves: where each
    /// is, its name and its keys.
    static TIPS: RefCell<Vec<(Rect, String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Names a control drawn at `r`, for its tooltip.
pub fn tip(r: Rect, label: &str, keys: &str) {
    TIPS.with(|t| t.borrow_mut().push((r, label.to_string(), keys.to_string())));
}

pub fn clear_tips() {
    TIPS.with(|t| t.borrow_mut().clear());
}

/// The control under a point that has a tooltip, if any.
pub fn tip_at(x: f32, y: f32) -> Option<(Rect, String, String)> {
    TIPS.with(|t| t.borrow().iter().rev().find(|(r, ..)| inside(*r, Some((x, y)))).cloned())
}

/// Draws a tooltip under a control, or above it where there is no room.
#[allow(clippy::too_many_arguments)]
pub fn tooltip(c: &mut Canvas, fonts: &mut Fonts, anchor: Rect, label: &str, keys: &str, w: f32, h: f32, scale: f32) {
    let size = px(theme::SMALL, scale);
    let pad = px(8.0, scale);
    let gap = px(6.0, scale);
    let tw = fonts.width(font::UI, size, label);
    let kw = if keys.is_empty() { 0.0 } else { kbd_width(fonts, keys, scale) + gap };
    let bw = (tw + kw + pad * 2.0).round();
    let bh = px(26.0, scale);
    let x = (anchor.0 + anchor.2 / 2.0 - bw / 2.0).clamp(px(4.0, scale), (w - bw - px(4.0, scale)).max(0.0)).round();
    let below = anchor.1 + anchor.3 + gap;
    let y = if below + bh <= h { below } else { anchor.1 - gap - bh }.round();
    surface(c, (x, y, bw, bh), px(theme::RADIUS_MD, scale), scale);
    let b = base(y, bh, size);
    text(c, fonts, x + pad, b, label, font::UI, size, theme::fg());
    if !keys.is_empty() {
        kbd(c, fonts, x + pad + tw + gap, y + bh / 2.0, keys, scale);
    }
}

// ---- surfaces -------------------------------------------------------------

/// A raised surface lying over the window: menus, dialogs, tooltips. It
/// casts a soft shadow and has a hairline edge.
pub fn surface(c: &mut Canvas, r: Rect, radius: f32, scale: f32) {
    let (shade, strength) = theme::shadow();
    c.shadow(r.0, r.1, r.2, r.3, radius, px(14.0, scale), px(6.0, scale), shade, strength);
    c.round_rect(r.0, r.1, r.2, r.3, radius, theme::raised());
    c.round_ring(r.0, r.1, r.2, r.3, radius, 1.0, theme::edge());
}

/// A hairline across a surface at `y`, inset from its sides.
pub fn rule(c: &mut Canvas, x: f32, y: f32, w: f32, inset: f32) {
    c.line(x + inset, y, (w - inset * 2.0).max(0.0), 1.0, theme::line());
}

/// The background of a row in a list: the chosen row, or the one under
/// the pointer. With `ring`, the chosen row has the keyboard and is
/// outlined in the focus colour as well.
pub fn row_ground(c: &mut Canvas, r: Rect, chosen: bool, hot: bool, ring: bool, scale: f32) {
    let radius = px(theme::RADIUS_MD, scale);
    if chosen {
        c.round_rect(r.0, r.1, r.2, r.3, radius, theme::active());
    } else if hot {
        c.round_rect(r.0, r.1, r.2, r.3, radius, theme::hover());
    }
    if chosen && ring {
        c.round_ring(r.0, r.1, r.2, r.3, radius, 1.0, theme::primary());
    }
}

/// A scroll bar's thumb along the right edge of a box: `at` and `len` are
/// fractions of the track.
pub fn scrollbar(c: &mut Canvas, track: Rect, at: f32, len: f32, hot: bool, scale: f32) {
    if len >= 1.0 {
        return;
    }
    let w = px(if hot { 8.0 } else { 6.0 }, scale);
    let inset = px(2.0, scale);
    let h = (track.3 * len).max(px(24.0, scale)).min(track.3);
    let y = track.1 + (track.3 - h) * (at / (1.0 - len)).clamp(0.0, 1.0);
    let ink = if hot { theme::faint() } else { theme::mix(theme::bg(), theme::faint(), 110) };
    c.round_rect(track.0 + track.2 - w - inset, y + inset, w, (h - inset * 2.0).max(w), w / 2.0, ink);
}

// ---- controls -------------------------------------------------------------

/// A square ghost button holding an icon, as on the title bar and in the
/// heads of panels. A toggle that is on lies on the active fill.
pub fn icon_button(c: &mut Canvas, r: Rect, icon: Icon, hot: bool, on: bool, scale: f32) {
    let radius = px(theme::RADIUS, scale);
    if on {
        c.round_rect(r.0, r.1, r.2, r.3, radius, theme::active());
    } else if hot {
        c.round_rect(r.0, r.1, r.2, r.3, radius, theme::hover());
    }
    let ink = if hot || on { theme::fg() } else { theme::icon() };
    let size = if r.3 < px(26.0, scale) { theme::SMALL_ICON } else { theme::ICON };
    icons::centred(c, icon, r, px(size, scale), ink);
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Variant {
    /// The one thing a panel or dialog exists to do: blue.
    Primary,
    /// A bordered button beside a primary one.
    Outline,
    /// A quiet button that lights up under the pointer.
    Ghost,
    /// Destroys something.
    Destructive,
}

/// The width a button needs for its label and icon.
pub fn button_width(fonts: &mut Fonts, label: &str, icon: bool, scale: f32) -> f32 {
    let size = px(theme::UI_TEXT, scale);
    let text = if label.is_empty() { 0.0 } else { fonts.width(font::UI_MEDIUM, size, label) };
    let icon =
        if icon { px(theme::SMALL_ICON, scale) + if label.is_empty() { 0.0 } else { px(6.0, scale) } } else { 0.0 };
    (text + icon + px(10.0, scale) * 2.0).round()
}

/// A button with a label and, before it, an icon.
#[allow(clippy::too_many_arguments)]
pub fn button(
    c: &mut Canvas,
    fonts: &mut Fonts,
    r: Rect,
    label: &str,
    icon: Option<Icon>,
    variant: Variant,
    hot: bool,
    scale: f32,
) {
    let radius = px(theme::RADIUS, scale);
    let ink = match variant {
        Variant::Primary => {
            c.round_rect(r.0, r.1, r.2, r.3, radius, if hot { theme::primary_hover() } else { theme::primary() });
            theme::on_primary()
        }
        Variant::Destructive => {
            let fill = if hot { theme::mix(theme::destructive(), 0, 30) } else { theme::destructive() };
            c.round_rect(r.0, r.1, r.2, r.3, radius, fill);
            0xffffff
        }
        Variant::Outline => {
            c.round_rect(r.0, r.1, r.2, r.3, radius, if hot { theme::hover() } else { theme::raised() });
            c.round_ring(r.0, r.1, r.2, r.3, radius, 1.0, theme::field());
            theme::fg()
        }
        Variant::Ghost => {
            if hot {
                c.round_rect(r.0, r.1, r.2, r.3, radius, theme::hover());
            }
            if hot { theme::fg() } else { theme::dim() }
        }
    };
    let size = px(theme::UI_TEXT, scale);
    let tw = if label.is_empty() { 0.0 } else { fonts.width(font::UI_MEDIUM, size, label) };
    let isz = px(theme::SMALL_ICON, scale);
    let gap = if icon.is_some() && !label.is_empty() { px(6.0, scale) } else { 0.0 };
    let total = tw + if icon.is_some() { isz + gap } else { 0.0 };
    let mut x = (r.0 + (r.2 - total) / 2.0).round();
    if let Some(icon) = icon {
        let iink = if variant == Variant::Ghost && !hot { theme::icon() } else { ink };
        icons::draw(c, icon, x, (r.1 + (r.3 - isz) / 2.0).round(), isz, iink);
        x += isz + gap;
    }
    if !label.is_empty() {
        text(c, fonts, x, base(r.1, r.3, size), label, font::UI_MEDIUM, size, ink);
    }
}

/// A round button holding an icon, as the composer's send and stop.
pub fn round_button(c: &mut Canvas, r: Rect, icon: Icon, fill: u32, ink: u32, scale: f32) {
    let d = r.2.min(r.3);
    c.circle(r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, d / 2.0, fill);
    icons::centred(c, icon, r, px(theme::ICON, scale), ink);
}

/// The caps a shortcut is shown in: its first set of keys, one cap each.
fn caps(keys: &str) -> Vec<String> {
    let first = keys.split(", ").next().unwrap_or("").trim();
    if first.is_empty() || first == "none" {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut rest = first;
    while !rest.is_empty() {
        let (part, after) = match rest.find('+') {
            // A `+` standing alone, as in Ctrl++, is the key itself.
            Some(0) => ("+", &rest[1..]),
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };
        out.push(
            match part {
                "Up" => "\u{2191}",
                "Down" => "\u{2193}",
                "Left" => "\u{2190}",
                "Right" => "\u{2192}",
                "Enter" => "Enter",
                "PgUp" => "PgUp",
                "PgDn" => "PgDn",
                p => p,
            }
            .to_string(),
        );
        rest = after;
    }
    out
}

/// The width of a shortcut's key caps.
pub fn kbd_width(fonts: &mut Fonts, keys: &str, scale: f32) -> f32 {
    let size = px(theme::SMALL - 1.0, scale);
    let (pad, gap, min) = (px(4.0, scale), px(3.0, scale), px(18.0, scale));
    let caps = caps(keys);
    let w: f32 = caps.iter().map(|k| (fonts.width(font::UI_MEDIUM, size, k) + pad * 2.0).max(min)).sum();
    w + gap * caps.len().saturating_sub(1) as f32
}

/// Draws a shortcut as key caps from `x`, centred on `mid`. Returns the
/// width used.
pub fn kbd(c: &mut Canvas, fonts: &mut Fonts, x: f32, mid: f32, keys: &str, scale: f32) -> f32 {
    let size = px(theme::SMALL - 1.0, scale);
    let (pad, gap, min, h) = (px(4.0, scale), px(3.0, scale), px(18.0, scale), px(18.0, scale));
    let fill = theme::mix(theme::raised(), theme::fg(), if theme::light() { 16 } else { 20 });
    let mut pen = x.round();
    for (i, cap) in caps(keys).iter().enumerate() {
        if i > 0 {
            pen += gap;
        }
        let tw = fonts.width(font::UI_MEDIUM, size, cap);
        let w = (tw + pad * 2.0).max(min).round();
        let y = (mid - h / 2.0).round();
        c.round_rect(pen, y, w, h, px(theme::RADIUS_SM, scale), fill);
        text(c, fonts, (pen + (w - tw) / 2.0).round(), base(y, h, size), cap, font::UI_MEDIUM, size, theme::dim());
        pen += w;
    }
    pen - x.round()
}

/// A field typed into: the canvas with an edge, which turns blue with a
/// soft ring while it has the keys.
pub fn input(c: &mut Canvas, r: Rect, focused: bool, scale: f32) {
    let radius = px(theme::RADIUS, scale);
    c.round_rect(r.0, r.1, r.2, r.3, radius, theme::bg());
    if focused {
        let ring = theme::mix(theme::bg(), theme::primary(), 70);
        c.round_ring(r.0 - 2.0, r.1 - 2.0, r.2 + 4.0, r.3 + 4.0, radius + 2.0, 2.0, ring);
        c.round_ring(r.0, r.1, r.2, r.3, radius, 1.0, theme::primary());
    } else {
        c.round_ring(r.0, r.1, r.2, r.3, radius, 1.0, theme::field());
    }
}

/// A caret in a field: a bar `h` tall around a row's middle.
pub fn caret(c: &mut Canvas, x: f32, mid: f32, h: f32, scale: f32) {
    INPUT_CARET.with(|r| *r.borrow_mut() = Some((x,mid-h/2.0,1.0,h)));
    let w = px(1.5, scale).max(1.0);
    c.rect(x.round(), (mid - h / 2.0).round(), w, h.round(), theme::caret());
}

/// The size of a switch.
pub fn switch_size(scale: f32) -> (f32, f32) {
    (px(30.0, scale), px(18.0, scale))
}

/// An on and off switch with its top left at `(x, y)`.
pub fn switch(c: &mut Canvas, x: f32, y: f32, on: bool, scale: f32) {
    let (w, h) = switch_size(scale);
    let track = if on { theme::primary() } else { theme::field() };
    c.round_rect(x, y, w, h, h / 2.0, track);
    let k = h - px(4.0, scale);
    let kx = if on { x + w - k - px(2.0, scale) } else { x + px(2.0, scale) };
    let knob = if on || theme::light() { 0xffffff } else { theme::dim() };
    c.circle(kx + k / 2.0, y + h / 2.0, k / 2.0, knob);
}

/// A small label on a quiet fill of its own colour. Returns its width.
#[allow(clippy::too_many_arguments)]
pub fn badge(c: &mut Canvas, fonts: &mut Fonts, x: f32, mid: f32, label: &str, color: u32, scale: f32) -> f32 {
    let size = px(theme::SMALL - 1.0, scale);
    let pad = px(5.0, scale);
    let h = px(18.0, scale);
    let w = (fonts.width(font::UI_MEDIUM, size, label) + pad * 2.0).round();
    let y = (mid - h / 2.0).round();
    c.round_rect(x, y, w, h, px(theme::RADIUS_SM, scale), theme::wash(color));
    text(c, fonts, x + pad, base(y, h, size), label, font::UI_MEDIUM, size, color);
    w
}

// ---- lists to pick from ---------------------------------------------------

/// One choice of a list.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub key: String,
    pub label: String,
    /// Quieter text: a folder for files, a price for models.
    pub detail: String,
    /// A shortcut, shown as key caps.
    pub keys: String,
    pub icon: Option<Icon>,
    /// A file name whose icon the icon theme draws, in place of `icon`.
    pub file: Option<String>,
}

impl Entry {
    pub fn new(key: &str, label: &str, detail: &str) -> Entry {
        Entry { key: key.into(), label: label.into(), detail: detail.into(), ..Entry::default() }
    }

    pub fn keys(mut self, keys: impl Into<String>) -> Entry {
        self.keys = keys.into();
        self
    }

    pub fn icon(mut self, icon: Icon) -> Entry {
        self.icon = Some(icon);
        self
    }
}

/// What a key or click did to a menu.
#[derive(PartialEq, Debug)]
pub enum Picked {
    None,
    Close,
    Key(String),
    /// Delete was pressed on an entry.
    Remove(String),
}

/// An entry that the filter left, with its score and the characters
/// matched in its label and its detail.
#[derive(Clone)]
struct Shown {
    index: usize,
    label: Vec<usize>,
    detail: Vec<usize>,
}

/// A list to pick one entry from, narrowed by typing: the command
/// palette, and the lists that drop from controls.
pub struct Menu {
    /// What the list is of, shown at the right of its field.
    pub title: String,
    /// What the field says until something is typed.
    pub hint: String,
    pub entries: Vec<Entry>,
    /// Where the menu drops from, for one that belongs to a control. One
    /// without it stands near the top of the window, as the palette does.
    pub anchor: Option<(f32, f32)>,
    /// The menu rises from its anchor rather than dropping from it, for a
    /// control at the bottom of the window.
    pub rise: bool,
    /// The entries are what the typing asked for, found elsewhere, and so
    /// are all shown rather than narrowed by it.
    pub live: bool,
    /// How many bytes at the start of what is typed choose what is listed
    /// rather than narrow it, as the palette's `>`.
    pub prefix: usize,
    /// Each entry's detail is the folder of a file it names: the two are
    /// matched as one path, and the detail follows the label.
    pub paths: bool,
    /// The icon in the field.
    pub icon: Icon,
    /// Keys and what they do, under the list.
    pub footer: Vec<(String, String)>,
    /// Width in logical pixels.
    pub width: f32,
    /// The entry that stands now, marked with a tick unless `tick` is off.
    current: String,
    pub tick: bool,
    sel: usize,
    filter: String,
    top: usize,
    /// What the filter left, and what it was computed for.
    shown: Vec<Shown>,
    shown_for: Option<(String, usize, usize)>,
    /// Where the rows were last drawn: left, top, width, row height, count.
    rows: (f32, f32, f32, f32, usize),
    panel: Rect,
    hover: Option<(f32, f32)>,
}

const ROWS: usize = 12;

impl Menu {
    pub(crate) fn input_text(&mut self) -> &mut String { &mut self.filter }
    pub(crate) fn input_changed(&mut self) { self.sel = 0; }
    pub fn new(title: &str, entries: Vec<Entry>, current: &str) -> Menu {
        let sel = entries.iter().position(|e| e.key == current).unwrap_or(0);
        Menu {
            title: title.into(),
            hint: "Filter\u{2026}".into(),
            entries,
            anchor: None,
            rise: false,
            live: false,
            prefix: 0,
            paths: false,
            icon: Icon::Search,
            footer: Vec::new(),
            width: 460.0,
            current: current.to_string(),
            tick: true,
            sel,
            filter: String::new(),
            top: 0,
            shown: Vec::new(),
            shown_for: None,
            rows: (0.0, 0.0, 0.0, 0.0, 0),
            panel: (0.0, 0.0, 0.0, 0.0),
            hover: None,
        }
    }

    /// Replaces the entries, keeping the choice at the top.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.shown_for = None;
        self.sel = 0;
        self.top = 0;
    }

    /// The part of what is typed that narrows the list.
    fn query(&self) -> &str {
        self.filter.get(self.prefix.min(self.filter.len())..).unwrap_or("")
    }

    /// Works out what the filter leaves, best first, unless that is known.
    fn refresh(&mut self) {
        let stamp = (self.filter.clone(), self.prefix, self.entries.len());
        if self.shown_for.as_ref() == Some(&stamp) {
            return;
        }
        let query = self.query().trim().to_string();
        let mut shown: Vec<(i32, Shown)> = Vec::new();
        for (index, e) in self.entries.iter().enumerate() {
            if self.live || query.is_empty() {
                shown.push((0, Shown { index, label: Vec::new(), detail: Vec::new() }));
                continue;
            }
            let found = if self.paths {
                let dir = if e.detail.is_empty() { String::new() } else { format!("{}/", e.detail) };
                fuzzy::score_path(&query, &format!("{dir}{}", e.label)).map(|(s, hits)| {
                    let split = dir.len();
                    let label = hits.iter().filter(|&&h| h >= split).map(|h| h - split).collect();
                    let detail = hits.iter().copied().filter(|&h| h < split).collect();
                    (s, label, detail)
                })
            } else {
                let label = fuzzy::score(&query, &e.label).map(|(s, h)| (s + 20, h, Vec::new()));
                let other = || {
                    let hay = format!("{} {}", e.detail, e.key);
                    fuzzy::score(&query, &hay).map(|(s, _)| (s - 30, Vec::new(), Vec::new()))
                };
                label.or_else(other)
            };
            if let Some((s, label, detail)) = found {
                shown.push((s, Shown { index, label, detail }));
            }
        }
        if !query.is_empty() && !self.live {
            shown.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.index.cmp(&b.1.index)));
        }
        self.shown = shown.into_iter().map(|(_, s)| s).collect();
        self.shown_for = Some(stamp);
        self.sel = self.sel.min(self.shown.len().saturating_sub(1));
    }

    fn pick(&mut self, at: usize) -> Picked {
        self.refresh();
        self.shown.get(at).map_or(Picked::None, |s| Picked::Key(self.entries[s.index].key.clone()))
    }

    /// The entry chosen, if any.
    pub fn chosen(&mut self) -> Option<&Entry> {
        self.refresh();
        self.shown.get(self.sel).map(|s| &self.entries[s.index])
    }

    /// What has been typed.
    pub fn typed(&self) -> &str {
        &self.filter
    }

    /// Types text, as if its keys were pressed.
    pub fn type_text(&mut self, s: &str) {
        self.filter += s;
        self.sel = 0;
    }

    /// Replaces what has been typed.
    pub fn set_typed(&mut self, s: &str) {
        self.filter = s.to_string();
        self.sel = 0;
    }

    pub fn key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> Picked {
        self.refresh();
        let n = self.shown.len();
        let page = self.rows.4.max(1);
        match key {
            Key::Escape => return Picked::Close,
            Key::Enter => return self.pick(self.sel),
            Key::Delete => {
                if let Picked::Key(k) = self.pick(self.sel) {
                    return Picked::Remove(k);
                }
            }
            Key::Up => self.sel = if self.sel == 0 { n.saturating_sub(1) } else { self.sel - 1 },
            Key::Down => self.sel = if self.sel + 1 >= n { 0 } else { self.sel + 1 },
            Key::Char('p') if mods.ctrl && !mods.shift => self.sel = self.sel.saturating_sub(1),
            Key::Char('n') if mods.ctrl && !mods.shift => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Home => self.sel = 0,
            Key::End => self.sel = n.saturating_sub(1),
            Key::PageUp => self.sel = self.sel.saturating_sub(page),
            Key::PageDown => self.sel = (self.sel + page).min(n.saturating_sub(1)),
            Key::Backspace if mods.ctrl => {
                let keep = editor::word_left(&self.filter, self.filter.len());
                self.filter.truncate(keep);
                self.sel = 0;
            }
            Key::Backspace => {
                self.filter.pop();
                self.sel = 0;
            }
            _ => {
                let typed = text.filter(|s| !mods.ctrl && !mods.alt && !s.chars().any(char::is_control));
                if let Some(s) = typed {
                    self.filter += s;
                    self.sel = 0;
                }
            }
        }
        Picked::None
    }

    pub fn scroll(&mut self, steps: f32) {
        self.refresh();
        let n = self.shown.len();
        let top = (self.top as f32 + steps).max(0.0) as usize;
        let count = self.rows.4.max(1);
        self.top = top.min(n.saturating_sub(count));
        self.sel = self.sel.clamp(self.top, (self.top + count - 1).min(n.saturating_sub(1)));
    }

    /// The row under the pointer becomes the chosen one.
    pub fn motion(&mut self, x: f32, y: f32) {
        self.hover = Some((x, y));
        let (rx, ry, rw, rh, n) = self.rows;
        if rh > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh * n as f32 {
            self.sel = self.top + ((y - ry) / rh) as usize;
        }
    }

    /// A click picks the row under it; one outside the menu closes it.
    pub fn press(&mut self, x: f32, y: f32) -> Picked {
        let (px, py, pw, ph) = self.panel;
        if x < px || x >= px + pw || y < py || y >= py + ph {
            return Picked::Close;
        }
        let (rx, ry, rw, rh, n) = self.rows;
        if rh <= 0.0 || x < rx || x >= rx + rw || y < ry {
            return Picked::None;
        }
        let i = ((y - ry) / rh) as usize;
        if i < n { self.pick(self.top + i) } else { Picked::None }
    }

    /// Draws the menu in a `w` by `h` window.
    pub fn draw(&mut self, c: &mut Canvas, fonts: &mut Fonts, w: f32, h: f32, scale: f32) {
        self.refresh();
        let palette = self.anchor.is_none();
        let size = px(if palette { theme::LARGE } else { theme::UI_TEXT }, scale);
        let small = px(theme::SMALL, scale);
        let row_h = px(if palette { 32.0 } else { theme::MENU_ROW }, scale);
        let field_h = px(if palette { 46.0 } else { 38.0 }, scale);
        let foot_h = if self.footer.is_empty() { 0.0 } else { px(34.0, scale) };
        let inset = px(6.0, scale);
        let margin = px(12.0, scale);
        let top_room = if palette { px(theme::TITLE + 6.0, scale) } else { 0.0 };
        let room = (h - top_room - field_h - foot_h - inset * 2.0 - margin).max(row_h);
        let capacity = ((room / row_h).floor() as usize).clamp(1, if palette { 13 } else { ROWS });
        let n = self.shown.len();
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + capacity {
            self.top = self.sel + 1 - capacity;
        }
        self.top = self.top.min(n.saturating_sub(capacity));
        let count = n.min(capacity);
        let pw = px(self.width, scale).min(w - margin * 2.0).max(px(160.0, scale)).round();
        let list_h = row_h * count.max(1) as f32 + inset * 2.0;
        let ph = field_h + list_h + foot_h;
        let (px_, py) = match self.anchor {
            Some((x, y)) => (
                x.min(w - pw - px(4.0, scale)).max(px(4.0, scale)).round(),
                if self.rise {
                    (y - ph - px(4.0, scale)).max(0.0)
                } else {
                    (y + px(4.0, scale)).min((h - ph).max(0.0))
                }
                .round(),
            ),
            // The palette keeps its place whatever it lists.
            None => (((w - pw) / 2.0).round(), top_room.max((h * 0.08).min(px(72.0, scale))).round()),
        };
        self.panel = (px_, py, pw, ph);
        let radius = px(if palette { theme::RADIUS_2XL } else { theme::RADIUS_XL }, scale);
        surface(c, self.panel, radius, scale);

        // The field: an icon, what is typed or the hint, and the title.
        let pad = px(14.0, scale);
        let isz = px(theme::ICON, scale);
        icons::draw(c, self.icon, px_ + pad, (py + (field_h - isz) / 2.0).round(), isz, theme::icon());
        let tx = px_ + pad + isz + px(10.0, scale);
        let b = base(py, field_h, size);
        let title_w = if self.title.is_empty() || self.hint.is_empty() {
            0.0
        } else {
            fonts.width(font::UI_MEDIUM, small, &self.title) + pad
        };
        let room = pw - (tx - px_) - pad - title_w;
        if self.filter.is_empty() {
            let (s, face, ink) = if self.hint.is_empty() {
                (&self.title, font::UI_MEDIUM, theme::fg())
            } else {
                (&self.hint, font::UI, theme::faint())
            };
            let s = fit(fonts, face, size, s, room);
            text(c, fonts, tx, b, &s, face, size, ink);
            caret(c, tx, py + field_h / 2.0, size * 1.25, scale);
        } else {
            let shown = fit_start(fonts, font::UI, size, &self.filter, room - px(4.0, scale));
            let tw = text(c, fonts, tx, b, &shown, font::UI, size, theme::fg());
            let caret_x = crate::bidi::caret_x(fonts,font::UI,size,&shown,shown.len());
            caret(c, tx + caret_x + 1.0, py + field_h / 2.0, size * 1.25, scale);
            // After a prefix, what to type next.
            if self.query().is_empty() && !self.hint.is_empty() {
                let hint = fit(fonts, font::UI, size, &self.hint, room - tw - px(8.0, scale));
                text(c, fonts, tx + tw + px(6.0, scale), b, &hint, font::UI, size, theme::faint());
            }
        }
        if title_w > 0.0 {
            let tw = fonts.width(font::UI_MEDIUM, small, &self.title);
            text(
                c,
                fonts,
                px_ + pw - pad - tw,
                base(py, field_h, small),
                &self.title,
                font::UI_MEDIUM,
                small,
                theme::dim(),
            );
        }
        rule(c, px_, py + field_h, pw, 0.0);

        // The rows.
        let ry = py + field_h + inset;
        let (rx, rw) = (px_ + inset, pw - inset * 2.0);
        self.rows = (rx, ry, rw, row_h, count);
        c.clip(px_, ry, pw, list_h - inset * 2.0);
        let hot = self.hover;
        for i in 0..count {
            let s = &self.shown[self.top + i];
            let e = &self.entries[s.index];
            let r = (rx, ry + row_h * i as f32, rw, row_h);
            let chosen = self.top + i == self.sel;
            row_ground(c, r, chosen, !chosen && inside(r, hot), false, scale);
            let mut x = r.0 + px(10.0, scale);
            if let Some(name) = &e.file {
                let isz = px(theme::ICON, scale);
                let kind = crate::icon_theme::Kind::File;
                crate::icon_theme::draw(c, name, kind, x, (r.1 + (row_h - isz) / 2.0).round(), isz, theme::icon());
                x += isz + px(10.0, scale);
            } else if let Some(icon) = e.icon {
                let isz = px(theme::ICON, scale);
                icons::draw(c, icon, x, (r.1 + (row_h - isz) / 2.0).round(), isz, theme::icon());
                x += isz + px(10.0, scale);
            }
            let mut right = r.0 + r.2 - px(10.0, scale);
            if self.tick && !self.current.is_empty() && e.key == self.current {
                let isz = px(theme::SMALL_ICON, scale);
                icons::draw(c, Icon::Check, right - isz, (r.1 + (row_h - isz) / 2.0).round(), isz, theme::fg());
                right -= isz + px(10.0, scale);
            }
            let kw = if e.keys.is_empty() { 0.0 } else { kbd_width(fonts, &e.keys, scale) };
            let mut end = right;
            if kw > 0.0 {
                kbd(c, fonts, right - kw, r.1 + row_h / 2.0, &e.keys, scale);
                end -= kw + px(10.0, scale);
            }
            let b = base(r.1, row_h, size);
            let face = font::UI;
            let ink = theme::fg();
            if self.paths {
                let lw = fonts.width(face, size, &e.label);
                let label = fit(fonts, face, size, &e.label, end - x);
                let used = text_marked(c, fonts, (x, b), &label, face, size, ink, &s.label, theme::link());
                if lw <= end - x && !e.detail.is_empty() {
                    let dx = x + used + px(8.0, scale);
                    let room = end - dx;
                    if room > px(30.0, scale) {
                        let detail = fit_start(fonts, font::UI, small, &e.detail, room);
                        let marks: Vec<usize> = if detail == e.detail { s.detail.clone() } else { Vec::new() };
                        text_marked(c, fonts, (dx, b), &detail, font::UI, small, theme::dim(), &marks, theme::link());
                    }
                }
            } else {
                let dw = if e.detail.is_empty() {
                    0.0
                } else {
                    let detail = fit(fonts, font::UI, small, &e.detail, ((end - x) * 0.45).max(0.0));
                    let dw = fonts.width(font::UI, small, &detail);
                    text(c, fonts, end - dw, b, &detail, font::UI, small, theme::dim());
                    dw + px(12.0, scale)
                };
                let label = fit(fonts, face, size, &e.label, (end - x - dw).max(0.0));
                let marks: &[usize] = if label == e.label { &s.label } else { &[] };
                text_marked(c, fonts, (x, b), &label, face, size, ink, marks, theme::link());
            }
        }
        c.unclip();
        if n > count && count > 0 {
            let track = (px_, ry, pw, row_h * count as f32);
            scrollbar(c, track, self.top as f32 / n as f32, count as f32 / n as f32, false, scale);
        }
        if self.shown.is_empty() {
            let said = if self.live && self.query().trim().is_empty() { "" } else { "No results" };
            let sw = fonts.width(font::UI, size, said);
            text(c, fonts, px_ + (pw - sw) / 2.0, base(ry, row_h, size), said, font::UI, size, theme::dim());
        }

        // The keys that work here.
        if foot_h > 0.0 {
            let fy = py + ph - foot_h;
            rule(c, px_, fy, pw, 0.0);
            let mut x = px_ + pad;
            let mid = fy + foot_h / 2.0;
            for (keys, does) in &self.footer {
                let need = kbd_width(fonts, keys, scale) + px(6.0, scale) + fonts.width(font::UI, small, does);
                if x + need > px_ + pw - pad {
                    break;
                }
                x += kbd(c, fonts, x, mid, keys, scale) + px(6.0, scale);
                x += text(c, fonts, x, base(fy, foot_h, small), does, font::UI, small, theme::dim()) + px(16.0, scale);
            }
        }
    }
}

/// One line of a context menu: something to do, or a rule between groups.
pub enum Item {
    Do { key: String, label: String, detail: String, enabled: bool },
    Rule,
}

impl Item {
    pub fn new(key: &str, label: &str, detail: impl Into<String>) -> Item {
        Item::Do { key: key.into(), label: label.into(), detail: detail.into(), enabled: true }
    }

    /// The same, greyed out unless `enabled`.
    pub fn when(mut self, on: bool) -> Item {
        if let Item::Do { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }

    fn live(&self) -> bool {
        matches!(self, Item::Do { enabled: true, .. })
    }
}

/// A menu of what can be done with what was right-clicked, standing where
/// the pointer is. The row under the pointer lights up; arrow keys and
/// Enter work in it too.
pub struct Popup {
    items: Vec<Item>,
    at: (f32, f32),
    sel: Option<usize>,
    panel: Rect,
    /// Where each line was last drawn: its top and height.
    places: Vec<(f32, f32)>,
    scroll: f32,
    tall: f32,
    reveal: bool,
}

impl Popup {
    pub fn new(mut items: Vec<Item>, at: (f32, f32)) -> Popup {
        // Rules only ever stand between things to do.
        items.dedup_by(|a, b| matches!((a, b), (Item::Rule, Item::Rule)));
        while matches!(items.last(), Some(Item::Rule)) {
            items.pop();
        }
        while matches!(items.first(), Some(Item::Rule)) {
            items.remove(0);
        }
        Popup {
            items,
            at,
            sel: None,
            panel: (0.0, 0.0, 0.0, 0.0),
            places: Vec::new(),
            scroll: 0.0,
            tall: 0.0,
            reveal: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.items.iter().any(Item::live)
    }

    fn pick(&self, i: usize) -> Picked {
        match self.items.get(i) {
            Some(Item::Do { key, enabled: true, .. }) => Picked::Key(key.clone()),
            _ => Picked::None,
        }
    }

    /// The next line that can be chosen, up or down from the chosen one.
    fn step(&self, down: bool) -> Option<usize> {
        let n = self.items.len();
        let mut at = self.sel.unwrap_or(if down { n - 1 } else { 0 });
        for _ in 0..n {
            at = if down { (at + 1) % n } else { (at + n - 1) % n };
            if self.items[at].live() {
                return Some(at);
            }
        }
        None
    }

    pub fn key(&mut self, key: Key) -> Picked {
        self.reveal = true;
        match key {
            Key::Escape => return Picked::Close,
            Key::Enter | Key::Char(' ') => return self.sel.map_or(Picked::None, |i| self.pick(i)),
            Key::Up | Key::Down | Key::Tab => self.sel = self.step(key != Key::Up),
            Key::Home | Key::End => {
                self.sel = None;
                self.sel = self.step(key == Key::Home);
            }
            _ => {}
        }
        Picked::None
    }

    fn line_at(&self, x: f32, y: f32) -> Option<usize> {
        let (px, py, pw, ph) = self.panel;
        if x < px || x >= px + pw || y < py || y >= py + ph {
            return None;
        }
        self.places.iter().position(|&(top, h)| y >= top && y < top + h)
    }

    /// The line under the pointer becomes the chosen one, if it can be.
    pub fn motion(&mut self, x: f32, y: f32) {
        self.sel = self.line_at(x, y).filter(|&i| self.items[i].live());
    }

    pub fn leave(&mut self) {
        self.sel = None;
    }

    pub fn scroll(&mut self, dy: f32) {
        self.scroll = (self.scroll + dy).clamp(0.0, (self.tall - self.panel.3).max(0.0));
        self.reveal = false;
        self.sel = None;
    }

    /// A click does what is under it; one outside the menu closes it.
    pub fn press(&mut self, x: f32, y: f32) -> Picked {
        let (px, py, pw, ph) = self.panel;
        if x < px || x >= px + pw || y < py || y >= py + ph {
            return Picked::Close;
        }
        self.line_at(x, y).map_or(Picked::None, |i| self.pick(i))
    }

    /// The middle of a row, by its label, for clicking it.
    #[cfg(test)]
    pub fn point(&self, name: &str) -> (f32, f32) {
        let i = self.items.iter().position(|i| matches!(i, Item::Do { label, .. } if label == name)).unwrap();
        let (top, h) = self.places[i];
        (self.panel.0 + self.panel.2 / 2.0, top + h / 2.0)
    }

    /// Draws the menu in a `w` by `h` window, beside the place it was
    /// opened at, turned back where it would run off the window.
    pub fn draw(&mut self, c: &mut Canvas, fonts: &mut Fonts, w: f32, h: f32, scale: f32) {
        let size = px(theme::UI_TEXT, scale);
        let small = px(theme::SMALL, scale);
        let row_h = px(theme::ITEM, scale);
        let rule_h = px(9.0, scale);
        let pad = px(10.0, scale);
        let inset = px(4.0, scale);
        let mut widest = 0.0f32;
        for item in &self.items {
            if let Item::Do { label, detail, .. } = item {
                let dw = if detail.is_empty() { 0.0 } else { fonts.width(font::UI, small, detail) + px(24.0, scale) };
                widest = widest.max(fonts.width(font::UI, size, label) + dw);
            }
        }
        let pw = (widest + pad * 2.0 + inset * 2.0).max(px(200.0, scale)).min(w).round();
        let height = |i: &Item| if matches!(i, Item::Rule) { rule_h } else { row_h };
        self.tall = self.items.iter().map(height).sum::<f32>() + inset * 2.0;
        let ph = self.tall.min((h - px(16.0, scale)).max(row_h));
        if std::mem::take(&mut self.reveal)
            && let Some(sel) = self.sel
        {
            let at = self.items[..sel].iter().map(height).sum::<f32>();
            self.scroll = self.scroll.max(at + height(&self.items[sel]) + inset * 2.0 - ph).min(at);
        }
        self.scroll = self.scroll.clamp(0.0, (self.tall - ph).max(0.0));
        let (ax, ay) = self.at;
        let px_ = if ax + pw > w { (ax - pw).max(0.0) } else { ax }.round();
        let py = if ay + ph > h { (ay - ph).max(0.0) } else { ay }.round();
        self.panel = (px_, py, pw, ph);
        surface(c, self.panel, px(theme::RADIUS, scale), scale);
        c.clip(px_ + 1.0, py + inset, pw - 2.0, ph - inset * 2.0);
        self.places.clear();
        let mut y = py + inset - self.scroll;
        for (i, item) in self.items.iter().enumerate() {
            match item {
                Item::Rule => {
                    self.places.push((y, rule_h));
                    c.line(px_ + inset * 2.0, y + (rule_h / 2.0).floor(), pw - inset * 4.0, 1.0, theme::line());
                    y += rule_h;
                }
                Item::Do { label, detail, enabled, .. } => {
                    self.places.push((y, row_h));
                    let r = (px_ + inset, y, pw - inset * 2.0, row_h);
                    let on = *enabled && self.sel == Some(i);
                    if on {
                        c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_SM, scale), theme::active());
                    }
                    let b = base(y, row_h, size);
                    let ink = if *enabled { theme::fg() } else { theme::faint() };
                    let dw = if detail.is_empty() { 0.0 } else { fonts.width(font::UI, small, detail) };
                    let label = fit(fonts, font::UI, size, label, r.2 - pad * 2.0 - dw - px(12.0, scale));
                    text(c, fonts, r.0 + pad, b, &label, font::UI, size, ink);
                    if dw > 0.0 {
                        let dink = if *enabled { theme::dim() } else { theme::faint() };
                        text(c, fonts, r.0 + r.2 - pad - dw, base(y, row_h, small), detail, font::UI, small, dink);
                    }
                    y += row_h;
                }
            }
        }
        c.unclip();
        if self.tall > ph {
            let track = (px_, py + inset, pw, ph - inset * 2.0);
            scrollbar(c, track, self.scroll / self.tall, ph / self.tall, false, scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_menus_reveal_and_hit_the_keyboard_choice_at_minimum_size() {
        let mut fonts = Fonts::new();
        let mut px = vec![0; 320 * 200];
        let mut canvas = Canvas::new(&mut px, 320, 200);
        let entries = (0..20).map(|i| Entry::new(&i.to_string(), &format!("Item {i}"), "")).collect();
        let mut menu = Menu::new("Choose", entries, "");
        menu.anchor = Some((0.0, 40.0));
        menu.key(Key::End, Mods::default(), None);
        menu.draw(&mut canvas, &mut fonts, 320.0, 200.0, 1.0);
        assert!(menu.panel.1 + menu.panel.3 <= 200.0);
        let (x, y, w, rh, count) = menu.rows;
        assert_eq!(menu.press(x + w / 2.0, y + rh * (count as f32 - 0.5)), Picked::Key("19".into()));

        let items = (0..20).map(|i| Item::new(&i.to_string(), &format!("Item {i}"), "")).collect();
        let mut popup = Popup::new(items, (30.0, 100.0));
        popup.key(Key::End);
        popup.draw(&mut canvas, &mut fonts, 320.0, 200.0, 1.0);
        assert!(popup.panel.1 + popup.panel.3 <= 200.0);
        let (x, y) = popup.point("Item 19");
        assert_eq!(popup.press(x, y), Picked::Key("19".into()));
        popup.scroll(-10_000.0);
        popup.draw(&mut canvas, &mut fonts, 320.0, 200.0, 1.0);
        let (x, y) = popup.point("Item 0");
        assert_eq!(popup.press(x, y), Picked::Key("0".into()));
    }

    #[test]
    fn typing_narrows_fuzzily_and_puts_the_best_first() {
        let names = ["Save as", "Find and replace", "File: rename", "Search the project"];
        let entries = names.iter().map(|n| Entry::new(n, n, "")).collect();
        let mut menu = Menu::new("", entries, "");
        for ch in "fr".chars() {
            menu.key(Key::Char(ch), Mods::default(), Some(&ch.to_string()));
        }
        let keys: Vec<String> = {
            menu.refresh();
            menu.shown.iter().map(|s| menu.entries[s.index].key.clone()).collect()
        };
        assert_eq!(keys[0], "File: rename");
        assert!(!keys.contains(&"Save as".to_string()));
        // A prefix chooses what is listed and does not narrow it.
        menu.set_typed(">");
        menu.prefix = 1;
        assert_eq!(menu.chosen().map(|e| e.key.clone()).as_deref(), Some("Save as"));
    }

    #[test]
    fn shortcuts_become_key_caps() {
        assert_eq!(caps("Ctrl+Shift+P"), ["Ctrl", "Shift", "P"]);
        assert_eq!(caps("Ctrl++, Ctrl+="), ["Ctrl", "+"]);
        assert_eq!(caps("none"), Vec::<String>::new());
        assert_eq!(caps("Shift+Up"), ["Shift", "\u{2191}"]);
    }
}
