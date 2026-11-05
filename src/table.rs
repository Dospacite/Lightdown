//! A bounded HTML table reader. Markup is data; scripts, styles and URLs never run.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}
pub(crate) fn entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|n| *n < 16) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let key = &rest[1..end];
        let ch = match key {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => key
                .strip_prefix("#x")
                .and_then(|s| u32::from_str_radix(s, 16).ok())
                .or_else(|| key.strip_prefix('#').and_then(|s| s.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(c) = ch {
            out.push(c);
        } else {
            out.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}
pub fn parse(html: &str) -> Option<Table> {
    if html.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut in_table = false;
    let mut in_cell = false;
    let mut skip = None;
    let mut truncated = false;
    let mut rest = html;
    while !rest.is_empty() {
        let i = rest.find('<').unwrap_or(rest.len());
        if in_cell && skip.is_none() {
            cell.push_str(&rest[..i]);
        }
        rest = &rest[i..];
        if rest.is_empty() {
            break;
        }
        let end = rest.find('>')?;
        let tag = rest[1..end].trim().to_ascii_lowercase();
        let name = tag.split_whitespace().next().unwrap_or("");
        rest = &rest[end + 1..];
        if matches!(name, "script" | "style") {
            skip = Some(name.to_string());
            continue;
        }
        if let Some(skipped) = &skip {
            if name == format!("/{skipped}") {
                skip = None;
            }
            continue;
        }
        match name {
            "table" => {
                if in_table {
                    return None;
                }
                in_table = true;
            }
            "/table" if in_table => {
                if !row.is_empty() {
                    rows.push(row);
                }
                break;
            }
            "tr" if in_table => {
                if !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
                if rows.len() >= 100 {
                    truncated = true;
                    break;
                }
            }
            "td" | "th" if in_table => {
                in_cell = true;
                cell.clear();
            }
            "/td" | "/th" if in_cell => {
                in_cell = false;
                let decoded = entities(&cell);
                let value = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
                if row.len() < 30 {
                    let short: String = value.chars().take(400).collect();
                    truncated |= short.len() != value.len();
                    row.push(short);
                } else {
                    truncated = true;
                }
            }
            "/tr" if in_table => {
                if !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
            }
            "br" | "br/" if in_cell => cell.push(' '),
            _ => {}
        }
    }
    (!rows.is_empty()).then_some(Table { rows, truncated })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pandas_tables_are_plain_bounded_data() {
        let t=parse("<style>bad</style><table><tr><th></th><th>A</th></tr><tr><th>0</th><td>&lt;b&gt;é &amp; &#128512;<script>bad()</script></td></tr></table>").unwrap();
        assert_eq!(t.rows[1][1], "<b>é & 😀");
        assert!(parse("<div>No table</div>").is_none());
        let t = parse(&format!("<table>{}</table>", "<tr><td>x</td></tr>".repeat(150))).unwrap();
        assert_eq!(t.rows.len(), 100);
        assert!(t.truncated);
    }
}

/// Pipe-table source shared by HTML and CSV paste.
pub fn markdown(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = String::new();
    for (n, row) in rows.iter().enumerate() {
        out.push('|');
        for i in 0..columns {
            let cell = row.get(i).map_or("", String::as_str).replace('\\', "\\\\").replace('|', "\\|").replace(['\n', '\r'], "<br>");
            out += &format!(" {cell} |");
        }
        out.push('\n');
        if n == 0 { out += &format!("|{}\n", " --- |".repeat(columns)); }
    }
    out
}
