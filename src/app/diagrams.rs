//! Diagram preparation is lazy and background work; source stays editable.
use super::*;
use std::sync::Mutex;
type Done = (u64, Vec<(usize, PathBuf)>, Vec<String>);
#[derive(Default)]
pub(super) struct Diagrams { version: Option<u64>, pub(super) pending: Option<Arc<Mutex<Option<Done>>>> }
impl App {
    pub(super) fn diagrams_tick(&mut self, force: bool) {
        if self.doc.doc != Doc::Markdown || self.reader.is_some() { self.doc.diagrams.clear(); return; }
        let version = self.doc.buf.version;
        if self.diagrams.pending.is_some() || (!force && self.diagrams.version == Some(version)) { return; }
        self.diagrams.version = Some(version);
        let fences = crate::diagrams::fences(&self.doc.buf.text);
        self.doc.diagrams.clear();
        if fences.is_empty() { return; }
        let (cache, wake) = (config::cache_dir(), self.waker.clone());
        let done = Arc::new(Mutex::new(None)); self.diagrams.pending = Some(done.clone());
        std::thread::spawn(move || {
            let (mut images, mut errors) = (Vec::new(), Vec::new());
            for diagram in fences {
                match crate::diagrams::render(&diagram, &cache, None) {
                    Ok(path) => images.push((diagram.line, path)), Err(e) => errors.push(e),
                }
            }
            *done.lock().unwrap() = Some((version, images, errors)); wake();
        });
    }
    pub(super) fn diagrams_poll(&mut self) {
        let Some(pending) = &self.diagrams.pending else { return };
        let result = pending.lock().unwrap().take();
        let Some((version, images, errors)) = result else { return };
        self.diagrams.pending = None;
        if self.doc.buf.version != version { self.diagrams_tick(false); return; }
        self.message = Some(errors.first().cloned().unwrap_or_else(|| format!("{} diagram(s) rendered", images.len())));
        self.doc.diagrams = images; self.redraw = true;
    }
}
