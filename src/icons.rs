//! Line icons in the manner of Lucide, which T3 Code uses: drawn on a 24
//! unit grid with round caps and joins, rasterized once per size through
//! resvg and kept as coverage masks to be laid down in any colour.

use std::cell::RefCell;
use std::collections::HashMap;

use resvg::{tiny_skia, usvg};

use crate::font::Bitmap;
use crate::gfx::Canvas;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    PanelLeft,
    PanelRight,
    PanelBottom,
    Search,
    Settings,
    X,
    Plus,
    Minus,
    ChevronRight,
    ChevronDown,
    ChevronLeft,
    ChevronsDownUp,
    ArrowUp,
    ArrowDown,
    Stop,
    Play,
    PlayAll,
    RotateCcw,
    Refresh,
    History,
    SquarePen,
    Message,
    ListTree,
    FileText,
    File,
    FileCode,
    FilePlus,
    Folder,
    FolderOpen,
    FolderPlus,
    Terminal,
    Columns,
    GitBranch,
    External,
    Ellipsis,
    Check,
    Dot,
    Copy,
    Eye,
    Code,
    AtSign,
    Alert,
    Lightbulb,
    Info,
    Hash,
    Braces,
    Moon,
    Sun,
    Keyboard,
    Command,
    Image,
    Gauge,
    Bot,
    Trash,
    Replace,
    ReplaceAll,
    Notebook,
    Maximize,
    Bug,
}

/// The SVG body of an icon on its 24 unit grid, and whether it is filled
/// rather than stroked.
fn body(icon: Icon) -> (&'static str, bool) {
    use Icon::*;
    let svg = match icon {
        PanelLeft => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/>"#,
        PanelRight => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M15 3v18"/>"#,
        PanelBottom => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 15h18"/>"#,
        Search => r#"<circle cx="11" cy="11" r="7"/><path d="m20 20-4.3-4.3"/>"#,
        Settings => {
            r#"<path d="M20 7h-9"/><path d="M14 17H5"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/>"#
        }
        X => r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#,
        Plus => r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#,
        Minus => r#"<path d="M5 12h14"/>"#,
        ChevronRight => r#"<path d="m9 18 6-6-6-6"/>"#,
        ChevronDown => r#"<path d="m6 9 6 6 6-6"/>"#,
        ChevronLeft => r#"<path d="m15 18-6-6 6-6"/>"#,
        ChevronsDownUp => r#"<path d="m7 20 5-5 5 5"/><path d="m7 4 5 5 5-5"/>"#,
        ArrowUp => r#"<path d="M12 19V5"/><path d="m5 12 7-7 7 7"/>"#,
        ArrowDown => r#"<path d="M12 5v14"/><path d="m19 12-7 7-7-7"/>"#,
        Stop => return (r#"<rect x="6" y="6" width="12" height="12" rx="2.5"/>"#, true),
        Play => r#"<path d="M7 4.5v15a1 1 0 0 0 1.5.86l12-7.5a1 1 0 0 0 0-1.72l-12-7.5A1 1 0 0 0 7 4.5z"/>"#,
        PlayAll => {
            r#"<path d="M4 5v14a1 1 0 0 0 1.5.86l10-7a1 1 0 0 0 0-1.72l-10-7A1 1 0 0 0 4 5z"/><path d="M19 5v14"/>"#
        }
        RotateCcw => r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"#,
        Refresh => {
            r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#
        }
        History => {
            r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>"#
        }
        SquarePen => {
            r#"<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.4 2.6a2.1 2.1 0 0 1 3 3L12 15l-4 1 1-4Z"/>"#
        }
        Message => r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#,
        ListTree => {
            r#"<path d="M21 12h-8"/><path d="M21 6H8"/><path d="M21 18h-8"/><path d="M3 6v4c0 1.1.9 2 2 2h3"/><path d="M3 10v6c0 1.1.9 2 2 2h3"/>"#
        }
        FileText => {
            r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M16 13H8"/><path d="M16 17H8"/><path d="M10 9H8"/>"#
        }
        File => {
            r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>"#
        }
        FileCode => {
            r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="m10 12.5-2 2.5 2 2.5"/><path d="m14 12.5 2 2.5-2 2.5"/>"#
        }
        FilePlus => {
            r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M9 15h6"/><path d="M12 18v-6"/>"#
        }
        Folder => {
            r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
        }
        FolderOpen => {
            r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#
        }
        FolderPlus => {
            r#"<path d="M12 10v6"/><path d="M9 13h6"/><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
        }
        Terminal => r#"<path d="m7 11 2-2-2-2"/><path d="M11 13h4"/><rect x="3" y="3" width="18" height="18" rx="2"/>"#,
        Columns => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M12 3v18"/>"#,
        GitBranch => {
            r#"<path d="M6 3v12"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"#
        }
        External => {
            r#"<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>"#
        }
        Ellipsis => {
            return (
                r#"<circle cx="5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="19" cy="12" r="1.6"/>"#,
                true,
            );
        }
        Check => r#"<path d="M20 6 9 17l-5-5"/>"#,
        Dot => return (r#"<circle cx="12" cy="12" r="4"/>"#, true),
        Copy => {
            r#"<rect x="8" y="8" width="14" height="14" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#
        }
        Eye => {
            r#"<path d="M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0"/><circle cx="12" cy="12" r="3"/>"#
        }
        Code => r#"<path d="m16 18 6-6-6-6"/><path d="m8 6-6 6 6 6"/>"#,
        AtSign => r#"<circle cx="12" cy="12" r="4"/><path d="M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-3.92 7.94"/>"#,
        Alert => {
            r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#
        }
        Info => r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#,
        Hash => r#"<path d="M4 9h16"/><path d="M4 15h16"/><path d="M10 3 8 21"/><path d="m16 3-2 18"/>"#,
        Braces => {
            r#"<path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1"/><path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1"/>"#
        }
        Moon => r#"<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>"#,
        Sun => {
            r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>"#
        }
        Keyboard => {
            r#"<rect x="2" y="5" width="20" height="14" rx="2"/><path d="M6 9h.01"/><path d="M10 9h.01"/><path d="M14 9h.01"/><path d="M18 9h.01"/><path d="M8 15h8"/>"#
        }
        Command => r#"<path d="M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3"/>"#,
        Image => {
            r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.09-3.09a2 2 0 0 0-2.82 0L6 21"/>"#
        }
        Gauge => r#"<path d="m12 14 4-4"/><path d="M3.34 19a10 10 0 1 1 17.32 0"/>"#,
        Bot => {
            r#"<path d="M12 8V4H8"/><rect x="4" y="8" width="16" height="12" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>"#
        }
        Trash => {
            r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>"#
        }
        Replace => {
            r#"<path d="M14 4a2 2 0 0 1 2-2"/><path d="M16 10a2 2 0 0 1-2-2"/><path d="M20 2a2 2 0 0 1 2 2"/><path d="M22 8a2 2 0 0 1-2 2"/><path d="m3 7 3 3 3-3"/><path d="M6 10V5a3 3 0 0 1 3-3h1"/><rect x="2" y="14" width="8" height="8" rx="2"/>"#
        }
        ReplaceAll => {
            r#"<path d="M14 14a2 2 0 0 1 2-2"/><path d="M14 4a2 2 0 0 1 2-2"/><path d="M20 2a2 2 0 0 1 2 2"/><path d="M22 8a2 2 0 0 1-2 2"/><path d="M22 14v4a2 2 0 0 1-2 2h-4"/><path d="m3 7 3 3 3-3"/><path d="M6 10V5a3 3 0 0 1 3-3h1"/><rect x="2" y="14" width="8" height="8" rx="2"/>"#
        }
        Notebook => {
            r#"<path d="M2 6h4"/><path d="M2 10h4"/><path d="M2 14h4"/><path d="M2 18h4"/><rect x="4" y="2" width="16" height="20" rx="2"/><path d="M16 2v20"/>"#
        }
        Maximize => r#"<rect x="5" y="5" width="14" height="14" rx="1.5"/>"#,
        Lightbulb => {
            r#"<path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5"/><path d="M9 18h6"/><path d="M10 22h4"/>"#
        }
        Bug => {
            r#"<path d="m8 2 1.88 1.88"/><path d="M14.12 3.88 16 2"/><path d="M9 7.13v-1a3 3 0 1 1 6 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6"/><path d="M12 20v-9"/><path d="M6 13H2"/><path d="M22 13h-4"/><path d="M6.53 9C4.6 8.8 3 7.1 3 5"/><path d="M20.97 5c0 2.1-1.6 3.8-3.5 4"/><path d="M3 21c0-2.1 1.7-3.9 3.8-4"/><path d="M17.2 17c2.1.1 3.8 1.9 3.8 4"/>"#
        }
    };
    (svg, false)
}

thread_local! {
    static CACHE: RefCell<HashMap<(Icon, u32, u32), Bitmap>> = RefCell::new(HashMap::new());
}

/// Rasterizes an icon `px` pixels square with strokes `stroke` units wide
/// on its grid.
fn render(icon: Icon, px: u32, stroke: f32) -> Bitmap {
    let empty = Bitmap { left: 0, top: 0, w: 0, h: 0, data: Box::new([]), color: false };
    let (shapes, filled) = body(icon);
    let paint = if filled {
        r##"fill="#fff" stroke="none""##.to_string()
    } else {
        format!(r##"fill="none" stroke="#fff" stroke-width="{stroke}" stroke-linecap="round" stroke-linejoin="round""##)
    };
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" {paint}>{shapes}</svg>"#);
    let Ok(tree) = usvg::Tree::from_str(&svg, &usvg::Options::default()) else { return empty };
    let Some(mut pixmap) = tiny_skia::Pixmap::new(px, px) else { return empty };
    let k = px as f32 / 24.0;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    let data: Box<[u8]> = pixmap.data().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    Bitmap { left: 0, top: 0, w: px, h: px, data, color: false }
}

/// Draws an icon `size` pixels square with its top left at `(x, y)`.
pub fn draw(c: &mut Canvas, icon: Icon, x: f32, y: f32, size: f32, color: u32) {
    let px = size.round().max(1.0) as u32;
    // Strokes are Lucide's 1.75 units, but never thinner than 1.3 device
    // pixels, so small icons stay as dark as the text beside them.
    let device = (1.75 * px as f32 / 24.0).max(1.3);
    let stroke = device * 24.0 / px as f32;
    let key = (icon, px, (stroke * 100.0) as u32);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let bmp = cache.entry(key).or_insert_with(|| render(icon, px, stroke));
        c.glyph(x.round() as i32, y.round() as i32, bmp, color);
    });
}

/// Draws an icon in the middle of a rectangle.
pub fn centred(c: &mut Canvas, icon: Icon, r: (f32, f32, f32, f32), size: f32, color: u32) {
    let size = size.round();
    draw(c, icon, (r.0 + (r.2 - size) / 2.0).round(), (r.1 + (r.3 - size) / 2.0).round(), size, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_draws_something() {
        use Icon::*;
        let all = [
            PanelLeft,
            PanelRight,
            PanelBottom,
            Search,
            Settings,
            X,
            Plus,
            Minus,
            ChevronRight,
            ChevronDown,
            ChevronLeft,
            ChevronsDownUp,
            ArrowUp,
            ArrowDown,
            Stop,
            Play,
            PlayAll,
            RotateCcw,
            Refresh,
            History,
            SquarePen,
            Message,
            ListTree,
            FileText,
            File,
            FileCode,
            FilePlus,
            Folder,
            FolderOpen,
            FolderPlus,
            Terminal,
            Columns,
            GitBranch,
            External,
            Ellipsis,
            Check,
            Dot,
            Copy,
            Eye,
            Code,
            AtSign,
            Alert,
            Info,
            Hash,
            Braces,
            Moon,
            Sun,
            Keyboard,
            Command,
            Image,
            Gauge,
            Bot,
            Trash,
            Replace,
            ReplaceAll,
            Notebook,
            Maximize,
            Lightbulb,
            Bug,
        ];
        for icon in all {
            let bmp = render(icon, 16, 1.75);
            assert!(bmp.data.iter().any(|&a| a > 96), "{icon:?} is empty");
        }
    }
}
