//! Citation lookup stays dormant until completion, hover or a PDF build.
use super::*;
use std::time::{Duration, Instant};
#[derive(Default)]
pub(super) struct Citations { root: PathBuf, at: Option<Instant>, entries: Vec<crate::citations::Entry> }
impl App {
    fn citation_entries(&mut self) {
        if self.citations.root == self.root && self.citations.at.is_some_and(|t| t.elapsed() < Duration::from_secs(20)) { return; }
        self.citations.entries = crate::citations::read(&self.root); self.citations.root = self.root.clone(); self.citations.at = Some(Instant::now());
    }
    pub(super) fn complete_citation(&mut self) -> bool {
        if !matches!(self.doc.doc, Doc::Markdown | Doc::Latex) { return false; }
        let Some((start, prefix)) = crate::citations::query(&self.doc.buf.text, self.doc.buf.cur) else { return false };
        let (start, prefix) = (start, prefix.to_string()); self.citation_entries();
        let items = self.citations.entries.iter().filter(|e| e.key.starts_with(&prefix)).take(60).map(|e| Candidate { name: e.key.clone(), kind: "citation".into() }).collect();
        self.comp = Some(Completion { query: py::Query { start, prefix, base: None }, items, sel: 0, top: 0, seq: 0 }); true
    }
    pub(super) fn citation_info(&mut self, at: usize) -> Option<crate::kernel::Info> {
        if !matches!(self.doc.doc, Doc::Markdown | Doc::Latex) { return None; }
        let (_, _, key) = crate::citations::at(&self.doc.buf.text, at)?; self.citation_entries();
        let entry = self.citations.entries.iter().find(|e| e.key == key)?;
        Some(crate::kernel::Info { name: key.clone(), kind: "citation".into(), signature: key, doc: entry.preview(), ..Default::default() })
    }
    pub(super) fn refresh_citations(&mut self) {
        self.citations.at = None; self.citation_entries(); self.message = Some(format!("{} bibliography entries", self.citations.entries.len()));
    }
}
