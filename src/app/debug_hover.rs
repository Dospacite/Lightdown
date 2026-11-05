//! Values under the pointer while paused. In the paused frame's file a name
//! or dotted chain such as `self.items` shows its value: the Python bridge
//! looks it up without calling properties or functions, and a DAP adapter
//! is asked with `evaluate` in the hover context. Nothing else is evaluated.
use super::*;
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct Hover {
    seq: u64,
    /// The request out: its number, the name's range and the chain.
    pending: Option<(u64, (usize, usize), String)>,
}

/// The dotted chain ending with the name at `range`, such as `self.items`
/// for `items` in `self.items.append(x)`. None when the chain hangs off a
/// call, an index or anything else that is not a plain name.
pub(super) fn chain(text: &str, range: (usize, usize)) -> Option<String> {
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit());
    let mut start = range.0;
    if !ident(text.get(range.0..range.1)?) { return None; }
    for _ in 0..10 {
        let before = &text[..start];
        let Some(dot) = before.strip_suffix('.') else { break };
        let from = dot.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + dot[i..].chars().next().map_or(1, char::len_utf8));
        if !ident(&dot[from..]) { return None; }
        start = from;
    }
    if text[..start].ends_with('.') || range.1 - start > 200 { return None; }
    Some(text[start..range.1].to_string())
}

impl App {
    /// Asks the paused debugger for the name at `range` in the document.
    /// Returns whether it was asked, so that nothing else is.
    pub(super) fn debug_hover_ask(&mut self, range: (usize, usize)) -> bool {
        let d = &self.debugger;
        if !d.paused || self.doc.buf.modified || self.nb.is_some() { return false; }
        let selected = d.snapshot["selected"].as_u64().unwrap_or(0) as usize;
        let frame = d.snapshot["stack"][selected].clone();
        if self.doc.path.as_deref() != frame["path"].as_str().map(Path::new) { return false; }
        let Some(expression) = chain(&self.doc.buf.text, range) else { return false };
        let keyword = matches!(expression.as_str(), "def" | "class" | "return" | "if" | "else" | "elif" | "for" | "while" | "in" | "import" | "from" | "not" | "and" | "or" | "is" | "fn" | "let" | "mut" | "pub" | "use" | "match");
        if keyword { return false; }
        self.debugger.hover.seq += 1;
        let id = self.debugger.hover.seq;
        if let Some(p) = &self.debugger.process {
            p.send(json!({"action":"hover","expression":expression,"id":id}));
        } else if self.debugger.dap.is_some() {
            let frame_id = frame["id"].clone();
            self.native_hover(id, &expression, frame_id, selected);
        } else {
            return false;
        }
        self.debugger.hover.pending = Some((id, range, expression));
        true
    }

    /// Takes the debugger's answer: a value is shown in the hover box, and
    /// a lookup that failed shows nothing, except a property left alone.
    pub(super) fn debug_hover_answer(&mut self, v: &Value) {
        let Some((id, range, expression)) = self.debugger.hover.pending.take() else { return };
        if v["id"].as_u64() != Some(id) { self.debugger.hover.pending = Some((id, range, expression)); return; }
        let text = if let Some(value) = v["value"].as_str() {
            let kind = v["type"].as_str().filter(|t| !t.is_empty()).map_or(String::new(), |t| format!(": {t}"));
            format!("{expression}{kind} = {}", value.chars().take(500).collect::<String>())
        } else if let Some(why) = v["error"].as_str().filter(|e| e.contains("property")) {
            why.chars().take(200).collect()
        } else {
            return;
        };
        self.docs_value(range, text);
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_are_plain_names_and_dots_only() {
        let text = "self.items.append(x) + f().y + a[0].b + obj.inner.value";
        let at = |name: &str, nth: usize| { let s = text.match_indices(name).nth(nth).unwrap().0; (s, s + name.len()) };
        assert_eq!(chain(text, at("items", 0)).as_deref(), Some("self.items"));
        assert_eq!(chain(text, at("self", 0)).as_deref(), Some("self"));
        assert_eq!(chain(text, at("append", 0)).as_deref(), Some("self.items.append"));
        assert_eq!(chain(text, at("y", 0)), None);
        assert_eq!(chain(text, at("b", 0)), None);
        assert_eq!(chain(text, at("value", 0)).as_deref(), Some("obj.inner.value"));
        assert_eq!(chain("x.ü", (2, 4)).as_deref(), Some("x.ü"));
    }

    fn wait(a: &mut App, what: &str, predicate: impl Fn(&App) -> bool) {
        let start = std::time::Instant::now();
        while !predicate(a) && start.elapsed().as_secs() < 5 {
            a.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(predicate(a), "{what}: {}", a.debugger.output);
    }

    #[test]
    fn python_hover_reads_stored_values_and_leaves_properties() {
        let dir = std::env::temp_dir().join(format!("ld-debug-hover-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let path = dir.join("main.py");
        let source = "calls = []\nclass Box:\n    def __init__(self):\n        self.items = [1, 2]\n    @property\n    def boom(self):\n        calls.append(1)\n    def go(self):\n        return self.items, self.boom\nBox().go()\nprint('calls', len(calls))\n";
        std::fs::write(&path, source).unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(1200, 800, 1.0);
        a.debug_settings();
        a.debugger.points = vec![(path.canonicalize().unwrap(), 9)];
        a.debug_command("start");
        wait(&mut a, "started", |a| a.debugger.paused);
        a.debug_command("continue");
        wait(&mut a, "paused", |a| a.debugger.paused && a.debugger.snapshot["stack"][0]["line"] == 9);
        let items = source.rfind("items").unwrap();
        a.docs_rest_on((items, items + 5));
        assert!(a.debugger.hover.pending.is_some());
        wait(&mut a, "hover", |a| a.debugger.hover.pending.is_none());
        let shown = |a: &App| a.docs_text();
        assert_eq!(shown(&a).as_deref(), Some("self.items: list = [1, 2]"));
        let boom = source.rfind("boom").unwrap();
        a.docs_rest_on((boom, boom + 4));
        wait(&mut a, "property", |a| a.debugger.hover.pending.is_none());
        assert!(shown(&a).is_some_and(|t| t.contains("property")));
        a.debug_command("continue");
        wait(&mut a, "finished", |a| a.debugger.process.is_none());
        assert!(a.debugger.output.contains("calls 1"), "{}", a.debugger.output);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
