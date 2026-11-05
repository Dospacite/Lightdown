//! Turns one source line (or one rendered table) into positioned glyphs,
//! decorations and a map between source offsets and screen positions.
//! All offsets in a `Layout` are relative to the start of its line.

use std::path::PathBuf;

use swash::text::analyze;
use swash::text::cluster::Boundary;

use crate::cells::Tag;
use crate::font::{self, Fonts};
use crate::md::{self, Kind, Prefix, PrefixKind, Table};
use crate::theme;

#[derive(Clone, Copy)]
pub struct Glyph {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: u32,
    pub face: u8,
    pub id: u16,
}

/// A run of source bytes that moves as one unit: usually one character,
/// sometimes a ligature, a hidden marker or a list-marker slot.
pub struct Cluster {
    pub start: u32,
    pub end: u32,
    pub x: f32,
    pub w: f32,
    pub row: u32,
    pub space: bool,
    pub rtl: bool,
}

/// One visual row. `x0..x1` bound the area that hit-tests to this row,
/// which is the whole column for text and a single cell inside tables.
pub struct Row {
    pub y: f32,
    pub h: f32,
    pub x0: f32,
    pub x1: f32,
    pub text_x: f32,
    pub first: u32,
    pub last: u32,
    pub start: u32,
    pub end: u32,
}

#[derive(Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: u32,
    /// Corners to round: `TOP`, `BOTTOM` or both.
    pub round: u8,
}

pub const TOP: u8 = 1;
/// Rounds every corner a little, as a checkbox's.
pub const SOFT: u8 = 4;
pub const BOTTOM: u8 = 2;

#[derive(Default)]
pub struct Layout {
    pub h: f32,
    pub rows: Vec<Row>,
    pub clusters: Vec<Cluster>,
    pub glyphs: Vec<Glyph>,
    /// Painted beneath the text: panels and highlights.
    pub under: Vec<Rect>,
    /// Painted over the text: rules, underlines, strikes, checkboxes.
    pub over: Vec<Rect>,
    /// Bounds of a task checkbox and the offset of the character it toggles.
    pub checkbox: Option<(Rect, u32)>,
    pub images: Vec<Placed>,
    /// A rendered table: rows are cells rather than wrapped lines.
    pub grid: bool,
    pub sig: u64,
    /// Only bidi lines allocate visual caret stops.
    pub carets: Vec<(u32, u32, f32)>,
}

/// An image to show under a line, at its natural size in device pixels.
pub struct Image {
    /// Offset of the image's markdown within the line.
    pub at: u32,
    pub w: f32,
    pub h: f32,
    pub path: PathBuf,
    /// Centred in the column, as displayed mathematics is.
    pub center: bool,
}

/// A formula set in a line of text in place of its source, which runs
/// from `start` to `end` in the line. It stands on the baseline, `ascent`
/// pixels of it above.
#[derive(Clone)]
pub struct MathBox {
    pub start: u32,
    pub end: u32,
    pub w: f32,
    pub h: f32,
    pub ascent: f32,
    pub path: PathBuf,
}

/// An image placed in a layout, scaled down to fit the column.
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub at: u32,
    pub path: PathBuf,
}

pub struct Input<'a> {
    pub doc: &'a str,
    pub start: usize,
    pub end: usize,
    /// One attribute word per byte of `start..end`; may be empty for plain text.
    pub attrs: &'a [crate::md::Attr],
    pub kind: Kind,
    pub prefix: &'a [Prefix],
    pub content: usize,
    /// The cursor is on this line: show its own syntax.
    pub line_active: bool,
    /// The cursor is in this line's block: show the block as source.
    pub block_active: bool,
    pub first: bool,
    pub mono: bool,
    pub images: &'a [Image],
    /// Formulas set in the line's text.
    pub maths: &'a [MathBox],
    /// For a cell's header line: what to write at its left and right ends.
    pub label: &'a str,
    pub tag: &'a str,
    /// What running a cell printed, shown under the cell's last line.
    pub output: &'a [OutItem],
    /// The family prose is set in: `font::SERIF` or `font::SANS`.
    pub family: u8,
    /// In a conversation, the kind of message the line belongs to, and
    /// whether it is the message's first and last line.
    pub talk: Option<(Tag, bool, bool)>,
    /// Lines break at the column's edge; without it each line is one row
    /// that may run past the edge.
    pub wrap: bool,
    pub bidi_base: Option<bool>,
}

/// One piece of a cell's output.
pub enum OutItem {
    Table(crate::table::Table),
    /// Text in the monospace face; `attrs`, when not empty, colour each byte.
    Text { text: String, dim: bool, error: bool, attrs: Vec<md::Attr> },
    Image(Image),
    /// Markdown, set as the document's own markdown is.
    Markdown(String),
    /// A formula TeX has set; its size is in points.
    Formula(Image),
    /// A cell's request for input: its prompt and what is typed so far.
    Input { prompt: String, typed: String },
}

/// Moves a layout's marks by `(dx, dy)` and adds them to `out`.
fn graft(out: &mut Layout, lay: Layout, dx: f32, dy: f32, at: u32) {
    out.glyphs.extend(lay.glyphs.into_iter().map(|g| Glyph { x: g.x + dx, y: g.y + dy, ..g }));
    let shift = |r: Rect| Rect { x: r.x + dx, y: r.y + dy, ..r };
    out.under.extend(lay.under.into_iter().map(shift));
    out.over.extend(lay.over.into_iter().map(shift));
    out.images.extend(lay.images.into_iter().map(|p| Placed { x: p.x + dx, y: p.y + dy, at, ..p }));
}

/// Lays out markdown as the lines of a document are, one under another.
fn markdown_output(fonts: &mut Fonts, text: &str, family: u8, px: f32, width: f32) -> Vec<(Layout, f32)> {
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let md = md::parse(text, &starts);
    let mut attrs = Vec::new();
    let mut out = Vec::new();
    for (li, &s) in starts.iter().enumerate() {
        let e = starts.get(li + 1).map_or(text.len(), |n| n - 1);
        let info = &md.lines[li];
        let (mut start, mut end, mut prefix, mut content) = (s, e, &info.prefix[..], info.content as usize);
        let mut table = None;
        if info.kind == Kind::Table {
            let block = &md.blocks[info.block.unwrap_or(0) as usize];
            if block.start as usize != s {
                continue;
            }
            (start, end, prefix, content) = (s, block.end as usize, &[][..], s);
            table = block.table.map(|t| &md.tables[t as usize]);
        }
        md.attrs(start, end, usize::MAX, false, &mut attrs);
        let inp = Input {
            doc: text,
            start,
            end,
            attrs: &attrs,
            kind: info.kind,
            prefix,
            content,
            line_active: false,
            block_active: false,
            first: false,
            mono: false,
            images: &[],
            maths: &[],
            label: "",
            tag: "",
            output: &[],
            family,
            talk: None,
            wrap: true,
            bidi_base: info.bidi_base,
        };
        let lay = match table {
            Some(t) => self::table(fonts, &inp, t, px, width),
            None => line(fonts, &inp, px, width),
        };
        // Blank lines only part paragraphs; a whole row each is too much under a cell.
        let h = if text[start..end].trim().is_empty() { (lay.h * 0.4).round() } else { lay.h };
        out.push((lay, h));
    }
    out
}

#[derive(Clone, Copy)]
struct Style {
    size: f32,
    bold: bool,
    mono: bool,
    color: u32,
    family: u8,
}

struct Item {
    start: u32,
    end: u32,
    w: f32,
    g0: u32,
    g1: u32,
    attr: crate::md::Attr,
    size: f32,
    brk: bool,
    space: bool,
}

/// A paragraph being built: items in source order, not yet broken into rows.
#[derive(Default)]
struct Flow {
    items: Vec<Item>,
    glyphs: Vec<Glyph>,
    /// Formulas to set in the text, and the item each became.
    maths: Vec<MathBox>,
    boxed: Vec<(usize, usize)>,
    bidi: Option<crate::bidi::Paragraph>,
    bidi_checked: bool,
}

struct Place {
    x0: f32,
    hang: f32,
    x1: f32,
    y: f32,
    row_h: f32,
    base: f32,
    align: f32,
    cell: (f32, f32),
    at: u32,
}

const DECO: crate::md::Attr = md::UNDER | md::LINK | md::STRIKE | md::CODE | md::MARK;

/// Marks every byte offset at which a line break is permitted.
fn breaks(text: &str) -> Vec<bool> {
    let mut brk = vec![false; text.len() + 1];
    for ((i, _), (_, b)) in text.char_indices().zip(analyze(text.chars())) {
        brk[i] = i > 0 && matches!(b, Boundary::Line | Boundary::Mandatory);
    }
    brk
}

impl Flow {
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, fonts: &mut Fonts, text: &str, from: usize, to: usize, attrs: &[crate::md::Attr], brk: &[bool], st: Style) {
        if !self.bidi_checked { self.bidi = crate::bidi::Paragraph::new(text); self.bidi_checked = true; }
        let attr_at = |i: usize| attrs.get(i).copied().unwrap_or(0);
        let mut i = from;
        while i < to {
            // A formula stands in for its source, as one piece.
            if let Some(m) = self.maths.iter().position(|m| m.start as usize == i && m.end as usize <= to) {
                let (end, w) = (self.maths[m].end, self.maths[m].w);
                let g = self.glyphs.len() as u32;
                self.boxed.push((self.items.len(), m));
                self.items.push(Item {
                    start: i as u32,
                    end,
                    w,
                    g0: g,
                    g1: g,
                    attr: 0,
                    size: st.size,
                    brk: brk[i],
                    space: false,
                });
                i = end as usize;
                continue;
            }
            let a = attr_at(i);
            let mut j = i + 1;
            while j < to && attr_at(j) == a
                && self.bidi.as_ref().is_none_or(|p| p.levels.get(j) == p.levels.get(i)) {
                j += 1;
            }
            if a & md::HIDDEN != 0 {
                let g = self.glyphs.len() as u32;
                self.items.push(Item {
                    start: i as u32,
                    end: j as u32,
                    w: 0.0,
                    g0: g,
                    g1: g,
                    attr: a,
                    size: st.size,
                    brk: brk[i],
                    space: false,
                });
                i = j;
                continue;
            }
            let code = a & md::CODE != 0;
            let face = if st.mono && a & md::BOLD != 0 {
                fonts.mono_bold()
            } else if st.mono || code {
                fonts.mono()
            } else {
                Fonts::face(st.family, st.bold || a & md::BOLD != 0, a & md::ITALIC != 0)
            };
            let size = if code && !st.mono { (st.size * theme::mono_scale()).round() } else { st.size };
            let color = if a & md::REMOVED != 0 {
                theme::danger()
            } else if a & md::PARAMETER != 0 {
                theme::parameter()
            } else if a & md::PROPERTY != 0 {
                theme::property()
            } else if a & md::DIM != 0 {
                theme::dim()
            } else if a & md::KEYWORD != 0 {
                theme::keyword()
            } else if a & md::SOFT != 0 {
                theme::string()
            } else if a & md::NUMBER != 0 {
                theme::number()
            } else if a & md::FUNCTION != 0 {
                theme::function()
            } else if a & md::SPECIAL != 0 {
                theme::special()
            } else if a & md::LINK != 0 {
                theme::link()
            } else if a & md::BOLD != 0 {
                theme::bright()
            } else {
                st.color
            };
            let run = &text[i..j];
            let tab = if run.contains('\t') { fonts.width(face, size, " ") * 4.0 } else { 0.0 };
            let (items, glyphs) = (&mut self.items, &mut self.glyphs);
            let rtl = self.bidi.as_ref().is_some_and(|p| p.levels[i].is_rtl());
            crate::bidi::shape_logical(fonts, face, size, run, rtl, |s, e, adv, shaped| {
                let piece = &run[s..e];
                let is_tab = piece == "\t";
                let g0 = glyphs.len() as u32;
                if !is_tab {
                    glyphs.extend(shaped.iter().map(|g| Glyph {
                        x: g.x,
                        y: -g.y,
                        size,
                        color,
                        face: g.face,
                        id: g.id,
                    }));
                }
                items.push(Item {
                    start: (i + s) as u32,
                    end: (i + e) as u32,
                    w: if is_tab { tab } else { adv },
                    g0,
                    g1: glyphs.len() as u32,
                    attr: a,
                    size,
                    brk: brk[i + s],
                    space: piece.chars().all(char::is_whitespace),
                });
            });
            i = j;
        }
    }

    /// A fixed-width slot standing in for a container marker.
    fn slot(&mut self, start: u32, end: u32, w: f32) {
        let g = self.glyphs.len() as u32;
        self.items.push(Item { start, end, w, g0: g, g1: g, attr: 0, size: 0.0, brk: false, space: false });
    }

    /// Draws `text` inside the most recent slot, `dx` from its left edge.
    fn slot_text(&mut self, fonts: &mut Fonts, face: u8, size: f32, color: u32, text: &str, dx: f32) {
        let Some(item) = self.items.last_mut() else { return };
        let glyphs = &mut self.glyphs;
        let mut pen = dx;
        fonts.shape(face, size, text, |_, _, adv, shaped| {
            glyphs.extend(shaped.iter().map(|g| Glyph { x: pen + g.x, y: -g.y, size, color, face: g.face, id: g.id }));
            pen += adv;
        });
        item.g1 = glyphs.len() as u32;
    }

    fn width(&self) -> f32 {
        self.items.iter().map(|i| i.w).sum()
    }

    /// Breaks the items into rows and emits them into `out`. Returns the y
    /// below the last row.
    fn place(&self, out: &mut Layout, p: Place) -> f32 {
        // Greedy line breaking: `bounds` holds the item index starting each row.
        let mut bounds = vec![0usize];
        let (mut x, mut row, mut last_brk) = (p.x0, 0usize, None);
        for (i, item) in self.items.iter().enumerate() {
            if item.brk && i > row {
                last_brk = Some(i);
            }
            while x + item.w > p.x1 && !item.space && i > row {
                let b = last_brk.take().unwrap_or(i);
                bounds.push(b);
                row = b;
                x = p.hang + self.items[b..i].iter().map(|it| it.w).sum::<f32>();
            }
            x += item.w;
        }
        bounds.push(self.items.len());

        let mut y = p.y;
        for (r, w) in bounds.windows(2).enumerate() {
            let items = &self.items[w[0]..w[1]];
            let mut x = if r == 0 { p.x0 } else { p.hang };
            let align = if self.bidi.as_ref().is_some_and(|p| p.rtl) && p.x1 < f32::MAX && p.align == 0.0 { 1.0 } else { p.align };
            if align > 0.0 {
                let used: f32 = items.iter().rev().skip_while(|i| i.space).map(|i| i.w).sum();
                x += ((p.x1 - x - used) * align).max(0.0);
            }
            let base = (y + p.base).round();
            let first = out.clusters.len() as u32;
            let row = out.rows.len() as u32;
            let text_x = x;
            let mut run: Option<(crate::md::Attr, f32, f32)> = None;
            let visual = self.bidi.as_ref().map(|p| {
                let starts: Vec<usize> = items.iter().map(|i| i.start as usize).collect();
                p.order(items.first().map_or(0,|i| i.start as usize)..items.last().map_or(0,|i| i.end as usize),&starts)
            });
            for n in 0..items.len() {
                let k = visual.as_ref().map_or(n,|v| v.0[n]);
                let item = &items[k];
                let rtl = visual.as_ref().is_some_and(|v| v.1[k]);
                if let Some(p) = &self.bidi {
                    let piece = &p.text()[item.start as usize..item.end as usize];
                    let stops: Vec<usize> = piece.char_indices().map(|c| c.0).chain(std::iter::once(piece.len())).collect();
                    let count = stops.len().saturating_sub(1).max(1) as f32;
                    for (i,at) in stops.iter().enumerate() {
                        let f = i as f32/count;
                        out.carets.push((item.start+*at as u32,row,x+item.w*if rtl { 1.0-f } else { f }));
                    }
                }
                out.clusters.push(Cluster { start: item.start, end: item.end, x, w: item.w, row, space: item.space, rtl });
                if let Some(&(_, m)) = self.boxed.iter().find(|b| b.0 == w[0] + k) {
                    let m = &self.maths[m];
                    let path = m.path.clone();
                    out.images.push(Placed { x, y: base - m.ascent.round(), w: m.w, h: m.h, at: item.start, path });
                }
                for g in &self.glyphs[item.g0 as usize..item.g1 as usize] {
                    out.glyphs.push(Glyph { x: x + g.x, y: base + g.y, ..*g });
                }
                let deco = item.attr & DECO;
                if run.is_some_and(|r| r.0 != deco) {
                    decorate(out, run.take().unwrap(), x, y, p.row_h, base);
                }
                if deco != 0 && run.is_none() {
                    run = Some((deco, x, item.size));
                }
                x += item.w;
                if n + 1 == items.len() && run.is_some() {
                    decorate(out, run.take().unwrap(), x, y, p.row_h, base);
                }
            }
            out.rows.push(Row {
                y,
                h: p.row_h,
                x0: p.cell.0,
                x1: p.cell.1,
                text_x,
                first,
                last: out.clusters.len() as u32,
                start: items.first().map_or(p.at, |i| i.start),
                end: items.last().map_or(p.at, |i| i.end),
            });
            y += p.row_h;
        }
        y
    }
}

fn decorate(out: &mut Layout, (deco, x0, size): (crate::md::Attr, f32, f32), x1: f32, y: f32, row_h: f32, base: f32) {
    let w = x1 - x0;
    let thick = (size * 0.06).round().max(1.0);
    if deco & md::CODE != 0 {
        let (pad, h) = (size * 0.2, (size * 1.45).round());
        out.under.push(Rect {
            x: x0 - pad,
            y: y + ((row_h - h) / 2.0).round(),
            w: w + pad * 2.0,
            h,
            color: theme::tint(),
            round: TOP | BOTTOM,
        });
    }
    if deco & md::MARK != 0 {
        let h = (size * 1.3).round();
        out.under.push(Rect {
            x: x0,
            y: y + ((row_h - h) / 2.0).round(),
            w,
            h,
            color: theme::found(),
            ..Rect::default()
        });
    }
    if deco & (md::UNDER | md::LINK) != 0 {
        let color = if deco & md::UNDER != 0 { theme::fg() } else { theme::mix(theme::bg(), theme::link(), 140) };
        out.over.push(Rect { x: x0, y: base + (size * 0.16).round(), w, h: thick, color, ..Rect::default() });
    }
    if deco & md::STRIKE != 0 {
        out.over.push(Rect {
            x: x0,
            y: base - (size * 0.27).round(),
            w,
            h: thick,
            color: theme::fg(),
            ..Rect::default()
        });
    }
}

/// Baseline offset that centres a face's ascent+descent in a row.
fn baseline(fonts: &Fonts, face: u8, size: f32, row_h: f32) -> f32 {
    let (asc, desc, _) = fonts.metrics(face, size);
    ((row_h - (asc + desc)) / 2.0 + asc).round()
}

/// Width of the slot a list marker occupies; shared by the item's own line
/// and its continuation lines so that their text aligns.
fn marker_slot(fonts: &mut Fonts, face: u8, px: f32, number: Option<&str>) -> f32 {
    let base = px * 1.5;
    match number {
        Some(t) => (fonts.width(face, px, t) + px * 0.45).max(base),
        None => base,
    }
}

/// The height of a notebook cell's header row, and the space under it,
/// for the controls laid over it.
pub fn header_row(px: f32) -> (f32, f32) {
    ((px * 1.75).round(), (px * 0.3).round())
}

#[allow(clippy::too_many_arguments)]
fn output_text(
    out: &mut Layout,
    fonts: &mut Fonts,
    face: u8,
    size: f32,
    color: u32,
    text: &str,
    x: f32,
    base: f32,
) -> f32 {
    let mut pen = x;
    fonts.shape(face, size, text, |_, _, adv, shaped| {
        out.glyphs.extend(shaped.iter().map(|g| Glyph {
            x: pen + g.x,
            y: base - g.y,
            size,
            color,
            face: g.face,
            id: g.id,
        }));
        pen += adv;
    });
    pen - x
}

/// Draw a cell header without editable text rows.
fn header(fonts: &mut Fonts, inp: &Input, px: f32, width: f32) -> Layout {
    let mut out = Layout::default();
    if let Some((tag, ..)) = inp.talk {
        let gap = match tag {
            _ if inp.first => (px * 0.4).round(),
            Tag::User | Tag::Assistant => (px * 1.1).round(),
            _ => (px * 0.5).round(),
        };
        if inp.label.is_empty() {
            out.h = gap;
            return out;
        }
        let size = (px * 0.86).round();
        let row_h = (size * 1.6).round();
        let base = gap + baseline(fonts, font::UI_MEDIUM, size, row_h);
        output_text(&mut out, fonts, font::UI_MEDIUM, size, theme::faint(), inp.label, 0.0, base);
        out.h = gap + row_h;
        return out;
    }
    let top = if inp.first { (px * 0.6).round() } else { (px * 0.9).round() };
    if inp.label.is_empty() && inp.tag.is_empty() {
        out.h = if inp.first { top } else { (px * 0.5).round() };
        return out;
    }
    let size = (px * 0.78).round().max(px.min(theme::SMALL));
    let (row_h, under) = header_row(px);
    // Execution counts read as code; names read as text.
    let face = if inp.label.contains('[') { fonts.mono() } else { font::UI_MEDIUM };
    let base = top + baseline(fonts, face, size, row_h);
    let gap = (px * 0.5).round();
    let ink = if inp.label.contains('*') || inp.label.contains('\u{b7}') {
        theme::warning()
    } else if inp.label.contains('[') && !inp.label.ends_with("[ ]") {
        theme::success()
    } else {
        theme::dim()
    };
    let left = output_text(&mut out, fonts, face, size, ink, inp.label, 0.0, base);
    let tw = fonts.width(font::UI_ITALIC, size, inp.tag);
    output_text(&mut out, fonts, font::UI_ITALIC, size, theme::faint(), inp.tag, width - tw, base);
    let x0 = if left > 0.0 { left + gap } else { 0.0 };
    let x1 = if tw > 0.0 { width - tw - gap } else { width };
    out.over.push(Rect {
        x: x0,
        y: top + (row_h / 2.0).round(),
        w: (x1 - x0).max(0.0),
        h: 1.0,
        color: theme::line(),
        ..Rect::default()
    });
    out.h = top + row_h + under;
    out
}

/// Places a cell's output below `y`. Output is not part of the document:
/// it gets glyphs but no rows, so it can be seen and not edited.
fn output(fonts: &mut Fonts, out: &mut Layout, inp: &Input, px: f32, width: f32, mut y: f32) -> f32 {
    let size = (px * 0.8).round();
    let row_h = (size * 1.45).round();
    let inset = (px * 0.8).round();
    let mono = fonts.mono();
    let base = baseline(fonts, mono, size, row_h);
    let at = (inp.end - inp.start) as u32;
    y += (px * 0.4).round();
    for item in inp.output {
        match item {
            OutItem::Text { text, dim, error, attrs } => {
                let color = if *error {
                    theme::danger()
                } else if *dim {
                    theme::dim()
                } else {
                    theme::fg()
                };
                let st = Style { size, bold: false, mono: true, color, family: inp.family };
                let mut from = 0;
                for line in text.split('\n') {
                    let line_attrs = attrs.get(from..from + line.len()).unwrap_or(&[]);
                    from += line.len() + 1;
                    let mut flow = Flow::default();
                    flow.text(fonts, line, 0, line.len(), line_attrs, &breaks(line), st);
                    let mut rows = Layout::default();
                    let place =
                        Place { x0: inset, hang: inset, x1: width, y, row_h, base, align: 0.0, cell: (0.0, width), at };
                    y = flow.place(&mut rows, place);
                    out.glyphs.extend(rows.glyphs);
                }
            }
            OutItem::Table(table) => {
                let cols = table.rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let cw = ((width - inset) / cols as f32).max(1.0);
                for (ri, row) in table.rows.iter().enumerate() {
                    if ri == 0 || ri % 2 == 0 {
                        out.under.push(Rect {
                            x: inset,
                            y,
                            w: (width - inset).max(0.0),
                            h: row_h + size * 0.4,
                            color: theme::hover(),
                            ..Default::default()
                        });
                    }
                    for ci in 0..cols {
                        let x = inset + ci as f32 * cw;
                        let value = row.get(ci).map_or("", String::as_str);
                        let text = crate::ui::fit(fonts, mono, size, value, (cw - size * 0.7).max(0.0));
                        output_text(out, fonts, mono, size, theme::fg(), &text, x + size * 0.3, y + base);
                        out.over.push(Rect {
                            x,
                            y,
                            w: 1.0,
                            h: row_h + size * 0.4,
                            color: theme::line(),
                            ..Default::default()
                        });
                    }
                    y += row_h + size * 0.4;
                    out.over.push(Rect {
                        x: inset,
                        y,
                        w: (width - inset).max(0.0),
                        h: 1.0,
                        color: theme::line(),
                        ..Default::default()
                    });
                }
                if table.truncated {
                    output_text(
                        out,
                        fonts,
                        mono,
                        size,
                        theme::dim(),
                        "Table truncated to 100 rows, 30 columns and 400 characters per cell",
                        inset,
                        y + base,
                    );
                    y += row_h;
                }
            }
            OutItem::Image(im) => {
                let w = im.w.min(width - inset).round().max(1.0);
                let h = (im.h * w / im.w).round().max(1.0);
                y += (px * 0.2).round();
                out.images.push(Placed { x: inset, y, w, h, at, path: im.path.clone() });
                y += h + (px * 0.2).round();
            }
            OutItem::Formula(im) => {
                // TeX's ten points to the body text's size, as in documents.
                let k = px / 9.5;
                let w = (im.w * k).min(width - inset).round().max(1.0);
                let h = (im.h * k * w / (im.w * k).max(1.0)).round().max(1.0);
                y += (px * 0.3).round();
                out.images.push(Placed { x: inset, y, w, h, at, path: im.path.clone() });
                y += h + (px * 0.3).round();
            }
            OutItem::Markdown(text) => {
                for (lay, h) in markdown_output(fonts, text, inp.family, (px * 0.9).round(), (width - inset).max(1.0)) {
                    graft(out, lay, inset, y, at);
                    y += h;
                }
            }
            OutItem::Input { prompt, typed } => {
                let field = row_h + size * 0.4;
                out.under.push(Rect {
                    x: inset,
                    y,
                    w: (width - inset).max(0.0),
                    h: field,
                    color: theme::hover(),
                    round: TOP | BOTTOM,
                });
                let base = y + (size * 0.2).round() + baseline(fonts, mono, size, row_h);
                let mut x = inset + size * 0.4;
                x += output_text(out, fonts, mono, size, theme::dim(), prompt, x, base);
                x += output_text(out, fonts, mono, size, theme::fg(), typed, x, base);
                out.over.push(Rect { x, y: y + size * 0.3, w: 2.0, h: field - size * 0.6, color: theme::primary(), round: 0 });
                y += field;
            }
        }
    }
    y + (px * 0.3).round()
}

pub fn line(fonts: &mut Fonts, inp: &Input, px: f32, width: f32) -> Layout {
    if inp.kind == Kind::Cell {
        return header(fonts, inp, px, width);
    }
    let text = &inp.doc[inp.start..inp.end];
    let rel = |abs: u32| abs - inp.start as u32;
    let mut out = Layout::default();
    let family = inp.family;
    let regular = Fonts::face(family, false, false);
    let mut st = Style { size: px, bold: false, mono: inp.mono, color: theme::fg(), family };
    let mut row_h = (px * theme::LINE_HEIGHT).round();
    let (mut top, mut bottom) = (if inp.first { (px * 1.4).round() } else { 0.0 }, 0.0);
    let (mut panel, mut hide, mut plain) = (false, false, false);
    let mono_px = (px * theme::mono_scale()).round();
    if inp.mono {
        st.size = mono_px;
        row_h = (mono_px * theme::code_line_height()).round();
    }
    // In a conversation: the user's messages lie in a bubble at the right;
    // the agent's work and thinking are quieter, set off by a rule.
    let (mut lead, mut trail) = (0.0, 0.0);
    let mut bubble = None;
    let mut rule = false;
    if let Some((tag, first, last)) = inp.talk {
        match tag {
            Tag::User => {
                let indent = (width * 0.14).min(px * 4.0).round();
                let pad = (px * 0.85).round();
                lead = indent + pad;
                trail = pad;
                let round = if first { TOP } else { 0 } | if last { BOTTOM } else { 0 };
                bubble = Some((indent, round));
                if first {
                    top += (px * 0.6).round();
                }
                if last {
                    bottom += (px * 0.6).round();
                }
            }
            Tag::Tool | Tag::Think => {
                lead = (px * 0.9).round();
                rule = true;
                st.size = (px * 0.93).round();
                row_h = (st.size * 1.55).round();
            }
            _ => {}
        }
    }
    match inp.kind {
        Kind::Text => {}
        Kind::Heading(level) => {
            st.size = (px * theme::HEADING[level as usize - 1]).round();
            st.bold = true;
            st.color = theme::bright();
            row_h = (st.size * 1.3).round();
            if !inp.first {
                top += (px * 1.5).round();
            }
            bottom = (px * (28.0 / theme::BODY)).round();
        }
        Kind::Underline if !inp.block_active => return out,
        Kind::Underline => st.color = theme::dim(),
        Kind::Code | Kind::Meta | Kind::FenceOpen | Kind::FenceClose => {
            st = Style { size: mono_px, bold: false, mono: true, color: theme::fg(), family };
            row_h = (mono_px * 1.55).round();
            panel = true;
            if inp.kind != Kind::Code {
                st.color = theme::dim();
            }
            if matches!(inp.kind, Kind::FenceOpen | Kind::FenceClose) && !inp.block_active {
                // Same height as when shown, so entering the block shifts nothing.
                hide = true;
            }
        }
        Kind::Table => {
            st = Style { size: mono_px, bold: false, mono: true, color: theme::fg(), family };
            row_h = (mono_px * 1.5).round();
            plain = true;
        }
        Kind::Rule => {
            hide = !inp.line_active;
            st.color = theme::dim();
        }
        Kind::Html => st = Style { size: mono_px, bold: false, mono: true, color: theme::dim(), family },
        Kind::Cell => {}
    }

    let mut flow = Flow { maths: inp.maths.to_vec(), ..Flow::default() };
    if inp.bidi_base.is_some() {
        flow.bidi = crate::bidi::Paragraph::with_base(text,inp.bidi_base);
        flow.bidi_checked = true;
    }
    let mut x = 0.0;
    let mut bars = Vec::new();
    let mut boxed = None;
    for p in inp.prefix {
        let (s, e) = (rel(p.start), rel(p.end));
        let w = match p.kind {
            PrefixKind::Quote => {
                let w = (px * 1.2).round();
                flow.slot(s, e, w);
                bars.push(x);
                if inp.line_active && e > s {
                    flow.slot_text(fonts, regular, px, theme::faint(), ">", px * 0.45);
                }
                if inp.kind == Kind::Text {
                    st.color = theme::quote();
                }
                w
            }
            PrefixKind::Bullet => {
                let w = marker_slot(fonts, regular, px, None);
                flow.slot(s, e, w);
                let raw = text[s as usize..e as usize].trim();
                let mark = if inp.line_active { raw } else { "\u{2022}" };
                let tw = fonts.width(regular, px, mark);
                flow.slot_text(fonts, regular, px, theme::dim(), mark, w - px * 0.55 - tw);
                w
            }
            PrefixKind::Number(m0, m1) => {
                let mark = &inp.doc[m0 as usize..m1 as usize];
                let w = marker_slot(fonts, regular, px, Some(mark));
                flow.slot(s, e, w);
                let tw = fonts.width(regular, px, mark);
                flow.slot_text(fonts, regular, px, theme::dim(), mark, w - px * 0.45 - tw);
                w
            }
            PrefixKind::Indent(m0, m1, ordered) => {
                let w = marker_slot(fonts, regular, px, ordered.then(|| &inp.doc[m0 as usize..m1 as usize]));
                flow.slot(s, e, w);
                w
            }
            PrefixKind::Task(done) => {
                let mut w = (px * 1.4).round();
                if inp.line_active {
                    let mono = fonts.mono();
                    let raw = text[s as usize..e as usize].trim_end();
                    let size = (mono_px * 0.9).round();
                    w = w.max((fonts.width(mono, size, raw) + px * 0.35).round());
                    flow.slot(s, e, w);
                    flow.slot_text(fonts, mono, size, theme::dim(), raw, 0.0);
                } else {
                    flow.slot(s, e, w);
                    boxed = Some((x, done, s + 1));
                }
                if done && inp.kind == Kind::Text {
                    st.color = theme::dim();
                }
                w
            }
        };
        x += w;
    }
    let content = rel(inp.content as u32) as usize;
    // Code sits inset from the edge of its panel.
    let inset = if panel { (px * 0.8).round() } else { 0.0 };
    if panel {
        flow.slot(content as u32, content as u32, inset);
    }
    let (panel_x, hang) = (lead + x, lead + x + inset);

    if hide {
        if content < text.len() {
            flow.slot(content as u32, text.len() as u32, 0.0);
        }
        // A closed code block names its language in the corner.
        let fence = text[content..].trim_start();
        let lang = fence.trim_start_matches(['`', '~']).trim();
        if inp.kind == Kind::FenceOpen && lang.len() < fence.len() && !lang.is_empty() {
            let size = (px * 0.78).round().max(px.min(theme::SMALL));
            let tw = fonts.width(font::UI_MEDIUM, size, lang);
            flow.slot_text(fonts, font::UI_MEDIUM, size, theme::faint(), lang, width - trail - inset - hang - tw);
        }
    } else {
        let brk = breaks(text);
        let attrs = if plain { &[][..] } else { inp.attrs };
        flow.text(fonts, text, content, text.len(), attrs, &brk, st);
    }

    let face = if st.mono { fonts.mono() } else { Fonts::face(family, st.bold, false) };
    let base = baseline(fonts, face, st.size, row_h);
    let place = Place {
        x0: lead,
        hang,
        x1: if inp.wrap { width - inset - trail } else { f32::MAX },
        y: top,
        row_h,
        base,
        align: 0.0,
        cell: (0.0, width),
        at: content as u32,
    };
    // A line that is nothing but images shows only them, until the cursor
    // comes to it and its markdown appears above.
    let bare = !inp.images.is_empty() && !inp.line_active && flow.items.iter().all(|i| i.w == 0.0);
    let mut y = if bare { top } else { flow.place(&mut out, place) };
    if !inp.images.is_empty() {
        let gap = (px * 0.4).round();
        let room = (width - trail - hang).max(1.0);
        let (mut x, mut tallest) = (hang, 0.0f32);
        y += gap;
        for im in inp.images {
            let w = im.w.min(room).round().max(1.0);
            let h = (im.h * w / im.w).round().max(1.0);
            if x > hang && x + w > width - trail {
                (x, y, tallest) = (hang, y + tallest + gap, 0.0);
            }
            if im.center {
                x = (hang + (room - w) / 2.0).round();
            }
            out.images.push(Placed { x, y, w, h, at: im.at, path: im.path.clone() });
            x += w + gap;
            tallest = tallest.max(h);
        }
        y += tallest + gap;
    }
    out.h = y + bottom;
    let body = out.h;
    if !inp.output.is_empty() {
        out.h = output(fonts, &mut out, inp, px, width, body);
    }

    if panel {
        // A fenced block rounds where it opens and where it closes.
        let round = match inp.kind {
            Kind::FenceOpen => TOP,
            Kind::FenceClose => BOTTOM,
            _ => 0,
        };
        let w = width - trail - panel_x;
        out.under.insert(0, Rect { x: panel_x, y: 0.0, w, h: body, color: theme::inset(), round });
    }
    if let Some((indent, round)) = bubble {
        out.under.insert(0, Rect { x: indent, y: 0.0, w: width - indent, h: out.h, color: theme::tint(), round });
    }
    if rule {
        out.under.push(Rect {
            x: 0.0,
            y: 0.0,
            w: (px * 0.13).round().max(2.0),
            h: out.h,
            color: theme::line(),
            ..Rect::default()
        });
    }
    for bx in bars {
        out.under.push(Rect {
            x: lead + bx + (px * 0.15).round(),
            y: 0.0,
            w: (px * 0.18).round().max(3.0),
            h: out.h,
            color: theme::line(),
            ..Rect::default()
        });
    }
    if let Some((bx, done, off)) = boxed {
        let s = (px * 0.85).round();
        let bx = if out.carets.is_empty() { lead+bx } else { out.clusters.iter().find(|c| c.start <= off && c.end > off).map_or(lead+bx,|c| c.x) };
        let (bx, by) = ((bx + px * 0.05).round(), (top + (row_h - s) / 2.0).round());
        if done {
            out.under.push(Rect { x: bx, y: by, w: s, h: s, color: theme::primary(), round: SOFT });
            // The tick, from the interface face.
            let size = (s * 0.8).round();
            let tw = fonts.width(font::UI_BOLD, size, "\u{2713}");
            let (gx, base) = (bx + ((s - tw) / 2.0).round(), by + (s * 0.5 + size * 0.36).round());
            fonts.shape(font::UI_BOLD, size, "\u{2713}", |_, _, _, shaped| {
                out.glyphs.extend(shaped.iter().map(|g| Glyph {
                    x: gx + g.x,
                    y: base - g.y,
                    size,
                    color: theme::on_primary(),
                    face: g.face,
                    id: g.id,
                }));
            });
        } else {
            let t = (px * 0.07).round().max(1.0);
            for (x, y, w, h) in [(bx, by, s, t), (bx, by + s - t, s, t), (bx, by, t, s), (bx + s - t, by, t, s)] {
                out.over.push(Rect { x, y, w, h, color: theme::faint(), ..Rect::default() });
            }
        }
        out.checkbox = Some((Rect { x: bx, y: by, w: s, h: s, color: 0, ..Rect::default() }, off));
    }
    if hide && inp.kind == Kind::Rule {
        out.over.push(Rect {
            x: hang,
            y: top + (row_h / 2.0).round(),
            w: width - trail - hang,
            h: 1.0,
            color: theme::line(),
            ..Rect::default()
        });
    }
    if inp.kind == Kind::Heading(1) {
        out.h += (px * 0.25).round();
    }
    out
}

/// Lays out a whole table as a grid. `inp` covers the table's source block.
pub fn table(fonts: &mut Fonts, inp: &Input, table: &Table, px: f32, width: f32) -> Layout {
    let text = &inp.doc[inp.start..inp.end];
    let brk = breaks(text);
    let mut out = Layout { grid: true, ..Layout::default() };
    let size = (px * 0.95).round();
    let row_h = (size * 1.45).round();
    let (pad, vpad) = ((px * 0.75).round(), (px * 0.32).round());
    let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(table.aligns.len());
    if cols == 0 {
        return out;
    }

    let mut widths = vec![px * 2.5; cols];
    let mut flows = Vec::new();
    for (r, row) in table.rows.iter().enumerate() {
        let st = Style {
            size,
            bold: r == 0,
            mono: false,
            color: if r == 0 { theme::bright() } else { theme::fg() },
            family: inp.family,
        };
        let mut cells = Vec::new();
        for (c, &(s, e)) in row.iter().enumerate() {
            let mut flow = Flow::default();
            flow.text(fonts, text, s as usize, e as usize, inp.attrs, &brk, st);
            widths[c] = widths[c].max(flow.width().ceil() + pad * 2.0 + 1.0);
            cells.push(flow);
        }
        flows.push(cells);
    }
    // Too wide: shrink the columns in proportion, keeping each readable.
    let total: f32 = widths.iter().sum();
    if total > width {
        let floor = px * 5.0;
        for w in &mut widths {
            *w = (*w * width / total).max(floor.min(*w));
        }
    }
    let total: f32 = widths.iter().map(|w| w.round()).sum();

    let base = baseline(fonts, Fonts::face(inp.family, false, false), size, row_h);
    let mut y = (px * 0.6).round();
    out.over.push(Rect { x: 0.0, y, w: total, h: 1.0, color: theme::faint(), ..Rect::default() });
    for (r, cells) in flows.iter().enumerate() {
        let (mut x, mut bottom) = (0.0, y + vpad + row_h);
        for (c, flow) in cells.iter().enumerate() {
            let w = widths[c].round();
            let align = *table.aligns.get(c).unwrap_or(&0) as f32 * 0.5;
            let at = table.rows[r][c].0;
            let place = Place {
                x0: x + pad,
                hang: x + pad,
                x1: x + w - pad,
                y: y + vpad,
                row_h,
                base,
                align,
                cell: (x, x + w),
                at,
            };
            bottom = bottom.max(flow.place(&mut out, place));
            x += w;
        }
        y = bottom + vpad;
        let color = if r == 0 || r + 1 == flows.len() { theme::faint() } else { theme::line() };
        out.over.push(Rect { x: 0.0, y, w: total, h: 1.0, color, ..Rect::default() });
    }
    out.h = y + (px * 0.6).round();
    out
}

impl Layout {
    /// Row and x position of a source offset.
    pub fn locate(&self, off: u32) -> (usize, f32) {
        if !self.carets.is_empty() {
            // Prefer the leading edge of the following logical cluster at a boundary.
            if let Some(c) = self.clusters.iter().find(|c| c.start == off) {
                return (c.row as usize,c.x+if c.rtl { c.w } else { 0.0 });
            }
            if let Some(&(_,r,x)) = self.carets.iter().find(|c| c.0 == off) { return (r as usize,x); }
        }
        let i = self.clusters.partition_point(|c| c.end <= off);
        match self.clusters.get(i) {
            Some(c) if c.start < off => {
                let frac = (off - c.start) as f32 / (c.end - c.start) as f32;
                (c.row as usize, c.x + c.w * frac)
            }
            Some(c) => (c.row as usize, c.x),
            None => match self.clusters.last() {
                Some(c) => (c.row as usize, c.x + c.w),
                None => (0, self.rows.first().map_or(0.0, |r| r.text_x)),
            },
        }
    }

    pub fn visual_move(&self, off: u32, right: bool) -> Option<u32> {
        if self.carets.is_empty() { return None; }
        let (row,_) = self.locate(off);
        let mut stops: Vec<_> = self.carets.iter().filter(|c| c.1 as usize == row).collect();
        stops.retain(|c| { let (r,x) = self.locate(c.0); r == row && (x-c.2).abs() < 0.01 });
        stops.sort_by(|a,b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        stops.dedup_by(|a,b| a.0 == b.0 && a.2 == b.2);
        let at = stops.iter().position(|c| c.0 == off)?;
        let next = if right { stops.get(at+1) } else { stops.get(at.wrapping_sub(1)) };
        Some(next.map_or(off,|c| c.0))
    }

    /// Contiguous visual pieces of a logical selection, including split bidi runs.
    pub fn selection_spans(&self, row: usize, a: u32, b: u32) -> Vec<(f32,f32)> {
        let r = &self.rows[row];
        let mut spans: Vec<(f32,f32)> = Vec::new();
        for c in &self.clusters[r.first as usize..r.last as usize] {
            if c.start >= b || c.end <= a { continue; }
            let edge = |off: u32| self.carets.iter().find(|s| s.0 == off && s.1 as usize == row && s.2 >= c.x && s.2 <= c.x+c.w).map(|s| s.2);
            let xa = edge(a.max(c.start)).unwrap_or(c.x+if c.rtl { c.w } else { 0.0 });
            let xb = edge(b.min(c.end)).unwrap_or(c.x+if c.rtl { 0.0 } else { c.w });
            let (l,h) = (xa.min(xb),xa.max(xb));
            if let Some(last) = spans.last_mut().filter(|s| (s.1-l).abs() < 0.1) { last.1 = h; }
            else { spans.push((l,h)); }
        }
        spans
    }

    /// The row a point falls on, or the nearest one.
    pub fn row_at(&self, x: f32, y: f32) -> usize {
        let dist = |r: &Row| {
            let dy = (r.y - y).max(y - (r.y + r.h)).max(0.0);
            let dx = (r.x0 - x).max(x - r.x1).max(0.0);
            dy * 1e4 + dx
        };
        (0..self.rows.len()).min_by(|&a, &b| dist(&self.rows[a]).total_cmp(&dist(&self.rows[b]))).unwrap_or(0)
    }

    /// Source offset nearest to `x` within a row.
    pub fn offset_in_row(&self, row: usize, x: f32) -> u32 {
        let Some(r) = self.rows.get(row) else { return 0 };
        if !self.carets.is_empty() {
            let cluster = self.clusters[r.first as usize..r.last as usize].iter().find(|c| x >= c.x && x < c.x+c.w);
            return self.carets.iter().filter(|c| c.1 as usize == row && cluster.is_none_or(|k|
                c.0 >= k.start && c.0 <= k.end && c.2 >= k.x && c.2 <= k.x+k.w))
                .min_by(|a,b| (a.2-x).abs().total_cmp(&(b.2-x).abs())).map_or(r.start,|c| c.0);
        }
        for c in &self.clusters[r.first as usize..r.last as usize] {
            let len = c.end - c.start;
            if c.w > 0.0 && len > 1 && x >= c.x && x < c.x + c.w {
                // Inside a multi-byte cluster such as a ligature.
                return c.start + ((x - c.x) / c.w * len as f32).round() as u32;
            }
            if x < c.x + c.w / 2.0 {
                return c.start;
            }
        }
        self.row_end(row)
    }

    pub fn hit(&self, x: f32, y: f32) -> u32 {
        // A click on an image selects its markdown.
        if let Some(im) = self.images.iter().find(|im| x >= im.x && x < im.x + im.w && y >= im.y && y < im.y + im.h) {
            return im.at;
        }
        self.offset_in_row(self.row_at(x, y), x)
    }

    /// Where End puts the cursor: before the space that wraps the row.
    pub fn row_end(&self, row: usize) -> u32 {
        let r = &self.rows[row];
        match self.clusters[r.first as usize..r.last as usize].last() {
            Some(c) if c.space && row + 1 < self.rows.len() && self.rows[row + 1].y > r.y => c.start,
            _ => r.end,
        }
    }
}
