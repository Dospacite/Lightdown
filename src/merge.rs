//! A three-way merge: ours, base and theirs side by side, aligned by
//! conflict, above the result being edited.
use std::ops::Range;

/// A conflict written into text with markers.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Marked {
    pub range: Range<usize>,
    pub ours: String,
    pub base: Option<String>,
    pub theirs: String,
}

fn starts(marker: &str, sign: &str) -> bool {
    marker.strip_prefix(sign).is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

/// The conflicts in a text, with or without a base section.
pub(crate) fn marked(text: &str) -> Vec<Marked> {
    let mut out = Vec::new();
    // Where the conflict starts, and where each of its parts starts.
    let mut open: Option<(usize, usize, Option<usize>, Option<usize>)> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let marker = line.trim_end_matches(['\r', '\n']);
        let next = at + line.len();
        match &mut open {
            None if starts(marker, "<<<<<<<") => open = Some((at, next, None, None)),
            Some((_, _, base @ None, None)) if starts(marker, "|||||||") => *base = Some(at),
            Some((_, _, _, theirs @ None)) if marker == "=======" => *theirs = Some(at),
            Some((start, ours, base, Some(sep))) if starts(marker, ">>>>>>>") => {
                let body = |from: usize| from + text[from..].find('\n').map_or(text.len() - from, |n| n + 1);
                out.push(Marked {
                    range: *start..next,
                    ours: text[*ours..base.unwrap_or(*sep)].into(),
                    base: base.map(|b| text[body(b)..*sep].into()),
                    theirs: text[body(*sep)..at].into(),
                });
                open = None;
            }
            _ => {}
        }
        at = next;
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Chunk {
    Same(String),
    Conflict { ours: String, base: String, theirs: String },
}

/// Splits `git merge-file --diff3 -p` output into unchanged text and
/// conflicts.
pub(crate) fn parse(merged: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut at = 0;
    for m in marked(merged) {
        if m.range.start > at { out.push(Chunk::Same(merged[at..m.range.start].into())); }
        out.push(Chunk::Conflict { ours: m.ours, base: m.base.unwrap_or_default(), theirs: m.theirs });
        at = m.range.end;
    }
    if at < merged.len() { out.push(Chunk::Same(merged[at..].into())); }
    out
}

/// One row of the three columns.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row {
    /// A line all three share.
    Same(String),
    /// Unchanged lines left out.
    Fold(usize),
    /// The heading of conflict `k`.
    Head(usize),
    /// A line of conflict `k` in each column, or none where a column has
    /// fewer lines, and whether it differs from the base.
    Line(usize, [Option<String>; 3], [bool; 3]),
}

/// Unchanged lines shown on each side of a conflict.
const CONTEXT: usize = 2;

fn lines(s: &str) -> Vec<String> {
    s.lines().map(|l| l.to_string()).collect()
}

fn layout(chunks: &[Chunk]) -> (Vec<Row>, Vec<usize>) {
    let (mut rows, mut heads) = (Vec::new(), Vec::new());
    let last = chunks.len().saturating_sub(1);
    for (i, chunk) in chunks.iter().enumerate() {
        match chunk {
            Chunk::Same(text) => {
                let all = lines(text);
                let keep_start = if i == 0 { 0 } else { CONTEXT };
                let keep_end = if i == last { 0 } else { CONTEXT };
                if all.len() <= keep_start + keep_end + 1 {
                    rows.extend(all.into_iter().map(Row::Same));
                } else {
                    rows.extend(all[..keep_start].iter().cloned().map(Row::Same));
                    rows.push(Row::Fold(all.len() - keep_start - keep_end));
                    rows.extend(all[all.len() - keep_end..].iter().cloned().map(Row::Same));
                }
            }
            Chunk::Conflict { ours, base, theirs } => {
                let k = heads.len();
                heads.push(rows.len());
                rows.push(Row::Head(k));
                let sides = [lines(ours), lines(base), lines(theirs)];
                let n = sides.iter().map(Vec::len).max().unwrap_or(0).max(1);
                for at in 0..n {
                    let cells = [0, 1, 2].map(|s| sides[s].get(at).cloned());
                    let changed = [0, 1, 2].map(|s| s != 1 && cells[s].as_ref().is_some_and(|l| !sides[1].contains(l)));
                    rows.push(Row::Line(k, cells, changed));
                }
            }
        }
    }
    (rows, heads)
}

/// What a choice puts in place of a conflict, numbered as the inline
/// conflict buttons are: ours, theirs, both, base, theirs then ours.
pub(crate) const CHOICES: [&str; 5] = ["Ours", "Theirs", "Ours, then theirs", "Base", "Theirs, then ours"];
/// The order the choices are shown in.
pub(crate) const SHOWN: [u8; 5] = [0, 3, 1, 2, 4];

#[derive(Clone, Debug, Default)]
pub(crate) struct Merge {
    pub chunks: Vec<Chunk>,
    pub rows: Vec<Row>,
    /// The row of each conflict's heading.
    pub heads: Vec<usize>,
    /// Column titles: ours, base, theirs.
    pub labels: [String; 3],
    /// The keys of each choice, for its tooltip.
    pub keys: [String; 5],
    pub scroll: f32,
    /// The pane's whole place, and the part the columns take of it.
    pub full: (f32, f32, f32, f32),
    pub panel: (f32, f32, f32, f32),
    pub pointer: (f32, f32),
    /// The conflict the result's cursor is in or before, once drawn.
    pub current: Option<usize>,
    pub followed: Option<usize>,
    /// Where each conflict is in the result, and for which version.
    found: Option<(u64, Vec<Option<Range<usize>>>)>,
}

impl Merge {
    /// The stages merged as `merge-file` writes them, and the result as it
    /// stands. When git wrote different conflicts into the result than
    /// `merge-file` finds, and as many, the result's sides are used.
    pub fn new(merged: &str, result: &str, labels: [String; 3]) -> Merge {
        let mut chunks = parse(merged);
        let mut m = Merge { labels, ..Merge::default() };
        let marks = marked(result);
        let count = chunks.iter().filter(|c| matches!(c, Chunk::Conflict { .. })).count();
        m.chunks = chunks.clone();
        if count == marks.len() && m.locate(result).iter().any(Option::is_none) {
            let mut k = 0;
            for chunk in &mut chunks {
                if let Chunk::Conflict { ours, theirs, .. } = chunk {
                    (*ours, *theirs) = (marks[k].ours.clone(), marks[k].theirs.clone());
                    k += 1;
                }
            }
        }
        (m.rows, m.heads) = layout(&chunks);
        m.chunks = chunks;
        m
    }

    pub fn conflicts(&self) -> impl Iterator<Item = (&String, &String, &String)> {
        self.chunks.iter().filter_map(|c| match c {
            Chunk::Conflict { ours, base, theirs } => Some((ours, base, theirs)),
            Chunk::Same(_) => None,
        })
    }

    /// Where each conflict stands in the result, or none once resolved.
    pub fn locate(&self, result: &str) -> Vec<Option<Range<usize>>> {
        let marks = marked(result);
        let mut used = vec![false; marks.len()];
        self.conflicts().map(|(ours, _, theirs)| {
            let i = (0..marks.len()).find(|&i| !used[i] && marks[i].ours == *ours && marks[i].theirs == *theirs)?;
            used[i] = true;
            Some(marks[i].range.clone())
        }).collect()
    }

    /// `locate`, kept for one version of the result.
    pub fn found(&mut self, result: &str, version: u64) -> &[Option<Range<usize>>] {
        if self.found.as_ref().is_none_or(|(v, _)| *v != version) {
            self.found = Some((version, self.locate(result)));
        }
        &self.found.as_ref().unwrap().1
    }

    /// The text that replaces conflict `k` for a choice.
    pub fn choice(&self, k: usize, choice: u8) -> Option<String> {
        let (ours, base, theirs) = self.conflicts().nth(k)?;
        Some(match choice {
            0 => ours.clone(),
            1 => theirs.clone(),
            3 => base.clone(),
            4 => format!("{theirs}{ours}"),
            _ => format!("{ours}{theirs}"),
        })
    }

    /// The conflict the cursor is in, or the next one after it.
    pub fn at_cursor(found: &[Option<Range<usize>>], cur: usize) -> Option<usize> {
        found.iter().position(|r| r.as_ref().is_some_and(|r| r.end > cur))
            .or_else(|| found.iter().rposition(Option::is_some))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGED: &str = "a\n<<<<<<< ours\nB1\n||||||| base\nb\n=======\nB2\nextra\n>>>>>>> theirs\nc\nd\ne\nf\ng\nh\n<<<<<<< ours\nX\n||||||| base\nx\n=======\nx\n>>>>>>> theirs\n";

    #[test]
    fn stages_split_into_unchanged_text_and_conflicts() {
        let chunks = parse(MERGED);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[1], Chunk::Conflict { ours: "B1\n".into(), base: "b\n".into(), theirs: "B2\nextra\n".into() });
        assert_eq!(chunks[2], Chunk::Same("c\nd\ne\nf\ng\nh\n".into()));
    }

    #[test]
    fn rows_align_each_conflict_and_fold_long_unchanged_runs() {
        let m = Merge::new(MERGED, "", Default::default());
        assert_eq!(m.heads, vec![1, 9]);
        assert_eq!(m.rows[2], Row::Line(0, [Some("B1".into()), Some("b".into()), Some("B2".into())], [true, false, true]));
        // The shorter sides are padded so the next conflict starts level.
        assert_eq!(m.rows[3], Row::Line(0, [None, None, Some("extra".into())], [false, false, true]));
        assert_eq!(m.rows[4..9], [Row::Same("c".into()), Row::Same("d".into()), Row::Fold(2), Row::Same("g".into()), Row::Same("h".into())]);
        // Theirs kept the base line, so only ours differs.
        assert_eq!(m.rows[10], Row::Line(1, [Some("X".into()), Some("x".into()), Some("x".into())], [true, false, false]));
    }

    #[test]
    fn conflicts_are_found_in_the_result_until_resolved() {
        let result = "a\n<<<<<<< HEAD\nB1\n=======\nB2\nextra\n>>>>>>> topic\nc\nX\n";
        let m = Merge::new(MERGED, result, Default::default());
        let found = m.locate(result);
        assert_eq!(found[0], Some(2..49));
        assert_eq!(found[1], None);
        assert_eq!(Merge::at_cursor(&found, 0), Some(0));
        assert_eq!(m.choice(0, 3).as_deref(), Some("b\n"));
        assert_eq!(m.choice(0, 4).as_deref(), Some("B2\nextra\nB1\n"));
    }

    #[test]
    fn differing_conflicts_take_the_results_sides() {
        let merged = "<<<<<<< o\nA\n||||||| b\nz\n=======\nB\n>>>>>>> t\n";
        let result = "<<<<<<< HEAD\nA\nshared\n=======\nB\nshared\n>>>>>>> topic\n";
        let m = Merge::new(merged, result, Default::default());
        assert!(m.locate(result)[0].is_some());
        assert_eq!(m.choice(0, 3).as_deref(), Some("z\n"));
    }
}
