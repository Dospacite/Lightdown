//! SyncTeX: where in the PDF a line of the source went, and which line a
//! place in the PDF came from, read from the `.synctex.gz` file that TeX
//! writes beside the PDF when it is asked to.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// A rectangle on a page, in PDF points from its top left corner: left,
/// top, width and height.
pub type Rect = [f32; 4];

/// A record of the file: something TeX put on a page, from a line of a
/// source file.
#[derive(Clone, Debug)]
struct Record {
    /// Pages count from 0.
    page: usize,
    tag: u32,
    line: usize,
    /// Where the record itself is, across.
    x: f32,
    /// The line of text it stands in: its own box, or the box around it.
    rect: Rect,
}

pub struct Sync {
    inputs: HashMap<u32, PathBuf>,
    records: Vec<Record>,
}

/// The SyncTeX file of a PDF, beside it.
pub fn file_of(pdf: &Path) -> PathBuf {
    pdf.with_extension("synctex.gz")
}

/// A path as it can be compared with another.
fn same(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl Sync {
    /// Reads the SyncTeX file of a PDF.
    pub fn open(pdf: &Path) -> Option<Sync> {
        let bytes = std::fs::read(file_of(pdf)).ok()?;
        let mut text = String::new();
        flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut text).ok()?;
        Some(Sync::parse(&text, pdf.parent().unwrap_or(Path::new("."))))
    }

    /// Reads SyncTeX's text. Relative paths are taken from `dir`.
    pub fn parse(text: &str, dir: &Path) -> Sync {
        let mut inputs = HashMap::new();
        let mut records = Vec::new();
        let (mut unit, mut mag, mut x_off, mut y_off) = (1.0f32, 1000.0f32, 0.0f32, 0.0f32);
        let mut page = 0;
        // The boxes open around where the reading is: horizontal ones give
        // their line of text to what is in them.
        let mut stack: Vec<Option<Rect>> = Vec::new();
        let mut content = false;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Input:") {
                if let Some((tag, path)) = rest.split_once(':')
                    && let Ok(tag) = tag.parse()
                    && !path.is_empty()
                {
                    inputs.insert(tag, same(&dir.join(path)));
                }
                continue;
            }
            if !content {
                let num = |s: &str| s.trim().parse::<f32>().ok();
                if let Some(v) = line.strip_prefix("Unit:").and_then(num) {
                    unit = v;
                } else if let Some(v) = line.strip_prefix("Magnification:").and_then(num) {
                    mag = v;
                } else if let Some(v) = line.strip_prefix("X Offset:").and_then(num) {
                    x_off = v;
                } else if let Some(v) = line.strip_prefix("Y Offset:").and_then(num) {
                    y_off = v;
                }
                content = line.starts_with("Content:");
                continue;
            }
            // Scaled points, as TeX counts them, into PDF points.
            let k = unit * mag / 1000.0 / 65536.0 * 72.0 / 72.27;
            let Some(kind) = line.chars().next() else { continue };
            match kind {
                '{' => {
                    page = line[1..].parse::<usize>().unwrap_or(1).saturating_sub(1);
                    stack.clear();
                    continue;
                }
                ']' | ')' => {
                    stack.pop();
                    continue;
                }
                '[' | '(' | 'h' | 'v' | 'x' | 'k' | 'g' | '$' => {}
                _ => continue,
            }
            let Some(rec) = record(&line[1..]) else {
                if matches!(kind, '[' | '(') {
                    stack.push(None);
                }
                continue;
            };
            let (tag, at, (x, y), size) = rec;
            let (x, y) = ((x + x_off) * k, (y + y_off) * k);
            let own = size.map(|(w, h, d)| [x, y - h * k, w * k, ((h + d) * k).max(1.0)]);
            let rect = match kind {
                '(' | 'h' => own,
                _ => stack.iter().rev().flatten().next().copied().or(own),
            };
            match kind {
                '(' => stack.push(own),
                '[' => stack.push(None),
                _ => {}
            }
            // Vertical boxes hold whole pages and paragraphs, not lines.
            if let Some(rect) = rect.filter(|_| !matches!(kind, '[' | 'v')) {
                records.push(Record { page, tag, line: at, x, rect });
            }
        }
        Sync { inputs, records }
    }

    /// Where a line of a source file went: a page, and the lines of text
    /// on it that came from that line, or from the nearest line after it
    /// that went anywhere.
    pub fn forward(&self, file: &Path, line: usize) -> Option<(usize, Rect)> {
        let file = same(file);
        let tags: Vec<u32> = self.inputs.iter().filter(|(_, p)| **p == file).map(|(t, _)| *t).collect();
        let ours = || self.records.iter().filter(|r| tags.contains(&r.tag));
        let best = ours().map(|r| r.line).filter(|&l| l >= line).min().or_else(|| ours().map(|r| r.line).max())?;
        let hits: Vec<&Record> = ours().filter(|r| r.line == best).collect();
        let page = hits.iter().map(|r| r.page).min()?;
        let rects = hits.iter().filter(|r| r.page == page).map(|r| r.rect);
        let union = rects.reduce(|a, b| {
            let (l, t) = (a[0].min(b[0]), a[1].min(b[1]));
            let (r, bottom) = ((a[0] + a[2]).max(b[0] + b[2]), (a[1] + a[3]).max(b[1] + b[3]));
            [l, t, r - l, bottom - t]
        })?;
        Some((page, union))
    }

    /// The source file and line a place on a page came from.
    pub fn inverse(&self, page: usize, x: f32, y: f32) -> Option<(PathBuf, usize)> {
        let on = |r: &&Record| r.page == page && self.inputs.contains_key(&r.tag);
        let within = |r: &Rect| x >= r[0] - 2.0 && x <= r[0] + r[2] + 2.0 && y >= r[1] - 2.0 && y <= r[1] + r[3] + 2.0;
        let around: Vec<&Record> = self.records.iter().filter(on).filter(|r| within(&r.rect)).collect();
        let pick = if around.is_empty() {
            // Nothing there: the line of text nearest above or below.
            let gap = |r: &Record| (y - (r.rect[1] + r.rect[3] / 2.0)).abs() + (x - r.x).abs() * 0.01;
            self.records.iter().filter(on).min_by(|a, b| gap(a).total_cmp(&gap(b)))?
        } else {
            // In a line of text, what stands last before the point.
            let before = around.iter().filter(|r| r.x <= x).max_by(|a, b| a.x.total_cmp(&b.x));
            before.or_else(|| around.iter().min_by(|a, b| a.x.total_cmp(&b.x))).copied()?
        };
        Some((self.inputs.get(&pick.tag)?.clone(), pick.line))
    }
}

/// What a record says after its kind: its file's tag and line, where it
/// is, and the width, height and depth of a box.
#[allow(clippy::type_complexity)]
fn record(s: &str) -> Option<(u32, usize, (f32, f32), Option<(f32, f32, f32)>)> {
    let mut parts = s.split(':');
    let (tag, line) = parts.next()?.split_once(',')?;
    let (x, y) = parts.next()?.split_once(',')?;
    let size = parts.next().and_then(|s| {
        let mut n = s.split(',').map(|v| v.parse::<f32>().ok());
        let w = n.next()??;
        Some((w, n.next().flatten().unwrap_or(0.0), n.next().flatten().unwrap_or(0.0)))
    });
    Some((tag.parse().ok()?, line.parse().ok()?, (x.parse().ok()?, y.parse().ok()?), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Tectonic wrote for a page with a heading and two paragraphs.
    const SAMPLE: &str = "SyncTeX Version:1\nInput:1:/doc/a.tex\nInput:2:\nOutput:pdf\nMagnification:1000\nUnit:1\n\
        X Offset:0\nY Offset:0\nContent:\n!158\n{1\n[1,7:4736287,46220575:26673152,41484288,0\n\
        [1,7:8799519,44254495:22609920,36044800,0\n(1,3:8799519,8865055:22609920,647391,5661\n\
        g1,3:8799519,8865055\n)\n(1,5:8799519,10301145:22609920,454820,7208\ng1,4:10492314,10301145\n\
        k1,5:31409439,10301145:19148964\n)\n(1,7:8799519,11087577:22609920,462029,135003\n\
        h1,6:8799519,11087577:983040,0,0\ng1,6:12003574,11087577\n)\n]\n]\n}1\n{2\n\
        [1,9:4736287,46220575:26673152,41484288,0\n(1,8:8799519,8865055:22609920,647391,5661\n\
        g1,8:8799519,8865055\n)\n]\n}2\n";

    #[test]
    fn lines_and_places_lead_to_each_other() {
        let sync = Sync::parse(SAMPLE, Path::new("/"));
        let a = Path::new("/doc/a.tex");
        // The heading on line 3 is on the first page, 134 points across.
        let (page, r) = sync.forward(a, 3).unwrap();
        assert_eq!(page, 0);
        assert!((r[0] - 133.8).abs() < 0.5 && (r[2] - 343.6).abs() < 1.0, "{r:?}");
        // Line 4's paragraph sits lower; line 8 is on the second page.
        let (_, four) = sync.forward(a, 4).unwrap();
        assert!(four[1] > r[1]);
        assert_eq!(sync.forward(a, 8).map(|f| f.0), Some(1));
        // And back: a click in the paragraph's line is line 4's.
        let (file, line) = sync.inverse(0, 200.0, four[1] + four[3] / 2.0).unwrap();
        assert_eq!((file.as_path(), line), (a, 4));
        assert_eq!(sync.inverse(1, 150.0, 130.0).map(|f| f.1), Some(8));
        assert!(sync.forward(Path::new("/doc/other.tex"), 1).is_none());
    }
}
