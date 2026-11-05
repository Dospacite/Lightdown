//! Multibuffers: excerpts of many files shown and edited as one document.
//!
//! The document is a `Doc::Cells` text: each excerpt is a cell whose header
//! names its file, and whose lines are a range of the file's lines. The
//! model here keeps, for each excerpt, the file and the range it shows and
//! the lines last seen there, so edits can be carried both ways: from the
//! multibuffer to the file's buffer, and from the file back to the excerpt.

use std::path::{Path, PathBuf};

use crate::cells::{self, Tag};
use crate::editor::Buffer;

/// Lines shown above and below each line of interest.
pub const CONTEXT: usize = 2;
/// Lines an excerpt grows by when expanded.
pub const STEP: usize = 5;
/// The most excerpts one multibuffer holds.
pub const MOST: usize = 2000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Search,
    Problems,
    References,
    Review,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Search => "Search",
            Kind::Problems => "Problems",
            Kind::References => "References",
            Kind::Review => "Review",
        }
    }
}

/// An agent's change shown in a review excerpt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hunk {
    /// The lines the change took away.
    pub removed: Vec<String>,
    /// The lines it wrote, in the file, from 0, end exclusive. Empty for a
    /// change that only removed lines; then it marks where they were.
    pub added: (usize, usize),
    /// The lines it replaced in what the file is reviewed against.
    pub old: (usize, usize),
}

#[derive(Clone, Debug)]
pub struct Excerpt {
    pub path: PathBuf,
    /// The file's lines shown, from 0, end exclusive. Never empty.
    pub start: usize,
    pub end: usize,
    /// The lines as last carried between the file and the multibuffer.
    pub lines: Vec<String>,
    /// Lines of interest in the file, from 0: matches, problems, references.
    pub hits: Vec<usize>,
    /// Shown under the excerpt, such as the problems' messages.
    pub notes: Vec<String>,
    pub hunk: Option<Hunk>,
}

/// A file that excerpts show: its text when last seen, and a buffer of its
/// own while no tab has the file open.
pub struct Source {
    pub path: PathBuf,
    pub text: String,
    pub version: u64,
    pub hidden: Option<Buffer>,
    /// The multibuffer has edited the file, so Ctrl+S there saves it.
    pub touched: bool,
    /// A hash of the file's bytes when the buffer of its own was read or
    /// last saved, to refuse saving over changes made on disk since.
    pub disk: u64,
}

/// A hash of a file's bytes; 0 for a file that is not there.
pub fn disk_hash(bytes: Option<&[u8]>) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(bytes) = bytes else { return 0 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish() | 1
}

/// A buffer edit made for the multibuffer: the file and its undo length after.
pub type Step = Vec<(PathBuf, usize)>;

pub struct Multi {
    pub kind: Kind,
    pub title: String,
    pub root: PathBuf,
    pub excerpts: Vec<Excerpt>,
    pub sources: Vec<Source>,
    /// The version of the multibuffer's own text last carried to the files.
    pub seen: u64,
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
    /// Undo (false) or redo (true) asked for by a key in the pane.
    pub request: Option<bool>,
    /// What a search looked for, to mark in the excerpts.
    pub pattern: Option<regex::Regex>,
    /// Matches to mark, as byte ranges of the multibuffer's text.
    pub marks: Vec<(usize, usize)>,
    /// Lines of the multibuffer to mark as added by a reviewed change.
    pub added: Vec<usize>,
    /// What the excerpts were last built from, to rebuild when it changes.
    pub stamp: u64,
}

/// The lines of a text, split at `\n`; an empty text has one empty line.
pub fn lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// Ranges of lines around `hits`, `context` lines each way, merged where
/// they touch or overlap, within a text of `count` lines.
pub fn ranges(hits: &[usize], context: usize, count: usize) -> Vec<(usize, usize)> {
    let mut hits: Vec<usize> = hits.iter().copied().filter(|&h| h < count).collect();
    hits.sort_unstable();
    hits.dedup();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for h in hits {
        let (s, e) = (h.saturating_sub(context), (h + context + 1).min(count));
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Ranges of lines to show for changes at `hunks` (their lines in the
/// file, in order and apart), with context that stops halfway to the next
/// change, so no two ranges overlap. A change that only removed lines shows
/// the line after where they were.
pub fn hunk_ranges(hunks: &[(usize, usize)], context: usize, count: usize) -> Vec<(usize, usize)> {
    let count = count.max(1);
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut core_end = 0;
    for &(a, b) in hunks {
        let (a, b) = if a >= count { (count - 1, count) } else { (a, b.max(a + 1).min(count)) };
        let mut s = a.saturating_sub(context);
        let e = (b + context).min(count);
        if let Some(prev) = out.last_mut().filter(|p| p.1 > s) {
            let mid = ((core_end + a) / 2).clamp(core_end.min(a), a.max(prev.0 + 1));
            prev.1 = mid.max(prev.0 + 1);
            s = prev.1;
        }
        out.push((s, e.max(s)));
        core_end = b;
    }
    out
}

/// Where two texts differ, by whole lines: the first line that differs,
/// and the end of the differing lines in the old text and in the new.
pub fn line_diff(old: &str, new: &str) -> (usize, usize, usize) {
    let (a, b) = (lines(old), lines(new));
    let p = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let most = a.len().min(b.len()) - p;
    let q = a.iter().rev().zip(b.iter().rev()).take(most).take_while(|(x, y)| x == y).count();
    (p, a.len() - q, b.len() - q)
}

/// The changes between two texts by lines, in order: where each starts in
/// the old text, how many lines it took away and how many it wrote. Lines
/// are matched as a longest common subsequence where the changed middle is
/// small enough, else the middle is one change.
pub fn line_changes(old: &str, new: &str) -> Vec<(usize, usize, usize)> {
    let (a, b) = (lines(old), lines(new));
    let p = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let most = a.len().min(b.len()) - p;
    let q = a.iter().rev().zip(b.iter().rev()).take(most).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[p..a.len() - q], &b[p..b.len() - q]);
    let (n, m) = (a.len(), b.len());
    if n == 0 && m == 0 {
        return Vec::new();
    }
    if n == 0 || m == 0 || n * m > 1 << 20 {
        return vec![(p, n, m)];
    }
    // Lengths of the longest common subsequences of the suffixes.
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * (m + 1) + j] = if a[i] == b[j] {
                table[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                table[(i + 1) * (m + 1) + j].max(table[i * (m + 1) + j + 1])
            };
        }
    }
    let (mut out, mut i, mut j) = (Vec::new(), 0, 0);
    let mut open: Option<(usize, usize)> = None;
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            if let Some((oi, oj)) = open.take() {
                out.push((p + oi, i - oi, j - oj));
            }
            (i, j) = (i + 1, j + 1);
            continue;
        }
        open.get_or_insert((i, j));
        if j < m && (i == n || table[i * (m + 1) + j + 1] >= table[(i + 1) * (m + 1) + j]) {
            j += 1;
        } else {
            i += 1;
        }
    }
    if let Some((oi, oj)) = open {
        out.push((p + oi, n - oi, m - oj));
    }
    out
}

/// Where a line range lies after all of `changes`, from `line_changes`.
pub fn map_through(range: (usize, usize), changes: &[(usize, usize, usize)]) -> (usize, usize) {
    changes.iter().rev().fold(range, |r, &(p, rem, add)| map_range(r, (p, p + rem, p + add)))
}

/// Where a line range lies after lines `p..old_end` became `p..new_end`.
/// A range the change took away whole comes back empty.
pub fn map_range((s, e): (usize, usize), (p, old_end, new_end): (usize, usize, usize)) -> (usize, usize) {
    let shift = |x: usize| (x + new_end).saturating_sub(old_end);
    if e <= p {
        return (s, e);
    }
    if s >= old_end {
        return (shift(s), shift(e));
    }
    let s2 = if s < p { s } else { s.min(new_end) };
    let e2 = if e >= old_end { shift(e) } else { e.min(new_end) };
    (s2, e2.max(s2))
}

/// Where a single line lies after the same change, if it is still there.
pub fn map_line(line: usize, change: (usize, usize, usize)) -> Option<usize> {
    let (s, e) = map_range((line, line + 1), change);
    (e > s).then_some(s)
}

/// The edit that makes lines `start..end` of `text` read `new`: a byte
/// range and what goes there, cut down to where they differ.
pub fn source_edit(text: &str, start: usize, end: usize, new: &[String]) -> (usize, usize, String) {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let count = starts.len();
    let (start, end) = (start.min(count), end.min(count).max(start.min(count)));
    let line_end = |l: usize| if l + 1 < count { starts[l + 1] - 1 } else { text.len() };
    let joined = new.join("\n");
    let (a, b, ins) = if start < end && !new.is_empty() {
        (starts[start], line_end(end - 1), joined)
    } else if start < end {
        // Every line goes: take a line break along.
        if end < count {
            (starts[start], starts[end], String::new())
        } else {
            (starts[start].saturating_sub(1), text.len(), String::new())
        }
    } else if new.is_empty() {
        (0, 0, String::new())
    } else if start < count {
        (starts[start], starts[start], joined + "\n")
    } else {
        (text.len(), text.len(), format!("\n{joined}"))
    };
    let old = &text[a..b];
    let mut head = old.bytes().zip(ins.bytes()).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(head) || !ins.is_char_boundary(head) {
        head -= 1;
    }
    let most = old.len().min(ins.len()) - head;
    let mut tail = old.bytes().rev().zip(ins.bytes().rev()).take(most).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(old.len() - tail) || !ins.is_char_boundary(ins.len() - tail) {
        tail -= 1;
    }
    (a + head, b - tail, ins[head..ins.len() - tail].to_string())
}

/// The path as the header shows it: from the project's folder if inside it.
pub fn short(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

impl Multi {
    pub fn new(kind: Kind, title: &str, root: &Path) -> Multi {
        Multi {
            kind,
            title: title.into(),
            root: root.into(),
            excerpts: Vec::new(),
            sources: Vec::new(),
            seen: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            request: None,
            pattern: None,
            marks: Vec::new(),
            added: Vec::new(),
            stamp: 0,
        }
    }

    pub fn source(&self, path: &Path) -> Option<&Source> {
        self.sources.iter().find(|s| s.path == path)
    }

    pub fn source_mut(&mut self, path: &Path) -> Option<&mut Source> {
        self.sources.iter_mut().find(|s| s.path == path)
    }

    /// Adds a file's excerpts around `hits`, in order after those there.
    /// Lines already shown for the file are left out.
    pub fn add_file(&mut self, path: &Path, text: &str, hits: &[usize], context: usize) -> usize {
        let all = lines(text);
        let mut added = 0;
        for (s, e) in ranges(hits, context, all.len()) {
            if self.excerpts.len() >= MOST {
                break;
            }
            if self.excerpts.iter().any(|x| x.path == path && x.start < e && s < x.end) {
                continue;
            }
            let hits = hits.iter().copied().filter(|h| (s..e).contains(h)).collect();
            let lines = all[s..e].iter().map(|l| l.to_string()).collect();
            self.excerpts.push(Excerpt {
                path: path.into(),
                start: s,
                end: e,
                lines,
                hits,
                notes: Vec::new(),
                hunk: None,
            });
            added += 1;
        }
        added
    }

    /// The multibuffer's text: each excerpt's header, then its lines.
    pub fn render(&self) -> String {
        if self.excerpts.is_empty() {
            return format!("{}\n", cells::header(Tag::Raw, "Nothing to show"));
        }
        let mut out = String::new();
        for (i, x) in self.excerpts.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&cells::header(Tag::Raw, &short(&self.root, &x.path).replace(cells::MARK, "")));
            out.push('\n');
            out.push_str(&x.lines.join("\n"));
        }
        out
    }

    /// What an excerpt's header says.
    pub fn label(&self, i: usize) -> String {
        let x = &self.excerpts[i];
        let line = x.hunk.as_ref().map(|h| h.added.0).or(x.hits.first().copied()).unwrap_or(x.start);
        format!("{}:{}", short(&self.root, &x.path), line + 1)
    }

    /// The controls at the right of an excerpt's header, in order.
    pub fn controls(&self, i: usize) -> &'static [&'static str] {
        match self.excerpts.get(i).and_then(|x| x.hunk.as_ref()) {
            Some(_) => &["Keep", "Revert", "\u{2191}", "\u{2193}"],
            None => &["\u{2191}", "\u{2193}"],
        }
    }

    /// The largest line number the gutter shows.
    pub fn widest(&self) -> usize {
        self.excerpts.iter().map(|x| x.end).max().unwrap_or(1)
    }

    /// Takes in the lines an excerpt now has in the multibuffer: the edit
    /// to make in its file's `text`, if any. The excerpt and those after it
    /// in the same file move with the edit.
    pub fn take(&mut self, i: usize, new: Vec<String>, text: &str) -> Option<(usize, usize, String)> {
        let x = &self.excerpts[i];
        if x.lines == new {
            return None;
        }
        let edit = source_edit(text, x.start, x.end, &new);
        let old = x.lines.join("\n");
        let (p, oe, ne) = line_diff(&old, &new.join("\n"));
        let (s, e) = (x.start, x.end);
        let change = (s + p, s + oe, s + ne);
        let path = x.path.clone();
        for (k, y) in self.excerpts.iter_mut().enumerate() {
            if y.path != path {
                continue;
            }
            if k == i {
                y.end = s + new.len();
                y.lines = new.clone();
            } else if y.start >= e {
                (y.start, y.end) = map_range((y.start, y.end), change);
            }
            y.hits = y.hits.iter().filter_map(|&h| map_line(h, change)).collect();
            if let Some(h) = &mut y.hunk {
                h.added = map_range(h.added, change);
            }
        }
        (edit.0 != edit.1 || !edit.2.is_empty()).then_some(edit)
    }

    /// Follows a change to a file made elsewhere: excerpts move with the
    /// lines, take the file's text, and those whose lines went away go.
    pub fn follow(&mut self, path: &Path, new: &str) {
        let Some(old) = self.source(path).map(|s| s.text.clone()) else { return };
        let changes = line_changes(&old, new);
        let all = lines(new);
        for x in self.excerpts.iter_mut().filter(|x| x.path == path) {
            (x.start, x.end) = map_through((x.start, x.end), &changes);
            x.end = x.end.min(all.len());
            x.start = x.start.min(x.end);
            x.lines = all[x.start..x.end].iter().map(|l| l.to_string()).collect();
            x.hits = x
                .hits
                .iter()
                .filter_map(|&h| {
                    let (s, e) = map_through((h, h + 1), &changes);
                    (e > s).then_some(s)
                })
                .collect();
            if let Some(h) = &mut x.hunk {
                h.added = map_through(h.added, &changes);
            }
        }
        self.excerpts.retain(|x| x.start < x.end);
        if let Some(s) = self.source_mut(path) {
            s.text = new.into();
        }
        self.tidy();
    }

    /// Grows an excerpt by `n` lines upward or downward within its file.
    pub fn expand(&mut self, i: usize, up: bool, n: usize) -> bool {
        let Some(x) = self.excerpts.get(i) else { return false };
        let Some(text) = self.source(&x.path).map(|s| s.text.clone()) else { return false };
        let all = lines(&text);
        let same = || self.excerpts.iter().enumerate().filter(|(k, y)| *k != i && y.path == self.excerpts[i].path);
        let floor = same().map(|(_, y)| y.end).filter(|&e| e <= self.excerpts[i].start).max().unwrap_or(0);
        let ceiling = same().map(|(_, y)| y.start).filter(|&s| s >= self.excerpts[i].end).min().unwrap_or(all.len());
        let x = &mut self.excerpts[i];
        let (s, e) =
            if up { (x.start.saturating_sub(n).max(floor), x.end) } else { (x.start, (x.end + n).min(ceiling)) };
        if (s, e) == (x.start, x.end) {
            return false;
        }
        (x.start, x.end) = (s, e);
        x.lines = all[s..e].iter().map(|l| l.to_string()).collect();
        self.tidy();
        true
    }

    /// Joins excerpts of a file that now touch or overlap, keeping the
    /// first's place in the list.
    pub fn tidy(&mut self) {
        let mut i = 0;
        while i < self.excerpts.len() {
            let (path, s, e) = (self.excerpts[i].path.clone(), self.excerpts[i].start, self.excerpts[i].end);
            let other = (0..self.excerpts.len()).find(|&k| {
                let y = &self.excerpts[k];
                k != i
                    && y.path == path
                    && y.hunk.is_none()
                    && self.excerpts[i].hunk.is_none()
                    && y.start <= e
                    && s <= y.end
            });
            let Some(k) = other else {
                i += 1;
                continue;
            };
            let y = self.excerpts.remove(k);
            let i2 = if k < i { i - 1 } else { i };
            let Some(text) = self.source(&path).map(|s| s.text.clone()) else { return };
            let all = lines(&text);
            let x = &mut self.excerpts[i2];
            (x.start, x.end) = (x.start.min(y.start), x.end.max(y.end).min(all.len()));
            x.lines = all[x.start..x.end].iter().map(|l| l.to_string()).collect();
            x.hits.extend(y.hits);
            x.hits.sort_unstable();
            x.hits.dedup();
            x.notes.extend(y.notes);
            i = 0;
        }
    }

    /// The excerpt and file line of a line of the multibuffer's body, given
    /// the cell it is in and how far into the cell's lines.
    pub fn locate(&self, cell: usize, offset: usize) -> Option<(&Path, usize)> {
        let x = self.excerpts.get(cell)?;
        Some((&x.path, x.start + offset.min(x.end - x.start - 1)))
    }

    /// The excerpt showing a file's line, and how far into it.
    pub fn find(&self, path: &Path, line: usize) -> Option<(usize, usize)> {
        let i = self.excerpts.iter().position(|x| x.path == path && (x.start..x.end).contains(&line))?;
        Some((i, line - self.excerpts[i].start))
    }

    /// Notes a step of edits for Undo; one that only grew the last step
    /// of typing is not a new one.
    pub fn record(&mut self, step: Step) {
        if step.is_empty() || self.undo.last() == Some(&step) {
            return;
        }
        self.undo.push(step);
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multi(text: &str, hits: &[usize]) -> Multi {
        let mut m = Multi::new(Kind::Search, "t", Path::new("/p"));
        m.sources.push(Source {
            path: "/p/a.rs".into(),
            text: text.into(),
            version: 0,
            hidden: None,
            touched: false,
            disk: 0,
        });
        m.add_file(Path::new("/p/a.rs"), text, hits, 1);
        m
    }

    fn numbered(n: usize) -> String {
        (0..n).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn ranges_take_context_and_merge_where_they_touch() {
        assert_eq!(ranges(&[5], 2, 100), vec![(3, 8)]);
        assert_eq!(ranges(&[0, 99], 2, 100), vec![(0, 3), (97, 100)]);
        assert_eq!(ranges(&[10, 14], 2, 100), vec![(8, 17)]);
        assert_eq!(ranges(&[10, 15], 2, 100), vec![(8, 18)]);
        assert_eq!(ranges(&[10, 16], 2, 100), vec![(8, 13), (14, 19)]);
        assert_eq!(ranges(&[3, 3, 200], 1, 10), vec![(2, 5)]);
    }

    #[test]
    fn hunk_ranges_never_overlap_and_keep_each_change() {
        assert_eq!(hunk_ranges(&[(5, 6)], 2, 100), vec![(3, 8)]);
        assert_eq!(hunk_ranges(&[(5, 6), (8, 9)], 2, 100), vec![(3, 7), (7, 11)]);
        // A removal shows the line after it; at the end, the last line.
        assert_eq!(hunk_ranges(&[(4, 4)], 1, 10), vec![(3, 6)]);
        assert_eq!(hunk_ranges(&[(10, 10)], 1, 10), vec![(8, 10)]);
        for r in hunk_ranges(&[(0, 1), (2, 2), (3, 5), (6, 6)], 3, 7).windows(2) {
            assert!(r[0].1 <= r[1].0 && r[0].0 < r[0].1, "{r:?}");
        }
    }

    #[test]
    fn line_changes_match_lines_between_separate_edits() {
        assert_eq!(line_changes("a\nb\nc", "a\nb\nc"), vec![]);
        assert_eq!(line_changes("a\nb\nc\nd\ne", "x\na\nb\nd\ne"), vec![(0, 0, 1), (2, 1, 0)]);
        assert_eq!(line_changes("a\nb\nc", "a\nB\nc"), vec![(1, 1, 1)]);
        assert_eq!(map_through((3, 5), &[(0, 0, 1), (2, 1, 0)]), (3, 5));
        assert_eq!(map_through((1, 2), &[(0, 0, 1), (2, 1, 0)]), (2, 3));
    }

    #[test]
    fn line_diff_finds_the_changed_lines() {
        assert_eq!(line_diff("a\nb\nc", "a\nB\nc"), (1, 2, 2));
        assert_eq!(line_diff("a\nb\nc", "a\nx\ny\nb\nc"), (1, 1, 3));
        assert_eq!(line_diff("a\nb\nc", "a\nc"), (1, 2, 1));
        assert_eq!(line_diff("a", "a"), (1, 1, 1));
    }

    #[test]
    fn ranges_move_grow_and_collapse_with_changes() {
        // Lines added above move a range down; below leave it.
        assert_eq!(map_range((10, 15), (5, 5, 7)), (12, 17));
        assert_eq!(map_range((10, 15), (15, 15, 17)), (10, 15));
        // Lines added at its first line push it down whole.
        assert_eq!(map_range((10, 15), (10, 10, 12)), (12, 17));
        // A change inside grows or shrinks it.
        assert_eq!(map_range((10, 15), (12, 13, 15)), (10, 17));
        assert_eq!(map_range((10, 15), (10, 11, 11)), (10, 15));
        // Taking its lines away collapses it.
        assert_eq!(map_range((10, 15), (10, 15, 10)), (10, 10));
        assert_eq!(map_range((10, 15), (8, 20, 9)), (9, 9));
        assert_eq!(map_line(12, (10, 15, 10)), None);
        assert_eq!(map_line(20, (10, 15, 10)), Some(15));
    }

    #[test]
    fn source_edits_replace_only_what_changed() {
        let text = "a\nbcd\ne";
        let (a, b, s) = source_edit(text, 1, 2, &["bXd".into()]);
        assert_eq!((a, b, s.as_str()), (3, 4, "X"));
        let (a, b, s) = source_edit(text, 1, 2, &["bcd".into(), "new".into()]);
        let mut t = text.to_string();
        t.replace_range(a..b, &s);
        assert_eq!(t, "a\nbcd\nnew\ne");
        let (a, b, s) = source_edit(text, 2, 3, &["e".into(), "f".into()]);
        let mut t = text.to_string();
        t.replace_range(a..b, &s);
        assert_eq!(t, "a\nbcd\ne\nf");
        let (a, b, s) = source_edit("é\nx", 0, 1, &["è".into()]);
        assert!("é\nx".is_char_boundary(a) && "é\nx".is_char_boundary(b) && s == "è");
    }

    #[test]
    fn excerpts_render_under_headers_and_take_edits_back_to_the_file() {
        let text = numbered(20);
        let mut m = multi(&text, &[2, 12]);
        assert_eq!(m.excerpts.iter().map(|x| (x.start, x.end)).collect::<Vec<_>>(), vec![(1, 4), (11, 14)]);
        let shown = m.render();
        assert_eq!(shown.lines().filter(|l| cells::parse(l).is_some()).count(), 2);
        assert!(shown.contains("l1\nl2\nl3\n") && shown.ends_with("l11\nl12\nl13"));
        assert_eq!(m.label(1), "a.rs:13");
        // Two new lines in the first excerpt move the second down.
        let edit = m.take(0, vec!["l1".into(), "l2".into(), "x".into(), "y".into(), "l3".into()], &text).unwrap();
        let mut t = text.clone();
        t.replace_range(edit.0..edit.1, &edit.2);
        assert!(t.starts_with("l0\nl1\nl2\nx\ny\nl3\nl4"));
        assert_eq!((m.excerpts[0].start, m.excerpts[0].end), (1, 6));
        assert_eq!((m.excerpts[1].start, m.excerpts[1].end, m.excerpts[1].hits[0]), (13, 16, 14));
        assert!(m.take(0, m.excerpts[0].lines.clone(), &t).is_none());
    }

    #[test]
    fn edits_elsewhere_move_excerpts_and_a_deleted_range_goes() {
        let text = numbered(20);
        let mut m = multi(&text, &[2, 12]);
        let mut lines: Vec<String> = lines(&text).iter().map(|s| s.to_string()).collect();
        lines.insert(0, "top".into());
        lines.drain(12..15);
        let new = lines.join("\n");
        m.follow(Path::new("/p/a.rs"), &new);
        assert_eq!(m.excerpts.len(), 1);
        assert_eq!((m.excerpts[0].start, m.excerpts[0].end), (2, 5));
        assert_eq!(m.excerpts[0].lines, vec!["l1", "l2", "l3"]);
        assert_eq!(m.source(Path::new("/p/a.rs")).unwrap().text, new);
    }

    #[test]
    fn expanding_grows_within_the_file_and_merges_neighbours() {
        let text = numbered(20);
        let mut m = multi(&text, &[2, 9]);
        assert_eq!(m.excerpts.len(), 2);
        assert!(m.expand(0, true, 5));
        assert_eq!((m.excerpts[0].start, m.excerpts[0].end), (0, 4));
        assert!(!m.expand(0, true, 5));
        assert!(m.expand(0, false, 5));
        assert_eq!(m.excerpts.len(), 1);
        assert_eq!((m.excerpts[0].start, m.excerpts[0].end), (0, 11));
        assert_eq!(m.excerpts[0].hits, vec![2, 9]);
        assert_eq!(m.find(Path::new("/p/a.rs"), 9), Some((0, 9)));
        assert_eq!(m.locate(0, 3), Some((Path::new("/p/a.rs"), 3)));
    }

    #[test]
    fn undo_steps_skip_repeats_and_new_ones_clear_redo() {
        let mut m = multi("a", &[0]);
        m.record(vec![("/p/a.rs".into(), 1)]);
        m.record(vec![("/p/a.rs".into(), 1)]);
        assert_eq!(m.undo.len(), 1);
        m.redo.push(vec![("/p/a.rs".into(), 2)]);
        m.record(vec![("/p/a.rs".into(), 2)]);
        assert!(m.redo.is_empty() && m.undo.len() == 2);
    }
}
