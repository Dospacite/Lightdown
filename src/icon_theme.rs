//! File icons for the explorer, the tabs and the file finder, from an icon
//! theme. The built-in theme draws Lightdown's line icons in the finder
//! only, as the interface always has; `Lightdown Lines` puts them in the
//! explorer and the tabs as well. Other themes are folders in
//! `~/.config/lightdown/icon-themes`, each with an `icon-theme.json`:
//!
//! ```json
//! {"name": "Mine", "file": "file.svg", "folder": "folder.svg", "folder_open": "open.svg",
//!  "file_names": {"Makefile": "make.svg"}, "suffixes": {"rs": "rust.svg", "tar.gz": "archive.svg"}}
//! ```
//!
//! SVGs are drawn in their own colours, or with `"tint": true` as a mask in
//! the icon colour; each is rasterized once per size.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use resvg::{tiny_skia, usvg};
use serde_json::Value;

use crate::font::Bitmap;
use crate::gfx::Canvas;
use crate::icons::{self, Icon};

pub const DEFAULT: &str = "Lightdown";
pub const LINES: &str = "Lightdown Lines";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    File,
    Folder,
    FolderOpen,
}

/// What an icon theme draws for a file.
#[derive(Clone, Debug, PartialEq)]
pub enum FileIcon {
    Line(Icon),
    Svg(PathBuf),
}

#[derive(Debug, Default)]
pub struct IconTheme {
    pub name: String,
    /// Where the SVGs are; empty for a built-in theme.
    dir: PathBuf,
    /// Icons in the explorer and the tabs as well as the finder.
    pub everywhere: bool,
    tint: bool,
    file: Option<String>,
    folder: Option<String>,
    folder_open: Option<String>,
    names: HashMap<String, String>,
    /// Suffixes, lower case, longest first.
    suffixes: Vec<(String, String)>,
}

/// The line icon for a file, by its extension, as the finder has drawn.
fn line_icon(name: &str, kind: Kind) -> Icon {
    match kind {
        Kind::Folder => return Icon::Folder,
        Kind::FolderOpen => return Icon::FolderOpen,
        Kind::File => {}
    }
    let ext = name.rsplit_once('.').map_or("", |(stem, e)| if stem.is_empty() { "" } else { e });
    match ext.to_ascii_lowercase().as_str() {
        "md" | "markdown" | "txt" | "tex" => Icon::FileText,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => Icon::Image,
        "ipynb" => Icon::Notebook,
        "" => Icon::File,
        _ => Icon::FileCode,
    }
}

impl IconTheme {
    pub fn built_in(everywhere: bool) -> IconTheme {
        IconTheme { name: if everywhere { LINES } else { DEFAULT }.into(), everywhere, ..IconTheme::default() }
    }

    /// Reads a theme folder's `icon-theme.json`.
    pub fn load(dir: &Path) -> Result<IconTheme, String> {
        let path = dir.join("icon-theme.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        IconTheme::parse(&text, dir).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str, dir: &Path) -> Result<IconTheme, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let name = v["name"].as_str().filter(|n| !n.trim().is_empty()).ok_or("an icon theme needs a `name`")?;
        let svg = |v: &Value| v.as_str().filter(|s| s.ends_with(".svg") && !s.contains("..")).map(str::to_string);
        let map = |key: &str| -> Vec<(String, String)> {
            v[key].as_object().into_iter().flatten().filter_map(|(k, f)| Some((k.clone(), svg(f)?))).collect()
        };
        let mut suffixes: Vec<(String, String)> =
            map("suffixes").into_iter().map(|(k, f)| (k.trim_start_matches('.').to_ascii_lowercase(), f)).collect();
        suffixes.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        Ok(IconTheme {
            name: name.trim().into(),
            dir: dir.to_path_buf(),
            everywhere: true,
            tint: v["tint"] == true,
            file: svg(&v["file"]),
            folder: svg(&v["folder"]),
            folder_open: svg(&v["folder_open"]).or_else(|| svg(&v["folder"])),
            names: map("file_names").into_iter().collect(),
            suffixes,
        })
    }

    /// The icon for a file or folder by its name.
    pub fn icon(&self, name: &str, kind: Kind) -> FileIcon {
        if self.dir.as_os_str().is_empty() {
            return FileIcon::Line(line_icon(name, kind));
        }
        let lower = name.to_ascii_lowercase();
        let file = match kind {
            Kind::Folder => self.folder.as_ref(),
            Kind::FolderOpen => self.folder_open.as_ref(),
            Kind::File => self.names.get(name).or_else(|| {
                self.suffixes
                    .iter()
                    .find(|(s, _)| lower.len() > s.len() + 1 && lower.ends_with(&format!(".{s}")))
                    .map(|(_, f)| f)
                    .or(self.file.as_ref())
            }),
        };
        match file {
            Some(f) => FileIcon::Svg(self.dir.join(f)),
            None => FileIcon::Line(line_icon(name, kind)),
        }
    }
}

/// The folder icon themes are read from.
pub fn dir() -> PathBuf {
    crate::config::config_dir().join("icon-themes")
}

/// Every icon theme: the built-in pair, then each folder in `dir()`. Folders
/// that cannot be read are named in the second list.
pub fn all() -> (Vec<IconTheme>, Vec<String>) {
    let mut out = vec![IconTheme::built_in(false), IconTheme::built_in(true)];
    let mut errors = Vec::new();
    let mut dirs: Vec<PathBuf> =
        std::fs::read_dir(dir()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs.into_iter().take(200) {
        match IconTheme::load(&d) {
            Ok(t) => {
                out.retain(|o| o.name != t.name);
                out.push(t);
            }
            Err(e) => errors.push(e),
        }
    }
    (out, errors)
}

thread_local! {
    static CURRENT: RefCell<Rc<IconTheme>> = RefCell::new(Rc::new(IconTheme::built_in(false)));
    static RASTERS: RefCell<HashMap<(PathBuf, u32, bool), Bitmap>> = RefCell::new(HashMap::new());
}

/// Makes a theme the one drawn.
pub fn set(theme: IconTheme) {
    CURRENT.with(|c| *c.borrow_mut() = Rc::new(theme));
    RASTERS.with(|r| r.borrow_mut().clear());
}

pub fn current() -> Rc<IconTheme> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Whether the explorer and tabs show file icons.
pub fn everywhere() -> bool {
    CURRENT.with(|c| c.borrow().everywhere)
}

fn rasterize(path: &Path, px: u32, tint: bool) -> Bitmap {
    let empty = Bitmap { left: 0, top: 0, w: 0, h: 0, data: Box::new([]), color: false };
    let Ok(data) = std::fs::read(path) else { return empty };
    if data.len() > 1 << 20 {
        return empty;
    }
    let options = usvg::Options { resources_dir: path.parent().map(Path::to_path_buf), ..usvg::Options::default() };
    let Ok(tree) = usvg::Tree::from_data(&data, &options) else { return empty };
    let Some(mut pixmap) = tiny_skia::Pixmap::new(px, px) else { return empty };
    let size = tree.size();
    let k = px as f32 / size.width().max(size.height()).max(1.0);
    let (dx, dy) = ((px as f32 - size.width() * k) / 2.0, (px as f32 - size.height() * k) / 2.0);
    resvg::render(&tree, tiny_skia::Transform::from_row(k, 0.0, 0.0, k, dx, dy), &mut pixmap.as_mut());
    let pixels = pixmap.data().as_chunks::<4>().0;
    if tint {
        let data: Box<[u8]> = pixels.iter().map(|p| p[3]).collect();
        return Bitmap { left: 0, top: 0, w: px, h: px, data, color: false };
    }
    // Pixmaps are premultiplied; colour glyphs are straight.
    let mut data = Vec::with_capacity(pixels.len() * 4);
    for p in pixels {
        let a = p[3] as u32;
        let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a / 2) / a).min(255) as u8 };
        data.extend([un(p[0]), un(p[1]), un(p[2]), p[3]]);
    }
    Bitmap { left: 0, top: 0, w: px, h: px, data: data.into(), color: true }
}

/// Draws the icon for a file or folder, `size` pixels square at `(x, y)`,
/// in `ink` where the theme leaves the colour to Lightdown.
pub fn draw(c: &mut Canvas, name: &str, kind: Kind, x: f32, y: f32, size: f32, ink: u32) {
    let theme = current();
    match theme.icon(name, kind) {
        FileIcon::Line(icon) => icons::draw(c, icon, x, y, size, ink),
        FileIcon::Svg(path) => {
            let px = size.round().max(1.0) as u32;
            RASTERS.with(|r| {
                let mut r = r.borrow_mut();
                if r.len() > 2000 {
                    r.clear();
                }
                let key = (path, px, theme.tint);
                let bmp = r.entry(key.clone()).or_insert_with(|| rasterize(&key.0, px, key.2));
                if bmp.w == 0 {
                    icons::draw(c, line_icon(name, kind), x, y, size, ink);
                } else {
                    c.glyph(x.round() as i32, y.round() as i32, bmp, ink);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_theme_keeps_the_finders_icons() {
        let t = IconTheme::built_in(false);
        assert!(!t.everywhere && IconTheme::built_in(true).everywhere);
        assert_eq!(t.icon("a.md", Kind::File), FileIcon::Line(Icon::FileText));
        assert_eq!(t.icon("a.PNG", Kind::File), FileIcon::Line(Icon::Image));
        assert_eq!(t.icon("nb.ipynb", Kind::File), FileIcon::Line(Icon::Notebook));
        assert_eq!(t.icon("Makefile", Kind::File), FileIcon::Line(Icon::File));
        assert_eq!(t.icon(".bashrc", Kind::File), FileIcon::Line(Icon::File));
        assert_eq!(t.icon("main.rs", Kind::File), FileIcon::Line(Icon::FileCode));
        assert_eq!(t.icon("src", Kind::FolderOpen), FileIcon::Line(Icon::FolderOpen));
    }

    #[test]
    fn a_theme_matches_names_then_the_longest_suffix_then_the_default() {
        let dir = Path::new("/icons");
        let t = IconTheme::parse(
            r#"{"name": "T", "file": "file.svg", "folder": "dir.svg",
                "file_names": {"Makefile": "make.svg"}, "suffixes": {"gz": "gz.svg", ".tar.gz": "tar.svg", "rs": "rust.svg", "x": "../evil.svg"}}"#,
            dir,
        )
        .unwrap();
        let svg = |f: &str| FileIcon::Svg(dir.join(f));
        assert_eq!(t.icon("Makefile", Kind::File), svg("make.svg"));
        assert_eq!(t.icon("a.tar.gz", Kind::File), svg("tar.svg"));
        assert_eq!(t.icon("a.gz", Kind::File), svg("gz.svg"));
        assert_eq!(t.icon("MAIN.RS", Kind::File), svg("rust.svg"));
        assert_eq!(t.icon("a.x", Kind::File), svg("file.svg"), "paths out of the folder are refused");
        assert_eq!(t.icon("rs", Kind::File), svg("file.svg"));
        assert_eq!(t.icon("src", Kind::FolderOpen), svg("dir.svg"), "an open folder falls back to the folder");
        assert!(IconTheme::parse("{}", dir).is_err());
    }

    #[test]
    fn svg_icons_are_rasterized_in_colour_or_as_a_mask() {
        let dir = std::env::temp_dir().join(format!("ld-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("red.svg");
        std::fs::write(&path, r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></svg>"##).unwrap();
        let bmp = rasterize(&path, 16, false);
        assert!(bmp.color && bmp.w == 16);
        assert_eq!(&bmp.data[16 * 4 * 8 + 32..16 * 4 * 8 + 36], &[255, 0, 0, 255]);
        let mask = rasterize(&path, 16, true);
        assert!(!mask.color && mask.data.iter().all(|&a| a == 255));
        assert_eq!(rasterize(&dir.join("missing.svg"), 16, false).w, 0);
        // Drawn into a canvas, the colour lands.
        let mut px = vec![0u32; 32 * 32];
        set(IconTheme::parse(r#"{"name": "T", "file": "red.svg"}"#, &dir).unwrap());
        draw(&mut Canvas::new(&mut px, 32, 32), "a.txt", Kind::File, 4.0, 4.0, 16.0, 0xffffff);
        assert_eq!(px[10 * 32 + 10], 0xff0000);
        set(IconTheme::built_in(false));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn theme_folders_are_listed_after_the_built_in_pair() {
        let (themes, _) = all();
        assert_eq!(themes[0].name, DEFAULT);
        assert_eq!(themes[1].name, LINES);
    }
}
