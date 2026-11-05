//! PDFs, read and drawn in-process by hayro: a document's pages are
//! measured when it is opened and drawn, at the size they are shown, on the
//! image worker as they come into view. Their text, with where each
//! character stands, is read on a thread of its own.
//!
//! A picture opened as the document is shown the same way, as one page the
//! size of its pixels.

mod marks;
mod vector;
pub use marks::{Bookmark, Link, Marks, Target, read_marks};
pub use vector::formula_svg;

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::hayro_syntax::{LoadPdfError, Pdf};
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache,
    InterpreterSettings, SoftMask, interpret_page,
};
use hayro::kurbo::{Affine, BezPath, Point, Rect};
use hayro::vello_cpu::color::palette::css::{TRANSPARENT, WHITE};
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings};

use crate::gfx::Canvas;
use crate::kernel::Waker;
use crate::pics::Pics;
use crate::theme;

/// The pages of a PDF file as it was when read.
#[derive(Clone, Debug)]
pub struct Pages {
    pub path: PathBuf,
    /// When the file was written and how long it is, so that pages drawn
    /// from an earlier version are not taken for these.
    stamp: String,
    /// Each page's size in points, upright, or a picture's in pixels.
    pub sizes: Vec<(f32, f32)>,
    /// It is a picture, not a PDF.
    pub picture: bool,
    /// The picture is an SVG, drawn afresh at whatever size it is shown.
    pub vector: bool,
}

/// The most pixels an SVG is drawn in; shown larger, it is stretched.
const MOST_DRAWN: f32 = 4096.0 * 4096.0;

/// Whether a file is a PDF, by its name or its first bytes.
pub fn is_pdf(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return true;
    }
    let mut head = [0u8; 5];
    let read = std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head));
    read.is_ok() && head == *b"%PDF-"
}

fn stamp(path: &Path) -> std::io::Result<String> {
    let meta = std::fs::metadata(path)?;
    let at = meta.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    Ok(format!("{}-{}", at.as_nanos(), meta.len()))
}

fn load(path: &Path) -> Result<Pdf, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    Pdf::new(Arc::new(data)).map_err(|e| match e {
        LoadPdfError::Decryption(_) => "it is encrypted".to_string(),
        LoadPdfError::Invalid => "it is not a PDF that can be read".to_string(),
    })
}

impl Pages {
    /// Reads a PDF's pages. Says why not where it cannot.
    pub fn open(path: &Path) -> Result<Pages, String> {
        let stamp = stamp(path).map_err(|e| e.to_string())?;
        let pdf = load(path)?;
        let sizes: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
        if sizes.is_empty() {
            return Err("it has no pages".into());
        }
        Ok(Pages { path: path.to_path_buf(), stamp, sizes, picture: false, vector: false })
    }

    /// Reads a picture's size, as one page. Says why not where it cannot.
    pub fn picture(path: &Path) -> Result<Pages, String> {
        let stamp = stamp(path).map_err(|e| e.to_string())?;
        let (w, h) = crate::pics::probe(path).ok_or("it is not a picture that can be read")?;
        if w == 0 || h == 0 {
            return Err("it has no pixels".into());
        }
        if w.max(h) > crate::pics::MAX_SIDE {
            return Err(format!("it is more than {} pixels on a side", crate::pics::MAX_SIDE));
        }
        let sizes = vec![(w as f32, h as f32)];
        let vector = crate::pics::is_vector(path);
        Ok(Pages { path: path.to_path_buf(), stamp, sizes, picture: true, vector })
    }

    /// Reads the file again, as what it was read as before.
    pub fn reopen(&self) -> Result<Pages, String> {
        if self.picture { Pages::picture(&self.path) } else { Pages::open(&self.path) }
    }

    /// Whether the file has been written since it was read.
    pub fn changed(&self) -> bool {
        stamp(&self.path).is_ok_and(|s| s != self.stamp)
    }

    /// The size page `n` is shown at, `k` pixels to a point.
    pub fn shown(&self, n: usize, k: f32) -> (f32, f32) {
        let (w, h) = self.sizes[n];
        ((w * k).round().max(1.0), (h * k).round().max(1.0))
    }

    /// The widest page's width, in points.
    pub fn widest(&self) -> f32 {
        self.sizes.iter().map(|s| s.0).fold(1.0, f32::max)
    }

    /// The name the image cache knows page `n` by. A zero byte cannot be in
    /// the name of a file, so no picture's path is ever taken for it.
    pub fn key(&self, n: usize) -> PathBuf {
        if self.picture {
            return crate::pics::whole_key(&self.path, &self.stamp);
        }
        let mut key = self.path.as_os_str().to_owned().into_vec();
        key.extend(format!("\0{}\0{n}", self.stamp).bytes());
        PathBuf::from(OsString::from_vec(key))
    }

    /// Draws the pages one under another from `pen` down, `k` pixels to a
    /// point, centred on `mid`. Only those that reach between `top` and
    /// `bottom` are drawn. Returns where the next thing would go.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        c: &mut Canvas,
        pics: &mut Pics,
        mid: f32,
        mut pen: f32,
        k: f32,
        gap: f32,
        top: f32,
        bottom: f32,
    ) -> f32 {
        for n in 0..self.sizes.len() {
            let (dw, dh) = self.shown(n, k);
            if pen + dh > top && pen < bottom {
                let x = (mid - dw / 2.0).round();
                if self.picture {
                    self.draw_picture(c, pics, x, pen, dw, dh);
                } else {
                    c.rect(x - 1.0, pen - 1.0, dw + 2.0, dh + 2.0, theme::line());
                    match pics.get(&self.key(n), dw as u32, dh as u32) {
                        Some(pic) => c.image(x as i32, pen as i32, dw as i32, dh as i32, pic),
                        None => c.rect(x, pen, dw, dh, theme::inset()),
                    }
                }
            }
            pen += dh + gap;
        }
        pen
    }

    /// Draws the picture `dw` by `dh` pixels at `x`, `y`. Pixels are never
    /// decoded larger than they are: shown larger, they are drawn as
    /// squares. An SVG is drawn at the size it is shown. What it lets
    /// through shows a checkerboard.
    fn draw_picture(&self, c: &mut Canvas, pics: &mut Pics, x: f32, y: f32, dw: f32, dh: f32) {
        let (w, h) = self.sizes[0];
        let (rw, rh) = if self.vector {
            let k = (MOST_DRAWN / (dw * dh)).sqrt().min(1.0);
            ((dw * k).round().max(1.0), (dh * k).round().max(1.0))
        } else if dw > w {
            (w, h)
        } else {
            (dw, dh)
        };
        let Some(pic) = pics.get(&self.key(0), rw as u32, rh as u32) else {
            return c.rect(x, y, dw, dh, theme::inset());
        };
        if !pic.opaque {
            checkerboard(c, x, y, dw, dh);
        }
        c.image(x as i32, y as i32, dw as i32, dh as i32, pic);
    }
}

/// The checkerboard behind a picture's transparent parts, in squares of 8
/// pixels, light or dark with the theme.
fn checkerboard(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32) {
    let (a, b) = if theme::light() { (0xffffff, 0xdde2ea) } else { (0x4a5466, 0x3a4352) };
    c.rect(x, y, w, h, a);
    let side = 8.0;
    let (across, down) = ((w / side).ceil() as usize, (h / side).ceil() as usize);
    for row in 0..down {
        for col in (row % 2..across).step_by(2) {
            let (sx, sy) = (x + col as f32 * side, y + row as f32 * side);
            c.rect(sx, sy, side.min(x + w - sx), side.min(y + h - sy), b);
        }
    }
}

/// The file, version and page a cache key names, if it names a page.
fn parse(key: &Path) -> Option<(PathBuf, String, usize)> {
    let bytes = key.as_os_str().as_bytes();
    let mut parts = bytes.splitn(4, |&b| b == 0);
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(parts.next()?));
    let stamp = String::from_utf8(parts.next()?.to_vec()).ok()?;
    let n = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    Some((path, stamp, n))
}

/// The key of a page drawn on nothing rather than on white, as a
/// formula is, so that it takes the colour of the text around it.
pub fn clear_key(page: &Path) -> PathBuf {
    let mut key = page.as_os_str().as_bytes().to_vec();
    key.extend(b"\0clear");
    PathBuf::from(OsString::from_vec(key))
}

/// Whether a page's key asks for it drawn on nothing.
pub fn is_clear(key: &Path) -> bool {
    key.as_os_str().as_bytes().ends_with(b"\0clear")
}

/// Whether a key of the image cache names a page.
pub fn is_page(key: &Path) -> bool {
    key.as_os_str().as_bytes().contains(&0)
}

/// The PDF pages were last drawn from, which the image worker keeps so
/// that each page does not read the file anew.
#[derive(Default)]
pub struct Shelf(Option<(PathBuf, String, Pdf)>);

impl Shelf {
    /// Draws the page a cache key names, `w` by `h` pixels, on nothing,
    /// as RGBA bytes that are not premultiplied.
    pub fn draw_clear(&mut self, key: &Path, w: u32, h: u32) -> Option<Vec<u8>> {
        let pixmap = self.render(key, w, h, false)?;
        let px = pixmap.data().iter();
        Some(
            px.flat_map(|p| {
                let a = p.a as u32;
                let un = |c: u8| (c as u32 * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255) as u8);
                [un(p.r), un(p.g), un(p.b), p.a]
            })
            .collect(),
        )
    }

    /// Draws the page a cache key names, `w` by `h` pixels, on white, as
    /// RGB.
    pub fn draw(&mut self, key: &Path, w: u32, h: u32) -> Option<Vec<u32>> {
        let pixmap = self.render(key, w, h, true)?;
        // On white every pixel is opaque, so premultiplying changed nothing.
        let px = pixmap.data().iter();
        Some(px.map(|p| (p.r as u32) << 16 | (p.g as u32) << 8 | p.b as u32).collect())
    }

    fn render(&mut self, key: &Path, w: u32, h: u32, white: bool) -> Option<Pixmap> {
        let (path, stamp, n) = parse(key)?;
        if self.0.as_ref().is_none_or(|(p, s, _)| *p != path || *s != stamp) {
            // A file written since the key was made is drawn as it is now;
            // the preview reads it anew and asks again.
            self.0 = Some((path.clone(), stamp, load(&path).ok()?));
        }
        let (_, _, pdf) = self.0.as_ref()?;
        let page = pdf.pages().get(n)?;
        let (pw, ph) = page.render_dimensions();
        let (w16, h16) = (u16::try_from(w).ok()?, u16::try_from(h).ok()?);
        let mut ctx = RenderContext::new(w16, h16);
        let scale = Affine::scale_non_uniform(w as f64 / pw.max(1.0) as f64, h as f64 / ph.max(1.0) as f64);
        let settings = InterpreterSettings::default();
        let cache = RenderCache::new();
        let transform = scale * page.initial_transform(true).to_kurbo();
        hayro::render_into(page, &cache, &settings, &RenderSettings::default(), &mut ctx, transform);
        ctx.flush();
        let mut pixmap = Pixmap::new(w16, h16);
        let ground = if white { TargetInit::Clear(WHITE) } else { TargetInit::Clear(TRANSPARENT) };
        let raster = RasterizerSettings { target_init: ground, ..Default::default() };
        ctx.render_with(&mut pixmap, &mut Resources::default(), raster);
        Some(pixmap)
    }
}

/// What a page says, in the order it is drawn, and where each character of
/// it stands.
#[derive(Clone, Default, Debug)]
pub struct Text {
    pub text: String,
    /// Each character: where it starts in `text`, and the box it fills, in
    /// points from the page's top left, as left, top, right, bottom. The
    /// spaces and line breaks put between words have boxes of no width.
    pub chars: Vec<(usize, [f32; 4])>,
}

impl Text {
    fn push(&mut self, ch: char, rect: [f32; 4]) {
        self.chars.push((self.text.len(), rect));
        self.text.push(ch);
    }

    /// Where in the text a point on the page comes: before or after the
    /// character nearest it.
    pub fn at(&self, x: f32, y: f32) -> usize {
        let away = |r: &[f32; 4]| {
            let dx = (r[0] - x).max(x - r[2]).max(0.0);
            let dy = (r[1] - y).max(y - r[3]).max(0.0);
            // Being on the line counts for more than being near across it.
            dy * 4.0 + dx
        };
        let shown = self.chars.iter().filter(|(_, r)| r[2] > r[0]);
        let Some(&(at, r)) = shown.min_by(|a, b| away(&a.1).total_cmp(&away(&b.1))) else { return 0 };
        if x < (r[0] + r[2]) / 2.0 { at } else { self.after(at) }
    }

    /// Where the character at `at` ends.
    fn after(&self, at: usize) -> usize {
        self.text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
    }

    /// The word around `at`, or the character there if it is not in one.
    pub fn word(&self, at: usize) -> (usize, usize) {
        let part = |c: char| c.is_alphanumeric() || c == '_';
        let start = self.text[..at].char_indices().rev().take_while(|&(_, c)| part(c)).last().map_or(at, |(i, _)| i);
        let end = at + self.text[at..].chars().take_while(|&c| part(c)).map(char::len_utf8).sum::<usize>();
        if start == end { (at, self.after(at)) } else { (start, end) }
    }

    /// The line around `at`.
    pub fn line(&self, at: usize) -> (usize, usize) {
        let start = self.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[at..].find('\n').map_or(self.text.len(), |i| at + i);
        (start, end)
    }

    /// The boxes that cover the text from `a` to `b`, those on one line
    /// joined into one.
    pub fn boxes(&self, a: usize, b: usize) -> Vec<[f32; 4]> {
        let mut out: Vec<[f32; 4]> = Vec::new();
        for &(_, r) in self.chars.iter().filter(|(at, r)| *at >= a && *at < b && r[2] > r[0]) {
            match out.last_mut() {
                Some(last) if (last[1] - r[1]).abs() < (r[3] - r[1]) * 0.5 && r[0] >= last[0] => {
                    (last[1], last[2], last[3]) = (last[1].min(r[1]), last[2].max(r[2]), last[3].max(r[3]));
                }
                _ => out.push(r),
            }
        }
        out
    }
}

/// Gathers the characters a page draws, ignoring everything else.
struct Reading {
    text: Text,
    /// Where the last character ended: its right edge, its baseline, its
    /// size and its box.
    last: Option<(f32, f32, f32, [f32; 4])>,
}

impl<'a> Device<'a> for Reading {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            // Glyphs are a thousand units to the em, set on their baseline.
            let t = props.transform * glyph.transform();
            let advance = match &**glyph {
                Glyph::Outline(g) => g.advance_width().filter(|a| *a > 0.0).unwrap_or(500.0),
                Glyph::Type3(_) => 500.0,
            } as f64;
            let corners =
                [(0.0, -220.0), (advance, -220.0), (0.0, 780.0), (advance, 780.0)].map(|p| t * Point::from(p));
            let bounds = corners.iter().fold(Rect::from_points(corners[0], corners[0]), |r, p| r.union_pt(*p));
            let rect = [bounds.x0 as f32, bounds.y0 as f32, bounds.x1 as f32, bounds.y1 as f32];
            let base = (t * Point::ZERO).y as f32;
            let size = ((t * Point::new(0.0, 1000.0)) - (t * Point::ZERO)).hypot() as f32;
            let said = match glyph.as_unicode() {
                Some(BfString::Char(c)) => c.to_string(),
                Some(BfString::String(s)) => s,
                None => '\u{fffd}'.to_string(),
            };
            let blank = said.trim().is_empty();
            if let Some((right, line, was, r)) = self.last {
                let between = [right, r[1], right, r[3]];
                let ends = |t: &Text| t.text.ends_with(char::is_whitespace);
                // Off the line, or back across it: a new line.
                if (base - line).abs() > was.max(size) * 0.5 || rect[2] < r[0] {
                    if !self.text.text.ends_with('\n') {
                        self.text.push('\n', between);
                    }
                } else if rect[0] - right > size * 0.15 && !blank && !ends(&self.text) {
                    self.text.push(' ', between);
                }
            }
            if blank && self.text.text.ends_with(char::is_whitespace) {
                continue;
            }
            // A ligature's letters share its box.
            let n = said.chars().count().max(1) as f32;
            let step = (rect[2] - rect[0]) / n;
            for (i, ch) in said.chars().enumerate() {
                let x = rect[0] + step * i as f32;
                self.text.push(ch, [x, rect[1], x + step, rect[3]]);
            }
            self.last = Some((rect[2], base, size, rect));
        }
    }
}

/// Reads the text of each page of a PDF on a thread of its own, sending
/// each page's as it is read.
pub fn read_text(path: PathBuf, wake: Waker) -> Receiver<(usize, Text)> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let Ok(pdf) = load(&path) else { return };
        let cache = InterpreterCache::new();
        for (n, page) in pdf.pages().iter().enumerate() {
            let (w, h) = page.render_dimensions();
            let bbox = Rect::new(0.0, 0.0, w as f64, h as f64);
            let settings = InterpreterSettings { render_annotations: false, ..Default::default() };
            let transform = page.initial_transform(true).to_kurbo();
            let mut ctx = Context::new(transform, bbox, &cache, page.xref(), settings);
            let mut reading = Reading { text: Text::default(), last: None };
            interpret_page(page, &mut ctx, &mut reading);
            if tx.send((n, reading.text)).is_err() {
                return;
            }
            wake();
        }
    });
    rx
}

#[cfg(test)]
pub mod tests {
    use super::*;
    pub use super::marks::tests::linked;

    /// A PDF of `pages` pages, each `w` by `h` points, with a red square in
    /// the top left of the first and two lines of text beside it.
    pub fn sample(pages: usize, w: u32, h: u32) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {pages} >>",
                (0..pages).map(|i| format!("{} 0 R", 4 + i)).collect::<Vec<_>>().join(" ")
            ),
        ];
        let draw = format!(
            "1 0 0 rg 0 {} 20 20 re f BT /F1 12 Tf 30 {} Td (Hello world) Tj 0 -20 Td (again) Tj ET",
            h - 20,
            h - 40
        );
        objects.push(format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()));
        let font = 4 + pages;
        for i in 0..pages {
            let content = if i == 0 { " /Contents 3 0 R" } else { "" };
            let resources = format!(" /Resources << /Font << /F1 {font} 0 R >> >>");
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}]{content}{resources} >>"));
        }
        objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for at in offsets {
            out.extend(format!("{at:010} 00000 n \n").bytes());
        }
        out.extend(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes(),
        );
        out
    }

    #[test]
    fn pages_are_measured_and_drawn() {
        let dir = std::env::temp_dir().join(format!("lightdown-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.pdf");
        std::fs::write(&file, sample(3, 200, 100)).unwrap();
        let pages = Pages::open(&file).unwrap();
        assert_eq!(pages.sizes, [(200.0, 100.0); 3]);
        assert!(is_pdf(&file) && !pages.changed());

        // A key names its page and nothing on disk.
        let key = pages.key(2);
        assert!(is_page(&key) && !is_page(&file));
        assert_eq!(parse(&key).map(|(p, _, n)| (p, n)), Some((file.clone(), 2)));

        // Drawn at twice its size: the square is red, the rest white.
        let px = Shelf::default().draw(&pages.key(0), 400, 200).unwrap();
        assert_eq!(px.len(), 400 * 200);
        assert_eq!((px[10 * 400 + 10], px[150 * 400 + 300]), (0xff0000, 0xffffff));

        // The text is read with where each letter stands.
        let read = read_text(file.clone(), Arc::new(|| {}));
        let (n, text) = read.recv().unwrap();
        assert_eq!((n, text.text.as_str()), (0, "Hello world\nagain"));
        let at = |c: &str| text.chars[text.text.find(c).unwrap()].1;
        let (h, a) = (at("H"), at("a"));
        // "Hello" stands at 30 points across, its baseline 40 down.
        assert!((h[0] - 30.0).abs() < 0.5 && h[1] < 40.0 && h[3] > 40.0 && a[1] > h[3] - 1.0);
        assert_eq!(text.at(h[0] + 1.0, h[1] + 3.0), 0);
        assert_eq!(text.word(2), (0, 5));
        assert_eq!(text.line(13), (12, 17));
        assert_eq!(text.boxes(0, 11).len(), 1);
        assert_eq!(text.boxes(0, 17).len(), 2);
        assert_eq!(read.iter().count(), 2);

        std::fs::write(&file, b"not a pdf").unwrap();
        assert!(Pages::open(&file).is_err() && !is_pdf(&dir.join("b.txt")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
