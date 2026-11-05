//! Semantic tokens: the client's legend, delta edits and decoding to highlight spans.
use super::*;
use crate::md::{self, Span};
/// Standard token types the client asks for, as the protocol names them.
pub const TYPES: [&str; 25] = [
    "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property",
    "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string", "number",
    "regexp", "operator", "decorator", "lifetime", "typeAlias",
];
pub const MODIFIERS: [&str; 10] = ["declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification", "documentation", "defaultLibrary"];
/// Tokens beyond this are dropped; it bounds memory and layout work on huge files.
const MAX_TOKENS: usize = 200_000;
/// The client capability advertised in initialize.
pub fn capability() -> Value {
    json!({"dynamicRegistration":false,"requests":{"range":true,"full":{"delta":true}},"tokenTypes":TYPES,"tokenModifiers":MODIFIERS,"formats":["relative"],"overlappingTokenSupport":false,"multilineTokenSupport":false})
}
/// Maps a server token type onto the editor's syntax roles. Names without a
/// colour of their own, such as variables and parameters, keep the base colour.
pub fn role(kind: &str) -> crate::md::Attr {
    match kind {
        "function" | "method" | "macro" => md::FUNCTION,
        "namespace" | "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "decorator" | "lifetime"
        | "typeAlias" | "builtinType" | "attribute" | "derive" => md::SPECIAL,
        "keyword" | "modifier" | "selfKeyword" | "selfTypeKeyword" => md::KEYWORD,
        "string" | "regexp" | "character" | "escapeSequence" => md::SOFT,
        "number" | "enumMember" | "boolean" => md::NUMBER,
        "comment" => md::DIM,
        _ => 0,
    }
}
/// The server's legend, turned into a role per token type index.
pub fn legend(capabilities: &Value) -> Option<Vec<crate::md::Attr>> {
    let provider = &capabilities["semanticTokensProvider"];
    let types = provider["legend"]["tokenTypes"].as_array()?;
    Some(types.iter().map(|t| role(t.as_str().unwrap_or(""))).collect())
}
/// Applies a delta reply to the previous data. Edits index the flat integer array.
pub fn apply_delta(old: &[u32], edits: &Value) -> Option<Vec<u32>> {
    let mut edits: Vec<(usize, usize, Vec<u32>)> = edits.as_array()?.iter().map(|e| {
        let data = e["data"].as_array().map(|d| d.iter().filter_map(|n| n.as_u64().map(|n| n as u32)).collect()).unwrap_or_default();
        Some((e["start"].as_u64()? as usize, e["deleteCount"].as_u64()? as usize, data))
    }).collect::<Option<_>>()?;
    edits.sort_by_key(|e| e.0);
    let mut out = Vec::with_capacity(old.len());
    let mut at = 0;
    for (start, delete, data) in edits {
        if start < at || start + delete > old.len() { return None; }
        out.extend_from_slice(&old[at..start]);
        out.extend(data);
        at = start + delete;
    }
    out.extend_from_slice(&old[at..]);
    (out.len() <= MAX_TOKENS * 5).then_some(out)
}
pub fn data(result: &Value) -> Vec<u32> {
    result["data"].as_array().into_iter().flatten().take(MAX_TOKENS * 5).filter_map(|n| n.as_u64().map(|n| n as u32)).collect()
}
/// Decodes relative tokens into byte spans of `text`, sorted by start.
/// Tokens outside the text or across a line end are skipped.
pub fn spans(text: &str, data: &[u32], roles: &[crate::md::Attr]) -> Vec<Span> {
    let mut out = Vec::new();
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let (mut line, mut col) = (0usize, 0usize);
    for t in data.chunks_exact(5) {
        let (dl, dc, len, kind) = (t[0] as usize, t[1] as usize, t[2] as usize, t[3] as usize);
        if dl > 0 { line += dl; col = dc; } else { col += dc; }
        let attr = roles.get(kind).copied().unwrap_or(0);
        if attr == 0 { continue; }
        let Some(&ls) = starts.get(line) else { break };
        let le = text[ls..].find('\n').map_or(text.len(), |n| ls + n);
        // Walk UTF-16 units to bytes within the line.
        let (mut units, mut a, mut b) = (0, None, None);
        for (i, ch) in text[ls..le].char_indices() {
            if units == col { a = Some(ls + i); }
            if units == col + len { b = Some(ls + i); break; }
            units += ch.len_utf16();
        }
        if units == col + len && b.is_none() { b = Some(le); }
        if let (Some(a), Some(b)) = (a, b) && a < b {
            out.push(Span { start: a as u32, end: b as u32, attr });
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens_decode_relative_utf16_positions_and_skip_plain_kinds() {
        let roles = legend(&json!({"semanticTokensProvider":{"legend":{"tokenTypes":["function","variable","struct"]}}})).unwrap();
        // "fn 😀foo(x: Bar)" with foo at UTF-16 column 5, then x and Bar.
        let text = "fn 😀foo(x: Bar)\nnext";
        let data = [0, 5, 3, 0, 0, 0, 4, 1, 1, 0, 0, 3, 3, 2, 0, 1, 0, 4, 0, 0];
        let s = spans(text, &data, &roles);
        assert_eq!(s.len(), 3);
        assert_eq!(&text[s[0].start as usize..s[0].end as usize], "foo");
        assert_eq!(s[0].attr, md::FUNCTION);
        assert_eq!(&text[s[1].start as usize..s[1].end as usize], "Bar");
        assert_eq!(s[1].attr, md::SPECIAL);
        assert_eq!(&text[s[2].start as usize..s[2].end as usize], "next");
        // A token running past the line end is dropped.
        assert!(spans("ab\ncd", &[0, 0, 5, 0, 0], &roles).is_empty());
    }
    #[test]
    fn deltas_splice_the_flat_array_and_refuse_bad_edits() {
        let old = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let new = apply_delta(&old, &json!([{"start":5,"deleteCount":5,"data":[0,0,1,0,0]}])).unwrap();
        assert_eq!(new, [1, 2, 3, 4, 5, 0, 0, 1, 0, 0]);
        assert_eq!(apply_delta(&old, &json!([{"start":0,"deleteCount":0}])).unwrap(), old);
        assert!(apply_delta(&old, &json!([{"start":8,"deleteCount":5}])).is_none());
    }
}
