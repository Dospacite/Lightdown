//! Where a PDF's links go, and its bookmarks, read once per file on a
//! thread of their own. Only jumps within the document and web or mail
//! addresses are kept: actions that launch programs, open other files or
//! run scripts are left out.
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier, Rect};
use hayro::hayro_interpret::util::TransformExt;
use hayro::kurbo::{Affine, Point};

use crate::kernel::Waker;

/// The most links read from a page, and bookmarks from a document.
const MOST_LINKS: usize = 2_000;
const MOST_BOOKMARKS: usize = 20_000;

/// Where a link or bookmark leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A web or mail address.
    Uri(String),
    /// A page, counted from 0, and how far down it, in points from its top.
    Page { page: usize, y: Option<f32> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// Left, top, right and bottom, in points from the page's top left.
    pub rect: [f32; 4],
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub depth: usize,
    pub title: String,
    pub target: Option<Target>,
}

#[derive(Clone, Debug, Default)]
pub struct Marks {
    /// Each page's links.
    pub links: Vec<Vec<Link>>,
    /// The outline, in reading order.
    pub bookmarks: Vec<Bookmark>,
}

/// PDFDocEncoding's characters from 0x18 to 0x1F and from 0x80 to 0xA0
/// that are not Latin-1's.
const LOW: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
const HIGH: [char; 33] = [
    '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚', '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ',
    'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{fffd}', '€',
];

/// A PDF text string: UTF-16 or UTF-8 behind a byte-order mark, or else
/// PDFDocEncoding.
pub fn text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes
        .iter()
        .map(|&b| match b {
            0x18..=0x1f => LOW[(b - 0x18) as usize],
            0x80..=0xa0 => HIGH[(b - 0x80) as usize],
            _ => b as char,
        })
        .collect()
}

/// Addresses a click may hand to the desktop.
fn uri(address: String) -> Option<Target> {
    let lower = address.to_ascii_lowercase();
    let allowed = ["http://", "https://", "mailto:"].iter().any(|p| lower.starts_with(p));
    (allowed && address.len() <= 4096 && !address.chars().any(char::is_control)).then_some(Target::Uri(address))
}

struct Resolver<'a, 'p> {
    /// Each page's object and how its points map to the top-left frame.
    pages: &'p [(Option<ObjectIdentifier>, Affine)],
    catalog: Option<&'p Dict<'a>>,
}

impl<'a> Resolver<'a, '_> {
    /// A destination named in the catalog's `/Dests` or its name tree.
    fn named(&self, key: &[u8]) -> Option<Object<'a>> {
        let catalog = self.catalog?;
        if let Some(found) = catalog.get::<Dict<'a>>(b"Dests").and_then(|d| d.get::<Object<'a>>(key)) {
            return Some(found);
        }
        let mut stack = vec![catalog.get::<Dict<'a>>(b"Names")?.get::<Dict<'a>>(b"Dests")?];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if seen.len() > 4096 || node.obj_id().is_some_and(|id| !seen.insert(id)) {
                continue;
            }
            if let Some(names) = node.get::<Array<'a>>(b"Names") {
                let mut pairs = names.iter::<Object<'a>>();
                while let (Some(name), Some(value)) = (pairs.next(), pairs.next()) {
                    if matches!(&name, Object::String(s) if s.as_bytes() == key) {
                        return Some(value);
                    }
                }
            }
            if let Some(kids) = node.get::<Array<'a>>(b"Kids") {
                stack.extend(kids.iter::<Dict<'a>>());
            }
        }
        None
    }

    fn dest(&self, d: Object<'a>, depth: usize) -> Option<Target> {
        if depth > 4 {
            return None;
        }
        match d {
            Object::Array(a) => {
                let mut items = a.raw_iter();
                let page = match items.next()? {
                    MaybeRef::Ref(r) => {
                        let id: ObjectIdentifier = r.into();
                        self.pages.iter().position(|p| p.0 == Some(id))?
                    }
                    // A page number, as in links to other files.
                    MaybeRef::NotRef(o) => usize::try_from(o.into_i32()?).ok().filter(|n| *n < self.pages.len())?,
                };
                let not_ref = |m: MaybeRef<Object<'a>>| match m {
                    MaybeRef::NotRef(o) => Some(o),
                    MaybeRef::Ref(_) => None,
                };
                let kind = items.next().and_then(not_ref).and_then(Object::into_name);
                let numbers: Vec<Option<f32>> = items.map(|m| not_ref(m).and_then(Object::into_f32)).collect();
                let top = match kind.as_deref() {
                    Some(b"XYZ") => numbers.get(1).copied().flatten(),
                    Some(b"FitH" | b"FitBH") => numbers.first().copied().flatten(),
                    Some(b"FitR") => numbers.get(3).copied().flatten(),
                    _ => None,
                };
                let y = top.map(|top| ((self.pages[page].1 * Point::new(0.0, top as f64)).y as f32).max(0.0));
                Some(Target::Page { page, y })
            }
            Object::Dict(d) => self.dest(d.get::<Object<'a>>(b"D")?, depth + 1),
            Object::Name(n) => self.dest(self.named(&n)?, depth + 1),
            Object::String(s) => self.dest(self.named(s.as_bytes())?, depth + 1),
            _ => None,
        }
    }

    fn action(&self, a: &Dict<'a>) -> Option<Target> {
        match a.get::<Name<'a>>(b"S").as_deref()? {
            b"URI" => uri(text(a.get::<hayro::hayro_interpret::hayro_syntax::object::String<'a>>(b"URI")?.as_bytes())),
            b"GoTo" => self.dest(a.get::<Object<'a>>(b"D")?, 0),
            _ => None,
        }
    }

    fn link(&self, n: usize, annot: &Dict<'a>) -> Option<Link> {
        if annot.get::<Name<'a>>(b"Subtype").as_deref() != Some(b"Link".as_slice()) {
            return None;
        }
        let rect = annot.get::<Rect>(b"Rect")?;
        let target = match annot.get::<Dict<'a>>(b"A") {
            Some(a) => self.action(&a)?,
            None => self.dest(annot.get::<Object<'a>>(b"Dest")?, 0)?,
        };
        let t = self.pages[n].1;
        let (a, b) = (t * Point::new(rect.x0, rect.y0), t * Point::new(rect.x1, rect.y1));
        Some(Link { rect: [a.x.min(b.x) as f32, a.y.min(b.y) as f32, a.x.max(b.x) as f32, a.y.max(b.y) as f32], target })
    }

    /// The outline in reading order: each item, then what is under it, then
    /// the item after it. Loops in a damaged outline are cut.
    fn bookmarks(&self, first: Option<Dict<'a>>) -> Vec<Bookmark> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(first, 0usize)];
        while let Some((item, depth)) = stack.pop() {
            let Some(item) = item else { continue };
            if out.len() >= MOST_BOOKMARKS || depth > 64 || item.obj_id().is_some_and(|id| !seen.insert(id)) {
                continue;
            }
            let title = item.get::<hayro::hayro_interpret::hayro_syntax::object::String<'a>>(b"Title").map_or(String::new(), |t| text(t.as_bytes()));
            let target = match item.get::<Dict<'a>>(b"A") {
                Some(a) => self.action(&a),
                None => item.get::<Object<'a>>(b"Dest").and_then(|d| self.dest(d, 0)),
            };
            out.push(Bookmark { depth, title: title.trim().replace(['\r', '\n'], " "), target });
            stack.push((item.get::<Dict<'a>>(b"Next"), depth));
            stack.push((item.get::<Dict<'a>>(b"First"), depth + 1));
        }
        out
    }
}

pub fn marks(pdf: &Pdf) -> Marks {
    let pages: Vec<_> = pdf.pages().iter().map(|p| (p.raw().obj_id(), p.initial_transform(true).to_kurbo())).collect();
    let xref = pdf.xref();
    let catalog = xref.get::<Dict<'_>>(xref.root_id());
    let r = Resolver { pages: &pages, catalog: catalog.as_ref() };
    let links = pdf
        .pages()
        .iter()
        .enumerate()
        .map(|(n, page)| {
            let annots = page.raw().get::<Array<'_>>(b"Annots");
            annots.map_or(Vec::new(), |a| a.iter::<Dict<'_>>().take(MOST_LINKS).filter_map(|annot| r.link(n, &annot)).collect())
        })
        .collect();
    let first = catalog.as_ref().and_then(|c| c.get::<Dict<'_>>(b"Outlines")).and_then(|o| o.get::<Dict<'_>>(b"First"));
    Marks { links, bookmarks: r.bookmarks(first) }
}

/// Reads a PDF's links and bookmarks on a thread of their own.
pub fn read_marks(path: PathBuf, wake: Waker) -> Receiver<Marks> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let found = super::load(&path).map(|pdf| marks(&pdf)).unwrap_or_default();
        if tx.send(found).is_ok() {
            wake();
        }
    });
    rx
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Arc;

    /// A PDF from its objects, numbered from 1, the first the catalog.
    pub fn build(objects: &[&str]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
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
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    /// Two pages; links to the web, to a named place and to a program; an
    /// outline with a child, a UTF-16 title and a loop.
    pub fn linked() -> Vec<u8> {
        build(&[
            "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R /Names << /Dests 10 0 R >> /Dests << /second [4 0 R /XYZ 0 500 0] >> >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Annots [5 0 R 11 0 R 12 0 R] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] >>",
            "<< /Type /Annot /Subtype /Link /Rect [100 700 200 720] /A << /S /URI /URI (https://example.com/a) >> >>",
            "<< /Type /Outlines /First 7 0 R /Last 8 0 R >>",
            "<< /Title (Intro) /Parent 6 0 R /Next 8 0 R /First 9 0 R /Dest [3 0 R /XYZ 0 800 0] >>",
            "<< /Title <FEFF0050006100720074002000420020203A> /Parent 6 0 R /Prev 7 0 R /A << /S /GoTo /D (named-two) >> >>",
            "<< /Title (Detail) /Parent 7 0 R /Next 9 0 R /Dest /second >>",
            "<< /Names [(named-two) [4 0 R /FitH 300]] >>",
            "<< /Type /Annot /Subtype /Link /Rect [100 600 200 620] /Dest (named-two) >>",
            "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /Launch /F (calc.exe) >> >>",
        ])
    }

    #[test]
    fn links_and_bookmarks_resolve_to_pages_and_safe_addresses() {
        let pdf = Pdf::new(Arc::new(linked())).unwrap();
        let m = marks(&pdf);
        assert_eq!(m.links.len(), 2);
        assert_eq!(
            m.links[0],
            vec![
                Link { rect: [100.0, 80.0, 200.0, 100.0], target: Target::Uri("https://example.com/a".into()) },
                Link { rect: [100.0, 180.0, 200.0, 200.0], target: Target::Page { page: 1, y: Some(500.0) } },
            ]
        );
        assert!(m.links[1].is_empty());
        let outline: Vec<_> = m.bookmarks.iter().map(|b| (b.depth, b.title.as_str(), b.target.clone())).collect();
        assert_eq!(
            outline,
            vec![
                (0, "Intro", Some(Target::Page { page: 0, y: Some(0.0) })),
                (1, "Detail", Some(Target::Page { page: 1, y: Some(300.0) })),
                (0, "Part B ›", Some(Target::Page { page: 1, y: Some(500.0) })),
            ]
        );
    }

    #[test]
    fn strings_decode_and_only_web_and_mail_addresses_are_kept() {
        assert_eq!(text(b"Caf\xe9 \x84 \x18"), "Café — ˘");
        assert_eq!(text(&[0xfe, 0xff, 0x00, 0x41, 0x20, 0xac]), "A€");
        assert_eq!(text(b"\xef\xbb\xbfna\xc3\xafve"), "naïve");
        assert!(uri("HTTPS://example.com".into()).is_some() && uri("mailto:a@b.c".into()).is_some());
        assert!(uri("file:///etc/passwd".into()).is_none() && uri("javascript:alert(1)".into()).is_none());
        assert!(uri("https://a\n.example".into()).is_none());
    }
}
