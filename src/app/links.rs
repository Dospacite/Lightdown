//! Local link validation runs after input, never during layout or the first frame.
use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type Broken = Vec<(usize, usize, String)>;
type Checked = (Option<PathBuf>, u64, Broken);
#[derive(Default)]
pub(super) struct Links {
    pub(super) pending: Option<Arc<Mutex<Option<Checked>>>>,
    key: Option<(Option<PathBuf>, u64)>,
    at: Option<Instant>,
    report: bool,
}
pub(crate) fn slug(title: &str) -> String {
    title.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
        .chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_')).collect()
}
fn anchors(text: &str) -> HashSet<String> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut found = HashSet::new();
    let (mut title, mut heading) = (String::new(), false);
    let mut counts = HashMap::new();
    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::Heading { .. }) => { heading = true; title.clear(); }
            Event::Text(s) | Event::Code(s) if heading => title += &s,
            Event::End(TagEnd::Heading(_)) => {
                let base = slug(&title);
                let n = counts.entry(base.clone()).or_insert(0);
                found.insert(if *n == 0 { base } else { format!("{base}-{n}") }); *n += 1; heading = false;
            }
            _ => {}
        }
    }
    found
}
fn check(root: &Path, document: Option<&Path>, text: &str, open: &[(PathBuf, String)]) -> Broken {
    let buf = editor::Buffer::new(text.into());
    let md = crate::md::parse(text, &buf.starts);
    let mut broken = Vec::new();
    let mut targets = HashMap::new();
    let mut remaining = 64 << 20;
    let paths: Vec<_> = open.iter().map(|(p, _)| p.clone()).collect();
    for link in &md.links {
        if link.url.contains(':') || link.url.starts_with("//") { continue; }
        let wiki = text[link.start as usize..].starts_with("[[");
        let (file, fragment) = link.url.split_once('#').map_or((link.url.as_str(), None), |(a, b)| (a, Some(pane::percent_decode(b))));
        let path = if wiki {
            match super::notes::resolve_note(root, document, file, &paths) { Ok(p) => Some(p), Err(_) => None }
        } else if file.is_empty() { document.map(Path::to_path_buf) }
        else { Some(pane::resolve(document, &pane::percent_decode(file.split('?').next().unwrap_or(file)))) };
        let current = file.is_empty();
        let exists = current || path.as_ref().is_some_and(|p| p.is_file() || open.iter().any(|(a, _)| a == p));
        let valid = if !exists { false } else if let Some(fragment) = fragment.filter(|s| !s.is_empty()) {
            let source = if current { Some(text.to_string()) } else if let Some(path) = &path {
                targets.entry(path.clone()).or_insert_with(|| {
                    if let Some((_, text)) = open.iter().find(|(p, _)| p == path) { return Some(text.clone()); }
                    let size = std::fs::metadata(path).ok()?.len() as usize;
                    if size > 2 << 20 || size > remaining { return None; }
                    remaining -= size; std::fs::read_to_string(path).ok()
                }).clone()
            } else { None };
            source.is_some_and(|s| {
                if let Some(line) = fragment.strip_prefix('L').and_then(|s| s.split('-').next()).and_then(|s| s.parse::<usize>().ok()) {
                    line > 0 && line <= s.lines().count()
                } else { anchors(&s).contains(&fragment) }
            })
        } else { true };
        if !valid { broken.push((link.start as usize, link.end as usize, format!("Broken link: {}", link.url))); }
    }
    broken
}
impl App {
    pub(super) fn links_tick(&mut self, force: bool) {
        if self.doc.doc != Doc::Markdown || self.reader.is_some() { self.doc.broken_links.clear(); return; }
        let key = (self.doc.path.clone(), self.doc.buf.version);
        if self.links.pending.is_some() || (!force && self.links.key.as_ref() == Some(&key) && self.links.at.is_some_and(|t| t.elapsed() < Duration::from_secs(3))) { return; }
        self.links.key = Some(key.clone()); self.links.at = Some(Instant::now()); self.links.report |= force;
        self.doc.broken_links.clear();
        let (root, text, waker) = (self.root.clone(), self.doc.buf.text.clone(), self.waker.clone());
        let open: Vec<_> = self.file_tabs.iter().enumerate().filter_map(|(i, t)| {
            let p = if i == self.active_file { &self.doc } else { &t.state.as_ref()?.pane };
            if p.buf.text.len() > 2 << 20 { return None; }
            Some((p.path.clone()?, p.buf.text.clone()))
        }).collect();
        let done = Arc::new(Mutex::new(None)); self.links.pending = Some(done.clone());
        std::thread::spawn(move || {
            let broken = check(&root, key.0.as_deref(), &text, &open);
            *done.lock().unwrap() = Some((key.0, key.1, broken)); waker();
        });
    }
    pub(super) fn links_poll(&mut self) {
        let Some(pending) = &self.links.pending else { return };
        let result = pending.lock().unwrap().take();
        let Some((path, version, broken)) = result else { return };
        self.links.pending = None;
        if self.doc.path == path && self.doc.buf.version == version {
            if std::mem::take(&mut self.links.report) {
                self.message = Some(broken.first().map_or("All local links resolve".into(), |(_, _, m)| format!("{} broken link(s). {m}", broken.len())));
            }
            self.doc.broken_links = broken;
            self.redraw = true;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_images_and_anchors_use_unsaved_targets() {
        let dir = std::env::temp_dir().join(format!("lightdown-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("photo.png"), b"data").unwrap();
        let source = "# A *title*\n\n[ok](#a-title) [bad](#no) [target](other.md#new) ![](photo.png) ![](lost.png) [web](https://invalid.example)\n";
        let open = vec![(dir.join("other.md"), "# New\n".into())];
        let broken = check(&dir, Some(&dir.join("note.md")), source, &open);
        assert_eq!(broken.len(), 2);
        assert!(broken[0].2.contains("#no")); assert!(broken[1].2.contains("lost.png"));
        assert!(anchors("# Same\n# Same\n").contains("same-1"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
