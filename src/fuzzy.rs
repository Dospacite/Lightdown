//! Fuzzy matching for lists that narrow as they are typed in: the query's
//! characters must appear in order, and a match scores higher where they
//! start words, follow one another and come early.

/// Scores how well `query` matches `text`, ignoring case. Returns the
/// score and the byte offsets in `text` of the characters matched, or
/// `None` if not every character of the query is there in order.
pub fn score(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let t: Vec<(usize, char)> = text.char_indices().collect();
    let lower: Vec<char> = t.iter().map(|&(_, c)| c.to_lowercase().next().unwrap_or(c)).collect();
    // A quick check that every character is there at all, in order.
    let mut at = 0;
    for &qc in &q {
        at = lower[at..].iter().position(|&c| c == qc)? + at + 1;
    }
    let n = t.len();
    let starts = |i: usize| {
        if i == 0 {
            return true;
        }
        let (prev, cur) = (t[i - 1].1, t[i].1);
        matches!(prev, '/' | '\\' | '_' | '-' | '.' | ' ' | ':' | '(' | '[')
            || (prev.is_lowercase() && cur.is_uppercase())
            || (!prev.is_ascii_digit() && cur.is_ascii_digit())
    };
    // Best score for matching q[..=j] with q[j] at text position i, by
    // dynamic programming over positions; `from` remembers the choice.
    const NONE: i32 = i32::MIN / 2;
    let mut best = vec![vec![NONE; n]; q.len()];
    let mut from = vec![vec![usize::MAX; n]; q.len()];
    for (j, &qc) in q.iter().enumerate() {
        let mut run_best = NONE;
        let mut run_at = usize::MAX;
        for i in 0..n {
            // The best end of the previous character strictly before i.
            if j > 0 && i > 0 && best[j - 1][i - 1] > run_best {
                run_best = best[j - 1][i - 1];
                run_at = i - 1;
            }
            if lower[i] != qc {
                continue;
            }
            let mut s = 16;
            if starts(i) {
                s += 24;
            }
            if t[i].1 == qc && qc.is_uppercase() {
                s += 2;
            }
            if j == 0 {
                // Earlier is better, a little.
                s -= (i as i32).min(20);
                best[j][i] = s;
            } else {
                let adjacent = if i > 0 { best[j - 1][i - 1] } else { NONE };
                let follow = adjacent + s + 28;
                let gap = if run_best > NONE { run_best + s - ((i - run_at) as i32 - 1).min(12) * 2 } else { NONE };
                if adjacent > NONE && follow >= gap {
                    best[j][i] = follow;
                    from[j][i] = i - 1;
                } else if run_best > NONE {
                    best[j][i] = gap;
                    from[j][i] = run_at;
                }
            }
        }
    }
    let last = q.len() - 1;
    let (mut i, total) = (0..n).map(|i| (i, best[last][i])).max_by_key(|&(_, s)| s)?;
    if total <= NONE {
        return None;
    }
    let mut hits = vec![0; q.len()];
    for j in (0..q.len()).rev() {
        hits[j] = t[i].0;
        if j > 0 {
            i = from[j][i];
        }
    }
    // Shorter texts win ties.
    Some((total - (n as i32 / 8), hits))
}

/// Scores a path, preferring matches in its last part: the file's name.
/// Returns the score and the byte offsets matched in the whole path.
pub fn score_path(query: &str, path: &str) -> Option<(i32, Vec<usize>)> {
    let name_at = path.rfind('/').map_or(0, |i| i + 1);
    let whole = score(query, path);
    match score(query, &path[name_at..]) {
        Some((s, hits)) => {
            let named = (s + 60, hits.into_iter().map(|h| h + name_at).collect());
            Some(match whole {
                Some(w) if w.0 > named.0 => w,
                _ => named,
            })
        }
        None => whole,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_must_come_in_order() {
        assert!(score("abc", "a_b_c").is_some());
        assert!(score("acb", "abc").is_none());
        assert_eq!(score("", "anything").unwrap().0, 0);
        assert_eq!(score("NM", "notes.md").unwrap().1, vec![0, 6]);
    }

    #[test]
    fn word_starts_and_runs_score_higher() {
        let a = score("fr", "file rename").unwrap().0;
        let b = score("fr", "buffer").unwrap().0;
        assert!(a > b, "{a} {b}");
        let run = score("save", "save as").unwrap().0;
        let spread = score("save", "show a view element").unwrap().0;
        assert!(run > spread);
    }

    #[test]
    fn paths_prefer_the_file_name() {
        let (s1, hits) = score_path("main", "src/main.rs").unwrap();
        assert_eq!(hits, vec![4, 5, 6, 7]);
        let (s2, _) = score_path("main", "main_tools/lib.rs").unwrap();
        assert!(s1 > s2, "{s1} {s2}");
        assert!(score_path("srcmain", "src/main.rs").is_some());
    }
}
