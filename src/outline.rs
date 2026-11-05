//! The outline: a document's headings, LaTeX sections or Python classes
//! and functions, listed in the sidebar. A click goes to one; the one the
//! cursor is under is marked.

use std::cell::RefCell;
use std::rc::Rc;

use crate::cells::{self, Tag};
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::pane::Doc;
use crate::ui::px;
use crate::{theme, ui};

/// A heading: how deep it stands, from 0, what it says, and where in the
/// text its line starts.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub depth: usize,
    pub title: String,
    pub at: usize,
}

/// LaTeX's sectioning commands, outermost first.
const SECTIONS: [&str; 7] = ["part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph"];

/// A markdown heading line: its level, from 0, and its text.
fn markdown(line: &str) -> Option<(usize, String)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim();
    (!title.is_empty()).then(|| (hashes - 1, title.to_string()))
}

/// What a group of braces at the start of `s` holds, its own braces kept.
fn braced(s: &str) -> Option<&str> {
    let s = s.strip_prefix('{')?;
    let mut depth = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(&s[..i]),
            '}' => depth -= 1,
            _ => {}
        }
    }
    Some(s)
}

/// A LaTeX sectioning command on a line: its level and title.
fn latex(line: &str) -> Option<(usize, String)> {
    let line = line.trim_start();
    let rest = line.strip_prefix('\\')?;
    let level = SECTIONS
        .iter()
        .position(|name| rest.strip_prefix(name).is_some_and(|after| after.starts_with(['{', '*', '['])))?;
    let mut after = rest[SECTIONS[level].len()..].trim_start_matches('*');
    // A short title in brackets is the one for the table of contents.
    if let Some(close) = after.strip_prefix('[').and_then(|s| s.find(']')) {
        after = &after[close + 2..];
    }
    let title = braced(after.trim_start())?;
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    Some((level, if title.is_empty() { "(untitled)".into() } else { title }))
}

/// A Python class or function: its depth, by indentation, and its name.
fn python(line: &str) -> Option<(usize, String)> {
    let body = line.trim_start();
    let indent = line.len() - body.len();
    let body = body.strip_prefix("async ").unwrap_or(body);
    let (kind, rest) =
        body.strip_prefix("class ").map(|r| ("class", r)).or(body.strip_prefix("def ").map(|r| ("def", r)))?;
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    if name.is_empty() {
        return None;
    }
    let title = if kind == "class" { name } else { format!("{name}()") };
    Some((indent / 4, title))
}

/// Lexical declarations for the languages highlighted by the source editor.
fn code(line: &str) -> Option<(usize, String)> {
    let body = line.trim_start();
    if body.starts_with(['/', '*', '#', '\'', '"']) {
        return None;
    }
    let words: Vec<&str> = body.split_whitespace().collect();
    let declaration = words.iter().position(|w| {
        matches!(*w, "fn" | "func" | "function" | "class" | "struct" | "enum" | "trait" | "interface" | "impl" | "type")
    });
    if let Some(i) = declaration {
        let mut rest = words.get(i + 1).copied()?;
        if words[i] == "func" && rest.starts_with('(') {
            rest = body.split_once(')')?.1.trim();
        }
        let name: String =
            rest.trim_start_matches('*').chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            return None;
        }
        let function = matches!(words[i], "fn" | "func" | "function");
        return Some(((line.len() - body.len()) / 4, if function { format!("{name}()") } else { name }));
    }
    // C-family definitions and JavaScript arrow functions.
    if body.contains("=>")
        && let Some(rest) = body.strip_prefix("const ").or_else(|| body.strip_prefix("let "))
    {
        let name = rest.split([' ', '=']).next()?;
        return Some(((line.len() - body.len()) / 4, format!("{name}()")));
    }
    if body.contains('{')
        && let Some((before, _)) = body.split_once('(')
    {
        let name = before.split_whitespace().last()?.trim_start_matches('*');
        if before.split_whitespace().count() >= 2
            && !matches!(name, "if" | "for" | "while" | "switch" | "catch")
            && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return Some(((line.len() - body.len()) / 4, format!("{name}()")));
        }
    }
    None
}

/// The outline of a document's text.
pub fn items(text: &str, doc: Doc) -> Vec<Item> {
    if matches!(doc, Doc::Code | Doc::Python | Doc::Markdown) && let Some(items) = crate::treesitter::outline(text) { return items; }
    lexical_items(text, doc)
}

/// Lexical fallback for files without an installed or usable grammar.
pub fn lexical_items(text: &str, doc: Doc) -> Vec<Item> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    // In a notebook, what kind of cell the line is in.
    let mut tag = if doc == Doc::Cells { None } else { Some(Tag::Markdown) };
    let mut at = 0;
    for line in text.split('\n') {
        let start = at;
        at += line.len() + 1;
        if doc == Doc::Cells
            && let Some((t, _)) = cells::parse(line)
        {
            (tag, fence) = (Some(t), None);
            continue;
        }
        let kind = match (doc, tag) {
            (Doc::Markdown, _) | (Doc::Cells, Some(Tag::Markdown)) => Doc::Markdown,
            (Doc::Cells, Some(Tag::Code)) => Doc::Python,
            (Doc::Latex | Doc::Python | Doc::Code, _) => doc,
            _ => continue,
        };
        let found = match kind {
            Doc::Markdown => {
                let trimmed = line.trim_start();
                let mark = if trimmed.starts_with("```") {
                    "```"
                } else if trimmed.starts_with("~~~") {
                    "~~~"
                } else {
                    ""
                };
                if !mark.is_empty() {
                    fence = match fence {
                        Some(open) if open == mark => None,
                        None => Some(mark),
                        open => open,
                    };
                    continue;
                }
                if fence.is_some() { None } else { markdown(line) }
            }
            Doc::Code => code(line),
            Doc::Latex => latex(line.split('%').next().unwrap_or("")),
            _ => python(line),
        };
        if let Some((depth, title)) = found {
            out.push(Item { depth, title, at: start });
        }
    }
    // Depths start from the outermost level used.
    let least = out.iter().map(|i| i.depth).min().unwrap_or(0);
    for item in &mut out {
        item.depth -= least;
    }
    out
}

pub struct Outline {
    pub items: Vec<Item>,
    /// The item the cursor is under.
    pub current: Option<usize>,
    scroll: f32,
    fonts: Rc<RefCell<Fonts>>,
    rect: (f32, f32, f32, f32),
    scale: f32,
    pub hover: Option<(f32, f32)>,
    /// The current item is to be scrolled into view.
    reveal: bool,
    /// The PDF whose bookmarks are listed, and how many there were.
    pdf: Option<(std::path::PathBuf, usize)>,
}

impl Outline {
    pub fn new(fonts: Rc<RefCell<Fonts>>) -> Outline {
        Outline {
            items: Vec::new(),
            current: None,
            scroll: 0.0,
            fonts,
            rect: (0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
            hover: None,
            reveal: true,
            pdf: None,
        }
    }

    /// Lists a PDF's bookmarks; an item's `at` is its index among them.
    pub fn show_bookmarks(&mut self, path: &std::path::Path, marks: &[crate::pdf::Bookmark], current: Option<usize>) {
        if self.pdf.as_ref().is_none_or(|(p, n)| p != path || *n != marks.len()) {
            self.items = marks.iter().enumerate().map(|(i, b)| Item { depth: b.depth, title: b.title.clone(), at: i }).collect();
            self.pdf = Some((path.to_path_buf(), marks.len()));
        }
        if current != self.current {
            self.reveal = true;
        }
        self.current = current;
    }

    /// Reads the outline from the text, marking where the cursor is.
    pub fn update(&mut self, text: &str, doc: Doc, cursor: usize) {
        self.pdf = None;
        self.items = items(text, doc);
        let current = self.items.iter().rposition(|i| i.at <= cursor);
        if current != self.current {
            self.reveal = true;
        }
        self.current = current;
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) {
        (self.rect, self.scale) = ((x, y, w, h), scale);
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        rw > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    fn row_h(&self) -> f32 {
        (theme::ROW * self.scale).round()
    }

    /// Where the rows are listed: the top of the list and its height.
    fn span(&self) -> (f32, f32) {
        let (_, y, _, h) = self.rect;
        let top = y + (4.0 * self.scale).round();
        (top, (y + h - top).max(0.0))
    }

    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let (top, tall) = self.span();
        if !self.contains(x, y) || y < top || y >= top + tall {
            return None;
        }
        let i = ((y - top + self.scroll) / self.row_h()) as usize;
        (i < self.items.len()).then_some(i)
    }

    /// Whether a point is on an item.
    pub fn hit(&self, x: f32, y: f32) -> bool {
        self.row_at(x, y).is_some()
    }

    /// A click: where in the text the item under it starts.
    pub fn press(&mut self, x: f32, y: f32) -> Option<usize> {
        let i = self.row_at(x, y)?;
        self.current = Some(i);
        Some(self.items[i].at)
    }

    fn clamp_scroll(&mut self) {
        let (rh, tall) = (self.row_h(), self.span().1);
        let all = self.items.len() as f32 * rh + (rh / 2.0).round();
        self.scroll = self.scroll.min(all - tall).max(0.0);
    }

    pub fn scroll(&mut self, dy: f32) {
        self.scroll += dy;
        self.reveal = false;
        self.clamp_scroll();
    }

    pub fn scroll_lines(&mut self, lines: f32) {
        self.scroll(lines * self.row_h());
    }

    pub fn draw(&mut self, c: &mut Canvas) {
        let (x, y, w, h) = self.rect;
        if w < 1.0 || h < 1.0 {
            return;
        }
        let s = self.scale;
        let size = px(theme::UI_TEXT, s);
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());
        let (top, tall) = self.span();
        let rh = self.row_h();
        if let Some(i) = self.current.filter(|_| std::mem::take(&mut self.reveal)) {
            let at = i as f32 * rh;
            self.scroll = self.scroll.max(at + rh - tall).min(at);
        }
        self.clamp_scroll();
        c.clip(x, top, w, tall);
        let step = px(14.0, s);
        let inset = px(6.0, s);
        let pad = px(12.0, s);
        let hot = self.hover.and_then(|(hx, hy)| self.row_at(hx, hy));
        for (i, item) in self.items.iter().enumerate().skip((self.scroll / rh) as usize) {
            let ry = top + i as f32 * rh - self.scroll;
            if ry >= top + tall {
                break;
            }
            let on = self.current == Some(i);
            let r = (x + inset, ry, (w - inset * 2.0).max(0.0), rh);
            ui::row_ground(c, r, on, hot == Some(i) && !on, false, s);
            let depth = item.depth.min(6);
            for d in 0..depth {
                c.rect((x + pad + d as f32 * step + px(3.0, s)).round(), ry, 1.0, rh, theme::line());
            }
            if on {
                c.round_rect(r.0, ry + px(5.0, s), px(2.0, s), rh - px(10.0, s), px(1.0, s), theme::primary());
            }
            let (face, ink) = match item.depth {
                _ if on => (font::UI_MEDIUM, theme::bright()),
                0 => (font::UI_MEDIUM, theme::fg()),
                _ => (font::UI, theme::dim()),
            };
            let left = x + pad + depth as f32 * step;
            let title = ui::fit(fonts, face, size, &item.title, x + w - pad - left);
            ui::text(c, fonts, left, ui::base(ry, rh, size), &title, face, size, ink);
        }
        if self.items.is_empty() {
            let said = "No headings in this document";
            ui::text(c, fonts, x + pad + px(4.0, s), ui::base(top, rh * 1.5, size), said, font::UI, size, theme::dim());
        }
        c.unclip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(text: &str, doc: Doc) -> Vec<(usize, String)> {
        items(text, doc).into_iter().map(|i| (i.depth, i.title)).collect()
    }

    #[test]
    fn markdown_headings_outside_code() {
        let text = "# Title\n\ntext\n## Part *one* ##\n```\n# not a heading\n```\n#nospace\n### Deep\n";
        assert_eq!(titles(text, Doc::Markdown), [(0, "Title".into()), (1, "Part *one*".into()), (2, "Deep".into())]);
        let at: Vec<usize> = items(text, Doc::Markdown).iter().map(|i| i.at).collect();
        assert_eq!(at, [0, 14, 64]);
    }

    #[test]
    fn latex_sections_and_python_definitions() {
        let tex = "\\section{Intro}\n% \\section{Gone}\n\\subsection*[short]{The {long} one}\n\\sectionmark{x}\n";
        assert_eq!(titles(tex, Doc::Latex), [(0, "Intro".into()), (1, "The {long} one".into())]);
        let py = "import os\nclass A:\n    def f(self):\n        pass\nasync def main():\n    pass\n";
        assert_eq!(titles(py, Doc::Python), [(0, "A".into()), (1, "f()".into()), (0, "main()".into())]);
    }

    #[test]
    fn notebook_cells_give_headings_and_definitions() {
        let md = cells::header(Tag::Markdown, "");
        let code = cells::header(Tag::Code, "");
        let text = format!("{md}\n# Analysis\n{code}\n# a comment\ndef load():\n    pass\n");
        assert_eq!(titles(&text, Doc::Cells), [(0, "Analysis".into()), (0, "load()".into())]);
    }
}
