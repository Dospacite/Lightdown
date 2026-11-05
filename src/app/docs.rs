//! Documentation at hand in Python, as an IDE gives it: what a name is when
//! the pointer rests on it or Ctrl+K asks, the parameters of the call being
//! typed, and the documentation of the name chosen in the completion list.
//! The Python helper finds it all; nothing here runs the user's code.

use std::time::{Duration, Instant};

use super::App;
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::kernel::{Info, Kernel};
use crate::pane::Doc;
use crate::{py, theme, ui};
use serde_json::Value;

/// How long the pointer rests on a name before its documentation shows.
const HOVER_DELAY: Duration = Duration::from_millis(450);

#[derive(Default)]
pub(super) struct Docs {
    seq: u64,
    hover: Option<Hover>,
    hint: Option<Hint>,
    /// The documentation of the completion chosen: its name, the request
    /// for it, and the answer once there is one.
    chosen: Option<(String, u64, Option<Info>)>,
    /// Where the hover box was last drawn, which the pointer may enter.
    hover_box: Option<(f32, f32, f32, f32)>,
}

/// A name's documentation, for the pointer resting on it or for Ctrl+K.
struct Hover {
    range: (usize, usize),
    /// When the pointer came to rest on the name; `None` when asked by key.
    since: Option<Instant>,
    /// The request, once made.
    seq: Option<u64>,
    info: Option<Info>,
    /// How far the box has been scrolled, in pixels.
    scroll: f32,
    /// Whether all of it fits; it is not scrolled further than that.
    room: f32,
}

/// The signature of the call the cursor is in, with the argument being
/// typed picked out.
struct Hint {
    open: usize,
    seq: u64,
    info: Option<Info>,
    index: usize,
    keyword: Option<String>,
}

/// A line of a documentation box: pieces of text, each in its own face and
/// colour, and the line's height.
struct Line {
    pieces: Vec<(String, u8, u32)>,
    size: f32,
    indent: f32,
    gap: f32,
}

impl App {
    /// The helper that knows the document's Python: a notebook's kernel, or
    /// the one that also completes names in files.
    pub(super) fn python_kernel(&mut self) -> Option<&mut Kernel> {
        let (python, dir) = self.python();
        match &mut self.nb {
            Some(nb) => {
                nb.env_root = Some(self.root.clone());
                nb.kernel(&python, &dir, &self.waker)
            }
            None => {
                if self.helper.is_none() {
                    self.helper = Kernel::spawn_in(&python, &dir, &self.root, self.waker.clone()).ok();
                }
                self.helper.as_mut()
            }
        }
    }

    /// Asks what is known of the name ending at `end` in the document, or
    /// in `code` given in its stead for the stretch around `end`. For a call,
    /// `paren` is the offset of its bracket. Returns the request's number.
    fn ask_info(&mut self, end: usize, code: Option<(String, usize)>, paren: Option<usize>) -> Option<u64> {
        let (s, e) = self.scope(end);
        let text = &self.doc.buf.text;
        let (code, end) = code.unwrap_or_else(|| (text[s..e].to_string(), end - s));
        let line_start = code[..end].rfind('\n').map_or(0, |i| i + 1);
        let line = code[..end].matches('\n').count() + 1;
        let col = code[line_start..end].chars().count();
        // jedi wants the place just inside the bracket, on the same line.
        let paren = paren
            .map(|p| p - s)
            .filter(|&p| p >= line_start && p < code.len() && !code[line_start..p].contains('\n'))
            .map(|p| code[line_start..=p].chars().count());
        self.docs.seq += 1;
        let seq = self.docs.seq;
        let (live, path) = (self.nb.is_some(), self.doc.path.clone());
        let k = self.python_kernel()?;
        k.info(seq, &code, line, col, live, path.as_deref(), paren);
        Some(seq)
    }

    /// Takes in an answer from the helper.
    pub(super) fn docs_answer(&mut self, id: u64, info: Option<Info>) {
        let d = &mut self.docs;
        if let Some(h) = d.hover.as_mut().filter(|h| h.seq == Some(id)) {
            if info.is_none() && h.since.is_none() {
                let name = self.doc.buf.text.get(h.range.0..h.range.1).unwrap_or("this name");
                self.message = Some(format!("Nothing is known of {name}"));
                d.hover = None;
            } else {
                h.info = info;
            }
        } else if let Some(h) = d.hint.as_mut().filter(|h| h.seq == id) {
            match info {
                Some(i) => h.info = Some(i),
                None => d.hint = None,
            }
        } else if let Some(c) = d.chosen.as_mut().filter(|c| c.1 == id) {
            c.2 = info;
        }
    }

    /// When the pointer, resting on a name, will have rested long enough.
    pub(super) fn docs_due(&self) -> Option<Instant> {
        let h = self.docs.hover.as_ref().filter(|h| h.seq.is_none())?;
        Some(h.since? + HOVER_DELAY)
    }

    /// Asks about the name under the pointer once it has rested there.
    pub fn tick(&mut self) {
        self.recovery_poll();
        self.watch_tick();
        self.remote_tick();
        self.links_tick(false);
        self.diagrams_tick(false);
        if !cfg!(test) {
            self.language_sync();
            self.lightbulb_tick();
        }
        self.docs_tick(false);
        self.git_tick();
        self.git_marks_tick();
        self.disk_poll(false);
        self.looks_tick();
    }

    fn docs_tick(&mut self, now: bool) {
        if !self.docs_due().is_some_and(|due| now || due <= Instant::now()) {
            return;
        }
        let end = self.docs.hover.as_ref().unwrap().range.1;
        if let Some(info) = self.config_info(end).or_else(||self.citation_info(end)) {
            let hover = self.docs.hover.as_mut().unwrap(); hover.info = Some(info); hover.seq = Some(u64::MAX); self.redraw = true; return;
        }
        // While paused, the debugger answers for names in the paused file.
        let range = self.docs.hover.as_ref().unwrap().range;
        if self.debug_hover_ask(range) {
            self.docs.hover.as_mut().unwrap().seq = Some(u64::MAX);
            return;
        }
        if !self.doc.python_at(end) {
            self.docs.hover.as_mut().unwrap().seq = Some(u64::MAX);
            self.language_pointer_hover(end);
            return;
        }
        let seq = self.ask_info(end, None, None);
        match (seq, self.docs.hover.as_mut()) {
            (Some(seq), Some(h)) => h.seq = Some(seq),
            _ => self.docs.hover = None,
        }
    }

    /// The pointer has moved: a name under it is documented once it rests
    /// there, and the box stays while the pointer is on the name or in it.
    pub(super) fn docs_motion(&mut self, x: f32, y: f32) {
        if self.docs.hover_box.is_some_and(|r| ui::inside(r, Some((x, y)))) {
            return;
        }
        let usable = !self.overlaid() && self.reader.is_none() && self.prompt.is_none();
        let name = usable.then(|| self.doc.name_under(x, y)).flatten();
        let d = &mut self.docs;
        match (&d.hover, name) {
            (Some(h), Some(r)) if h.range == r => {}
            // One asked for by key stays until the cursor moves.
            (Some(h), None) if h.since.is_none() => {}
            (_, name) => {
                if d.hover.as_ref().is_some_and(|h| h.info.is_some()) {
                    self.redraw = true;
                }
                d.hover = name.map(|range| Hover {
                    range,
                    since: Some(Instant::now()),
                    seq: None,
                    info: None,
                    scroll: 0.0,
                    room: 0.0,
                });
            }
        }
    }

    /// The wheel over the hover box scrolls it. Returns whether it was there.
    pub(super) fn docs_scroll(&mut self, dy: f32) -> bool {
        let at = Some(self.pointer);
        let Some(h) = self.docs.hover.as_mut().filter(|_| self.docs.hover_box.is_some_and(|r| ui::inside(r, at)))
        else {
            return false;
        };
        h.scroll = (h.scroll + dy).clamp(0.0, h.room.max(0.0));
        self.redraw = true;
        true
    }

    /// A press in the hover box is kept from the document under it.
    pub(super) fn docs_press(&self, x: f32, y: f32) -> bool {
        self.docs.hover.is_some() && self.docs.hover_box.is_some_and(|r| ui::inside(r, Some((x, y))))
    }

    /// Ctrl+K: the documentation of the name at the cursor or, where there
    /// is none, the signature of the call the cursor is in.
    pub(super) fn show_docs(&mut self) {
        let cur = self.doc.buf.cur;
        if let Some(info) = self.config_info(cur) {
            self.docs.hover = Some(Hover { range: (cur,cur), since: None, seq: Some(u64::MAX), info: Some(info), scroll: 0.0, room: 0.0 }); self.redraw = true; return;
        }
        if let Some(info) = self.citation_info(cur) {
            let (a, b, _) = crate::citations::at(&self.doc.buf.text, cur).unwrap();
            self.docs.hover = Some(Hover { range: (a, b), since: None, seq: Some(u64::MAX), info: Some(info), scroll: 0.0, room: 0.0 }); self.redraw = true; return;
        }
        if !self.doc.python_at(cur) {
            self.language_command("hover");
            return;
        }
        if !self.doc.python_at(cur) || self.reader.is_some() {
            self.message = Some("Documentation is shown for Python names".into());
            return;
        }
        if let Some(range) = py::name_at(&self.doc.buf.text, cur) {
            self.comp = None;
            let seq = self.ask_info(range.1, None, None);
            self.docs.hover =
                seq.map(|s| Hover { range, since: None, seq: Some(s), info: None, scroll: 0.0, room: 0.0 });
        } else if !self.hint_update(true) {
            self.message = Some("No name or call at the cursor".into());
        }
    }

    /// Esc takes away what is shown. Returns whether there was anything.
    pub(super) fn docs_escape(&mut self) -> bool {
        let had = self.docs.hover.as_ref().is_some_and(|h| h.info.is_some()) || self.docs.hint.is_some();
        self.docs.hover = None;
        self.docs.hint = None;
        had
    }

    /// The document is being typed into: a pointer's box goes out of the
    /// way, and typing a call's bracket or comma shows its parameters.
    pub(super) fn docs_typed(&mut self, text: Option<&str>) {
        self.docs.hover = None;
        let trigger = text.is_some_and(|t| t.ends_with(['(', ',', ')']));
        if trigger && self.cfg.autocomplete && self.doc.doc == Doc::Code {
            self.language_command("signature"); return;
        }
        if trigger && self.cfg.autocomplete || self.docs.hint.is_some() {
            self.hint_update(trigger);
        }
    }

    /// Finds the call at the cursor and asks for its signature when it is
    /// new and `ask` allows. Returns whether there is a call.
    fn hint_update(&mut self, ask: bool) -> bool {
        let call = self.call_here();
        match (call, self.docs.hint.as_mut()) {
            (None, _) => self.docs.hint = None,
            (Some(c), Some(h)) if h.open == c.open => { if h.seq != u64::MAX { (h.index, h.keyword) = (c.index, c.keyword); } },
            (Some(c), _) if ask => {
                let seq = self.ask_info(c.callee, None, Some(c.open));
                self.docs.hint =
                    seq.map(|seq| Hint { open: c.open, seq, info: None, index: c.index, keyword: c.keyword });
            }
            (Some(_), _) => self.docs.hint = None,
        }
        self.docs.hint.is_some()
    }

    /// The call around the cursor, with offsets in the document.
    fn call_here(&self) -> Option<py::Call> {
        let cur = self.doc.buf.cur;
        if (!self.doc.python_at(cur) && self.doc.doc != Doc::Code) || self.doc.buf.has_sel() {
            return None;
        }
        let (s, _) = self.scope(cur);
        let mut c = py::call_at(&self.doc.buf.text[s..cur], cur - s)?;
        (c.callee, c.open) = (c.callee + s, c.open + s);
        Some(c)
    }

    /// Asks for the documentation of the completion chosen, unless it is
    /// what was asked for last.
    fn chosen_update(&mut self) {
        let python = !matches!(self.doc.doc, Doc::Latex) && self.doc.python_at(self.doc.buf.cur);
        let Some(c) = self.comp.as_ref().filter(|c| python && !c.items.is_empty()) else {
            self.docs.chosen = None;
            return;
        };
        let name = c.items[c.sel].name.clone();
        if self.docs.chosen.as_ref().is_some_and(|c| c.0 == name) {
            return;
        }
        // The text as it would be with the name taken.
        let (start, cur) = (c.query.start, self.doc.buf.cur);
        let (s, e) = self.scope(cur);
        let text = &self.doc.buf.text;
        let code = format!("{}{name}{}", &text[s..start], &text[cur..e]);
        let end = start - s + name.len();
        let seq = self.ask_info(cur, Some((code, end)), None);
        self.docs.chosen = seq.map(|seq| (name, seq, None));
    }

    /// Waits for the answers asked for. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn docs_settle(&mut self) {
        self.docs_tick(true);
        if self.comp.is_some() {
            self.chosen_update();
        }
        loop {
            let d = &self.docs;
            let waiting = d.hover.as_ref().is_some_and(|h| h.info.is_none() && h.seq.is_some())
                || d.hint.as_ref().is_some_and(|h| h.info.is_none())
                || d.chosen.as_ref().is_some_and(|c| c.2.is_none());
            if !waiting {
                return;
            }
            let (python, dir) = self.python();
            let ev = match &mut self.nb {
                // A notebook's kernel is waited on only while no cell runs,
                // so that no cell's output is taken here.
                Some(nb) if nb.busy() => return,
                Some(nb) => nb.kernel(&python, &dir, &self.waker).and_then(Kernel::wait),
                None => self.helper.as_mut().and_then(Kernel::wait),
            };
            match ev {
                Some(crate::kernel::Event::Ready(v, _)) if self.nb.is_some() => {
                    self.nb.as_mut().unwrap().version = Some(v)
                }
                Some(crate::kernel::Event::Info { id, info }) => {
                    let pending = |d: &Docs| {
                        d.hover.as_ref().is_some_and(|h| h.seq == Some(id))
                            || d.hint.as_ref().is_some_and(|h| h.seq == id)
                            || d.chosen.as_ref().is_some_and(|c| c.1 == id)
                    };
                    let known = pending(&self.docs);
                    self.docs_answer(id, info);
                    // A request that found nothing is not waited for again.
                    if known {
                        if let Some(c) = self.docs.chosen.as_mut().filter(|c| c.1 == id && c.2.is_none()) {
                            c.1 = 0;
                        }
                        if let Some(h) = self.docs.hover.as_mut().filter(|h| h.seq == Some(id) && h.info.is_none()) {
                            h.seq = Some(0);
                            self.docs.hover = None;
                        }
                    }
                }
                Some(crate::kernel::Event::Matches { id, items }) => self.offer(id, items),
                Some(_) => {}
                None => return,
            }
        }
    }

    // ---- painting -------------------------------------------------------

    /// The boxes of documentation, over the document.
    pub(super) fn draw_docs(&mut self, c: &mut Canvas) {
        self.docs.hover_box = None;
        if self.overlaid() || self.reader.is_some() {
            return;
        }
        self.chosen_update();
        // A hint lasts while the cursor stays in its call.
        let here = self.docs.hint.as_ref().and_then(|_| self.call_here());
        match (here, self.docs.hint.as_mut()) {
            (Some(c), Some(h)) if c.open == h.open => { if h.seq != u64::MAX { (h.index, h.keyword) = (c.index, c.keyword); } },
            (_, Some(_)) => self.docs.hint = None,
            _ => {}
        }
        // One asked for by key lasts while the cursor stays on the name.
        let cur = self.doc.buf.cur;
        if self.docs.hover.as_ref().is_some_and(|h| h.since.is_none() && (cur < h.range.0 || cur > h.range.1)) {
            self.docs.hover = None;
        }
        let bottom = self.body_bottom() - self.term_height();
        let top = self.title_h() + self.head_h();
        let list = self.comp_box;

        let hint = self.docs.hint.as_ref().and_then(|h| Some((h.info.clone()?, h.index, h.keyword.clone())));
        if let Some((info, index, keyword)) = hint
            && let Some((x, y, lh)) = self.doc.caret()
        {
            let names: Vec<&str> = info.params.iter().map(|p| p.0.as_str()).collect();
            let active = py::active_param(&names, index, keyword.as_deref());
            let lines = self.info_lines(&info, active, true);
            // Above the line, unless the list of completions is there.
            let above = !list.is_some_and(|l| l.1 < y) && y - top > lh * 3.0;
            let max_h = (if above { y - top } else { bottom - y - lh }).min(220.0 * self.scale);
            let (box_y, side) = if above { (y, -1.0) } else { (y + lh, 1.0) };
            self.info_box(c, &lines, x, box_y, side, max_h, 0.0);
        }

        let len = self.doc.buf.text.len();
        let hover = self.docs.hover.as_ref().filter(|h| h.info.is_some() && h.range.1 <= len);
        let hover = hover.map(|h| (h.range, h.info.clone().unwrap(), h.scroll));
        if let Some((range, info, scroll)) = hover
            && let Some((x, y, lh)) = self.doc.spot(range.0)
        {
            let lines = self.info_lines(&info, None, false);
            let (room_below, room_above) = (bottom - y - lh, y - top);
            let below = room_below >= (240.0 * self.scale).min(room_above);
            let max_h = if below { room_below } else { room_above }.min(360.0 * self.scale).max(0.0);
            let (box_y, side) = if below { (y + lh, 1.0) } else { (y, -1.0) };
            let (r, full) = self.info_box(c, &lines, x, box_y, side, max_h, scroll);
            self.docs.hover_box = Some(r);
            if let Some(h) = self.docs.hover.as_mut() {
                h.room = full - r.3;
            }
        }

        let chosen = self.docs.chosen.as_ref().and_then(|c| c.2.clone());
        if let (Some(info), Some((lx, ly, lw, lh))) = (chosen, list) {
            let lines = self.info_lines(&info, None, false);
            let gap = (2.0 * self.scale).round();
            let (right, left) = (self.w - (lx + lw + gap), lx - gap);
            let w = (360.0 * self.scale).min(right.max(left)).round();
            if w < 220.0 * self.scale {
                return;
            }
            let x = if right >= w { lx + lw + gap } else { lx - gap - w };
            let max_h = (bottom - ly).min(lh.max(260.0 * self.scale));
            self.info_panel(c, &lines, (x, ly, w), max_h, 0.0);
        }
    }

    /// The lines a name's documentation is shown in: its signature in the
    /// code's face, with the parameter being typed picked out; what kind of
    /// thing it is and where from; and its docstring, or for a hint the
    /// start of it.
    fn info_lines(&mut self, info: &Info, active: Option<usize>, brief: bool) -> Vec<Line> {
        let fonts = &mut self.fonts.borrow_mut();
        let mono = fonts.mono();
        let bold = fonts.mono_bold();
        let ui_size = (theme::UI_TEXT * self.scale).round();
        let size = (self.doc.px() * theme::mono_scale()).round().max(ui_size);
        let mut lines = Vec::new();
        let sig = &info.signature;
        let range = active.and_then(|i| info.params.get(i)).and_then(|p| p.1);
        let mut pieces = Vec::new();
        match range {
            Some((a, b)) if b <= sig.len() && sig.is_char_boundary(a) && sig.is_char_boundary(b) => {
                pieces.push((sig[..a].to_string(), mono, theme::fg()));
                pieces.push((sig[a..b].to_string(), bold, theme::link()));
                pieces.push((sig[b..].to_string(), mono, theme::fg()));
            }
            _ => pieces.push((sig.clone(), mono, theme::fg())),
        }
        lines.push(Line { pieces, size, indent: 0.0, gap: 0.0 });
        // The kind, unless the signature says it already.
        let kind = Some(info.kind.as_str()).filter(|k| !sig.starts_with(&format!("{k} ")));
        let what = match (kind.unwrap_or(""), info.module.as_str()) {
            ("", "") => String::new(),
            (kind, "") => kind.to_string(),
            ("", module) => format!("from {module}"),
            (kind, module) => format!("{kind} from {module}"),
        };
        if !what.is_empty() && !brief {
            lines.push(Line {
                pieces: vec![(what, font::UI_ITALIC, theme::dim())],
                size: ui_size,
                indent: 0.0,
                gap: 0.0,
            });
        }
        let doc = info.doc.trim();
        let doc = if brief { doc.split("\n\n").next().unwrap_or("") } else { doc };
        for (i, l) in doc.split('\n').enumerate().filter(|_| !doc.is_empty()) {
            let text = l.trim_end().replace('\t', "    ");
            let lead = text.len() - text.trim_start().len();
            let indent = fonts.width(font::UI, ui_size, " ") * lead as f32;
            let gap = if i == 0 { ui_size * 0.6 } else { 0.0 };
            let color = if brief { theme::dim() } else { theme::fg() };
            lines.push(Line {
                pieces: vec![(text.trim_start().to_string(), font::UI, color)],
                size: ui_size,
                indent,
                gap,
            });
        }
        lines
    }

    /// A box of documentation against a point: below it when `side` is
    /// positive, above it otherwise. Returns where it was drawn and the
    /// height all of it would take.
    #[allow(clippy::too_many_arguments)]
    fn info_box(
        &mut self,
        c: &mut Canvas,
        lines: &[Line],
        x: f32,
        y: f32,
        side: f32,
        max_h: f32,
        scroll: f32,
    ) -> ((f32, f32, f32, f32), f32) {
        let pad = (theme::SPACE * self.scale).round();
        let widest = {
            let fonts = &mut self.fonts.borrow_mut();
            lines
                .iter()
                .map(|l| l.indent + l.pieces.iter().map(|p| fonts.width(p.1, l.size, &p.0)).sum::<f32>())
                .fold(0.0, f32::max)
        };
        // Rows are wrapped by the widths of single characters, which run a
        // little wider than the shaped line.
        let w = (widest * 1.05 + pad * 2.0).clamp(200.0 * self.scale, (560.0 * self.scale).min(self.w)).round();
        let x = (x - pad).clamp(0.0, (self.w - w).max(0.0)).round();
        let full = self.measure(lines, w - pad * 2.0) + pad * 2.0;
        let h = full.min(max_h).max(0.0).round();
        let top = if side > 0.0 { y } else { y - h }.round();
        self.info_panel(c, lines, (x, top, w), h, scroll);
        ((x, top, w, h), full)
    }

    /// The height lines take when wrapped to a width.
    fn measure(&mut self, lines: &[Line], width: f32) -> f32 {
        let fonts = &mut self.fonts.borrow_mut();
        lines.iter().map(|l| l.gap + wrap(fonts, l, width).len() as f32 * (l.size * 1.45).round()).sum()
    }

    /// Draws lines in a bordered panel no taller than `max_h`, scrolled by
    /// `scroll` pixels.
    fn info_panel(&mut self, c: &mut Canvas, lines: &[Line], (x, y, w): (f32, f32, f32), max_h: f32, scroll: f32) {
        let pad = (theme::SPACE * self.scale).round();
        let full = self.measure(lines, w - pad * 2.0) + pad * 2.0;
        let h = full.min(max_h).round();
        if h < pad * 3.0 {
            return;
        }
        let radius = (theme::RADIUS * self.scale).round();
        ui::surface(c, (x, y, w, h), radius, self.scale);
        c.clip(x, y, w, h);
        let fonts = &mut self.fonts.borrow_mut();
        let mut pen = y + pad - scroll;
        for (i, l) in lines.iter().enumerate() {
            pen += l.gap;
            if i == 1 && l.gap > 0.0 {
                c.line(x + pad, pen - l.gap / 2.0, w - pad * 2.0, 1.0, theme::line());
            }
            let row = (l.size * 1.45).round();
            for pieces in wrap(fonts, l, w - pad * 2.0) {
                if pen + row > y && pen < y + h {
                    let base = (pen + row * 0.5 + l.size * 0.34).round();
                    let mut px = x + pad + l.indent;
                    for (s, face, color) in &pieces {
                        px += ui::text(c, fonts, px, base, s, *face, l.size, *color);
                    }
                }
                pen += row;
            }
        }
        c.unclip();
        // A bar shows how much more there is.
        if full > h + 0.5 {
            let t = (4.0 * self.scale).round().max(2.0);
            let inset = (4.0 * self.scale).round();
            let len = ((h - inset * 2.0) * h / full).max(t * 4.0);
            let at = y + inset + (h - inset * 2.0 - len) * (scroll / (full - h)).clamp(0.0, 1.0);
            c.round_rect(x + w - t - inset, at, t, len, t / 2.0, theme::faint());
        }
    }
}

/// Splits a line into rows that fit a width, breaking after spaces and
/// commas where it can. Each row is pieces of text as the line's are.
fn wrap(fonts: &mut Fonts, line: &Line, width: f32) -> Vec<Vec<(String, u8, u32)>> {
    let width = (width - line.indent).max(1.0);
    let mut rows = vec![Vec::new()];
    let mut used = 0.0;
    for (text, face, color) in &line.pieces {
        let mut rest = text.as_str();
        while !rest.is_empty() {
            // As much of the rest as fits, broken at a space or comma.
            let mut fit = 0;
            let mut brk = None;
            let mut w = used;
            for (i, ch) in rest.char_indices() {
                let cw = fonts.width(*face, line.size, &rest[i..i + ch.len_utf8()]);
                if w + cw > width && fit > 0 || w + cw > width && used > 0.0 {
                    break;
                }
                w += cw;
                fit = i + ch.len_utf8();
                if matches!(ch, ' ' | ',') {
                    brk = Some(fit);
                }
            }
            if fit == 0 {
                // Nothing more fits on this row.
                rows.push(Vec::new());
                used = 0.0;
                continue;
            }
            let end = if fit < rest.len() { brk.unwrap_or(fit) } else { fit };
            let part = &rest[..end];
            used += fonts.width(*face, line.size, part);
            rows.last_mut().unwrap().push((part.to_string(), *face, *color));
            rest = &rest[end..];
            if !rest.is_empty() {
                rows.push(Vec::new());
                used = 0.0;
                rest = rest.trim_start_matches(' ');
            }
        }
    }
    rows
}

impl App {
    pub(super) fn language_signature(&mut self, result:&Value) {
        let n = result["activeSignature"].as_u64().unwrap_or(0) as usize;
        let sig = &result["signatures"][n];
        let Some(signature) = sig["label"].as_str() else { self.docs.hint = None; return; };
        let index = sig["activeParameter"].as_u64().or(result["activeParameter"].as_u64()).unwrap_or(0) as usize;
        let params = sig["parameters"].as_array().into_iter().flatten().filter_map(|p| {
            let range = if let Some(label) = p["label"].as_str() {
                signature.find(label).map(|a| (a,a+label.len()))
            } else {
                let point = |n:u64| { let mut units=0; for (i,c) in signature.char_indices() { if units==n { return Some(i); } units+=c.len_utf16() as u64; } (units==n).then_some(signature.len()) };
                point(p["label"][0].as_u64()?).zip(point(p["label"][1].as_u64()?))
            }?;
            Some((signature.get(range.0..range.1)?.to_string(),Some(range)))
        }).collect();
        let doc = sig["documentation"].as_str().or(sig["documentation"]["value"].as_str()).unwrap_or("").to_string();
        let info = Info { signature:signature.into(),params,doc,..Default::default() };
        if let Some(call) = self.call_here() {
            self.docs.hint = Some(Hint { open:call.open,seq:u64::MAX,info:Some(info),index,keyword:None });
        } else { self.language_hover_at(format!("{}\n\n{}",info.signature,info.doc),None); }
    }
    /// Shows a paused value for the name the pointer still rests on.
    pub(super) fn docs_value(&mut self, range: (usize, usize), text: String) {
        if let Some(h) = self.docs.hover.as_mut().filter(|h| h.range == range) {
            h.info = Some(Info { signature: text, ..Default::default() });
            h.seq = Some(u64::MAX);
        }
    }

    /// Rests the pointer on a range at once. For tests.
    #[cfg(test)]
    pub(super) fn docs_rest_on(&mut self, range: (usize, usize)) {
        self.docs.hover = Some(Hover { range, since: Some(Instant::now()), seq: None, info: None, scroll: 0.0, room: 0.0 });
        self.docs_tick(true);
    }

    #[cfg(test)]
    pub(super) fn docs_text(&self) -> Option<String> {
        self.docs.hover.as_ref()?.info.as_ref().map(|i| if i.signature.is_empty() { i.doc.clone() } else { i.signature.clone() })
    }

    pub(super) fn language_hover_at(&mut self, text: String, at: Option<usize>) {
        if let Some(at) = at
            && !self.docs.hover.as_ref().is_some_and(|h| at >= h.range.0 && at <= h.range.1)
        {
            return;
        }
        if text.is_empty() {
            self.message = Some("No documentation found".into());
            return;
        }
        let range = py::name_at(&self.doc.buf.text, at.unwrap_or(self.doc.buf.cur))
            .unwrap_or((self.doc.buf.cur, self.doc.buf.cur));
        self.docs.hover = Some(Hover {
            range,
            since: at.map(|_| Instant::now()),
            seq: Some(u64::MAX),
            info: Some(Info { doc: text, ..Default::default() }),
            scroll: 0.0,
            room: 0.0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Key, Mods};

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    fn typed(a: &mut App, s: &str) {
        for ch in s.chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
    }

    fn draw(a: &mut App) {
        let mut px = vec![0u32; 800 * 600];
        a.draw(&mut Canvas::new(&mut px, 800, 600));
    }

    #[test]
    fn calls_show_their_parameters() {
        if crate::kernel::Kernel::spawn("python3", std::path::Path::new("."), std::sync::Arc::new(|| {})).is_err() {
            return;
        }
        let mut a = App::new(Some(std::env::temp_dir().join("lightdown-docs-test/a.py")));
        a.resize(800, 600, 1.0);
        a.paste_text("def add(x, y=1, *rest, key=None):\n    \"\"\"Adds them.\"\"\"\n\n");
        typed(&mut a, "add(");
        a.settle();
        let active = |a: &App| {
            let h = a.docs.hint.as_ref()?;
            let info = h.info.as_ref()?;
            let names: Vec<&str> = info.params.iter().map(|p| p.0.as_str()).collect();
            py::active_param(&names, h.index, h.keyword.as_deref()).map(|i| names[i].to_string())
        };
        let sig = a.docs.hint.as_ref().and_then(|h| h.info.as_ref()).map(|i| i.signature.clone());
        assert_eq!(sig.as_deref(), Some("add(x, y=1, *rest, key=None)"));
        assert_eq!(active(&a).as_deref(), Some("x"));
        typed(&mut a, "1, f(2), ");
        // The inner call had a hint of its own; the outer one is asked again.
        a.settle();
        assert_eq!(active(&a).as_deref(), Some("*rest"));
        // The argument being typed is followed without asking again.
        typed(&mut a, "key=");
        assert_eq!(active(&a).as_deref(), Some("key=None"));
        draw(&mut a);
        assert!(a.docs.hint.is_some());
        // Leaving the call takes the hint away; so does Esc.
        typed(&mut a, ")");
        assert!(a.docs.hint.is_none());
        typed(&mut a, "\nadd(");
        a.settle();
        assert!(a.docs.hint.as_ref().is_some_and(|h| h.info.is_some()));
        a.key(Key::Escape, NONE, None);
        assert!(a.docs.hint.is_none());
        a.key(Key::Left, NONE, None);
        draw(&mut a);
        assert!(a.docs.hint.is_none());
    }

    #[test]
    fn names_are_documented_on_request() {
        if crate::kernel::Kernel::spawn("python3", std::path::Path::new("."), std::sync::Arc::new(|| {})).is_err() {
            return;
        }
        let mut a = App::new(Some(std::env::temp_dir().join("lightdown-docs-test/b.py")));
        a.resize(800, 600, 1.0);
        a.paste_text("n = len");
        a.key(Key::Left, NONE, None);
        a.key(Key::Char('k'), CTRL, None);
        a.settle();
        let doc = a.docs.hover.as_ref().and_then(|h| h.info.as_ref()).map(|i| i.doc.clone());
        assert!(doc.is_some_and(|d| d.contains("number of items")));
        draw(&mut a);
        assert!(a.docs.hover_box.is_some());
        // It stays while the cursor is on the name, and goes when it leaves.
        a.key(Key::Left, NONE, None);
        draw(&mut a);
        assert!(a.docs.hover.is_some());
        a.key(Key::Home, NONE, None);
        draw(&mut a);
        assert!(a.docs.hover.is_none());
        // Use an unknown name; Jedi may infer n as an alias for len.
        a.doc.select(0, a.doc.buf.text.len());
        a.paste_text("unknown_name");
        // Where nothing is known, the status bar says so.
        a.key(Key::Char('k'), CTRL, None);
        a.settle();
        assert!(
            a.docs.hover.is_none()
                && a.message.as_deref().is_some_and(|m| m.contains("Nothing is known of unknown_name"))
        );
    }
}
