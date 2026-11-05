//! Resolve nested captures before the renderer combines text attributes.
use super::*;
use std::collections::BTreeSet;
pub(super) fn flatten(mut captures: Vec<(Span, usize)>, deadline: Instant) -> Result<Vec<Span>, String> {
    captures.sort_by_key(|(s, priority)| (s.start, s.end, std::cmp::Reverse(*priority)));
    captures.dedup_by_key(|(s, _)| (s.start, s.end));
    let mut events = Vec::with_capacity(captures.len() * 2);
    for (i, (s, _)) in captures.iter().enumerate() {
        if s.end > s.start {
            events.push((s.start, true, i));
            events.push((s.end, false, i));
        }
    }
    events.sort_unstable();
    let mut active: BTreeSet<(u32, usize)> = BTreeSet::new();
    let mut out: Vec<Span> = Vec::new();
    let mut previous = 0;
    for (at, start, i) in events {
        if Instant::now() >= deadline {
            return Err("highlight merging exceeded query budget".into());
        }
        if at > previous
            && let Some(&(_, index)) = active.first()
        {
            let attr = captures[index].0.attr;
            if attr != 0 {
                if let Some(last) = out.last_mut().filter(|s| s.end == previous && s.attr == attr) {
                    last.end = at;
                } else {
                    out.push(Span { start: previous, end: at, attr });
                }
            }
        }
        let s = captures[i].0;
        if start {
            active.insert((s.end - s.start, i));
        } else {
            active.remove(&(s.end - s.start, i));
        }
        previous = at;
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inner_captures_and_later_patterns_override_outer_colours() {
        let s = |start, end, attr| Span { start, end, attr };
        let spans = flatten(
            vec![
                (s(0, 20, md::SOFT), 0),
                (s(3, 15, 0), 1),
                (s(5, 8, md::NUMBER), 2),
                (s(16, 19, md::PROPERTY), 0),
                (s(16, 19, md::FUNCTION), 4),
            ],
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            spans.iter().map(|s| (s.start, s.end, s.attr)).collect::<Vec<_>>(),
            [(0, 3, md::SOFT), (5, 8, md::NUMBER), (15, 16, md::SOFT), (16, 19, md::FUNCTION), (19, 20, md::SOFT)]
        );
    }
}
