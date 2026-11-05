//! Optional diagram programs run only when a matching fence is requested.
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
#[derive(Clone, Debug)]
pub struct Diagram { pub start: usize, pub end: usize, pub line: usize, pub language: String, pub source: String }
pub fn fences(text: &str) -> Vec<Diagram> {
    let mut diagrams = Vec::new(); let mut current = None;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(name))) => {
                let lang = name.split_whitespace().next().unwrap_or("");
                let language = match lang { "mermaid" => "mermaid", "dot" | "graphviz" => "graphviz", "plantuml" | "puml" => "plantuml", _ => continue };
                current = Some(Diagram { start: range.start, end: range.end, line: text[..range.end].trim_end_matches('\n').bytes().filter(|&b| b == b'\n').count(), language: language.into(), source: String::new() });
            }
            Event::Text(source) => if let Some(d) = &mut current { d.source += &source; },
            Event::End(TagEnd::CodeBlock) => if let Some(d) = current.take() { diagrams.push(d); },
            _ => {}
        }
    }
    diagrams
}
pub fn directory(diagram: &Diagram, cache: &Path) -> PathBuf {
    let mut hash = DefaultHasher::new(); diagram.language.hash(&mut hash); diagram.source.hash(&mut hash);
    cache.join("diagrams").join(format!("{:016x}", hash.finish()))
}
pub fn render(diagram: &Diagram, cache: &Path, executable: Option<&Path>) -> Result<PathBuf, String> {
    if diagram.source.len() > 1024 * 1024 { return Err("Diagram source exceeds 1 MB".into()); }
    let dir = directory(diagram, cache);
    let svg = dir.join("diagram.svg"); let png = dir.join("diagram.png");
    if svg.is_file() && png.is_file() { return Ok(svg); }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (tool, extension) = match diagram.language.as_str() { "mermaid" => ("mmdc", "mmd"), "graphviz" => ("dot", "dot"), "plantuml" => ("plantuml", "puml"), _ => return Err("Unsupported diagram language".into()) };
    let source = dir.join(format!("diagram.{extension}"));
    std::fs::write(&source, &diagram.source).map_err(|e| e.to_string())?;
    let mut command = Command::new("timeout"); command.arg("60").arg(executable.unwrap_or(Path::new(tool)));
    match tool {
        "mmdc" => { command.arg("-i").arg(&source).arg("-o").arg(&svg); }
        "dot" => { command.args(["-Tsvg", "-o"]).arg(&svg).arg(&source); }
        _ => { command.args(["-tsvg", "-nometadata"]).arg(&source); }
    }
    let result = command.stdin(Stdio::null()).output().map_err(|e| format!("Cannot run {tool} or timeout: {e}"))?;
    if !result.status.success() {
        let error = String::from_utf8_lossy(&result.stderr).chars().take(500).collect::<String>();
        return Err(format!("Diagram needs {tool} on PATH, or its source failed: {error}"));
    }
    let (w, h) = crate::pics::probe(&svg).ok_or_else(|| format!("{tool} did not produce a valid SVG"))?;
    if w as u64 * h as u64 > 32_000_000 { return Err("Diagram exceeds 32 million pixels".into()); }
    crate::pics::svg_png(&svg, &png).ok_or("Cannot render diagram SVG for PDF")?;
    Ok(svg)
}
/// PDF builds use the images already prepared by the diagram worker. Each
/// fence becomes its picture over as many lines as the fence took, so later
/// lines keep their numbers for markdown SyncTeX.
pub fn pdf_source(text: &str, cache: &Path) -> String {
    let mut out = text.to_string();
    for d in fences(text).into_iter().rev() {
        let png = directory(&d, cache).join("diagram.png");
        if png.is_file() {
            let path = png.to_string_lossy().replace(' ', "%20").replace('(', "%28").replace(')', "%29");
            let lines = text[d.start..d.end].matches('\n').count();
            out.replace_range(d.start..d.end, &format!("\n![]({path}){}", "\n".repeat(lines.saturating_sub(1))));
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn fences_run_optional_tools_and_pdf_reuses_the_image() {
        let dir = std::env::temp_dir().join(format!("lightdown-diagrams-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("dot");
        std::fs::write(&fake, "#!/bin/sh\nwhile [ $# -gt 0 ]; do\nif [ \"$1\" = -o ]; then shift; out=$1; fi\nshift\ndone\nprintf '<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"10\"><rect width=\"20\" height=\"10\" fill=\"red\"/></svg>' > \"$out\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let text = "# Diagram\n\n```graphviz\ndigraph { a -> b }\n```\n";
        let d = fences(text); assert_eq!(d.len(), 1); assert_eq!(d[0].line, 4);
        let path = render(&d[0], &dir, Some(&fake)).unwrap();
        assert_eq!(crate::pics::probe(&path), Some((20, 10)));
        assert_eq!(crate::pics::probe(&path.with_extension("png")), Some((20, 10)));
        assert!(!pdf_source(text, &dir).contains("```graphviz"));
        let longer = "# Diagram\n\n```graphviz\ndigraph { a -> b }\n```\nafter\n";
        let replaced = pdf_source(longer, &dir);
        assert_eq!(replaced.matches('\n').count(), longer.matches('\n').count());
        assert!(replaced.ends_with("\nafter\n") && replaced.contains("![]("));
        let other = Diagram { source: "bad".into(), ..d[0].clone() };
        assert!(render(&other, &dir, Some(&dir.join("missing"))).unwrap_err().contains("dot"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
