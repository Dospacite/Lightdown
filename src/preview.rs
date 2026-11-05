//! The preview: a document's PDF, page under page, in the sidebar, with
//! what went wrong in building it above the pages.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::pdf::Pages;
use crate::pics::Pics;
use crate::tex::Problem;
use crate::ui::px;
use crate::{theme, ui};

/// A control of the preview.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Build,
    Open,
    Setup,
    /// An error, by its place in the list, to go to.
    Problem(usize),
}

pub struct Preview {
    /// The pages of the last build that could be read.
    pub pages: Option<Pages>,
    pub pdf: Option<PathBuf>,
    pub problems: Vec<Problem>,
    /// How the last build went, or what is under way.
    pub state: String,
    /// No TeX engine is installed: setting one up is offered.
    pub no_engine: bool,
    /// Why there are no pages, where that is not plain from the rest.
    pub note: Option<String>,
    scroll: f32,
    /// How tall everything below the header was when last drawn.
    tall: f32,
    fonts: Rc<RefCell<Fonts>>,
    pics: Rc<RefCell<Pics>>,
    rect: (f32, f32, f32, f32),
    scale: f32,
    hits: Vec<((f32, f32, f32, f32), Hit)>,
    /// Where the pointer is, for lighting the control under it.
    pub hover: Option<(f32, f32)>,
    /// Where the pages were last drawn: the middle of the column, the top
    /// of the first page as if not scrolled, pixels to a point, and the
    /// gap between pages.
    placed: Option<(f32, f32, f32, f32)>,
    /// A place on a page that was gone to, lit until the view moves.
    spot: Option<(usize, crate::synctex::Rect)>,
    /// A place to go to once the pages have been laid out.
    pending: Option<(usize, crate::synctex::Rect)>,
    /// The preview is to be drawn again: what it went to moved it.
    pub again: bool,
}

/// Breaks text into lines that fit a width, at spaces.
fn wrap(fonts: &mut Fonts, face: u8, size: f32, text: &str, max: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let longer = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && fonts.width(face, size, &longer) > max {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = longer;
        }
    }
    lines.push(line);
    lines
}

impl Preview {
    pub fn new(fonts: Rc<RefCell<Fonts>>, pics: Rc<RefCell<Pics>>) -> Preview {
        Preview {
            pages: None,
            pdf: None,
            problems: Vec::new(),
            state: String::new(),
            no_engine: false,
            note: None,
            scroll: 0.0,
            tall: 0.0,
            fonts,
            pics,
            rect: (0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
            hits: Vec::new(),
            hover: None,
            placed: None,
            spot: None,
            pending: None,
            again: false,
        }
    }

    /// Forgets what was shown, for another document.
    pub fn clear(&mut self) {
        (self.pages, self.pdf, self.problems, self.note) = (None, None, Vec::new(), None);
        (self.state, self.scroll) = (String::new(), 0.0);
        (self.placed, self.spot) = (None, None);
    }

    /// Each page's left and top on the screen, and its size there.
    fn page_rects(&self) -> Vec<(f32, f32, f32, f32)> {
        let (Some(pages), Some((mid, top, k, gap))) = (&self.pages, self.placed) else { return Vec::new() };
        let mut pen = top - self.scroll;
        (0..pages.sizes.len())
            .map(|n| {
                let (w, h) = pages.shown(n, k);
                let r = ((mid - w / 2.0).round(), pen, w, h);
                pen += h + gap;
                r
            })
            .collect()
    }

    /// The page under a point, and the point on it in PDF points.
    pub fn page_at(&self, x: f32, y: f32) -> Option<(usize, f32, f32)> {
        let k = self.placed?.2;
        let rects = self.page_rects();
        let n = rects.iter().position(|r| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3)?;
        Some((n, (x - rects[n].0) / k, (y - rects[n].1) / k))
    }

    /// Scrolls a place on a page into view, a third of the way down, and
    /// lights it.
    pub fn show_spot(&mut self, page: usize, rect: crate::synctex::Rect) {
        let Some((_, _, k, _)) = self.placed else {
            self.pending = Some((page, rect));
            return;
        };
        let Some(r) = self.page_rects().get(page).copied() else { return };
        let room = self.rect.3 - self.head();
        let at = r.1 + self.scroll + rect[1] * k - self.rect.1 - self.head();
        self.scroll = (at - room / 3.0).max(0.0);
        self.spot = Some((page, rect));
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) {
        (self.rect, self.scale) = ((x, y, w, h), scale);
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        rw > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    /// The control under a point, if any.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let inside = |r: &(f32, f32, f32, f32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
        self.hits.iter().find(|(r, _)| inside(r)).map(|(_, h)| *h)
    }

    /// The line saying how the last build went.
    fn head(&self) -> f32 {
        (34.0 * self.scale).round()
    }

    pub fn scroll(&mut self, dy: f32) {
        self.spot = None;
        let room = self.rect.3 - self.head();
        self.scroll = (self.scroll + dy).min(self.tall - room).max(0.0);
    }

    pub fn scroll_lines(&mut self, lines: f32) {
        self.scroll(lines * (40.0 * self.scale).round());
    }

    /// Draws the preview. `nothing` says why there is nothing to build,
    /// for a document that cannot be.
    pub fn draw(&mut self, c: &mut Canvas, busy: bool, nothing: Option<&str>) {
        let (x, y, w, h) = self.rect;
        if w < 1.0 || h < 1.0 {
            return;
        }
        let s = self.scale;
        let size = px(theme::UI_TEXT, s);
        let small = px(theme::SMALL, s);
        let pad = px(theme::PAD, s);
        let line_h = (size * 1.55).round();
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        self.hits.clear();
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());

        // How the last build went, as a badge, the words after it.
        let head = self.head();
        let (badge, ink) = if busy {
            ("Building", theme::warning())
        } else if !self.problems.is_empty() {
            ("Failed", theme::danger())
        } else if self.pdf.is_some() {
            ("Built", theme::success())
        } else {
            ("Not built", theme::dim())
        };
        let mid = y + head / 2.0;
        let bw = if nothing.is_none() { ui::badge(c, fonts, x + pad, mid, badge, ink, s) } else { 0.0 };
        let state = ui::fit(fonts, font::UI, small, &self.state, w - pad * 2.0 - bw - px(8.0, s));
        if nothing.is_none() {
            ui::text(
                c,
                fonts,
                x + pad + bw + px(8.0, s),
                ui::base(y, head, small),
                &state,
                font::UI,
                small,
                theme::dim(),
            );
        }
        c.line(x, y + head - 1.0, w, 1.0, theme::line());

        let top = y + head;
        c.clip(x, top, w, (y + h - top).max(0.0));
        let mut pen = top + pad - self.scroll;
        let text_w = w - pad * 2.0;
        let say = |c: &mut Canvas, fonts: &mut Fonts, pen: &mut f32, text: &str, face: u8, ink: u32, indent: f32| {
            for line in wrap(fonts, face, size, text, text_w - indent) {
                ui::text(c, fonts, x + pad + indent, (*pen + size).round(), &line, face, size, ink);
                *pen += line_h;
            }
        };
        if let Some(why) = nothing {
            say(c, fonts, &mut pen, why, font::UI, theme::dim(), 0.0);
        } else if self.no_engine {
            let card_top = pen;
            pen += pad;
            let lines = [
                "No TeX engine is installed, so nothing can be built yet.",
                "Tectonic is one program that fetches the TeX packages a document needs as it builds. \
                Lightdown can download it, about 10 MB, into its own folder; no administrator rights are needed.",
            ];
            for (i, text) in lines.iter().enumerate() {
                let (face, ink) = if i == 0 { (font::UI_MEDIUM, theme::fg()) } else { (font::UI, theme::dim()) };
                say(c, fonts, &mut pen, text, face, ink, pad);
                pen += line_h / 3.0;
            }
            if !busy {
                let label = "Set up Tectonic";
                let bh = px(theme::CONTROL, s);
                let bw = ui::button_width(fonts, label, true, s);
                let r = ((x + pad * 2.0).round(), pen.round(), bw, bh);
                ui::button(
                    c,
                    fonts,
                    r,
                    label,
                    Some(crate::icons::Icon::ArrowDown),
                    ui::Variant::Primary,
                    ui::inside(r, self.hover),
                    s,
                );
                self.hits.push((r, Hit::Setup));
                pen += bh + pad;
            }
            let card = (x + pad, card_top, text_w, pen - card_top);
            c.round_ring(card.0, card.1, card.2, card.3, px(theme::RADIUS, s), 1.0, theme::edge());
            pen += pad;
            let other = "Or install TeX Live with the system's packages, such as texlive on Arch Linux or \
                texlive-latex-recommended on Ubuntu, and choose its engine in the settings.";
            say(c, fonts, &mut pen, other, font::UI, theme::faint(), 0.0);
        } else {
            // What went wrong comes first; a line it names is a click away.
            for (i, p) in self.problems.iter().enumerate() {
                let at = match p.line {
                    Some(line) if p.file.is_empty() => format!("Line {line}"),
                    Some(line) => format!("{}, line {line}", p.file),
                    None => "Error".to_string(),
                };
                let indent = px(24.0, s);
                let lines = wrap(fonts, font::UI, size, &p.text, text_w - indent - pad).len().max(1);
                let tall = line_h * (lines + 1) as f32 + pad;
                let r = (x + pad, pen, text_w, tall);
                let lit = p.line.is_some() && ui::inside(r, self.hover);
                c.round_rect(
                    r.0,
                    r.1,
                    r.2,
                    r.3,
                    px(theme::RADIUS, s),
                    if lit { theme::hover() } else { theme::wash(theme::danger()) },
                );
                let isz = px(theme::SMALL_ICON, s);
                crate::icons::draw(
                    c,
                    crate::icons::Icon::Alert,
                    r.0 + px(9.0, s),
                    (pen + pad / 2.0 + (line_h - isz) / 2.0).round(),
                    isz,
                    theme::danger(),
                );
                let mut inner = pen + pad / 2.0;
                ui::text(
                    c,
                    fonts,
                    r.0 + indent + px(4.0, s),
                    (inner + size).round(),
                    &at,
                    font::UI_MEDIUM,
                    size,
                    theme::danger(),
                );
                inner += line_h;
                for line in wrap(fonts, font::UI, size, &p.text, text_w - indent - pad) {
                    ui::text(
                        c,
                        fonts,
                        r.0 + indent + px(4.0, s),
                        (inner + size).round(),
                        &line,
                        font::UI,
                        size,
                        theme::fg(),
                    );
                    inner += line_h;
                }
                if p.line.is_some() {
                    self.hits.push((r, Hit::Problem(i)));
                }
                pen += tall + px(6.0, s);
            }
            if !self.problems.is_empty() {
                pen += line_h / 2.0;
            }
            if let Some(note) = &self.note {
                say(c, fonts, &mut pen, note, font::UI, theme::dim(), 0.0);
                pen += line_h / 2.0;
            }
            if self.pages.is_none() && self.problems.is_empty() && self.note.is_none() && !busy {
                say(
                    c,
                    fonts,
                    &mut pen,
                    "Build to see the pages here. F5 builds, and so does saving once the preview is open.",
                    font::UI,
                    theme::dim(),
                    0.0,
                );
            }
            if let Some(pages) = &self.pages {
                // The widest page fills the column.
                let (k, gap) = (text_w / pages.widest(), px(12.0, s));
                let mid = x + pad + text_w / 2.0;
                self.placed = Some((mid, pen + self.scroll, k, gap));
                let pics = &mut self.pics.borrow_mut();
                pen = pages.draw(c, pics, mid, pen, k, gap, top, y + h);
            }
            if let Some((page, r)) = self.spot
                && let Some(p) = self.page_rects().get(page)
            {
                let k = self.placed.map_or(1.0, |p| p.2);
                let (sx, sy) = (p.0 + r[0] * k - 2.0 * s, p.1 + r[1] * k - 2.0 * s);
                c.shade(sx, sy, r[2] * k + 4.0 * s, r[3] * k + 4.0 * s, theme::primary(), 80);
            }
        }
        self.tall = pen + self.scroll - top + pad;
        let room = y + h - top;
        self.scroll = self.scroll.min(self.tall - room).max(0.0);
        c.unclip();
        if let Some((page, rect)) = self.pending.take().filter(|_| self.placed.is_some()) {
            self.show_spot(page, rect);
            self.again = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_up_tectonic_is_offered_only_without_an_engine() {
        let fonts = Rc::new(RefCell::new(Fonts::new()));
        let pics = Rc::new(RefCell::new(Pics::new()));
        let mut view = Preview::new(fonts, pics);
        view.set_rect(0.0, 0.0, 400.0, 600.0, 1.0);
        let mut px = vec![0; 400 * 600];
        let mut canvas = Canvas::new(&mut px, 400, 600);
        let has = |v: &Preview, hit| v.hits.iter().any(|(_, h)| *h == hit);
        view.draw(&mut canvas, false, None);
        assert!(view.hits.is_empty());
        view.no_engine = true;
        view.draw(&mut canvas, false, None);
        assert!(has(&view, Hit::Setup));
        view.draw(&mut canvas, true, None);
        assert!(!has(&view, Hit::Setup));
        view.pdf = Some("earlier.pdf".into());
        view.draw(&mut canvas, false, Some("This document cannot be previewed."));
        assert!(view.hits.is_empty());
    }
}
