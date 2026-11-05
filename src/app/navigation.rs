//! Locations before deliberate jumps, separate from ordinary cursor motion.
use super::*;

#[derive(Clone, Debug, PartialEq)]
struct Location {
    path: Option<PathBuf>,
    tab: u64,
    cur: usize,
    anchor: usize,
}
#[derive(Default)]
pub(super) struct History {
    back: Vec<Location>,
    forward: Vec<Location>,
    restoring: bool,
}
impl App {
    fn location(&self) -> Location {
        Location {
            path: self.doc.path.clone(),
            tab: self.file_tabs[self.active_file].id,
            cur: self.doc.buf.cur,
            anchor: self.doc.buf.anchor,
        }
    }
    pub(super) fn remember_jump(&mut self) {
        if self.history.restoring {
            return;
        }
        let at = self.location();
        if self.history.back.last() != Some(&at) {
            self.history.back.push(at);
            if self.history.back.len() > 200 {
                self.history.back.remove(0);
            }
        }
        self.history.forward.clear();
    }
    /// A file and position form one jump rather than two history entries.
    pub(super) fn jump_to(&mut self, path: PathBuf, at: usize, end: usize) {
        self.remember_jump();
        self.history.restoring = true;
        self.pick(path.clone(), false);
        if self.doc.path.as_deref() == Some(path.as_path()) && self.reader.is_none() {
            let a = editor::floor(&self.doc.buf.text, at);
            let b = editor::floor(&self.doc.buf.text, end);
            self.doc.select(a, b);
            self.doc.moved();
        }
        self.history.restoring = false;
    }
    pub(super) fn navigate(&mut self, forward: bool) {
        let target = if forward { self.history.forward.pop() } else { self.history.back.pop() };
        let Some(target) = target else {
            self.message = Some("No more locations".into());
            return;
        };
        let current = self.location();
        self.history.restoring = true;
        let existing = self.file_tabs.iter().position(|t| t.id == target.tab);
        if let Some(i) = existing {
            self.activate_file(i);
        } else if let Some(path) = target.path.as_ref().filter(|p| p.is_file()) {
            self.open_tab(path.clone());
        } else {
            self.history.restoring = false;
            self.message = Some("That location is no longer open or on disk".into());
            return;
        }
        if forward {
            self.history.back.push(current);
        } else {
            self.history.forward.push(current);
        }
        if self.reader.is_none() {
            self.go_to(target.cur);
            self.doc.buf.anchor = editor::floor(&self.doc.buf.text, target.anchor);
        }
        self.history.restoring = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_crosses_files_and_restores_unsaved_tabs() {
        let dir = std::env::temp_dir().join(format!("ld-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.py"), dir.join("b.py"));
        std::fs::write(&a, "alpha\nbeta\n").unwrap();
        std::fs::write(&b, "other\nlast\n").unwrap();
        let mut app = App::new(Some(a.clone()));
        app.doc.edit(0, 0, "local ", EditKind::Other);
        app.doc.select(2, 2);
        app.jump_to(b.clone(), 6, 10);
        app.navigate(false);
        assert_eq!(app.doc.path.as_ref(), Some(&a));
        assert_eq!(app.doc.buf.cur, 2);
        assert!(app.doc.buf.text.starts_with("local"));
        app.navigate(true);
        assert_eq!(app.doc.path.as_ref(), Some(&b));
        assert_eq!(app.doc.buf.sel(), (6, 10));
        app.go_to(0);
        app.navigate(false);
        assert_eq!(app.doc.buf.sel(), (6, 10));
        app.go_to(1);
        assert!(app.history.forward.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    #[test]
    fn back_reopens_a_clean_file_after_its_preview_tab_is_reused() {
        let dir = std::env::temp_dir().join(format!("ld-history-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.py"), dir.join("b.py"));
        std::fs::write(&a, "first").unwrap();
        std::fs::write(&b, "second").unwrap();
        let mut app = App::new(Some(a.clone()));
        app.go_to(3);
        app.open_tab(b.clone());
        assert_eq!(app.file_tabs.len(), 1);
        app.navigate(false);
        assert_eq!(app.doc.path.as_ref(), Some(&a));
        assert_eq!(app.doc.buf.cur, 3);
        app.navigate(true);
        assert_eq!(app.doc.path.as_ref(), Some(&b));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;
    #[test]
    fn local_links_and_find_results_can_return_to_the_origin() {
        let dir = std::env::temp_dir().join(format!("ld-history-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.md"), dir.join("b.md"));
        std::fs::write(&a, "first\nsecond\n").unwrap();
        std::fs::write(&b, "# Heading\nbody\n## Target title\n").unwrap();
        let mut app = App::new(Some(a.clone()));
        app.doc.select(2, 2);
        app.follow("b.md#target-title");
        assert_eq!(app.doc.path.as_ref(), Some(&b));
        assert_eq!(app.doc.buf.line_of(app.doc.buf.cur), 2);
        app.navigate(false);
        assert_eq!(app.doc.path.as_ref(), Some(&a));
        assert_eq!(app.doc.buf.cur, 2);
        app.ask(PromptKind::Find);
        let p = app.prompt.as_mut().unwrap();
        p.text = "second".into();
        app.prompt_changed();
        app.prompt = None;
        assert_eq!(app.doc.buf.line_of(app.doc.buf.cur), 1);
        app.navigate(false);
        assert_eq!(app.doc.buf.cur, 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
