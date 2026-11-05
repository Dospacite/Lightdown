//! Markdown analysis. The source text stays the document; this module only
//! describes it: a kind and container prefix per line, styled inline spans,
//! and the syntax markers that are hidden until the cursor reveals them.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Style bits of a character: markdown emphasis and syntax colour roles.
pub type Attr = u32;

pub const BOLD: Attr = 1;
pub const ITALIC: Attr = 1 << 1;
pub const STRIKE: Attr = 1 << 2;
pub const UNDER: Attr = 1 << 3;
pub const CODE: Attr = 1 << 4;
pub const LINK: Attr = 1 << 5;
pub const MARK: Attr = 1 << 6;
pub const DIM: Attr = 1 << 7;
pub const HIDDEN: Attr = 1 << 8;
/// String literals and mathematics.
pub const SOFT: Attr = 1 << 9;
pub const KEYWORD: Attr = 1 << 10;
pub const NUMBER: Attr = 1 << 11;
pub const FUNCTION: Attr = 1 << 12;
pub const SPECIAL: Attr = 1 << 13;
pub const PARAMETER: Attr = 1 << 14;
pub const PROPERTY: Attr = 1 << 15;
/// Text an edit prediction would delete.
pub const REMOVED: Attr = 1 << 16;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Text,
    Heading(u8),
    /// The `===` / `---` line under a setext heading.
    Underline,
    Code,
    FenceOpen,
    FenceClose,
    Meta,
    Table,
    Rule,
    Html,
    /// The header line of a cell, drawn as a divider.
    Cell,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PrefixKind {
    Quote,
    Bullet,
    /// Ordered list marker; the range is the marker text itself.
    Number(u32, u32),
    Task(bool),
    /// Continuation line of a list item; the range is that item's marker.
    Indent(u32, u32, bool),
}

/// A container marker at the start of a line (`>`, `-`, `1.`, `[ ]`, or the
/// indentation that continues one).
#[derive(Clone, Copy, Debug)]
pub struct Prefix {
    pub start: u32,
    pub end: u32,
    pub kind: PrefixKind,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub kind: Kind,
    pub prefix: Vec<Prefix>,
    /// Offset where the line's own content begins, after any prefix.
    pub content: u32,
    /// Multi-line block this line belongs to, if any.
    pub block: Option<u32>,
    /// Shared first-strong direction for a markdown paragraph containing RTL.
    pub bidi_base: Option<bool>,
}

/// A run of text sharing one set of style bits.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub attr: Attr,
}

/// Syntax that is hidden unless the cursor is within `reveal` (inclusive).
#[derive(Clone, Copy, Debug)]
pub struct Marker {
    pub start: u32,
    pub end: u32,
    pub reveal: (u32, u32),
}

#[derive(Clone, Debug)]
pub struct Link {
    pub start: u32,
    pub end: u32,
    pub url: String,
}

/// A group of lines that switch between rendered and source form together.
#[derive(Clone, Debug)]
pub struct Block {
    pub start: u32,
    pub end: u32,
    pub table: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    /// Cell content ranges, relative to the start of the table block.
    pub rows: Vec<Vec<(u32, u32)>>,
    /// Per column: 0 left, 1 center, 2 right.
    pub aligns: Vec<u8>,
}

/// A formula: the range of its whole source, delimiters and all, its
/// TeX, and whether it is displayed (`$$`) rather than inline (`$`).
#[derive(Clone, Debug)]
pub struct Math {
    pub start: u32,
    pub end: u32,
    pub tex: String,
    pub display: bool,
}

#[derive(Default)]
pub struct Md {
    pub lines: Vec<Line>,
    pub spans: Vec<Span>,
    pub markers: Vec<Marker>,
    pub links: Vec<Link>,
    /// Images, by the range of their whole `![alt](source)`.
    pub images: Vec<Link>,
    pub blocks: Vec<Block>,
    pub tables: Vec<Table>,
    /// The formulas, in order.
    pub maths: Vec<Math>,
}

enum Open {
    Inline,
    Link(String),
    Image(String),
    Heading(u8),
    Paragraph,
    Code(bool),
    Table,
    Meta,
    Html,
    Cell,
    Other,
}

struct Frame {
    start: usize,
    end: usize,
    first: Option<usize>,
    last: usize,
    saved: Attr,
    open: Open,
}

enum ContKind {
    Quote,
    Item(bool),
}

struct Cont {
    start: usize,
    end: usize,
    kind: ContKind,
    indent: usize,
    marker: (u32, u32),
}

/// Inline HTML tags that map onto a style, e.g. `<u>` for underline.
fn html_style(tag: &str) -> Option<Attr> {
    Some(match tag {
        "u" | "ins" => UNDER,
        "b" | "strong" => BOLD,
        "i" | "em" => ITALIC,
        "s" | "del" | "strike" => STRIKE,
        "mark" => MARK,
        "code" | "kbd" => CODE,
        _ => return None,
    })
}

pub fn parse(text: &str, starts: &[usize]) -> Md {
    let b = text.as_bytes();
    let n = starts.len();
    let line_of = |off: usize| starts.partition_point(|&s| s <= off) - 1;
    let line_end = |l: usize| if l + 1 < n { starts[l + 1] - 1 } else { text.len() };
    let trim_nl = |start: usize, mut end: usize| {
        while end > start && b[end - 1] == b'\n' {
            end -= 1;
        }
        end
    };

    let mut md =
        Md { lines: vec![Line { kind: Kind::Text, prefix: Vec::new(), content: 0, block: None, bidi_base: None }; n], ..Md::default() };
    let mut conts: Vec<Cont> = Vec::new();
    let mut tasks: Vec<(usize, bool)> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut lists: Vec<bool> = Vec::new();
    // Open inline HTML tags: name, attr to restore, index of the opening marker.
    let mut html: Vec<(String, Attr, usize)> = Vec::new();
    let mut attr = 0 as Attr;

    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_WIKILINKS;

    for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
        // Text between the previous sibling and this event is syntax.
        let gap = stack.last().map(|f| f.last).unwrap_or(0);
        if let Some(f) = stack.last_mut() {
            f.first.get_or_insert(r.start);
            if !matches!(ev, Event::Start(_) | Event::End(_)) {
                f.last = f.last.max(r.end);
            }
        }
        let raw = matches!(stack.last().map(|f| &f.open), Some(Open::Code(_) | Open::Meta | Open::Html));
        match ev {
            Event::Start(tag) => {
                let saved = attr;
                let open = match tag {
                    Tag::Emphasis => {
                        attr |= ITALIC;
                        Open::Inline
                    }
                    Tag::Strong => {
                        attr |= BOLD;
                        Open::Inline
                    }
                    Tag::Strikethrough => {
                        attr |= STRIKE;
                        Open::Inline
                    }
                    Tag::Link { dest_url, .. } => {
                        attr |= LINK;
                        Open::Link(dest_url.into_string())
                    }
                    Tag::Image { dest_url, .. } => {
                        attr |= LINK | ITALIC | DIM;
                        Open::Image(dest_url.into_string())
                    }
                    Tag::Paragraph => Open::Paragraph,
                    Tag::Heading { level, .. } => Open::Heading(level as u8),
                    Tag::CodeBlock(kind) => Open::Code(matches!(kind, CodeBlockKind::Fenced(_))),
                    Tag::MetadataBlock(_) => Open::Meta,
                    Tag::HtmlBlock => Open::Html,
                    Tag::BlockQuote(_) => {
                        conts.push(Cont {
                            start: r.start,
                            end: r.end,
                            kind: ContKind::Quote,
                            indent: 0,
                            marker: (0, 0),
                        });
                        Open::Other
                    }
                    Tag::List(first) => {
                        lists.push(first.is_some());
                        Open::Other
                    }
                    Tag::Item => {
                        let ordered = lists.last().copied().unwrap_or(false);
                        conts.push(Cont {
                            start: r.start,
                            end: r.end,
                            kind: ContKind::Item(ordered),
                            indent: 0,
                            marker: (0, 0),
                        });
                        Open::Other
                    }
                    Tag::Table(aligns) => {
                        let aligns = aligns
                            .iter()
                            .map(|a| match a {
                                pulldown_cmark::Alignment::Center => 1,
                                pulldown_cmark::Alignment::Right => 2,
                                _ => 0,
                            })
                            .collect();
                        md.tables.push(Table { rows: Vec::new(), aligns });
                        Open::Table
                    }
                    Tag::TableHead | Tag::TableRow => {
                        if let Some(t) = md.tables.last_mut() {
                            t.rows.push(Vec::new());
                        }
                        Open::Other
                    }
                    Tag::TableCell => Open::Cell,
                    _ => Open::Other,
                };
                stack.push(Frame { start: r.start, end: r.end, first: None, last: r.start, saved, open });
            }
            Event::End(tag) => {
                if matches!(tag, TagEnd::List(_)) {
                    lists.pop();
                }
                let Some(f) = stack.pop() else { continue };
                if let Some(p) = stack.last_mut() {
                    p.last = p.last.max(f.end);
                }
                let (start, end) = (f.start, f.end);
                match f.open {
                    Open::Inline | Open::Link(_) | Open::Image(_) => {
                        attr = f.saved;
                        if let Some(first) = f.first {
                            let reveal = (start as u32, end as u32);
                            md.markers.push(Marker { start: start as u32, end: first as u32, reveal });
                            md.markers.push(Marker { start: f.last as u32, end: end as u32, reveal });
                        }
                        let image = matches!(f.open, Open::Image(_));
                        if let Open::Link(url) | Open::Image(url) = f.open {
                            if image {
                                md.images.push(Link { start: start as u32, end: end as u32, url: url.clone() });
                            }
                            md.links.push(Link { start: start as u32, end: end as u32, url });
                        }
                    }
                    Open::Paragraph => {
                        if crate::bidi::needed(&text[start..end]) {
                            let rtl = crate::bidi::first_strong(&text[start..end]).unwrap_or(false);
                            for line in line_of(start)..=line_of(end.saturating_sub(1).max(start)) {
                                md.lines[line].bidi_base = Some(rtl);
                            }
                        }
                        close_html(&mut html, &mut md.markers, &mut attr);
                    }
                    Open::Heading(level) => {
                        let end = trim_nl(start, end);
                        let l0 = line_of(start);
                        let last = f.first.map(|first| f.last.max(first).min(end));
                        let lc = last.map(line_of).unwrap_or(l0);
                        let l1 = line_of(end.saturating_sub(1).max(start));
                        for l in l0..=lc {
                            md.lines[l].kind = Kind::Heading(level);
                        }
                        if l1 > lc {
                            let block = md.blocks.len() as u32;
                            md.blocks.push(Block { start: starts[l0] as u32, end: end as u32, table: None });
                            for l in l0..=l1 {
                                md.lines[l].block = Some(block);
                                if l > lc {
                                    md.lines[l].kind = Kind::Underline;
                                }
                            }
                        } else if let (Some(first), Some(last)) = (f.first, last) {
                            let reveal = (starts[l0] as u32, line_end(l0) as u32);
                            md.markers.push(Marker { start: start as u32, end: first as u32, reveal });
                            md.markers.push(Marker { start: last as u32, end: line_end(l0) as u32, reveal });
                        }
                    }
                    Open::Code(fenced) => {
                        let end = trim_nl(start, end);
                        let l0 = line_of(start);
                        let l1 = line_of(end.saturating_sub(1).max(start));
                        let block = md.blocks.len() as u32;
                        md.blocks.push(Block { start: starts[l0] as u32, end: line_end(l1) as u32, table: None });
                        for l in l0..=l1 {
                            md.lines[l].kind = Kind::Code;
                            md.lines[l].block = Some(block);
                        }
                        if fenced {
                            md.lines[l0].kind = Kind::FenceOpen;
                            let last = text[starts[l1]..line_end(l1)].trim_start_matches([' ', '\t', '>']).trim();
                            let closes =
                                last.len() >= 3 && (last.bytes().all(|c| c == b'`') || last.bytes().all(|c| c == b'~'));
                            if l1 > l0 && closes {
                                md.lines[l1].kind = Kind::FenceClose;
                            }
                            if l1 > l0 {
                                let fence = text[start..line_end(l0)].trim_start_matches(['`', '~']);
                                let code_start = starts[l0 + 1];
                                let code_end = if closes { starts[l1] } else { end };
                                crate::syntax::spans(
                                    &text[code_start..code_end],
                                    code_start,
                                    fence.trim(),
                                    &mut md.spans,
                                );
                            }
                        }
                    }
                    Open::Meta => {
                        let end = trim_nl(start, end);
                        let l0 = line_of(start);
                        let l1 = line_of(end.saturating_sub(1).max(start));
                        let block = md.blocks.len() as u32;
                        md.blocks.push(Block { start: starts[l0] as u32, end: line_end(l1) as u32, table: None });
                        for l in l0..=l1 {
                            md.lines[l].kind = Kind::Meta;
                            md.lines[l].block = Some(block);
                        }
                        md.lines[l0].kind = Kind::FenceOpen;
                        if l1 > l0 {
                            md.lines[l1].kind = Kind::FenceClose;
                        }
                    }
                    Open::Html => {
                        let end = trim_nl(start, end);
                        for l in line_of(start)..=line_of(end.saturating_sub(1).max(start)) {
                            md.lines[l].kind = Kind::Html;
                        }
                    }
                    Open::Table => {
                        let end = trim_nl(start, end);
                        let l0 = line_of(start);
                        let l1 = line_of(end.saturating_sub(1).max(start));
                        let base = starts[l0] as u32;
                        let table = md.tables.len() as u32 - 1;
                        for row in &mut md.tables[table as usize].rows {
                            for c in row {
                                *c = (c.0 - base, c.1 - base);
                            }
                        }
                        let block = md.blocks.len() as u32;
                        md.blocks.push(Block { start: base, end: line_end(l1) as u32, table: Some(table) });
                        for l in l0..=l1 {
                            md.lines[l].kind = Kind::Table;
                            md.lines[l].block = Some(block);
                        }
                    }
                    Open::Cell => {
                        let cell = &text[start..end];
                        let s = start + (cell.len() - cell.trim_start().len());
                        let e = (start + cell.trim_end().len()).max(s);
                        if let Some(row) = md.tables.last_mut().and_then(|t| t.rows.last_mut()) {
                            row.push((s as u32, e as u32));
                        }
                        close_html(&mut html, &mut md.markers, &mut attr);
                    }
                    Open::Other => close_html(&mut html, &mut md.markers, &mut attr),
                }
            }
            Event::Text(_) if raw => {}
            Event::Text(_) => {
                // A backslash escape leaves its backslash outside the text.
                if r.start > gap && b[r.start - 1] == b'\\' {
                    let ch = text[r.start..].chars().next().map_or(0, char::len_utf8);
                    md.markers.push(Marker {
                        start: r.start as u32 - 1,
                        end: r.start as u32,
                        reveal: (r.start as u32 - 1, (r.start + ch) as u32),
                    });
                }
                if r.end > r.start && attr != 0 {
                    md.spans.push(Span { start: r.start as u32, end: r.end as u32, attr });
                }
            }
            Event::Code(_) => {
                let s = &text[r.clone()];
                let ticks = s.len() - s.trim_start_matches('`').len();
                if s.len() > ticks * 2 {
                    let reveal = (r.start as u32, r.end as u32);
                    let (a, z) = ((r.start + ticks) as u32, (r.end - ticks) as u32);
                    md.markers.push(Marker { start: r.start as u32, end: a, reveal });
                    md.markers.push(Marker { start: z, end: r.end as u32, reveal });
                    md.spans.push(Span { start: a, end: z, attr: attr | CODE });
                }
            }
            Event::InlineMath(ref tex) | Event::DisplayMath(ref tex) => {
                md.spans.push(Span { start: r.start as u32, end: r.end as u32, attr: attr | CODE });
                let display = matches!(ev, Event::DisplayMath(_));
                md.maths.push(Math { start: r.start as u32, end: r.end as u32, tex: tex.to_string(), display });
            }
            Event::FootnoteReference(_) => {
                md.spans.push(Span { start: r.start as u32, end: r.end as u32, attr: attr | DIM });
            }
            Event::InlineHtml(_) => {
                let s = text[r.clone()].trim_matches(['<', '>']).trim().to_ascii_lowercase();
                let closing = s.starts_with('/');
                let name = s.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_string();
                match html_style(&name) {
                    Some(style) if !closing => {
                        html.push((name, attr, md.markers.len()));
                        md.markers.push(Marker { start: r.start as u32, end: r.end as u32, reveal: (0, 0) });
                        attr |= style;
                    }
                    Some(_) if html.last().is_some_and(|h| h.0 == name) => {
                        let (_, saved, open) = html.pop().unwrap();
                        attr = saved;
                        let reveal = (md.markers[open].start, r.end as u32);
                        md.markers[open].reveal = reveal;
                        md.markers.push(Marker { start: r.start as u32, end: r.end as u32, reveal });
                    }
                    _ => md.spans.push(Span { start: r.start as u32, end: r.end as u32, attr: attr | DIM }),
                }
            }
            Event::TaskListMarker(done) => tasks.push((r.start, done)),
            Event::Rule => md.lines[line_of(r.start)].kind = Kind::Rule,
            _ => {}
        }
    }
    close_html(&mut html, &mut md.markers, &mut attr);

    md.markers.retain(|m| m.end > m.start);
    md.markers.sort_by_key(|m| m.start);
    md.spans.sort_by_key(|s| s.start);
    md.images.sort_by_key(|i| i.start);
    prefixes(&mut md, text, starts, &mut conts, &tasks);
    md
}

/// Unclosed inline HTML tags must not hide anything: drop their markers.
fn close_html(html: &mut Vec<(String, Attr, usize)>, markers: &mut [Marker], attr: &mut Attr) {
    for (_, saved, open) in html.drain(..).rev() {
        markers[open].end = markers[open].start;
        *attr = saved;
    }
}

/// Walks the lines once with the stack of containers covering each, consuming
/// the quote and list markers at the start of every line.
fn prefixes(md: &mut Md, text: &str, starts: &[usize], conts: &mut [Cont], tasks: &[(usize, bool)]) {
    let b = text.as_bytes();
    let n = starts.len();
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0;
    for l in 0..n {
        let ls = starts[l];
        let le = if l + 1 < n { starts[l + 1] - 1 } else { text.len() };
        while stack.last().is_some_and(|&c| conts[c].end <= ls) {
            stack.pop();
        }
        while next < conts.len() && conts[next].start <= le {
            if conts[next].end > ls {
                stack.push(next);
            }
            next += 1;
        }
        md.lines[l].content = ls as u32;
        if stack.is_empty() {
            continue;
        }
        let blank = text[ls..le].trim().is_empty();
        let mut pos = ls;
        let mut prefix = Vec::new();
        let mut push = |s: usize, e: usize, kind| prefix.push(Prefix { start: s as u32, end: e as u32, kind });
        for &c in &stack {
            match conts[c].kind {
                ContKind::Quote => {
                    let mut p = pos;
                    while p < le && b[p] == b' ' && p - pos < 3 {
                        p += 1;
                    }
                    if p < le && b[p] == b'>' {
                        p += 1;
                        if p < le && b[p] == b' ' {
                            p += 1;
                        }
                        push(pos, p, PrefixKind::Quote);
                        pos = p;
                    } else if !blank {
                        push(pos, pos, PrefixKind::Quote);
                    }
                }
                ContKind::Item(_) if blank => {}
                ContKind::Item(ordered) if conts[c].start >= ls => {
                    let mut p = pos.max(conts[c].start);
                    while p < le && matches!(b[p], b' ' | b'\t') {
                        p += 1;
                    }
                    let m0 = p;
                    if ordered {
                        while p < le && b[p].is_ascii_digit() {
                            p += 1;
                        }
                    }
                    if p < le {
                        p += 1;
                    }
                    let m1 = p;
                    while p < le && b[p] == b' ' && p - m1 < 4 {
                        p += 1;
                    }
                    conts[c].indent = p - pos;
                    conts[c].marker = (m0 as u32, m1 as u32);
                    let kind = if ordered { PrefixKind::Number(m0 as u32, m1 as u32) } else { PrefixKind::Bullet };
                    push(pos, p, kind);
                    pos = p;
                    if let Ok(i) = tasks.binary_search_by_key(&p, |t| t.0) {
                        let mut q = (p + 3).min(le);
                        if q < le && b[q] == b' ' {
                            q += 1;
                        }
                        push(p, q, PrefixKind::Task(tasks[i].1));
                        pos = q;
                    }
                }
                ContKind::Item(ordered) => {
                    let mut p = pos;
                    while p < le && p - pos < conts[c].indent && matches!(b[p], b' ' | b'\t') {
                        p += 1;
                    }
                    let (m0, m1) = conts[c].marker;
                    push(pos, p, PrefixKind::Indent(m0, m1, ordered));
                    pos = p;
                }
            }
        }
        md.lines[l].prefix = prefix;
        md.lines[l].content = pos as u32;
    }
}

impl Md {
    /// An analysis that says nothing: every line is plain text.
    pub fn plain(starts: &[usize]) -> Md {
        let line = |&s: &usize| Line { kind: Kind::Text, prefix: Vec::new(), content: s as u32, block: None, bidi_base: None };
        Md { lines: starts.iter().map(line).collect(), ..Md::default() }
    }

    /// Lays the analysis of a stretch of the document over this one. `part`
    /// was parsed from the text starting at byte `off`, which is the start
    /// of line `line`. Parts must be added in document order.
    pub fn overlay(&mut self, part: Md, off: usize, line: usize) {
        let o = off as u32;
        let (blocks, tables) = (self.blocks.len() as u32, self.tables.len() as u32);
        for (i, mut l) in part.lines.into_iter().enumerate() {
            for p in &mut l.prefix {
                (p.start, p.end) = (p.start + o, p.end + o);
                if let PrefixKind::Number(a, b) | PrefixKind::Indent(a, b, _) = &mut p.kind {
                    (*a, *b) = (*a + o, *b + o);
                }
            }
            l.content += o;
            l.block = l.block.map(|b| b + blocks);
            self.lines[line + i] = l;
        }
        self.spans.extend(part.spans.iter().map(|s| Span { start: s.start + o, end: s.end + o, attr: s.attr }));
        self.markers.extend(part.markers.iter().map(|m| Marker {
            start: m.start + o,
            end: m.end + o,
            reveal: (m.reveal.0 + o, m.reveal.1 + o),
        }));
        let moved = |l: Link| Link { start: l.start + o, end: l.end + o, url: l.url };
        self.links.extend(part.links.into_iter().map(moved));
        self.images.extend(part.images.into_iter().map(moved));
        self.maths.extend(part.maths.into_iter().map(|m| Math { start: m.start + o, end: m.end + o, ..m }));
        self.blocks.extend(part.blocks.into_iter().map(|b| Block {
            start: b.start + o,
            end: b.end + o,
            table: b.table.map(|t| t + tables),
        }));
        self.tables.extend(part.tables);
    }

    /// Fills `out` with one attribute word per byte of `start..end`.
    /// Markers are hidden unless `focus` falls in their reveal range.
    pub fn attrs(&self, start: usize, end: usize, focus: usize, reveal_all: bool, out: &mut Vec<Attr>) {
        out.clear();
        out.resize(end - start, 0);
        let (s, e, focus) = (start as u32, end as u32, focus as u32);
        let from = self.spans.partition_point(|sp| sp.end <= s);
        for sp in self.spans[from..].iter().take_while(|sp| sp.start < e) {
            for a in &mut out[(sp.start.max(s) - s) as usize..(sp.end.min(e) - s) as usize] {
                *a |= sp.attr;
            }
        }
        let from = self.markers.partition_point(|m| m.end <= s);
        for m in self.markers[from..].iter().take_while(|m| m.start < e) {
            let shown = reveal_all || (m.reveal.0 <= focus && focus <= m.reveal.1);
            for a in &mut out[(m.start.max(s) - s) as usize..(m.end.min(e) - s) as usize] {
                *a = if shown { DIM } else { HIDDEN };
            }
        }
    }

    pub fn block_active(&self, block: Option<u32>, focus: usize) -> bool {
        block.is_some_and(|i| {
            let b = &self.blocks[i as usize];
            b.start as usize <= focus && focus <= b.end as usize
        })
    }

    pub fn link_at(&self, off: usize) -> Option<&Link> {
        self.links.iter().find(|l| (l.start as usize) <= off && off < l.end as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(text: &str) -> Vec<usize> {
        std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect()
    }

    fn shown(text: &str, focus: usize) -> String {
        let md = parse(text, &starts(text));
        let mut attrs = Vec::new();
        md.attrs(0, text.len(), focus, false, &mut attrs);
        text.char_indices().filter(|&(i, _)| attrs[i] & HIDDEN == 0).map(|(_, c)| c).collect()
    }

    #[test]
    fn inline_markers_hide_until_focused() {
        let t = "x\n\na **bold** `code` [link](http://u) <u>und</u> \\* ~~s~~ ![alt](i.png)";
        assert_eq!(shown(t, 0), "x\n\na bold code link und * s alt");
        // Cursor inside the bold span reveals only its asterisks.
        assert_eq!(shown(t, 8), "x\n\na **bold** code link und * s alt");
        assert_eq!(shown(t, 25), "x\n\na bold code [link](http://u) und * s alt");
    }

    #[test]
    fn images_are_listed_with_their_source() {
        let t = "a ![one](x.png) b\n\n[![two](y%20z.jpg)](http://u) ![[three.webp]]";
        let md = parse(t, &starts(t));
        let found: Vec<(&str, &str)> =
            md.images.iter().map(|i| (&t[i.start as usize..i.end as usize], i.url.as_str())).collect();
        assert_eq!(
            found,
            [("![one](x.png)", "x.png"), ("![two](y%20z.jpg)", "y%20z.jpg"), ("![[three.webp]]", "three.webp")]
        );
    }

    #[test]
    fn headings() {
        let t = "## Title ##\n\nSetext\n===\n";
        let md = parse(t, &starts(t));
        assert_eq!(md.lines[0].kind, Kind::Heading(2));
        assert_eq!(md.lines[2].kind, Kind::Heading(1));
        assert_eq!(md.lines[3].kind, Kind::Underline);
        assert_eq!(shown(t, 20), "Title\n\nSetext\n===\n");
        assert_eq!(shown(t, 3), "## Title ##\n\nSetext\n===\n");
        assert!(md.block_active(md.lines[3].block, 14) && !md.block_active(md.lines[3].block, 0));
    }

    #[test]
    fn containers() {
        let t = "> quote\nlazy\n> > deep\n\n  - one\n    more\n  - [x] done\n    1. num\n";
        let md = parse(t, &starts(t));
        let kinds = |l: usize| md.lines[l].prefix.iter().map(|p| p.kind).collect::<Vec<_>>();
        assert_eq!(kinds(0), [PrefixKind::Quote]);
        assert_eq!(kinds(1), [PrefixKind::Quote]);
        assert_eq!(kinds(2), [PrefixKind::Quote, PrefixKind::Quote]);
        assert_eq!(kinds(4), [PrefixKind::Bullet]);
        assert!(matches!(kinds(5)[..], [PrefixKind::Indent(_, _, false)]));
        assert_eq!(kinds(6), [PrefixKind::Bullet, PrefixKind::Task(true)]);
        assert!(matches!(kinds(7)[..], [PrefixKind::Indent(..), PrefixKind::Number(..)]));
        let content = |l: usize| &t[md.lines[l].content as usize..];
        assert!(content(0).starts_with("quote") && content(2).starts_with("deep"));
        assert!(content(5).starts_with("more") && content(6).starts_with("done") && content(7).starts_with("num"));
    }

    #[test]
    fn blocks() {
        let t = "---\nk: v\n---\n\n```rs\nfn x() {}\n```\n\n| a | b |\n|:-:|--:|\n| 1 | *2* |\n\n---\n";
        let md = parse(t, &starts(t));
        let kinds: Vec<Kind> = md.lines.iter().map(|l| l.kind).collect();
        use Kind::*;
        assert_eq!(
            kinds,
            [
                FenceOpen, Meta, FenceClose, Text, FenceOpen, Code, FenceClose, Text, Table, Table, Table, Text, Rule,
                Text
            ]
        );
        let table = &md.tables[0];
        assert_eq!(table.aligns, [1, 2]);
        let base = md.blocks[md.lines[8].block.unwrap() as usize].start as usize;
        let cell = |r: usize, c: usize| &t[base + table.rows[r][c].0 as usize..base + table.rows[r][c].1 as usize];
        assert_eq!((cell(0, 0), cell(0, 1), cell(1, 0), cell(1, 1)), ("a", "b", "1", "*2*"));
    }
    #[test]
    fn fenced_languages_highlight_without_styling_the_fences_or_following_prose() {
        let text = "```python\n# note\ndef café():\n    return 42\n```\nreturn 42\n\n```rust\nlet x = \"hello\nworld\";\n```\n";
        let starts: Vec<_> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let md = parse(text, &starts);
        let at = text.find("42").unwrap();
        assert!(md.spans.iter().any(|s| s.start as usize == at && s.attr & NUMBER != 0));
        let last_prose = text.find("return 42\n\n").unwrap();
        assert!(!md.spans.iter().any(|s| s.start as usize >= last_prose && (s.start as usize) < last_prose + 9));
        assert!(
            md.spans
                .iter()
                .any(|s| &text[s.start as usize..s.end as usize] == "\"hello\nworld\"" && s.attr & SOFT != 0)
        );
        assert!(
            md.spans.iter().all(|s| text.is_char_boundary(s.start as usize) && text.is_char_boundary(s.end as usize))
        );
    }
}
