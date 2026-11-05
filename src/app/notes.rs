//! Project note links and an on-demand backlinks index.
use super::*;
use crate::tools;
use std::sync::Mutex;

type Backlink = (PathBuf, usize, String);
type Backlinks = (PathBuf, Vec<Backlink>);
#[derive(Default)]
pub(super) struct Notes {
    pub(super) pending: Option<Arc<Mutex<Option<Backlinks>>>>,
    links: Vec<Backlink>,
}
fn note(path: &Path) -> bool { matches!(ext(path).as_str(), "md" | "markdown") }

/// A note's explicit path wins; a bare name then finds a unique project stem.
pub(super) fn resolve_note(root: &Path, doc: Option<&Path>, name: &str, paths: &[PathBuf]) -> Result<PathBuf, String> {
    let name = name.split('#').next().unwrap_or(name);
    if name.is_empty() { return doc.map(Path::to_path_buf).ok_or("Save this note first".into()); }
    let name = pane::percent_decode(name);
    let relative = Path::new(&name);
    if relative.is_absolute() || relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("Note links must stay within the project".into());
    }
    let mut path = root.join(relative);
    if path.extension().is_none() { path.set_extension("md"); }
    let beside = doc.and_then(Path::parent).unwrap_or(root).join(relative);
    let beside = if beside.extension().is_none() { beside.with_extension("md") } else { beside };
    if beside.is_file() { return Ok(beside); }
    if path.is_file() { return Ok(path); }
    if !name.contains('/') {
        let matches: Vec<_> = paths.iter().filter(|p| note(p) && p.file_stem() == relative.file_stem()).collect();
        if matches.len() > 1 { return Err("Ambiguous note name; include its project folder".into()); }
        if let Some(found) = matches.first() { return Ok((*found).clone()); }
    }
    if path.strip_prefix(root).is_err() { return Err("Note link leaves the project".into()); }
    Ok(path)
}
fn wiki_query(text: &str, cur: usize) -> Option<(usize, &str)> {
    let start = text[..cur].rfind("[[")? + 2;
    let prefix = &text[start..cur];
    (!prefix.contains([']', '[', '\n', '|', '#'])).then_some((start, prefix))
}
fn same_file(a: &Path, b: &Path) -> bool {
    a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(a, b)| a == b)
}
impl App {
    pub(super) fn complete_note(&mut self) -> bool {
        if self.doc.doc != Doc::Markdown { return false; }
        let cur = self.doc.buf.cur;
        let Some((start, prefix)) = wiki_query(&self.doc.buf.text, cur) else { return false };
        let (start, prefix) = (start, prefix.to_string());
        self.index_files(false);
        let close = if self.doc.buf.text[cur..].starts_with("]]" ) { "" } else { "]]" };
        let mut names: Vec<_> = self.file_index.paths.iter().filter(|p| note(p)).filter_map(|path| {
            let mut relative = path.strip_prefix(&self.root).ok()?.to_path_buf();
            relative.set_extension("");
            let name = relative.to_string_lossy();
            (name.to_lowercase().contains(&prefix.to_lowercase())).then(|| Candidate { name: format!("{name}{close}"), kind: "note".into() })
        }).collect();
        names.sort_by(|a, b| a.name.cmp(&b.name)); names.truncate(60);
        self.comp = Some(Completion { query: py::Query { start, prefix, base: None }, items: names, sel: 0, top: 0, seq: 0 });
        true
    }
    pub(super) fn follow_note(&mut self, url: &str) {
        let (name, anchor) = url.split_once('#').map_or((url, None), |(a, b)| (a, Some(b)));
        let paths = search::files(&self.root);
        let result = resolve_note(&self.root, self.doc.path.as_deref(), name, &paths).and_then(|path| {
            if !path.exists() {
                // Canonicalize the nearest existing ancestor before creating folders.
                let mut ancestor = path.parent().ok_or("Invalid note path")?;
                while !ancestor.exists() { ancestor = ancestor.parent().ok_or("Invalid note folder")?; }
                let real = std::fs::canonicalize(ancestor).map_err(|e| e.to_string())?;
                let root = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
                if !real.starts_with(root) { return Err("Note link leaves the project through a symlink".into()); }
                std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
            }
            let real = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
            let root = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
            if !real.starts_with(root) { return Err("Note link leaves the project through a symlink".into()); }
            Ok(path)
        });
        match result {
            Ok(path) => {
                self.jump_to(path.clone(), 0, 0);
                if let Some(anchor) = anchor { self.follow(&format!("#{}", anchor)); }
                self.index_files(true);
            }
            Err(e) => self.message = Some(e),
        }
    }
    pub(super) fn open_backlinks(&mut self) {
        self.show_side(Some(Tab::Backlinks));
        self.notes.links.clear();
        self.outline.current = None;
        self.outline.items = vec![crate::outline::Item { depth: 0, title: "Scanning backlinks…".into(), at: usize::MAX }];
        let Some(target) = self.doc.path.clone() else {
            self.outline.items[0].title = "Save this note to find backlinks".into(); return;
        };
        let (root, waker) = (self.root.clone(), self.waker.clone());
        let open: Vec<_> = self.file_tabs.iter().enumerate().filter_map(|(i, t)| {
            let pane = if i == self.active_file { &self.doc } else { &t.state.as_ref()?.pane };
            Some((pane.path.clone()?, pane.buf.text.clone()))
        }).collect();
        let done = Arc::new(Mutex::new(None)); self.notes.pending = Some(done.clone());
        std::thread::spawn(move || {
            let mut paths = search::files(&root);
            for (p, _) in &open { if !paths.contains(p) { paths.push(p.clone()); } }
            let mut found = Vec::new();
            let mut remaining = 64 << 20;
            for path in paths.iter().filter(|p| note(p) && !same_file(p, &target)) {
                let source = open.iter().find(|(p, _)| p == path).map(|(_, s)| s.clone()).or_else(|| {
                    let size = std::fs::metadata(path).ok()?.len() as usize;
                    if size > 2 << 20 || size > remaining { return None; }
                    remaining -= size; std::fs::read_to_string(path).ok()
                });
                let Some(source) = source else { continue };
                let b = editor::Buffer::new(source);
                let md = crate::md::parse(&b.text, &b.starts);
                for link in md.links.iter().filter(|l| b.text[l.start as usize..].starts_with("[[")) {
                    if resolve_note(&root, Some(path), &link.url, &paths).is_ok_and(|p| same_file(&p, &target)) {
                        let at = link.start as usize;
                        let line = b.line_of(at);
                        let (a, z) = b.line(line);
                        found.push((path.clone(), at, format!("{}:{}  {}", tools::short(&root, path), line + 1, b.text[a..z].trim())));
                    }
                }
            }
            *done.lock().unwrap() = Some((target, found)); waker();
        });
    }
    pub(super) fn backlinks_poll(&mut self) {
        let Some(pending) = &self.notes.pending else { return };
        let result = pending.lock().unwrap().take();
        let Some((target, found)) = result else { return };
        self.notes.pending = None;
        if self.tab != Tab::Backlinks || self.doc.path.as_ref() != Some(&target) { return; }
        self.notes.links = found;
        self.outline.items = self.notes.links.iter().enumerate().map(|(i, (_, _, title))| crate::outline::Item { depth: 0, title: title.clone(), at: i }).collect();
        if self.outline.items.is_empty() { self.outline.items.push(crate::outline::Item { depth: 0, title: "No backlinks".into(), at: usize::MAX }); }
        self.redraw = true;
    }
    pub(super) fn backlink_pick(&mut self, index: usize) {
        if let Some((path, at, _)) = self.notes.links.get(index).cloned() {
            self.jump_to(path, 0, 0); self.go_to(at); self.open_backlinks();
        }
    }
    pub(super) fn follow_note_at_cursor(&mut self) {
        let b = &self.doc.buf;
        let md = crate::md::parse(&b.text, &b.starts);
        if let Some(link) = md.link_at(b.cur).filter(|l| b.text[l.start as usize..].starts_with("[[")) {
            self.follow_note(&link.url);
        } else { self.message = Some("Place the cursor on a wikilink".into()); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follow_creates_notes_and_backlinks_include_unsaved_tabs() {
        let dir = std::env::temp_dir().join(format!("lightdown-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("source.md"), "[[target|label]]").unwrap();
        let mut a = App::new(Some(dir.join("source.md")));
        a.follow_note("target");
        assert!(dir.join("target.md").is_file());
        a.open_backlinks();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while a.notes.pending.is_some() && std::time::Instant::now() < until { a.backlinks_poll(); std::thread::yield_now(); }
        assert_eq!(a.notes.links.len(), 1);
        assert_eq!(a.notes.links[0].0, dir.join("source.md"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn project_notes_resolve_and_reject_ambiguous_or_escaping_names() {
        let root = Path::new("/project");
        let paths = vec![root.join("a/one.md"), root.join("b/two.md")];
        assert_eq!(resolve_note(root, None, "one", &paths).unwrap(), paths[0]);
        assert_eq!(resolve_note(root, None, "new/note", &paths).unwrap(), root.join("new/note.md"));
        let mut ambiguous = paths.clone(); ambiguous.push(root.join("b/one.md"));
        assert!(resolve_note(root, None, "one", &ambiguous).is_err());
        assert!(resolve_note(root, None, "../escape", &paths).is_err());
        assert_eq!(wiki_query("é [[no", 7), Some((5, "no")));
        assert!(wiki_query("[[note]] end", 12).is_none());
    }
}
