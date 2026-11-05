//! Clipboard HTML is input data. No browser, scripts or remote resources.
use crate::table;

fn gap(out: &mut String, lines: usize) {
    while out.ends_with(' ') { out.pop(); }
    if out.is_empty() { return; }
    let have = out.bytes().rev().take_while(|&b| b == b'\n').count();
    out.push_str(&"\n".repeat(lines.saturating_sub(have)));
}
fn escaped(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\`*_[]<>#|".contains(c) { out.push('\\'); }
        out.push(c);
    }
    out
}
fn text(out: &mut String, s: &str) {
    for c in table::entities(s).chars() {
        if c.is_whitespace() {
            if !out.is_empty() && !out.ends_with(char::is_whitespace) { out.push(' '); }
        } else {
            out.push_str(&escaped(&c.to_string()));
        }
    }
}
/// A tag ends only outside quoted attributes.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if q == c => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}
fn attribute(tag: &str, wanted: &str) -> Option<String> {
    let mut rest = tag.split_once(char::is_whitespace)?.1.trim();
    while !rest.is_empty() {
        let end = rest.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(rest.len());
        let key = &rest[..end];
        rest = rest[end..].trim_start();
        if !rest.starts_with('=') {
            if end == 0 { rest = &rest[1..]; }
            continue;
        }
        rest = rest[1..].trim_start();
        let value;
        if let Some(q) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) {
            rest = &rest[1..];
            let end = rest.find(q)?;
            value = &rest[..end];
            rest = &rest[end + 1..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            value = &rest[..end];
            rest = &rest[end..];
        }
        if key.eq_ignore_ascii_case(wanted) { return Some(table::entities(value)); }
        rest = rest.trim_start();
    }
    None
}
fn destination(url: &str) -> Option<String> {
    if url.chars().any(char::is_control) { return None; }
    if url.contains(':') && !["http://", "https://", "mailto:", "ftp://"].iter().any(|s| url.starts_with(s)) { return None; }
    Some(url.replace(' ', "%20").replace('(', "%28").replace(')', "%29").replace('<', "%3C").replace('>', "%3E").replace('\\', "%5C"))
}
fn literal(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while !rest.is_empty() {
        let i = rest.find('<').unwrap_or(rest.len());
        out.push_str(&rest[..i]);
        if i == rest.len() { break; }
        let Some(end) = tag_end(&rest[i..]) else { break };
        rest = &rest[i + end + 1..];
    }
    table::entities(&out)
}

pub fn html(html: &str) -> Result<String, String> {
    if html.len() > 4 * 1024 * 1024 { return Err("HTML clipboard exceeds 4 MB; use plain paste".into()); }
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut pos = 0;
    let mut lists: Vec<usize> = Vec::new();
    let mut links = Vec::new();
    let mut tags = 0;
    while pos < html.len() {
        let next = html[pos..].find('<').map_or(html.len(), |i| pos + i);
        text(&mut out, &html[pos..next]);
        if next == html.len() { break; }
        if html[next..].starts_with("<!--") {
            pos = html[next + 4..].find("-->").map_or(html.len(), |i| next + 4 + i + 3);
            continue;
        }
        let Some(end) = tag_end(&html[next + 1..]).map(|i| next + 1 + i) else { return Err("Malformed HTML; use plain paste".into()) };
        let tag = html[next + 1..end].trim();
        let name = tag.split_whitespace().next().unwrap_or("").trim_end_matches('/').to_ascii_lowercase();
        pos = end + 1;
        tags += 1;
        if tags > 100_000 || lists.len() > 32 || links.len() > 32 { return Err("HTML is too complex; use plain paste".into()); }
        match name.as_str() {
            "script" | "style" | "head" | "iframe" | "svg" => {
                let close = format!("</{name}>");
                pos = lower[pos..].find(&close).map_or(html.len(), |i| pos + i + close.len());
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => { gap(&mut out, 2); out += &"#".repeat(name.as_bytes()[1] as usize - b'0' as usize); out.push(' '); }
            "/h1" | "/h2" | "/h3" | "/h4" | "/h5" | "/h6" | "p" | "/p" | "div" | "/div" => gap(&mut out, 2),
            "br" => { while out.ends_with(' ') { out.pop(); } out += "  \n"; }
            "hr" => { gap(&mut out, 2); out += "---\n\n"; }
            "strong" | "b" | "/strong" | "/b" => out += "**",
            "em" | "i" | "/em" | "/i" => out.push('*'),
            "del" | "s" | "/del" | "/s" => out += "~~",
            "ul" | "ol" => { gap(&mut out, if lists.is_empty() { 2 } else { 1 }); lists.push(if name == "ol" { 1 } else { 0 }); }
            "/ul" | "/ol" => { lists.pop(); gap(&mut out, if lists.is_empty() { 2 } else { 1 }); }
            "li" => {
                gap(&mut out, 1);
                out += &"  ".repeat(lists.len().saturating_sub(1));
                match lists.last_mut() {
                    Some(n) if *n > 0 => { out += &format!("{n}. "); *n += 1; }
                    _ => out += "- ",
                }
            }
            "/li" => gap(&mut out, 1),
            "a" => {
                let url = attribute(tag, "href").and_then(|u| destination(&u));
                if url.is_some() { out.push('['); }
                links.push(url);
            }
            "/a" => if let Some(Some(url)) = links.pop() { out += &format!("]({url})"); },
            "pre" | "code" => {
                let close = format!("</{name}>");
                let end = lower[pos..].find(&close).map_or(html.len(), |i| pos + i);
                let code = literal(&html[pos..end]);
                let ticks = code.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1;
                if name == "pre" {
                    gap(&mut out, 2);
                    let fence = "`".repeat(ticks.max(3));
                    out += &format!("{fence}\n{}\n{fence}\n\n", code.trim_end_matches('\n'));
                } else {
                    let fence = "`".repeat(ticks);
                    out += &format!("{fence} {code} {fence}");
                }
                pos = (end + close.len()).min(html.len());
            }
            "table" => {
                let end = lower[pos..].find("</table>").map_or(html.len(), |i| pos + i + 8);
                let t = table::parse(&html[next..end]).ok_or("Cannot convert HTML table; use plain paste")?;
                if t.truncated { return Err("HTML table is too large; use plain paste".into()); }
                gap(&mut out, 2);
                out += &table::markdown(&t.rows);
                gap(&mut out, 2);
                pos = end;
            }
            _ => {}
        }
    }
    Ok(out.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headings_lists_links_tables_and_code() {
        let md = html("<h2>Café &amp; tea</h2><p>A <strong>bold</strong> <a href='https://e.org/a(b)?q=&quot;x&quot;'>link</a>.</p><ol><li>one</li><li>two</li></ol><pre><code>x &lt; 2\n  `yes`</code></pre><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>x|y</td></tr></table>").unwrap();
        assert!(md.starts_with("## Café & tea\n\nA **bold**"));
        assert!(md.contains("[link](https://e.org/a%28b%29?q=\"x\")"));
        assert!(md.contains("1. one\n2. two"));
        assert!(md.contains("```\nx < 2\n  `yes`\n```"));
        assert!(md.contains("| A | B |\n| --- | --- |\n| 1 | x\\|y |"));
    }
    #[test]
    fn unsafe_content_is_inert_and_quotes_do_not_end_tags() {
        let md = html("<!--discard--><script>alert('bad')</script><style>bad</style><a href='javascript:bad()'>safe</a><a title='>' href='/a b'>local</a><p>[literal] *text*</p>").unwrap();
        assert!(!md.contains("bad") && !md.contains("discard"));
        assert!(md.contains("safe[local](/a%20b)"));
        assert!(md.contains(r"\[literal\] \*text\*"));
    }
}

/// RFC 4180-style CSV, or TSV. Refuse ragged input and incomplete quotes.
pub fn csv(text: &str) -> Result<String, String> {
    if text.len() > 4 * 1024 * 1024 { return Err("CSV exceeds 4 MB".into()); }
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let delimiter = if text.contains('\t') { '\t' } else { ',' };
    let mut chars = text.chars().peekable();
    let (mut rows, mut row, mut cell) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut closed) = (false, false);
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') { chars.next(); cell.push('"'); }
                else { quoted = false; closed = true; }
            } else { cell.push(c); }
        } else if c == delimiter || c == '\n' {
            row.push(std::mem::take(&mut cell)); closed = false;
            if c == '\n' { rows.push(std::mem::take(&mut row)); }
        } else if c == '"' && cell.is_empty() && !closed { quoted = true; }
        else if closed || c == '"' { return Err("Malformed CSV quoting".into()); }
        else { cell.push(c); }
        if rows.len() > 10_000 || row.len() > 200 { return Err("CSV exceeds 10,000 rows or 200 columns".into()); }
    }
    if quoted { return Err("Unclosed CSV quote".into()); }
    if !cell.is_empty() || !row.is_empty() || closed { row.push(cell); rows.push(row); }
    let columns = rows.first().map_or(0, Vec::len);
    if columns == 0 || rows.iter().any(|r| r.len() != columns) { return Err("CSV rows have different column counts".into()); }
    Ok(table::markdown(&rows))
}
#[cfg(test)]
mod csv_tests {
    use super::*;
    #[test]
    fn quoted_csv_and_tsv_are_tables() {
        assert_eq!(csv("a,b\r\n\"one,two\",\"line\nnext\"\r\n").unwrap(), "| a | b |\n| --- | --- |\n| one,two | line<br>next |\n");
        assert!(csv("a\tb\n1\t2").unwrap().contains("| 1 | 2 |"));
        assert!(csv("a,b\n1").is_err());
        assert!(csv("\"bad").is_err());
    }
}
