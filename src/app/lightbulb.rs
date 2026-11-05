//! A gutter lightbulb on the cursor line when the server has code actions there.
use super::*;
#[derive(Default)]
pub(super) struct Bulb {
    /// The file, version and line last asked about.
    asked: Option<(PathBuf, u64, usize)>,
    /// The same for the answer, and whether a quick fix is among the actions.
    shown: Option<(PathBuf, u64, usize, bool)>,
}
/// Whether a code-action reply holds anything to offer, and whether any is a quick fix.
pub(super) fn offers(result: &Value) -> Option<bool> {
    let actions: Vec<&Value> = result.as_array().into_iter().flatten().filter(|a| a["disabled"].is_null()).collect();
    if actions.is_empty() { return None; }
    Some(actions.iter().any(|a| a["kind"].as_str().is_some_and(|k| k == "quickfix" || k.starts_with("quickfix.")) || a["diagnostics"].as_array().is_some_and(|d| !d.is_empty())))
}
impl App {
    /// Asks about the cursor line once the file, version or line changes.
    pub(in crate::app) fn lightbulb_tick(&mut self) {
        let Some(path) = self.doc.path.clone() else { return };
        let Some(key) = self.languages.key.clone() else { return };
        if !self.languages.clients.contains_key(&key) || self.languages.opened.get(&path).is_none_or(|(v, _)| *v != self.doc.buf.version) { return; }
        let Some(caps) = self.languages.capabilities.get(&key) else { return };
        if caps["codeActionProvider"].is_null() || caps["codeActionProvider"] == false { return; }
        let line = self.doc.buf.line_of(self.doc.buf.cur);
        let here = (path.clone(), self.doc.buf.version, line);
        if self.languages.bulb.asked.as_ref() == Some(&here) || self.languages.pending.values().any(|p| p.action == "lightbulb") { return; }
        self.languages.bulb.asked = Some(here);
        let (a, b) = self.doc.buf.line(line);
        let text = &self.doc.buf.text;
        let diagnostics: Vec<Value> = self.languages.diagnostics.get(&path).filter(|(v, _)| *v == self.doc.buf.version).map(|(_, d)| {
            d.iter().filter(|d| d["range"]["start"]["line"].as_u64().is_some_and(|s| s as usize <= line) && d["range"]["end"]["line"].as_u64().is_some_and(|e| e as usize >= line)).cloned().collect()
        }).unwrap_or_default();
        self.language_request("lightbulb", "textDocument/codeAction", json!({"textDocument":{"uri":lsp::uri(&path)},"range":{"start":lsp::position(text,a),"end":lsp::position(text,b)},"context":{"diagnostics":diagnostics,"triggerKind":2}}));
    }
    pub(in crate::app) fn lightbulb_result(&mut self, path: PathBuf, version: u64, cursor: usize, result: &Value) {
        let line = self.doc.buf.line_of(cursor.min(self.doc.buf.text.len()));
        self.languages.bulb.shown = offers(result).map(|fix| (path, version, line, fix));
    }
    /// Drawn in the margin left of the line numbers; a click opens the code actions menu.
    pub(in crate::app) fn draw_lightbulb(&mut self, c: &mut Canvas) {
        let Some((path, version, line, fix)) = self.languages.bulb.shown.clone() else { return };
        if self.doc.path.as_ref() != Some(&path) || self.doc.buf.version != version || self.doc.buf.line_of(self.doc.buf.cur) != line || !self.doc.numbers { return; }
        let Some((_, y, h)) = self.doc.spot(self.doc.buf.starts[line]) else { return };
        let (left, top, _, height) = self.doc.bounds();
        if y < top || y + h > top + height { return; }
        let s = self.scale;
        let size = (14.0 * s).round();
        // Beside the line numbers, right of any blame column.
        let x = (self.doc.column().0 - self.doc.marks_gutter() - size - 2.0 * s).max(left + 2.0 * s);
        let r = (x - 2.0 * s, y, size + 4.0 * s, h);
        if ui::inside(r, Some(self.pointer)) { c.round_rect(r.0, r.1 + (h - size - 4.0 * s) / 2.0, r.2, size + 4.0 * s, theme::RADIUS_SM * s, theme::hover()); }
        crate::icons::draw(c, Icon::Lightbulb, x, (y + (h - size) / 2.0).round(), size, if fix { theme::warning() } else { theme::icon() });
        let keys = self.keys_of("lsp_actions");
        self.push_hit(r, Hit::Lightbulb, if fix { "Quick fixes and code actions" } else { "Code actions" }, &keys);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offers_tells_quick_fixes_from_other_actions() {
        assert_eq!(offers(&json!([])), None);
        assert_eq!(offers(&json!([{"title":"x","disabled":{"reason":"no"}}])), None);
        assert_eq!(offers(&json!([{"title":"Extract","kind":"refactor.extract"}])), Some(false));
        assert_eq!(offers(&json!([{"title":"Fix","kind":"quickfix"}])), Some(true));
        assert_eq!(offers(&json!([{"title":"Fix","diagnostics":[{}]}])), Some(true));
    }
}
