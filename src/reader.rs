//! A PDF opened as the document: its pages one under another down the
//! middle of the window, to read, select from and search. Nothing in it is
//! edited.
//!
//! A picture is shown here too, as a page of its own: whole in the window,
//! and at most its actual size until zoomed. Dragging it moves it about.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use crate::app::{Key, Mods};
use crate::gfx::Canvas;
use crate::kernel::Waker;
use crate::pdf::{self, Bookmark, Link, Marks, Pages, Target, Text};
use crate::pics::Pics;
use crate::theme;

/// A place in the text: the page, and where on it.
pub type Pos = (usize, usize);

/// Where a search goes from the selection.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Seek {
    /// To the first match that starts where the selection does or later,
    /// as the words looked for are typed.
    Here,
    Next,
    Back,
}

pub struct Reader {
    pub pages: Pages,
    /// The text of each page, once it has been read.
    texts: Vec<Option<Text>>,
    reading: Option<Receiver<(usize, Text)>>,
    /// Each page's links, and the bookmarks, once read.
    links: Vec<Vec<Link>>,
    bookmarks: Vec<Bookmark>,
    marking: Option<Receiver<Marks>>,
    /// The link a press began on, followed if it ends without a drag.
    pressed: Option<Target>,
    /// The bookmark last gone to, and the scroll it left, so the outline
    /// keeps it while the view stays there.
    picked: Option<(usize, f32)>,
    wake: Waker,
    /// Where the selection started and where it reaches.
    sel: Option<(Pos, Pos)>,
    dragging: bool,
    /// Where the pointer was as it drags a picture about.
    grab: Option<(f32, f32)>,
    scroll: f32,
    across: f32,
    zoom: f32,
    pics: Rc<RefCell<Pics>>,
    rect: (f32, f32, f32, f32),
    scale: f32,
}

impl Reader {
    pub fn new(pages: Pages, pics: Rc<RefCell<Pics>>, wake: Waker) -> Reader {
        let mut reader = Reader {
            pages,
            texts: Vec::new(),
            reading: None,
            links: Vec::new(),
            bookmarks: Vec::new(),
            marking: None,
            pressed: None,
            picked: None,
            wake,
            sel: None,
            dragging: false,
            grab: None,
            scroll: 0.0,
            across: 0.0,
            zoom: 1.0,
            pics,
            rect: (0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
        };
        reader.read();
        reader
    }

    /// Starts reading the text of the pages. A picture has none.
    fn read(&mut self) {
        self.texts = vec![None; self.pages.sizes.len()];
        self.reading = match self.pages.picture {
            true => None,
            false => Some(pdf::read_text(self.pages.path.clone(), self.wake.clone())),
        };
        (self.links, self.bookmarks) = (Vec::new(), Vec::new());
        self.marking = (!self.pages.picture).then(|| pdf::read_marks(self.pages.path.clone(), self.wake.clone()));
    }

    /// Takes in the links and bookmarks once they are read.
    fn take_marks(&mut self) {
        if let Some(marks) = self.marking.as_ref().and_then(|m| m.try_recv().ok()) {
            (self.links, self.bookmarks) = (marks.links, marks.bookmarks);
            self.marking = None;
        }
    }

    /// Whether this is a picture rather than a PDF.
    pub fn picture(&self) -> bool {
        self.pages.picture
    }

    /// How large a picture is shown, as a share of its actual size.
    pub fn percent(&self) -> u32 {
        (self.fit() * 100.0).round() as u32
    }

    /// Takes in the text of the pages read since.
    fn take_text(&mut self) {
        let Some(reading) = &self.reading else { return };
        for (n, text) in reading.try_iter() {
            if let Some(slot) = self.texts.get_mut(n) {
                *slot = Some(text);
            }
        }
    }

    /// Waits for the text of every page. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn settle(&mut self) {
        if let Some(marks) = self.marking.take().and_then(|m| m.recv().ok()) {
            (self.links, self.bookmarks) = (marks.links, marks.bookmarks);
        }
        if let Some(reading) = self.reading.take() {
            for (n, text) in reading.iter() {
                if let Some(slot) = self.texts.get_mut(n) {
                    *slot = Some(text);
                }
            }
        }
    }

    /// The text of every page read so far, a line between pages.
    pub fn text(&mut self) -> String {
        self.take_text();
        let pages = self.texts.iter().map(|t| t.as_ref().map_or("", |t| t.text.as_str()));
        pages.collect::<Vec<_>>().join("\n\n")
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) {
        if ((x, y, w, h), scale) != (self.rect, self.scale) {
            self.refit(|r| (r.rect, r.scale) = ((x, y, w, h), scale));
        }
    }

    /// Makes a change to the size of the pages or the window, keeping the
    /// same place in view.
    fn refit(&mut self, change: impl FnOnce(&mut Reader)) {
        let (k, w) = (self.fit(), self.rect.2);
        let middle = (self.across + w / 2.0) / self.wide(k).max(w);
        // What of a picture is in the middle of the view, from its top.
        let down = (self.rect.1 + self.rect.3 / 2.0 - self.top(k)) / k;
        change(self);
        let (fit, w) = (self.fit(), self.rect.2);
        self.scroll *= fit / k;
        if self.pages.picture {
            self.scroll = 0.0;
            self.scroll = self.top(fit) + down * fit - (self.rect.1 + self.rect.3 / 2.0);
        }
        self.across = middle * self.wide(fit).max(w) - w / 2.0;
        self.scroll(0.0);
        self.scroll_across(0.0);
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        rw > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    fn pad(&self) -> f32 {
        (16.0 * self.scale).round()
    }

    fn gap(&self) -> f32 {
        (12.0 * self.scale).round()
    }

    /// Pixels to a point: a page at its printed size on a screen of 96
    /// dots to the inch, or narrower if the window is, and then zoomed.
    fn fit(&self) -> f32 {
        self.base() * self.zoom
    }

    /// Pixels to a point before zooming. A picture is shown a pixel to a
    /// pixel, or smaller to fit both ways.
    fn base(&self) -> f32 {
        let room = (self.rect.2 - self.pad() * 2.0).max(1.0);
        if self.pages.picture {
            let (w, h) = self.pages.sizes[0];
            let tall = (self.rect.3 - self.pad() * 2.0).max(1.0);
            return 1f32.min(room / w).min(tall / h);
        }
        (96.0 / 72.0 * self.scale).min(room / self.pages.widest())
    }

    /// The most a zoom may enlarge: four times, or for a picture eight
    /// pixels to each of its own, to look closely; an SVG as far as four
    /// times the window, too.
    fn most(&self) -> f32 {
        if !self.pages.picture {
            return 4.0;
        }
        let (w, h) = self.pages.sizes[0];
        let room = (self.rect.2 - self.pad() * 2.0).max(1.0);
        let tall = (self.rect.3 - self.pad() * 2.0).max(1.0);
        let fill = if self.pages.vector { (room / w).min(tall / h) * 4.0 } else { 0.0 };
        (8f32.max(fill) / self.base()).max(4.0)
    }

    /// How tall the pages are, one under another, `k` pixels to a point.
    fn tall(&self, k: f32) -> f32 {
        let pages: f32 = (0..self.pages.sizes.len()).map(|n| self.pages.shown(n, k).1 + self.gap()).sum();
        pages - self.gap() + self.pad() * 2.0
    }

    /// How wide the widest page is with room beside it.
    fn wide(&self, k: f32) -> f32 {
        (self.pages.widest() * k).round() + self.pad() * 2.0
    }

    /// Where the middle of the pages is across the window.
    fn mid(&self, k: f32) -> f32 {
        self.rect.0 + self.wide(k).max(self.rect.2) / 2.0 - self.across
    }

    /// Where the first page's top is. A picture shorter than the window
    /// stands in the middle of it.
    fn top(&self, k: f32) -> f32 {
        let spare = if self.pages.picture { (self.rect.3 - self.tall(k)).max(0.0) / 2.0 } else { 0.0 };
        self.rect.1 + self.pad() + spare.round() - self.scroll
    }

    /// Where each page is drawn: its left, top, width and height.
    fn places(&self, k: f32) -> impl Iterator<Item = (f32, f32, f32, f32)> + '_ {
        let mid = self.mid(k);
        let mut pen = self.top(k);
        (0..self.pages.sizes.len()).map(move |n| {
            let (w, h) = self.pages.shown(n, k);
            let place = ((mid - w / 2.0).round(), pen, w, h);
            pen += h + self.gap();
            place
        })
    }

    pub fn scroll(&mut self, dy: f32) {
        let end = self.tall(self.fit()) - self.rect.3;
        self.scroll = (self.scroll + dy).min(end).max(0.0);
    }

    pub fn scroll_lines(&mut self, lines: f32) {
        self.scroll(lines * (40.0 * self.scale).round());
    }

    /// Moves the pages sideways, where they are wider than the window.
    pub fn scroll_across(&mut self, dx: f32) {
        let end = self.wide(self.fit()) - self.rect.2;
        self.across = (self.across + dx).min(end).max(0.0);
    }

    pub fn scroll_across_lines(&mut self, lines: f32) {
        self.scroll_across(lines * (40.0 * self.scale).round());
    }

    pub fn zoom_by(&mut self, steps: i32) {
        self.refit(|r| r.zoom = (r.zoom * 1.1f32.powi(steps)).clamp(0.25, r.most()));
    }

    pub fn zoom_reset(&mut self) {
        self.refit(|r| r.zoom = 1.0);
    }

    /// Moves through the pages by a key. Returns whether it was one that
    /// does.
    pub fn key(&mut self, key: Key, mods: Mods) -> bool {
        let screen = (self.rect.3 - self.gap() * 2.0).max(self.rect.3 / 2.0);
        match key {
            Key::Up => self.scroll_lines(-1.0),
            Key::Down => self.scroll_lines(1.0),
            Key::Left => self.scroll_across_lines(-1.0),
            Key::Right => self.scroll_across_lines(1.0),
            Key::PageUp => self.scroll(-screen),
            Key::PageDown => self.scroll(screen),
            Key::Char(' ') if !mods.ctrl && !mods.alt => self.scroll(if mods.shift { -screen } else { screen }),
            Key::Home => self.scroll = 0.0,
            Key::End => self.scroll = f32::MAX,
            Key::Escape => self.sel = None,
            _ => return false,
        }
        self.scroll(0.0);
        true
    }

    /// The page in the middle of the view, counting from 1.
    pub fn page(&self) -> usize {
        let middle = self.rect.1 + self.rect.3 / 2.0;
        let gap = self.gap();
        let below = self.places(self.fit()).position(|(_, top, _, h)| top + h + gap > middle);
        below.unwrap_or(self.pages.sizes.len() - 1) + 1
    }

    /// The page a point is on or nearest, and the point on it in points
    /// from its top left.
    fn locate(&self, x: f32, y: f32) -> (usize, f32, f32) {
        let k = self.fit();
        let away = |&(_, top, _, h): &(f32, f32, f32, f32)| (top - y).max(y - top - h).max(0.0);
        let places: Vec<_> = self.places(k).collect();
        let n = (0..places.len()).min_by(|&a, &b| away(&places[a]).total_cmp(&away(&places[b]))).unwrap_or(0);
        let (left, top, ..) = places[n];
        (n, (x - left) / k, (y - top) / k)
    }

    /// Whether a point is on a page.
    pub fn on_page(&self, x: f32, y: f32) -> bool {
        self.places(self.fit()).any(|(l, t, w, h)| x >= l && x < l + w && y >= t && y < t + h)
    }

    /// Where the link under a point leads.
    pub fn link_at(&self, x: f32, y: f32) -> Option<&Target> {
        if self.pages.picture || !self.on_page(x, y) {
            return None;
        }
        let (n, px, py) = self.locate(x, y);
        let inside = |l: &&Link| px >= l.rect[0] && px <= l.rect[2] && py >= l.rect[1] && py <= l.rect[3];
        self.links.get(n)?.iter().find(inside).map(|l| &l.target)
    }

    /// Shows a page, from a place on it in points from its top, or from
    /// its top.
    pub fn go(&mut self, page: usize, y: Option<f32>) {
        let k = self.fit();
        let Some((_, top, _, _)) = self.places(k).nth(page) else { return };
        self.scroll += top + y.unwrap_or(0.0) * k - self.rect.1 - self.pad();
        self.scroll(0.0);
    }

    /// Where a point on a page, in points from its top left, is drawn.
    #[cfg(test)]
    pub fn screen_point(&self, page: usize, x: f32, y: f32) -> (f32, f32) {
        let k = self.fit();
        let (left, top, ..) = self.places(k).nth(page).unwrap();
        (left + x * k, top + y * k)
    }

    /// The bookmarks, once read.
    pub fn bookmarks(&mut self) -> &[Bookmark] {
        self.take_marks();
        &self.bookmarks
    }

    /// Goes where a bookmark leads, if it leads within the document.
    pub fn go_bookmark(&mut self, i: usize) {
        if let Some(Some(Target::Page { page, y })) = self.bookmarks.get(i).map(|b| b.target.clone()) {
            self.go(page, y);
            self.picked = Some((i, self.scroll));
        }
    }

    /// The bookmark last gone to while the view stays there, or else the
    /// last one at or above the top of the view.
    pub fn current_bookmark(&self) -> Option<usize> {
        if let Some((i, scroll)) = self.picked
            && (scroll - self.scroll).abs() < 0.5
        {
            return Some(i);
        }
        let (n, _, y) = self.locate(self.rect.0 + self.rect.2 / 2.0, self.rect.1 + self.pad() + 1.0);
        self.bookmarks.iter().rposition(|b| match &b.target {
            Some(Target::Page { page, y: at }) => (*page, at.unwrap_or(0.0)) <= (n, y.max(0.0)),
            _ => false,
        })
    }

    /// The place in the text under a point, if that page's text is read.
    fn pos_at(&self, x: f32, y: f32) -> Option<(Pos, &Text)> {
        let (n, px, py) = self.locate(x, y);
        let text = self.texts.get(n)?.as_ref()?;
        Some(((n, text.at(px, py)), text))
    }

    /// A press on the pages: starts a selection there, or with Shift takes
    /// the one there is that far; a second press selects a word, a third a
    /// line. On a picture it takes hold of it, to drag it about.
    pub fn press(&mut self, x: f32, y: f32, mods: Mods, clicks: u32) {
        if self.pages.picture {
            self.grab = Some((x, y));
            return;
        }
        self.take_text();
        self.take_marks();
        self.pressed = if clicks == 1 && !mods.shift && !mods.ctrl { self.link_at(x, y).cloned() } else { None };
        let picked = self.pos_at(x, y).map(|((n, at), text)| match (clicks, self.sel) {
            (1, Some((anchor, _))) if mods.shift => (anchor, (n, at)),
            (2, _) => {
                let (a, b) = text.word(at);
                ((n, a), (n, b))
            }
            (c, _) if c >= 3 => {
                let (a, b) = text.line(at);
                ((n, a), (n, b))
            }
            _ => ((n, at), (n, at)),
        });
        self.sel = picked;
        self.dragging = true;
    }

    /// The pointer moved: a selection being dragged follows it.
    pub fn motion(&mut self, x: f32, y: f32) -> bool {
        if let Some((gx, gy)) = self.grab {
            self.scroll(gy - y);
            self.scroll_across(gx - x);
            self.grab = Some((x, y));
            return true;
        }
        if !self.dragging {
            return false;
        }
        let Some((anchor, _)) = self.sel else { return false };
        let Some((head, _)) = self.pos_at(x, y) else { return false };
        self.sel = Some((anchor, head));
        true
    }

    /// The press ends. One that began on a link and selected nothing
    /// follows it: a page is shown here, and an address is returned for
    /// the desktop to open.
    pub fn release(&mut self) -> Option<String> {
        self.dragging = false;
        self.grab = None;
        let target = self.pressed.take()?;
        if self.ordered().is_some() {
            return None;
        }
        match target {
            Target::Uri(address) => Some(address),
            Target::Page { page, y } => {
                self.go(page, y);
                None
            }
        }
    }

    /// Whether a selection is being dragged out.
    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// The selection, start first.
    fn ordered(&self) -> Option<(Pos, Pos)> {
        self.sel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) }).filter(|(a, b)| a != b)
    }

    /// The text selected, if any, a line between pages.
    pub fn selected(&self) -> Option<String> {
        let (a, b) = self.ordered()?;
        let mut out = Vec::new();
        for n in a.0..=b.0 {
            let Some(text) = self.texts.get(n).and_then(Option::as_ref) else { continue };
            let from = if n == a.0 { a.1 } else { 0 };
            let to = if n == b.0 { b.1 } else { text.text.len() };
            out.push(&text.text[from.min(to)..to]);
        }
        Some(out.join("\n"))
    }

    pub fn select_all(&mut self) {
        self.take_text();
        let last = self.texts.len().saturating_sub(1);
        let end = self.texts.last().and_then(Option::as_ref).map_or(0, |t| t.text.len());
        self.sel = Some(((0, 0), (last, end)));
    }

    /// Selects the next match of `needle` from the selection, or from the
    /// page in view, and brings it into view. Case is ignored unless the
    /// words looked for have capitals. Returns whether one was found.
    pub fn find(&mut self, needle: &str, seek: Seek) -> bool {
        self.take_text();
        if needle.is_empty() {
            return false;
        }
        let fold = !needle.chars().any(char::is_uppercase);
        let needle = if fold { needle.to_ascii_lowercase() } else { needle.to_string() };
        let mut found: Vec<(Pos, Pos)> = Vec::new();
        for (n, text) in self.texts.iter().enumerate() {
            let Some(text) = text else { continue };
            let hay = if fold { text.text.to_ascii_lowercase() } else { text.text.clone() };
            found.extend(hay.match_indices(&needle).map(|(i, m)| ((n, i), (n, i + m.len()))));
        }
        let from = self.ordered().map_or((self.page() - 1, 0), |(a, _)| a);
        let hit = match seek {
            Seek::Here => found.iter().find(|m| m.0 >= from).or(found.first()),
            Seek::Next => found.iter().find(|m| m.0 > from).or(found.first()),
            Seek::Back => found.iter().rev().find(|m| m.0 < from).or(found.last()),
        };
        let Some(&(a, b)) = hit else { return false };
        self.sel = Some((a, b));
        self.reveal(a, b);
        true
    }

    /// Scrolls so that the text from `a` to `b`, on one page, is in view.
    fn reveal(&mut self, a: Pos, b: Pos) {
        let Some(text) = self.texts.get(a.0).and_then(Option::as_ref) else { return };
        let Some(r) = text.boxes(a.1, b.1).first().copied() else { return };
        let k = self.fit();
        let Some((left, top, ..)) = self.places(k).nth(a.0) else { return };
        let (x, y, w, h) = self.rect;
        let (bx0, by0, bx1, by1) = (left + r[0] * k, top + r[1] * k, left + r[2] * k, top + r[3] * k);
        let margin = (h / 4.0).min(80.0 * self.scale);
        if by0 < y + margin || by1 > y + h - margin {
            self.scroll(by0 - (y + h / 3.0));
        }
        if bx0 < x || bx1 > x + w {
            self.scroll_across(bx0 - (x + w / 3.0));
        }
    }

    /// Reads the file again if it has been written since, keeping the
    /// place. Returns whether it was.
    pub fn refresh(&mut self) -> bool {
        if !self.pages.changed() {
            return false;
        }
        match self.pages.reopen() {
            Ok(pages) => {
                self.pages = pages;
                self.sel = None;
                self.read();
                true
            }
            Err(_) => false,
        }
    }

    pub fn draw(&mut self, c: &mut Canvas) {
        let (x, y, w, h) = self.rect;
        if w < 1.0 || h < 1.0 {
            return;
        }
        self.take_text();
        self.take_marks();
        let k = self.fit();
        self.scroll = self.scroll.min(self.tall(k) - h).max(0.0);
        self.across = self.across.min(self.wide(k) - w).max(0.0);
        c.clip(x, y, w, h);
        // Pages lie on the raised ground, so a white page keeps its edge on
        // a white sheet; a picture lies on the sheet itself.
        c.rect(x, y, w, h, if self.picture() { theme::bg() } else { theme::inset() });
        let gap = self.gap();
        let pics = &mut self.pics.borrow_mut();
        self.pages.draw(c, pics, self.mid(k), self.top(k), k, gap, y, y + h);
        if let Some((a, b)) = self.ordered() {
            for (n, (left, top, _, ph)) in self.places(k).enumerate() {
                if n < a.0 || n > b.0 || top > y + h || top + ph < y {
                    continue;
                }
                let Some(text) = &self.texts[n] else { continue };
                let from = if n == a.0 { a.1 } else { 0 };
                let to = if n == b.0 { b.1 } else { text.text.len() };
                for r in text.boxes(from, to) {
                    let (bx, by) = (left + r[0] * k, top + r[1] * k);
                    c.shade(bx, by, (r[2] - r[0]) * k, (r[3] - r[1]) * k, theme::primary(), 110);
                }
            }
        }
        c.unclip();
    }
}
