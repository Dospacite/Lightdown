//! Compare saved bytes before reloading or writing, including inactive tabs.
use super::*;
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub(super) struct Disk {
    pub(super) bytes: Option<Vec<u8>>,
    stamp: Option<(SystemTime, u64)>,
    pub(super) conflict: bool,
    prompted: bool,
}
impl Disk {
    pub(super) fn baseline(&self) -> serde_json::Value {
        serde_json::json!(self.bytes.as_deref().and_then(|b| std::str::from_utf8(b).ok()))
    }
    pub(super) fn recover_baseline(&mut self, v: &serde_json::Value) {
        self.bytes = v.as_str().map(|s| s.as_bytes().to_vec());
        // Force the next comparison, even if the file changed while we were away.
        self.stamp = None;
    }
}
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = path.metadata().ok()?;
    Some((m.modified().ok()?, m.len()))
}
impl App {
    pub(super) fn disk_remember(&mut self) {
        self.disk_remember_tab(self.active_file);
    }
    pub(super) fn disk_remember_tab(&mut self, index: usize) {
        let path = if index == self.active_file {
            self.doc.path.as_deref().filter(|_| self.reader.is_none())
        } else {
            self.file_tabs[index].state.as_ref().and_then(|s| s.pane.path.as_deref().filter(|_| s.reader.is_none()))
        };
        self.file_tabs[index].disk = Disk {
            bytes: path.and_then(|p| std::fs::read(p).ok()),
            stamp: path.and_then(stamp),
            conflict: false,
            prompted: false,
        };
    }
    pub(super) fn disk_safe(&mut self) -> bool {
        let Some(path) = self.doc.path.as_deref() else { return true };
        match std::fs::read(path) {
            Ok(bytes) if self.file_tabs[self.active_file].disk.bytes.as_deref() == Some(&bytes) => true,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && self.file_tabs[self.active_file].disk.bytes.is_none() =>
            {
                true
            }
            _ => {
                self.file_tabs[self.active_file].disk.conflict = true;
                self.ask(PromptKind::Reload);
                false
            }
        }
    }
    pub(super) fn disk_poll(&mut self, force: bool) {
        if !force && self.disk_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.disk_checked = Instant::now();
        for i in 0..self.file_tabs.len() {
            let active = i == self.active_file;
            let tab = &mut self.file_tabs[i];
            let (pane, nb, reader) = if active {
                (&mut self.doc, &mut self.nb, &self.reader)
            } else {
                let s = tab.state.as_mut().unwrap();
                (&mut s.pane, &mut s.notebook, &s.reader)
            };
            if reader.is_some() {
                continue;
            }
            let Some(path) = pane.path.as_deref() else { continue };
            let current = stamp(path);
            let disk = &mut tab.disk;
            if current == disk.stamp && !force {
                continue;
            }
            disk.stamp = current;
            let bytes = match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => continue,
            };
            if bytes == disk.bytes {
                disk.conflict = false;
                continue;
            }
            if pane.buf.modified || nb.as_ref().is_some_and(Notebook::busy) || bytes.is_none() {
                disk.conflict = true;
                continue;
            }
            if let Some(bytes) = bytes {
                if reload(pane, nb, &bytes, &self.pics).is_ok() {
                    disk.bytes = Some(bytes);
                    disk.conflict = false;
                    if active {
                        self.comp = None;
                        self.docs = Default::default();
                        self.suggest = None;
                        self.words = None;
                    }
                    self.redraw = true;
                    self.retitle = true;
                } else {
                    disk.conflict = true;
                }
            }
        }
        if self.file_tabs[self.active_file].disk.conflict
            && !self.file_tabs[self.active_file].disk.prompted
            && !self.overlaid()
            && self.prompt.is_none()
        {
            self.file_tabs[self.active_file].disk.prompted = true;
            self.ask(PromptKind::Reload);
        }
    }
    pub(super) fn disk_decide(&mut self, reload_disk: bool) {
        if reload_disk {
            let result = self
                .doc
                .path
                .as_deref()
                .ok_or_else(|| "No file".to_string())
                .and_then(|p| std::fs::read(p).map_err(|e| e.to_string()))
                .and_then(|bytes| reload(&mut self.doc, &mut self.nb, &bytes, &self.pics));
            if let Err(e) = result {
                self.message = Some(format!("Cannot reload: {e}"));
                return;
            }
            self.comp = None;
            self.docs = Default::default();
            self.suggest = None;
            self.words = None;
            self.message = Some("Reloaded from disk".into());
        } else {
            // Acknowledge this disk version, but do not save until explicitly asked.
            self.doc.buf.modified = true;
            self.message = Some("Edits kept. Ctrl+S writes them to disk.".into());
        }
        self.disk_remember();
        self.prompt = None;
        self.pin_edited_tab();
        self.retitle = true;
        self.redraw = true;
    }
}
fn reload(
    pane: &mut Pane,
    nb: &mut Option<Notebook>,
    bytes: &[u8],
    pics: &Rc<RefCell<Pics>>,
) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut eol = editor::Eol::detect(text);
    if eol == editor::Eol::Lf && pane.path.as_deref().is_some_and(|p| super::saving::Rules::read(p).cr()) {
        eol = editor::Eol::Cr;
    }
    let text = if eol == editor::Eol::Cr {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.replace("\r\n", "\n")
    };
    let (cur, anchor) = (pane.buf.cur, pane.buf.anchor);
    let kind = pane.doc;
    let scroll = pane.scroll_position();
    let bookmarks = std::mem::take(&mut pane.bookmarks);
    if nb.is_some() {
        let (mut opened, source, raw) = Notebook::open(&text, pics.clone())?;
        pane.set_text(source, Doc::Cells);
        opened.attach(pane, raw);
        *nb = Some(opened);
    } else {
        pane.set_text(text.replace(cells::MARK, ""), kind);
    }
    pane.buf.eol = eol;
    pane.keep_bookmarks(bookmarks);
    pane.select(editor::floor(&pane.buf.text, anchor), editor::floor(&pane.buf.text, cur));
    pane.restore_scroll(scroll);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clean_and_background_tabs_reload_but_dirty_tabs_require_a_choice() {
        let dir = std::env::temp_dir().join(format!("ld-disk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.py"), dir.join("b.py"));
        std::fs::write(&a, "first\n").unwrap();
        std::fs::write(&b, "second\n").unwrap();
        let mut app = App::new(Some(a.clone()));
        app.file_tabs[0].pinned = true;
        app.open_tab(b.clone());
        std::fs::write(&a, "outside a\n").unwrap();
        std::fs::write(&b, "outside b\n").unwrap();
        app.disk_poll(true);
        assert_eq!(app.doc.buf.text, "outside b\n");
        app.activate_file(0);
        assert_eq!(app.doc.buf.text, "outside a\n");
        app.doc.edit(0, 0, "local ", EditKind::Other);
        std::fs::write(&a, "new disk\n").unwrap();
        app.save();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "new disk\n");
        assert!(app.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::Reload));
        app.disk_decide(false);
        assert_eq!(app.doc.buf.text, "local outside a\n");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "new disk\n");
        app.save();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "local outside a\n");
        std::fs::write(&a, "another\n").unwrap();
        app.doc.edit(0, 0, "edit ", EditKind::Other);
        app.disk_poll(true);
        app.disk_decide(true);
        assert_eq!(app.doc.buf.text, "another\n");
        assert!(!app.doc.buf.modified);
        std::fs::remove_file(&a).unwrap();
        app.disk_poll(true);
        app.save();
        assert!(!a.exists());
        app.disk_decide(false);
        app.save();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "another\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn notebooks_reload_from_json_and_preserve_cells() {
        let dir = std::env::temp_dir().join(format!("ld-disk-nb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.ipynb");
        let json = |text| {
            serde_json::json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[{"cell_type":"code","source":[text],"metadata":{},"execution_count":null,"outputs":[]}]}).to_string()
        };
        std::fs::write(&path, json("one")).unwrap();
        let mut app = App::new(Some(path.clone()));
        std::fs::write(&path, json("two")).unwrap();
        app.disk_poll(true);
        assert!(app.nb.is_some() && app.doc.buf.text.contains("two"));
        assert_eq!(app.doc.cells.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod dialog_tests {
    use super::*;
    #[test]
    fn reload_dialog_fits_both_themes_and_discards_a_stale_ghost() {
        let dir = std::env::temp_dir().join(format!("ld-disk-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.py");
        std::fs::write(&path, "long enough original\n").unwrap();
        let mut app = App::new(Some(path.clone()));
        app.doc.select(10, 10);
        app.doc.show_ghost("suggestion");
        std::fs::write(&path, "x\n").unwrap();
        app.disk_poll(true);
        app.doc.key(Key::Char('z'), Mods::default(), Some("z"));
        assert_eq!(app.doc.buf.text, "x\nz");
        for light in [false, true] {
            theme::set_light(light);
            for w in [360, 1000] {
                app.resize(w, 700, 1.0);
                app.ask(PromptKind::Reload);
                let mut pixels = vec![0; w as usize * 700];
                app.draw(&mut Canvas::new(&mut pixels, w as i32, 700));
                for (r, _) in app.hits.iter().filter(|(_, h)| matches!(h, Hit::Dialog(_))) {
                    assert!(r.0 >= 0.0 && r.0 + r.2 <= w as f32);
                }
                assert!(app.hits.iter().any(|(_, h)| *h == Hit::Dialog(dialog::DialogHit::Alternate)));
            }
        }
        theme::set_light(false);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
