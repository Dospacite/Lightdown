//! Lazy call and type hierarchies, drawn with the outline's list but without its parsers.
use super::*;
#[derive(Default)]
pub(super) struct Hierarchy {
    direction: String,
    /// Item, depth, expanded.
    nodes: Vec<(Value, usize, bool)>,
    expanding: Option<usize>,
    waiting: bool,
}
const LIMIT: usize = 5000;
fn title(direction: &str) -> &'static str {
    match direction { "incoming" => "Incoming calls", "outgoing" => "Outgoing calls", "supertypes" => "Supertypes", _ => "Subtypes" }
}
impl App {
    pub(in crate::app) fn hierarchy_start(&mut self, direction: &str) {
        self.language_sync();
        let Some(path) = self.doc.path.clone() else { return };
        let calls = matches!(direction, "incoming" | "outgoing");
        let provider = if calls { "callHierarchyProvider" } else { "typeHierarchyProvider" };
        if let Some(caps) = self.languages.key.as_ref().and_then(|k| self.languages.capabilities.get(k))
            && (caps[provider].is_null() || caps[provider] == false)
        {
            self.message = Some(format!("This language server has no {}", if calls { "call hierarchy" } else { "type hierarchy" }));
            return;
        }
        self.languages.hierarchy = Hierarchy { direction: direction.into(), waiting: true, ..Default::default() };
        let method = if calls { "textDocument/prepareCallHierarchy" } else { "textDocument/prepareTypeHierarchy" };
        self.language_request("hierarchy_prepare", method, json!({"textDocument":{"uri":lsp::uri(&path)},"position":lsp::position(&self.doc.buf.text,self.doc.buf.cur)}));
        self.show_side(Some(Tab::Hierarchy));
    }
    pub(in crate::app) fn hierarchy_result(&mut self, action: &str, result: &Value) {
        let h = &mut self.languages.hierarchy;
        h.waiting = false;
        let nodes: Vec<Value> = result.as_array().into_iter().flatten().take(1000).filter_map(|v| {
            let item = match (action, h.direction.as_str()) {
                ("hierarchy_prepare", _) => v,
                (_, "incoming") => &v["from"],
                (_, "outgoing") => &v["to"],
                _ => v,
            };
            item["uri"].is_string().then(|| item.clone())
        }).collect();
        if action == "hierarchy_prepare" {
            h.nodes = nodes.into_iter().map(|v| (v, 0, false)).collect();
            // Show the first level straight away; deeper levels load on click.
            if h.nodes.len() == 1 { self.hierarchy_expand(0); }
        } else if let Some(index) = h.expanding.take().filter(|i| *i < h.nodes.len()) {
            let depth = h.nodes[index].1 + 1;
            h.nodes[index].2 = true;
            let room = LIMIT.saturating_sub(h.nodes.len());
            h.nodes.splice(index + 1..index + 1, nodes.into_iter().take(room).map(|item| (item, depth, false)));
        }
    }
    pub(in crate::app) fn hierarchy_rows(&mut self) {
        let h = &self.languages.hierarchy;
        self.outline.current = None;
        let mut items = Vec::new();
        if !h.direction.is_empty() {
            items.push(crate::outline::Item { depth: 0, at: usize::MAX, title: title(&h.direction).into() });
        }
        for (i, (v, depth, expanded)) in h.nodes.iter().enumerate() {
            let file = v["uri"].as_str().and_then(lsp::path).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
            let line = v.pointer("/selectionRange/start/line").and_then(Value::as_u64).unwrap_or(0) + 1;
            let mark = if *expanded { "▾" } else if h.expanding == Some(i) { "…" } else { "▸" };
            items.push(crate::outline::Item { depth: *depth, at: i, title: format!("{mark} {}  {file}:{line}", v["name"].as_str().unwrap_or("Symbol")) });
        }
        let empty = if h.direction.is_empty() { Some("Run Code: Incoming calls or Code: Supertypes at a symbol") }
            else if h.waiting { Some("Asking the language server…") }
            else if h.nodes.is_empty() { Some("No symbol at the cursor") } else { None };
        if let Some(text) = empty { items.push(crate::outline::Item { depth: 0, at: usize::MAX, title: text.into() }); }
        self.outline.items = items;
    }
    /// A click jumps to the item and toggles its children.
    pub(in crate::app) fn hierarchy_pick(&mut self, index: usize) {
        let Some((item, depth, expanded)) = self.languages.hierarchy.nodes.get(index).cloned() else { return };
        if let Some(path) = item["uri"].as_str().and_then(lsp::path) {
            if self.doc.path.as_ref() != Some(&path) { self.jump_to(path.clone(), 0, 0); }
            if self.doc.path.as_ref() == Some(&path)
                && let Some(at) = lsp::offset(&self.doc.buf.text, &item["selectionRange"]["start"])
            {
                self.doc.select(at, at);
                self.doc.moved();
            }
        }
        if expanded {
            let nodes = &mut self.languages.hierarchy.nodes;
            let end = nodes[index + 1..].iter().position(|(_, d, _)| *d <= depth).map_or(nodes.len(), |n| index + 1 + n);
            nodes.drain(index + 1..end);
            nodes[index].2 = false;
            return;
        }
        self.hierarchy_expand(index);
    }
    /// Discarded or timed-out replies must not leave a row loading forever.
    pub(in crate::app) fn hierarchy_settle(&mut self) {
        self.languages.hierarchy.expanding = None;
        self.languages.hierarchy.waiting = false;
    }
    fn hierarchy_expand(&mut self, index: usize) {
        let Some((item, ..)) = self.languages.hierarchy.nodes.get(index).cloned() else { return };
        if self.languages.hierarchy.expanding.is_some() { return; }
        self.language_sync();
        let method = match self.languages.hierarchy.direction.as_str() {
            "incoming" => "callHierarchy/incomingCalls",
            "outgoing" => "callHierarchy/outgoingCalls",
            "supertypes" => "typeHierarchy/supertypes",
            _ => "typeHierarchy/subtypes",
        };
        self.languages.hierarchy.expanding = Some(index);
        self.language_request("hierarchy_children", method, json!({"item":item}));
    }
}
