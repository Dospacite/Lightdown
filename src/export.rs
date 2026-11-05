//! Markdown as a LaTeX document, to be built into a PDF. Each piece of
//! markdown becomes its LaTeX counterpart; mathematics is LaTeX already and
//! goes through as it is written.

use std::collections::HashMap;
use std::path::Path;

use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};

const PREAMBLE: &str = r"\documentclass[11pt]{article}
\usepackage{iftex}
\ifPDFTeX
  \usepackage[utf8]{inputenc}
\else
  \usepackage{fontspec}
\fi
\usepackage[margin=2.5cm]{geometry}
\usepackage{amsmath,amssymb}
\usepackage{graphicx}
\usepackage{xcolor}
\usepackage{listings}
\usepackage[normalem]{ulem}
\usepackage{hyperref}
\hypersetup{colorlinks=true,linkcolor=black,urlcolor=blue!40!black}
\lstset{basicstyle=\ttfamily\small,breaklines=true,columns=fullflexible,keepspaces=true,
  frame=single,framesep=6pt,rulecolor=\color{black!25},xleftmargin=6pt,xrightmargin=6pt}
\setlength{\parskip}{0.6em}
\setlength{\parindent}{0pt}
\newlength\ldimage
\newcommand\image[2]{\setlength\ldimage{#1}\ifdim\ldimage>\linewidth\setlength\ldimage{\linewidth}\fi
  \includegraphics[width=\ldimage]{#2}}
";

/// Text as LaTeX sets it, with its special characters spelt out.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out += r"\textbackslash{}",
            '{' | '}' | '$' | '&' | '#' | '_' | '%' => {
                out.push('\\');
                out.push(ch);
            }
            '^' => out += r"\^{}",
            '~' => out += r"\textasciitilde{}",
            '<' => out += r"\textless{}",
            '>' => out += r"\textgreater{}",
            '|' => out += r"\textbar{}",
            _ => out.push(ch),
        }
    }
    out
}

/// A web address as `\href` reads it, or none if it cannot be written.
fn address(url: &str) -> Option<String> {
    let web = url.contains("://") || url.starts_with("mailto:");
    (web && !url.contains(['{', '}', '\\'])).then(|| url.replace('#', r"\#").replace('%', r"\%"))
}

/// A picture to include, as `\image` takes it: its width and its path. The
/// engines read PNG, JPEG and PDF; other pictures keep their alt text.
fn picture(doc: Option<&Path>, url: &str) -> Option<(String, String)> {
    let path = crate::pane::image_file(doc, url)?;
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let shown = path.to_str()?.to_string();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "pdf")
        || !path.is_file()
        || shown.contains(['%', '#', '{', '}', '\\', '~', '^', '$'])
    {
        return None;
    }
    // A pixel is three quarters of a point, as on the screen.
    let width = crate::pics::probe(&path).map_or(r"\linewidth".to_string(), |(w, _)| format!("{}pt", w * 3 / 4));
    Some((width, shown))
}

#[derive(Default)]
struct Writer {
    /// What is being written; a footnote is written on a sheet of its own,
    /// put on top until it ends.
    out: Vec<String>,
    footnotes: HashMap<String, String>,
    /// For each list that is open: whether it opened an environment, which
    /// LaTeX allows only four deep.
    lists: Vec<bool>,
    /// Where the last `\item` was written, for a task's box to go in.
    item: usize,
    /// For each link open: whether it is one LaTeX can follow.
    links: Vec<bool>,
    /// For each picture open: whether its alt text stands in for it.
    pictures: Vec<bool>,
    code: bool,
    meta: Option<String>,
    head: bool,
    /// Cells written in the row so far.
    cells: usize,
    /// Underlines opened by `<u>` and not yet closed.
    under: usize,
}

impl Writer {
    fn put(&mut self, s: &str) {
        if let Some(top) = self.out.last_mut() {
            *top += s;
        }
    }

    fn close_under(&mut self) {
        let n = std::mem::take(&mut self.under);
        self.put(&"}".repeat(n));
    }

    fn start(&mut self, tag: Tag, doc: Option<&Path>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                let command = ["section", "subsection", "subsubsection", "paragraph", "subparagraph", "subparagraph"];
                self.put(&format!("\\{}*{{", command[level as usize - 1]));
            }
            Tag::BlockQuote(_) => self.put("\\begin{quote}\n"),
            // A fence names its language, but listings knows few of them and
            // fails on the rest.
            Tag::CodeBlock(_) => {
                self.code = true;
                self.put("\\begin{lstlisting}\n");
            }
            Tag::List(first) => {
                let opens = self.lists.iter().filter(|o| **o).count() < 4;
                self.lists.push(opens);
                if opens {
                    let depth = self.lists.iter().filter(|o| **o).count();
                    match first {
                        None => self.put("\\begin{itemize}\n"),
                        Some(n) => {
                            self.put("\\begin{enumerate}\n");
                            if n != 1 {
                                let counter = ["i", "ii", "iii", "iv"][depth - 1];
                                self.put(&format!("\\setcounter{{enum{counter}}}{{{}}}\n", n.saturating_sub(1)));
                            }
                        }
                    }
                }
            }
            Tag::Item => {
                self.item = self.out.last().map_or(0, String::len);
                // The braces keep a bracket that starts the item from being
                // read as its label.
                self.put("\\item{} ");
            }
            Tag::Table(aligns) => {
                let spec: String = aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::Center => 'c',
                        Alignment::Right => 'r',
                        _ => 'l',
                    })
                    .collect();
                self.put(&format!("\\begin{{center}}\n\\begin{{tabular}}{{{spec}}}\n\\hline\n"));
            }
            Tag::TableHead => (self.head, self.cells) = (true, 0),
            Tag::TableRow => self.cells = 0,
            Tag::TableCell => {
                if self.cells > 0 {
                    self.put(" & ");
                }
                self.cells += 1;
                if self.head {
                    self.put("\\textbf{");
                }
            }
            Tag::Emphasis => self.put("\\emph{"),
            Tag::Strong => self.put("\\textbf{"),
            Tag::Strikethrough => self.put("\\sout{"),
            Tag::Superscript => self.put("\\textsuperscript{"),
            Tag::Subscript => self.put("\\textsubscript{"),
            Tag::Link { dest_url, .. } => {
                let href = address(&dest_url);
                if let Some(url) = &href {
                    self.put(&format!("\\href{{{url}}}{{"));
                }
                self.links.push(href.is_some());
            }
            Tag::Image { dest_url, .. } => {
                let pic = picture(doc, &dest_url);
                match &pic {
                    Some((width, path)) => self.put(&format!("\\image{{{width}}}{{{path}}}")),
                    None => self.put("\\emph{"),
                }
                self.pictures.push(pic.is_none());
            }
            Tag::MetadataBlock(_) => self.meta = Some(String::new()),
            Tag::FootnoteDefinition(_)
            | Tag::HtmlBlock
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.close_under();
                self.put("\n\n");
            }
            TagEnd::Heading(_) => {
                self.close_under();
                self.put("}\n\n");
            }
            TagEnd::BlockQuote(_) => self.put("\\end{quote}\n"),
            TagEnd::CodeBlock => {
                self.code = false;
                self.put("\\end{lstlisting}\n");
            }
            TagEnd::List(ordered) => {
                if self.lists.pop() == Some(true) {
                    self.put(if ordered { "\\end{enumerate}\n" } else { "\\end{itemize}\n" });
                }
            }
            TagEnd::Item => {
                self.close_under();
                self.put("\n");
            }
            TagEnd::FootnoteDefinition => {}
            TagEnd::Table => self.put("\\hline\n\\end{tabular}\n\\end{center}\n"),
            TagEnd::TableHead => {
                self.head = false;
                self.put(" \\tabularnewline\n\\hline\n");
            }
            TagEnd::TableRow => self.put(" \\tabularnewline\n"),
            TagEnd::TableCell => {
                self.close_under();
                if self.head {
                    self.put("}");
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Superscript | TagEnd::Subscript => {
                self.put("}")
            }
            TagEnd::Link => {
                if self.links.pop() == Some(true) {
                    self.put("}");
                }
            }
            TagEnd::Image => {
                if self.pictures.pop() == Some(true) {
                    self.put("}");
                }
            }
            TagEnd::MetadataBlock(_) => {}
            TagEnd::HtmlBlock
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
        }
    }

    /// The tags of HTML that mean something here: line breaks and
    /// underlines.
    fn html(&mut self, html: &str) {
        let tag = html.trim().to_ascii_lowercase().replace(' ', "");
        match tag.as_str() {
            "<br>" | "<br/>" => self.put("\\newline\n"),
            "<u>" => {
                self.under += 1;
                self.put("\\uline{");
            }
            "</u>" if self.under > 0 => {
                self.under -= 1;
                self.put("}");
            }
            _ => {}
        }
    }
}

/// The title, author and date that front matter gives, as LaTeX.
fn front(meta: &str) -> String {
    let mut out = String::new();
    for line in meta.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim().trim_matches(['"', '\'']);
        if matches!(key.trim(), "title" | "author" | "date") && !value.is_empty() {
            out += &format!("\\{}{{{}}}\n", key.trim(), escape(value));
        }
    }
    if !out.is_empty() && !out.contains("\\date{") {
        out += "\\date{}\n";
    }
    out
}

/// Markdown as a whole LaTeX document. Pictures are found from `doc`, the
/// markdown's file; `name` titles the PDF.
pub fn latex(markdown: &str, doc: Option<&Path>, name: &str) -> String {
    latex_inner(markdown, doc, name, false)
}

/// Build-only source markers for markdown SyncTeX. Ordinary exports stay plain.
pub fn latex_mapped(markdown: &str, doc: Option<&Path>, name: &str) -> String {
    latex_inner(markdown, doc, name, true)
}

fn latex_inner(markdown: &str, doc: Option<&Path>, name: &str, mapped: bool) -> String {
    // Diagram images and citations are substituted line for line, so
    // markdown SyncTeX markers keep pointing at the source lines.
    let diagrams = crate::diagrams::pdf_source(markdown, &crate::config::cache_dir());
    let markdown = diagrams.as_str();
    let bibliography = doc.map(|p| crate::citations::read(&crate::citations::root(p))).unwrap_or_default();
    let expanded = crate::citations::bibliography(markdown, &bibliography);
    let markdown = expanded.as_str();
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_WIKILINKS;
    let mut w = Writer { out: vec![String::new()], ..Writer::default() };
    let mut title = String::new();
    let starts: Vec<_> = std::iter::once(0).chain(markdown.match_indices('\n').map(|(i,_)| i+1)).collect();
    for (event, range) in Parser::new_ext(markdown, opts).into_offset_iter() {
        let block = matches!(&event, Event::Start(Tag::Paragraph | Tag::Heading {..} | Tag::Item | Tag::CodeBlock(_)));
        let text_line = matches!(&event, Event::Text(_)) && w.out[0].ends_with('\n');
        if mapped && w.out.len() == 1 && !w.code && w.meta.is_none() && (block || text_line) {
            let line = starts.partition_point(|start| *start <= range.start);
            if !w.out[0].is_empty() && !w.out[0].ends_with('\n') { w.put("\n"); }
            w.put(&format!("%lightdown-source:{line}\n"));
        }
        match event {
            Event::Start(Tag::FootnoteDefinition(label)) => {
                w.out.push(format!("\u{0}{label}\u{0}"));
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                let sheet = w.out.pop().unwrap_or_default();
                let (label, body) = sheet[1..].split_once('\u{0}').unwrap_or(("", ""));
                w.footnotes.insert(label.to_string(), body.trim().to_string());
            }
            Event::Start(tag) => w.start(tag, doc),
            Event::End(TagEnd::MetadataBlock(_)) => title = front(&w.meta.take().unwrap_or_default()),
            Event::End(tag) => w.end(tag),
            Event::Text(text) if w.meta.is_some() => *w.meta.as_mut().unwrap() += &text,
            Event::Text(_) if w.pictures.last() == Some(&false) => {}
            Event::Text(text) if w.code => w.put(&text.replace("\\end{lstlisting}", "\\end {lstlisting}")),
            Event::Text(text) => w.put(&escape(&text)),
            Event::Code(code) => w.put(&format!("\\texttt{{{}}}", escape(&code))),
            Event::InlineMath(math) => w.put(&format!("${math}$")),
            Event::DisplayMath(math) => w.put(&format!("\\[{math}\\]")),
            Event::Html(html) | Event::InlineHtml(html) => w.html(&html),
            Event::FootnoteReference(label) => w.put(&format!("\u{1}{label}\u{1}")),
            Event::SoftBreak => w.put("\n"),
            Event::HardBreak => w.put("\\newline\n"),
            Event::Rule => w.put("\\noindent\\rule{\\linewidth}{0.4pt}\n\n"),
            Event::TaskListMarker(done) => {
                let mark = if done { "\\item[$\\boxtimes$] " } else { "\\item[$\\square$] " };
                let at = w.item;
                if let Some(top) = w.out.last_mut().filter(|t| t[at..].starts_with("\\item{} ")) {
                    top.replace_range(at..at + 8, mark);
                }
            }
        }
    }
    let mut body = w.out.swap_remove(0);
    // Footnotes are written where they are referred to.
    while let Some(at) = body.find('\u{1}') {
        let end = body[at + 1..].find('\u{1}').map_or(body.len(), |n| at + 2 + n);
        let label = body[at + 1..end.saturating_sub(1).max(at + 1)].to_string();
        let note = w.footnotes.get(&label).map_or(String::new(), |n| format!("\\footnote{{{n}}}"));
        body.replace_range(at..end, &note);
    }
    let pdf_title = format!("\\hypersetup{{pdftitle={{{}}}}}\n", escape(name));
    let make = if title.is_empty() { "" } else { "\\maketitle\n\n" };
    format!("{PREAMBLE}{pdf_title}{title}\\begin{{document}}\n{make}{}\n\\end{{document}}\n", body.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(md: &str) -> String {
        let doc = latex(md, None, "t");
        let s = doc.find("\\begin{document}\n").unwrap() + 17;
        doc[s..doc.rfind("\n\\end{document}").unwrap()].to_string()
    }

    #[test]
    fn prose_becomes_latex() {
        assert_eq!(
            body("# Top\n\nSome *very* **bold** ~~gone~~ `x_1` 50% & $a^2$."),
            "\\section*{Top}\n\n\
            Some \\emph{very} \\textbf{bold} \\sout{gone} \\texttt{x\\_1} 50\\% \\& $a^2$."
        );
        assert_eq!(body("a<u>b</u> <u>c\n\nd"), "a\\uline{b} \\uline{c}\n\nd");
        assert_eq!(body("[site](https://a.b/#x) and [file](notes.md)"), "\\href{https://a.b/\\#x}{site} and file");
        assert_eq!(body("![alt](missing.png)"), "\\emph{alt}");
        assert_eq!(body("$$\nx\n$$"), "\\[\nx\n\\]");
    }

    #[test]
    fn blocks_become_environments() {
        let lists = body("- [ ] a\n- [x] b\n- [c] d\n\n3. three\n4. four");
        assert_eq!(
            lists,
            "\\begin{itemize}\n\\item[$\\square$] a\n\\item[$\\boxtimes$] b\n\\item{} [c] d\n\\end{itemize}\n\
            \\begin{enumerate}\n\\setcounter{enumi}{2}\n\\item{} three\n\\item{} four\n\\end{enumerate}"
        );
        let table = body("| a | b |\n|:-:|--:|\n| 1 | 2 |");
        assert_eq!(
            table,
            "\\begin{center}\n\\begin{tabular}{cr}\n\\hline\n\\textbf{a} & \\textbf{b} \\tabularnewline\n\
            \\hline\n1 & 2 \\tabularnewline\n\\hline\n\\end{tabular}\n\\end{center}"
        );
        assert_eq!(
            body("```rust\nfn x() {}\n```\n\n> q"),
            "\\begin{lstlisting}\nfn x() {}\n\\end{lstlisting}\n\\begin{quote}\nq\n\n\\end{quote}"
        );
        assert_eq!(body("Note[^1].\n\n[^1]: Said *here*."), "Note\\footnote{Said \\emph{here}.}.");
        let deep = body("- a\n  - b\n    - c\n      - d\n        - e");
        assert_eq!(deep.matches("\\begin{itemize}").count(), 4);
        assert_eq!(deep.matches("\\end{itemize}").count(), 4);
        // Front matter gives the title.
        let doc = latex("---\ntitle: \"My notes\"\nauthor: Me\n---\n\nText", None, "notes");
        assert!(doc.contains("\\title{My notes}\n\\author{Me}\n\\date{}\n\\begin{document}\n\\maketitle"));
    }
}
