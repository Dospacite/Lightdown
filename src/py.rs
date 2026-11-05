//! Python support that needs no interpreter: token classes for highlighting,
//! indentation, and the names offered for completion.

use std::collections::HashSet;

use crate::md::{self, Span};

const KEYWORDS: [&str; 35] = [
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
    "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
    "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

/// Keywords only where a statement starts.
const SOFT_KEYWORDS: [&str; 2] = ["match", "case"];

const BUILTINS: [&str; 92] = [
    "abs",
    "all",
    "any",
    "ascii",
    "bin",
    "bool",
    "breakpoint",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "compile",
    "complex",
    "delattr",
    "dict",
    "dir",
    "divmod",
    "enumerate",
    "eval",
    "exec",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "globals",
    "hasattr",
    "hash",
    "help",
    "hex",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "list",
    "locals",
    "map",
    "max",
    "memoryview",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
    "self",
    "cls",
    "__init__",
    "__name__",
    "__main__",
    "__file__",
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "Exception",
    "FileNotFoundError",
    "ImportError",
    "IndexError",
    "KeyError",
    "KeyboardInterrupt",
    "NameError",
    "NotImplementedError",
    "OSError",
    "RuntimeError",
    "StopIteration",
    "TypeError",
    "ValueError",
    "ZeroDivisionError",
    "match",
];

fn word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// Appends the styled stretches of `text`: keywords, strings, comments and
/// decorators. Offsets are shifted by `base`.
pub fn spans(text: &str, base: usize, out: &mut Vec<Span>) {
    let b = text.as_bytes();
    let mut push = |s: usize, e: usize, attr: crate::md::Attr| {
        if e > s {
            out.push(Span { start: (base + s) as u32, end: (base + e) as u32, attr });
        }
    };
    let line_end = |i: usize| b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n);
    let mut i = 0;
    // Nothing but blanks so far on this line.
    let mut fresh = true;
    while i < b.len() {
        let c = b[i];
        let was_fresh = fresh;
        fresh = c == b'\n' || (fresh && (c == b' ' || c == b'\t'));
        match c {
            b'#' => {
                let e = line_end(i);
                push(i, e, md::DIM);
                i = e;
            }
            b'@' if was_fresh => {
                let e = i + 1 + b[i + 1..].iter().take_while(|&&c| word(c) || c == b'.').count();
                push(i, e, md::SPECIAL);
                i = e;
            }
            b'"' | b'\'' => i = string(b, i, i, &mut push),
            _ if c.is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < b.len()
                    && (word(b[i]) || b[i] == b'.' || (matches!(b[i], b'+' | b'-') && matches!(b[i - 1], b'e' | b'E')))
                {
                    i += 1;
                }
                push(start, i, md::NUMBER);
            }
            _ if word(c) => {
                let e = i + b[i..].iter().take_while(|&&c| word(c)).count();
                let w = &text[i..e];
                let prefix = w.len() <= 3 && w.bytes().all(|c| b"rbfutRBFUT".contains(&c));
                if prefix && matches!(b.get(e), Some(b'"' | b'\'')) {
                    i = string(b, i, e, &mut push);
                    continue;
                }
                let soft = was_fresh && b.get(e) == Some(&b' ') && b.get(e + 1) != Some(&b'=');
                if KEYWORDS.contains(&w) || (soft && SOFT_KEYWORDS.contains(&w)) {
                    push(i, e, md::BOLD | md::KEYWORD);
                } else if (w != "match" && BUILTINS.contains(&w)) || text[e..].trim_start().starts_with('(') {
                    push(i, e, md::FUNCTION);
                }
                i = e;
            }
            _ => i += 1,
        }
    }
}

/// Styles the string literal whose prefix starts at `from` and whose opening
/// quote is at `quote`. Returns the offset after it.
fn string(b: &[u8], from: usize, quote: usize, push: &mut impl FnMut(usize, usize, crate::md::Attr)) -> usize {
    let q = b[quote];
    let triple = b.get(quote + 1) == Some(&q) && b.get(quote + 2) == Some(&q);
    let mut i = quote + if triple { 3 } else { 1 };
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'\n' if !triple => break,
            c if c == q && (!triple || (b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q))) => {
                i += if triple { 3 } else { 1 };
                push(from, i, md::SOFT);
                return i;
            }
            _ => {}
        }
        i += 1;
    }
    let end = i.min(b.len());
    push(from, end, md::SOFT);
    end
}

/// The indentation for a new line typed after `line`: the same, deeper after
/// a colon or an open bracket, shallower after a statement that leaves.
pub fn indent_after(line: &str) -> String {
    let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut indent = line[..lead].to_string();
    let code = line.split('#').next().unwrap_or("").trim();
    let leaves = ["return", "pass", "break", "continue", "raise"]
        .iter()
        .any(|k| code.strip_prefix(k).is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '('])));
    if code.ends_with([':', '(', '[', '{']) {
        indent += "    ";
    } else if leaves {
        indent.truncate(indent.len().saturating_sub(4));
    }
    indent
}

/// What is being typed at the cursor, as far as completion cares.
#[derive(Clone, PartialEq, Debug)]
pub struct Query {
    /// Where the word under completion starts.
    pub start: usize,
    pub prefix: String,
    /// The dotted expression before the word, when it follows a dot.
    pub base: Option<String>,
}

pub fn query(text: &str, cur: usize) -> Option<Query> {
    let b = text.as_bytes();
    let mut start = cur;
    while start > 0 && word(b[start - 1]) {
        start -= 1;
    }
    let prefix = &text[start..cur];
    if prefix.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let mut base = None;
    if start > 0 && b[start - 1] == b'.' {
        let mut s = start - 1;
        while s > 0 && (word(b[s - 1]) || b[s - 1] == b'.') {
            s -= 1;
        }
        let expr = &text[s..start - 1];
        // A number before the dot is a float being typed, not an object.
        if expr.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        base = Some(expr.to_string());
    }
    (base.is_some() || !prefix.is_empty()).then(|| Query { start, prefix: prefix.to_string(), base })
}

/// The name at an offset, by its byte range: a word that does not start
/// with a digit and is not a keyword.
pub fn name_at(text: &str, off: usize) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let (mut s, mut e) = (off.min(b.len()), off.min(b.len()));
    while s > 0 && word(b[s - 1]) {
        s -= 1;
    }
    while e < b.len() && word(b[e]) {
        e += 1;
    }
    let w = &text[s..e];
    let usable = !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()) && !KEYWORDS.contains(&w);
    usable.then_some((s, e))
}

/// The call the cursor is in, as far as parameter hints care.
#[derive(Clone, PartialEq, Debug)]
pub struct Call {
    /// Where the callee's name ends.
    pub callee: usize,
    /// Where the call's bracket is.
    pub open: usize,
    /// Which argument is being typed, counted from 0.
    pub index: usize,
    /// The name the argument is given, when it is passed by keyword.
    pub keyword: Option<String>,
}

/// Finds the innermost call around `cur`, reading `text` from its start and
/// passing over strings and comments. Brackets that do not follow a name
/// (`(1 + 2)`, `if (`) and definitions (`def f(`) are not calls.
pub fn call_at(text: &str, cur: usize) -> Option<Call> {
    let b = text.as_bytes();
    let cur = cur.min(b.len());
    // Each open bracket: where it is, which it is, its commas so far, and
    // where its current argument starts.
    let mut stack: Vec<(usize, u8, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < cur {
        match b[i] {
            b'#' => {
                i += b[i..].iter().position(|&c| c == b'\n').unwrap_or(b.len() - i);
                continue;
            }
            q @ (b'"' | b'\'') => {
                let triple = b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q);
                i += if triple { 3 } else { 1 };
                while i < cur {
                    match b[i] {
                        b'\\' => i += 1,
                        b'\n' if !triple => break,
                        c if c == q && (!triple || (b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q))) => {
                            i += if triple { 2 } else { 0 };
                            break;
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            c @ (b'(' | b'[' | b'{') => stack.push((i, c, 0, i + 1)),
            b')' | b']' | b'}' => {
                stack.pop();
            }
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.2 += 1;
                    top.3 = i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let &(open, _, index, arg) = stack.iter().rev().find(|e| e.1 == b'(')?;
    let mut callee = open;
    while callee > 0 && matches!(b[callee - 1], b' ' | b'\t') {
        callee -= 1;
    }
    let (s, _) = name_at(text, callee).filter(|&(s, e)| e == callee && s < e)?;
    let before = text[..s].trim_end_matches([' ', '\t']);
    let prior = &before[before.len() - before.bytes().rev().take_while(|&c| word(c)).count()..];
    if matches!(prior, "def" | "class") {
        return None;
    }
    let typed = text[arg.min(cur)..cur].trim_start();
    let kw_len = typed.bytes().take_while(|&c| word(c)).count();
    let rest = typed[kw_len..].trim_start();
    let keyword = (kw_len > 0 && rest.starts_with('=') && !rest.starts_with("==")).then(|| typed[..kw_len].to_string());
    Some(Call { callee, open, index, keyword })
}

/// Which of a signature's parameters an argument goes to. Parameters are
/// as written: `a`, `b=1`, `*args`, `c: int = 2`, `**kw`.
pub fn active_param(params: &[&str], index: usize, keyword: Option<&str>) -> Option<usize> {
    let name = |p: &str| p.trim_start_matches('*').split([':', '=']).next().unwrap_or("").trim().to_string();
    if let Some(k) = keyword {
        return params.iter().position(|p| name(p) == k).or_else(|| params.iter().position(|p| p.starts_with("**")));
    }
    let star = params.iter().position(|p| p.starts_with('*') && !p.starts_with("**"));
    match star {
        Some(s) if index >= s => Some(s),
        _ => (index < params.len() && !params[index].starts_with("**")).then_some(index),
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Candidate {
    pub name: String,
    pub kind: String,
}

/// Completions that can be found without running anything: names used in
/// the code itself, keywords and builtins. `code` is the text to gather
/// names from, in pieces; the word being typed at `skip` is left out.
pub fn candidates<'a>(code: impl Iterator<Item = (usize, &'a str)>, q: &Query, limit: usize) -> Vec<Candidate> {
    let attr = q.base.is_some();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (off, text) in code {
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if !word(b[i]) || b[i].is_ascii_digit() {
                i += 1 + b[i + 1..].iter().take_while(|&&c| b[i].is_ascii_digit() && word(c)).count();
                continue;
            }
            let e = i + b[i..].iter().take_while(|&&c| word(c)).count();
            let (w, dotted) = (&text[i..e], i > 0 && b[i - 1] == b'.');
            let fits = dotted == attr && off + i != q.start && w.len() > q.prefix.len() && w.starts_with(&q.prefix);
            if fits && seen.insert(w) {
                out.push(Candidate { name: w.to_string(), kind: String::new() });
            }
            i = e;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if !attr {
        for (list, kind) in [(&KEYWORDS[..], "keyword"), (&BUILTINS[..], "builtin")] {
            for w in list.iter().filter(|w| w.len() > q.prefix.len() && w.starts_with(&q.prefix)) {
                if seen.insert(w) {
                    out.push(Candidate { name: w.to_string(), kind: kind.to_string() });
                }
            }
        }
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(text: &str) -> Vec<(&str, crate::md::Attr)> {
        let mut out = Vec::new();
        spans(text, 0, &mut out);
        out.iter().map(|s| (&text[s.start as usize..s.end as usize], s.attr)).collect()
    }

    #[test]
    fn tokens() {
        let t = "@dec.x\ndef f(a): # hi\n    return f'{a}' + \"\"\"two\nlines\"\"\" + r'\\'\nmatch = 1e5\nmatch x:\n";
        assert_eq!(
            styled(t),
            [
                ("@dec.x", md::SPECIAL),
                ("def", md::BOLD | md::KEYWORD),
                ("f", md::FUNCTION),
                ("# hi", md::DIM),
                ("return", md::BOLD | md::KEYWORD),
                ("f'{a}'", md::SOFT),
                ("\"\"\"two\nlines\"\"\"", md::SOFT),
                ("r'\\'", md::SOFT),
                ("1e5", md::NUMBER),
                ("match", md::BOLD | md::KEYWORD),
            ]
        );
        // An unterminated string stops at the end of its line.
        assert_eq!(styled("x = 'abc\ny"), [("'abc", md::SOFT)]);
    }

    #[test]
    fn indentation() {
        assert_eq!(indent_after("  if x:  # why"), "      ");
        assert_eq!(indent_after("    return x"), "");
        assert_eq!(indent_after("    returned = 1"), "    ");
        assert_eq!(indent_after("x = ["), "    ");
    }

    #[test]
    fn completion() {
        let t = "import os\nvalue = os.path\nva";
        let q = query(t, t.len()).unwrap();
        assert_eq!((q.start, q.prefix.as_str(), &q.base), (t.len() - 2, "va", &None));
        let names: Vec<String> = candidates([(0, t)].into_iter(), &q, 9).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["value", "vars"]);

        let t = "os.path.jo";
        let q = query(t, t.len()).unwrap();
        assert_eq!((q.prefix.as_str(), q.base.as_deref()), ("jo", Some("os.path")));
        assert_eq!(query("x = 1.", 6), None);
        assert_eq!(query("x = ", 4), None);
        assert!(query("os.", 3).is_some());
    }

    #[test]
    fn names_and_calls() {
        let t = "x = os.path.join(a1, 2)";
        assert_eq!(name_at(t, 13), Some((12, 16)));
        assert_eq!(name_at(t, 16), Some((12, 16)));
        assert_eq!(name_at("for x", 1), None);
        assert_eq!(name_at("1abc", 2), None);

        let call = |t: &str| call_at(t, t.len());
        let c = call("print(a, f(1), 'x, y', sep=").unwrap();
        assert_eq!((c.callee, c.open, c.index, c.keyword.as_deref()), (5, 5, 3, Some("sep")));
        // Inside a list that is an argument, the call still holds.
        assert_eq!(call("f(a, [1, 2").map(|c| c.index), Some(1));
        assert_eq!(call("f (x == 1").map(|c| (c.callee, c.keyword)), Some((1, None)));
        // A closed call, a bare bracket, a definition and a comment are not.
        assert_eq!(call("f(a)"), None);
        assert_eq!(call("x = (1, "), None);
        assert_eq!(call("if ("), None);
        assert_eq!(call("def f(a, "), None);
        assert_eq!(call("f(a) # g("), None);
        assert_eq!(call("f('(', "), Some(Call { callee: 1, open: 1, index: 1, keyword: None }));
        assert_eq!(call("f('''a\n(''', ").map(|c| c.index), Some(1));

        let p = ["a", "b=1", "*rest", "key=None", "**kw"];
        assert_eq!(active_param(&p, 1, None), Some(1));
        assert_eq!(active_param(&p, 5, None), Some(2));
        assert_eq!(active_param(&p, 0, Some("key")), Some(3));
        assert_eq!(active_param(&p, 0, Some("other")), Some(4));
        assert_eq!(active_param(&["a: int = 2"], 0, Some("a")), Some(0));
        assert_eq!(active_param(&["a"], 3, None), None);
    }
}
