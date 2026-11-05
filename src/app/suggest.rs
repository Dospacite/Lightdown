//! Suggestions at the cursor: on a key, a model is asked what comes next,
//! and its answer is shown in place, dimmed, to take or to leave.

use super::{App, Focus, Key, Mods};
use crate::agent::{self, Stream, Wire};
use crate::editor::{self, EditKind};
use crate::pane::Doc;
use crate::tools;

const PROMPT: &str = "You are the inline completion engine of a text editor. You are given a file in two parts: \
<prefix> holds what comes before the cursor and <suffix> what comes after it. Write the text that should be \
inserted at the cursor: the rest of the current line or statement, and further lines only where they plainly \
belong to it, such as the body of the function being written. Reply with that text and nothing else: no \
explanation, no code fences, nothing that repeats the prefix or the suffix. Keep to the file's language, \
indentation and style. If nothing should be inserted, reply with nothing.";

/// How much of the file goes along, before and after the cursor, in bytes.
const BEFORE: usize = 8000;
const AFTER: usize = 2000;
/// A suggestion longer than this is cut: it is meant to be read at a glance.
const LINES: usize = 40;

/// A request for a suggestion that has not been answered in full.
pub(super) struct Suggesting {
    stream: Stream,
    prediction: Option<super::predictions::Context>,
    model: String,
    reply: String,
    /// The text around the cursor that was sent, to tidy the answer by.
    before: String,
    after: String,
    edit: Option<(u64, u64, usize, usize, String)>,
    location: Option<(u64, u64, usize)>,
}

/// What a model answered, made fit to insert: without the wrapping and the
/// repetition that models add in spite of being asked not to.
fn clean(reply: &str, before: &str, after: &str) -> String {
    let mut s = reply.replace("\r\n", "\n");
    // A fenced block is unwrapped.
    if let Some(rest) = s.trim_start().strip_prefix("```") {
        let body = rest.split_once('\n').map_or("", |(_, body)| body);
        s = body.rsplit_once("```").map_or(body, |(code, _)| code).trim_end_matches('\n').to_string();
    }
    // The line being completed is often said again from its start.
    let head = before.rsplit('\n').next().unwrap_or("");
    let again = !head.is_empty() && s.starts_with(head) && (head.trim().len() >= 3 || head.trim().is_empty());
    if again {
        s.drain(..head.len());
    } else if head.trim().is_empty() && !head.is_empty() {
        // Or its indentation alone, in part.
        let shared = s.bytes().zip(head.bytes().rev()).take_while(|(a, b)| a == b && *a == b' ').count();
        if shared > 0 && s[shared..].starts_with(|c: char| !c.is_whitespace()) {
            s.drain(..shared);
        }
    }
    // What follows the cursor is sometimes said again at the end.
    let next = after.split('\n').next().unwrap_or("").trim();
    if next.len() >= 2 && s.trim_end().ends_with(next) {
        s.truncate(s.trim_end().len() - next.len());
    }
    let s = s.trim_end();
    let end = s.match_indices('\n').nth(LINES - 1).map_or(s.len(), |(i, _)| i);
    s[..end].to_string()
}

impl App {
    /// Whether a suggestion is showing or on its way.
    pub(super) fn suggesting(&self) -> bool {
        self.suggest.is_some() || self.doc.ghost().is_some() || self.predictions.shown.is_some() || self.copilot_pending()
    }

    /// Takes away the suggestion and forgets any that is on its way. Call
    /// before the document is edited, saved or read by anything else.
    pub(super) fn unsuggest(&mut self) {
        self.suggest = None;
        self.cancel_copilot_prediction();
        let origin = self.predictions.origin.take().filter(|_| !self.predictions.jumped);
        self.predictions.shown = None;
        self.doc.clear_prediction();
        self.suggest_due = None;
        self.doc.drop_ghost();
        if let Some((anchor,cur)) = origin { self.doc.select(anchor,cur); }
        if let Some((a, original, anchor, cur)) = self.inline_preview.take() {
            self.doc.preview_raw(a, a, &original);
            self.doc.select(anchor, cur);
        }
    }

    /// Asks the suggestion model what belongs at the cursor.
    pub(super) fn ask_legacy_suggestion(&mut self) {
        self.unsuggest();
        self.comp = None;
        if self.doc.read_only || self.doc.buf.has_sel() {
            return;
        }
        let Some(key) = self.cfg.key_for(&self.cfg.suggest_model) else {
            let keys = self.keys_of("settings");
            self.message = Some(format!("Configure the suggestion model's provider API key in the settings ({keys})"));
            return;
        };
        if let Some(why) = self.over_limit() {
            self.message = Some(why);
            return;
        }
        let (text, cur) = (&self.doc.buf.text, self.doc.buf.cur);
        let from = editor::floor(text, cur.saturating_sub(BEFORE));
        let mut to = (cur + AFTER).min(text.len());
        while !text.is_char_boundary(to) {
            to += 1;
        }
        let (before, after) = (self.plain(&text[from..cur]), self.plain(&text[cur..to]));
        let name = self.doc.path.as_deref().map_or("untitled".into(), |p| tools::short(&self.root, p));
        let language = match (self.doc.doc, &self.nb) {
            (_, Some(_)) => "a Jupyter notebook; cells are divided by `# %%` lines",
            (Doc::Python, _) => "Python",
            (Doc::Markdown, _) => "markdown",
            (Doc::Latex, _) => "LaTeX",
            _ => "see the file name",
        };
        let user = format!("File: {name} ({language})\n<prefix>{before}</prefix>\n<suffix>{after}</suffix>");
        self.ai.catalog.refresh(&self.waker);
        self.ai.catalog.poll();
        let model = self.cfg.agent_model(&self.ai.catalog, &self.cfg.suggest_model);
        let session = format!("{}-suggest", self.ai.thread.id);
        let req = agent::completion(&model, &key, &session, PROMPT, &user, &self.cfg.suggest_reasoning);
        let stream = Stream::start(req, self.waker.clone());
        self.suggest = Some(Suggesting { stream, prediction: None, model: model.key, reply: String::new(), before, after, edit: None, location: Some((self.file_tabs[self.active_file].id, self.doc.buf.version, cur)) });
        self.message = Some(format!("Asking {} for a suggestion\u{2026}", model.name));
    }

    /// Takes in what has come of the suggestion asked for.
    pub(super) fn suggest_poll(&mut self) {
        self.copilot_poll();
        self.auto_suggestion_poll();
        let wires = self.suggest.as_ref().map_or(Vec::new(), |s| s.stream.poll());
        for w in wires {
            if self.suggest_wire(w) {
                break;
            }
        }
    }

    /// Takes one piece of the answer. Returns whether that was the end.
    fn suggest_wire(&mut self, w: Wire) -> bool {
        let Some(s) = &mut self.suggest else { return true };
        match w {
            Wire::Text(t) => s.reply += &t,
            Wire::Usage(u) => {
                let cost = self.cfg.agent_model(&self.ai.catalog, &s.model).price(&u);
                self.ai.spend(&s.model, &self.root.to_string_lossy(), cost);
            }
            Wire::Fail(why) => {
                self.message = Some(format!("No suggestion: {why}"));
                self.suggest = None;
                return true;
            }
            Wire::Done => {
                if let Some(cx) = s.prediction.take() {
                    let reply = s.reply.clone();
                    self.suggest = None;
                    self.prediction_reply(cx, &reply);
                    return true;
                }
                if let Some((tab, version, a, b, _)) = s.edit.take() {
                    let reply = s.reply.clone();
                    self.suggest = None;
                    if self.file_tabs[self.active_file].id != tab || self.doc.buf.version != version {
                        self.message = Some("Inline edit discarded because the source changed".into());
                        return true;
                    }
                    let text = if reply.trim_start().starts_with("```") { clean(&reply, "", "") } else { reply };
                    self.inline_preview(a, b, &text);
                    return true;
                }
                if s.location.is_some_and(|loc| loc != (self.file_tabs[self.active_file].id, self.doc.buf.version, self.doc.buf.cur)) {
                    self.suggest = None;
                    return true;
                }
                let text = clean(&s.reply, &s.before, &s.after);
                self.suggest = None;
                self.message = text.is_empty().then(|| "Nothing to suggest here".into());
                if self.focus == Focus::Doc {
                    self.doc.show_ghost(&text);
                }
                return true;
            }
            Wire::Reasoning(_) | Wire::Call(_) | Wire::Blocks(_) => {}
        }
        false
    }

    /// Waits for the suggestion asked for. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn suggest_settle(&mut self) {
        while let Some(w) = self.suggest.as_ref().map(|s| s.stream.wait()) {
            if w.is_none_or(|w| self.suggest_wire(w)) {
                self.suggest = None;
            }
        }
    }

    /// A key pressed while a suggestion shows or is on its way. Returns
    /// whether the key was for the suggestion; any other key dismisses it
    /// and then does what it always does.
    pub(super) fn suggest_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if self.inline_preview.is_none() && self.predictions.shown.is_none() { self.prediction_observe(); }
        if let Some(taken) = self.prediction_key(key, mods, text) { return taken; }
        if let Some(taken) = self.inline_key(key, mods) {
            return taken;
        }
        if !self.suggesting() {
            return false;
        }
        let plain = !mods.ctrl && !mods.alt;
        let typed = text.filter(|t| plain && !t.is_empty() && !t.chars().any(char::is_control));
        let ghost = self.doc.ghost().unwrap_or("");
        match key {
            Key::Tab if plain && !mods.shift && !ghost.is_empty() => self.doc.accept_ghost(ghost.len()),
            // Word by word, taking the space before a word with it.
            Key::Right if mods.ctrl && !ghost.is_empty() => self.doc.accept_ghost(editor::word_right(ghost, 0)),
            Key::Escape => self.unsuggest(),
            // Typing what is suggested keeps the rest of it.
            _ if typed.is_some_and(|t| ghost.starts_with(t) && ghost.len() > t.len()) => {
                let (typed, rest) = (typed.unwrap().to_string(), ghost[typed.unwrap().len()..].to_string());
                self.doc.drop_ghost();
                self.doc.insert(&typed, EditKind::Type);
                self.doc.moved();
                self.doc.show_ghost(&rest);
            }
            _ => {
                self.unsuggest();
                return false;
            }
        }
        self.edited();
        true
    }
}

impl Suggesting {
    pub(super) fn prediction(stream: Stream, model: String, cx: super::predictions::Context) -> Self {
        Self { stream, prediction: Some(cx), model, reply: String::new(), before: String::new(), after: String::new(), edit: None, location: None }
    }
    pub(super) fn inline(stream: Stream, model: String, edit: (u64, u64, usize, usize, String)) -> Self {
        Self { stream, prediction: None, model, reply: String::new(), before: String::new(), after: String::new(), edit: Some(edit), location: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    #[test]
    fn next_edit_is_local_and_automatic_requests_are_opt_in_and_debounced() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("foo foobar foo");
        a.doc.edit(0, 3, "bar", EditKind::Other);
        a.suggest_next_edit();
        assert_eq!(a.doc.buf.text, "bar foobar bar");
        a.key(Key::Escape, NONE, None);
        assert_eq!(a.doc.buf.text, "bar foobar foo");
        a.suggest_next_edit(); a.key(Key::Tab, NONE, None);
        assert_eq!(a.doc.buf.text, "bar foobar bar");
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "bar foobar foo");
        a.cfg.api_key = "fixture-key-never-sent".into();
        a.schedule_suggestion(); assert!(a.suggest_due.is_none());
        a.cfg.auto_suggest = true;
        a.doc.select(0, 0); a.schedule_suggestion();
        let due = a.suggest_due.unwrap();
        a.schedule_suggestion(); assert!(a.suggest_due.unwrap().0 >= due.0);
        a.doc.select(1, 1); a.auto_suggestion_poll();
        assert!(a.suggest_due.is_none() && a.suggest.is_none());
    }

    #[test]
    fn answers_are_tidied() {
        assert_eq!(clean("a + b", "def add(a, b):\n    return ", "\n"), "a + b");
        // Fences, the line said again, and the text after the cursor go.
        assert_eq!(clean("```python\n    return a + b\n```", "def add(a, b):\n    return ", ""), "a + b");
        assert_eq!(clean("    return a + b", "def add(a, b):\n    ", ""), "return a + b");
        assert_eq!(clean("  return a + b", "def add(a, b):\n    ", ""), "return a + b");
        assert_eq!(clean("a, b):", "def add(", "):"), "a, b");
        // Lines after the first keep their own indentation.
        assert_eq!(clean("a + b\n\nprint(add(1, 2))\n", "    return ", ""), "a + b\n\nprint(add(1, 2))");
        assert_eq!(clean(&"x\n".repeat(100), "", "").lines().count(), LINES);
    }

    #[test]
    fn suggestions_are_taken_or_left() {
        let mut a = App::new(Some(std::env::temp_dir().join("lightdown-suggest-test/a.py")));
        a.resize(800, 600, 1.0);
        a.paste_text("def add(a, b):\n    ");
        a.doc.buf.modified = false;
        let typed = "def add(a, b):\n    ";

        // Shown, it is in the way of nothing: any other key removes it.
        a.doc.show_ghost("return a + b\n\nprint(add(1, 2))");
        assert!(a.suggesting() && !a.doc.buf.modified && a.doc.buf.cur == typed.len());
        a.key(Key::Left, NONE, None);
        assert_eq!(a.doc.buf.text, typed);
        a.key(Key::Right, NONE, None);

        // Typing along with it keeps the rest; Ctrl+Right takes a word.
        a.doc.show_ghost("return a + b");
        a.key(Key::Char('r'), NONE, Some("r"));
        assert_eq!((a.doc.ghost(), a.doc.buf.cur), (Some("eturn a + b"), typed.len() + 1));
        a.key(Key::Right, CTRL, None);
        assert_eq!(a.doc.ghost(), Some(" a + b"));
        a.key(Key::Right, CTRL, None);
        assert_eq!(a.doc.ghost(), Some(" + b"));
        // Tab takes all of it, and undo gives it back piece by piece.
        a.key(Key::Tab, NONE, None);
        assert_eq!((a.doc.buf.text.as_str(), a.suggesting()), ("def add(a, b):\n    return a + b", false));
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "def add(a, b):\n    return a");

        // Esc leaves the text as it was, and a save never writes a suggestion.
        a.doc.show_ghost(" - b");
        a.key(Key::Escape, NONE, None);
        assert_eq!(a.doc.buf.text, "def add(a, b):\n    return a");
        a.doc.show_ghost(" - b");
        a.key(Key::Char('s'), CTRL, None);
        assert!(!a.suggesting() && a.doc.buf.text.trim_end().ends_with("return a"));
        // Without a key there is nothing to ask with.
        a.key(Key::Char('\\'), Mods { ctrl: false, shift: false, alt: true }, None);
        assert!(!a.suggesting() && a.message.as_deref().is_some_and(|m| m.contains("API key")));
    }
}

impl App {
    pub(super) fn schedule_suggestion(&mut self) {
        self.prediction_observe();
        self.suggest_due = None;
        if self.cfg.auto_suggest && self.focus == Focus::Doc && !self.ai.busy() && !self.doc.read_only
            && !self.doc.buf.has_sel() && !self.suggesting() && self.inline_preview.is_none()
            && (self.cfg.edit_prediction_provider == "copilot" || self.cfg.key_for(&self.cfg.suggest_model).is_some())
        {
            self.suggest_due = Some((self.prediction_due(), self.file_tabs[self.active_file].id, self.doc.buf.version, self.doc.buf.cur));
        }
    }
    fn auto_suggestion_poll(&mut self) {
        let Some((due, tab, version, cur)) = self.suggest_due else { return };
        if !self.cfg.auto_suggest || self.focus != Focus::Doc || self.overlaid() || self.ai.busy()
            || (tab, version, cur) != (self.file_tabs[self.active_file].id, self.doc.buf.version, self.doc.buf.cur)
        { self.suggest_due = None; return; }
        if std::time::Instant::now() >= due {
            self.suggest_due = None;
            self.ask_suggestion();
        }
    }
    pub(super) fn suggest_next_edit(&mut self) {
        self.unsuggest();
        if self.doc.read_only || self.nb.is_some() { self.message = Some("Next edits require editable source text".into()); return; }
        let Some((old, new)) = self.doc.buf.last_replacement().map(|(a, b)| (a.to_string(), b.to_string())) else { self.message = Some("Replace one occurrence first".into()); return; };
        let cur = self.doc.buf.cur;
        let text = &self.doc.buf.text;
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let bounded = old.chars().all(word);
        let positions = text.match_indices(&old).filter(|(i, _)| !bounded || (text[..*i].chars().next_back().is_none_or(|c| !word(c)) && text[*i + old.len()..].chars().next().is_none_or(|c| !word(c))));
        let positions: Vec<_> = positions.map(|(i, _)| i).collect();
        let Some(at) = positions.iter().copied().find(|&i| i >= cur).or_else(|| positions.first().copied()) else { self.message = Some("No other matching occurrence".into()); return; };
        self.doc.select(at, at + old.len()); self.doc.moved();
        self.inline_preview(at, at + old.len(), &new);
        self.focus = Focus::Doc;
    }
}
