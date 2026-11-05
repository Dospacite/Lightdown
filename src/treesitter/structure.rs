//! Outline and fold captures, shared by the editor and the disk symbol index.
use super::*;
use crate::outline::Item;

pub(super) fn extract(
    l: &Loaded,
    tree: &Tree,
    text: &str,
    deadline: Instant,
) -> Result<(Vec<(usize, usize)>, Vec<Item>), String> {
    let mut declarations = Vec::new();
    let mut folds = Vec::new();
    for (query, outline) in [(&l.outline, true), (&l.folds, false)] {
        let mut cursor = QueryCursor::new();
        cursor.set_match_limit(4096);
        #[allow(deprecated)]
        cursor.set_timeout_micros(deadline.saturating_duration_since(Instant::now()).as_micros().max(1) as u64);
        let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
        while let Some(m) = matches.next() {
            if Instant::now() >= deadline {
                return Err("structure queries exceeded budget".into());
            }
            if outline {
                let Some(item) =
                    m.captures.iter().find(|c| query.capture_names()[c.index as usize] == "item").map(|c| c.node)
                else {
                    continue;
                };
                let mut names: Vec<_> = m
                    .captures
                    .iter()
                    .filter(|c| matches!(query.capture_names()[c.index as usize], "name" | "context"))
                    .map(|c| c.node)
                    .collect();
                names.sort_by_key(|n| n.start_byte());
                let mut title = names
                    .iter()
                    .filter_map(|n| text.get(n.byte_range()))
                    .flat_map(str::split_whitespace)
                    .collect::<Vec<_>>()
                    .join(" ");
                if title.is_empty() || title.len() > 200 {
                    continue;
                }
                if query
                    .property_settings(m.pattern_index)
                    .iter()
                    .any(|p| p.key.as_ref() == "kind" && p.value.as_deref() == Some("function"))
                {
                    title.push_str("()");
                }
                let heading = match item.kind() {
                    "atx_heading" => {
                        Some(text[item.start_byte()..].bytes().take_while(|b| *b == b'#').count().saturating_sub(1))
                    }
                    "setext_heading" => Some(
                        if text[item.byte_range()].lines().last().is_some_and(|l| l.trim_start().starts_with('-')) {
                            1
                        } else {
                            0
                        },
                    ),
                    _ => None,
                };
                declarations.push((item.start_byte(), item.end_byte(), heading, title));
            } else {
                for c in m.captures.iter().filter(|c| query.capture_names()[c.index as usize] == "fold") {
                    let n = c.node;
                    let (a, b) = (n.start_position().row, n.end_position().row);
                    let b = b.saturating_sub(usize::from(n.end_position().column == 0 && b > a));
                    if b > a {
                        folds.push((a, b));
                    }
                }
            }
            if declarations.len() > 10_000 || folds.len() > 100_000 {
                return Err("too many structure captures".into());
            }
        }
        drop(matches);
        if cursor.did_exceed_match_limit() {
            return Err("structure query match limit".into());
        }
    }
    declarations.sort_by_key(|(a, b, _, _)| (*a, std::cmp::Reverse(*b)));
    declarations.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    let mut open = Vec::new();
    let mut outline = Vec::new();
    for (a, b, heading, title) in declarations {
        while open.last().is_some_and(|end| *end <= a) {
            open.pop();
        }
        outline.push(Item { at: a, depth: heading.unwrap_or(open.len()), title });
        open.push(b);
    }
    folds.sort_unstable_by_key(|&(a, b)| (a, std::cmp::Reverse(b)));
    folds.dedup_by_key(|f| f.0);
    Ok((folds, outline))
}
