//! The text buffer: content, line index, selection and undo history.

use swash::text::{ClusterBreak, Codepoint};

#[derive(Clone, Copy, PartialEq)]
pub enum EditKind {
    Type,
    Delete,
    Other,
}

/// How lines end in the file. The text itself always uses `\n`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Eol {
    #[default]
    Lf,
    Crlf,
    Cr,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
            Eol::Cr => "CR",
        }
    }
    /// The ending a file's bytes use: CRLF where any line has it, CR where
    /// lines end in a lone CR and none in LF.
    pub fn detect(text: &str) -> Eol {
        if text.contains("\r\n") {
            Eol::Crlf
        } else if text.contains('\r') && !text.contains('\n') {
            Eol::Cr
        } else {
            Eol::Lf
        }
    }
    /// The text as written to the file.
    pub fn apply(self, text: &str) -> String {
        match self {
            Eol::Lf => text.to_string(),
            Eol::Crlf => text.replace('\n', "\r\n"),
            Eol::Cr => text.replace('\n', "\r"),
        }
    }
}

struct Edit {
    pos: usize,
    del: String,
    ins: String,
    before: (usize, usize),
    /// A change of line endings, from and to, with no text changed.
    eol: Option<(Eol, Eol)>,
}

/// Which lines an edit touched, so cached layouts can be shifted.
pub struct Change {
    pub line: usize,
    pub removed: usize,
    pub added: usize,
    /// Where the edit began, and how many bytes it took away.
    pub pos: usize,
    pub del: usize,
    /// The text taken away ended with a line break, leaving the line after
    /// it whole.
    pub ends_line: bool,
}

impl Change {
    fn new(buf: &Buffer, pos: usize, del: &str, ins: &str) -> Change {
        Change {
            line: buf.line_of(pos),
            removed: newlines(del),
            added: newlines(ins),
            pos,
            del: del.len(),
            ends_line: del.ends_with('\n'),
        }
    }
}

/// The undo history at a point: see `Buffer::checkpoint`.
pub struct Checkpoint {
    undo: usize,
    text: String,
    before: (usize, usize),
    eol: Eol,
}

pub struct Buffer {
    pub text: String,
    /// Byte offset at which each line starts. Never empty.
    pub starts: Vec<usize>,
    pub cur: usize,
    pub anchor: usize,
    pub modified: bool,
    /// How lines end when the text is saved.
    pub eol: Eol,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    last: EditKind,
    /// Changes with every edit, and differs between buffers, so what was
    /// worked out from the text can tell it is out of date.
    pub version: u64,
}

static VERSIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn eol_named(name: &str) -> Option<Eol> {
    [Eol::Lf, Eol::Crlf, Eol::Cr].into_iter().find(|e| e.label() == name)
}

fn newlines(s: &str) -> usize {
    s.bytes().filter(|&b| b == b'\n').count()
}

impl Buffer {
    pub fn new(text: String) -> Buffer {
        let mut b = Buffer {
            text,
            starts: Vec::new(),
            cur: 0,
            anchor: 0,
            modified: false,
            eol: Eol::Lf,
            undo: Vec::new(),
            redo: Vec::new(),
            last: EditKind::Other,
            version: 0,
        };
        b.reindex();
        b
    }

    fn reindex(&mut self) {
        self.version = VERSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.starts.clear();
        self.starts.push(0);
        self.starts.extend(self.text.match_indices('\n').map(|(i, _)| i + 1));
    }

    /// Recovery keeps both sides of undo, including an undone edit awaiting redo.
    pub(crate) fn recovery(&self) -> serde_json::Value {
        let edits = |list: &[Edit]| list.iter().map(|e| serde_json::json!({
            "pos": e.pos, "del": e.del, "ins": e.ins, "before": [e.before.0, e.before.1],
            "eol": e.eol.map(|(a, b)| [a.label(), b.label()])
        })).collect::<Vec<_>>();
        serde_json::json!({"text": self.text, "cursor": self.cur, "anchor": self.anchor, "eol": self.eol.label(),
            "modified": self.modified, "undo": edits(&self.undo), "redo": edits(&self.redo)})
    }

    pub(crate) fn recover(v: &serde_json::Value) -> Option<Self> {
        let mut b = Self::new(v["text"].as_str()?.into());
        let parse = |key: &str| -> Option<Vec<Edit>> {
            v[key].as_array()?.iter().map(|e| Some(Edit {
                pos: usize::try_from(e["pos"].as_u64()?).ok()?,
                del: e["del"].as_str()?.into(), ins: e["ins"].as_str()?.into(),
                before: (usize::try_from(e["before"][0].as_u64()?).ok()?,
                    usize::try_from(e["before"][1].as_u64()?).ok()?),
                eol: e["eol"].as_array().and_then(|a| Some((eol_named(a.first()?.as_str()?)?, eol_named(a.get(1)?.as_str()?)?))),
            })).collect()
        };
        b.undo = parse("undo")?;
        b.redo = parse("redo")?;
        // Treat damaged snapshots as data, never as trusted byte offsets.
        for (edits, undo) in [(&b.undo, true), (&b.redo, false)] {
            let mut text = b.text.clone();
            for e in edits.iter().rev() {
                let (del, ins) = if undo { (&e.ins, &e.del) } else { (&e.del, &e.ins) };
                let end = e.pos.checked_add(del.len())?;
                if text.get(e.pos..end)? != del { return None; }
                text.replace_range(e.pos..end, ins);
                if undo && (!text.is_char_boundary(e.before.0) || !text.is_char_boundary(e.before.1)) {
                    return None;
                }
            }
        }
        b.cur = floor(&b.text, v["cursor"].as_u64()? as usize);
        b.anchor = floor(&b.text, v["anchor"].as_u64()? as usize);
        b.modified = v["modified"].as_bool()?;
        b.eol = v["eol"].as_str().and_then(eol_named).unwrap_or_default();
        Some(b)
    }

    pub fn lines(&self) -> usize {
        self.starts.len()
    }

    pub fn line_of(&self, off: usize) -> usize {
        self.starts.partition_point(|&s| s <= off) - 1
    }

    /// Start and end of a line, excluding its newline.
    pub fn line(&self, l: usize) -> (usize, usize) {
        let end = if l + 1 < self.starts.len() { self.starts[l + 1] - 1 } else { self.text.len() };
        (self.starts[l], end)
    }

    pub fn sel(&self) -> (usize, usize) {
        (self.cur.min(self.anchor), self.cur.max(self.anchor))
    }

    pub fn has_sel(&self) -> bool {
        self.cur != self.anchor
    }

    /// Ends the current run of typing, so the next edit is its own undo step.
    pub fn commit(&mut self) {
        self.last = EditKind::Other;
    }

    /// Marks the undo history, so that the edits made after it can become
    /// one undo step with `finish`.
    pub fn checkpoint(&mut self) -> Checkpoint {
        self.commit();
        Checkpoint { undo: self.undo.len(), text: self.text.clone(), before: (self.cur, self.anchor), eol: self.eol }
    }

    /// Turns the edits made since a checkpoint into one undo step. Does
    /// nothing if some of them were undone in between.
    pub fn finish(&mut self, mark: Checkpoint) {
        let Checkpoint { undo, text: old, before, eol } = mark;
        if self.undo.len() < undo {
            return;
        }
        self.undo.truncate(undo);
        // A change of line endings in between stays part of the one step.
        let eol = (eol != self.eol).then_some((eol, self.eol));
        if old != self.text || eol.is_some() {
            let new = &self.text;
            let head = floor(&old, old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count());
            let mut tail = old[head..].bytes().rev().zip(new[head..].bytes().rev()).take_while(|(a, b)| a == b).count();
            while !old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail) {
                tail -= 1;
            }
            let (del, ins) = (old[head..old.len() - tail].to_string(), new[head..new.len() - tail].to_string());
            self.undo.push(Edit { pos: head, del, ins, before, eol });
        }
        self.commit();
    }

    pub fn replace(&mut self, start: usize, end: usize, ins: &str, kind: EditKind) -> Change {
        let change = Change::new(self, start, &self.text[start..end], ins);
        let merged = match self.undo.last_mut() {
            Some(e) if kind == self.last && kind == EditKind::Type && start == end && e.pos + e.ins.len() == start => {
                e.ins.push_str(ins);
                true
            }
            Some(e) if kind == self.last && kind == EditKind::Delete && ins.is_empty() && e.ins.is_empty() => {
                if end == e.pos {
                    e.del.insert_str(0, &self.text[start..end]);
                    e.pos = start;
                    true
                } else if start == e.pos {
                    e.del.push_str(&self.text[start..end]);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !merged {
            self.undo.push(Edit {
                pos: start,
                del: self.text[start..end].to_string(),
                ins: ins.to_string(),
                before: (self.cur, self.anchor),
                eol: None,
            });
        }
        self.last = if ins.contains('\n') { EditKind::Other } else { kind };
        self.redo.clear();
        self.text.replace_range(start..end, ins);
        self.cur = start + ins.len();
        self.anchor = self.cur;
        self.modified = true;
        self.reindex();
        change
    }

    /// Changes how lines end on saving, as one undo step of its own.
    pub fn set_eol(&mut self, eol: Eol) -> Option<Change> {
        if eol == self.eol {
            return None;
        }
        let pos = self.cur;
        self.undo.push(Edit { pos, del: String::new(), ins: String::new(), before: (self.cur, self.anchor), eol: Some((self.eol, eol)) });
        self.redo.clear();
        self.last = EditKind::Other;
        self.eol = eol;
        self.modified = true;
        self.version = VERSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(Change::new(self, pos, "", ""))
    }

    /// Makes the text equal to `text` by rewriting it from the first
    /// difference on, outside the undo history. For documents the program
    /// writes rather than the user, such as a chat transcript.
    pub fn sync(&mut self, text: &str) -> Option<Change> {
        let same = self.text.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
        let same = floor(text, floor(&self.text, same));
        if same == self.text.len() && same == text.len() {
            return None;
        }
        let change = Change::new(self, same, &self.text[same..], &text[same..]);
        self.text.truncate(same);
        self.text.push_str(&text[same..]);
        (self.cur, self.anchor) = (floor(&self.text, self.cur), floor(&self.text, self.anchor));
        self.reindex();
        Some(change)
    }

    /// Replaces a range without a word to the undo history, the cursor or
    /// the modified flag. For text that is shown but not yet the user's,
    /// which must be taken out again before anything else is edited.
    pub fn raw(&mut self, start: usize, end: usize, ins: &str) -> Change {
        let change = Change::new(self, start, &self.text[start..end], ins);
        self.text.replace_range(start..end, ins);
        self.reindex();
        change
    }

    /// The latest replacement, for local next-edit suggestions. No history copy.
    pub fn last_replacement(&self) -> Option<(&str, &str)> {
        let e = self.undo.last()?;
        (!e.del.is_empty() && !e.ins.is_empty() && e.del != e.ins && e.del.len() <= 200 && e.ins.len() <= 200).then_some((e.del.as_str(), e.ins.as_str()))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Makes the edits since the history was `mark` long, which took the
    /// text from `before` to what it is, one step to undo, returning the
    /// selection to `sel`. Does nothing if undo has gone back past `mark`.
    pub fn squash(&mut self, mark: usize, before: &str, sel: (usize, usize)) {
        if self.undo.len() <= mark + 1 || !self.redo.is_empty() {
            return;
        }
        let a = before.bytes().zip(self.text.bytes()).take_while(|(x, y)| x == y).count();
        let a = floor(before, floor(&self.text, a));
        let most = before.len().min(self.text.len()) - a;
        let mut z = before.bytes().rev().zip(self.text.bytes().rev()).take(most).take_while(|(x, y)| x == y).count();
        while !before.is_char_boundary(before.len() - z) || !self.text.is_char_boundary(self.text.len() - z) {
            z -= 1;
        }
        // A change of line endings among the squashed edits stays in the step.
        let first = self.undo[mark..].iter().find_map(|e| e.eol.map(|(from, _)| from));
        let edit = Edit {
            pos: a,
            del: before[a..before.len() - z].to_string(),
            ins: self.text[a..self.text.len() - z].to_string(),
            before: sel,
            eol: first.filter(|&from| from != self.eol).map(|from| (from, self.eol)),
        };
        self.undo.truncate(mark);
        self.undo.push(edit);
        self.last = EditKind::Other;
    }

    pub fn undo(&mut self) -> Option<Change> {
        let e = self.undo.pop()?;
        if let Some((from, _)) = e.eol {
            self.eol = from;
        }
        let change = Change::new(self, e.pos, &e.ins, &e.del);
        self.text.replace_range(e.pos..e.pos + e.ins.len(), &e.del);
        (self.cur, self.anchor) = e.before;
        self.redo.push(e);
        self.last = EditKind::Other;
        self.modified = true;
        self.reindex();
        Some(change)
    }

    pub fn redo(&mut self) -> Option<Change> {
        let e = self.redo.pop()?;
        if let Some((_, to)) = e.eol {
            self.eol = to;
        }
        let change = Change::new(self, e.pos, &e.del, &e.ins);
        self.text.replace_range(e.pos..e.pos + e.del.len(), &e.ins);
        self.cur = e.pos + e.ins.len();
        self.anchor = self.cur;
        self.undo.push(e);
        self.last = EditKind::Other;
        self.modified = true;
        self.reindex();
        Some(change)
    }
}

fn extends(c: char) -> bool {
    matches!(c.cluster_break(), ClusterBreak::EX | ClusterBreak::ZWJ | ClusterBreak::SM)
}

fn regional(c: char) -> bool {
    matches!(c.cluster_break(), ClusterBreak::RI)
}

/// Largest char boundary at or below `off`.
pub fn ceil(text: &str, mut off: usize) -> usize {
    off = off.min(text.len());
    while !text.is_char_boundary(off) {
        off += 1;
    }
    off
}

pub fn floor(text: &str, mut off: usize) -> usize {
    off = off.min(text.len());
    while !text.is_char_boundary(off) {
        off -= 1;
    }
    off
}

/// End of the grapheme starting at `off`.
pub fn next(text: &str, off: usize) -> usize {
    let mut it = text[off..].chars();
    let Some(first) = it.next() else { return off };
    let mut end = off + first.len_utf8();
    let mut prev = first;
    let mut flag = regional(first);
    for c in it {
        let joined = extends(c) || prev == '\u{200d}' || (flag && regional(c));
        if !joined {
            break;
        }
        flag = false;
        end += c.len_utf8();
        prev = c;
    }
    end
}

/// Start of the grapheme ending at `off`.
pub fn prev(text: &str, off: usize) -> usize {
    let mut p = off;
    while p > 0 {
        let c = text[..p].chars().next_back().unwrap();
        p -= c.len_utf8();
        let before = text[..p].chars().next_back();
        if !(extends(c) || before == Some('\u{200d}')) {
            break;
        }
    }
    p
}

fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

pub fn word_right(text: &str, off: usize) -> usize {
    let mut it = text[off..].char_indices().peekable();
    while it.next_if(|&(_, c)| class(c) == 0).is_some() {}
    let Some(&(_, first)) = it.peek() else { return text.len() };
    while it.next_if(|&(_, c)| class(c) == class(first)).is_some() {}
    it.peek().map_or(text.len(), |&(i, _)| off + i)
}

pub fn word_left(text: &str, off: usize) -> usize {
    let mut it = text[..off].char_indices().rev().peekable();
    while it.next_if(|&(_, c)| class(c) == 0).is_some() {}
    let Some(&(_, first)) = it.peek() else { return 0 };
    let mut start = off;
    while let Some((i, _)) = it.next_if(|&(_, c)| class(c) == class(first)) {
        start = i;
    }
    start
}

/// The run of same-class characters around `off`, for double-click.
pub fn word_at(text: &str, off: usize) -> (usize, usize) {
    let Some(c) = text[off..].chars().next().filter(|&c| c != '\n').or_else(|| text[..off].chars().next_back()) else {
        return (off, off);
    };
    let k = class(c);
    let same = |c: &char| class(*c) == k && *c != '\n';
    let end = off + text[off..].chars().take_while(same).map(char::len_utf8).sum::<usize>();
    let start = off - text[..off].chars().rev().take_while(same).map(char::len_utf8).sum::<usize>();
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_ending_change_is_one_undo_step_and_survives_recovery() {
        assert_eq!((Eol::detect("a\r\nb"), Eol::detect("a\rb"), Eol::detect("a\nb")), (Eol::Crlf, Eol::Cr, Eol::Lf));
        assert_eq!(Eol::Crlf.apply("a\nb\n"), "a\r\nb\r\n");
        let mut b = Buffer::new("a\nb".into());
        b.replace(1, 1, "x", EditKind::Type);
        assert!(b.set_eol(Eol::Crlf).is_some());
        assert!(b.set_eol(Eol::Crlf).is_none());
        let r = Buffer::recover(&b.recovery()).unwrap();
        assert_eq!(r.eol, Eol::Crlf);
        b.undo();
        assert_eq!((b.eol, b.text.as_str()), (Eol::Lf, "ax\nb"));
        b.redo();
        assert_eq!(b.eol, Eol::Crlf);
        let mut r = r;
        r.undo();
        assert_eq!((r.eol, r.text.as_str()), (Eol::Lf, "ax\nb"));
    }

    #[test]
    fn undo_merges_typing() {
        let mut b = Buffer::new("ab\ncd".into());
        for (i, s) in ["x", "y", "z"].iter().enumerate() {
            b.replace(2 + i, 2 + i, s, EditKind::Type);
        }
        assert_eq!(b.text, "abxyz\ncd");
        b.replace(4, 5, "", EditKind::Delete);
        b.replace(3, 4, "", EditKind::Delete);
        assert_eq!(b.text, "abx\ncd");
        b.undo();
        assert_eq!((b.text.as_str(), b.cur), ("abxyz\ncd", 5));
        b.undo();
        assert_eq!(b.text, "ab\ncd");
        b.redo();
        assert_eq!((b.text.as_str(), b.lines(), b.line(1)), ("abxyz\ncd", 2, (6, 8)));
    }

    #[test]
    fn movement() {
        let t = "héllo, wörld  foo_bar e\u{301}x 👍🏽!";
        assert_eq!(word_right(t, 0), 6);
        assert_eq!(word_right(t, 6), 7);
        assert_eq!(word_left(t, 14), 8);
        assert_eq!(&t[word_at(t, 18).0..word_at(t, 18).1], "foo_bar");
        let e = t.find("e\u{301}").unwrap();
        assert_eq!(next(t, e), e + 3);
        assert_eq!(prev(t, e + 3), e);
        let thumb = t.find('👍').unwrap();
        assert_eq!(next(t, thumb), thumb + 8);
        assert_eq!(prev(t, thumb + 8), thumb);
    }
}
