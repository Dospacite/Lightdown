//! Standalone HTML from the existing parser. Resources are embedded on demand.
use std::collections::HashMap;
use std::path::Path;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
fn safe_url(url: &str) -> bool {
    !url.chars().any(char::is_control) && (!url.contains(':') || ["http://", "https://", "mailto:", "ftp://", "data:image/"].iter().any(|s| url.starts_with(s)))
}
fn image(document: Option<&Path>, url: &str) -> Result<String, String> {
    if url.contains("://") && !url.starts_with("file://") { return Err(format!("Remote image cannot be embedded: {url}")); }
    let path = crate::pane::image_file(document, url).ok_or("Invalid image path")?;
    let mime = match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "webp" => "image/webp", "gif" => "image/gif", "svg" => "image/svg+xml", _ => return Err("Unsupported HTML image format".into()),
    };
    let size = std::fs::metadata(&path).map_err(|e| format!("Cannot embed {}: {e}", path.display()))?.len();
    if size > 32 << 20 { return Err("HTML image exceeds 32 MB".into()); }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(format!("data:{mime};base64,{}", crate::images::encode(&bytes)))
}
/// A single renderer supplies export and rich-copy HTML.
pub fn render(source: &str, document: Option<&Path>, engine: &str) -> Result<String, String> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES | Options::ENABLE_MATH | Options::ENABLE_WIKILINKS | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut out = String::new();
    let (mut metadata, mut image_depth, mut header, mut column) = (false, 0usize, false, 0usize);
    let mut aligns = Vec::new();
    let mut formulas = HashMap::new();
    let mut counts = HashMap::new();
    let mut ids = Vec::new();
    let (mut heading, mut title) = (false, String::new());
    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(Tag::Heading { .. }) => { heading = true; title.clear(); }
            Event::Text(s) | Event::Code(s) if heading => title += &s,
            Event::End(TagEnd::Heading(_)) => {
                let base = crate::app::links::slug(&title);
                let n = counts.entry(base.clone()).or_insert(0);
                ids.push(if *n == 0 { base } else { format!("{base}-{n}") }); *n += 1; heading = false;
            }
            _ => {}
        }
    }
    let mut ids = ids.into_iter();
    for event in Parser::new_ext(source, options) {
        let displayed = matches!(&event, Event::DisplayMath(_));
        if image_depth > 0 {
            match event {
                Event::Start(Tag::Image { .. }) => image_depth += 1,
                Event::End(TagEnd::Image) => { image_depth -= 1; if image_depth == 0 { out += "\"/>"; } }
                Event::Text(s) | Event::Code(s) => out += &escape(&s),
                _ => {}
            }
            continue;
        }
        if metadata {
            if matches!(event, Event::End(TagEnd::MetadataBlock(_))) { metadata = false; }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out += "<p>",
                Tag::Heading { level, .. } => {
                    out += &format!("<h{} id=\"{}\">", level as usize, escape(&ids.next().unwrap_or_default()));
                }
                Tag::BlockQuote(_) => out += "<blockquote>",
                Tag::CodeBlock(kind) => {
                    let name = match kind { pulldown_cmark::CodeBlockKind::Fenced(name) => name.into_string(), _ => String::new() };
                    out += &format!("<pre><code class=\"language-{}\">", escape(&name));
                }
                Tag::List(Some(n)) => out += &format!("<ol start=\"{n}\">"),
                Tag::List(None) => out += "<ul>", Tag::Item => out += "<li>",
                Tag::Emphasis => out += "<em>", Tag::Strong => out += "<strong>", Tag::Strikethrough => out += "<del>",
                Tag::Link { dest_url, title, .. } => {
                    let url = if safe_url(&dest_url) { dest_url.as_ref() } else { "#" };
                    out += &format!("<a href=\"{}\" title=\"{}\">", escape(url), escape(&title));
                }
                Tag::Image { dest_url, .. } => { out += &format!("<img src=\"{}\" alt=\"", image(document, &dest_url)?); image_depth = 1; }
                Tag::Table(a) => { aligns = a; out += "<table>"; }
                Tag::TableHead => { header = true; column = 0; out += "<thead><tr>"; }
                Tag::TableRow => { column = 0; out += "<tr>"; }
                Tag::TableCell => {
                    let align = match aligns.get(column) { Some(pulldown_cmark::Alignment::Center) => "center", Some(pulldown_cmark::Alignment::Right) => "right", _ => "left" };
                    out += &format!("<{} style=\"text-align:{align}\">", if header { "th" } else { "td" }); column += 1;
                }
                Tag::FootnoteDefinition(id) => out += &format!("<aside id=\"fn-{}\"><p><b>{}</b></p>", escape(&id), escape(&id)),
                Tag::MetadataBlock(_) => metadata = true,
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => out += "</p>\n", TagEnd::Heading(level) => out += &format!("</h{}>\n", level as usize),
                TagEnd::BlockQuote(_) => out += "</blockquote>\n", TagEnd::CodeBlock => out += "</code></pre>\n",
                TagEnd::List(true) => out += "</ol>\n", TagEnd::List(false) => out += "</ul>\n", TagEnd::Item => out += "</li>\n",
                TagEnd::Emphasis => out += "</em>", TagEnd::Strong => out += "</strong>", TagEnd::Strikethrough => out += "</del>", TagEnd::Link => out += "</a>",
                TagEnd::Table => out += "</tbody></table>\n", TagEnd::TableHead => { out += "</tr></thead><tbody>"; header = false; }
                TagEnd::TableRow => out += "</tr>\n", TagEnd::TableCell => out += if header { "</th>" } else { "</td>" },
                TagEnd::FootnoteDefinition => out += "</aside>\n", _ => {}
            },
            Event::Text(text) => out += &escape(&text),
            Event::Code(code) => out += &format!("<code>{}</code>", escape(&code)),
            Event::Html(html) | Event::InlineHtml(html) => {
                if matches!(html.trim(), "<u>" | "</u>" | "<br>" | "<br/>" | "<br />") { out += &html; }
                else { out += &escape(&html); }
            }
            Event::InlineMath(tex) | Event::DisplayMath(tex) => {
                let key = (tex.to_string(), displayed);
                if !formulas.contains_key(&key) { formulas.insert(key.clone(), crate::math::svg(&tex, displayed, engine)?); }
                let svg = &formulas[&key];
                let style = if displayed { "display:block;text-align:center" } else { "" };
                out += &format!("<span class=\"math\" style=\"{style}\">{svg}</span>");
            }
            Event::SoftBreak => out.push('\n'), Event::HardBreak => out += "<br/>\n", Event::Rule => out += "<hr/>\n",
            Event::TaskListMarker(checked) => out += if checked { "<input type=\"checkbox\" disabled checked/> " } else { "<input type=\"checkbox\" disabled/> " },
            Event::FootnoteReference(id) => out += &format!("<sup><a href=\"#fn-{}\">{}</a></sup>", escape(&id), escape(&id)),
        }
        if out.len() > 64 << 20 { return Err("HTML output exceeds 64 MB".into()); }
    }
    let title = document.and_then(Path::file_stem).and_then(|s| s.to_str()).unwrap_or("Document");
    Ok(format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><style>body{{max-width:54em;margin:3em auto;padding:0 1em;font:16px/1.6 system-ui;color:#18181b;background:white}}img{{max-width:100%}}table{{border-collapse:collapse}}td,th{{border:1px solid #aaa;padding:.3em .6em}}pre{{overflow:auto;background:#f4f4f5;padding:1em}}.math svg{{vertical-align:middle;max-width:100%}}</style></head><body>\n{out}</body></html>\n", escape(title)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standalone_html_contains_tables_headings_footnotes_and_safe_links() {
        let html = render("# Hello *world*\n\n[link](javascript:bad)\n\n| A | B |\n|---|---:|\n|1|2|\n\nNote[^1]\n\n[^1]: text\n\n<script>bad()</script>\n", None, "").unwrap();
        assert!(html.contains("<h1 id=\"hello-world\">Hello <em>world</em></h1>"));
        assert!(html.contains("<table>") && html.contains("text-align:right"));
        assert!(html.contains("id=\"fn-1\""));
        assert!(!html.contains("href=\"javascript:") && !html.contains("<script>"));
    }
    #[test]
    fn local_images_are_embedded() {
        let dir = std::env::temp_dir().join(format!("lightdown-html-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), crate::pics::tests::png(1, 1, &[0, 0, 0, 255])).unwrap();
        let html = render("![alt](a.png)", Some(&dir.join("note.md")), "").unwrap();
        assert!(html.contains("data:image/png;base64,") && html.contains("alt=\"alt\""));
        assert!(render("![](missing.png)", Some(&dir.join("note.md")), "").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn tex_formulas_export_as_vector_paths_when_engine_exists() {
        let Some(_) = crate::tex::find("") else { return };
        let html = render("$x^2 + \\frac{1}{2}$", None, "").unwrap();
        assert!(html.contains("<svg") && html.contains("<path") && !html.contains("<text"));
    }
}
