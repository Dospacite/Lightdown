//! Images. Sizes are read from file headers when a line is laid out;
//! pixels are decoded and scaled on a worker thread, so a large photo never
//! delays a frame. Colour is preserved in photos, diagrams and notebook plots.
//! An animated GIF plays, and an SVG is drawn at the size it is shown.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use resvg::usvg::{self, fontdb};

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_png::PngDecoder;

/// Images larger than this on either side are not loaded.
pub const MAX_SIDE: u32 = 20_000;
/// Scaled bitmaps kept in memory before the least recently shown are dropped.
const CACHE_BYTES: usize = 192 << 20;
/// An animation's frames are decoded up to this many bytes; it plays those.
const FRAME_BYTES: usize = 256 << 20;

/// A bitmap at display size: premultiplied luminance and alpha pairs, or
/// opaque RGB for PDF pages, or premultiplied RGBA for colour images.
pub struct Pic {
    pub w: u32,
    pub h: u32,
    pub la: Vec<u8>,
    pub rgb: Vec<u32>,
    /// For an animation, every frame, one after another.
    pub rgba: Vec<u8>,
    /// Nothing in it shows what is under it.
    pub opaque: bool,
    /// How long each frame of an animation shows, in milliseconds; empty
    /// for a still picture.
    delays: Vec<u32>,
    /// The frame of an animation on show, and when it began to play.
    frame: usize,
    born: Option<Instant>,
    used: u64,
}

impl Pic {
    fn new(w: u32, h: u32) -> Pic {
        let (la, rgb, rgba, delays) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        Pic { w, h, la, rgb, rgba, opaque: true, delays, frame: 0, born: None, used: 0 }
    }

    /// The premultiplied RGBA pixels on show: an animation's current frame.
    pub fn shown(&self) -> &[u8] {
        let size = self.w as usize * self.h as usize * 4;
        self.rgba.get(self.frame * size..(self.frame + 1) * size).unwrap_or(&self.rgba)
    }

    /// Moves an animation on to the frame for `now`. Returns when that
    /// frame ends.
    fn play(&mut self, now: Instant) -> Option<Instant> {
        let total: u32 = self.delays.iter().sum();
        if self.delays.len() < 2 || total == 0 {
            return None;
        }
        let born = *self.born.get_or_insert(now);
        let into = (now.duration_since(born).as_millis() % total as u128) as u32;
        let mut end = 0;
        for (n, delay) in self.delays.iter().enumerate() {
            end += delay;
            if into < end {
                self.frame = n;
                break;
            }
        }
        Some(now + Duration::from_millis((end - into) as u64))
    }
}

/// What ends the cache key of a picture shown on its own.
const WHOLE: &[u8] = b"\0picture";

/// The name the cache knows a picture by when it is the document: drawn in
/// its own colours, never lightened for a dark theme, and decoded anew once
/// `stamp`, which changes when the file is written, does. A zero byte cannot
/// be in a file's name, so no picture in a document is taken for it.
pub fn whole_key(path: &Path, stamp: &str) -> PathBuf {
    let mut key = path.as_os_str().to_owned().into_vec();
    key.extend(format!("\0{stamp}").bytes());
    key.extend(WHOLE);
    PathBuf::from(OsString::from_vec(key))
}

/// The file a key made by `whole_key` names.
fn whole(key: &Path) -> Option<PathBuf> {
    let bytes = key.as_os_str().as_bytes().strip_suffix(WHOLE)?;
    let file = bytes.split(|&b| b == 0).next()?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(file)))
}

struct Job {
    path: PathBuf,
    w: u32,
    h: u32,
}

struct Done {
    job: Job,
    pic: Option<Pic>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct Pics {
    sizes: HashMap<PathBuf, Option<(u32, u32)>>,
    ready: HashMap<PathBuf, Pic>,
    /// Size each image has been requested at and not yet delivered.
    pending: HashMap<PathBuf, (u32, u32)>,
    jobs: Option<Sender<Job>>,
    done: (Sender<Done>, Receiver<Done>),
    waker: Option<Waker>,
    frame: u64,
    /// The time of the frame being drawn, and the soonest an animation
    /// drawn in it moves on.
    now: Instant,
    wake: Option<Instant>,
}

impl Pics {
    pub fn new() -> Pics {
        Pics {
            sizes: HashMap::new(),
            ready: HashMap::new(),
            pending: HashMap::new(),
            jobs: None,
            done: channel(),
            waker: None,
            frame: 0,
            now: Instant::now(),
            wake: None,
        }
    }

    /// When an animation that was drawn moves on, so that the window is
    /// drawn again then.
    pub fn next_frame(&self) -> Option<Instant> {
        self.wake
    }

    /// Sets what to call, from the worker thread, when an image is ready.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// Natural size of the image at `path`, or `None` if it is not one we
    /// can show.
    pub fn size(&mut self, path: &Path) -> Option<(u32, u32)> {
        if let Some(size) = self.sizes.get(path) {
            return *size;
        }
        let size = probe(path).filter(|&(w, h)| w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE);
        self.sizes.insert(path.to_path_buf(), size);
        size
    }

    /// The bitmap for an image shown at `w` x `h`. Requests it if missing or
    /// of another size; until it arrives, whatever is already there is
    /// returned, to be drawn stretched.
    pub fn get(&mut self, path: &Path, w: u32, h: u32) -> Option<&Pic> {
        let fresh = self.ready.get(path).is_some_and(|p| (p.w, p.h) == (w, h));
        if !fresh && self.pending.get(path) != Some(&(w, h)) {
            self.pending.insert(path.to_path_buf(), (w, h));
            let (done, waker) = (self.done.0.clone(), self.waker.clone());
            let jobs = self.jobs.get_or_insert_with(|| {
                let (tx, rx) = channel();
                std::thread::spawn(move || worker(rx, done, waker));
                tx
            });
            let _ = jobs.send(Job { path: path.to_path_buf(), w, h });
        }
        let (frame, now) = (self.frame, self.now);
        let pic = self.ready.get_mut(path)?;
        pic.used = frame;
        if let Some(ends) = pic.play(now) {
            self.wake = Some(self.wake.map_or(ends, |w| w.min(ends)));
        }
        Some(&*pic)
    }

    /// Drops every decoded image, so that each is made anew for a changed
    /// theme.
    pub fn forget(&mut self) {
        self.ready.clear();
        self.pending.clear();
    }

    /// Takes in finished work. Call once per frame, before drawing.
    pub fn poll(&mut self) {
        self.frame += 1;
        (self.now, self.wake) = (Instant::now(), None);
        while let Ok(done) = self.done.1.try_recv() {
            self.accept(done);
        }
    }

    /// Blocks until nothing is being decoded. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn wait(&mut self) {
        while !self.pending.is_empty() {
            let Ok(done) = self.done.1.recv() else { break };
            self.accept(done);
        }
    }

    fn accept(&mut self, Done { job, pic }: Done) {
        if self.pending.get(&job.path) == Some(&(job.w, job.h)) {
            self.pending.remove(&job.path);
        }
        match pic {
            Some(mut pic) => {
                pic.used = self.frame;
                // A new size goes on playing where the old one was.
                pic.born = self.ready.get(&job.path).and_then(|old| old.born);
                self.ready.insert(job.path, pic);
            }
            None => {
                // Unreadable after all: fall back to showing its alt text.
                self.ready.remove(&job.path);
                self.sizes.insert(job.path, None);
            }
        }
        let size = |p: &Pic| p.la.len() + p.rgb.len() * 4 + p.rgba.len();
        let mut total: usize = self.ready.values().map(size).sum();
        while total > CACHE_BYTES {
            let oldest = self.ready.iter().filter(|(_, p)| p.used < self.frame).min_by_key(|(_, p)| p.used);
            let Some(path) = oldest.map(|(path, _)| path.clone()) else { break };
            total -= self.ready.remove(&path).map_or(0, |p| size(&p));
        }
    }
}

enum Format {
    Png,
    Jpeg,
    Gif,
    WebP,
    Svg,
}

fn sniff(head: &[u8], path: &Path) -> Option<Format> {
    let svgz = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svgz"));
    Some(match head {
        [0x89, b'P', b'N', b'G', ..] => Format::Png,
        [0xff, 0xd8, 0xff, ..] => Format::Jpeg,
        [b'G', b'I', b'F', b'8', ..] => Format::Gif,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Format::WebP,
        [0x1f, 0x8b, ..] if svgz => Format::Svg,
        _ if is_svg(head) => Format::Svg,
        _ => return None,
    })
}

/// Whether text begins as an SVG document does: with `<svg`, perhaps after
/// an XML declaration, a doctype and comments.
fn is_svg(head: &[u8]) -> bool {
    let mut s = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let find = |s: &[u8], what: &[u8]| s.windows(what.len()).position(|w| w == what);
    loop {
        s = s.trim_ascii_start();
        if s.starts_with(b"<svg") {
            return s.get(4).is_some_and(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/');
        }
        let end = if s.starts_with(b"<?") {
            find(s, b"?>").map(|i| i + 2)
        } else if s.starts_with(b"<!--") {
            find(s, b"-->").map(|i| i + 3)
        } else if s.starts_with(b"<!") {
            // A doctype may hold declarations of its own in brackets.
            let close = find(s, b">");
            match find(s, b"[") {
                Some(open) if close.is_some_and(|c| open < c) => find(&s[open..], b"]>").map(|i| open + i + 2),
                _ => close.map(|i| i + 1),
            }
        } else {
            None
        };
        match end {
            Some(end) => s = &s[end..],
            None => return false,
        }
    }
}

/// Opens a file if it is an image we know, with a copy of its first bytes.
/// Probing and decoding share this one reader type, so each decoder is
/// compiled only once.
fn open(path: &Path) -> Option<(Format, [u8; 24], BufReader<File>)> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = BufReader::new(File::open(path).ok()?);
    let start = file.fill_buf().ok()?;
    let format = sniff(start, path)?;
    let mut head = [0; 24];
    let n = start.len().min(24);
    head[..n].copy_from_slice(&start[..n]);
    Some((format, head, file))
}

/// Whether a picture is drawn from shapes, an SVG, rather than pixels.
pub fn is_vector(path: &Path) -> bool {
    open(path).is_some_and(|(format, ..)| matches!(format, Format::Svg))
}

/// Reads an image's size from its header without decoding it.
pub fn probe(path: &Path) -> Option<(u32, u32)> {
    let (format, head, file) = open(path)?;
    let (w, h) = match format {
        // The decoder would read on through the pixel data; the size is at a
        // fixed place in the first chunk.
        Format::Png => {
            let b = Some(&head[12..24]).filter(|b| b.starts_with(b"IHDR"))?;
            let side = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
            (side(4), side(8))
        }
        Format::Jpeg => {
            let mut d = JpegDecoder::new(file);
            d.decode_headers().ok()?;
            let info = d.info()?;
            let (w, h) = (info.width as usize, info.height as usize);
            if orientation(d.exif()) >= 5 { (h, w) } else { (w, h) }
        }
        Format::Gif => {
            let b = &head[6..10];
            (u16::from_le_bytes([b[0], b[1]]) as usize, u16::from_le_bytes([b[2], b[3]]) as usize)
        }
        Format::WebP => {
            let (w, h) = image_webp::WebPDecoder::new(file).ok()?.dimensions();
            (w as usize, h as usize)
        }
        Format::Svg => {
            let size = parse_svg(path, file, None)?.size();
            (size.width().ceil() as usize, size.height().ceil() as usize)
        }
    };
    Some((w.try_into().ok()?, h.try_into().ok()?))
}

/// The EXIF orientation tag (1 to 8) from a TIFF block, 1 if absent.
fn orientation(exif: Option<&Vec<u8>>) -> u8 {
    let Some(b) = exif else { return 1 };
    let little = match b.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return 1,
    };
    let u16_at = |i: usize| {
        let v = b.get(i..i + 2)?;
        Some(if little { u16::from_le_bytes([v[0], v[1]]) } else { u16::from_be_bytes([v[0], v[1]]) })
    };
    let ifd = match b.get(4..8) {
        Some(v) if little => u32::from_le_bytes([v[0], v[1], v[2], v[3]]) as usize,
        Some(v) => u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize,
        None => return 1,
    };
    let entries = u16_at(ifd).unwrap_or(0) as usize;
    (0..entries)
        .map(|i| ifd + 2 + i * 12)
        .find(|&e| u16_at(e) == Some(0x0112))
        .and_then(|e| u16_at(e + 8))
        .map_or(1, |v| v as u8)
}

/// Reads an SVG document. Its text is drawn in `fonts`; without them, as
/// when only its size is wanted, it is left out.
fn parse_svg(path: &Path, mut file: BufReader<File>, fonts: Option<Arc<fontdb::Database>>) -> Option<usvg::Tree> {
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut data).ok()?;
    let mut options = usvg::Options { resources_dir: path.parent().map(Path::to_path_buf), ..Default::default() };
    if let Some(fonts) = fonts {
        options.fontdb = fonts;
        options.font_resolver.select_fallback = Box::new(fallback);
    }
    usvg::Tree::from_data(&data, &options).ok()
}

/// A font for a character the first font chosen for some SVG text lacks:
/// the one fontconfig picks for it, like the first in family, weight and
/// slant. usvg's own choice is the first font it holds with the character,
/// whatever it looks like, and it sets the whole line in that font when it
/// can.
fn fallback(c: char, used: &[fontdb::ID], db: &mut Arc<fontdb::Database>) -> Option<fontdb::ID> {
    type Picks = std::sync::Mutex<HashMap<(String, char), Option<(PathBuf, u32)>>>;
    static PICKS: std::sync::OnceLock<Picks> = std::sync::OnceLock::new();
    let base = db.face(*used.first()?)?;
    let family = base.families.first().map_or("sans-serif", |f| f.0.as_str());
    let mut pattern = format!("{}:charset={:x}", family.replace([':', '-', ','], " "), c as u32);
    if base.weight.0 >= 600 {
        pattern += ":weight=bold";
    }
    if base.style != fontdb::Style::Normal {
        pattern += ":slant=italic";
    }
    let picks = PICKS.get_or_init(Default::default);
    let pick = picks.lock().ok()?.get(&(pattern.clone(), c)).cloned();
    let pick = pick.unwrap_or_else(|| {
        let out = std::process::Command::new("fc-match").args(["-f", "%{file}\n%{index}", &pattern]).output();
        let out = out.ok().and_then(|o| String::from_utf8(o.stdout).ok());
        let pick = out.as_deref().and_then(|o| o.split_once('\n'));
        let pick = pick.map(|(file, index)| (PathBuf::from(file), index.trim().parse().unwrap_or(0)));
        if let Ok(mut picks) = picks.lock() {
            picks.insert((pattern, c), pick.clone());
        }
        pick
    });
    let (file, index) = pick?;
    let face = db.faces().find(|f| {
        let path = match &f.source {
            fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => Some(path),
            fontdb::Source::Binary(_) => None,
        };
        path == Some(&file) && f.index == index
    })?;
    (!used.contains(&face.id)).then_some(face.id)
}

/// The fonts an SVG's text is drawn in: the system's, with the generic
/// families as fontconfig resolves them, and Libron when it has none.
fn svg_fonts() -> Arc<fontdb::Database> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    for data in crate::font::LIBRON {
        db.load_font_source(fontdb::Source::Binary(Arc::new(data)));
    }
    let family = |pattern: &str| {
        let out = std::process::Command::new("fc-match").args(["-f", "%{family[0]}", pattern]).output().ok()?;
        String::from_utf8(out.stdout).ok().filter(|f| !f.trim().is_empty())
    };
    let libron = || "Libron".to_string();
    db.set_serif_family(family("serif").unwrap_or_else(libron));
    db.set_sans_serif_family(family("sans-serif").unwrap_or_else(libron));
    db.set_monospace_family(family("monospace").unwrap_or_else(libron));
    Arc::new(db)
}

/// Draws an SVG `w` by `h` pixels, stretched to fill them.
fn render_svg(tree: &usvg::Tree, w: usize, h: usize) -> Option<Raw> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w as u32, h as u32)?;
    let size = tree.size();
    let fit = resvg::tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(tree, fit, &mut pixmap.as_mut());
    let px: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Raw::from_pixels(w, h, 4, &px)
}

/// Decoded pixels: luminance, premultiplied luminance/alpha, or premultiplied
/// RGBA, and for an animation every frame of it, one after another.
struct Raw {
    w: usize,
    h: usize,
    ch: usize,
    data: Vec<u8>,
    /// How long each frame of an animation shows, in milliseconds; empty
    /// for a still picture.
    delays: Vec<u32>,
}

impl Raw {
    /// Converts interleaved 8-bit pixels of `n` channels (L, LA, RGB, RGBA).
    fn from_pixels(w: usize, h: usize, n: usize, px: &[u8]) -> Option<Raw> {
        if !(1..=4).contains(&n) || px.len() < w * h * n {
            return None;
        }
        // Keep monochrome transparent line art on the existing luminance
        // path, where it can adapt to a dark page without recolouring plots.
        if n >= 3 && px[..w * h * n].chunks_exact(n).any(|p| p[0] != p[1] || p[1] != p[2]) {
            let data = px[..w * h * n]
                .chunks_exact(n)
                .flat_map(|p| {
                    let a = if n == 4 { p[3] as u32 } else { 255 };
                    let premul = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                    [premul(p[0]), premul(p[1]), premul(p[2]), a as u8]
                })
                .collect();
            return Some(Raw { w, h, ch: 4, data, delays: Vec::new() });
        }
        let lum = |p: &[u8]| match n {
            1 | 2 => p[0] as u32,
            _ => (p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8,
        };
        let px = &px[..w * h * n];
        let translucent = n.is_multiple_of(2) && px.chunks_exact(n).any(|p| p[n - 1] != 255);
        let data = if translucent {
            let pair = |p: &[u8]| {
                let a = p[n - 1] as u32;
                [((lum(p) * a + 127) / 255) as u8, a as u8]
            };
            px.chunks_exact(n).flat_map(pair).collect()
        } else {
            px.chunks_exact(n).map(|p| lum(p) as u8).collect()
        };
        Some(Raw { w, h, ch: if translucent { 2 } else { 1 }, data, delays: Vec::new() })
    }

    /// An animation of RGBA frames whose pixels are either clear or
    /// opaque, as a GIF's are.
    fn animation(w: usize, h: usize, mut px: Vec<u8>, delays: Vec<u32>) -> Raw {
        for p in px.chunks_exact_mut(4).filter(|p| p[3] == 0) {
            p[..3].fill(0);
        }
        Raw { w, h, ch: 4, data: px, delays }
    }

    /// Applies an EXIF orientation, so that the image is upright.
    fn oriented(self, o: u8) -> Raw {
        if !(2..=8).contains(&o) {
            return self;
        }
        let (w, h, ch) = (self.w, self.h, self.ch);
        let (ow, oh) = if o >= 5 { (h, w) } else { (w, h) };
        let mut data = vec![0; self.data.len()];
        for y in 0..oh {
            for x in 0..ow {
                let (sx, sy) = match o {
                    2 => (w - 1 - x, y),
                    3 => (w - 1 - x, h - 1 - y),
                    4 => (x, h - 1 - y),
                    5 => (y, x),
                    6 => (y, h - 1 - x),
                    7 => (w - 1 - y, h - 1 - x),
                    _ => (w - 1 - y, x),
                };
                let (s, d) = ((sy * w + sx) * ch, (y * ow + x) * ch);
                data[d..d + ch].copy_from_slice(&self.data[s..s + ch]);
            }
        }
        Raw { w: ow, h: oh, ch, data, delays: self.delays }
    }

    /// Resamples to display size: an area average when shrinking, linear
    /// interpolation when enlarging. With `lighten`, dark line art on a
    /// transparent ground is shown light in a dark theme.
    fn scaled(&self, w: usize, h: usize, lighten: bool) -> Pic {
        let (across, down) = (taps(self.w, w), taps(self.h, h));
        let frames = self.data.chunks_exact(self.w * self.h * self.ch);
        let out: Vec<u8> = frames.flat_map(|frame| self.resample(frame, &across, &down, w, h)).collect();
        self.finish(out, w, h, lighten)
    }

    /// Resamples one frame along both axes.
    fn resample(
        &self,
        frame: &[u8],
        across: &[(usize, Vec<f32>)],
        down: &[(usize, Vec<f32>)],
        w: usize,
        h: usize,
    ) -> Vec<u8> {
        let ch = self.ch;
        let mut wide = vec![0u8; w * self.h * ch];
        for (src, dst) in frame.chunks_exact(self.w * ch).zip(wide.chunks_exact_mut(w * ch)) {
            for ((first, weights), out) in across.iter().zip(dst.chunks_exact_mut(ch)) {
                for (c, o) in out.iter_mut().enumerate() {
                    let sum: f32 =
                        weights.iter().enumerate().map(|(k, wt)| wt * src[(first + k) * ch + c] as f32).sum();
                    *o = (sum + 0.5) as u8;
                }
            }
        }
        let mut out = vec![0u8; w * h * ch];
        for ((first, weights), dst) in down.iter().zip(out.chunks_exact_mut(w * ch)) {
            for (i, o) in dst.iter_mut().enumerate() {
                let sum: f32 =
                    weights.iter().enumerate().map(|(k, wt)| wt * wide[(first + k) * w * ch + i] as f32).sum();
                *o = (sum + 0.5) as u8;
            }
        }
        out
    }

    /// Makes resampled pixels into a bitmap to draw.
    fn finish(&self, out: Vec<u8>, w: usize, h: usize, lighten: bool) -> Pic {
        let (ch, mut pic) = (self.ch, Pic::new(w as u32, h as u32));
        if ch == 4 {
            pic.opaque = out.chunks_exact(4).all(|p| p[3] == 255);
            (pic.rgba, pic.delays) = (out, self.delays.clone());
            return pic;
        }
        let mut la: Vec<u8> = if ch == 2 { out } else { out.into_iter().flat_map(|l| [l, 255]).collect() };
        // Dark line art on a transparent ground would vanish against a dark
        // page, so there it is shown light instead.
        let (mut light, mut cover, mut clear) = (0u64, 0u64, 0usize);
        for p in la.chunks_exact(2) {
            light += p[0] as u64;
            cover += p[1] as u64;
            clear += (p[1] < 128) as usize;
        }
        if lighten && !crate::theme::light() && clear * 50 > w * h && light * 255 < cover * 96 {
            for p in la.chunks_exact_mut(2) {
                p[0] = p[1] - p[0].min(p[1]);
            }
        }
        pic.opaque = ch == 1 || la.chunks_exact(2).all(|p| p[1] == 255);
        pic.la = la;
        pic
    }
}

/// For each output sample along one axis: the first input sample that
/// contributes to it and the weight of each one from there.
fn taps(n_in: usize, n_out: usize) -> Vec<(usize, Vec<f32>)> {
    let step = n_in as f32 / n_out as f32;
    (0..n_out)
        .map(|i| {
            if step > 1.0 {
                let (a, b) = (i as f32 * step, ((i + 1) as f32 * step).min(n_in as f32));
                let first = (a.floor() as usize).min(n_in - 1);
                let last = (b.ceil() as usize).clamp(first + 1, n_in);
                let weights: Vec<f32> = (first..last).map(|j| ((j + 1) as f32).min(b) - (j as f32).max(a)).collect();
                let total: f32 = weights.iter().sum();
                (first, weights.into_iter().map(|w| w / total.max(f32::EPSILON)).collect())
            } else {
                let at = ((i as f32 + 0.5) * step - 0.5).clamp(0.0, (n_in - 1) as f32);
                let first = at.floor() as usize;
                let f = at - first as f32;
                (first, if first + 1 < n_in { vec![1.0 - f, f] } else { vec![1.0] })
            }
        })
        .collect()
}

/// What a picture is drawn from: its pixels, or for an SVG its shapes.
enum Source {
    Pixels(Raw),
    Shapes(Box<usvg::Tree>),
}

/// Reads a picture to draw. An SVG's text needs the system's fonts, which
/// are read once, when the first one is.
fn load(path: &Path, fonts: &mut Option<Arc<fontdb::Database>>) -> Option<Source> {
    let (format, _, file) = open(path)?;
    if let Format::Svg = format {
        let fonts = fonts.get_or_insert_with(svg_fonts).clone();
        return parse_svg(path, file, Some(fonts)).map(|tree| Source::Shapes(Box::new(tree)));
    }
    decode_as(format, file).map(Source::Pixels)
}

#[cfg(test)]
fn decode(path: &Path) -> Option<Raw> {
    let (format, _, file) = open(path)?;
    decode_as(format, file)
}

fn decode_as(format: Format, file: BufReader<File>) -> Option<Raw> {
    match format {
        Format::Png => {
            let options = DecoderOptions::default().png_set_strip_to_8bit(true);
            let mut d = PngDecoder::new_with_options(file, options);
            let px = d.decode().ok()?.u8()?;
            let (w, h) = d.dimensions()?;
            Raw::from_pixels(w, h, d.colorspace()?.num_components(), &px)
        }
        Format::Jpeg => {
            let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
            let mut d = JpegDecoder::new_with_options(file, options);
            let px = d.decode().ok()?;
            let info = d.info()?;
            let raw = Raw::from_pixels(info.width as usize, info.height as usize, 3, &px)?;
            Some(raw.oriented(orientation(d.exif())))
        }
        Format::Gif => {
            let mut options = gif::DecodeOptions::new();
            options.set_color_output(gif::ColorOutput::RGBA);
            let mut d = options.read_info(file).ok()?;
            let (w, h) = (d.width() as usize, d.height() as usize);
            let (mut canvas, mut frames, mut delays) = (vec![0u8; w * h * 4], Vec::new(), Vec::new());
            while frames.len() + canvas.len() <= FRAME_BYTES {
                let Ok(Some(frame)) = d.read_next_frame() else { break };
                let before = (frame.dispose == gif::DisposalMethod::Previous).then(|| canvas.clone());
                let (left, top) = (frame.left as usize, frame.top as usize);
                let (fw, fh) = (frame.width as usize, frame.height as usize);
                // Each frame is laid over what the last one left; its clear
                // pixels let that show.
                for (y, row) in frame.buffer.chunks_exact(fw * 4).take(fh).enumerate() {
                    for (x, px) in row.chunks_exact(4).enumerate().filter(|(_, px)| px[3] != 0) {
                        let (x, y) = (left + x, top + y);
                        if x < w && y < h {
                            canvas[(y * w + x) * 4..][..4].copy_from_slice(px);
                        }
                    }
                }
                frames.extend_from_slice(&canvas);
                // As browsers do, a frame with next to no delay shows for a tenth of a second.
                delays.push(if frame.delay <= 1 { 100 } else { frame.delay as u32 * 10 });
                match frame.dispose {
                    gif::DisposalMethod::Background => {
                        for y in top.min(h)..(top + fh).min(h) {
                            canvas[(y * w + left.min(w)) * 4..(y * w + (left + fw).min(w)) * 4].fill(0);
                        }
                    }
                    gif::DisposalMethod::Previous => canvas = before.unwrap_or(canvas),
                    _ => {}
                }
            }
            match delays.len() {
                0 => None,
                1 => Raw::from_pixels(w, h, 4, &frames),
                _ => Some(Raw::animation(w, h, frames, delays)),
            }
        }
        Format::WebP => {
            let mut d = image_webp::WebPDecoder::new(file).ok()?;
            let (w, h) = d.dimensions();
            let mut px = vec![0u8; d.output_buffer_size()?];
            d.read_image(&mut px).ok()?;
            Raw::from_pixels(w as usize, h as usize, if d.has_alpha() { 4 } else { 3 }, &px)
        }
        Format::Svg => None,
    }
}

fn worker(jobs: Receiver<Job>, done: Sender<Done>, waker: Option<Waker>) {
    // The last full-size decode is kept: resizing the window asks for the
    // same image again and again at new sizes.
    let mut last: Option<(PathBuf, Option<Source>)> = None;
    let mut fonts = None;
    // A PDF's pages are drawn at the size asked for, in colour, not decoded
    // and scaled.
    let mut shelf = crate::pdf::Shelf::default();
    while let Ok(job) = jobs.recv() {
        let mut batch = vec![job];
        batch.extend(jobs.try_iter());
        for i in 0..batch.len() {
            // A newer request for the same image supersedes this one.
            if batch[i + 1..].iter().any(|j| j.path == batch[i].path) {
                continue;
            }
            let job = Job { path: batch[i].path.clone(), ..batch[i] };
            let (w, h) = (job.w.max(1) as usize, job.h.max(1) as usize);
            let picture = whole(&job.path);
            let pic = if picture.is_none() && crate::pdf::is_clear(&job.path) {
                // A formula: dark on nothing, and so made light on a dark page.
                let rgba = shelf.draw_clear(&job.path, w as u32, h as u32);
                rgba.and_then(|px| Raw::from_pixels(w, h, 4, &px)).map(|raw| raw.scaled(w, h, true))
            } else if picture.is_none() && crate::pdf::is_page(&job.path) {
                let rgb = shelf.draw(&job.path, w as u32, h as u32);
                rgb.map(|rgb| Pic { rgb, ..Pic::new(w as u32, h as u32) })
            } else {
                if last.as_ref().is_none_or(|(key, _)| *key != job.path) {
                    let file = picture.as_deref().unwrap_or(&job.path);
                    last = Some((job.path.clone(), load(file, &mut fonts)));
                }
                let lighten = picture.is_none();
                match last.as_ref().and_then(|(_, source)| source.as_ref()) {
                    Some(Source::Pixels(raw)) => Some(raw.scaled(w, h, lighten)),
                    Some(Source::Shapes(tree)) => render_svg(tree, w, h).map(|raw| raw.scaled(w, h, lighten)),
                    None => None,
                }
            };
            if done.send(Done { job, pic }).is_err() {
                return;
            }
            if let Some(wake) = &waker {
                wake();
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A PNG of `w` by `h` RGBA pixels, stored without compression.
    pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in bytes {
                c ^= b as u32;
                for _ in 0..8 {
                    c = if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            let start = out.len();
            out.extend(kind);
            out.extend(data);
            let sum = crc(&out[start..]);
            out.extend(sum.to_be_bytes());
        }
        let rows: Vec<u8> = rgba.chunks_exact(w as usize * 4).flat_map(|r| [&[0u8][..], r].concat()).collect();
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = rows.chunks(65_535).collect();
        for (i, block) in blocks.iter().enumerate() {
            z.push((i + 1 == blocks.len()) as u8);
            let n = block.len() as u16;
            z.extend(n.to_le_bytes());
            z.extend((!n).to_le_bytes());
            z.extend(*block);
        }
        let (mut s1, mut s2) = (1u32, 0u32);
        for &b in &rows {
            s1 = (s1 + b as u32) % 65_521;
            s2 = (s2 + s1) % 65_521;
        }
        z.extend((s2 << 16 | s1).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut head = [w.to_be_bytes(), h.to_be_bytes()].concat();
        head.extend([8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &head);
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn resampling_preserves_flat_areas_and_averages() {
        let raw = Raw { w: 4, h: 2, ch: 1, data: vec![0, 0, 200, 200, 0, 0, 200, 200], delays: Vec::new() };
        assert_eq!(raw.scaled(2, 1, true).la, [0, 255, 200, 255]);
        assert_eq!(raw.scaled(1, 1, true).la, [100, 255]);
        let up = raw.scaled(8, 4, true);
        assert_eq!((up.la[0], up.la[14]), (0, 200));
    }

    #[test]
    fn colour_and_alpha_survive_resampling_and_drawing() {
        let raw = Raw::from_pixels(2, 1, 4, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        assert_eq!(raw.scaled(1, 1, true).rgba, [128, 0, 128, 255]);
        let pic = Raw::from_pixels(1, 1, 4, &[255, 0, 0, 128]).unwrap().scaled(1, 1, true);
        let mut px = [0x0000ff];
        crate::gfx::Canvas::new(&mut px, 1, 1).image(0, 0, 1, 1, &pic);
        assert_eq!(px[0], 0x80007f);
    }

    #[test]
    fn orientation_is_read_and_applied() {
        let mut exif = b"MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        exif.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
        assert_eq!(orientation(Some(&exif)), 6);
        assert_eq!(orientation(None), 1);
        // Rotating a 2x1 image a quarter turn clockwise stacks it top to bottom.
        let turned = Raw { w: 2, h: 1, ch: 1, data: vec![10, 20], delays: Vec::new() }.oriented(6);
        assert_eq!((turned.w, turned.h, turned.data), (1, 2, vec![10, 20]));
        let back = Raw { w: 2, h: 1, ch: 1, data: vec![10, 20], delays: Vec::new() }.oriented(8);
        assert_eq!(back.data, [20, 10]);
    }

    #[test]
    fn dark_art_on_transparency_is_lightened() {
        // One opaque black pixel beside a transparent one.
        let raw = Raw::from_pixels(2, 1, 4, &[0, 0, 0, 255, 0, 0, 0, 0]).unwrap();
        assert_eq!(raw.scaled(2, 1, true).la, [255, 255, 0, 0]);
        let photo = Raw::from_pixels(2, 1, 3, &[0, 0, 0, 255, 255, 255]).unwrap();
        assert_eq!(photo.scaled(2, 1, true).la, [0, 255, 255, 255]);
    }

    #[test]
    fn a_picture_on_its_own_keeps_its_colours() {
        let art = Raw::from_pixels(2, 1, 4, &[0, 0, 0, 255, 0, 0, 0, 0]).unwrap();
        let pic = art.scaled(2, 1, false);
        assert_eq!(pic.la, [0, 255, 0, 0]);
        assert!(!pic.opaque && Raw::from_pixels(1, 1, 3, &[9, 9, 9]).unwrap().scaled(1, 1, false).opaque);
        let key = whole_key(Path::new("/a/b.png"), "12-34");
        assert_eq!(whole(&key).as_deref(), Some(Path::new("/a/b.png")));
        assert_eq!(whole(Path::new("/a/b.png")), None);
    }

    /// A frame of a test GIF: its left, top and width, its pixels, its
    /// delay and its disposal.
    pub type GifFrame = (u16, u16, u16, Vec<u8>, u16, gif::DisposalMethod);

    /// A GIF `w` by `h` of frames, each placed at its left and top, its
    /// pixels picked from clear, red and blue, shown for a number of
    /// hundredths of a second and then disposed of.
    pub fn gif(w: u16, h: u16, frames: &[GifFrame]) -> Vec<u8> {
        let mut out = Vec::new();
        let palette = [0, 0, 0, 255, 0, 0, 0, 0, 255];
        let mut e = gif::Encoder::new(&mut out, w, h, &palette).unwrap();
        for (left, top, fw, px, delay, dispose) in frames {
            let fh = px.len() as u16 / fw;
            let mut f = gif::Frame::from_palette_pixels(*fw, fh, px.clone(), palette, Some(0));
            (f.left, f.top, f.delay, f.dispose) = (*left, *top, *delay, *dispose);
            e.write_frame(&f).unwrap();
        }
        drop(e);
        out
    }

    #[test]
    fn gif_frames_are_laid_over_each_other() {
        use gif::DisposalMethod::{Background, Keep};
        let file = std::env::temp_dir().join(format!("lightdown-anim-{}.gif", std::process::id()));
        let frames = [
            (0, 0, 2, vec![1, 2], 5, Keep),
            // Blue over the red, cleared away after.
            (0, 0, 1, vec![2], 0, Background),
            // Clear itself, so what is under it shows.
            (1, 0, 1, vec![0], 2, Keep),
        ];
        std::fs::write(&file, gif(2, 1, &frames)).unwrap();
        let raw = decode(&file).unwrap();
        let (red, blue, clear) = ([255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 0, 0]);
        assert_eq!(raw.data, [red, blue, blue, blue, clear, blue].concat());
        assert_eq!(raw.delays, [50, 100, 20]);
        let mut pic = raw.scaled(2, 1, true);
        assert_eq!((pic.opaque, pic.shown()), (false, &[red, blue].concat()[..]));
        std::fs::remove_file(file).unwrap();

        // Each frame shows for its time, round and round.
        let t = Instant::now();
        pic.born = Some(t);
        let ms = |n| t + Duration::from_millis(n);
        assert_eq!(pic.play(ms(60)), Some(ms(150)));
        assert_eq!(pic.shown(), [blue, blue].concat());
        assert_eq!((pic.play(ms(165)), pic.frame), (Some(ms(170)), 2));
        assert_eq!((pic.play(ms(175)), pic.frame), (Some(ms(220)), 0));
    }

    #[test]
    fn svg_is_known_by_how_it_begins() {
        let p = Path::new("a");
        let doctype = b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\" [\n<!ENTITY ns \"y\">\n]>\n<!-- made by hand -->\n<svg xmlns=\"http://www.w3.org/2000/svg\">";
        assert!(matches!(sniff(doctype, p), Some(Format::Svg)));
        assert!(is_svg(b"  <svg>") && is_svg(b"<svg/>"));
        assert!(!is_svg(b"<!DOCTYPE html><html><svg></svg></html>") && !is_svg(b"<svgfoo>") && !is_svg(b"<svg"));
        assert!(
            sniff(&[0x1f, 0x8b, 8], p).is_none()
                && matches!(sniff(&[0x1f, 0x8b], Path::new("a.svgz")), Some(Format::Svg))
        );
    }

    #[test]
    fn svg_is_drawn_at_the_size_shown() {
        let file = std::env::temp_dir().join(format!("lightdown-shapes-{}.svg", std::process::id()));
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20.5">
            <rect width="20" height="21" fill="red"/>
            <text x="22" y="15" font-family="No Such Font" font-size="14">Ab</text></svg>"#;
        std::fs::write(&file, svg).unwrap();
        assert!(is_vector(&file));
        assert_eq!(probe(&file), Some((40, 21)));
        let Some(Source::Shapes(tree)) = load(&file, &mut None) else { panic!("not read as an SVG") };
        let raw = render_svg(&tree, 400, 205).unwrap();
        assert_eq!((raw.w, raw.h, raw.ch), (400, 205, 4));
        let at = |x: usize, y: usize| &raw.data[(y * 400 + x) * 4..][..4];
        assert_eq!((at(10, 10), at(390, 5)), (&[255, 0, 0, 255][..], &[0, 0, 0, 0][..]));
        // Text in a font there is not is drawn in another.
        assert!(raw.data.chunks_exact(4).skip(200).any(|p| p[3] != 0 && p[0] == 0));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn the_test_png_decodes() {
        let file = std::env::temp_dir().join(format!("lightdown-png-{}.png", std::process::id()));
        std::fs::write(&file, png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128])).unwrap();
        assert_eq!(probe(&file), Some((2, 1)));
        let raw = decode(&file).unwrap();
        assert_eq!((raw.ch, raw.data), (4, vec![255, 0, 0, 255, 0, 0, 128, 128]));
        std::fs::remove_file(file).unwrap();
    }
}

/// Rasterize cached diagram shapes for the existing PDF image exporter.
pub fn svg_png(source: &Path, target: &Path) -> Option<()> {
    let (_, _, file) = open(source)?;
    let tree = parse_svg(source, file, Some(svg_fonts()))?;
    let size = tree.size();
    let (w, h) = (size.width().ceil() as u32, size.height().ceil() as u32);
    if w as u64 * h as u64 > 32_000_000 { return None; }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    pixmap.save_png(target).ok()
}
