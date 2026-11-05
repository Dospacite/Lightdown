//! The sidebar: a conversation with the agent. The transcript and the
//! message being written are both text panes; this module lays them out
//! with the controls around them and says what a click lands on.

use std::cell::RefCell;
use std::rc::Rc;

use crate::agent::{Item, Thread};
use crate::cells::{self, Tag};
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::icons::{self, Icon};
use crate::pane::{Doc, Pane};
use crate::pics::Pics;
use crate::tools;
use crate::ui::px;
use crate::{theme, ui};

/// A control in the agent's panel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Threads,
    Model,
    Reasoning,
    Context,
    Stop,
    Send,
    Allow,
    Always,
    Deny,
    Settings,
}

/// What the panel shows around its two panes.
#[derive(Default)]
pub struct View {
    pub images: usize,
    pub title: String,
    pub model: String,
    pub reasoning: String,
    pub context: bool,
    /// Tokens and cost so far, and how spending stands against a limit.
    pub usage: String,
    pub busy: bool,
    /// A tool call waiting for permission: what it would do.
    pub approval: Option<String>,
    /// Shown in place of an empty transcript.
    pub hint: String,
    /// No key to the service is set: the settings are offered instead.
    pub no_key: bool,
    pub focused: bool,
    /// The keys of the panel's controls, for their tooltips.
    pub keys: Keys,
}

/// The keys of the panel's controls as they stand.
#[derive(Default)]
pub struct Keys {
    pub threads: String,
    pub model: String,
    pub reasoning: String,
    pub context: String,
    pub settings: String,
}

pub struct Side {
    pub transcript: Pane,
    pub input: Pane,
    fonts: Rc<RefCell<Fonts>>,
    rect: (f32, f32, f32, f32),
    scale: f32,
    /// Where each control was last drawn.
    hits: Vec<((f32, f32, f32, f32), Hit)>,
    /// Where the pointer is, for lighting the control under it.
    pub hover: Option<(f32, f32)>,
}

/// A thread as the transcript pane shows it: one cell for each thing said
/// or done.
pub fn transcript(thread: &Thread, models: impl Fn(&str) -> String, show_reasoning: bool, busy: bool) -> String {
    let mut out = String::new();
    let clean = |s: &str| s.trim().replace(cells::MARK, "");
    let cell = |out: &mut String, tag: Tag, label: &str, body: &str| {
        if !out.is_empty() {
            out.push('\n');
        }
        *out += &cells::header(tag, label);
        out.push('\n');
        *out += body;
    };
    let answered = |id: &str| thread.items.iter().any(|i| matches!(i, Item::Tool { id: t, .. } if t == id));
    let mut tool_open = false;
    for (n, item) in thread.items.iter().enumerate() {
        let last = n + 1 == thread.items.len();
        let was_tool = std::mem::take(&mut tool_open);
        match item {
            Item::User { text, images, .. } => {
                let body = if images.is_empty() {
                    clean(text)
                } else {
                    format!("{}\n\n[{} attached screenshot(s)]", clean(text), images.len())
                };
                cell(&mut out, Tag::User, "You", &body);
            }
            Item::Assistant { text, reasoning, calls, model, .. } => {
                let (text, reasoning) = (clean(text), clean(reasoning));
                if show_reasoning && !reasoning.is_empty() {
                    cell(&mut out, Tag::Think, "Thinking", &reasoning);
                }
                if !text.is_empty() {
                    cell(&mut out, Tag::Assistant, &models(model), &text);
                } else if last && busy && calls.is_empty() && (reasoning.is_empty() || !show_reasoning) {
                    cell(&mut out, Tag::Assistant, &models(model), "\u{2026}");
                }
                let waiting: Vec<String> = calls
                    .iter()
                    .filter(|c| !answered(&c.id))
                    .map(|c| format!("{} \u{2026}", tools::describe(c)))
                    .collect();
                if !waiting.is_empty() {
                    cell(&mut out, Tag::Tool, "", &clean(&waiting.join("\n")));
                }
            }
            Item::Tool { summary, .. } => {
                if was_tool {
                    out.push('\n');
                    out += &clean(summary);
                } else {
                    cell(&mut out, Tag::Tool, "", &clean(summary));
                }
                tool_open = true;
            }
            Item::Note(text) => cell(&mut out, Tag::Note, "", &clean(text)),
        }
    }
    out
}

impl Side {
    pub fn new(fonts: Rc<RefCell<Fonts>>, pics: Rc<RefCell<Pics>>) -> Side {
        let pane = |doc: Doc| {
            let mut p = Pane::new(fonts.clone(), pics.clone());
            p.set_text(String::new(), doc);
            (p.size, p.margin, p.fill, p.lead, p.keep, p.focused) = (theme::LARGE, 18.0, true, false, 1.0, false);
            p.family = font::SANS;
            p
        };
        let (mut transcript, mut input) = (pane(Doc::Cells), pane(Doc::Text));
        (transcript.read_only, transcript.follow, transcript.chat) = (true, true, true);
        input.margin = 16.0;
        let rect = (0.0, 0.0, 0.0, 0.0);
        Side { transcript, input, fonts: fonts.clone(), rect, scale: 1.0, hits: Vec::new(), hover: None }
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) {
        (self.rect, self.scale) = ((x, y, w, h), scale);
        if w < 1.0 || h < 1.0 {
            self.hits.clear();
            self.input.set_rect(x, y, 0.0, 0.0, scale);
            self.transcript.set_rect(x, y, 0.0, 0.0, scale);
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        rw > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    /// Where a control was last drawn, if it was.
    pub fn place_of(&self, hit: Hit) -> Option<(f32, f32, f32, f32)> {
        self.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)
    }

    /// The control under a point, if any.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let inside = |r: &(f32, f32, f32, f32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
        self.hits.iter().find(|(r, _)| inside(r)).map(|(_, h)| *h)
    }

    fn hot(&self, r: (f32, f32, f32, f32)) -> bool {
        ui::inside(r, self.hover)
    }

    /// One of the composer's controls: an icon and a label on a quiet
    /// button, as T3 Code's model picker. Returns its width.
    #[allow(clippy::too_many_arguments)]
    fn chip(
        &mut self,
        c: &mut Canvas,
        fonts: &mut Fonts,
        x: f32,
        mid: f32,
        icon: Icon,
        label: &str,
        on: bool,
        max: f32,
    ) -> f32 {
        let s = self.scale;
        let size = px(theme::UI_TEXT, s);
        let isz = px(theme::SMALL_ICON, s);
        let pad = px(8.0, s);
        let label = ui::fit(fonts, font::UI_MEDIUM, size, label, (max - isz - pad * 2.0 - px(6.0, s)).max(0.0));
        let tw = if label.is_empty() { 0.0 } else { fonts.width(font::UI_MEDIUM, size, &label) + px(6.0, s) };
        let h = px(theme::CONTROL, s);
        let r = (x.round(), (mid - h / 2.0).round(), (isz + tw + pad * 2.0).round(), h);
        let hot = self.hot(r);
        if hot {
            c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS, s), theme::hover());
        } else if on {
            c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS, s), theme::active());
        }
        let ink = if hot || on { theme::fg() } else { theme::dim() };
        icons::draw(c, icon, r.0 + pad, (mid - isz / 2.0).round(), isz, if on { theme::fg() } else { theme::icon() });
        if !label.is_empty() {
            ui::text(
                c,
                fonts,
                r.0 + pad + isz + px(6.0, s),
                ui::base(r.1, h, size),
                &label,
                font::UI_MEDIUM,
                size,
                ink,
            );
        }
        r.2
    }

    pub fn draw(&mut self, c: &mut Canvas, view: &View) {
        let (x, y, w, h) = self.rect;
        if w < 1.0 || h < 1.0 {
            return;
        }
        let s = self.scale;
        let size = px(theme::UI_TEXT, s);
        let small = px(theme::SMALL, s);
        let fc = self.fonts.clone();
        self.hits.clear();
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());
        let margin = px(12.0, s);

        // The thread: its title, which leads to the others, and what it has
        // used.
        let bar = px(32.0, s);
        let mut top = y;
        if !self.transcript.buf.text.is_empty() {
            let fonts = &mut fc.borrow_mut();
            let right_w =
                if view.usage.is_empty() { 0.0 } else { fonts.width(font::UI, small, &view.usage) + px(12.0, s) };
            let title = ui::fit(fonts, font::UI_MEDIUM, size, &view.title, w - margin * 2.0 - right_w - px(24.0, s));
            let tw = fonts.width(font::UI_MEDIUM, size, &title);
            let r = (x + margin - px(6.0, s), y + px(4.0, s), tw + px(28.0, s), bar - px(8.0, s));
            if self.hot(r) {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::hover());
            }
            ui::text(c, fonts, x + margin, ui::base(y, bar, size), &title, font::UI_MEDIUM, size, theme::fg());
            let isz = px(12.0, s);
            icons::draw(
                c,
                Icon::ChevronDown,
                x + margin + tw + px(5.0, s),
                (y + (bar - isz) / 2.0).round(),
                isz,
                theme::icon(),
            );
            self.hits.push((r, Hit::Threads));
            ui::tip(r, "Threads", &view.keys.threads);
            if !view.usage.is_empty() {
                let uw = fonts.width(font::UI, small, &view.usage);
                ui::text(
                    c,
                    fonts,
                    x + w - margin - uw,
                    ui::base(y, bar, small),
                    &view.usage,
                    font::UI,
                    small,
                    theme::faint(),
                );
            }
            top = y + bar;
        }

        // The composer: what is being written, and below it the model, the
        // reasoning, the context and the send button.
        let bx = x + margin;
        let bw = w - margin * 2.0;
        let foot = px(44.0, s);
        let pad_top = px(if view.images > 0 { 34.0 } else { 12.0 }, s);
        self.input.set_rect(bx, 0.0, bw, h, s);
        let line = (self.input.px() * theme::LINE_HEIGHT).round();
        let most = (h * 0.4).max(line * 2.0);
        let text_h = self.input.content_height(most).clamp(line * 2.0, most);
        let box_h = (pad_top + text_h + foot).round();
        let box_y = (y + h - margin - box_h).round();
        let radius = px(theme::RADIUS_2XL, s);
        self.input.focused = view.focused && view.approval.is_none();
        c.round_rect(bx, box_y, bw, box_h, radius, theme::raised());
        let edge = if self.input.focused { theme::mix(theme::edge(), theme::primary(), 110) } else { theme::edge() };
        c.round_ring(bx, box_y, bw, box_h, radius, 1.0, edge);
        self.input.set_rect(bx + 1.0, box_y + pad_top, bw - 2.0, text_h, s);
        self.input.draw(c);
        c.clip(x, y, w, h);
        let mut guard = fc.borrow_mut();
        let fonts = &mut *guard;
        if view.images > 0 {
            ui::text(
                c,
                fonts,
                bx + px(16.0, s),
                box_y + px(20.0, s),
                &format!("{} screenshot(s) attached", view.images),
                font::UI,
                small,
                theme::dim(),
            );
        }
        if self.input.buf.text.is_empty() {
            let hint = if view.busy {
                "Working\u{2026} Esc stops it"
            } else {
                "Ask anything. Enter sends, Shift+Enter adds a line"
            };
            let px_ = self.input.px();
            let hint = ui::fit(fonts, font::UI, px_, hint, bw - px(32.0, s));
            let base = (box_y + pad_top + line / 2.0 + px_ * 0.36).round();
            ui::text(c, fonts, bx + px(16.0, s), base, &hint, font::UI, px_, theme::faint());
        }
        let mid = box_y + box_h - foot / 2.0 - px(2.0, s);
        // Send, or Stop while the agent works.
        let d = px(32.0, s);
        let send = (bx + bw - px(8.0, s) - d, (mid - d / 2.0).round(), d, d);
        if view.busy {
            let fill = if self.hot(send) { theme::mix(theme::destructive(), 0, 30) } else { theme::destructive() };
            ui::round_button(c, send, Icon::Stop, fill, 0xffffff, s);
            self.hits.push((send, Hit::Stop));
            ui::tip(send, "Stop", "Esc");
        } else if !self.input.buf.text.trim().is_empty() || view.images > 0 {
            let fill = if self.hot(send) { theme::primary_hover() } else { theme::primary() };
            ui::round_button(c, send, Icon::ArrowUp, fill, theme::on_primary(), s);
            self.hits.push((send, Hit::Send));
            ui::tip(send, "Send", "Enter");
        } else {
            ui::round_button(c, send, Icon::ArrowUp, theme::mix(theme::raised(), theme::fg(), 30), theme::dim(), s);
        }
        let mut cx = bx + px(8.0, s);
        let room = send.0 - px(8.0, s) - cx;
        let reasoning_w = if view.reasoning.is_empty() { 0.0 } else { px(110.0, s) };
        let model_max = (room - reasoning_w - px(44.0, s)).max(px(60.0, s));
        let mw = self.chip(c, fonts, cx, mid, Icon::Bot, &view.model, false, model_max);
        let r = (cx, mid - px(14.0, s), mw, px(28.0, s));
        self.hits.push((r, Hit::Model));
        ui::tip(r, "Model", &view.keys.model);
        cx += mw + px(2.0, s);
        if !view.reasoning.is_empty() && cx + px(60.0, s) < send.0 {
            let rw = self.chip(
                c,
                fonts,
                cx,
                mid,
                Icon::Gauge,
                &view.reasoning,
                false,
                (send.0 - px(44.0, s) - cx).min(px(140.0, s)),
            );
            let r = (cx, mid - px(14.0, s), rw, px(28.0, s));
            self.hits.push((r, Hit::Reasoning));
            ui::tip(r, "Reasoning", &view.keys.reasoning);
            cx += rw + px(2.0, s);
        }
        if cx + px(36.0, s) < send.0 {
            let cw = self.chip(c, fonts, cx, mid, Icon::AtSign, "", view.context, px(40.0, s));
            let r = (cx, mid - px(14.0, s), cw, px(28.0, s));
            self.hits.push((r, Hit::Context));
            let tip = if view.context {
                "IDE context is sent: the open file, cursor and selection"
            } else {
                "IDE context is not sent"
            };
            ui::tip(r, tip, &view.keys.context);
        }

        // A tool call waiting for a yes or no sits above the composer.
        let mut bottom = box_y - px(8.0, s);
        if let Some(what) = &view.approval {
            let mono = fonts.mono();
            let lines: Vec<String> =
                what.lines().take(3).map(|l| ui::fit(fonts, mono, small, l, bw - px(28.0, s))).collect();
            let row = (small * 1.5).round();
            let card_h = px(14.0, s)
                + px(20.0, s)
                + px(6.0, s)
                + row * lines.len() as f32
                + px(12.0, s)
                + px(30.0, s)
                + px(14.0, s);
            let cy = (bottom - card_h).round();
            c.round_rect(bx, cy, bw, card_h, px(theme::RADIUS_XL, s), theme::wash(theme::warning()));
            c.round_ring(
                bx,
                cy,
                bw,
                card_h,
                px(theme::RADIUS_XL, s),
                1.0,
                theme::mix(theme::bg(), theme::warning(), 90),
            );
            let mut pen = cy + px(14.0, s);
            let isz = px(theme::ICON, s);
            icons::draw(
                c,
                Icon::Alert,
                bx + px(14.0, s),
                (pen + (px(20.0, s) - isz) / 2.0).round(),
                isz,
                theme::warning(),
            );
            ui::text(
                c,
                fonts,
                bx + px(38.0, s),
                ui::base(pen, px(20.0, s), size),
                "Allow this?",
                font::UI_BOLD,
                size,
                theme::fg(),
            );
            pen += px(26.0, s);
            for l in &lines {
                ui::text(c, fonts, bx + px(14.0, s), ui::base(pen, row, small), l, mono, small, theme::fg());
                pen += row;
            }
            pen += px(12.0, s);
            let mut ax = bx + px(14.0, s);
            for (label, variant, hit, keys) in [
                ("Allow once", ui::Variant::Primary, Hit::Allow, "Enter"),
                ("Always allow", ui::Variant::Outline, Hit::Always, "A"),
                ("Deny", ui::Variant::Ghost, Hit::Deny, "Esc"),
            ] {
                let aw = ui::button_width(fonts, label, false, s);
                if ax + aw > bx + bw - px(8.0, s) {
                    break;
                }
                let r = (ax, pen, aw, px(30.0, s));
                ui::button(c, fonts, r, label, None, variant, self.hot(r), s);
                self.hits.push((r, hit));
                ui::tip(r, label, keys);
                ax += aw + px(8.0, s);
            }
            bottom = cy - px(8.0, s);
        }

        // The conversation, or what to do first.
        drop(guard);
        self.transcript.set_rect(x, top, w, (bottom - top).max(0.0), s);
        if self.transcript.buf.text.is_empty() {
            let fonts = &mut fc.borrow_mut();
            let lines: Vec<&str> = view.hint.lines().collect();
            let row = (size * 1.6).round();
            let block = px(36.0, s) + px(28.0, s) + row * lines.len() as f32 + px(16.0, s) + px(30.0, s);
            let mut pen = (top + ((bottom - top) - block).max(0.0) * 0.42).round();
            let isz = px(28.0, s);
            icons::draw(c, Icon::Message, (x + (w - isz) / 2.0).round(), pen, isz, theme::faint());
            pen += px(36.0, s);
            let title = "Ask the agent";
            let large = px(theme::LARGE + 1.0, s);
            let tw = fonts.width(font::UI_BOLD, large, title);
            ui::text(
                c,
                fonts,
                (x + (w - tw) / 2.0).round(),
                ui::base(pen, px(24.0, s), large),
                title,
                font::UI_BOLD,
                large,
                theme::fg(),
            );
            pen += px(28.0, s);
            for l in lines {
                let l = ui::fit(fonts, font::UI, size, l, w - margin * 2.0);
                let lw = fonts.width(font::UI, size, &l);
                ui::text(
                    c,
                    fonts,
                    (x + (w - lw) / 2.0).round(),
                    ui::base(pen, row, size),
                    &l,
                    font::UI,
                    size,
                    theme::dim(),
                );
                pen += row;
            }
            pen += px(16.0, s);
            if view.no_key {
                let label = "Open settings";
                let bw2 = ui::button_width(fonts, label, true, s);
                let r = ((x + (w - bw2) / 2.0).round(), pen, bw2, px(30.0, s));
                ui::button(c, fonts, r, label, Some(Icon::Settings), ui::Variant::Primary, self.hot(r), s);
                self.hits.push((r, Hit::Settings));
                ui::tip(r, "Settings", &view.keys.settings);
            } else {
                let keys = [(view.keys.model.as_str(), "model"), (view.keys.threads.as_str(), "threads")];
                let widths: Vec<f32> = keys
                    .iter()
                    .map(|(k, d)| ui::kbd_width(fonts, k, s) + px(6.0, s) + fonts.width(font::UI, small, d))
                    .collect();
                let total = widths.iter().sum::<f32>() + px(18.0, s);
                let mut hx = (x + (w - total) / 2.0).round();
                if total < w - margin * 2.0 {
                    for ((k, d), kw) in keys.iter().zip(widths) {
                        let used = ui::kbd(c, fonts, hx, pen + px(15.0, s), k, s) + px(6.0, s);
                        ui::text(
                            c,
                            fonts,
                            hx + used,
                            ui::base(pen, px(30.0, s), small),
                            d,
                            font::UI,
                            small,
                            theme::faint(),
                        );
                        hx += kw + px(18.0, s);
                    }
                }
            }
        } else {
            self.transcript.draw(c);
        }
        c.unclip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::agent::Call;
    use serde_json::Value;

    #[test]
    fn threads_read_as_cells() {
        let mut t = Thread::new();
        t.items.push(Item::User { text: "Fix it".into(), context: "ctx".into(), images: Vec::new() });
        t.items.push(Item::Assistant {
            text: String::new(),
            reasoning: "Let me look".into(),
            calls: vec![
                Call { id: "1".into(), name: "read_file".into(), args: r#"{"path":"a.py"}"#.into() },
                Call { id: "2".into(), name: "run_command".into(), args: r#"{"command":"pytest"}"#.into() },
            ],
            model: "go/m".into(),
            blocks: Value::Null,
        });
        t.items.push(Item::Tool { id: "1".into(), summary: "read a.py".into(), result: "x".into(), ok: true });
        let name = |m: &str| m.to_uppercase();
        assert_eq!(
            transcript(&t, name, true, true),
            "\u{1e}user You\nFix it\n\u{1e}think Thinking\nLet me look\n\u{1e}tool\nrun pytest \u{2026}\n\u{1e}tool\nread a.py"
        );
        t.items.push(Item::Tool { id: "2".into(), summary: "run pytest".into(), result: "ok".into(), ok: true });
        t.items.push(Item::Assistant {
            text: "Done.".into(),
            reasoning: String::new(),
            calls: Vec::new(),
            model: "go/m".into(),
            blocks: Value::Null,
        });
        assert_eq!(
            transcript(&t, name, false, false),
            "\u{1e}user You\nFix it\n\u{1e}tool\nread a.py\nrun pytest\n\u{1e}assistant GO/M\nDone."
        );
    }
}
