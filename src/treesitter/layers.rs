//! Query loading and nested language trees. All offsets stay in host coordinates.
use super::*;
use tree_sitter::Range;

type Languages = HashMap<&'static str, Result<Arc<Loaded>, String>>;
pub(super) struct Layer {
    grammar: &'static str,
    pub(super) parsed: Parsed,
}

pub(super) fn load_query(
    g: &Grammar,
    dir: &Path,
    language: &tree_sitter::Language,
    kind: &str,
) -> Result<Query, String> {
    let path = dir.join(format!("{kind}.scm"));
    let source = if path.is_file() {
        if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1 << 20 {
            return Err(format!("{kind} query too large"));
        }
        let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        if kind == "locals" {
            format!("{source}\n{}", super::query_sets::bundled(g.name, kind))
        } else if source.trim().is_empty() {
            super::query_sets::bundled(g.name, kind).to_string()
        } else {
            source
        }
    } else {
        super::query_sets::bundled(g.name, kind).to_string()
    };
    Query::new(language, &source).map_err(|e| format!("{kind} query: {e}"))
}

pub(super) fn install_queries(g: &Grammar, tmp: &Path) -> Result<(), String> {
    for kind in ["injections", "locals"] {
        let path = format!("queries/{kind}.scm");
        // Some multi-parser repositories keep shared queries at their root.
        let candidates = [tmp.join(g.dir).join(&path), tmp.join(&path)];
        let mut source = candidates.iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        if matches!(g.name, "typescript" | "tsx") {
            let js = grammar("javascript").unwrap();
            let file = tmp.join(format!("javascript-{kind}.scm"));
            fetch(
                &format!("https://raw.githubusercontent.com/{}/{}/queries/{kind}.scm", js.repo, js.revision),
                &file,
                1 << 20,
            )?;
            source = format!("{}\n{source}", std::fs::read_to_string(file).map_err(|e| e.to_string())?);
        }
        if kind == "injections" && source.trim().is_empty() {
            source = super::query_sets::bundled(g.name, kind).to_string();
        }
        std::fs::write(tmp.join(format!("{kind}.scm")), source).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn injections(
    l: &Loaded,
    tree: &Tree,
    text: &str,
    deadline: Instant,
) -> Result<Vec<(&'static Grammar, Vec<Range>)>, String> {
    let deadline = deadline.min(Instant::now() + Duration::from_micros(QUERY_MICROS));
    let q = &l.injections;
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(4096);
    #[allow(deprecated)]
    cursor.set_timeout_micros(deadline.saturating_duration_since(Instant::now()).as_micros().max(1) as u64);
    let mut matches = cursor.matches(q, tree.root_node(), text.as_bytes());
    let mut out: Vec<(&'static Grammar, Vec<Range>)> = Vec::new();
    while let Some(m) = matches.next() {
        if Instant::now() >= deadline {
            return Err("injections exceeded query budget".into());
        }
        let props = q.property_settings(m.pattern_index);
        let capture =
            |name: &str| m.captures.iter().find(|c| q.capture_names()[c.index as usize] == name).map(|c| c.node);
        let name = capture("injection.language")
            .and_then(|n| text.get(n.byte_range()))
            .or_else(|| props.iter().find(|p| p.key.as_ref() == "injection.language").and_then(|p| p.value.as_deref()));
        let Some(g) = name.and_then(|s| for_language(&s.trim().trim_matches(['\'', '"']).to_ascii_lowercase())) else {
            continue;
        };
        if !installed(g) {
            continue;
        }
        let include = props.iter().any(|p| p.key.as_ref() == "injection.include-children");
        let combined = props.iter().any(|p| p.key.as_ref() == "injection.combined");
        let mut ranges = Vec::new();
        for c in m.captures.iter().filter(|c| q.capture_names()[c.index as usize] == "injection.content") {
            let node = c.node;
            if include {
                ranges.push(node.range());
                continue;
            }
            let mut start = node.start_byte();
            let mut point = node.start_position();
            let mut walk = node.walk();
            for child in node.named_children(&mut walk) {
                if start < child.start_byte() {
                    ranges.push(Range {
                        start_byte: start,
                        end_byte: child.start_byte(),
                        start_point: point,
                        end_point: child.start_position(),
                    });
                }
                start = child.end_byte();
                point = child.end_position();
            }
            if start < node.end_byte() {
                ranges.push(Range {
                    start_byte: start,
                    end_byte: node.end_byte(),
                    start_point: point,
                    end_point: node.end_position(),
                });
            }
        }
        ranges.retain(|r| r.end_byte > r.start_byte);
        if ranges.is_empty() {
            continue;
        }
        if combined && let Some((_, prior)) = out.iter_mut().find(|(other, _)| other.name == g.name) {
            prior.extend(ranges);
        } else {
            out.push((g, ranges));
        }
        if out.len() > 256 {
            return Err("too many injection layers".into());
        }
    }
    drop(matches);
    if cursor.did_exceed_match_limit() {
        return Err("too many injection matches".into());
    }
    Ok(out)
}

pub(super) fn inject(
    l: &Loaded,
    text: &str,
    result: &mut Parsed,
    old: Option<(tree_sitter::InputEdit, &Parsed)>,
    loaded: &mut Languages,
    deadline: Instant,
    depth: usize,
) -> Result<(), String> {
    if depth >= 8 {
        return Ok(());
    }
    for (g, mut ranges) in injections(l, &result.tree, text, deadline)? {
        ranges.sort_by_key(|r| r.start_byte);
        ranges.dedup();
        // Never recursively inject a language into exactly its own input.
        if result.tree.included_ranges() == ranges {
            continue;
        }
        let Ok(child) =
            loaded.entry(g.name).or_insert_with(|| load(g, &directory().join(g.name)).map(Arc::new)).clone()
        else {
            continue;
        };
        let previous = old.and_then(|(edit, p)| {
            p.layers.iter().find_map(|layer| {
                if layer.grammar != g.name {
                    return None;
                }
                let mut tree = layer.parsed.tree.clone();
                tree.edit(&edit);
                (tree.included_ranges() == ranges).then_some((edit, &layer.parsed))
            })
        });
        let mut parser = Parser::new();
        parser.set_language(&child.language).map_err(|e| e.to_string())?;
        parser.set_included_ranges(&ranges).map_err(|e| e.to_string())?;
        #[allow(deprecated)]
        parser.set_timeout_micros(deadline.saturating_duration_since(Instant::now()).as_micros().max(1) as u64);
        let prior = previous.map(|(edit, p)| {
            let mut tree = p.tree.clone();
            tree.edit(&edit);
            tree
        });
        let tree = parser.parse(text, prior.as_ref()).ok_or("injected parse timed out")?;
        let mut parsed = analyse(&child, tree, text, deadline)?;
        inject(&child, text, &mut parsed, previous, loaded, deadline, depth + 1)?;
        // Split host captures so embedded tokens replace strings/comments rather than OR with them.
        for range in &ranges {
            let mut spans = Vec::new();
            for s in result.spans.drain(..) {
                let (a, b) = (range.start_byte as u32, range.end_byte as u32);
                if s.end <= a || s.start >= b {
                    spans.push(s);
                } else {
                    if s.start < a {
                        spans.push(Span { end: a, ..s });
                    }
                    if s.end > b {
                        spans.push(Span { start: b, ..s });
                    }
                }
            }
            result.spans = spans;
        }
        // Included ranges can join a token across interpolation holes. Clip it to input.
        result.spans.extend(parsed.spans.iter().flat_map(|s| {
            ranges.iter().filter_map(move |r| {
                let start = s.start.max(r.start_byte as u32);
                let end = s.end.min(r.end_byte as u32);
                (end > start).then_some(Span { start, end, attr: s.attr })
            })
        }));
        result.layers.push(Layer { grammar: g.name, parsed });
        if Instant::now() >= deadline {
            return Err("nested languages exceeded query budget".into());
        }
    }
    result.spans.sort_by_key(|s| (s.start, s.end));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Downloads HTML, JavaScript, CSS and Markdown grammars with cc"]
    fn grammars_build_nested_injections() {
        for name in ["html", "javascript", "css", "markdown"] {
            super::super::tests::ready(name);
        }
        let mut loaded = HashMap::new();
        let l = load(grammar("html").unwrap(), &directory().join("html")).unwrap();
        let old = "<script>const value = 1;</script><style>p { color: red; }</style>";
        let mut p = parse(&l, old, None).unwrap();
        inject(&l, old, &mut p, None, &mut loaded, Instant::now() + Duration::from_secs(2), 0).unwrap();
        assert_eq!(p.layers.len(), 2);
        assert!(p.spans.iter().any(|s| &old[s.start as usize..s.end as usize] == "const" && s.attr & md::KEYWORD != 0));
        let new = old.replace("1", "12");
        let edit = Delta::between(old, &new).input(old, &new);
        let mut next = parse(&l, &new, Some((edit, &p.tree))).unwrap();
        inject(&l, &new, &mut next, Some((edit, &p)), &mut loaded, Instant::now() + Duration::from_secs(2), 0).unwrap();
        assert_eq!(next.layers.len(), 2);
        assert!(next.spans.iter().any(|s| &new[s.start as usize..s.end as usize] == "12" && s.attr == md::NUMBER));
        let l = load(grammar("markdown").unwrap(), &directory().join("markdown")).unwrap();
        let text = "# Title\n```html\n<script>const a = 1;</script>\n```\n";
        let mut p = parse(&l, text, None).unwrap();
        inject(&l, text, &mut p, None, &mut loaded, Instant::now() + Duration::from_secs(2), 0).unwrap();
        assert_eq!(p.layers[0].grammar, "html");
        assert_eq!(p.layers[0].parsed.layers[0].grammar, "javascript");
    }
}

#[cfg(test)]
mod tagged_tests {
    use super::*;
    #[test]
    #[ignore = "Downloads JavaScript, TypeScript, TSX, HTML and CSS grammars with cc"]
    fn grammars_build_tagged_templates_skip_interpolations() {
        for name in ["javascript", "typescript", "tsx", "html", "css"] {
            super::super::tests::ready(name);
        }
        let mut loaded = HashMap::new();
        for name in ["javascript", "typescript", "tsx"] {
            let l = load(grammar(name).unwrap(), &directory().join(name)).unwrap();
            let text = "const view = html`<div>${value}</div>`; const style = css`p { color: red; }`;";
            let mut p = parse(&l, text, None).unwrap();
            inject(&l, text, &mut p, None, &mut loaded, Instant::now() + Duration::from_secs(1), 0).unwrap();
            assert!(p.layers.iter().any(|l| l.grammar == "html"), "{name}");
            assert!(p.layers.iter().any(|l| l.grammar == "css"), "{name}");
            let at = text.find("value").unwrap();
            assert!(
                !p.spans.iter().any(|s| s.start as usize <= at && s.end as usize > at && s.attr == md::SOFT),
                "host interpolation must not be a string: {name}"
            );
            for layer in &p.layers {
                for range in layer.parsed.tree.included_ranges() {
                    assert!(
                        !(range.start_byte <= at && range.end_byte > at),
                        "interpolation entered injected parser: {name}"
                    );
                }
            }
        }
    }
}
