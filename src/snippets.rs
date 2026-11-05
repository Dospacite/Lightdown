//! Project templates use $1, ${1:default}, and $0. Repeated stops are rejected.
use serde_json::Value;
use std::path::Path;
#[derive(Clone)]
pub struct Snippet {
    pub prefix: String,
    pub body: String,
}
pub fn load(root: &Path, extension: &str) -> Vec<Snippet> {
    merge(load_file(&crate::config::config_dir().join("snippets.json"), extension),
          load_file(&root.join(".lightdown/snippets.json"), extension))
}
fn merge(user: Vec<Snippet>, project: Vec<Snippet>) -> Vec<Snippet> {
    let mut entries = std::collections::BTreeMap::new();
    for snippet in user.into_iter().chain(project) {
        entries.insert(snippet.prefix.clone(), snippet);
    }
    entries.into_values().collect()
}
fn load_file(path: &Path, extension: &str) -> Vec<Snippet> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    if bytes.len() > 512 * 1024 {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return Vec::new() };
    v[extension]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(Snippet {
                prefix: s["prefix"].as_str()?.into(),
                body: match &s["body"] {
                    Value::String(s) => s.clone(),
                    Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
                    _ => return None,
                },
            })
        })
        .collect()
}
/// Substitute context variables as literal template text, before parsing stops.
pub fn variables(body: &str, values: &[(&str, String)]) -> String {
    let mut out = String::new();
    let mut it = body.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            out.push(c);
            if let Some(next) = it.next() { out.push(next); }
            continue;
        }
        if c != '$' { out.push(c); continue; }
        let braced = it.peek() == Some(&'{');
        if braced { it.next(); }
        let mut name = String::new();
        while it.peek().is_some_and(|c| c.is_ascii_uppercase() || *c == '_') {
            name.push(it.next().unwrap());
        }
        let mut fallback = None;
        if braced && it.peek() == Some(&':') {
            it.next();
            let mut text = String::new();
            while it.peek().is_some_and(|c| *c != '}') { text.push(it.next().unwrap()); }
            fallback = Some(text);
        }
        let closed = !braced || it.peek() == Some(&'}');
        if closed && (!name.is_empty()) && (fallback.is_some() || values.iter().any(|(key, _)| *key == name)) {
            let value = values.iter().find(|(key, _)| *key == name).map(|(_,v)|v).filter(|v|!v.is_empty()).or(fallback.as_ref()).map(String::as_str).unwrap_or("");
            if braced { it.next(); }
            for c in value.chars() {
                if matches!(c, '$' | '\\' | '}') { out.push('\\'); }
                out.push(c);
            }
        } else {
            out.push('$');
            if braced { out.push('{'); }
            out.push_str(&name);
        }
    }
    out
}
/// A stop of an expanded snippet: where it lies, where its mirrors lie,
/// and the choices it offers, if any.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Stop {
    pub at: (usize, usize),
    pub mirrors: Vec<(usize, usize)>,
    pub choices: Vec<String>,
}

/// A snippet body expanded: its text and its stops in order, `$0` last.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Expansion {
    pub text: String,
    pub stops: Vec<Stop>,
}

/// The text and the stops' first ranges, without mirrors or choices.
pub fn expand(body: &str) -> Result<(String, Vec<(usize, usize)>), String> {
    let e = expand_full(body)?;
    Ok((e.text, e.stops.into_iter().map(|s| s.at).collect()))
}

/// A piece of a parsed body.
#[derive(Clone, Debug)]
enum Piece {
    Text(String),
    Stop { n: usize, inner: Vec<Piece>, choices: Vec<String> },
}

/// Reads pieces up to the end, or up to an unescaped `}` when `nested`.
fn pieces(it: &mut std::iter::Peekable<std::str::Chars>, depth: usize, nested: bool) -> Result<Vec<Piece>, String> {
    if depth > 16 {
        return Err("Snippet nesting or size limit exceeded".into());
    }
    let mut out = Vec::new();
    let mut text = String::new();
    loop {
        let Some(c) = it.next() else {
            if nested {
                return Err("Unclosed snippet default".into());
            }
            break;
        };
        match c {
            '\\' => text.push(it.next().unwrap_or('\\')),
            '}' if nested => break,
            '$' if it.peek().is_some_and(|c| c.is_ascii_digit() || *c == '{') => {
                let braced = it.peek() == Some(&'{');
                if braced {
                    it.next();
                }
                let mut number = String::new();
                while it.peek().is_some_and(|c| c.is_ascii_digit()) {
                    number.push(it.next().unwrap());
                }
                if number.is_empty() {
                    // `${` without a number is text, as before.
                    text.push_str("${");
                    continue;
                }
                let n = number.parse::<usize>().map_err(|_| "Snippet stop number too large")?;
                let (mut inner, mut choices) = (Vec::new(), Vec::new());
                if braced {
                    match it.next() {
                        Some('}') => {}
                        Some(':') => inner = pieces(it, depth + 1, true)?,
                        Some('|') => {
                            let mut choice = String::new();
                            loop {
                                match it.next() {
                                    None => return Err("Unclosed snippet choice".into()),
                                    Some('\\') => choice.push(it.next().unwrap_or('\\')),
                                    Some(',') => choices.push(std::mem::take(&mut choice)),
                                    Some('|') if it.peek() == Some(&'}') => {
                                        it.next();
                                        choices.push(choice);
                                        break;
                                    }
                                    Some(c) => choice.push(c),
                                }
                            }
                            inner = vec![Piece::Text(choices[0].clone())];
                        }
                        Some(_) => return Err("Use ${1:default} for snippet stops".into()),
                        None => return Err("Unclosed snippet stop".into()),
                    }
                }
                if !text.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut text)));
                }
                out.push(Piece::Stop { n, inner, choices });
            }
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    Ok(out)
}

/// The occurrence of each stop that gives its text: the first with a
/// default or choices, else the first.
fn defining(list: &[Piece], found: &mut std::collections::BTreeMap<usize, (Vec<Piece>, Vec<String>)>) {
    for p in list {
        if let Piece::Stop { n, inner, choices } = p {
            let e = found.entry(*n).or_insert_with(|| (inner.clone(), choices.clone()));
            if e.0.is_empty() && !inner.is_empty() {
                *e = (inner.clone(), choices.clone());
            }
            defining(inner, found);
        }
    }
}

/// Writes the pieces out, recording stops. The first occurrence of a
/// number is its stop and shows its defining text; later ones mirror it as
/// plain text.
fn render(
    list: &[Piece],
    defs: &std::collections::BTreeMap<usize, (Vec<Piece>, Vec<String>)>,
    plain: bool,
    out: &mut String,
    stops: &mut std::collections::BTreeMap<usize, Stop>,
) {
    for p in list {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Stop { n, .. } => {
                let (inner, choices) = &defs[n];
                let a = out.len();
                let first = !plain && !stops.contains_key(n);
                if first {
                    stops.insert(*n, Stop { choices: choices.clone(), ..Default::default() });
                }
                render(inner, defs, plain || !first, out, stops);
                let range = (a, out.len());
                if let Some(s) = stops.get_mut(n).filter(|_| !plain) {
                    if first {
                        s.at = range;
                    } else {
                        s.mirrors.push(range);
                    }
                }
            }
        }
    }
}

/// Expands a body: `$1`, `${1:default}` with nested stops, `${1|a,b|}`
/// choices, repeated numbers as mirrors, and `$0` as the end.
pub fn expand_full(body: &str) -> Result<Expansion, String> {
    if body.len() > 512 * 1024 {
        return Err("Snippet nesting or size limit exceeded".into());
    }
    let list = pieces(&mut body.chars().peekable(), 0, false)?;
    let mut defs = Default::default();
    defining(&list, &mut defs);
    let mut text = String::new();
    let mut stops = std::collections::BTreeMap::new();
    render(&list, &defs, false, &mut text, &mut stops);
    let last = stops.remove(&0).unwrap_or(Stop { at: (text.len(), text.len()), ..Default::default() });
    let mut stops: Vec<Stop> = stops.into_values().collect();
    stops.push(last);
    Ok(Expansion { text, stops })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variable_defaults_and_nested_defaults() {
        let body = variables("${WORKSPACE_NAME:project} ${UNKNOWN:fallback}", &[("WORKSPACE_NAME", "".into())]);
        assert_eq!(expand(&body).unwrap().0, "project fallback");
        assert_eq!(expand("${1:hello ${2:world}} $0").unwrap().0, "hello world ");
    }
    #[test]
    fn variables_are_literal_and_keep_numbered_stops() {
        let body = variables("$TM_FILENAME ${TM_SELECTED_TEXT} ${1:name} $0", &[
            ("TM_FILENAME", "a.rs".into()), ("TM_SELECTED_TEXT", "$1\\é".into())]);
        let (text, ranges) = expand(&body).unwrap();
        assert_eq!(text, "a.rs $1\\é name ");
        assert_eq!(&text[ranges[0].0..ranges[0].1], "name");
    }
    #[test]
    fn project_prefixes_override_user_prefixes() {
        let entry = |prefix: &str, body: &str| Snippet { prefix: prefix.into(), body: body.into() };
        let merged = merge(vec![entry("a", "user"), entry("b", "kept")], vec![entry("a", "project")]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].body, "project");
        assert_eq!(merged[1].body, "kept");
    }
    #[test]
    fn expansion_preserves_unicode_and_stop_order() {
        let (s, r) = expand("${2:é} = ${1:😀};$0").unwrap();
        assert_eq!(s, "é = 😀;");
        assert_eq!(r, vec![(5, 9), (0, 2), (10, 10)]);
        assert!(expand("${1:no").is_err());
        assert!(expand("${1|a,b").is_err());
    }
    #[test]
    fn repeated_stops_mirror_the_first_and_choices_default_to_the_first() {
        let e = expand_full("$1 = ${1:x}; ${2|one,two\\,three|} $0").unwrap();
        assert_eq!(e.text, "x = x; one ");
        assert_eq!(e.stops[0].at, (0, 1));
        assert_eq!(e.stops[0].mirrors, [(4, 5)]);
        assert_eq!(e.stops[1].choices, ["one", "two,three"]);
        assert_eq!(e.stops[2].at, (11, 11));
        // Nested stops lie inside their parent; a mirror of the parent
        // copies the text without stops of its own.
        let e = expand_full("${1:a ${2:b}} $1").unwrap();
        assert_eq!(e.text, "a b a b");
        assert_eq!((e.stops[0].at, e.stops[1].at), ((0, 3), (2, 3)));
        assert_eq!(e.stops[0].mirrors, [(4, 7)]);
        assert!(e.stops[1].mirrors.is_empty());
    }
}
/// Local calendar values use libc, already linked by the application.
pub fn context(workspace: &Path) -> Vec<(&'static str, String)> {
    let mut values = vec![("WORKSPACE_NAME", workspace.file_name().unwrap_or_default().to_string_lossy().into_owned())];
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&now, &mut tm).is_null() {
            for (key,n,width) in [("CURRENT_YEAR",tm.tm_year+1900,4),("CURRENT_MONTH",tm.tm_mon+1,2),("CURRENT_DATE",tm.tm_mday,2),("CURRENT_HOUR",tm.tm_hour,2),("CURRENT_MINUTE",tm.tm_min,2)] {
                values.push((key,format!("{n:0width$}")));
            }
        }
    }
    use std::io::Read;
    let mut bytes = [0u8;16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f|f.read_exact(&mut bytes)).is_ok() {
        bytes[6]=(bytes[6]&15)|64; bytes[8]=(bytes[8]&63)|128;
        values.push(("UUID",bytes.iter().enumerate().map(|(i,b)|format!("{}{b:02x}",if [4,6,8,10].contains(&i) {"-"} else {""})).collect()));
    }
    values
}
