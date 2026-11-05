//! Word occurrences and source text transformations.
use super::*;

pub(super) fn occurrences(text: &str, at: usize) -> Vec<(usize, usize)> {
    let (a, b) = editor::word_at(text, at);
    let word = &text[a..b];
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    if word.is_empty() || word.len() > 128 || !word.chars().all(ident) {
        return Vec::new();
    }
    text.match_indices(word)
        .filter(|&(p, _)| {
            text[..p].chars().next_back().is_none_or(|c| !ident(c))
                && text[p + word.len()..].chars().next().is_none_or(|c| !ident(c))
        })
        .map(|(p, s)| (p, p + s.len()))
        .take(20_000)
        .collect()
}
fn case_words(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut words = Vec::new(); let mut word = String::new();
    for (i,&c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !word.is_empty() { words.push(std::mem::take(&mut word)); }
            continue;
        }
        let boundary = c.is_uppercase() && i > 0 &&
            (chars[i-1].is_lowercase() || chars[i-1].is_numeric() ||
             (chars[i-1].is_uppercase() && chars.get(i+1).is_some_and(|c| c.is_lowercase())));
        if boundary && !word.is_empty() { words.push(std::mem::take(&mut word)); }
        word.push(c);
    }
    if !word.is_empty() { words.push(word); }
    words
}
fn convert_case(text:&str,kind:&str) -> String {
    let words = case_words(text);
    let title = |s:&str| { let lower = s.to_lowercase(); let mut chars = lower.chars();
        chars.next().map_or(String::new(),|c| c.to_uppercase().collect::<String>()+chars.as_str()) };
    match kind {
        "snake" => words.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>().join("_"),
        "kebab" => words.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>().join("-"),
        "camel" => words.iter().enumerate().map(|(i,s)| if i == 0 {s.to_lowercase()} else {title(s)}).collect(),
        "pascal" => words.iter().map(|s| title(s)).collect(),
        "title" => words.iter().map(|s| title(s)).collect::<Vec<_>>().join(" "),
        _ => text.into(),
    }
}
impl Pane {
    pub(super) fn occurrence_marks(&mut self) -> &[(usize, usize)] {
        let word = editor::word_at(&self.buf.text, self.buf.cur);
        let key = (self.buf.version, word);
        if self.occurrence_key != Some(key) {
            self.occurrence_key = Some(key);
            self.occurrences = occurrences(&self.buf.text, self.buf.cur);
        }
        &self.occurrences
    }
    pub(crate) fn text_tool(&mut self, action: &str) {
        if self.read_only {
            self.note = Some("This document is read-only".into());
            return;
        }
        let (mut a, mut b) = self.buf.sel();
        if matches!(action, "upper" | "lower" | "snake" | "camel" | "pascal" | "kebab" | "title") {
            if a == b {
                (a, b) = editor::word_at(&self.buf.text, a);
            }
        } else if a != b {
            (a, b) = self.selected_lines();
        } else if action == "join" {
            let l = self.buf.line_of(a);
            (a, b) = (self.buf.starts[l], self.buf.line((l + 1).min(self.buf.lines() - 1)).1);
        } else if self.doc == Doc::Cells {
            (a, b) = self.cell_body(self.cell_at(a));
        } else {
            (a, b) = (0, self.buf.text.len());
        }
        if self.doc == Doc::Cells {
            let (s, e) = self.cell_body(self.cell_at(a));
            if a < s || b > e {
                self.note = Some("Select text within one cell".into());
                return;
            }
        }
        let source = &self.buf.text[a..b];
        let text = match action {
            "snake" | "camel" | "pascal" | "kebab" | "title" => convert_case(source,action),
            "upper" => source.to_uppercase(),
            "lower" => source.to_lowercase(),
            "join" => source.lines().map(str::trim).collect::<Vec<_>>().join(" "),
            "sort" | "unique" => {
                let trailing = source.ends_with('\n');
                let mut lines: Vec<_> = source.split_terminator('\n').collect();
                if action == "sort" {
                    lines.sort();
                } else {
                    let mut seen = std::collections::HashSet::new();
                    lines.retain(|l| seen.insert(*l));
                }
                let mut out = lines.join("\n");
                if trailing {
                    out.push('\n');
                }
                out
            }
            _ => return,
        };
        if text != source && self.edit(a, b, &text, EditKind::Other) {
            self.select(a, a + text.len());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cases_split_acronyms_separators_unicode_and_keep_one_undo_step() {
        for (kind,out) in [("snake","xml_http_café42_name"),("camel","xmlHttpCafé42Name"),("pascal","XmlHttpCafé42Name"),("kebab","xml-http-café42-name"),("title","Xml Http Café42 Name")] {
            assert_eq!(convert_case("XMLHttp café42_name",kind),out);
        }
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())),Rc::new(RefCell::new(Pics::new())));
        p.set_text("other someHTTPValue".into(),Doc::Code); p.select(10,10); p.text_tool("snake");
        assert_eq!(p.buf.text,"other some_http_value");
        p.key(Key::Char('z'),Mods {ctrl:true,..Default::default()},None); assert_eq!(p.buf.text,"other someHTTPValue");
    }
    #[test]
    fn unicode_occurrences_use_word_boundaries() {
        assert_eq!(occurrences("café caféteria café", 2), vec![(0, 5), (17, 22)]);
        assert!(occurrences(" + ", 1).is_empty());
    }
    #[test]
    fn text_tools_preserve_newline_and_undo_unicode() {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text("β\na\nβ\n".into(), Doc::Code);
        p.text_tool("unique");
        assert_eq!(p.buf.text, "β\na\n");
        p.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(p.buf.text, "β\na\nβ\n");
        p.select(0, 2);
        p.text_tool("upper");
        assert!(p.buf.text.starts_with('Β'));
    }
}
