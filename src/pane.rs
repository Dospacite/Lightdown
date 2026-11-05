//! A text pane: one document with its cursor, scroll position and layout,
//! drawn into a rectangle of the window. The editor, the chat transcript
//! and the chat input are all panes.

mod indentation;
mod file_format;
pub(crate) use file_format::{IndentSource, unit_label};
mod hscroll;
mod minimap;
use minimap::Overview;
mod bookmarks;
mod editing;
mod comparison;
pub(crate) mod vim;
mod merging;
mod folding;
mod headers;
mod excerpts;
mod multiple;
mod reading;
mod prose;
mod tables;
use crate::{ui, font};
use editing::matching;

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::app::{Key, Mods};
use crate::cells::{self, Cell, Tag};
use crate::editor::{self, Buffer, Change, EditKind};
use crate::font::Fonts;
use crate::gfx::Canvas;
use crate::layout::{self, Input, Layout, OutItem};
use crate::md::{self, Kind, Md, PrefixKind, Span};
use crate::pics::Pics;
use crate::{latex, py, theme};

/// How a document is read and shown.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Doc {
    Markdown,
    /// Prose without markup, in the reading font.
    Text,
    /// Any other file, in the monospace font.
    Code,
    Python,
    /// LaTeX source, in the monospace font.
    Latex,
    /// Cells under header lines: a notebook or a chat transcript.
    Cells,
}

/// What a cell shows besides its text.
#[derive(Default)]
pub struct Decor {
    /// Replaces the label in the cell's header line.
    pub label: String,
    pub output: Vec<OutItem>,
    /// Changes whenever the output does.
    pub stamp: u64,
}

/// What a mouse drag extends by: characters, or the word/line first clicked.
enum Drag {
    None,
    Char,
    Unit(usize, usize, bool),
    /// The scrollbar, held this far below the top of its thumb.
    Bar(f32),
    Horizontal(f32),
    Minimap(f32),
    Column(usize),
}

/// Where a row's text ends. A row's own bounds span the whole column.
/// A snippet being filled in: its stops, the one being typed, and how to
/// make what is typed there one step to undo.
#[derive(Debug)]
pub(crate) struct SnippetSession {
    pub stops: Vec<crate::snippets::Stop>,
    pub index: usize,
    /// The undo history's length, the text and the selection when the stop
    /// was entered.
    mark: usize,
    before: Option<String>,
    sel: (usize, usize),
}

/// Where a snippet's range lies after an edit of `pos..end` that now ends
/// at `new_end`. The range being typed in grows; ranges after the edit
/// move; ranges around it grow; ranges it took away close up at its end.
fn snippet_range_after((x, y): (usize, usize), pos: usize, end: usize, new_end: usize, target: bool) -> (usize, usize) {
    let shift = |v: usize| (v as isize + new_end as isize - end as isize).max(0) as usize;
    if target {
        (x, shift(y).max(x))
    } else if x >= end && (x > pos || end == pos) {
        (shift(x), shift(y))
    } else if y <= pos {
        (x, y)
    } else if x <= pos && end <= y {
        (x, shift(y))
    } else if x >= pos && y <= end {
        (new_end, new_end)
    } else {
        (x.min(pos), shift(y.max(end)).max(new_end))
    }
}

/// Where a Find in selection range lies after an edit: moved by edits
/// before it, grown or shrunk by edits in it, and stretched over an edit
/// that crosses one of its ends.
fn find_scope_after(a: usize, b: usize, c: &crate::editor::Change, delta: isize) -> (usize, usize) {
    let (pos, end) = (c.pos, c.pos + c.del);
    let ins = (c.del as isize + delta).max(0) as usize;
    let shift = |x: usize| (x as isize + delta).max(0) as usize;
    if end <= a && !(pos == a && c.del == 0 && a < b) {
        (shift(a), shift(b))
    } else if pos >= a && end <= b {
        (a, shift(b))
    } else if pos > b {
        (a, b)
    } else {
        (a.min(pos), shift(b.max(end)).max(pos + ins))
    }
}

fn text_end(lay: &crate::layout::Layout, row: &crate::layout::Row) -> f32 {
    let clusters = &lay.clusters[row.first as usize..(row.last as usize).min(lay.clusters.len())];
    clusters.iter().map(|k| k.x + k.w).fold(row.text_x, f32::max)
}

/// Where a second view of a document stands: its cursor and selection,
/// and the start of its top line with how far into it the view begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mirror {
    cur: usize,
    anchor: usize,
    top_at: usize,
    top_off: f32,
    horizontal: f32,
}

pub struct Pane {
    pub(super) vim: vim::Vim,
    pub buf: Buffer,
    pub doc: Doc,
    md: Option<Md>,
    /// The cells of a `Doc::Cells` document, in order. Never empty there.
    pub cells: Vec<Cell>,
    pub decor: HashMap<u64, Decor>,
    next_id: u64,
    /// The file shown, which relative links and images are resolved from.
    pub path: Option<PathBuf>,
    pub read_only: bool,
    /// The document must begin with a cell header, as a notebook does.
    pub headed: bool,
    /// Source line numbers, enabled only for document panes.
    pub numbers: bool,
    pub(crate) git_marks: Vec<(usize, crate::app::repository::Mark)>,
    pub(crate) diff: bool,
    pub(crate) comparison: Option<crate::diff::View>,
    /// A three-way merge shown above the text, which is its result.
    pub(crate) merge: Option<crate::merge::Merge>,
    pub(crate) output_name: Option<String>,
    pub(crate) blame: Option<(usize, String)>,
    pub(crate) file_blame: Vec<(String, String)>,
    /// Review comments shown after their lines, counted from 0.
    pub(crate) review_notes: Vec<(usize, String)>,
    pub edit_mode: bool,
    /// Whether the caret is drawn.
    pub focused: bool,
    pub(crate) focus_paragraph: bool,
    pub(crate) typewriter: bool,
    /// The cursor moved before the pane had a size; it is shown once it has.
    reveal_later: bool,
    /// A second view of this document: its own cursor, selection and scroll.
    pub(crate) mirror: Option<Mirror>,
    /// Excerpts of other files, when the pane is a multibuffer.
    pub(crate) multi: Option<Box<crate::multibuffer::Multi>>,
    /// The family prose is set in: `font::SERIF` or `font::SANS`.
    pub family: u8,
    /// A conversation: the user's messages lie in bubbles and the agent's
    /// work is set apart from what it says.
    pub chat: bool,
    /// Matches of a search, marked under the text.
    pub found: Vec<(usize, usize)>,
    /// Where Find in selection limits matches. It moves with edits, and
    /// shows as a faint range while `show_find_scope` is set.
    pub find_scope: Option<(usize, usize)>,
    pub show_find_scope: bool,
    occurrence_key: Option<(u64, (usize, usize))>,
    occurrences: Vec<(usize, usize)>,
    inferred_indent: Option<String>,
    pub(crate) bookmarks: Vec<usize>,
    /// The bookmarks changed since they were last stored.
    pub(crate) bookmarks_changed: bool,
    pub(crate) word_wrap: bool,
    pub(crate) minimap: String,
    pub(crate) minimap_size: f32,
    overview: Overview,
    /// How far unwrapped text is scrolled left; see `hscroll`.
    horizontal: f32,
    /// The widest unwrapped line drawn so far, and a scroll the caret asked
    /// for before its line was measured.
    hwidest: f32,
    horizontal_floor: f32,
    hbar_hot: bool,
    pub(crate) file_indent: Option<String>,
    pub(crate) indent_guides: bool,
    pub(crate) ruler: usize,
    pub(crate) test_marks: Vec<(usize, usize, String)>,
    /// Failed tests' messages at the lines they failed on, from 0, sorted.
    pub(crate) test_notes: Vec<(usize, String)>,
    /// Lines a coverage report measured, counted from 0, and whether each ran.
    pub(crate) coverage_marks: Vec<(usize, bool)>,
    pub(crate) test_request: Option<usize>,
    pub(crate) breakpoints: Vec<usize>,
    /// Paused values at line ends, and the exception stopped on.
    pub(crate) debug_values: Vec<(usize, String)>,
    pub(crate) debug_exception: Option<(usize, String)>,
    pub(crate) debug_line: Option<usize>,
    pub(crate) snippet: Option<SnippetSession>,
    /// Set while mirrors of a snippet stop are being written.
    snippet_mirroring: bool,
    pub(crate) extra_cursors: Vec<(usize, usize)>,
    pub(crate) folds: Vec<(usize, usize)>,
    pub(crate) sticky: bool,
    sticky_cache: Option<(u64,Vec<(usize,usize,String)>)>,
    sticky_hits: Vec<((f32,f32,f32,f32),usize)>,
    auto_pairs: Vec<(usize, char)>,
    pair_text_len: usize,
    fonts: Rc<RefCell<Fonts>>,
    pics: Rc<RefCell<Pics>>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    scale: f32,
    pub zoom: f32,
    /// Body text size in logical pixels at zoom 1.
    pub size: f32,
    /// Space between the pane's edges and its text, in logical pixels.
    pub margin: f32,
    /// Use the whole width rather than a reading column.
    pub fill: bool,
    /// Leave room above the first line.
    pub lead: bool,
    /// How much of the view the end of the document must keep covered.
    pub keep: f32,
    /// Stay at the end as the text grows, unless scrolled away from it.
    pub follow: bool,
    at_tail: bool,
    top: usize,
    top_off: f32,
    cache: Vec<Option<Rc<Layout>>>,
    empty: Rc<Layout>,
    attrs: Vec<crate::md::Attr>,
    /// The offset that decides which syntax is revealed. Follows the cursor,
    /// except while the mouse is dragging, so text does not shift under it.
    focus: usize,
    drag: Drag,
    goal_x: Option<f32>,
    /// A suggestion shown where it would go, as if typed but dimmed. Its
    /// text is in the buffer for as long as it shows, and no longer.
    ghost: Option<(usize, usize)>,
    prediction: Option<(usize, usize, bool)>,
    pub(crate) visible: Vec<(usize, f32)>,
    /// Words marked as misspelt, by where they are, in order.
    pub(crate) diagrams: Vec<(usize, PathBuf)>,
    pub(crate) broken_links: Vec<(usize, usize, String)>,
    pub(crate) diagnostics: Vec<(usize, usize, String)>,
    pub misspelled: Vec<(usize, usize)>,
    /// Typesets formulas, where mathematics is drawn rather than shown as
    /// source.
    pub maths: Option<Rc<RefCell<crate::math::Maths>>>,
    /// The language a notebook's code cells are highlighted as: its kernel's.
    pub cell_language: String,
    /// Where the scrollbar's thumb was last drawn, its top and length, and
    /// how many lines the view held; `None` while everything fits.
    bar: Option<(f32, f32, usize)>,
    /// The pointer is over the scrollbar.
    bar_hot: bool,
    // Requests for the owner, cleared by it once served.
    pub redraw: bool,
    pub copy: Option<String>,
    pub note: Option<String>,
}

impl Pane {
    pub fn new(fonts: Rc<RefCell<Fonts>>, pics: Rc<RefCell<Pics>>) -> Pane {
        Pane {
            vim: vim::Vim::default(),
            buf: Buffer::new(String::new()),
            doc: Doc::Markdown,
            md: None,
            cells: Vec::new(),
            decor: HashMap::new(),
            next_id: 0,
            path: None,
            read_only: false,
            headed: false,
            numbers: false,
            git_marks: Vec::new(),
            diff: false,
            comparison: None,
            merge: None,
            output_name: None,
            blame: None,
            file_blame: Vec::new(),
            review_notes: Vec::new(),
            edit_mode: false,
            focused: true,
            focus_paragraph: false,
            typewriter: false,
            reveal_later: false,
            mirror: None,
            multi: None,
            family: crate::font::SANS,
            chat: false,
            found: Vec::new(),
            find_scope: None,
            show_find_scope: false,
            occurrence_key: None,
            occurrences: Vec::new(),
            inferred_indent: None,
            bookmarks: Vec::new(),
            bookmarks_changed: false,
            word_wrap: true,
            minimap: "auto".into(),
            minimap_size: 72.0,
            overview: Overview::default(),
            horizontal: 0.0,
            hwidest: 0.0,
            horizontal_floor: 0.0,
            hbar_hot: false,
            file_indent: None,
            indent_guides: true,
            ruler: 80,
            test_marks: Vec::new(),
            test_notes: Vec::new(),
            coverage_marks: Vec::new(),
            test_request: None,
            breakpoints: Vec::new(),
            debug_values: Vec::new(),
            debug_exception: None,
            debug_line: None,
            snippet: None,
            snippet_mirroring: false,
            extra_cursors: Vec::new(),
            folds: Vec::new(),
            sticky: true,
            sticky_cache: None,
            sticky_hits: Vec::new(),
            auto_pairs: Vec::new(),
            pair_text_len: 0,
            fonts,
            pics,
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            scale: 1.0,
            zoom: 1.0,
            size: theme::BODY,
            margin: 28.0,
            fill: false,
            lead: true,
            keep: 0.4,
            follow: false,
            at_tail: true,
            top: 0,
            top_off: 0.0,
            cache: Vec::new(),
            empty: Rc::new(Layout::default()),
            attrs: Vec::new(),
            focus: 0,
            drag: Drag::None,
            goal_x: None,
            ghost: None,
            prediction: None,
            visible: Vec::new(),
            maths: None,
            cell_language: "python".into(),
            misspelled: Vec::new(),
            diagrams: Vec::new(),
            broken_links: Vec::new(),
            diagnostics: Vec::new(),
            bar: None,
            bar_hot: false,
            redraw: true,
            copy: None,
            note: None,
        }
    }

    /// Where this view stands, to keep for a second view of the document.
    fn view_now(&self) -> Mirror {
        let top_at = self.buf.starts.get(self.top).copied().unwrap_or(0);
        Mirror { cur: self.buf.cur, anchor: self.buf.anchor, top_at, top_off: self.top_off, horizontal: self.horizontal }
    }

    /// Opens a second view of the document where this one stands.
    pub(crate) fn mirror_open(&mut self) {
        self.mirror = Some(self.view_now());
    }

    /// Trades this view's cursor, selection and scroll for the second
    /// view's. Positions are kept within the text and on character starts.
    pub(crate) fn mirror_swap(&mut self) {
        let Some(m) = self.mirror else { return };
        let now = self.view_now();
        let fix = |at: usize| editor::floor(&self.buf.text, at.min(self.buf.text.len()));
        let (cur, anchor, top_at) = (fix(m.cur), fix(m.anchor), fix(m.top_at));
        self.extra_cursors.clear();
        (self.buf.cur, self.buf.anchor) = (cur, anchor);
        self.top = self.buf.line_of(top_at);
        self.top_off = if m.top_off.is_finite() { m.top_off.max(0.0) } else { 0.0 };
        self.horizontal = m.horizontal;
        self.mirror = Some(now);
    }

    /// Replaces the document.
    pub(crate) fn scroll_position(&self) -> (usize, f32) {
        (self.top, self.top_off)
    }
    pub(crate) fn restore_scroll(&mut self, position: (usize, f32)) {
        (self.top, self.top_off) = (position.0.min(self.buf.lines() - 1), if position.1.is_finite() { position.1.max(0.0) } else { 0.0 });
        self.clamp_scroll();
    }

    pub fn set_text(&mut self, text: String, doc: Doc) {
        self.doc = doc;
        self.auto_pairs.clear();
        self.extra_cursors.clear();
        self.folds.clear();
        self.sticky_cache = None;
        self.sticky_hits.clear();
        self.snippet = None;
        self.ghost = None;
        self.drag = Drag::None;
        self.found.clear();
        self.git_marks.clear();
        self.test_marks.clear();
        self.test_notes.clear();
        self.coverage_marks.clear();
        self.test_request = None;
        self.blame = None;
        self.file_blame.clear();
        self.diff = false;
        self.comparison = None;
        self.output_name = None;
        self.misspelled.clear();
        self.diagnostics.clear();
        self.broken_links.clear();
        self.diagrams.clear();
        self.pair_text_len = text.len();
        self.inferred_indent = indentation::detect(&text);
        self.file_indent = None;
        self.bookmarks.clear();
        (self.horizontal, self.hwidest, self.horizontal_floor) = (0.0, 0.0, 0.0);
        self.buf = Buffer::new(text);
        self.cache.clear();
        self.cache.resize(self.buf.lines(), None);
        self.rescan();
        self.reparse();
        (self.top, self.top_off, self.focus, self.goal_x) = (0, 0.0, 0, None);
        self.buf.cur = self.snap(0, true);
        self.buf.anchor = self.buf.cur;
        self.redraw = true;
    }

    /// Changes how the same text is read, as after saving under a new name.
    pub fn set_doc(&mut self, doc: Doc) {
        self.doc = doc;
        self.rescan();
        self.reparse();
        self.redraw = true;
    }

    /// Makes the text equal to `text`, outside the undo history.
    pub fn sync(&mut self, text: &str) {
        if let Some(c) = self.buf.sync(text) {
            self.changed(c);
            self.focus = self.focus.min(self.buf.text.len());
            self.redraw = true;
        }
    }

    pub(crate) fn reparse(&mut self) {
        self.sticky_cache = None;
        let (text, starts) = (&self.buf.text, &self.buf.starts);
        self.md = match self.doc {
            Doc::Markdown => {
                let md = md::parse(text, starts);
                let _ = crate::treesitter::spans(text, "markdown");
                Some(md)
            },
            Doc::Python => {
                let mut md = Md::plain(starts);
                crate::syntax::spans(text, 0, "python", &mut md.spans);
                Some(md)
            }
            Doc::Latex => {
                let mut md = Md::plain(starts);
                latex::spans(text, 0, &mut md.spans);
                Some(md)
            }
            Doc::Cells => Some(self.read_cells()),
            Doc::Code => {
                let mut md = Md::plain(starts);
                let language = self.path.as_deref().and_then(|p| p.extension()).and_then(|s| s.to_str()).unwrap_or("");
                crate::syntax::spans(text, 0, language, &mut md.spans);
                Some(md)
            }
            Doc::Text => None,
        };
    }

    /// Analyses each cell by its own rules: markdown for prose, Python for
    /// code, a single style for the rest.
    fn read_cells(&self) -> Md {
        let (text, starts) = (&self.buf.text, &self.buf.starts);
        let mut md = Md::plain(starts);
        for (i, cell) in self.cells.iter().enumerate() {
            let first = cell.line + cell.headed as usize;
            let end = self.cells.get(i + 1).map_or(starts.len(), |c| c.line);
            if cell.headed {
                md.lines[cell.line].kind = Kind::Cell;
            }
            if first >= end {
                continue;
            }
            let (s, e) = (starts[first], self.buf.line(end - 1).1);
            let all = |attr| Span { start: s as u32, end: e as u32, attr };
            match cell.tag {
                tag if tag.prose() => {
                    let local: Vec<usize> = starts[first..end].iter().map(|o| o - s).collect();
                    md.overlay(md::parse(&text[s..e], &local), s, first);
                }
                Tag::Code | Tag::Raw => {
                    for line in &mut md.lines[first..end] {
                        line.kind = Kind::Code;
                    }
                    if cell.tag == Tag::Code && matches!(self.cell_language.as_str(), "python" | "") {
                        py::spans(&text[s..e], s, &mut md.spans);
                    } else if let Some(language) = self.excerpt_language(i) {
                        crate::syntax::spans(&text[s..e], s, &language, &mut md.spans);
                    } else if cell.tag == Tag::Code {
                        crate::syntax::spans(&text[s..e], s, &self.cell_language, &mut md.spans);
                    }
                }
                Tag::Think => md.spans.push(all(md::DIM | md::ITALIC)),
                Tag::Tool => md.spans.push(all(md::DIM)),
                _ => md.spans.push(all(md::ITALIC)),
            }
        }
        md
    }

    // ---- cells ----------------------------------------------------------

    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The cell a line starts, if any. Text before the first header is a
    /// code cell without one.
    fn cell_start(&self, line: usize) -> Option<(Tag, bool)> {
        let (s, e) = self.buf.line(line);
        match cells::parse(&self.buf.text[s..e]) {
            Some((tag, _)) => Some((tag, true)),
            None => (line == 0).then_some((Tag::Code, false)),
        }
    }

    fn rescan(&mut self) {
        self.cells.clear();
        if self.doc != Doc::Cells {
            return;
        }
        for line in 0..self.buf.lines() {
            if let Some((tag, headed)) = self.cell_start(line) {
                let id = self.fresh_id();
                self.cells.push(Cell { line, tag, headed, id });
            }
        }
    }

    /// Brings the list of cells up to date after an edit. A cell whose
    /// header the edit left alone keeps its identity, wherever it moved;
    /// headers the edit wrote take over from those it removed, in order.
    fn retrack(&mut self, c: &Change) {
        let old = std::mem::take(&mut self.cells);
        let (a, old_end, new_end) = (c.line, c.line + c.removed, c.line + c.added);
        // Text put in at the very start of a line pushes that line down whole.
        let pushed = c.del == 0 && c.pos == self.buf.starts[a];
        let (mut kept, mut lost, mut moved) = (Vec::new(), Vec::new(), Vec::new());
        for mut cell in old {
            let l = cell.line;
            let whole_after = l > old_end || (l == old_end && if c.removed > 0 { c.ends_line } else { pushed });
            // A header the edit began inside stays where it is, but may
            // say something else now.
            let reread =
                if l == a && cell.headed && !pushed && c.pos > self.buf.starts[a] { self.cell_start(a) } else { None };
            if let Some((tag, headed)) = reread {
                kept.push(Cell { tag, headed, ..cell });
            } else if l < a {
                kept.push(cell);
            } else if cell.headed && whole_after {
                cell.line = l + c.added - c.removed;
                moved.push(cell);
            } else {
                lost.push(cell.id);
            }
        }
        let mut lost = lost.into_iter();
        let taken = |cells: &[Cell], line: usize| cells.iter().any(|cell| cell.line == line);
        for line in a..=new_end.min(self.buf.lines() - 1) {
            if taken(&kept, line) || taken(&moved, line) {
                continue;
            }
            if let Some((tag, headed)) = self.cell_start(line) {
                let id = lost.next().unwrap_or_else(|| self.fresh_id());
                kept.push(Cell { line, tag, headed, id });
            }
        }
        kept.append(&mut moved);
        self.cells = kept;
    }

    /// Index of the cell holding an offset.
    pub fn cell_at(&self, off: usize) -> usize {
        let line = self.buf.line_of(off.min(self.buf.text.len()));
        self.cells.partition_point(|c| c.line <= line).saturating_sub(1)
    }

    /// The lines of a cell's text: first, and one past the last.
    fn cell_lines(&self, i: usize) -> (usize, usize) {
        let first = self.cells[i].line + self.cells[i].headed as usize;
        (first, self.cells.get(i + 1).map_or(self.buf.lines(), |c| c.line).max(first))
    }

    /// The byte range of a cell's text, without its header.
    pub fn cell_body(&self, i: usize) -> (usize, usize) {
        let (first, end) = self.cell_lines(i);
        if first >= end || first >= self.buf.lines() {
            let at = self.buf.text.len();
            return (at, at);
        }
        (self.buf.starts[first], self.buf.line(end - 1).1)
    }

    /// The byte range of a whole cell: header, text and the newline after.
    pub fn cell_span(&self, i: usize) -> (usize, usize) {
        let start = self.buf.starts[self.cells[i].line];
        let end = self.cells.get(i + 1).map_or(self.buf.text.len(), |c| self.buf.starts[c.line]);
        (start, end)
    }

    /// Whether an edit leaves every header line whole.
    fn keeps_cells(&self, start: usize, end: usize, ins: &str) -> bool {
        let t = &self.buf.text;
        let (l0, l1) = (self.buf.line_of(start), self.buf.line_of(end));
        let (s0, (s1, e1)) = (self.buf.starts[l0], self.buf.line(l1));
        // A header may go whole or stay whole, but not be cut.
        if cells::parse(&t[s1..e1]).is_some() && end > s1 && end < e1 {
            return false;
        }
        let joined = format!("{}{ins}{}", &t[s0..start], &t[end..e1]);
        // A header needs a line after it for its cell's text.
        let ends_bare = e1 == t.len() && cells::parse(joined.rsplit('\n').next().unwrap_or("")).is_some();
        cells::well_formed(&joined) && !ends_bare
    }

    /// Moves an offset off a header line: on to the cell's text, or back to
    /// the end of the cell before.
    fn snap(&self, mut off: usize, forward: bool) -> usize {
        if self.doc != Doc::Cells {
            return off;
        }
        for _ in 0..self.cells.len() + 1 {
            let (s, e) = self.buf.line(self.buf.line_of(off));
            if !self.buf.text[s..].starts_with(cells::MARK) {
                break;
            }
            off = if forward || s == 0 { (e + 1).min(self.buf.text.len()) } else { s - 1 };
        }
        off
    }

    /// Markdown shortcuts apply at this offset.
    pub fn prose_at(&self, off: usize) -> bool {
        match self.doc {
            Doc::Markdown => true,
            Doc::Cells => self.cells.get(self.cell_at(off)).is_some_and(|c| c.tag.prose()),
            _ => false,
        }
    }

    /// Inline formatting is appropriate in prose, outside code and metadata blocks.
    pub fn formatting_at(&self, off: usize) -> bool {
        self.prose_at(off)
            && self.md.as_ref().is_some_and(|md| {
                !matches!(
                    md.lines[self.buf.line_of(off)].kind,
                    Kind::Code | Kind::FenceOpen | Kind::FenceClose | Kind::Meta | Kind::Html
                )
            })
    }

    /// Whether the document is shown in the monospace font.
    fn mono(&self) -> bool {
        matches!(self.doc, Doc::Code | Doc::Python | Doc::Latex)
    }

    /// Python editing applies at this offset.
    pub fn python_at(&self, off: usize) -> bool {
        match self.doc {
            Doc::Python => true,
            Doc::Cells => self.cells.get(self.cell_at(off)).is_some_and(|c| c.tag == Tag::Code),
            _ => false,
        }
    }

    // ---- geometry -------------------------------------------------------

    pub(crate) fn bounds(&self) -> (f32,f32,f32,f32) { (self.x,self.y,self.w,self.h) }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) {
        let (x, y, w, h) = self.merge_place(x, y, w, h, scale);
        (self.x, self.y, self.w, self.h, self.scale) = (x, y, w, h, scale);
        if self.reveal_later && w > 0.0 && h > 0.0 {
            self.reveal_cursor();
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        if let Some(m) = &self.merge { return crate::ui::inside(m.full, Some((x, y))); }
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn px(&self) -> f32 {
        (self.size * self.zoom * self.scale).round()
    }

    /// The whole-file blame column's width, at the far left of the gutter.
    pub(crate) fn blame_width(&self) -> f32 {
        if self.file_blame.is_empty() { 0.0 } else { (self.w * 0.3).min(210.0 * self.scale).round() }
    }

    /// Where the whole-file blame column lies, when it is shown.
    pub(crate) fn blame_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let w = self.blame_width();
        (w > 0.0).then(|| (self.column().0 - self.gutter(), self.y, w, self.h))
    }

    /// One line's part of the blame column: a rule and the commit's label on
    /// the first line of each run of lines from one commit.
    /// `text` and `row` are the top and height of the line's first row.
    fn draw_file_blame(&self, c: &mut Canvas, line: usize, y: f32, height: f32, text: f32, row: f32) {
        let Some((x, top, width, tall)) = self.blame_rect() else { return };
        let Some((id, label)) = self.file_blame.get(line) else { return };
        let starts = line == 0 || self.file_blame.get(line - 1).is_none_or(|(prev, _)| prev != id);
        if !starts && line != self.top {
            return;
        }
        let y0 = y.max(top);
        let bottom = (y + height).min(top + tall);
        if bottom <= y0 {
            return;
        }
        if starts && line > 0 {
            c.rect(x, y.round(), width - 4.0 * self.scale, 1.0, theme::line());
        }
        c.clip(x, y0, width - 6.0 * self.scale, bottom - y0);
        let mut fonts = self.fonts.borrow_mut();
        let size = theme::SMALL * self.scale;
        let uncommitted = id.bytes().all(|b| b == b'0');
        let ink = if uncommitted { theme::warning() } else { theme::faint() };
        let label = ui::fit(&mut fonts, font::UI, size, label, width - 12.0 * self.scale);
        let base = text + row / 2.0 + size * 0.35;
        ui::text(c, &mut fonts, x + 4.0 * self.scale, base.round(), &label, font::UI, size, ink);
        // Back to the pane's own clip.
        c.clip(self.x, self.y, self.w, self.h);
    }

    /// Width of everything left of the text: the blame column and the marks.
    pub(crate) fn gutter(&self) -> f32 {
        self.blame_width() + self.marks_gutter()
    }

    /// Width of the line numbers and the marks beside them.
    pub(crate) fn marks_gutter(&self) -> f32 {
        if !self.numbers {
            return 0.0;
        }
        let widest = self.multi.as_ref().map_or(self.buf.lines(), |m| m.widest().max(self.buf.lines()));
        let digits = widest.max(1).ilog10() + 1;
        (self.px() * (digits.max(2) as f32 * 0.6 + 1.0 + if self.test_marks.is_empty() { 0.0 } else { 1.0 })).round()
    }

    pub(crate) fn column(&self) -> (f32, f32) {
        let margin = (self.margin * self.scale).round();
        let gutter = self.gutter();
        let view_width = self.w - self.minimap_width();
        let room = (view_width - margin * 2.0 - gutter).max(1.0);
        let wide = self.fill || self.mono();
        let expanded = self.numbers && matches!(self.doc, Doc::Markdown | Doc::Cells);
        let measure = theme::COLUMN * if expanded { 1.5 } else { 1.0 };
        let cw = if wide { room } else { room.min(measure * self.px()) }.round();
        (self.x + ((view_width - cw - gutter) / 2.0).round() + gutter, cw)
    }

    /// Visible notebook header rectangles, clipped to the document pane.
    pub(crate) fn cell_headers(&self) -> Vec<(usize, (f32, f32, f32, f32))> {
        if !self.headed || self.w <= 0.0 || self.h <= 0.0 {
            return Vec::new();
        }
        let (x, w) = self.column();
        self.visible
            .iter()
            .filter_map(|&(li, y)| {
                let ci = self.cells.binary_search_by_key(&li, |c| c.line).ok()?;
                let h = self.cache.get(li)?.as_ref()?.h;
                let top = y.max(self.y);
                let bottom = (y + h).min(self.y + self.h);
                (bottom > top).then_some((ci, (x, top, w, bottom - top)))
            })
            .collect()
    }

    fn source_number(&self, li: usize) -> Option<usize> {
        if self.multi.is_some() {
            return self.excerpt_number(li);
        }
        if self.headed && self.doc == Doc::Cells {
            let ci = self.cells.partition_point(|c| c.line <= li).saturating_sub(1);
            let header = self.cells[ci].line;
            (li > header).then_some(li - header)
        } else {
            Some(li + 1)
        }
    }

    fn view(&self) -> (f32, f32) {
        (self.y, self.y + self.h)
    }

    /// Height of the whole document, counted no further than `max`.
    pub fn content_height(&mut self, max: f32) -> f32 {
        let mut h = 0.0;
        for li in 0..self.buf.lines() {
            h += self.height(li);
            if h >= max {
                return max;
            }
        }
        h
    }

    /// Where the caret is on screen: x, top and height.
    pub fn caret(&mut self) -> Option<(f32, f32, f32)> {
        self.spot(self.buf.cur)
    }

    /// Where an offset is on screen: x, top and height of its row.
    pub fn spot(&mut self, off: usize) -> Option<(f32, f32, f32)> {
        let cl = self.buf.line_of(off);
        let top = self.line_top(cl)?;
        let lay = self.layout(cl);
        let (row, x) = lay.locate((off - self.buf.starts[cl]) as u32);
        let r = lay.rows.get(row)?;
        Some((self.text_screen_x(x), self.y + top + r.y, r.h))
    }

    /// The Python name under a point, when the point is on its letters
    /// rather than merely nearest to them. Comments and strings hold none.
    pub fn name_under(&mut self, x: f32, y: f32) -> Option<(usize, usize)> {
        if !self.contains(x, y) || self.on_bar(x, y) || !matches!(self.drag, Drag::None) {
            return None;
        }
        let off = self.offset_at(x, y);
        if matches!(self.doc, Doc::Markdown | Doc::Latex) && let Some((a, b, _)) = crate::citations::at(&self.buf.text, off) { return Some((a, b)); }
        let (s, e) = crate::py::name_at(&self.buf.text, off)?;
        if !(self.python_at(s) || self.doc == Doc::Code) || self.attr_at(s) & (crate::md::DIM | crate::md::SOFT) != 0 {
            return None;
        }
        let (sx, top, h) = self.spot(s)?;
        let (ex, etop, _) = self.spot(e)?;
        (etop == top && x >= sx && x < ex && y >= top && y < top + h).then_some((s, e))
    }

    /// The style bits of the byte at an offset.
    /// The words in view worth checking the spelling of: those of prose,
    /// leaving out code, links, LaTeX's commands and mathematics, and the
    /// word being typed at the cursor.
    pub fn spell_words(&self) -> Vec<(usize, usize)> {
        /// LaTeX commands whose argument is a name, not prose.
        const NAMES: [&str; 17] = [
            "label",
            "ref",
            "eqref",
            "autoref",
            "cref",
            "Cref",
            "pageref",
            "cite",
            "citep",
            "citet",
            "usepackage",
            "documentclass",
            "includegraphics",
            "input",
            "include",
            "bibliography",
            "bibliographystyle",
        ];
        let mut out = Vec::new();
        let mut attrs = Vec::new();
        for &(li, _) in &self.visible {
            let (ls, le) = self.buf.line(li);
            let line = &self.buf.text[ls..le];
            let skip: crate::md::Attr = match self.doc {
                Doc::Text => 0,
                Doc::Latex => md::KEYWORD | md::SOFT | md::DIM,
                Doc::Markdown | Doc::Cells if self.prose_at(ls) => {
                    let Some(md) = &self.md else { continue };
                    let kind = md.lines.get(li).map(|l| l.kind);
                    if matches!(
                        kind,
                        Some(Kind::Code | Kind::FenceOpen | Kind::FenceClose | Kind::Meta | Kind::Html | Kind::Cell)
                    ) {
                        continue;
                    }
                    md::CODE | md::LINK
                }
                _ => continue,
            };
            let mut masked = line.as_bytes().to_vec();
            if skip != 0
                && let Some(md) = &self.md
            {
                md.attrs(ls, le, usize::MAX, true, &mut attrs);
                for (i, a) in attrs.iter().enumerate() {
                    if a & skip != 0 {
                        masked[i] = b' ';
                    }
                }
            }
            if self.doc == Doc::Latex {
                for name in NAMES {
                    let mut from = 0;
                    while let Some(i) = line[from..].find(&format!("\\{name}")).map(|i| i + from) {
                        let open = line[i..].find('{').map(|j| i + j);
                        let close = open.and_then(|o| line[o..].find('}').map(|j| o + j + 1)).unwrap_or(line.len());
                        masked[i..close].fill(b' ');
                        from = close.max(i + 1);
                    }
                }
            }
            // Bytes of characters cut into are blanked whole, so the text stays text.
            for (i, ch) in line.char_indices() {
                if masked[i..i + ch.len_utf8()].contains(&b' ') && ch != ' ' {
                    masked[i..i + ch.len_utf8()].fill(b' ');
                }
            }
            let masked = String::from_utf8(masked).unwrap_or_default();
            crate::spell::words(&masked, ls, &mut out);
        }
        let cur = self.buf.cur;
        out.retain(|&(_, b)| b != cur);
        out
    }

    /// Lays extra spans, such as a language server's semantic tokens, over
    /// the base highlighting. They last until the text is parsed again.
    pub(crate) fn overlay_spans(&mut self, spans: &[Span]) -> bool {
        let Some(md) = &mut self.md else { return false };
        md.spans.extend_from_slice(spans);
        md.spans.sort_by_key(|s| s.start);
        self.redraw = true;
        true
    }

    /// How many highlight spans the text has; it changes when the text is parsed again.
    pub(crate) fn span_count(&self) -> usize {
        self.md.as_ref().map_or(0, |md| md.spans.len())
    }

    pub fn attr_at(&self, off: usize) -> crate::md::Attr {
        let Some(md) = &self.md else { return 0 };
        let covers = |s: &&Span| (s.start as usize) <= off && off < s.end as usize;
        md.spans.iter().filter(covers).fold(0, |a, s| a | s.attr)
    }

    pub fn link_at(&self, x: f32, y: f32) -> Option<&str> {
        let off = self.offset_at(x, y);
        self.md.as_ref().and_then(|md| md.link_at(off)).map(|l| l.url.as_str())
    }

    // ---- layout ---------------------------------------------------------

    pub(crate) fn layout(&mut self, li: usize) -> Rc<Layout> {
        self.layout_for(li, self.focus)
    }

    /// Layout of a line as it looks with the cursor at `focus`. Cached, and
    /// validated by a signature of everything that feeds it, so no explicit
    /// invalidation is ever needed.
    fn layout_for(&mut self, li: usize, focus: usize) -> Rc<Layout> {
        if self.fold_hidden(li) {
            return self.empty.clone();
        }
        let (ls, le) = self.buf.line(li);
        let (px, cw, wrap) = (self.px(), self.column().1, self.wraps());
        let doc = self.buf.text.as_str();
        let (mut start, mut end) = (ls, le);
        let (mut kind, mut prefix, mut content) = (Kind::Text, &[][..], ls);
        let (mut line_active, mut block_active, mut table) = (false, false, None);
        let (mut label, mut tag, mut output, mut stamp) = ("", "", &[][..], 0);
        let mut talk = None;
        let mut images = Vec::new();
        let mut maths_in: Vec<layout::MathBox> = Vec::new();
        let numbered_label;
        let excerpt_label;
        self.attrs.clear();
        if let Some(md) = &self.md {
            let info = &md.lines[li];
            (kind, prefix, content) = (info.kind, &info.prefix[..], info.content as usize);
            line_active = self.edit_mode || (ls <= focus && focus <= le);
            block_active = self.edit_mode || md.block_active(info.block, focus);
            if kind == Kind::Table && !block_active {
                // A rendered table hangs off its first line; the rest vanish.
                let block = &md.blocks[info.block.unwrap() as usize];
                if block.start as usize != ls {
                    return self.empty.clone();
                }
                (start, end, prefix, content) = (ls, block.end as usize, &[][..], ls);
                table = block.table.map(|t| &md.tables[t as usize]);
            }
            md.attrs(start, end, focus, self.edit_mode, &mut self.attrs);
            if let Some((gs, ge)) = self.ghost.filter(|g| g.0 < end && g.1 > start) {
                self.attrs[gs.max(start) - start..ge.min(end) - start].fill(md::DIM | md::ITALIC);
            }
            // Images that can be shown go under the line; their markdown is
            // hidden unless the cursor is on the line.
            let shows = table.is_none() && matches!(kind, Kind::Text | Kind::Heading(_));
            let from = md.images.partition_point(|i| (i.start as usize) < start);
            for img in md.images[from..].iter().take_while(|i| shows && (i.start as usize) < end) {
                let Some(path) = image_file(self.path.as_deref(), &img.url) else { continue };
                let Some((w, h)) = self.pics.borrow_mut().size(&path) else { continue };
                let (a, b) = (img.start as usize - start, (img.end as usize).min(end) - start);
                if !line_active {
                    self.attrs[a..b].fill(md::HIDDEN);
                }
                let scale = px / theme::BODY;
                let (w, h) = (w as f32 * scale, h as f32 * scale);
                images.push(layout::Image { at: a as u32, w, h, path, center: false });
            }
            // Formulas TeX has set stand in for their source, unless the
            // cursor is in them. A displayed one is centred under its first
            // line, and the rest of its lines fold away.
            if let Some(maths) = self.maths.clone().filter(|_| !self.edit_mode && table.is_none()) {
                let mut maths = maths.borrow_mut();
                let fits = !matches!(kind, Kind::Code | Kind::Meta | Kind::FenceOpen | Kind::FenceClose | Kind::Html);
                // TeX's ten points to the body text's size.
                let k = px / 9.5;
                let from = md.maths.partition_point(|m| (m.end as usize) < start);
                for m in md.maths[from..].iter().take_while(|m| fits && (m.start as usize) <= end) {
                    let (ms, me) = (m.start as usize, m.end as usize);
                    if ms <= focus && focus <= me {
                        continue;
                    }
                    let Some(f) = maths.get(&m.tex, m.display) else { continue };
                    if m.display && ms < start {
                        return self.empty.clone();
                    }
                    if m.display {
                        self.attrs[ms - start..me.min(end) - start].fill(md::HIDDEN);
                        let (w, h, path) = (f.w * k, f.h * k, f.key.clone());
                        images.push(layout::Image { at: (ms - start) as u32, w, h, path, center: true });
                    } else if ms >= start && me <= end {
                        // A tall formula is made to fit the line.
                        let fit = (px * 1.7 / (f.h * k)).min(1.0);
                        let (w, h, ascent) = (f.w * k * fit, f.h * k * fit, f.ascent * k * fit);
                        let (s0, e0) = ((ms - start) as u32, (me - start) as u32);
                        maths_in.push(layout::MathBox { start: s0, end: e0, w, h, ascent, path: f.key.clone() });
                    }
                }
            }
        }
        for (_, path) in self.diagrams.iter().filter(|(line, _)| *line == li) {
            if let Some((w, h)) = self.pics.borrow_mut().size(path) {
                images.push(layout::Image { at: 0, w: w as f32 * self.scale, h: h as f32 * self.scale, path: path.clone(), center: false });
            }
        }
        if let Some((gs, ge)) = self.ghost.filter(|g| self.md.is_none() && g.0 < end && g.1 > start) {
            self.attrs.resize(end - start, 0);
            self.attrs[gs.max(start) - start..ge.min(end) - start].fill(md::DIM | md::ITALIC);
        }
        if let Some((a,b,_)) = self.prediction.filter(|(a,b,_)| *a < end && *b > start) {
            self.attrs.resize(end-start, 0);
            self.attrs[a.max(start)-start..b.min(end)-start].fill(md::STRIKE | md::REMOVED);
        }
        if self.doc == Doc::Cells {
            let ci = self.cells.partition_point(|c| c.line <= li).saturating_sub(1);
            let cell = &self.cells[ci];
            let decor = self.decor.get(&cell.id);
            if self.chat {
                let first = cell.line + cell.headed as usize;
                let end = self.cells.get(ci + 1).map_or(self.buf.lines(), |c| c.line);
                talk = Some((cell.tag, li == first, li + 1 == end));
            }
            if kind == Kind::Cell && self.chat {
                // The user's messages need no name; the agent's carry the
                // model's, and its thinking says so.
                let (_, text) = cells::parse(&doc[ls..le]).unwrap_or((cell.tag, ""));
                label = match cell.tag {
                    Tag::Assistant | Tag::Think => text,
                    _ => "",
                };
            } else if kind == Kind::Cell {
                let (t, text) = cells::parse(&doc[ls..le]).unwrap_or((cell.tag, ""));
                label = decor.map(|d| d.label.as_str()).filter(|l| !l.is_empty()).unwrap_or(text);
                tag = if matches!(t, Tag::Markdown | Tag::Raw) { t.name() } else { "" };
                if let Some(m) = self.multi.as_ref().filter(|m| ci < m.excerpts.len()) {
                    excerpt_label = m.label(ci);
                    label = &excerpt_label;
                    numbered_label = m.controls(ci).join(excerpts::GAP);
                    tag = &numbered_label;
                } else if self.multi.is_some() {
                    tag = "";
                } else if self.headed {
                    numbered_label = format!("Cell {}  {}", ci + 1, label);
                    label = &numbered_label;
                    tag = "";
                }
            } else if let Some(d) =
                decor.filter(|_| li + 1 == self.cells.get(ci + 1).map_or(self.buf.lines(), |c| c.line))
            {
                (output, stamp) = (&d.output[..], d.stamp);
            }
        }

        let bidi_base = self.md.as_ref().and_then(|md| md.lines[li].bidi_base);
        let mono = self.mono();
        let first = li == 0 && self.lead;
        let mut h = DefaultHasher::new();
        for im in &images {
            (im.at, im.w.to_bits(), im.h.to_bits(), &im.path, im.center).hash(&mut h);
        }
        for m in &maths_in {
            (m.start, m.end, m.w.to_bits(), m.h.to_bits(), &m.path).hash(&mut h);
        }
        (kind, table.is_some(), &doc[start..end], &self.attrs[..]).hash(&mut h);
        for p in prefix {
            (p.start as usize - ls, p.end as usize - ls, std::mem::discriminant(&p.kind)).hash(&mut h);
            match p.kind {
                PrefixKind::Number(a, b) | PrefixKind::Indent(a, b, _) => doc[a as usize..b as usize].hash(&mut h),
                PrefixKind::Task(done) => done.hash(&mut h),
                _ => {}
            }
        }
        (content - ls, line_active, block_active, first, mono, px.to_bits(), cw.to_bits(), wrap).hash(&mut h);
        (label, tag, stamp, !output.is_empty(), theme::stamp(), crate::font::generation(), self.family, talk, bidi_base).hash(&mut h);
        let sig = h.finish();
        if let Some(Some(l)) = self.cache.get(li) {
            if l.sig == sig {
                return l.clone();
            }
        }

        let inp = Input {
            doc,
            start,
            end,
            attrs: &self.attrs,
            kind,
            prefix,
            content,
            line_active,
            block_active,
            first,
            mono,
            images: &images,
            maths: &maths_in,
            label,
            tag,
            output,
            family: self.family,
            talk,
            wrap,
            bidi_base,
        };
        let fonts = &mut self.fonts.borrow_mut();
        let mut lay = match table {
            Some(t) => layout::table(fonts, &inp, t, px, cw),
            None => layout::line(fonts, &inp, px, cw),
        };
        lay.sig = sig;
        let lay = Rc::new(lay);
        self.cache[li] = Some(lay.clone());
        lay
    }

    fn height(&mut self, li: usize) -> f32 {
        self.layout(li).h
    }

    // ---- scrolling ------------------------------------------------------

    pub fn scroll(&mut self, dy: f32) {
        if self.comparison_scroll(dy) || self.merge_scroll(dy) { return; }
        self.top_off += dy;
        while self.top_off < 0.0 && self.top > 0 {
            self.top -= 1;
            self.top_off += self.height(self.top);
        }
        self.top_off = self.top_off.max(0.0);
        loop {
            let h = self.height(self.top);
            if self.top_off < h || self.top + 1 >= self.buf.lines() {
                break;
            }
            self.top_off -= h;
            self.top += 1;
        }
        self.clamp_scroll();
        self.at_tail = self.scroll_limit().is_none_or(|limit| (self.top, self.top_off) >= limit);
        self.redraw = true;
    }

    pub fn scroll_lines(&mut self, lines: f32) {
        self.scroll(lines * (self.px() * theme::LINE_HEIGHT).round());
    }

    /// Scrolls as far down as the document goes.
    pub fn scroll_end(&mut self) {
        (self.top, self.top_off) = (self.buf.lines() - 1, f32::MAX);
        self.clamp_scroll();
        self.at_tail = true;
        self.redraw = true;
    }

    /// The furthest the view may scroll: the end of the document stays in
    /// the lower part of the view. `None` if the document fits entirely.
    fn scroll_limit(&mut self) -> Option<(usize, f32)> {
        let n = self.buf.lines();
        let view_h = self.h.max(1.0);
        let mut total = 0.0;
        let mut li = 0;
        while li < n && total <= view_h {
            total += self.height(li);
            li += 1;
        }
        if total <= view_h {
            return None;
        }
        let keep = (view_h * self.keep).round();
        let (mut acc, mut li) = (0.0, n);
        while li > 0 && acc < keep {
            li -= 1;
            acc += self.height(li);
        }
        Some(if acc >= keep { (li, acc - keep) } else { (0, 0.0) })
    }

    fn clamp_scroll(&mut self) {
        self.top = self.top.min(self.buf.lines() - 1);
        if self.typewriter { return; }
        self.top_off = self.top_off.max(0.0);
        match self.scroll_limit() {
            None => (self.top, self.top_off) = (0, 0.0),
            Some(limit) if (self.top, self.top_off) > limit => (self.top, self.top_off) = limit,
            Some(_) => {}
        }
    }

    fn reveal_cursor(&mut self) {
        // With no room yet, every line looks out of view; wait for a size.
        if self.w <= 0.0 || self.h <= 0.0 {
            self.reveal_later = true;
            return;
        }
        self.reveal_later = false;
        let cl = self.buf.line_of(self.buf.cur);
        let ls = self.buf.starts[cl];
        let lay = self.layout(cl);
        self.reveal_across(lay.locate((self.buf.cur - ls) as u32).1);
        let row = lay.locate((self.buf.cur - ls) as u32).0;
        let (ry, rh) = lay.rows.get(row).map_or((0.0, 0.0), |r| (r.y, r.h));
        let view_h = self.h;
        if self.typewriter {
            let mut need = view_h * 0.45 - ry;
            let mut top = cl;
            while top > 0 && need > 0.0 { top -= 1; need -= self.height(top); }
            (self.top, self.top_off) = (top, -need);
            return;
        }
        let margin = (rh * 0.6).min(view_h / 4.0);
        if cl < self.top || (cl == self.top && ry - margin < self.top_off) {
            (self.top, self.top_off) = (cl, (ry - margin).max(0.0));
            return;
        }
        let (mut y, mut li) = (-self.top_off, self.top);
        while li < cl && y <= view_h * 2.0 {
            y += self.height(li);
            li += 1;
        }
        if li < cl || y + ry + rh + margin > view_h {
            let mut need = view_h - margin - (ry + rh);
            let mut t = cl;
            while t > 0 && need > 0.0 {
                t -= 1;
                need -= self.height(t);
            }
            (self.top, self.top_off) = (t, (-need).max(0.0));
        }
    }

    // ---- editing --------------------------------------------------------

    fn changed(&mut self, c: Change) {
        let at_start = self.buf.starts.get(c.line) == Some(&c.pos);
        self.bookmarks_changed |= bookmarks::track(&mut self.bookmarks, &c, at_start);
        self.broken_links.clear();
        self.diagrams.clear();
        self.folds.clear();
        self.extra_cursors.clear();
        let delta = self.buf.text.len() as isize - self.pair_text_len as isize;
        // A second view's places move with the text before them.
        if let Some(m) = &mut self.mirror {
            for at in [&mut m.cur, &mut m.anchor, &mut m.top_at] {
                if *at >= c.pos + c.del {
                    *at = (*at as isize + delta).max(c.pos as isize) as usize;
                } else if *at > c.pos {
                    *at = c.pos;
                }
            }
        }
        if let Some((a, b)) = &mut self.find_scope {
            (*a, *b) = find_scope_after(*a, *b, &c, delta);
        }
        if let Some(s) = &mut self.snippet {
            // Edits in the stop being typed, and the mirrors written for
            // it, move the stops; any other edit ends the snippet.
            let (a, b) = s.stops[s.index].at;
            let (pos, end) = (c.pos, c.pos + c.del);
            if self.snippet_mirroring || (pos >= a && end <= b) {
                let new_end = (end as isize + delta).max(pos as isize) as usize;
                let index = s.index;
                for (i, stop) in s.stops.iter_mut().enumerate() {
                    let active = i == index && !self.snippet_mirroring;
                    stop.at = snippet_range_after(stop.at, pos, end, new_end, active);
                    for m in &mut stop.mirrors {
                        let target = self.snippet_mirroring && *m == (pos, end);
                        *m = snippet_range_after(*m, pos, end, new_end, target);
                    }
                }
            } else {
                self.snippet = None;
            }
        }
        self.auto_pairs.retain_mut(|(pos, _)| {
            if *pos < c.pos {
                return true;
            }
            if *pos < c.pos + c.del {
                return false;
            }
            *pos = (*pos as isize + delta).max(0) as usize;
            true
        });
        self.pair_text_len = self.buf.text.len();
        let at = c.line.min(self.cache.len());
        let end = (at + c.removed + 1).min(self.cache.len());
        self.cache.splice(at..end, std::iter::repeat_n(None, c.added + 1));
        self.cache.resize(self.buf.lines(), None);
        if self.doc == Doc::Cells {
            self.retrack(&c);
        }
        self.reparse();
    }

    /// Replaces a range of the text. Returns whether the edit was made.
    pub(crate) fn preview_raw(&mut self, a: usize, b: usize, s: &str) {
        let c = self.buf.raw(a, b, s);
        self.changed(c);
    }

    /// Starts a snippet expanded at `at`, with its first stop selected.
    pub(crate) fn start_snippet(&mut self, at: usize, e: crate::snippets::Expansion) {
        let shift = |(a, b): (usize, usize)| (a + at, b + at);
        let stops: Vec<_> = e
            .stops
            .into_iter()
            .map(|mut s| {
                s.at = shift(s.at);
                s.mirrors.iter_mut().for_each(|m| *m = shift(*m));
                s
            })
            .collect();
        if stops.is_empty() {
            return;
        }
        self.snippet = Some(SnippetSession { stops, index: 0, mark: 0, before: None, sel: (0, 0) });
        self.snippet_enter();
    }

    /// Selects the current stop and remembers the text, so what is typed in
    /// it, mirrors included, becomes one step to undo.
    fn snippet_enter(&mut self) {
        let Some(s) = &self.snippet else { return };
        let (a, b) = s.stops[s.index].at;
        self.select(a, b);
        self.moved();
        let small = self.buf.text.len() <= 4 << 20;
        let (mark, before, sel) = (self.buf.undo_len(), small.then(|| self.buf.text.clone()), (self.buf.anchor, self.buf.cur));
        if let Some(s) = &mut self.snippet {
            (s.mark, s.before, s.sel) = (mark, before, sel);
        }
    }

    /// Leaves the current stop for the next, or with `back` the one before;
    /// past the last, the snippet ends.
    pub(crate) fn snippet_step(&mut self, back: bool) {
        let Some(s) = &mut self.snippet else { return };
        if let Some(before) = s.before.take() {
            self.buf.squash(s.mark, &before, s.sel);
        }
        let Some(s) = &mut self.snippet else { return };
        if back {
            s.index = s.index.saturating_sub(1);
        } else {
            s.index += 1;
        }
        if s.index >= s.stops.len() {
            self.snippet = None;
            return;
        }
        self.snippet_enter();
    }

    /// The choices of the stop being typed, while it is still as inserted.
    pub(crate) fn snippet_choices(&self) -> Option<(usize, &[String])> {
        let s = self.snippet.as_ref()?;
        let stop = &s.stops[s.index];
        (!stop.choices.is_empty() && self.buf.sel() == stop.at).then_some((stop.at.0, &stop.choices[..]))
    }

    /// Writes the text of each stop into its mirrors, keeping the caret
    /// where it was in the stop being typed.
    fn snippet_mirror(&mut self) {
        let Some(s) = &self.snippet else { return };
        if self.snippet_mirroring || s.stops.iter().all(|st| st.mirrors.is_empty()) {
            return;
        }
        let a = s.stops[s.index].at.0;
        let (cur, anchor) = (self.buf.cur.saturating_sub(a), self.buf.anchor.saturating_sub(a));
        self.snippet_mirroring = true;
        for i in 0..s.stops.len() {
            let mut k = 0;
            loop {
                let Some(st) = self.snippet.as_ref().map(|s| &s.stops[i]) else { break };
                let Some(&(x, y)) = st.mirrors.get(k) else { break };
                let (p, q) = st.at;
                let text = self.buf.text[p..q].to_string();
                if self.buf.text[x..y] != text {
                    let c = self.buf.replace(x, y, &text, EditKind::Other);
                    self.changed(c);
                }
                k += 1;
            }
        }
        self.snippet_mirroring = false;
        if let Some(s) = &self.snippet {
            let a = s.stops[s.index].at.0;
            let len = self.buf.text.len();
            (self.buf.cur, self.buf.anchor) = ((a + cur).min(len), (a + anchor).min(len));
        }
    }

    pub fn edit(&mut self, start: usize, end: usize, ins: &str, kind: EditKind) -> bool {
        if start == end && ins.is_empty() {
            return false;
        }
        if self.read_only {
            self.note = Some("This file is read-only".into());
            return false;
        }
        if self.multi.is_some() && !self.excerpts_allow(start, end, ins) {
            return false;
        }
        if self.doc == Doc::Cells && !self.keeps_cells(start, end, ins) {
            return false;
        }
        // An edit that takes the first header away leaves a new one behind.
        let mut headed = String::new();
        if self.headed && start == 0 {
            let first = match ins.split_once('\n') {
                Some((line, _)) => line.to_string(),
                None => format!("{ins}{}", self.buf.text[end..].split('\n').next().unwrap_or("")),
            };
            if cells::parse(&first).is_none() {
                headed = format!("{}\n{ins}", cells::header(Tag::Code, ""));
            }
        }
        let ins = if headed.is_empty() { ins } else { &headed };
        let c = self.buf.replace(start, end, ins, kind);
        self.changed(c);
        self.snippet_mirror();
        true
    }

    /// Inserts at the cursor, replacing any selection.
    pub fn insert(&mut self, s: &str, kind: EditKind) {
        let (a, b) = self.buf.sel();
        self.edit(a, b, s, if a == b { kind } else { EditKind::Other });
    }

    /// Call after anything that moves the cursor.
    pub fn moved(&mut self) {
        let line = self.buf.line_of(self.buf.cur);
        self.folds.retain(|&(a, b)| line <= a || line > b);
        self.buf.cur = self.snap(self.buf.cur, true);
        self.refocus();
        self.reveal_cursor();
        self.redraw = true;
    }

    /// Distance from the top of the view to the top of a line on screen.
    fn line_top(&mut self, line: usize) -> Option<f32> {
        let mut y = -self.top_off;
        for l in self.top..line {
            y += self.height(l);
            if y > self.h {
                return None;
            }
        }
        (line >= self.top).then_some(y)
    }

    /// Moves the focus to the cursor. Syntax appearing and disappearing above
    /// the cursor changes heights there; scroll so that the cursor's own line
    /// stays where it was instead of jumping.
    fn refocus(&mut self) {
        let line = self.buf.line_of(self.buf.cur);
        let before = self.line_top(line);
        self.focus = self.buf.cur;
        if let (Some(before), Some(after)) = (before, self.line_top(line)) {
            if after != before {
                self.scroll(after - before);
            }
        }
    }

    fn set_cursor(&mut self, off: usize, select: bool) {
        self.buf.cur = self.snap(off, off >= self.buf.cur);
        if !select {
            self.buf.anchor = self.buf.cur;
        }
        self.buf.commit();
    }

    pub fn select(&mut self, anchor: usize, cur: usize) {
        self.extra_cursors.clear();
        (self.buf.anchor, self.buf.cur) = (anchor, cur);
        self.moved();
    }

    /// Offset one row above or below the cursor, keeping its x position.
    fn vertical(&mut self, down: bool) -> usize {
        let cur = self.buf.cur;
        let cl = self.buf.line_of(cur);
        let (ls, _) = self.buf.line(cl);
        let lay = self.layout(cl);
        let (row, x) = lay.locate((cur - ls) as u32);
        let gx = *self.goal_x.get_or_insert(x);
        let snap = |buf: &Buffer, l: usize, off: u32| {
            let (s, e) = buf.line(l);
            editor::floor(&buf.text, (s + off as usize).min(e))
        };
        if down {
            if row + 1 < lay.rows.len() {
                return snap(&self.buf, cl, lay.offset_in_row(row + 1, gx));
            }
            for l in cl + 1..self.buf.lines() {
                let lay = self.layout_for(l, self.buf.starts[l]);
                if !lay.rows.is_empty() {
                    return snap(&self.buf, l, lay.offset_in_row(0, gx));
                }
            }
            self.buf.text.len()
        } else {
            if row > 0 {
                return snap(&self.buf, cl, lay.offset_in_row(row - 1, gx));
            }
            for l in (0..cl).rev() {
                let lay = self.layout_for(l, self.buf.starts[l]);
                if let Some(last) = lay.rows.len().checked_sub(1) {
                    return snap(&self.buf, l, lay.offset_in_row(last, gx));
                }
            }
            0
        }
    }

    fn row_edge(&mut self, end: bool) -> usize {
        let cur = self.buf.cur;
        let cl = self.buf.line_of(cur);
        let (ls, le) = self.buf.line(cl);
        let lay = self.layout(cl);
        let row = lay.locate((cur - ls) as u32).0;
        if lay.rows.is_empty() {
            return if end { le } else { ls };
        }
        if !lay.carets.is_empty() { return ls+if end { lay.rows[row].end as usize } else { lay.rows[row].start as usize }; }
        if end {
            return (ls + lay.row_end(row) as usize).min(le);
        }
        if row > 0 {
            return ls + lay.rows[row].start as usize;
        }
        // Home stops after the list or quote marker first, then at column 0.
        // In code it stops at the indentation.
        let mut content = self.md.as_ref().map_or(ls, |md| md.lines[cl].content as usize);
        if !self.prose_at(cur) && self.doc != Doc::Text {
            let line = &self.buf.text[ls..le];
            content = ls + line.len() - line.trim_start_matches([' ', '\t']).len();
        }
        if cur > content { content } else { ls }
    }

    /// Enter continues the list or quote the cursor is in; on an empty item
    /// it ends the list instead. In code it keeps the indentation.
    fn newline(&mut self) {
        let cl = self.buf.line_of(self.buf.cur);
        let (ls, le) = self.buf.line(cl);
        let text = &self.buf.text;
        if !self.prose_at(self.buf.cur) && self.doc != Doc::Text {
            let before = &text[ls..self.buf.sel().0.max(ls)];
            let indent = if self.python_at(self.buf.cur) {
                let mut indent = py::indent_after(before);
                if let Some(unit) = self.indentation() {
                    let lead = &before[..before.len() - before.trim_start_matches([' ', '\t']).len()];
                    if indent.len() > lead.len() {
                        indent.truncate(lead.len());
                        indent.push_str(&unit);
                    } else if indent.len() < lead.len() {
                        indent = lead[..lead.len().saturating_sub(unit.len())].to_string();
                    }
                }
                indent
            } else {
                before[..before.len() - before.trim_start_matches([' ', '\t']).len()].to_string()
            };
            // An environment begun on the line is ended below it.
            let at_end = |_: &&str| !self.buf.has_sel() && text[self.buf.cur..le].trim().is_empty();
            let env = latex::opens(before).filter(|_| self.doc == Doc::Latex).filter(at_end);
            if let Some(env) = env.filter(|env| latex::unclosed(text, env)).map(str::to_string) {
                let inner = format!("\n{indent}    ");
                let at = self.buf.sel().0 + inner.len();
                let end = format!("{inner}\n{indent}\\end{{{env}}}");
                self.insert(&end, EditKind::Other);
                return self.select(at, at);
            }
            return self.insert(&format!("\n{indent}"), EditKind::Other);
        }
        let mut cont = String::new();
        let (mut marked, mut ends_list) = (false, false);
        if let (Some(md), false) = (&self.md, self.buf.has_sel()) {
            let info = &md.lines[cl];
            if self.buf.cur >= info.content as usize && !matches!(info.kind, Kind::Code | Kind::FenceOpen | Kind::Meta)
            {
                for p in &info.prefix {
                    let raw = &text[p.start as usize..p.end as usize];
                    match p.kind {
                        PrefixKind::Number(m0, m1) => {
                            let mark = &text[m0 as usize..m1 as usize];
                            let digits = mark.trim_end_matches(['.', ')']);
                            let next = digits.parse::<u64>().map_or(1, |n| n + 1);
                            cont += &text[p.start as usize..m0 as usize];
                            cont += &format!("{next}{} ", &mark[digits.len()..]);
                            marked = true;
                        }
                        PrefixKind::Task(_) => cont += "[ ] ",
                        PrefixKind::Quote | PrefixKind::Bullet => {
                            cont += raw;
                            marked |= !raw.is_empty();
                        }
                        PrefixKind::Indent(..) => cont += raw,
                    }
                }
                ends_list = marked && self.buf.cur == le && text[info.content as usize..le].trim().is_empty();
            }
        }
        if ends_list {
            self.edit(ls, le, "", EditKind::Other);
            return;
        }
        if !marked {
            cont.clear();
        }
        self.insert(&format!("\n{cont}"), EditKind::Other);
    }

    /// Rewrites every line the selection touches, keeping the selection on
    /// those lines.
    fn rewrite_lines(&mut self, f: impl Fn(&str) -> String) {
        let (a, b) = self.buf.sel();
        let (l0, l1) = (
            self.buf.line_of(a),
            self.buf.line_of(if b > a && self.buf.starts.binary_search(&b).is_ok() { b - 1 } else { b }),
        );
        let (start, end) = (self.buf.starts[l0], self.buf.line(l1).1);
        let block: Vec<String> = self.buf.text[start..end].split('\n').map(f).collect();
        let block = block.join("\n");
        let delta = block.len() as isize - (end - start) as isize;
        if block == self.buf.text[start..end] {
            return;
        }
        let (cur, anchor) = (self.buf.cur, self.buf.anchor);
        if !self.edit(start, end, &block, EditKind::Other) {
            return;
        }
        if l0 == l1 {
            let shift = |o: usize| (o as isize + delta).max(start as isize) as usize;
            (self.buf.cur, self.buf.anchor) = (shift(cur), shift(anchor));
        } else {
            (self.buf.anchor, self.buf.cur) = (start, start + block.len());
        }
    }

    /// A unit set for this file, else EditorConfig's, else the hint detected
    /// on opening.
    pub(crate) fn indentation(&self) -> Option<String> {
        self.file_indent.clone().or_else(|| self.path.as_deref().and_then(|p| crate::app::saving::Rules::read(p).indent())
            .or_else(|| self.inferred_indent.clone()))
    }

    /// Indents or outdents every line the selection touches.
    fn indent(&mut self, out: bool) {
        let (a, b) = self.buf.sel();
        let (l0, l1) = (
            self.buf.line_of(a),
            self.buf.line_of(if b > a && self.buf.starts.binary_search(&b).is_ok() { b - 1 } else { b }),
        );
        let listed = self.md.as_ref().is_some_and(|md| {
            md.lines[l0].prefix.iter().any(|p| matches!(p.kind, PrefixKind::Bullet | PrefixKind::Number(..)))
        });
        let unit = &self.indent_unit_at(a);
        if !out && l0 == l1 && !listed {
            return self.insert(unit, EditKind::Type);
        }
        self.rewrite_lines(|line| {
            if !out {
                return format!("{unit}{line}");
            }
            let cut = if line.starts_with('\t') {
                1
            } else {
                line.bytes().take(if unit == "\t" { 4 } else { unit.len() }).take_while(|&c| c == b' ').count()
            };
            line[cut..].to_string()
        });
    }

    /// Comments the selected lines out, or back in if they all are.
    fn comment(&mut self) {
        let Some((open, close)) = self.comment_mark() else {
            self.note = Some("This language has no comment syntax".into());
            return;
        };
        let (start, end) = self.selected_lines();
        let lines = || self.buf.text[start..end].split('\n').filter(|l| !l.trim().is_empty());
        let all = lines().all(|l| l.trim().starts_with(open) && (close.is_empty() || l.trim().ends_with(close)));
        let depth = lines().map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
        self.rewrite_lines(|line| {
            if line.trim().is_empty() {
                return line.to_string();
            }
            if !all {
                return format!(
                    "{}{open} {}{}",
                    &line[..depth],
                    &line[depth..],
                    if close.is_empty() { String::new() } else { format!(" {close}") }
                );
            }
            let (lead, rest) = line.split_at(line.len() - line.trim_start().len());
            let rest = rest
                .strip_prefix(open)
                .unwrap_or(rest)
                .strip_prefix(' ')
                .unwrap_or_else(|| rest.strip_prefix(open).unwrap_or(rest));
            let rest = if close.is_empty() {
                rest
            } else {
                rest.trim_end()
                    .strip_suffix(close)
                    .unwrap_or(rest)
                    .strip_suffix(' ')
                    .unwrap_or_else(|| rest.trim_end().strip_suffix(close).unwrap_or(rest))
            };
            format!("{lead}{rest}")
        });
    }

    /// Wraps the selection in an emphasis marker, or unwraps it.
    fn emphasize(&mut self, mark: &str) {
        self.wrap(mark, mark);
    }

    /// Puts the selection between `open` and `close`, or takes it out from
    /// between them.
    fn wrap(&mut self, open: &str, close: &str) {
        let (a, b) = self.buf.sel();
        let n = open.len();
        let text = &self.buf.text;
        if a >= n && text[..a].ends_with(open) && text[b..].starts_with(close) {
            let inner = text[a..b].to_string();
            if self.edit(a - n, b + close.len(), &inner, EditKind::Other) {
                (self.buf.anchor, self.buf.cur) = (a - n, b - n);
            }
        } else {
            let wrapped = format!("{open}{}{close}", &text[a..b]);
            if self.edit(a, b, &wrapped, EditKind::Other) {
                (self.buf.anchor, self.buf.cur) = (a + n, b + n);
            }
        }
    }

    /// The saved document, excluding unaccepted ghost text.
    pub fn source_text(&self) -> String {
        let mut text = self.buf.text.clone();
        if let Some((a,b)) = self.ghost { text.replace_range(a..b, ""); }
        text
    }
    pub fn clear_prediction(&mut self) {
        if self.prediction.take().is_some() { self.cache.fill(None); }
    }
    pub fn prediction_jump(&mut self, jump: bool) {
        if let Some(p) = &mut self.prediction { p.2 = jump; }
        self.redraw = true;
    }
    pub fn show_prediction(&mut self, a: usize, b: usize, text: &str, jump: bool) {
        self.drop_ghost();
        if !text.is_empty() {
            let c = self.buf.raw(b,b,text);
            self.ghost = Some((b,b+text.len()));
            self.changed(c);
        }
        self.prediction = Some((a,b,jump));
        self.cache.fill(None);
        self.redraw = true;
    }
    // ---- suggestions ----------------------------------------------------

    /// Shows a suggestion at the cursor: text that is not the user's yet.
    /// Whoever shows one must take it away again, with `drop_ghost` or
    /// `accept_ghost`, before the document is edited, saved or read.
    pub fn show_ghost(&mut self, text: &str) {
        self.drop_ghost();
        let text = text.replace("\r\n", "\n").replace(cells::MARK, "");
        if text.is_empty() || self.read_only || self.buf.has_sel() {
            return;
        }
        let at = self.buf.cur;
        let c = self.buf.raw(at, at, &text);
        self.ghost = Some((at, at + text.len()));
        self.changed(c);
        self.redraw = true;
    }

    /// The suggestion on show, if there is one.
    pub fn ghost(&self) -> Option<&str> {
        self.ghost.map(|(s, e)| &self.buf.text[s..e])
    }

    /// Takes the suggestion away. Returns what it was.
    pub fn drop_ghost(&mut self) -> Option<String> {
        let (s, e) = self.ghost.take()?;
        let text = self.buf.text[s..e].to_string();
        let c = self.buf.raw(s, e, "");
        self.changed(c);
        self.redraw = true;
        Some(text)
    }

    /// Makes the first `len` bytes of the suggestion the user's own text,
    /// as one edit, and keeps showing the rest.
    pub fn accept_ghost(&mut self, len: usize) {
        let Some(text) = self.drop_ghost() else { return };
        let len = editor::floor(&text, len);
        self.insert(&text[..len], EditKind::Other);
        self.moved();
        self.show_ghost(&text[len..]);
    }

    pub fn paste_text(&mut self, s: &str) {
        self.drop_ghost();
        let mut s = s.replace("\r\n", "\n").replace('\r', "\n");
        if self.doc == Doc::Cells {
            // Cells copied from a notebook come back as cells when they land
            // at the start of a line, and as their text anywhere else.
            let (a, b) = self.buf.sel();
            let whole = a == self.buf.starts[self.buf.line_of(a)] && self.keeps_cells(a, b, &s);
            if !whole {
                let body: Vec<&str> = s.split('\n').filter(|l| !l.starts_with(cells::MARK)).collect();
                s = body.join("\n").replace(cells::MARK, "");
            }
        } else {
            s = s.replace(cells::MARK, "");
        }
        if !self.multiple_insert(&s) {
            self.insert(&s, EditKind::Other);
        }
        self.moved();
    }

    // ---- keyboard -------------------------------------------------------

    /// Handles an editing or movement key. Returns whether it was one.
    pub fn key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if mods.ctrl && !mods.alt && matches!(key, Key::Char('z' | 'y')) && self.multi_undo(key == Key::Char('y') || mods.shift) {
            return true;
        }
        if self.vim_key(key, mods, text) { return true; }
        if self.comparison_key(key, mods) { return true; }
        self.drop_ghost();
        self.redraw = true;
        if key == Key::Tab && !mods.ctrl && !mods.alt && self.snippet.is_some() {
            self.snippet_step(mods.shift);
            return true;
        }
        if self.table_key(key, mods, text) { return true; }
        if self.multiple_key(key, mods, text) {
            return true;
        }
        let sel = mods.shift;
        if !matches!(key, Key::Up | Key::Down | Key::PageUp | Key::PageDown) {
            self.goal_x = None;
        }
        let cur = self.buf.cur;
        let visual = if matches!(key,Key::Left | Key::Right) && !mods.ctrl {
            let li = self.buf.line_of(cur);
            let ls = self.buf.starts[li];
            let (a,b) = self.buf.line(li);
            if crate::bidi::needed(&self.buf.text[a..b]) || self.md.as_ref().is_some_and(|md| md.lines[li].bidi_base.is_some()) {
                self.layout(li).visual_move((cur-ls) as u32,key == Key::Right).map(|at| ls+at as usize)
            } else { None }
        } else { None };
        let t = &self.buf.text;
        match key {
            Key::Up | Key::Down if mods.alt && !mods.ctrl => {
                self.line_operation(if key == Key::Up { "up" } else { "down" });
            }
            Key::Char('d') if mods.ctrl && mods.alt => self.line_operation("duplicate"),
            Key::Char('k') if mods.ctrl && mods.shift => self.line_operation("delete"),
            Key::Char('e') if mods.ctrl && self.prose_at(cur) && !self.read_only => {
                self.edit_mode = !self.edit_mode;
                self.moved();
            }
            Key::Char('z') if mods.ctrl && !mods.shift => {
                if let Some(c) = self.buf.undo() {
                    self.changed(c);
                }
                self.moved();
            }
            Key::Char('z' | 'y') if mods.ctrl => {
                if let Some(c) = self.buf.redo() {
                    self.changed(c);
                }
                self.moved();
            }
            Key::Char('a') if mods.ctrl => {
                (self.buf.anchor, self.buf.cur) = (0, t.len());
                self.focus = self.buf.cur;
            }
            Key::Char('c' | 'x') if mods.ctrl => {
                let (a, b) = self.buf.sel();
                if a != b {
                    self.copy = Some(t[a..b].to_string());
                    if key == Key::Char('x') {
                        self.edit(a, b, "", EditKind::Other);
                        self.moved();
                    }
                }
            }
            Key::Char('b') if mods.ctrl && self.prose_at(cur) => {
                self.emphasize("**");
                self.moved();
            }
            Key::Char('i') if mods.ctrl && self.prose_at(cur) => {
                self.emphasize("*");
                self.moved();
            }
            Key::Char('b' | 'i') if mods.ctrl && self.doc == Doc::Latex => {
                self.wrap(if key == Key::Char('b') { "\\textbf{" } else { "\\emph{" }, "}");
                self.moved();
            }
            Key::Char('/') if mods.ctrl => {
                self.comment();
                self.moved();
            }
            Key::Left | Key::Right => {
                let right = key == Key::Right;
                let (a, b) = self.buf.sel();
                let to = if let Some(at) = visual { at } else if a != b && !sel && !mods.ctrl {
                    if right { b } else { a }
                } else {
                    match (right, mods.ctrl) {
                        (true, true) => editor::word_right(t, cur),
                        (true, false) => editor::next(t, cur),
                        (false, true) => editor::word_left(t, cur),
                        (false, false) => editor::prev(t, cur),
                    }
                };
                self.set_cursor(to, sel);
                self.moved();
            }
            Key::Up | Key::Down => {
                let to = self.vertical(key == Key::Down);
                self.set_cursor(to, sel);
                self.moved();
            }
            Key::PageUp | Key::PageDown => {
                let rows = (self.h / (self.px() * theme::LINE_HEIGHT)).floor().max(2.0) as usize - 1;
                for _ in 0..rows {
                    let to = self.vertical(key == Key::PageDown);
                    self.set_cursor(to, sel);
                    self.focus = self.buf.cur;
                }
                self.moved();
            }
            Key::Home | Key::End => {
                let to = match (key == Key::End, mods.ctrl) {
                    (false, true) => 0,
                    (true, true) => t.len(),
                    (end, false) => self.row_edge(end),
                };
                self.set_cursor(to, sel);
                self.moved();
            }
            Key::Backspace
                if !mods.ctrl
                    && !self.buf.has_sel()
                    && self.auto_pairs.iter().any(|&(at, close)| {
                        at == cur
                            && cur > 0
                            && matching(self.buf.text[..cur].chars().next_back().unwrap()) == Some(close)
                    }) =>
            {
                self.edit(cur - 1, cur + 1, "", EditKind::Other);
                self.moved();
            }
            Key::Backspace | Key::Delete => {
                let (a, b) = self.buf.sel();
                let (a, b) = if a != b {
                    (a, b)
                } else {
                    match (key == Key::Delete, mods.ctrl) {
                        (false, false) => (self.dedent_stop(cur).unwrap_or_else(|| editor::prev(t, cur)), cur),
                        (false, true) => (editor::word_left(t, cur), cur),
                        (true, false) => (cur, editor::next(t, cur)),
                        (true, true) => (cur, editor::word_right(t, cur)),
                    }
                };
                if a != b {
                    self.edit(a, b, "", EditKind::Delete);
                }
                self.moved();
            }
            Key::Enter => {
                self.newline();
                self.moved();
            }
            Key::Tab => {
                self.indent(mods.shift);
                self.moved();
            }
            Key::Escape => self.buf.anchor = cur,
            _ => {
                let typed = text.filter(|s| !mods.ctrl && !mods.alt && !s.chars().any(char::is_control));
                let Some(s) = typed.filter(|s| !s.is_empty()) else { return false };
                // A space ends the undo step, so undo goes word by word.
                let kind = if s == " " { EditKind::Other } else { EditKind::Type };
                if !self.type_pair(s) {
                    self.insert(s, kind);
                }
                self.moved();
            }
        }
        true
    }

    /// In Python, Backspace inside the indentation goes back one level.
    fn dedent_stop(&self, cur: usize) -> Option<usize> {
        let ls = self.buf.starts[self.buf.line_of(cur)];
        let lead = &self.buf.text[ls..cur];
        let level = self.python_at(cur) && !lead.is_empty() && lead.bytes().all(|c| c == b' ');
        let width = self.indentation()
            .filter(|s| s.bytes().all(|b| b == b' '))
            .map_or(4, |s| s.len());
        level.then(|| ls + (lead.len() - 1) / width * width)
    }

    pub fn zoom_by(&mut self, steps: i32) {
        self.zoom = (self.zoom * 1.1f32.powi(steps)).clamp(0.5, 4.0);
        self.reveal_cursor();
        self.redraw = true;
    }

    pub fn zoom_reset(&mut self) {
        self.zoom = 1.0;
        self.reveal_cursor();
        self.redraw = true;
    }

    // ---- pointer --------------------------------------------------------

    fn line_at(&self, y: f32) -> Option<(usize, f32)> {
        let i = self.visible.partition_point(|&(_, top)| top <= y);
        self.visible.get(i.saturating_sub(1)).copied()
    }

    /// Document offset under a point, by what is currently on screen.
    fn offset_at(&self, x: f32, y: f32) -> usize {
        let Some((li, top)) = self.line_at(y) else { return 0 };
        let Some(Some(lay)) = self.cache.get(li) else { return self.buf.starts[li] };
        let (ls, mut le) = self.buf.line(li);
        if let Some(c) = lay.clusters.last() {
            le = le.max(ls + c.end as usize);
        }
        let off = ls + lay.hit(x - self.text_screen_x(0.0), y - top) as usize;
        self.snap(editor::floor(&self.buf.text, off.min(le)), true)
    }

    /// Whether a point is on the scrollbar, which can be dragged.
    pub(crate) fn git_under(&self, x: f32, y: f32) -> Option<usize> {
        let left = self.column().0 - self.marks_gutter();
        if self.read_only || !self.contains(x, y) || x < left || x > left + 6.0 * self.scale { return None; }
        let (line, _) = self.line_at(y)?;
        self.git_marks.iter().any(|m| m.0 == line).then_some(line)
    }
    pub(crate) fn test_under(&self, x: f32, y: f32) -> Option<usize> {
        if !self.contains(x, y) || x >= self.column().0 || x < self.column().0 - self.marks_gutter() {
            return None;
        }
        let (line, _) = self.line_at(y)?;
        self.test_marks.iter().find(|m| m.0 == line).map(|m| m.1)
    }
    pub fn on_bar(&self, x: f32, y: f32) -> bool {
        self.on_minimap(x, y) || (self.bar.is_some() && self.contains(x, y) && x >= self.x + self.w - (12.0 * self.scale).round())
    }

    /// Scrolls so that the scrollbar's thumb is held at `y`.
    fn drag_bar(&mut self, y: f32, grab: f32) {
        let Some((_, len, shown)) = self.bar else { return };
        let room = (self.h - len).max(1.0);
        let t = ((y - self.y - grab) / room).clamp(0.0, 1.0);
        let n = self.buf.lines();
        self.top = (t * n.saturating_sub(shown) as f32).round() as usize;
        self.top_off = 0.0;
        self.clamp_scroll();
        self.at_tail = self.scroll_limit().is_none_or(|limit| (self.top, self.top_off) >= limit);
        self.redraw = true;
    }

    /// Moves the cursor to a point, as a click there would, but without
    /// ticking a checkbox or following a link.
    pub fn place_at(&mut self, x: f32, y: f32) {
        self.drop_ghost();
        self.goal_x = None;
        let off = self.offset_at(x, y);
        self.set_cursor(off, false);
        self.moved();
    }

    /// Whether a point is on the selected text.
    pub fn on_selection(&self, x: f32, y: f32) -> bool {
        let (a, b) = self.buf.sel();
        a != b && (a..b).contains(&self.offset_at(x, y))
    }

    /// The selected text, if any.
    pub fn selected(&self) -> Option<String> {
        let (a, b) = self.buf.sel();
        (a != b).then(|| self.buf.text[a..b].to_string())
    }

    /// A press of the left button. Returns a link to follow, if Ctrl was
    /// held over one.
    pub fn press(&mut self, x: f32, y: f32, mods: Mods, clicks: u32) -> Option<String> {
        if self.merge.as_ref().is_some_and(|m| crate::ui::inside(m.panel, Some((x, y)))) { return None; }
        self.drop_ghost();
        self.redraw = true;
        self.goal_x = None;
        if let Some((_,at)) = self.sticky_hits.iter().find(|(r,_)| crate::ui::inside(*r,Some((x,y)))).copied() {
            self.select(at,at); self.moved(); return None;
        }
        if self.minimap_press(x, y) { return None; }
        if self.horizontal_bar_press(x, y) { return None; }
        if self.on_bar(x, y) {
            // On the thumb it is held where it was taken; elsewhere on the
            // bar the thumb comes to the pointer.
            let (top, len, _) = self.bar.unwrap();
            let grab = if y >= self.y + top && y < self.y + top + len { y - self.y - top } else { len / 2.0 };
            self.drag = Drag::Bar(grab);
            self.drag_bar(y, grab);
            return None;
        }
        if let Some(id) = self.test_under(x, y) {
            self.test_request = Some(id);
            return None;
        }
        if let Some((li, top)) = self.line_at(y) {
            // A click on a rendered checkbox ticks it instead of moving the cursor.
            if let Some(Some(lay)) = self.cache.get(li) {
                let (cx, ls) = (self.text_screen_x(0.0), self.buf.starts[li]);
                if let Some((r, off)) = &lay.checkbox {
                    let (px, py) = (x - cx - r.x, y - top - r.y);
                    if clicks == 1 && px >= 0.0 && py >= 0.0 && px < r.w && py < r.h {
                        let at = ls + *off as usize;
                        let tick = if self.buf.text[at..].starts_with(' ') { "x" } else { " " };
                        let keep = (self.buf.cur, self.buf.anchor);
                        if self.edit(at, at + 1, tick, EditKind::Other) {
                            (self.buf.cur, self.buf.anchor) = keep;
                        }
                        return None;
                    }
                }
            }
        }
        let off = self.offset_at(x, y);
        self.extra_cursors.clear();
        if mods.alt && mods.shift {
            self.buf.anchor = off;
            self.buf.cur = off;
            self.drag = Drag::Column(off);
            return None;
        }
        if mods.ctrl {
            if let Some(url) = self.md.as_ref().and_then(|md| md.link_at(off)).map(|l| l.url.clone()) {
                let wiki = self.md.as_ref().and_then(|md| md.link_at(off)).is_some_and(|l| self.buf.text[l.start as usize..].starts_with("[["));
                return Some(if wiki { format!("lightdown-note:{url}") } else { url });
            }
        }
        match clicks {
            1 => {
                self.set_cursor(off, mods.shift);
                self.drag = Drag::Char;
            }
            2 => {
                let (a, b) = editor::word_at(&self.buf.text, off);
                (self.buf.anchor, self.buf.cur) = (a, b);
                self.drag = Drag::Unit(a, b, false);
            }
            _ => {
                let (a, b) = self.buf.line(self.buf.line_of(off));
                let b = (b + 1).min(self.buf.text.len());
                (self.buf.anchor, self.buf.cur) = (a, b);
                self.drag = Drag::Unit(a, b, true);
            }
        }
        None
    }

    pub fn motion(&mut self, x: f32, y: f32) {
        if let Some(m) = &mut self.merge { m.pointer = (x, y); }
        let hot = self.on_bar(x, y) || matches!(self.drag, Drag::Bar(_));
        if hot != self.bar_hot {
            self.bar_hot = hot;
            self.redraw = true;
        }
        if self.hover_horizontal_bar(x, y) {
            self.redraw = true;
        }
        if let Drag::Minimap(grab) = self.drag { return self.minimap_drag(y, grab); }
        if let Drag::Horizontal(grab) = self.drag {
            return self.drag_horizontal(x, grab);
        }
        if let Drag::Bar(grab) = self.drag {
            return self.drag_bar(y, grab);
        }
        if matches!(self.drag, Drag::None) {
            return;
        }
        let (y0, y1) = self.view();
        if y < y0 {
            self.scroll(y - y0);
        } else if y > y1 {
            self.scroll(y - y1);
        }
        self.drag_across_edges(x);
        let off = self.offset_at(x, y.clamp(y0, y1 - 1.0));
        match self.drag {
            Drag::Column(from) => {
                self.column_select(from, off);
                return;
            }
            Drag::Unit(a, b, lines) => {
                let (s, e) = if lines {
                    let (s, e) = self.buf.line(self.buf.line_of(off));
                    (s, (e + 1).min(self.buf.text.len()))
                } else {
                    editor::word_at(&self.buf.text, off)
                };
                (self.buf.anchor, self.buf.cur) = if off < a { (b, s) } else { (a, e.max(b)) };
            }
            _ => self.buf.cur = off,
        }
        self.redraw = true;
    }

    pub fn release(&mut self) {
        if matches!(self.drag, Drag::Bar(_) | Drag::Minimap(_)) {
            self.drag = Drag::None;
            self.redraw = true;
        } else if !matches!(self.drag, Drag::None) {
            self.drag = Drag::None;
            self.refocus();
            self.redraw = true;
        }
    }

    /// Whether a selection is being made with the pointer.
    pub fn selecting(&self) -> bool {
        matches!(self.drag, Drag::Char | Drag::Unit(..) | Drag::Column(_))
    }

    pub fn leave(&mut self) {
        if self.bar_hot {
            self.bar_hot = false;
            self.redraw = true;
        }
    }

    // ---- painting -------------------------------------------------------

    /// Draws the document into the pane's rectangle.
    /// What the debugger writes at the end of a line while paused: the
    /// exception stopped on, in red, or the values used there, dimmed.
    fn debug_label(&self, line: usize) -> Option<(String, u32)> {
        if let Some((l, text)) = &self.debug_exception && *l == line { return Some((text.clone(), theme::danger())); }
        self.debug_values.iter().find(|(l, _)| *l == line).map(|(_, text)| (text.clone(), theme::dim()))
    }

    /// Small text placed after a line's end, cut to the pane's width.
    fn draw_line_end(&self, c: &mut Canvas, x: f32, mid: f32, label: &str, color: u32) {
        let px = self.px();
        let size = (px * 0.75).round();
        let mut fonts = self.fonts.borrow_mut();
        let label = crate::ui::fit(&mut fonts, crate::font::UI, size, label, (self.x + self.w - x - px).max(0.0));
        crate::ui::text(c, &mut fonts, x, mid + size * 0.32, &label, crate::font::UI, size, color);
    }

    pub fn draw(&mut self, c: &mut Canvas) {
        if self.draw_comparison(c) { return; }
        self.draw_merge(c);
        if self.w < 1.0 || self.h < 1.0 {
            return;
        }
        c.clip(self.x, self.y, self.w, self.h);
        if self.follow && self.at_tail {
            (self.top, self.top_off) = (self.buf.lines() - 1, f32::MAX);
        }
        self.clamp_scroll();
        let fixed_cx = self.column().0;
        let cx = self.text_screen_x(0.0);
        let (y0, y1) = self.view();
        let px = self.px();
        let (s0, s1) = self.buf.sel();
        let cur = self.buf.cur;
        let cursor_line = self.buf.line_of(cur);
        let mut paragraph = (cursor_line, cursor_line);
        if self.focus_paragraph {
            let blank = |line| { let (a,b) = self.buf.line(line); self.buf.text[a..b].trim().is_empty() };
            while paragraph.0 > 0 && !blank(paragraph.0-1) { paragraph.0 -= 1; }
            while paragraph.1+1 < self.buf.lines() && !blank(paragraph.1+1) { paragraph.1 += 1; }
        }
        let word_marks = if self.focused && self.numbers { self.occurrence_marks().to_vec() } else { Vec::new() };
        let brackets = self.matching_brackets();
        let mut highlights = self.found.clone();
        if let Some(m) = &self.multi {
            highlights.extend(m.marks.iter().copied());
        }
        highlights.extend(word_marks);
        if self.focused {
            highlights.extend(brackets.iter().map(|&p| (p, p + 1)));
        }
        highlights.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (a, b) in highlights {
            if let Some(last) = merged.last_mut().filter(|last| a <= last.1) {
                last.1 = last.1.max(b);
            } else {
                merged.push((a, b));
            }
        }
        let highlights = merged;
        let caret = (px * 0.09).round().max(2.0);

        let cw = {
            let mut f = self.fonts.borrow_mut();
            let face = f.mono();
            f.width(face, px, " ")
        };
        if self.numbers && self.mono() && self.ruler > 0 {
            self.clip_text(c);
            c.line(cx + cw * self.ruler as f32, y0, self.scale.max(1.0), y1 - y0, theme::line());
            self.clip_pane(c);
        }
        let indent = self.indentation()
            .map_or(4, |s| if s == "\t" { 4 } else { s.len().max(1) });
        self.visible.clear();
        let mut y = (y0 - self.top_off).round();
        let mut li = self.top;
        while li < self.buf.lines() && y < y1 {
            let lay = self.layout(li);
            if lay.h > 0.0 {
                self.visible.push((li, y));
            }
            let ls = self.buf.starts[li];
            if y + lay.h > y0 {
                for r in &lay.under {
                    under(c, cx + r.x, y + r.y, r, self.scale);
                }
                let span = if lay.carets.is_empty() { lay.clusters.last().map_or(0,|c| c.end as usize) }
                    else { lay.clusters.iter().map(|cl| cl.end as usize).max().unwrap_or(0) };
                // The line the cursor is on, in code, lies on a quiet band.
                if self.focused && self.numbers && self.mono() && li == cursor_line && s0 == s1 {
                    c.rect(self.x, y, self.w, lay.h, theme::hover());
                }
                let (row_y, row_h) = lay.rows.first().map_or((0.0, lay.h), |r| (r.y, r.h));
                self.draw_file_blame(c, li, y, lay.h, y + row_y, row_h);
                if let Some(&(a, b)) = self.folds.iter().find(|&&(a, _)| a == li) {
                    let mut fonts = self.fonts.borrow_mut();
                    let x = cx + lay.rows.first().map_or(0.0, |r| r.x1) + px;
                    crate::ui::text(
                        c,
                        &mut fonts,
                        x,
                        y + px,
                        &format!("… {} lines", b - a),
                        crate::font::UI,
                        px * 0.75,
                        theme::dim(),
                    );
                }
                if self.debug_line == Some(li) {
                    c.rect(self.x, y, self.w, lay.h, theme::found());
                }
                if self.breakpoints.contains(&li) {
                    c.round_rect(
                        fixed_cx - self.marks_gutter() + 6.0 * self.scale,
                        y + px * 0.4,
                        px * 0.5,
                        px * 0.5,
                        px * 0.25,
                        theme::danger(),
                    );
                }
                self.clip_text(c);
                if self.numbers && self.mono() && self.indent_guides {
                    let end = self.buf.line(li).1;
                    let columns = self.buf.text[ls..end]
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .fold(0, |n, c| if c == '\t' { n + indent - n % indent } else { n + 1 });
                    for col in (0..columns).step_by(indent).take(200) {
                        c.line(cx + cw * col as f32, y, self.scale.max(1.0), lay.h, theme::line());
                    }
                }
                if let Some((a, b)) = self.find_scope.filter(|_| self.show_find_scope) {
                    if a <= ls + span && b > ls {
                        c.rect(self.x, y, self.w, lay.h, theme::tint());
                    }
                }
                // Matches of a search lie on a fill of their own.
                let from = highlights.partition_point(|m| m.1 <= ls);
                for &(fa, fb) in highlights[from..].iter().take_while(|m| m.0 <= ls + span) {
                    let (a, b) = (fa.saturating_sub(ls) as u32, (fb - ls) as u32);
                    for row in &lay.rows {
                        let cl = &lay.clusters[row.first as usize..row.last as usize];
                        let mut hit = cl.iter().filter(|k| k.start < b && k.end > a);
                        let Some(first) = hit.next() else { continue };
                        let (l, r) = hit.fold((first.x, first.x + first.w), |(l, r), k| (l.min(k.x), r.max(k.x + k.w)));
                        c.round_rect(
                            cx + l,
                            y + row.y + 1.0,
                            r - l,
                            row.h - 2.0,
                            (2.0 * self.scale).round(),
                            theme::found(),
                        );
                    }
                }
                for (s0, s1) in
                    std::iter::once((s0, s1)).chain(self.extra_cursors.iter().map(|&(a, b)| (a.min(b), a.max(b))))
                {
                    let selected = s0 != s1 && s0 <= ls + span && s1 >= ls;
                    if selected {
                        let (a, b) = (s0.saturating_sub(ls) as u32, (s1 - ls) as u32);
                        for (ri, row) in lay.rows.iter().enumerate() {
                            if !lay.carets.is_empty() {
                                for (l,r) in lay.selection_spans(ri,a,b) {
                                    c.rect(cx+l,y+row.y,r-l,row.h,theme::selection());
                                }
                                if !lay.grid && ri+1 == lay.rows.len() && b > row.end {
                                    let x = lay.locate(row.end).1;
                                    c.rect(cx+x,y+row.y,px*0.4,row.h,theme::selection());
                                }
                                continue;
                            }
                            let cl = &lay.clusters[row.first as usize..row.last as usize];
                            let mut hit = cl.iter().filter(|k| k.start < b && k.end > a);
                            let mut span = hit.next().map(|k| (k.x, k.x + k.w));
                            for k in hit {
                                span = span.map(|(l, r)| (l.min(k.x), r.max(k.x + k.w)));
                            }
                            // A selected line break shows as a sliver past the text.
                            if !lay.grid && ri + 1 == lay.rows.len() && a <= row.end && b > row.end {
                                let end = cl.last().map_or(row.text_x, |k| k.x + k.w);
                                span = Some(span.map_or((end, end + px * 0.4), |(l, r)| (l, r.max(end + px * 0.4))));
                            }
                            if let Some((l, r)) = span {
                                c.rect(cx + l, y + row.y, r - l, row.h, theme::selection());
                            }
                        }
                    }
                }
                let mut fonts = self.fonts.borrow_mut();
                let diff_color = self
                    .diff
                    .then(|| {
                        let line = &self.buf.text[ls..self.buf.line(li).1];
                        if line.starts_with('+') {
                            Some(theme::success())
                        } else if line.starts_with('-') {
                            Some(theme::danger())
                        } else if line.starts_with("@@") {
                            Some(theme::info())
                        } else {
                            None
                        }
                    })
                    .flatten();
                for g in &lay.glyphs {
                    let gx = cx + g.x;
                    let ix = gx.floor();
                    let bmp = fonts.glyph(g.face, g.id, g.size, ((gx - ix) * 4.0) as u8);
                    let color = if self.focus_paragraph && (li < paragraph.0 || li > paragraph.1)
                        && !(s0 != s1 && s0 <= ls+span && s1 > ls) { theme::dim() }
                        else { diff_color.unwrap_or(g.color) };
                    c.glyph(ix as i32, (y + g.y) as i32, bmp, color);
                }
                drop(fonts);
                let from = self.misspelled.partition_point(|m| m.1 <= ls);
                // Warnings wave in amber, everything else in red.
                let ink = |m: &str| if m.starts_with("Warning:") { theme::warning() } else { theme::danger() };
                for (a, b, wave) in self.misspelled[from..].iter().map(|&(a, b)| (a, b, theme::danger())).take_while(|m| m.0 < ls + span).chain(
                    self.diagnostics
                        .iter().chain(self.broken_links.iter())
                        .map(|d| (d.0, d.1.max(crate::editor::next(&self.buf.text, d.0)), ink(&d.2)))
                        .filter(|m| m.0 < ls + span && m.1 > ls),
                ) {
                    let (a, b) = ((a - ls.min(a)) as u32, (b - ls) as u32);
                    for row in &lay.rows {
                        let cl = &lay.clusters[row.first as usize..row.last as usize];
                        let mut hit = cl.iter().filter(|k| k.start >= a && k.end <= b && !k.space);
                        let Some(first) = hit.next() else { continue };
                        let (l, r) = hit.fold((first.x, first.x + first.w), |(l, r), k| (l.min(k.x), r.max(k.x + k.w)));
                        let base = y + row.y + row.h - (px * 0.12).round().max(2.0);
                        squiggle(c, cx + l, cx + r, base, self.scale, wave);
                    }
                }
                self.clip_pane(c);
                self.draw_bookmark(c, li, fixed_cx - self.marks_gutter(), y, &lay);
                if let Some((a,_,jump)) = self.prediction.filter(|(a,_,_)| self.buf.line_of(*a) == li) {
                    c.rect(fixed_cx-self.marks_gutter()+self.scale,y,3.0*self.scale,lay.h.min(px*1.6),theme::success());
                    if jump {
                        let label = "Tab to jump";
                        let x = cx+lay.rows.first().map_or(0.0,|r| r.text_x)+lay.rows.first().map_or(0.0,|r| r.x1);
                        let _ = a;
                        let mut fonts = self.fonts.borrow_mut();
                        ui::text(c,&mut fonts,x+12.0*self.scale,y+px,label,font::UI,theme::SMALL*self.scale,theme::success());
                    }
                }
                // Coverage shades the number's column: green ran, red did not.
                if self.numbers
                    && let Ok(index) = self.coverage_marks.binary_search_by_key(&li, |(line, _)| *line)
                {
                    let (color, alpha) = if self.coverage_marks[index].1 { (theme::success(), 56) } else { (theme::danger(), 72) };
                    let x = fixed_cx - self.marks_gutter() + 5.0 * self.scale;
                    // The line's own rows, not the padding above the first line.
                    let top = lay.rows.first().map_or(0.0, |r| r.y);
                    let bottom = lay.rows.last().map_or(lay.h, |r| r.y + r.h);
                    c.shade(x, y + top, (self.marks_gutter() - 5.0 * self.scale - px * 0.3).max(0.0), bottom - top, color, alpha);
                }
                if self.numbers
                    && let Some(number) = self.source_number(li)
                    && let Some(row) = lay.rows.first()
                {
                    let text = number.to_string();
                    let mut fonts = self.fonts.borrow_mut();
                    let face = fonts.mono();
                    let size = (px * 0.72).round().max(1.0);
                    let width = fonts.width(face, size, &text);
                    crate::ui::text(
                        c,
                        &mut fonts,
                        fixed_cx - px * 0.65 - width,
                        y + row.y + row.h / 2.0 + size * 0.32,
                        &text,
                        face,
                        size,
                        if li == cursor_line { theme::fg() } else { theme::faint() },
                    );
                }
                if let Some((_, _, label)) = self.test_marks.iter().find(|(line, _, _)| *line == li) {
                    let mut fonts = self.fonts.borrow_mut();
                    let color = if label == "×" {
                        theme::danger()
                    } else if label == "✓" {
                        theme::success()
                    } else {
                        theme::primary()
                    };
                    crate::ui::text(
                        c,
                        &mut fonts,
                        fixed_cx - self.marks_gutter() + 4.0 * self.scale,
                        y + px,
                        label,
                        crate::font::UI,
                        px * 0.8,
                        color,
                    );
                }
                if self.numbers
                    && let Ok(index) = self.git_marks.binary_search_by_key(&li, |(line, _)| *line)
                {
                    use crate::app::repository::Mark;
                    let mark = self.git_marks[index].1;
                    let color = match mark {
                        Mark::Added => theme::success(),
                        Mark::Modified => theme::warning(),
                        Mark::Deleted => theme::danger(),
                    };
                    let x = fixed_cx - self.marks_gutter() + self.scale;
                    let h = if mark == Mark::Deleted { 3.0 * self.scale } else { lay.h.max(3.0 * self.scale) };
                    c.rect(x, y, 3.0 * self.scale, h, color);
                }
                self.clip_text(c);
                let debug_label = self.debug_label(li);
                if let Some((line, label)) = &self.blame
                    && *line == li
                    && li == cursor_line
                    && debug_label.is_none()
                    && let Some(row) = lay.rows.last()
                {
                    let x = self.line_end_x(&lay, row);
                    let size = (px * 0.75).round();
                    let mut fonts = self.fonts.borrow_mut();
                    let label =
                        crate::ui::fit(&mut fonts, crate::font::UI, size, label, (self.x + self.w - x - px).max(0.0));
                    crate::ui::text(
                        c,
                        &mut fonts,
                        x,
                        y + row.y + row.h / 2.0 + size * 0.32,
                        &label,
                        crate::font::UI,
                        size,
                        theme::dim(),
                    );
                }
                if let Some((label, color)) = &debug_label
                    && let Some(row) = lay.rows.last()
                {
                    self.draw_line_end(c, self.line_end_x(&lay, row), y + row.y + row.h / 2.0, label, *color);
                }
                // A review comment follows its line, unless blame or a
                // paused value is shown there.
                if let Ok(i) = self.review_notes.binary_search_by_key(&li, |(line, _)| *line)
                    && debug_label.is_none()
                    && !(li == cursor_line && self.blame.as_ref().is_some_and(|(line, _)| *line == li))
                    && let Some(row) = lay.rows.last()
                {
                    let x = self.line_end_x(&lay, row);
                    let size = (px * 0.75).round();
                    let mut fonts = self.fonts.borrow_mut();
                    let note = format!("◆ {}", self.review_notes[i].1);
                    let note = crate::ui::fit(&mut fonts, crate::font::UI, size, &note, (self.x + self.w - x - px).max(0.0));
                    crate::ui::text(c, &mut fonts, x, y + row.y + row.h / 2.0 + size * 0.32, &note, crate::font::UI, size, theme::warning());
                }
                // A failed test's message follows the line it failed on,
                // dimmed, unless blame, a review comment or a paused value is
                // shown there.
                if let Ok(i) = self.test_notes.binary_search_by_key(&li, |(line, _)| *line)
                    && debug_label.is_none()
                    && !(li == cursor_line && self.blame.as_ref().is_some_and(|(line, _)| *line == li))
                    && self.review_notes.binary_search_by_key(&li, |(line, _)| *line).is_err()
                    && let Some(row) = lay.rows.last()
                {
                    let x = self.line_end_x(&lay, row);
                    let size = (px * 0.75).round();
                    let mut fonts = self.fonts.borrow_mut();
                    let note = crate::ui::fit(&mut fonts, crate::font::UI, size, &self.test_notes[i].1, (self.x + self.w - x - px).max(0.0));
                    let ink = theme::mix(theme::bg(), theme::danger(), 200);
                    crate::ui::text(c, &mut fonts, x, y + row.y + row.h / 2.0 + size * 0.32, &note, crate::font::UI, size, ink);
                }
                for r in &lay.over {
                    c.line(cx + r.x, y + r.y, r.w, r.h, r.color);
                }
                for im in &lay.images {
                    let (ix, iy) = (cx + im.x, y + im.y);
                    if iy + im.h <= y0 || iy >= y1 {
                        continue;
                    }
                    match self.pics.borrow_mut().get(&im.path, im.w as u32, im.h as u32) {
                        Some(pic) => c.image(ix as i32, iy as i32, im.w as i32, im.h as i32, pic),
                        // Still loading: hold its place.
                        None => {
                            c.rect(ix, iy, im.w, im.h, theme::inset());
                            c.frame(ix, iy, im.w, im.h, 1.0, theme::line());
                        }
                    }
                    if s0 <= ls + im.at as usize && s1 > ls + im.at as usize {
                        c.frame(ix, iy, im.w, im.h, (px * 0.12).round().max(2.0), theme::primary());
                    }
                }
                if self.focused {
                    let block = self.vim_block();
                    for cur in std::iter::once(block.unwrap_or(cur)).chain(self.extra_cursors.iter().map(|&(_, cur)| cur)) {
                        if li == self.buf.line_of(cur) && !lay.rows.is_empty() {
                            let (row, x) = lay.locate((cur - ls) as u32);
                            let r = &lay.rows[row];
                            let inset = (r.h * 0.12).round();
                            if block == Some(cur) {
                                let w = self.vim_block_width(&lay, ls, cur);
                                c.shade(cx + x, y + r.y + inset, w, r.h - inset * 2.0, theme::caret(), 150);
                                continue;
                            }
                            c.rect(cx + x - caret / 2.0, y + r.y + inset, caret, r.h - inset * 2.0, theme::caret());
                        }
                    }
                }
                self.clip_pane(c);
            }
            y += lay.h;
            li += 1;
        }
        self.measure_widths();
        self.draw_horizontal_bar(c);
        if self.minimap_width() == 0.0 { self.scrollbar(c, li); }
        self.draw_minimap(c);
        c.unclip();
        self.excerpts_draw(c);
        self.sticky_draw(c);
    }

    fn scrollbar(&mut self, c: &mut Canvas, bottom_line: usize) {
        let n = self.buf.lines() as f32;
        let shown = (bottom_line - self.top) as f32;
        if shown >= n {
            self.bar = None;
            return;
        }
        let len = (self.h * shown / n).max(24.0 * self.scale).min(self.h);
        let pos = (self.h - len) * (self.top as f32 / (n - shown).max(1.0)).min(1.0);
        // Under the pointer it widens and brightens, to show it can be taken.
        let (w, ink) = match self.bar_hot {
            true => ((8.0 * self.scale).round(), theme::mix(theme::bg(), theme::fg(), 70)),
            false => ((6.0 * self.scale).round(), theme::mix(theme::bg(), theme::fg(), 34)),
        };
        let inset = (3.0 * self.scale).round();
        let (top, len) = (self.y + pos + inset, (len - inset * 2.0).max(w));
        c.round_rect(self.x + self.w - w - inset, top, w, len, w / 2.0, ink);
        self.bar = Some((pos, len, shown as usize));
    }
}

/// Paints a rectangle laid under a line's text, rounding the corners it
/// asks for: code panels where their block opens and closes, inline code
/// all round.
fn under(c: &mut Canvas, x: f32, y: f32, r: &crate::layout::Rect, scale: f32) {
    use crate::layout::{BOTTOM, TOP};
    let radius = (theme::RADIUS_MD * scale).round().min((r.h / 2.0).floor()).min((r.w / 2.0).floor());
    if r.round & crate::layout::SOFT != 0 {
        return c.round_rect(x, y, r.w, r.h, (r.h * 0.22).round(), r.color);
    }
    match r.round {
        0 => c.rect(x, y, r.w, r.h, r.color),
        TOP => {
            c.round_rect(x, y, r.w, (radius * 2.0).min(r.h), radius, r.color);
            c.rect(x, y + radius, r.w, r.h - radius, r.color);
        }
        BOTTOM => {
            c.rect(x, y, r.w, r.h - radius, r.color);
            let h = (radius * 2.0).min(r.h);
            c.round_rect(x, y + r.h - h, r.w, h, radius, r.color);
        }
        _ => c.round_rect(x, y, r.w, r.h, radius, r.color),
    }
}

/// A wavy line from `x0` to `x1` with its middle at `y`, under a word.
fn squiggle(c: &mut Canvas, x0: f32, x1: f32, y: f32, scale: f32, color: u32) {
    let (step, amp) = ((2.0 * scale).round().max(2.0), (1.0 * scale).round().max(1.0));
    let dot = scale.round().max(1.0);
    let mut x = x0.floor();
    let mut i = 0;
    while x < x1 {
        let up = i % 2 == 0;
        c.rect(x, y + if up { -amp } else { amp }, step, dot, color);
        x += step;
        i += 1;
    }
}

pub fn resolve(doc: Option<&Path>, input: &str) -> PathBuf {
    let p = match input.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(input),
    };
    match doc.and_then(Path::parent) {
        Some(dir) if p.is_relative() => dir.join(p),
        _ => p,
    }
}

/// The local file an image source refers to. Remote images are not fetched.
pub fn image_file(doc: Option<&Path>, url: &str) -> Option<PathBuf> {
    let local = match url.strip_prefix("file://") {
        Some(path) => path,
        None if url.contains("://") || url.starts_with("data:") => return None,
        None => url,
    };
    (!local.is_empty()).then(|| resolve(doc, &percent_decode(local)))
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = s.get(i + 1..i + 3).filter(|_| b[i] == b'%').and_then(|h| u8::from_str_radix(h, 16).ok());
        out.push(hex.unwrap_or(b[i]));
        i += if hex.is_some() { 3 } else { 1 };
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.numbers = true;
        p.set_rect(0.0, 0.0, 1600.0, 800.0, 1.0);
        p
    }

    #[test]
    fn a_cursor_placed_before_the_pane_has_a_size_is_shown_once_it_has_one() {
        let text = format!("# Notes\n\n{}", "line\n".repeat(60));
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text(text.clone(), Doc::Markdown);
        p.select(0, 0);
        p.set_rect(0.0, 0.0, 900.0, 300.0, 1.0);
        assert_eq!((p.top, p.top_off), (0, 0.0), "the first line stays in view");
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text(text, Doc::Markdown);
        let at = p.buf.starts[40];
        p.select(at, at);
        p.set_rect(0.0, 0.0, 900.0, 300.0, 1.0);
        assert!(p.top > 20 && p.top <= 40, "line 41 is scrolled to, top {}", p.top);
    }

    #[test]
    fn document_columns_are_one_and_a_half_times_wider_and_leave_a_number_gutter() {
        let mut p = pane();
        p.set_text("text".into(), Doc::Markdown);
        assert_eq!(p.column().1, (theme::COLUMN * theme::BODY * 1.5).round());
        p.set_text(format!("{}\nprint(1)", cells::header(Tag::Code, "")), Doc::Cells);
        p.headed = true;
        assert_eq!(p.column().1, (theme::COLUMN * theme::BODY * 1.5).round());
        p.set_rect(0.0, 0.0, 320.0, 400.0, 1.0);
        let (left, width) = p.column();
        assert!(left >= p.gutter() && left + width <= 320.0);
    }

    #[test]
    fn source_numbers_skip_headers_and_restart_in_each_cell() {
        let mut p = pane();
        let source = format!("{}\na\nb\n{}\ntext\n", cells::header(Tag::Code, ""), cells::header(Tag::Markdown, ""));
        p.set_text(source, Doc::Cells);
        p.headed = true;
        let numbers: Vec<_> = (0..p.buf.lines()).map(|li| p.source_number(li)).collect();
        assert_eq!(numbers, [None, Some(1), Some(2), None, Some(1), Some(2)]);
        p.set_text("line one wraps across many screen rows\nsecond".into(), Doc::Code);
        assert_eq!(p.source_number(0), Some(1));
        assert_eq!(p.source_number(1), Some(2));
    }
}
