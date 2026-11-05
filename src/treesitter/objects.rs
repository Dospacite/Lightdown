//! Cached query text objects, also used by selection expansion.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Object {
    kind: char,
    around: bool,
    a: usize,
    b: usize,
    outer: (usize, usize),
}
fn inside(text: &str, a: usize, b: usize, comment: bool) -> (usize, usize) {
    let s = &text[a..b];
    let pairs = if comment {
        &[("/*", "*/"), ("<!--", "-->"), ("{-", "-}")][..]
    } else {
        &[("{", "}"), ("(", ")"), ("[", "]")][..]
    };
    for &(open, close) in pairs {
        if s.starts_with(open) && s.ends_with(close) && s.len() >= open.len() + close.len() {
            return (a + open.len(), b - close.len());
        }
    }
    if comment {
        for prefix in ["///", "//", "#", "--", ";"] {
            if s.starts_with(prefix) {
                let t = s[prefix.len()..].trim_start();
                return (b - t.len(), b);
            }
        }
    }
    (a, b)
}
fn parameter_around(text: &str, a: usize, b: usize) -> (usize, usize) {
    let suffix = &text[b..];
    let trimmed = suffix.trim_start_matches([' ', '\t']);
    if let Some(rest) = trimmed.strip_prefix(',') {
        return (a, text.len() - rest.trim_start_matches([' ', '\t']).len());
    }
    let prefix = &text[..a];
    let trimmed = prefix.trim_end_matches([' ', '\t']);
    if trimmed.ends_with(',') {
        return (trimmed.len() - 1, b);
    }
    (a, b)
}
pub(super) fn extract(q: &Query, tree: &Tree, text: &str, deadline: Instant) -> Result<Vec<Object>, String> {
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(4096);
    #[allow(deprecated)]
    cursor.set_timeout_micros(deadline.saturating_duration_since(Instant::now()).as_micros().max(1) as u64);
    let mut captures = cursor.captures(q, tree.root_node(), text.as_bytes());
    let mut out = Vec::new();
    while let Some((m, i)) = captures.next() {
        if Instant::now() >= deadline {
            return Err("text objects exceeded query budget".into());
        }
        let c = m.captures[*i];
        let Some((kind, scope)) = q.capture_names()[c.index as usize].split_once('.') else { continue };
        let kind = match kind {
            "function" => 'f',
            "class" => 'c',
            "parameter" => 'a',
            "comment" => 'C',
            _ => continue,
        };
        let around = scope == "around";
        let (mut a, mut b) = (c.node.start_byte(), c.node.end_byte());
        if !around {
            (a, b) = inside(text, a, b, kind == 'C');
        } else if kind == 'a' {
            (a, b) = parameter_around(text, a, b);
        }
        let enclosing = m
            .captures
            .iter()
            .find(|other| {
                q.capture_names()[other.index as usize]
                    == match kind {
                        'f' => "function.around",
                        'c' => "class.around",
                        _ => "",
                    }
            })
            .map(|c| c.node);
        let parent = enclosing.unwrap_or_else(|| {
            if !around && matches!(kind, 'f' | 'c') { c.node.parent().unwrap_or(c.node) } else { c.node }
        });
        let outer = (parent.start_byte(), parent.end_byte());
        if a <= b {
            out.push(Object { kind, around, a, b, outer });
        }
        if out.len() > 100_000 {
            return Err("too many text objects".into());
        }
    }
    drop(captures);
    if cursor.did_exceed_match_limit() {
        return Err("text object query match limit".into());
    }
    Ok(out)
}
pub(super) fn ranges(p: &Parsed, a: usize, b: usize, out: &mut Vec<(usize, usize)>) {
    for o in &p.objects {
        if o.a <= a && o.b >= b {
            out.push((o.a, o.b));
        }
    }
    let mut node = p.tree.root_node().named_descendant_for_byte_range(a, b);
    while let Some(n) = node {
        if !n.is_error() && !n.is_missing() {
            out.push((n.start_byte(), n.end_byte()));
        }
        node = n.parent();
    }
    for layer in &p.layers {
        ranges(&layer.parsed, a, b, out);
    }
}
pub(super) fn select(p: &Parsed, at: usize, around: bool, kind: char, count: usize) -> Option<(usize, usize)> {
    fn collect(p: &Parsed, at: usize, around: bool, kind: char, out: &mut Vec<(usize, usize)>) {
        out.extend(
            p.objects
                .iter()
                .filter(|o| o.kind == kind && o.around == around && o.outer.0 <= at && at < o.outer.1)
                .map(|o| (o.a, o.b)),
        );
        for l in &p.layers {
            collect(&l.parsed, at, around, kind, out);
        }
    }
    let mut out = Vec::new();
    collect(p, at, around, kind, &mut out);
    out.sort_by_key(|&(a, b)| b - a);
    out.dedup();
    out.get(count.max(1) - 1).copied()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delimiters_and_parameter_separators() {
        assert_eq!(inside("{ a }", 0, 5, false), (1, 4));
        assert_eq!(inside("/* hi */", 0, 8, true), (2, 6));
        assert_eq!(inside("// hi", 0, 5, true), (3, 5));
        assert_eq!(parameter_around("(a, b)", 1, 2), (1, 4));
        assert_eq!(parameter_around("(a, b)", 4, 5), (2, 5));
    }
}

#[cfg(test)]
mod grammar_tests {
    use super::*;
    #[test]
    #[ignore = "Downloads Python and Rust grammars with cc"]
    fn grammars_build_text_objects_and_selection() {
        let g = super::super::tests::ready("rust");
        let l = load(g, &directory().join(g.name)).unwrap();
        let text = "// hello\nstruct A { value: u8 }\nfn outer(one: u8, two: u8) {\n fn inner() {}\n}\n";
        let p = parse(&l, text, None).unwrap();
        let at = text.find("outer").unwrap();
        assert_eq!(
            &text[select(&p, at, true, 'f', 1).unwrap().0..select(&p, at, true, 'f', 1).unwrap().1],
            &text[text.find("fn outer").unwrap()..text.len() - 1]
        );
        let (a, b) = select(&p, at, false, 'f', 1).unwrap();
        assert!(text[a..b].contains("fn inner"));
        assert!(!text[a..b].starts_with('{'));
        let at = text.find("one").unwrap();
        let (a, b) = select(&p, at, true, 'a', 1).unwrap();
        assert_eq!(&text[a..b], "one: u8, ");
        let at = text.find("two").unwrap();
        let (a, b) = select(&p, at, true, 'a', 1).unwrap();
        assert_eq!(&text[a..b], ", two: u8");
        let at = text.find("inner").unwrap();
        assert!(select(&p, at, true, 'f', 2).is_some());
        let (a, b) = select(&p, 3, false, 'C', 1).unwrap();
        assert_eq!(&text[a..b], "hello");
        let at = text.find("value").unwrap();
        let (a, b) = select(&p, at, false, 'c', 1).unwrap();
        assert_eq!(&text[a..b], " value: u8 ");
        let mut ranges_out = Vec::new();
        ranges(&p, at, at, &mut ranges_out);
        assert!(ranges_out.contains(&(a, b)));
        let g = super::super::tests::ready("python");
        let l = load(g, &directory().join(g.name)).unwrap();
        let text = "def sample(first, second):\n return first\n";
        let p = parse(&l, text, None).unwrap();
        let at = text.find("first").unwrap();
        let (a, b) = select(&p, at, false, 'a', 1).unwrap();
        assert_eq!(&text[a..b], "first");
    }
}
