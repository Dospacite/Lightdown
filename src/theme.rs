//! Colour roles and scale-aware metrics for every panel, after T3 Code.
//!
//! Surfaces are flat and separated by hairline borders, not gaps. The dark
//! theme is true black with raised surfaces (menus, dialogs, the composer)
//! a step lighter. Colour is almost all neutral: one blue marks actions and
//! focus, and green, amber and red are kept for state.

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

/// Every colour role. Theme files name them as these fields are named;
/// `ROLES` lists them with the role each falls back to.
#[derive(Clone, PartialEq, Debug)]
pub struct Palette {
    /// The canvas: editor, sidebars, panels, title and status bars.
    pub bg: u32,
    /// Raised surfaces: menus, dialogs, the composer, tooltips.
    pub raised: u32,
    /// Code blocks and other insets within the canvas.
    pub inset: u32,
    /// Inline code and the user's messages in the agent's thread.
    pub tint: u32,
    pub fg: u32,
    pub bright: u32,
    /// Supporting text: labels, metadata, inactive tabs.
    pub dim: u32,
    /// Hints, placeholders, line numbers.
    pub faint: u32,
    /// Icons at rest.
    pub icon: u32,
    pub quote: u32,
    /// Hairlines between panels and around raised surfaces.
    pub line: u32,
    /// The edge of a raised surface, which lies on the canvas.
    pub edge: u32,
    /// The edge of a field typed into.
    pub field: u32,
    /// Hover on rows and ghost buttons.
    pub hover: u32,
    /// The chosen row, a pressed toggle, the active tab.
    pub active: u32,
    /// Blue: the main action, focus rings, the on switch.
    pub primary: u32,
    pub primary_hover: u32,
    pub on_primary: u32,
    /// Selected text.
    pub selection: u32,
    /// Matches of a search, other than the one chosen.
    pub found: u32,
    pub caret: u32,
    /// Links and highlighted characters in lists.
    pub link: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    /// Fill of destructive buttons, under white text.
    pub destructive: u32,
    pub info: u32,
    pub keyword: u32,
    pub string: u32,
    pub number: u32,
    pub function: u32,
    pub special: u32,
    // Tree-sitter identifier roles.
    pub parameter: u32,
    pub property: u32,
    /// ANSI blue in the terminal, which has no other role to borrow.
    pub term_blue: u32,
    /// The terminal's sixteen ANSI colours: black, red, green, yellow,
    /// blue, magenta, cyan and white, then their bright forms.
    pub ansi: [u32; 16],
    /// The window's shadow colour and how strong it is, out of 255.
    pub shadow: (u32, u32),
    /// How far a modal dims what is under it, out of 255.
    pub scrim: u32,
}

pub const DARK: Palette = Palette {
    bg: 0x000000,
    raised: 0x111111,
    inset: 0x0d0d0d,
    tint: 0x1e1e1e,
    fg: 0xf5f5f5,
    bright: 0xffffff,
    dim: 0xa1a1a1,
    faint: 0x8a8a8a,
    icon: 0x8b8b8b,
    quote: 0xc4c4c4,
    line: 0x1c1c1c,
    edge: 0x262626,
    field: 0x2c2c2c,
    hover: 0x141414,
    active: 0x1f1f1f,
    primary: 0x346bf1,
    primary_hover: 0x2f60d9,
    on_primary: 0xffffff,
    selection: 0x1d3a75,
    found: 0x3a3214,
    caret: 0xb4cbff,
    link: 0x51a2ff,
    success: 0x00d492,
    warning: 0xffb900,
    danger: 0xff6467,
    destructive: 0xe5343d,
    info: 0x51a2ff,
    keyword: 0xc8aaff,
    string: 0xa9d79c,
    number: 0xf1b57e,
    function: 0x82d1da,
    special: 0xe7a6cb,
    parameter: 0xe4c18b,
    property: 0x9cc8f0,
    term_blue: 0x8cb4f5,
    ansi: [
        0x000000, 0xff6467, 0x00d492, 0xffb900, 0x8cb4f5, 0xc8aaff, 0x82d1da, 0xf5f5f5, 0x8a8a8a, 0xff6467, 0x00d492,
        0xffb900, 0x8cb4f5, 0xe7a6cb, 0x82d1da, 0xffffff,
    ],
    shadow: (0x000000, 200),
    scrim: 150,
};

pub const LIGHT: Palette = Palette {
    bg: 0xfcfcfc,
    raised: 0xffffff,
    inset: 0xf4f4f5,
    tint: 0xebebed,
    fg: 0x18181b,
    bright: 0x09090b,
    dim: 0x5c5c66,
    faint: 0x6b6b75,
    icon: 0x6e6e78,
    quote: 0x3f3f46,
    line: 0xe4e4e7,
    edge: 0xe4e4e7,
    field: 0xd4d4d8,
    hover: 0xf2f2f3,
    active: 0xe8e8ea,
    primary: 0x1b4ed8,
    primary_hover: 0x3160db,
    on_primary: 0xffffff,
    selection: 0xcddcfc,
    found: 0xfbe9a8,
    caret: 0x1b4ed8,
    link: 0x1447e6,
    success: 0x006d4c,
    warning: 0xa04200,
    danger: 0xc10007,
    destructive: 0xdc2626,
    info: 0x1447e6,
    keyword: 0x6a3d9e,
    string: 0x2e6829,
    number: 0x8e450d,
    function: 0x0e6370,
    special: 0x92336d,
    parameter: 0x795020,
    property: 0x225f99,
    term_blue: 0x2a58a8,
    ansi: [
        0xfcfcfc, 0xc10007, 0x006d4c, 0xa04200, 0x2a58a8, 0x6a3d9e, 0x0e6370, 0x18181b, 0x6b6b75, 0xc10007, 0x006d4c,
        0xa04200, 0x2a58a8, 0x92336d, 0x0e6370, 0x09090b,
    ],
    shadow: (0x000000, 46),
    scrim: 70,
};

static IS_LIGHT: AtomicBool = AtomicBool::new(false);
/// A theme loaded from a file, or null for the built-in pair.
static CURRENT: AtomicPtr<Palette> = AtomicPtr::new(std::ptr::null_mut());

/// Shows the built-in dark or light theme.
pub fn set_light(light: bool) {
    CURRENT.store(std::ptr::null_mut(), Ordering::Relaxed);
    IS_LIGHT.store(light, Ordering::Relaxed);
}

/// Shows a palette, light or dark by its appearance. Palettes are kept for
/// the life of the process, once each, so they can be read without a lock.
pub fn set_palette(p: &Palette, light: bool) {
    static KEPT: std::sync::Mutex<Vec<&'static Palette>> = std::sync::Mutex::new(Vec::new());
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let found = match kept.iter().find(|k| **k == p) {
        Some(k) => *k,
        None => {
            let k: &'static Palette = Box::leak(Box::new(p.clone()));
            kept.push(k);
            k
        }
    };
    IS_LIGHT.store(light, Ordering::Relaxed);
    CURRENT.store(found as *const Palette as *mut Palette, Ordering::Relaxed);
}

pub fn light() -> bool {
    IS_LIGHT.load(Ordering::Relaxed)
}

pub fn palette() -> &'static Palette {
    let p = CURRENT.load(Ordering::Relaxed);
    if !p.is_null() {
        // Safety: only leaked, never freed palettes are stored.
        return unsafe { &*p };
    }
    if light() { &LIGHT } else { &DARK }
}

/// Changes whenever another palette is shown, for caches of drawn text.
pub fn stamp() -> usize {
    CURRENT.load(Ordering::Relaxed) as usize ^ light() as usize
}

static MONO_SCALE_BITS: AtomicU32 = AtomicU32::new(0);
static CODE_LINE_BITS: AtomicU32 = AtomicU32::new(0);

/// Code's size as a fraction of the document's: `MONO_SCALE`, unless the
/// settings give code a size of its own.
pub fn mono_scale() -> f32 {
    match MONO_SCALE_BITS.load(Ordering::Relaxed) {
        0 => MONO_SCALE,
        bits => f32::from_bits(bits),
    }
}

/// Rows of code as a multiple of code's size.
pub fn code_line_height() -> f32 {
    match CODE_LINE_BITS.load(Ordering::Relaxed) {
        0 => CODE_LINE_HEIGHT,
        bits => f32::from_bits(bits),
    }
}

/// Sets code's size relative to the document's and its line height.
pub fn set_code_metrics(scale: f32, line_height: f32) {
    MONO_SCALE_BITS.store(scale.clamp(0.2, 5.0).to_bits(), Ordering::Relaxed);
    CODE_LINE_BITS.store(line_height.clamp(1.0, 3.0).to_bits(), Ordering::Relaxed);
}

/// One of the terminal's sixteen ANSI colours.
pub fn ansi(i: usize) -> u32 {
    palette().ansi[i & 15]
}

/// How a role a theme leaves out is filled.
#[derive(Clone, Copy, Debug)]
pub enum Fallback {
    /// From the built-in theme of the same appearance.
    Base,
    /// The same as another role, listed earlier.
    Role(&'static str),
    /// The second role laid over the first at an alpha out of 255.
    Mix(&'static str, &'static str, u32),
    /// White or black, whichever reads better on the role.
    On(&'static str),
}

pub struct Role {
    pub name: &'static str,
    pub fallback: Fallback,
    pub get: fn(&mut Palette) -> &mut u32,
}

macro_rules! roles {
    ($($name:ident $(. $i:literal)? : $fb:expr),* $(,)?) => {
        &[$(Role { name: roles!(@name $name $($i)?), fallback: $fb, get: |p| &mut p.$name $([$i])? }),*]
    };
    (@name $name:ident) => { stringify!($name) };
    (@name $name:ident $i:literal) => { ANSI_NAMES[$i] };
}

pub const ANSI_NAMES: [&str; 16] = [
    "ansi_black",
    "ansi_red",
    "ansi_green",
    "ansi_yellow",
    "ansi_blue",
    "ansi_magenta",
    "ansi_cyan",
    "ansi_white",
    "ansi_bright_black",
    "ansi_bright_red",
    "ansi_bright_green",
    "ansi_bright_yellow",
    "ansi_bright_blue",
    "ansi_bright_magenta",
    "ansi_bright_cyan",
    "ansi_bright_white",
];

/// Every colour role in the order a theme fills them: a role falls back
/// only to roles above it. A new role is one line here, falling back to the
/// existing role nearest in meaning, so older theme files still work.
pub static ROLES: &[Role] = roles!(
    bg: Fallback::Base,
    fg: Fallback::Base,
    raised: Fallback::Mix("bg", "fg", 10),
    inset: Fallback::Mix("bg", "fg", 6),
    tint: Fallback::Mix("bg", "fg", 24),
    bright: Fallback::Role("fg"),
    dim: Fallback::Mix("fg", "bg", 90),
    faint: Fallback::Role("dim"),
    icon: Fallback::Role("faint"),
    quote: Fallback::Role("dim"),
    line: Fallback::Mix("bg", "fg", 26),
    edge: Fallback::Role("line"),
    field: Fallback::Mix("bg", "fg", 40),
    hover: Fallback::Mix("bg", "fg", 14),
    active: Fallback::Mix("bg", "fg", 26),
    primary: Fallback::Base,
    success: Fallback::Base,
    warning: Fallback::Base,
    danger: Fallback::Base,
    primary_hover: Fallback::Mix("primary", "bg", 30),
    on_primary: Fallback::On("primary"),
    selection: Fallback::Mix("bg", "primary", 90),
    found: Fallback::Mix("bg", "warning", 60),
    caret: Fallback::Role("primary"),
    link: Fallback::Role("primary"),
    destructive: Fallback::Role("danger"),
    info: Fallback::Role("link"),
    keyword: Fallback::Base,
    string: Fallback::Role("success"),
    number: Fallback::Role("warning"),
    function: Fallback::Role("link"),
    special: Fallback::Role("keyword"),
    parameter: Fallback::Role("fg"),
    property: Fallback::Role("fg"),
    term_blue: Fallback::Role("link"),
    ansi.0: Fallback::Role("bg"),
    ansi.1: Fallback::Role("danger"),
    ansi.2: Fallback::Role("success"),
    ansi.3: Fallback::Role("warning"),
    ansi.4: Fallback::Role("term_blue"),
    ansi.5: Fallback::Role("keyword"),
    ansi.6: Fallback::Role("function"),
    ansi.7: Fallback::Role("fg"),
    ansi.8: Fallback::Role("faint"),
    ansi.9: Fallback::Role("ansi_red"),
    ansi.10: Fallback::Role("ansi_green"),
    ansi.11: Fallback::Role("ansi_yellow"),
    ansi.12: Fallback::Role("ansi_blue"),
    ansi.13: Fallback::Role("special"),
    ansi.14: Fallback::Role("ansi_cyan"),
    ansi.15: Fallback::Role("bright"),
);

impl Palette {
    /// The value of a role by its name.
    pub fn role(&self, name: &str) -> Option<u32> {
        let mut p = self.clone();
        ROLES.iter().find(|r| r.name == name).map(|r| *(r.get)(&mut p))
    }

    /// Builds a palette from the colours a theme gives, by role name,
    /// filling the rest by each role's fallback. `light` picks the
    /// built-in theme that `Base` roles and the shadow come from.
    pub fn build(light: bool, given: &dyn Fn(&str) -> Option<u32>) -> Palette {
        let base = if light { &LIGHT } else { &DARK };
        let mut p = base.clone();
        for r in ROLES {
            let value = given(r.name).unwrap_or_else(|| {
                let of = |name: &str| p.role(name).unwrap_or(0);
                match r.fallback {
                    Fallback::Base => *(r.get)(&mut base.clone()),
                    Fallback::Role(a) => of(a),
                    Fallback::Mix(a, b, alpha) => mix(of(a), of(b), alpha),
                    Fallback::On(a) => {
                        let ground = of(a);
                        if contrast(0xffffff, ground) >= contrast(0, ground) { 0xffffff } else { 0 }
                    }
                }
            });
            *(r.get)(&mut p) = value;
        }
        p
    }
}

macro_rules! colours {
    ($($name:ident),*) => {$(
        pub fn $name() -> u32 {
            palette().$name
        }
    )*};
}
colours!(
    bg,
    raised,
    inset,
    tint,
    fg,
    bright,
    dim,
    faint,
    icon,
    quote,
    line,
    edge,
    field,
    hover,
    active,
    primary,
    primary_hover,
    on_primary,
    selection,
    found,
    caret,
    link,
    success,
    warning,
    danger,
    destructive,
    info,
    keyword,
    string,
    number,
    function,
    special,
    parameter,
    property
);

pub fn shadow() -> (u32, u32) {
    palette().shadow
}

pub fn scrim() -> u32 {
    palette().scrim
}

/// `b` laid over `a` at `alpha` out of 255.
pub fn mix(a: u32, b: u32, alpha: u32) -> u32 {
    let ch = |shift: u32| (((a >> shift & 255) * (255 - alpha) + (b >> shift & 255) * alpha + 127) / 255) << shift;
    ch(16) | ch(8) | ch(0)
}

/// A state colour as a quiet fill behind its own text, as badges use.
pub fn wash(color: u32) -> u32 {
    mix(bg(), color, if light() { 22 } else { 40 })
}

/// WCAG relative luminance in linear sRGB, also used for terminal colours.
pub fn luminance(rgb: u32) -> f64 {
    let channel = |shift: u32| {
        let v = ((rgb >> shift) & 255u32) as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

pub fn contrast(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

// ---- metrics, in logical pixels ----------------------------------------

/// Interface text: lists, tabs, menus, buttons.
pub const UI_TEXT: f32 = 13.0;
/// Metadata: the status bar, key caps, badges, group labels.
pub const SMALL: f32 = 12.0;
/// Titles of dialogs and the agent's messages.
pub const LARGE: f32 = 14.0;
/// The title bar, which is the window's own where the compositor draws none.
pub const TITLE: f32 = 40.0;
/// The status bar along the bottom.
pub const STATUS: f32 = 24.0;
/// Heads of panels: the editor's tabs, the explorer, the right panel. They
/// share a height so their bottom edges line up.
pub const HEAD: f32 = 36.0;
/// Rows of the explorer and the outline.
pub const ROW: f32 = 24.0;
/// Rows of the palette and of lists that drop from a control.
pub const MENU_ROW: f32 = 30.0;
/// Rows of context menus.
pub const ITEM: f32 = 28.0;
/// Buttons and toggles.
pub const CONTROL: f32 = 28.0;
pub const SMALL_CONTROL: f32 = 24.0;
pub const ICON: f32 = 16.0;
pub const SMALL_ICON: f32 = 14.0;
pub const SPACE: f32 = 8.0;
pub const PAD: f32 = 12.0;
/// Corner radii, after T3 Code's 8 pixel base.
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 6.0;
pub const RADIUS: f32 = 8.0;
pub const RADIUS_XL: f32 = 12.0;
pub const RADIUS_2XL: f32 = 16.0;

/// Body text size in logical pixels at zoom 1.
pub const BODY: f32 = 16.0;
pub const LINE_HEIGHT: f32 = 1.6;
/// Maximum width of the text column, in ems of body text.
pub const COLUMN: f32 = 36.0;
pub const HEADING: [f32; 6] = [1.75, 1.42, 1.2, 1.06, 1.0, 0.92];
pub const MONO_SCALE: f32 = 0.86;
/// Rows of code as a multiple of its size, unless the settings say.
pub const CODE_LINE_HEIGHT: f32 = 1.5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_colour_has_a_role() {
        // A field added to `Palette` needs its line in `ROLES`, or theme
        // files could not set it.
        let words = std::mem::size_of::<Palette>() / 4;
        assert_eq!(ROLES.len(), words - 3, "the shadow and the scrim are the three words without a role");
        let mut names: Vec<&str> = ROLES.iter().map(|r| r.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ROLES.len());
        let mut p = DARK.clone();
        for (i, r) in ROLES.iter().enumerate() {
            *(r.get)(&mut p) = i as u32;
        }
        assert_eq!(p.ansi[15], ROLES.len() as u32 - 1);
        assert_eq!(p.role("keyword"), Some(ROLES.iter().position(|r| r.name == "keyword").unwrap() as u32));
    }

    #[test]
    fn both_palettes_keep_text_and_focus_readable() {
        for p in [&DARK, &LIGHT] {
            for ground in [p.bg, p.raised, p.inset, p.hover, p.active] {
                for ink in [p.fg, p.bright, p.dim, p.quote, p.link, p.success, p.warning, p.danger] {
                    assert!(contrast(ink, ground) >= 4.5, "{ink:06x} on {ground:06x}: {:.2}", contrast(ink, ground));
                }
                for ink in [p.keyword, p.string, p.number, p.function, p.special, p.parameter, p.property, p.term_blue] {
                    assert!(contrast(ink, ground) >= 4.5, "syntax {ink:06x} on {ground:06x}");
                }
                // Hints and icons carry no information alone, but stay legible.
                for ink in [p.faint, p.icon] {
                    assert!(contrast(ink, ground) >= 3.8, "{ink:06x} on {ground:06x}: {:.2}", contrast(ink, ground));
                }
            }
            for ground in [p.bg, p.raised] {
                assert!(contrast(p.faint, ground) >= 4.5, "faint on {ground:06x}");
            }
            // Text stays readable inside a selection, a match and a bubble.
            for ground in [p.selection, p.found, p.tint] {
                assert!(contrast(p.fg, ground) >= 4.5, "fg on {ground:06x}");
            }
            assert!(contrast(p.on_primary, p.primary) >= 4.5);
            assert!(contrast(0xffffff, p.destructive) >= 3.0);
            // The focus ring and the caret stand out from every surface.
            for ground in [p.bg, p.raised] {
                assert!(contrast(p.primary, ground) >= 3.0, "ring on {ground:06x}");
                assert!(contrast(p.caret, ground) >= 3.0, "caret on {ground:06x}");
            }
        }
    }
}
