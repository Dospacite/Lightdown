//! Fonts: the embedded Libron family for serif reading, Inter for the
//! interface and sans-serif reading, a lazily resolved system monospace
//! face, per-character fallback through fontconfig, shaping and a glyph
//! cache.

use std::collections::HashMap;
use std::process::Command;
use std::thread::JoinHandle;

use swash::proxy::CharmapProxy;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::shape::ShapeContext;
use swash::text::{Codepoint, Script};
use swash::zeno::{Format, Vector};
use swash::{CacheKey, FontRef, Metrics};

/// The serif family: regular, italic, bold and bold italic follow it.
pub const SERIF: u8 = 0;
pub const REGULAR: u8 = 0;
/// The sans-serif family, laid out as the serif one is.
pub const SANS: u8 = 4;
/// The interface face: labels, bars, menus and controls. Inter, unless the
/// settings name another family; then these five faces are replaced.
pub const UI: u8 = 9;
pub const UI_ITALIC: u8 = 10;
/// Semibold: titles, headings and strong text.
pub const UI_BOLD: u8 = 11;
/// Medium: buttons, active tabs, the open file.
pub const UI_MEDIUM: u8 = 13;
/// Faces embedded in the binary, and the interface's copies of Inter;
/// fallback faces load after them.
const EMBEDDED: usize = 14;

static GENERATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Changes whenever a family or its features change, for caches of
/// shaped text.
pub fn generation() -> u32 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

fn bump() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) static LIBRON: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/Libron-Regular.ttf"),
    include_bytes!("../assets/fonts/Libron-Italic.ttf"),
    include_bytes!("../assets/fonts/Libron-Bold.ttf"),
    include_bytes!("../assets/fonts/Libron-BoldItalic.ttf"),
];

/// Inter, in the order of the serif faces, then its medium weight.
pub(crate) static INTER: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Italic.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBoldItalic.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
];

struct Face {
    data: &'static [u8],
    offset: u32,
    key: CacheKey,
    charmap: CharmapProxy,
    metrics: Metrics,
    path: String,
}

impl Face {
    fn new(data: &'static [u8], index: usize, path: String) -> Option<Face> {
        let font = FontRef::from_index(data, index)?;
        Some(Face {
            data,
            offset: font.offset,
            key: font.key,
            charmap: CharmapProxy::from_font(&font),
            metrics: font.metrics(&[]),
            path,
        })
    }

    fn font(&self) -> FontRef<'static> {
        FontRef { data: self.data, offset: self.offset, key: self.key }
    }

    fn has(&self, ch: char) -> bool {
        self.charmap.materialize(&self.font()).map(ch) != 0
    }
}

/// A rasterized glyph: an alpha mask, or straight RGBA pixels when `color`.
pub struct Bitmap {
    pub left: i32,
    pub top: i32,
    pub w: u32,
    pub h: u32,
    pub data: Box<[u8]>,
    pub color: bool,
}

pub struct Shaped {
    pub face: u8,
    pub id: u16,
    pub x: f32,
    pub y: f32,
}

pub struct Fonts {
    faces: Vec<Face>,
    shape: ShapeContext,
    scale: ScaleContext,
    cache: HashMap<u64, Bitmap>,
    mono: Option<u8>,
    mono_bold: Option<u8>,
    /// The fontconfig query for the monospace face, started at launch so it
    /// overlaps with connecting to the compositor instead of delaying a frame.
    mono_query: Option<JoinHandle<Option<(String, usize)>>>,
    /// The family code is set in; empty for the system's monospace.
    mono_family: String,
    /// OpenType features for code, as the settings give them.
    mono_features: Vec<swash::Setting<u16>>,
    fallback: HashMap<char, u8>,
}

impl Fonts {
    pub fn new() -> Fonts {
        let faces = LIBRON
            .iter()
            .chain(&INTER)
            .chain(&INTER)
            .map(|d| Face::new(d, 0, String::new()).expect("embedded font"))
            .collect();
        Fonts {
            faces,
            shape: ShapeContext::new(),
            scale: ScaleContext::new(),
            cache: HashMap::new(),
            mono: None,
            mono_bold: None,
            mono_query: Some(std::thread::spawn(|| fc_match("monospace"))),
            mono_family: String::new(),
            mono_features: Vec::new(),
            fallback: HashMap::new(),
        }
    }

    /// Sets code in a fontconfig family, or the system's monospace when
    /// empty. The family is looked for in the background.
    pub fn set_mono_family(&mut self, family: &str) {
        if family == self.mono_family && self.mono_query.is_some() {
            return;
        }
        if family == self.mono_family && self.mono.is_some() {
            return;
        }
        self.mono_family = family.to_string();
        (self.mono, self.mono_bold) = (None, None);
        let family = family.to_string();
        self.mono_query = Some(std::thread::spawn(move || match family.as_str() {
            "" => fc_match("monospace"),
            f => fc_family(f, "").or_else(|| fc_match("monospace")),
        }));
        bump();
    }

    /// Turns OpenType features of code on or off, by four-letter tag.
    pub fn set_mono_features(&mut self, features: &std::collections::BTreeMap<String, u16>) {
        let settings: Vec<swash::Setting<u16>> =
            features.iter().filter(|(t, _)| t.len() == 4).map(|(t, &v)| (t.as_str(), v).into()).collect();
        if settings != self.mono_features {
            self.mono_features = settings;
            bump();
        }
    }

    /// Sets the interface in a fontconfig family, or in Inter when empty.
    /// Says why when the family is not installed.
    pub fn set_ui_family(&mut self, family: &str) -> Result<(), String> {
        let ui = UI as usize;
        if family.is_empty() {
            for (i, d) in INTER.iter().enumerate() {
                self.faces[ui + i] = Face::new(d, 0, String::new()).expect("embedded font");
            }
        } else {
            let styles = [":style=Regular", ":italic", ":bold", ":bold:italic", ":medium"];
            let mut found = Vec::new();
            for style in styles {
                let (path, index) = fc_family(family, style).ok_or_else(|| format!("No font family {family} is installed"))?;
                let data: &'static [u8] = Box::leak(std::fs::read(&path).map_err(|e| e.to_string())?.into_boxed_slice());
                found.push(Face::new(data, index, path).ok_or_else(|| format!("{family} could not be read"))?);
            }
            for (i, face) in found.into_iter().enumerate() {
                self.faces[ui + i] = face;
            }
        }
        self.cache.retain(|k, _| !(ui as u64..ui as u64 + 5).contains(&(k >> 40)));
        bump();
        Ok(())
    }

    /// A face of a family, `SERIF` or `SANS`.
    pub fn face(family: u8, bold: bool, italic: bool) -> u8 {
        family + ((bold as u8) << 1 | italic as u8)
    }

    /// The system monospace face, resolved on first use. Falls back to the
    /// regular serif when fontconfig has nothing to offer.
    pub fn mono(&mut self) -> u8 {
        if let Some(m) = self.mono {
            return m;
        }
        let found = self.mono_query.take().and_then(|q| q.join().ok()).flatten();
        let m = found.and_then(|(p, i)| self.load(&p, i)).unwrap_or(REGULAR);
        self.mono = Some(m);
        m
    }

    /// The bold weight of the monospace face, for keywords in code. Falls
    /// back to the regular weight.
    pub fn mono_bold(&mut self) -> u8 {
        if let Some(m) = self.mono_bold {
            return m;
        }
        let family = self.mono_family.clone();
        let query = if family.is_empty() { fc_match("monospace:bold") } else { fc_family(&family, ":bold") };
        let found = query.and_then(|(p, i)| self.load(&p, i));
        let m = found.unwrap_or_else(|| self.mono());
        self.mono_bold = Some(m);
        m
    }

    fn load(&mut self, path: &str, index: usize) -> Option<u8> {
        if let Some(i) = self.faces.iter().position(|f| f.path == path) {
            return Some(i as u8);
        }
        if self.faces.len() >= 250 {
            return None;
        }
        let data: &'static [u8] = Box::leak(std::fs::read(path).ok()?.into_boxed_slice());
        self.faces.push(Face::new(data, index, path.to_string())?);
        Some(self.faces.len() as u8 - 1)
    }

    /// Scaled metrics for a face: (ascent, descent, x-height).
    pub fn metrics(&self, face: u8, size: f32) -> (f32, f32, f32) {
        let m = self.faces[face as usize].metrics.scale(size);
        (m.ascent, m.descent, m.x_height)
    }

    fn face_for(&mut self, want: u8, ch: char) -> u8 {
        if (ch as u32) < 0x80 || self.faces[want as usize].has(ch) {
            return want;
        }
        if let Some(&f) = self.fallback.get(&ch) {
            return f;
        }
        let loaded = (EMBEDDED..self.faces.len()).find(|&i| self.faces[i].has(ch)).map(|i| i as u8);
        let face = loaded
            .or_else(|| {
                let (path, index) = fc_match(&format!(":charset={:x}", ch as u32))?;
                let f = self.load(&path, index)?;
                self.faces[f as usize].has(ch).then_some(f)
            })
            .unwrap_or(want);
        self.fallback.insert(ch, face);
        face
    }

    /// Shapes `text` in the wanted face, substituting fallback faces for
    /// characters it lacks. Calls `f(start, end, advance, glyphs)` for each
    /// cluster, in source order, with byte offsets into `text`.
    pub fn shape(&mut self, want: u8, size: f32, text: &str, mut f: impl FnMut(usize, usize, f32, &[Shaped])) {
        let mut run = 0;
        let mut face = want;
        let mut script = Script::Unknown;
        let mut out = Vec::new();
        let mut it = text.char_indices().peekable();
        while let Some((i, ch)) = it.next() {
            // Marks and joiners stay with whatever face drew their base.
            let sticky = i > run && !matches!(ch.cluster_break(), swash::text::ClusterBreak::XX);
            let fc = if sticky { face } else { self.face_for(want, ch) };
            let sc = ch.script();
            let strong = !matches!(sc, Script::Common | Script::Inherited | Script::Unknown);
            if (fc != face || (strong && script != Script::Unknown && sc != script)) && i > run {
                self.shape_run(face, size, text, run, i, &mut out, &mut f);
                run = i;
                script = Script::Unknown;
            }
            if strong { script = sc; }
            face = fc;
            if it.peek().is_none() {
                self.shape_run(face, size, text, run, text.len(), &mut out, &mut f);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn shape_run(
        &mut self,
        face: u8,
        size: f32,
        text: &str,
        start: usize,
        end: usize,
        out: &mut Vec<Shaped>,
        f: &mut impl FnMut(usize, usize, f32, &[Shaped]),
    ) {
        let run = &text[start..end];
        let script = run
            .chars()
            .map(|c| c.script())
            .find(|s| !matches!(s, Script::Common | Script::Inherited | Script::Unknown))
            .unwrap_or(Script::Latin);
        let font = self.faces[face as usize].font();
        let rtl = matches!(script, Script::Arabic | Script::Hebrew | Script::Syriac | Script::Thaana);
        let direction = if rtl { swash::shape::Direction::RightToLeft } else { swash::shape::Direction::LeftToRight };
        let code = !self.mono_features.is_empty() && (Some(face) == self.mono || Some(face) == self.mono_bold);
        let features: &[swash::Setting<u16>] = if code { &self.mono_features } else { &[] };
        let mut shaper =
            self.shape.builder(font).script(script).direction(direction).size(size).features(features.iter().copied()).build();
        shaper.add_str(run);
        shaper.shape_with(|c| {
            out.clear();
            let mut pen = 0.0;
            for g in c.glyphs {
                out.push(Shaped { face, id: g.id, x: pen + g.x, y: g.y });
                pen += g.advance;
            }
            f(start + c.source.start as usize, start + c.source.end as usize, pen, out);
        });
    }

    pub fn width(&mut self, face: u8, size: f32, text: &str) -> f32 {
        let mut w = 0.0;
        self.shape(face, size, text, |_, _, adv, _| w += adv);
        w
    }

    /// Returns the cached bitmap for a glyph at a quarter-pixel x offset.
    pub fn glyph(&mut self, face: u8, id: u16, size: f32, sub: u8) -> &Bitmap {
        let key = (face as u64) << 40 | (id as u64) << 24 | ((size * 16.0) as u64 & 0xffff) << 8 | sub as u64;
        let (faces, scale) = (&self.faces, &mut self.scale);
        self.cache.entry(key).or_insert_with(|| {
            let mut scaler = scale.builder(faces[face as usize].font()).size(size).hint(false).build();
            let image =
                Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline])
                    .format(Format::Alpha)
                    .offset(Vector::new(sub as f32 / 4.0, 0.0))
                    .render(&mut scaler, id);
            let Some(image) = image else {
                return Bitmap { left: 0, top: 0, w: 0, h: 0, data: Box::new([]), color: false };
            };
            let p = image.placement;
            let (data, color) = match image.content {
                Content::Mask => (image.data, false),
                Content::Color => {
                    let mut data = image.data;
                    // Swash composites COLR layers into premultiplied RGBA;
                    // embedded PNG strikes supply straight RGBA already.
                    if matches!(image.source, Source::ColorOutline(_)) {
                        for pixel in data.chunks_exact_mut(4) {
                            let a = pixel[3] as u32;
                            if a > 0 {
                                for c in &mut pixel[..3] { *c = ((*c as u32 * 255 + a/2)/a).min(255) as u8; }
                            }
                        }
                    }
                    (data, true)
                }
                Content::SubpixelMask => (image.data.chunks_exact(4)
                    .map(|p| ((p[0] as u32+p[1] as u32+p[2] as u32)/3) as u8).collect(), false),
            };
            Bitmap { left: p.left, top: p.top, w: p.width, h: p.height, data: data.into(), color }
        })
    }
}

/// The file of a family in a style, only if fontconfig has that family
/// rather than a stand-in.
fn fc_family(family: &str, style: &str) -> Option<(String, usize)> {
    let mut escaped = String::new();
    for c in family.trim().chars() {
        if matches!(c, '-' | ':' | ',' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    let pattern = format!("{escaped}{style}");
    let out = Command::new("fc-match").args(["-f", "%{file}\n%{index}\n%{family}", &pattern]).output().ok()?;
    let out = String::from_utf8(out.stdout).ok()?;
    let mut lines = out.splitn(3, '\n');
    let (file, index, families) = (lines.next()?, lines.next()?, lines.next().unwrap_or(""));
    let wanted = family.trim().to_lowercase();
    families.split(',').any(|f| f.trim().to_lowercase() == wanted).then(|| (file.to_string(), index.trim().parse().unwrap_or(0)))
}

/// Installed font families, monospace ones only when `mono`, sorted, each
/// by the first of its names.
pub fn families(mono: bool) -> Vec<String> {
    let pattern = if mono { ":spacing=mono" } else { ":" };
    let Ok(out) = Command::new("fc-list").args([pattern, "family"]).output() else { return Vec::new() };
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|f| f.trim().replace('\\', ""))
        .filter(|f| !f.is_empty())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// Asks fontconfig for the best file matching a pattern.
fn fc_match(pattern: &str) -> Option<(String, usize)> {
    let out = Command::new("fc-match").args(["-f", "%{file}\n%{index}", pattern]).output().ok()?;
    let out = String::from_utf8(out.stdout).ok()?;
    let (file, index) = out.split_once('\n')?;
    Some((file.to_string(), index.trim().parse().unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_noto_color_emoji_keeps_its_rgb_pixels() {
        let Some((path, index)) = fc_match("emoji") else { return };
        if !path.contains("NotoColorEmoji") { return; }
        let mut fonts = Fonts::new();
        let face = fonts.load(&path,index).unwrap();
        let f = &fonts.faces[face as usize];
        let id = f.charmap.materialize(&f.font()).map('😀');
        let bmp = fonts.glyph(face,id,24.0,0);
        assert!(bmp.color && bmp.w > 0);
        assert!(bmp.data.chunks_exact(4).any(|p| p[3] > 0 && (p[0] != p[1] || p[1] != p[2])));
    }

    #[test]
    fn code_features_reach_the_shaper_and_the_cursor_sits_inside_a_ligature() {
        use crate::layout::{Cluster, Layout, Row};
        // A cluster two characters wide: the cursor can stand in its middle.
        let mut l = Layout::default();
        l.clusters.push(Cluster { start: 0, end: 2, x: 10.0, w: 20.0, row: 0, space: false, rtl: false });
        l.clusters.push(Cluster { start: 2, end: 3, x: 30.0, w: 10.0, row: 0, space: false, rtl: false });
        l.rows.push(Row { y: 0.0, h: 20.0, x0: 0.0, x1: 100.0, text_x: 10.0, first: 0, last: 2, start: 0, end: 3 });
        assert_eq!(l.locate(1), (0, 20.0));
        assert_eq!(l.locate(2), (0, 30.0));
        assert_eq!(l.offset_in_row(0, 17.0), 1);
        assert_eq!(l.offset_in_row(0, 26.0), 2);
        // With a ligature font installed, turning `calt` and `liga` off
        // changes the glyphs of `->` but never which bytes they cover.
        let mut fonts = Fonts::new();
        let Some(family) = families(true).into_iter().find(|f| f.contains("JetBrains") || f.contains("Fira Code")) else { return };
        fonts.set_mono_family(&family);
        let mono = fonts.mono();
        let shape = |fonts: &mut Fonts| {
            let mut out = Vec::new();
            fonts.shape(mono, 16.0, "a->b", |s, e, _, g| out.push((s, e, g.iter().map(|g| g.id).collect::<Vec<_>>())));
            out
        };
        let on = shape(&mut fonts);
        fonts.set_mono_features(&[("calt".to_string(), 0), ("liga".to_string(), 0)].into_iter().collect());
        let off = shape(&mut fonts);
        assert_ne!(on, off, "{family} should draw -> as a ligature");
        let covered = |v: &Vec<(usize, usize, Vec<u16>)>| v.iter().map(|c| c.1 - c.0).sum::<usize>();
        assert_eq!((covered(&on), covered(&off)), (4, 4));
    }
}
