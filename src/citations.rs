//! A bounded BibTeX reader for completion, previews and a built-in bibliography.
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub struct Entry { pub key: String, pub title: String, pub author: String, pub year: String, pub venue: String }
impl Entry {
    pub fn preview(&self) -> String { format!("{}\n{} ({})\n{}", self.title, self.author, self.year, self.venue) }
}
fn braced(source: &str, start: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    let opener = *bytes.get(start)?;
    let closer = if opener == b'{' { b'}' } else if opener == b'"' { b'"' } else { return None };
    let mut depth = 0;
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' { i += 2; continue; }
        if bytes[i] == b'{' { depth += 1; }
        else if bytes[i] == b'}' && depth > 0 { depth -= 1; }
        else if bytes[i] == closer && depth == 0 { return Some((source[start + 1..i].into(), i + 1)); }
        i += 1;
    }
    None
}
pub fn parse(source: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut pos = 0;
    while let Some(i) = source[pos..].find('@') {
        pos += i + 1;
        let Some(open) = source[pos..].find('{').map(|i| pos + i) else { break };
        let kind = source[pos..open].trim().to_ascii_lowercase();
        let Some((body, end)) = braced(source, open) else { break };
        pos = end;
        if matches!(kind.as_str(), "comment" | "preamble" | "string") { continue; }
        let Some((key, fields)) = body.split_once(',') else { continue };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_alphanumeric() || "_:-./+".contains(c)) { continue; }
        let mut entry = Entry { key: key.into(), title: String::new(), author: String::new(), year: String::new(), venue: String::new() };
        let mut at = 0;
        while at < fields.len() {
            let Some(eq) = fields[at..].find('=').map(|i| at + i) else { break };
            let name = fields[at..eq].trim().trim_start_matches(',').trim().to_ascii_lowercase();
            at = eq + 1;
            at += fields[at..].len() - fields[at..].trim_start().len();
            let (value, end) = if fields[at..].starts_with(['{', '"']) {
                match braced(fields, at) { Some(v) => v, None => break }
            } else {
                let end = fields[at..].find(',').map_or(fields.len(), |i| at + i);
                (fields[at..end].trim().to_string(), end)
            };
            let value = value.replace(['{', '}'], "").split_whitespace().collect::<Vec<_>>().join(" ");
            match name.as_str() { "title" => entry.title = value, "author" | "editor" if entry.author.is_empty() => entry.author = value, "year" => entry.year = value, "journal" | "booktitle" | "publisher" if entry.venue.is_empty() => entry.venue = value, _ => {} }
            at = end;
            if let Some(comma) = fields[at..].find(',') { at += comma + 1; } else { break; }
        }
        entries.push(entry);
        if entries.len() == 10_000 { break; }
    }
    entries
}
pub fn root(document: &Path) -> PathBuf {
    let folder = document.parent().unwrap_or(Path::new("."));
    folder.ancestors().find(|p| p.join(".git").exists()).unwrap_or(folder).to_path_buf()
}
pub fn read(root: &Path) -> Vec<Entry> {
    let (mut dirs, mut paths, mut remaining) = (vec![root.to_path_buf()], Vec::new(), 4 << 20);
    let mut visited = 0;
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(dir) else { continue };
        for entry in read.flatten() {
            visited += 1; if visited > 20_000 { break; }
            let name = entry.file_name(); let name = name.to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules" | "venv" | "__pycache__") { continue; }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() { dirs.push(entry.path()); }
            else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "bib") { paths.push(entry.path()); }
        }
        if visited > 20_000 { break; }
    }
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        let size = std::fs::metadata(&path).map_or(usize::MAX, |m| m.len() as usize);
        if size > 1 << 20 || size > remaining { continue; }
        remaining -= size;
        if let Ok(text) = std::fs::read_to_string(path) {
            for entry in parse(&text) { if !entries.iter().any(|e: &Entry| e.key == entry.key) { entries.push(entry); } }
        }
    }
    entries
}
pub fn query(text: &str, cur: usize) -> Option<(usize, &str)> {
    let before = &text[..cur];
    let start = if let Some(i) = before.rfind("\\cite{").filter(|&i| !before[i + 6..].contains('}')) {
        i + 6 + before[i + 6..].rfind(',').map_or(0, |n| n + 1)
    } else {
        before.rfind("[@").filter(|&i| !before[i + 2..].contains(']'))? + 2
    };
    let prefix = &text[start..cur];
    (!prefix.contains(['\n', '{', '}', '[', ']', ' '])).then_some((start, prefix))
}
pub fn at(text: &str, off: usize) -> Option<(usize, usize, String)> {
    let off = crate::editor::floor(text, off.min(text.len()));
    let word = |c: char| c.is_alphanumeric() || "_:-./+".contains(c);
    let a = text[..off].char_indices().rev().find(|(_, c)| !word(*c)).map_or(0, |(i, c)| i + c.len_utf8());
    let b = off + text[off..].find(|c| !word(c)).unwrap_or(text.len() - off);
    let prefix = &text[..a];
    let citation = prefix.ends_with("[@") || prefix.rfind("\\cite{").is_some_and(|i| !text[i + 6..a].contains(['}', '\n']));
    (citation && a < b).then(|| (a, b, text[a..b].to_string()))
}
pub fn bibliography(markdown: &str, entries: &[Entry]) -> String {
    let buffer = crate::editor::Buffer::new(markdown.into());
    let md = crate::md::parse(markdown, &buffer.starts);
    let mut used = Vec::new();
    let mut out = markdown.to_string();
    let mut edits = Vec::new();
    let mut pos = 0;
    while let Some(i) = markdown[pos..].find("[@") {
        let a = pos + i;
        let Some(end) = markdown[a..].find(']') else { break }; let b = a + end + 1; pos = b;
        let kind = md.lines[buffer.line_of(a)].kind;
        if matches!(kind, crate::md::Kind::Code | crate::md::Kind::FenceOpen | crate::md::Kind::Meta) || md.spans.iter().any(|s| s.start as usize <= a && a < s.end as usize && s.attr & crate::md::CODE != 0) { continue; }
        let keys: Vec<_> = markdown[a + 2..b - 1].split(';').map(|s| s.trim().trim_start_matches('@')).collect();
        let found: Option<Vec<_>> = keys.iter().map(|k| entries.iter().position(|e| e.key == *k)).collect();
        let Some(found) = found else { continue };
        let labels: Vec<_> = found.into_iter().map(|i| {
            if !used.contains(&i) { used.push(i); }
            let e = &entries[i]; format!("{} {}", e.author, e.year)
        }).collect();
        edits.push((a, b, format!("({})", labels.join("; "))));
    }
    for (a, b, text) in edits.into_iter().rev() { out.replace_range(a..b, &text); }
    if !used.is_empty() {
        out += "\n\n## References\n\n";
        for i in used {
            let e = &entries[i]; out += &format!("- {} ({}). {}. {}\n", e.author, e.year, e.title, e.venue);
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bibtex_nested_braces_quotes_and_bibliography() {
        let entries = parse("@article{key, title={A {Nested} title}, author=\"One and Two\", year=2024, journal={Journal}}\n@comment{ignored}");
        assert_eq!(entries.len(), 1); assert_eq!(entries[0].title, "A Nested title"); assert_eq!(entries[0].year, "2024");
        let source = "Text [@key]. `[@key]`\n\n```text\n[@key]\n```\n";
        let expanded = bibliography(source, &entries);
        assert!(expanded.contains("Text (One and Two 2024). `[@key]`"));
        assert!(expanded.contains("## References") && expanded.contains("```text\n[@key]"));
        assert_eq!(query("\\cite{ke", 8), Some((6, "ke")));
        assert_eq!(at("[@key]", 3), Some((2, 5, "key".into())));
    }
}
