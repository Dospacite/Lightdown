//! Highlighting shared by source files and fenced code blocks.
//! Installed tree-sitter grammars run on workers; lexical spans are the fallback.

use crate::md::{self, Span};

pub fn spans(text: &str, base: usize, language: &str, out: &mut Vec<Span>) {
    let language = language.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if let Some(spans) = crate::treesitter::spans(text, &language) {
        out.extend(spans.into_iter().map(|mut s| { s.start += base as u32; s.end += base as u32; s }));
        return;
    }
    if matches!(language.as_str(), "py" | "python" | "python3") {
        return crate::py::spans(text, base, out);
    }
    if matches!(language.as_str(), "tex" | "latex") {
        return crate::latex::spans(text, base, out);
    }
    let (keywords, hash, slash, block, backtick) = match language.as_str() {
        "rs" | "rust" => (
            "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while",
            false,
            true,
            true,
            false,
        ),
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" => (
            "abstract as async await break case catch class const continue debugger declare default delete do else enum export extends false finally for from function if implements import in instanceof interface let new null of private protected public readonly return static super switch this throw true try type typeof undefined var void while with yield",
            false,
            true,
            true,
            true,
        ),
        "c" | "h" | "cpp" | "hpp" | "c++" | "cc" | "java" | "cs" | "go" => (
            "auto bool break case catch char class const continue default defer delete do double else enum export extern false final finally float for func goto if import int interface long namespace new null nullptr package private protected public return short signed sizeof static string struct super switch template this throw true try typedef typename union unsigned using var virtual void volatile while",
            false,
            true,
            true,
            false,
        ),
        "json" | "jsonc" => ("true false null", false, language == "jsonc", language == "jsonc", false),
        "sh" | "bash" | "zsh" | "shell" => (
            "case do done elif else esac export fi for function if in local readonly return then until while",
            true,
            false,
            false,
            true,
        ),
        "toml" | "yaml" | "yml" => ("true false null yes no on off", true, false, false, false),
        "sql" => (
            "select from where join left right inner outer on as and or not null true false insert into values update set delete create table index drop alter group by order having limit offset distinct union all with primary key references",
            false,
            false,
            true,
            false,
        ),
        "css" | "scss" => ("important inherit initial unset auto none", false, false, true, false),
        _ => return,
    };
    let b = text.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 128;
    let mut push = |s: usize, e: usize, attr| {
        out.push(Span { start: (base + s) as u32, end: (base + e) as u32, attr });
    };
    let mut i = 0;
    while i < b.len() {
        let from = i;
        let rest = &b[i..];
        if (hash && b[i] == b'#')
            || (slash && rest.starts_with(b"//"))
            || (language == "sql" && rest.starts_with(b"--"))
        {
            i += rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len());
            push(from, i, md::DIM);
        } else if block && rest.starts_with(b"/*") {
            i += 2;
            // Rust permits nested block comments.
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if (language == "rust" || language == "rs") && b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                    continue;
                }
                if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            push(from, i, md::DIM);
        } else if matches!(b[i], b'"' | b'\'') || (backtick && b[i] == b'`') {
            let quote = b[i];
            // A Rust lifetime is an identifier, not an unterminated string.
            if quote == b'\'' && matches!(language.as_str(), "rs" | "rust") && b.get(i + 1).is_some_and(|c| ident(*c)) {
                let end = i + 1 + b[i + 1..].iter().take_while(|c| ident(**c)).count();
                if b.get(end) != Some(&b'\'') {
                    i = end;
                    push(from, i, md::SPECIAL);
                    continue;
                }
            }
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                } else if b[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            push(from, i, md::SOFT);
        } else if b[i].is_ascii_digit() {
            i += 1;
            while i < b.len()
                && (ident(b[i]) || b[i] == b'.' || (matches!(b[i], b'+' | b'-') && matches!(b[i - 1], b'e' | b'E')))
            {
                i += 1;
            }
            push(from, i, md::NUMBER);
        } else if ident(b[i]) {
            i += b[i..].iter().take_while(|c| ident(**c)).count();
            let word = &text[from..i];
            let sql_word = word.to_ascii_lowercase();
            let key = if language == "sql" { &sql_word } else { word };
            if keywords.split_whitespace().any(|k| k == key) {
                push(from, i, md::KEYWORD | md::BOLD);
            } else if text[i..].trim_start().starts_with('(') {
                push(from, i, md::FUNCTION);
            } else if word.starts_with(char::is_uppercase) {
                push(from, i, md::SPECIAL);
            }
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_respect_comments_strings_and_unicode_offsets() {
        let text = "const café = call(42); /* return 1 */\nlet s = \"if 3\";";
        let mut out = Vec::new();
        spans(text, 7, "javascript", &mut out);
        let tokens: Vec<_> = out.iter().map(|s| (&text[s.start as usize - 7..s.end as usize - 7], s.attr)).collect();
        assert_eq!(
            tokens,
            [
                ("const", md::KEYWORD | md::BOLD),
                ("call", md::FUNCTION),
                ("42", md::NUMBER),
                ("/* return 1 */", md::DIM),
                ("let", md::KEYWORD | md::BOLD),
                ("\"if 3\"", md::SOFT)
            ]
        );
    }

    #[test]
    fn rust_lifetimes_and_nested_comments_do_not_swallow_code() {
        let mut out = Vec::new();
        let text = "fn f<'a>(s: &'a str) { /* a /* b */ c */ return 1; }";
        spans(text, 0, "rust", &mut out);
        assert!(out.iter().any(|s| &text[s.start as usize..s.end as usize] == "return"));
        assert_eq!(out.iter().filter(|s| s.attr == md::DIM).count(), 1);
    }

    #[test]
    fn unknown_languages_stay_plain() {
        let mut out = Vec::new();
        spans("# a heading", 0, "unknown", &mut out);
        assert!(out.is_empty());
    }
}
