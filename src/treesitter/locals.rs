//! Resolve query-defined definitions and references within lexical scopes.
use super::*;

pub(super) fn resolve(
    q: &Query,
    tree: &Tree,
    text: &str,
    deadline: Instant,
) -> Result<HashMap<(usize, usize), crate::md::Attr>, String> {
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(4096);
    #[allow(deprecated)]
    cursor.set_timeout_micros(deadline.saturating_duration_since(Instant::now()).as_micros().max(1) as u64);
    let mut matches = cursor.matches(q, tree.root_node(), text.as_bytes());
    let mut scopes = vec![(0, text.len(), true)];
    let mut definitions = Vec::new();
    let mut references = Vec::new();
    while let Some(m) = matches.next() {
        if Instant::now() >= deadline {
            return Err("locals exceeded query budget".into());
        }
        let props = q.property_settings(m.pattern_index);
        let parameter = props.iter().any(|p| p.key.as_ref() == "local.role" && p.value.as_deref() == Some("parameter"));
        for c in m.captures {
            let n = c.node;
            let range = (n.start_byte(), n.end_byte());
            match q.capture_names()[c.index as usize] {
                "local.scope" => scopes.push((
                    range.0,
                    range.1,
                    !props
                        .iter()
                        .any(|p| p.key.as_ref() == "local.scope-inherits" && p.value.as_deref() == Some("false")),
                )),
                name if name.starts_with("local.definition") => {
                    let attr = if parameter {
                        md::PARAMETER
                    } else if name.ends_with(".function") {
                        md::FUNCTION
                    } else {
                        0
                    };
                    if n.named_child_count() == 0 {
                        definitions.push((range, attr));
                    } else {
                        let mut walk = n.walk();
                        loop {
                            let v = walk.node();
                            if matches!(v.kind(), "identifier" | "simple_identifier" | "variable_name" | "variable") {
                                definitions.push(((v.start_byte(), v.end_byte()), attr));
                            }
                            if walk.goto_first_child() {
                                continue;
                            }
                            loop {
                                if walk.goto_next_sibling() {
                                    break;
                                }
                                if !walk.goto_parent() {
                                    break;
                                }
                            }
                            if walk.node() == n {
                                break;
                            }
                        }
                    }
                }
                "local.reference" => references.push(range),
                _ => {}
            }
        }
        if definitions.len() + references.len() + scopes.len() > 200_000 {
            return Err("too many locals".into());
        }
    }
    drop(matches);
    if cursor.did_exceed_match_limit() {
        return Err("locals match limit".into());
    }
    scopes.sort_unstable_by_key(|&(a, b, inherit)| (a, std::cmp::Reverse(b), inherit));
    scopes.dedup_by_key(|s| (s.0, s.1));
    let mut parents = Vec::with_capacity(scopes.len());
    let mut open: Vec<usize> = Vec::new();
    for (i, scope) in scopes.iter().enumerate() {
        while open.last().is_some_and(|&p| scopes[p].1 < scope.1 || scopes[p].1 <= scope.0) {
            open.pop();
        }
        parents.push(open.last().copied());
        open.push(i);
    }
    let scope_for = |a: usize, b: usize| {
        let mut i = scopes.partition_point(|s| s.0 <= a).saturating_sub(1);
        while scopes[i].1 < b {
            if let Some(p) = parents[i] {
                i = p;
            } else {
                break;
            }
        }
        i
    };
    let mut names: HashMap<(&str, usize), Vec<(usize, crate::md::Attr)>> = HashMap::new();
    let mut out = HashMap::new();
    // Bundled parameter captures refine the upstream definition captures.
    definitions.sort_by_key(|&(r, a)| (r, std::cmp::Reverse(a)));
    definitions.dedup_by_key(|d| d.0);
    for ((a, b), attr) in definitions {
        if Instant::now() >= deadline {
            return Err("local resolution exceeded query budget".into());
        }
        names.entry((&text[a..b], scope_for(a, b))).or_default().push((a, attr));
        out.insert((a, b), attr);
    }
    for (a, b) in references {
        let mut scope = Some(scope_for(a, b));
        while let Some(i) = scope {
            if Instant::now() >= deadline {
                return Err("local resolution exceeded query budget".into());
            }
            if let Some(defs) = names.get(&(&text[a..b], i)) {
                let end = defs.partition_point(|(at, _)| *at <= a);
                if end > 0 {
                    out.insert((a, b), defs[end - 1].1);
                    break;
                }
            }
            if !scopes[i].2 {
                break;
            }
            scope = parents[i];
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Downloads JavaScript grammar and compiles it with cc"]
    fn grammars_build_scoped_locals() {
        let g = super::super::tests::ready("javascript");
        let l = load(g, &directory().join(g.name)).unwrap();
        let text = "function outer(value) { function inner(value) { return value; } return value; }\nvalue;";
        let p = parse(&l, text, None).unwrap();
        let locals = resolve(&l.locals, &p.tree, text, Instant::now() + Duration::from_secs(1)).unwrap();
        let values: Vec<_> = text.match_indices("value").map(|(a, _)| locals.get(&(a, a + 5)).copied()).collect();
        assert_eq!(
            values,
            vec![Some(md::PARAMETER), Some(md::PARAMETER), Some(md::PARAMETER), Some(md::PARAMETER), None]
        );
        assert!(p.spans.iter().any(|s| s.attr == md::PARAMETER));
    }
}
