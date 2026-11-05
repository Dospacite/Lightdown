//! Values while paused: for each line from the paused function's start to
//! the stopped line, the locals named there with their short values. They
//! come from the variables the debugger already sent; nothing is evaluated.
use serde_json::Value;

/// Lines looked at above the stopped line, values per line, and characters
/// per value.
const MAX_LINES: usize = 500;
const PER_LINE: usize = 4;
const VALUE_CHARS: usize = 60;
/// Variables that are definitions rather than data are left out.
const QUIET_TYPES: &[&str] = &["function", "type", "module", "builtin_function_or_method", "method"];

/// The labels for lines `start..=stop` of `source`, by line.
pub(super) fn labels(source: &str, start: usize, stop: usize, variables: &Value, python: bool) -> Vec<(usize, String)> {
    let known: Vec<(&str, &str)> = variables.as_array().into_iter().flatten().take(200).filter_map(|v| {
        let name = v["name"].as_str()?;
        let kind = v["type"].as_str().unwrap_or("");
        (!QUIET_TYPES.contains(&kind) && !(name.starts_with("__") && name.ends_with("__"))).then(|| (name, v["value"].as_str().unwrap_or("")))
    }).collect();
    if known.is_empty() { return Vec::new(); }
    let start = start.max(stop.saturating_sub(MAX_LINES)).min(stop);
    let mut triple: Option<&'static str> = None;
    let mut out = Vec::new();
    for (line, text) in source.lines().enumerate().take(stop + 1) {
        // Triple-quoted strings are followed from the file's top.
        let names = names(text, python, &mut triple);
        if line < start { continue; }
        let mut shown: Vec<String> = Vec::new();
        let mut used: Vec<&str> = Vec::new();
        for n in names {
            if used.contains(&n) || shown.len() >= PER_LINE { continue; }
            if let Some((name, value)) = known.iter().find(|(k, _)| *k == n) {
                used.push(name);
                let value: String = value.replace('\n', " ");
                let short: String = value.chars().take(VALUE_CHARS).collect();
                let more = if value.chars().count() > VALUE_CHARS { "…" } else { "" };
                shown.push(format!("{name} = {short}{more}"));
            }
        }
        if !shown.is_empty() { out.push((line, shown.join(", "))); }
    }
    out
}

/// The plain names on a line, in order: not inside strings or comments, and
/// not attributes after a dot.
fn names<'a>(text: &'a str, python: bool, triple: &mut Option<&'static str>) -> Vec<&'a str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut after_dot = false;
    while i < bytes.len() {
        if let Some(close) = *triple {
            match text[i..].find(close) { Some(at) => { i += at + 3; *triple = None; continue; } None => return out }
        }
        let b = bytes[i];
        if python && (text[i..].starts_with("\"\"\"") || text[i..].starts_with("'''")) {
            *triple = Some(if b == b'"' { "\"\"\"" } else { "'''" });
            i += 3;
            continue;
        }
        if (python && b == b'#') || (!python && text[i..].starts_with("//")) { break; }
        if b == b'"' || (b == b'\'' && (python || bytes.get(i + 2) == Some(&b'\'') || bytes.get(i + 1) == Some(&b'\\'))) {
            i += 1;
            while i < bytes.len() && bytes[i] != b {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            after_dot = false;
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' || b >= 0x80 {
            let s = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 0x80) { i += 1; }
            if !after_dot { out.push(&text[s..i]); }
            after_dot = false;
            continue;
        }
        if b.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') { i += 1; }
            continue;
        }
        if !b.is_ascii_whitespace() { after_dot = b == b'.' || (b == b'>' && i > 0 && bytes[i - 1] == b'-'); }
        i += 1;
    }
    out
}

/// Where the function holding `stop` likely starts, for adapters that do
/// not say: the nearest header line above it, within a bound.
pub(super) fn function_start(source: &str, stop: usize) -> usize {
    let lines: Vec<&str> = source.lines().take(stop + 1).collect();
    for line in (stop.saturating_sub(MAX_LINES)..lines.len()).rev() {
        let t = lines[line].trim_start();
        let words = ["fn ", "pub fn ", "pub(crate) fn ", "async fn ", "pub async fn ", "def ", "async def ", "func "];
        let c_like = !lines[line].starts_with([' ', '\t', '{', '}', '#', '/', '*']) && t.contains('(') && !t.trim_end().ends_with(';');
        if words.iter().any(|w| t.starts_with(w)) || c_like { return line; }
    }
    stop
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uses_whole_names_in_frame_and_bounds_values() {
        let vars = json!([{"name":"x","value":"3"}]);
        assert_eq!(labels("x = 1\nxx = 2\nx + 1\nx", 0, 2, &vars, true), vec![(0, "x = 3".into()), (2, "x = 3".into())]);
        let long = json!([{"name":"s","value":"a".repeat(100)}]);
        assert_eq!(labels("s", 0, 0, &long, true)[0].1, format!("s = {}…", "a".repeat(60)));
    }

    #[test]
    fn skips_strings_comments_attributes_and_definitions() {
        let vars = json!([{"name":"items","value":"[1]"},{"name":"self","value":"<A object>"},{"name":"f","type":"function","value":"<function f>"},
            {"name":"n","value":"2"},{"name":"__name__","value":"'__main__'"}]);
        let source = "self.items.append(n)  # items\nprint('items', \"n\")\n\"\"\"\nitems\n\"\"\"\nf(n) if __name__ else items";
        assert_eq!(labels(source, 0, 5, &vars, true), vec![(0, "self = <A object>, n = 2".into()), (5, "n = 2, items = [1]".into())]);
        let rust = "let total = x->count; // total\nlet c = 'x'; total";
        let vars = json!([{"name":"total","value":"5"},{"name":"count","value":"1"},{"name":"x","value":"0"}]);
        assert_eq!(labels(rust, 0, 1, &vars, false), vec![(0, "total = 5, x = 0".into()), (1, "total = 5".into())]);
    }

    #[test]
    fn bounds_values_per_line_and_lines_before_the_stop() {
        let vars = json!([{"name":"a","value":"1"},{"name":"b","value":"1"},{"name":"c","value":"1"},{"name":"d","value":"1"},{"name":"e","value":"1"}]);
        assert_eq!(labels("a b c d e a", 0, 0, &vars, true)[0].1.matches(" = ").count(), PER_LINE);
        let source = "a\n".repeat(MAX_LINES + 50);
        let out = labels(&source, 0, MAX_LINES + 40, &vars, true);
        assert_eq!(out.len(), MAX_LINES + 1);
        assert_eq!(out[0].0, 40);
    }

    #[test]
    fn guesses_where_native_functions_start() {
        let rust = "use x;\n\nfn main() {\n    let a = 1;\n    if a > 0 {\n        go(a);\n    }\n}\n";
        assert_eq!(function_start(rust, 5), 2);
        let c = "#include <x.h>\nint square(int x)\n{\n    int y = x * x;\n    return y;\n}\n";
        assert_eq!(function_start(c, 4), 1);
        assert_eq!(function_start("    a\n    b\n", 1), 1);
    }
}
