//! Private, atomic checkpoints. A lock per window prevents recovering a live
//! window's buffers, including when several windows close or launch together.
use super::*;
use serde_json::{Value, json};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub(super) struct Recovery {
    own: Option<(PathBuf, File)>,
    claimed: Vec<(PathBuf, File)>,
    checked: Option<Instant>,
    signature: Vec<(u64, u64, usize, usize, bool)>,
    enabled: bool,
}
impl Recovery {
    pub(super) fn new() -> Self {
        let mut recovery = Self::default();
        recovery.checked = Some(Instant::now());
        recovery
    }
}
impl Drop for Recovery {
    fn drop(&mut self) {
        // A subprocess may briefly inherit a descriptor between fork and exec.
        // Unlock explicitly so it cannot keep a closed window's lease alive.
        for (_, file) in self.own.iter().chain(self.claimed.iter()) {
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN); }
        }
    }
}
fn lock(path: &Path) -> std::io::Result<File> {
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600)
        .open(path.with_extension("lock"))?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(f)
}
impl App {
    pub(super) fn recovery_poll(&mut self) {
        if self.wait_mode { return; }
        if cfg!(test) && !self.recovery.enabled { return; }
        if self.recovery.checked.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) { return; }
        self.recovery.checked = Some(Instant::now());
        if self.recovery.own.is_none() {
            self.recovery_restore();
        }
        if let Err(e) = self.recovery_save() {
            self.message = Some(format!("Recovery checkpoint failed: {e}"));
        }
    }

    fn recovery_restore(&mut self) {
        let dir = config::data_dir().join("recovery");
        let entries = std::fs::read_dir(&dir).into_iter().flatten().flatten();
        let mut count = 0;
        for entry in entries {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") { continue; }
            let Ok(guard) = lock(&path) else { continue };
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            if v["version"] != 1 { continue; }
            for tab in v["tabs"].as_array().into_iter().flatten() {
                let Some(buf) = editor::Buffer::recover(&tab["buffer"]) else { continue };
                // Preserve a requested file or folder's clean tabs alongside recovered work.
                self.file_tabs[self.active_file].pinned = self.doc.path.is_some() || self.doc.buf.modified;
                self.open_output(String::new());
                self.doc.read_only = false;
                self.doc.output_name = None;
                self.doc.path = tab["path"].as_str().map(PathBuf::from);
                self.root = tab["root"].as_str().map(PathBuf::from).unwrap_or_else(|| self.root.clone());
                if let Err(e) = self.cfg.ensure_project(&self.root) { self.message = Some(e); }
                let kind = doc_for(self.doc.path.as_deref());
                if let Some(text) = tab["notebook"].as_str() {
                    if let Ok((mut nb, source, raw)) = Notebook::open(text, self.pics.clone()) {
                        self.doc.set_text(source, Doc::Cells);
                        nb.attach(&mut self.doc, raw);
                        self.doc.headed = true;
                        self.nb = Some(nb);
                    } else { continue; }
                } else {
                    self.doc.set_text(buf.text.clone(), kind);
                }
                // Cell markers must match the parsed notebook before restoring its history.
                if self.nb.is_some() && self.doc.buf.text != buf.text { continue; }
                self.doc.buf = buf;
                self.bookmarks_load();
                if tab["buffer"]["eol"].is_null() && tab["crlf"].as_bool() == Some(true) {
                    self.doc.buf.eol = editor::Eol::Crlf;
                }
                self.doc.moved();
                let scroll = &tab["scroll"];
                self.doc.restore_scroll((scroll[0].as_u64().unwrap_or(0) as usize,
                    scroll[1].as_f64().unwrap_or(0.0).max(0.0) as f32));
                self.disk_remember();
                self.file_tabs[self.active_file].disk.recover_baseline(&tab["disk"]);
                count += 1;
            }
            self.recovery.claimed.push((path, guard));
        }
        if count > 0 {
            self.message = Some(format!("Recovered {count} unsaved tab(s)"));
            self.arrange();
            self.retitle = true;
            self.redraw = true;
        }
    }

    pub(super) fn recovery_save(&mut self) -> std::io::Result<()> {
        if self.wait_mode { return Ok(()); }
        if cfg!(test) && !self.recovery.enabled { return Ok(()); }
        if self.doc.ghost().is_some() || self.inline_preview.is_some() { return Ok(()); }
        let signature: Vec<_> = self.file_tabs.iter().enumerate().filter_map(|(i, t)| {
            let p = if i == self.active_file { &self.doc } else { &t.state.as_ref()?.pane };
            (!p.read_only && p.multi.is_none() && p.buf.modified).then_some((t.id, p.buf.version, p.buf.cur, p.buf.anchor, p.buf.modified))
        }).collect();
        if signature == self.recovery.signature && self.recovery.own.is_some() && self.recovery.claimed.is_empty() {
            return Ok(());
        }
        let mut tabs = Vec::new();
        for (i, tab) in self.file_tabs.iter().enumerate() {
            let (pane, nb, root) = if i == self.active_file {
                (&self.doc, &self.nb, &self.root)
            } else {
                let s = tab.state.as_ref().unwrap();
                (&s.pane, &s.notebook, &s.root)
            };
            if pane.read_only || pane.multi.is_some() || !pane.buf.modified { continue; }
            tabs.push(json!({"path": pane.path, "root": root, "crlf": pane.buf.eol == editor::Eol::Crlf,
                "buffer": pane.buf.recovery(), "scroll": pane.scroll_position(),
                "notebook": nb.as_ref().map(|nb| nb.to_json(pane)), "disk": tab.disk.baseline()}));
        }
        if self.recovery.own.is_none() {
            let dir = config::data_dir().join("recovery");
            std::fs::create_dir_all(&dir)?;
            let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path = dir.join(format!("{}-{nonce}.json", std::process::id()));
            let guard = lock(&path)?;
            self.recovery.own = Some((path, guard));
        }
        if signature != self.recovery.signature || !self.recovery.claimed.is_empty() {
            let (path, _) = self.recovery.own.as_ref().unwrap();
            config::write_private(path, serde_json::to_string(&json!({"version": 1, "tabs": tabs}))?.as_bytes())?;
            self.recovery.signature = signature;
            // Only remove old checkpoints after the new owner has written them safely.
            for (path, _) in self.recovery.claimed.drain(..) {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(path.with_extension("lock"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoints_keep_undo_redo_untitled_and_live_window_locks() {
        let mut a = App::new(None);
        a.recovery.enabled = true;
        a.doc.insert("café", EditKind::Other);
        a.doc.insert(" two", EditKind::Other);
        a.doc.buf.undo();
        a.recovery_save().unwrap();
        let path = a.recovery.own.as_ref().unwrap().0.clone();
        assert!(lock(&path).is_err());
        drop(a);
        let mut b = App::new(None);
        b.recovery.enabled = true;
        b.recovery.checked = None;
        b.recovery_poll();
        assert_eq!(b.doc.buf.text, "café");
        assert!(b.doc.buf.modified);
        b.doc.buf.redo();
        assert_eq!(b.doc.buf.text, "café two");
        b.doc.buf.undo();
        b.doc.buf.undo();
        assert_eq!(b.doc.buf.text, "");
        assert!(!path.exists());
        if let Some((path, _)) = &b.recovery.own { let _ = std::fs::remove_file(path); }
    }
    #[test]
    fn corrupt_undo_offsets_are_rejected() {
        let mut b = editor::Buffer::new("hello".into());
        b.replace(0, 0, "x", EditKind::Other);
        let mut value = b.recovery();
        value["undo"][0]["pos"] = json!(99999);
        assert!(editor::Buffer::recover(&value).is_none());
    }
}
