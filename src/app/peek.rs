//! Peek: definition or reference targets shown in a block under the cursor
//! line, the way the git hunk preview is, without leaving the file.
use super::*;
pub(super) struct Peek {
    title: &'static str,
    path: PathBuf,
    version: u64,
    line: usize,
    pub(super) items: Vec<Item>,
    pub(super) sel: usize,
}
pub(super) struct Item {
    location: Value,
    path: PathBuf,
    line: usize,
    /// The first context line's number and the lines themselves.
    first: usize,
    lines: Vec<String>,
}
const RESULTS: usize = 200;
const CONTEXT: usize = 3;
const SHOWN: usize = 9;
const LIST: usize = 5;
/// Context lines around a zero-based line, each cut to a sane length.
fn context(text: &str, line: usize) -> (usize, Vec<String>) {
    let first = line.saturating_sub(CONTEXT);
    let lines = text.lines().skip(first).take(SHOWN).map(|l| l.replace('\t', "    ").chars().take(400).collect()).collect();
    (first, lines)
}
impl App {
    /// Open tabs speak for their files, so unsaved text is what is shown.
    fn peek_text(&self, path: &Path, cache: &mut HashMap<PathBuf, Option<String>>) -> Option<String> {
        cache.entry(path.to_path_buf()).or_insert_with(|| {
            for (i, t) in self.file_tabs.iter().enumerate() {
                let pane = if i == self.active_file { Some(&self.doc) } else { t.state.as_ref().map(|s| &s.pane) };
                if let Some(p) = pane.filter(|p| p.path.as_deref() == Some(path)) { return Some(p.buf.text.clone()); }
            }
            if std::fs::metadata(path).map_or(true, |m| m.len() > 2 << 20) { return None; }
            std::fs::read_to_string(path).ok().map(|t| t.replace("\r\n", "\n"))
        }).clone()
    }
    pub(in crate::app) fn peek_show(&mut self, action: &str, result: &Value) {
        let list: Vec<Value> = if let Some(a) = result.as_array() { a.clone() } else if result.is_object() { vec![result.clone()] } else { Vec::new() };
        let mut cache = HashMap::new();
        let mut items = Vec::new();
        for location in list.into_iter().take(RESULTS) {
            let Some(path) = location["uri"].as_str().or(location["targetUri"].as_str()).and_then(lsp::path) else { continue };
            let range = if location["range"].is_object() { &location["range"] } else { &location["targetSelectionRange"] };
            let Some(line) = range["start"]["line"].as_u64().map(|n| n as usize) else { continue };
            let Some(text) = self.peek_text(&path, &mut cache) else { continue };
            let (first, lines) = context(&text, line);
            items.push(Item { location, path, line, first, lines });
        }
        if items.is_empty() { self.message = Some("No locations found".into()); return; }
        let Some(path) = self.doc.path.clone() else { return };
        let title = if action == "peek_references" { "References" } else { "Definition" };
        self.languages.peek = Some(Peek { title, path, version: self.doc.buf.version, line: self.doc.buf.line_of(self.doc.buf.cur), items, sel: 0 });
        self.message = None;
    }
    /// Up and Down choose, Enter opens, Esc closes. Other keys reach the document.
    pub(in crate::app) fn peek_key(&mut self, key: Key, mods: Mods) -> bool {
        let Some(peek) = self.languages.peek.as_mut() else { return false };
        if mods.ctrl || mods.alt || mods.shift { return false; }
        match key {
            Key::Up => peek.sel = peek.sel.saturating_sub(1),
            Key::Down => peek.sel = (peek.sel + 1).min(peek.items.len() - 1),
            Key::Enter => self.peek_open(None),
            Key::Escape => self.languages.peek = None,
            _ => return false,
        }
        self.redraw = true;
        true
    }
    pub(in crate::app) fn peek_open(&mut self, index: Option<usize>) {
        let Some(peek) = self.languages.peek.take() else { return };
        let Some(item) = peek.items.into_iter().nth(index.unwrap_or(peek.sel)) else { return };
        self.languages.locations = vec![item.location];
        self.language_pick("0");
    }
    pub(in crate::app) fn draw_peek(&mut self, c: &mut Canvas) {
        let Some(peek) = &self.languages.peek else { return };
        if self.doc.path.as_ref() != Some(&peek.path) || self.doc.buf.version != peek.version || peek.line >= self.doc.buf.lines() {
            self.languages.peek = None;
            return;
        }
        let at = self.doc.buf.starts[peek.line];
        let Some((_, line_y, line_h)) = self.doc.spot(at) else { return };
        let peek = self.languages.peek.as_ref().unwrap();
        let (x, w) = self.doc.column();
        let s = self.scale;
        let row = 22.0 * s;
        let listed = if peek.items.len() > 1 { peek.items.len().min(LIST) } else { 0 };
        let item = &peek.items[peek.sel];
        let rows = 1 + listed + item.lines.len().max(1);
        let bottom = if self.term_open { self.term_top() } else { self.body_bottom() };
        let top = self.title_h() + self.head_h();
        let height = (rows as f32 * row + 6.0 * s).min((bottom - top).max(row));
        let y = (line_y + line_h).max(top).min(bottom - height);
        c.rect(x, y, w, height, theme::raised());
        c.frame(x, y, w, height, s, theme::edge());
        let fonts = self.fonts.clone();
        let mut fonts = fonts.borrow_mut();
        let size = theme::SMALL * s;
        let name = |p: &Path| p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().into_owned();
        // The head: what this is, which one, and the keys.
        let head = format!("{} · {} of {} · {}:{}", peek.title, peek.sel + 1, peek.items.len(), name(&item.path), item.line + 1);
        let hint = "↑↓ choose · Enter opens · Esc closes";
        let hint_w = fonts.width(font::UI, size, hint);
        let head = ui::fit(&mut fonts, font::UI_MEDIUM, size, &head, (w - hint_w - 32.0 * s).max(0.0));
        ui::text(c, &mut fonts, x + 8.0 * s, ui::base(y, row, size), &head, font::UI_MEDIUM, size, theme::fg());
        ui::text(c, &mut fonts, x + w - hint_w - 8.0 * s, ui::base(y, row, size), hint, font::UI, size, theme::faint());
        // The results, scrolled to keep the chosen one in view.
        let start = peek.sel.saturating_sub(LIST - 1).min(peek.items.len().saturating_sub(LIST));
        let mut hits = Vec::new();
        for (n, (i, it)) in peek.items.iter().enumerate().skip(start).take(listed).enumerate() {
            let r = (x + 1.0, y + (n + 1) as f32 * row, w - 2.0, row);
            if i == peek.sel { c.rect(r.0, r.1, r.2, r.3, theme::active()); }
            let source = it.lines.get(it.line - it.first).map_or("", |l| l.trim());
            let label = format!("{}:{}  {}", name(&it.path), it.line + 1, source);
            let label = ui::fit(&mut fonts, font::UI, size, &label, w - 24.0 * s);
            ui::text(c, &mut fonts, x + 12.0 * s, ui::base(r.1, row, size), &label, font::UI, size, if i == peek.sel { theme::fg() } else { theme::dim() });
            hits.push((r, i));
        }
        // The chosen target's lines, its own line marked.
        let code_y = y + (1 + listed) as f32 * row;
        c.line(x, code_y, w, s, theme::line());
        c.clip(x, code_y, w, (y + height - code_y).max(0.0));
        let face = fonts.mono();
        let mono = 12.0 * s;
        let gutter = fonts.width(face, mono, &format!("{}", item.first + item.lines.len())) + 16.0 * s;
        for (n, text) in item.lines.iter().enumerate() {
            let ly = code_y + n as f32 * row + 3.0 * s;
            let target = item.first + n == item.line;
            if target { c.rect(x + 1.0, ly, w - 2.0, row, theme::active()); }
            ui::text(c, &mut fonts, x + 8.0 * s, ui::base(ly, row, mono), &format!("{}", item.first + n + 1), face, mono, theme::faint());
            ui::text(c, &mut fonts, x + 8.0 * s + gutter, ui::base(ly, row, mono), text, face, mono, if target { theme::fg() } else { theme::dim() });
        }
        c.unclip();
        drop(fonts);
        for (r, i) in hits { self.push_hit(r, Hit::Peek(i), "", ""); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_starts_three_lines_up_and_caps_lines() {
        let text = (0..20).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let (first, lines) = context(&text, 1);
        assert_eq!(first, 0);
        assert_eq!(lines[1], "line 1");
        let (first, lines) = context(&text, 10);
        assert_eq!((first, lines.len()), (7, SHOWN));
        assert_eq!(context("\tx", 0).1[0], "    x");
    }
}
