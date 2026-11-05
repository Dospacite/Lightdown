//! Shared two-column presentation of unified text patches, including word changes.
use std::ops::Range;
#[derive(Clone, Debug, Default)]
pub(crate) struct Cell {
    pub number: Option<usize>,
    pub text: String,
    pub changed: Vec<Range<usize>>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Row { pub left: Cell, pub right: Cell, pub heading: bool }
#[derive(Clone, Debug, Default)]
pub(crate) struct View { pub rows: Vec<Row>, pub scroll: f32, pub horizontal: f32 }
fn tokens(s: &str) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut previous = None;
    for (at, c) in s.char_indices() {
        let kind = if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
        if previous == Some(kind) && kind != 2 { out.last_mut().unwrap().end = at + c.len_utf8(); }
        else { out.push(at..at + c.len_utf8()); }
        previous = Some(kind);
    }
    out
}
/// LCS over words and punctuation. Bound the table for generated/minified lines.
pub(crate) fn words(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (aa, bb) = (tokens(a), tokens(b));
    if aa.len().saturating_mul(bb.len()) > 65_536 {
        return (vec![0..a.len()], vec![0..b.len()]);
    }
    let width = bb.len() + 1;
    let mut dp = vec![0u16; (aa.len() + 1) * width];
    for i in (0..aa.len()).rev() { for j in (0..bb.len()).rev() {
        dp[i * width + j] = if a[aa[i].clone()] == b[bb[j].clone()] { 1 + dp[(i + 1) * width + j + 1] }
            else { dp[(i + 1) * width + j].max(dp[i * width + j + 1]) };
    }}
    let (mut i, mut j) = (0, 0);
    let (mut left, mut right) = (Vec::new(), Vec::new());
    while i < aa.len() || j < bb.len() {
        if i < aa.len() && j < bb.len() && a[aa[i].clone()] == b[bb[j].clone()] { i += 1; j += 1; }
        else if i < aa.len() && (j == bb.len() || dp[(i + 1) * width + j] >= dp[i * width + j + 1]) { left.push(aa[i].clone()); i += 1; }
        else { right.push(bb[j].clone()); j += 1; }
    }
    (left, right)
}
impl View {
    pub fn parse(patch: &str) -> Self {
        fn flush(rows: &mut Vec<Row>, removed: &mut Vec<Cell>, added: &mut Vec<Cell>) {
            let n = removed.len().max(added.len());
            let mut l = removed.drain(..); let mut r = added.drain(..);
            for _ in 0..n {
                let (mut left, mut right) = (l.next().unwrap_or_default(), r.next().unwrap_or_default());
                (left.changed, right.changed) = words(&left.text, &right.text);
                rows.push(Row { left, right, heading: false });
            }
        }
        let (mut rows, mut removed, mut added) = (Vec::new(), Vec::new(), Vec::new());
        let (mut old, mut new) = (0, 0);
        let mut in_hunk = false;
        for line in patch.lines() {
            if line.starts_with("@@ ") {
                flush(&mut rows, &mut removed, &mut added);
                let fields: Vec<_> = line.split_whitespace().collect();
                let start = |n: usize| fields.get(n).and_then(|s| s[1..].split(',').next()).and_then(|s| s.parse().ok()).unwrap_or(0);
                old = start(1); new = start(2); in_hunk = true;
                rows.push(Row { left: Cell { text: line.into(), ..Cell::default() }, heading: true, ..Row::default() });
            } else if in_hunk && line.starts_with('-') {
                removed.push(Cell { number: Some(old), text: line[1..].into(), changed: Vec::new() }); old += 1;
            } else if in_hunk && line.starts_with('+') {
                added.push(Cell { number: Some(new), text: line[1..].into(), changed: Vec::new() }); new += 1;
            } else if in_hunk && line.starts_with(' ') {
                flush(&mut rows, &mut removed, &mut added);
                rows.push(Row { left: Cell { number: Some(old), text: line[1..].into(), changed: Vec::new() }, right: Cell { number: Some(new), text: line[1..].into(), changed: Vec::new() }, heading: false }); old += 1; new += 1;
            } else if line.starts_with('\\') { /* no newline annotation */ }
            else {
                flush(&mut rows, &mut removed, &mut added);
                in_hunk = false;
                rows.push(Row { left: Cell { text: line.into(), ..Cell::default() }, heading: true, ..Row::default() });
            }
        }
        flush(&mut rows, &mut removed, &mut added);
        Self { rows, scroll: 0.0, horizontal: 0.0 }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_changes_keep_separated_matches_and_unicode_boundaries() {
        let (a, b) = words("hello κόσμε old value", "hi κόσμε new value");
        assert_eq!(a.iter().map(|r| &"hello κόσμε old value"[r.clone()]).collect::<Vec<_>>(), ["hello", "old"]);
        assert_eq!(b.iter().map(|r| &"hi κόσμε new value"[r.clone()]).collect::<Vec<_>>(), ["hi", "new"]);
    }
    #[test]
    fn aligned_rows_keep_line_numbers_insertions_and_multiple_files() {
        let v = View::parse("--- a/x\n+++ b/x\n@@ -2,2 +2,3 @@\n-old text\n+new text\n+added\n same\n--- a/y\n+++ b/y\n@@ -1 +1 @@\n-x\n+y\n");
        assert_eq!(v.rows[3].left.number, Some(2));
        assert_eq!(v.rows[3].right.text, "new text");
        assert_eq!(v.rows[4].left.number, None);
        assert_eq!(v.rows[4].right.number, Some(3));
        assert_eq!(v.rows[5].right.number, Some(4));
        assert_eq!(v.rows.last().unwrap().left.text, "x");
    }
}
