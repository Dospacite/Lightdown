//! Automatic saves of the active tab, on focus loss or a second after
//! editing stops. They reuse the manual save's cleanup, formatter and
//! checks, but never ask anything: a file changed on disk is left alone.
use super::*;
impl App {
    pub fn focus_lost(&mut self) {
        if self.cfg.auto_save_for(&self.root) == "on_focus_loss" {
            self.autosave_attempt();
        }
    }
    pub(super) fn autosave_poll(&mut self) {
        if self.autosave_due.is_some_and(|due| due <= std::time::Instant::now()) {
            self.autosave_due = None;
            if self.cfg.auto_save_for(&self.root) == "after_delay" {
                self.autosave_attempt();
            }
        }
    }
    fn autosave_attempt(&mut self) {
        let conflict = self.file_tabs.get(self.active_file).is_some_and(|t| t.disk.conflict);
        if self.wait_mode
            || self.prompt.is_some()
            || conflict
            || self.doc.path.is_none()
            || !self.doc.buf.modified
            || self.doc.read_only
            || self.nb.as_ref().is_some_and(|nb| nb.busy())
        {
            return;
        }
        // A file changed on disk is reported, not saved over and not asked
        // about; the usual check offers to reload when the user saves.
        if !self.autosave_disk_unchanged() {
            self.message = Some("Auto-save skipped: the file changed on disk".into());
            return;
        }
        self.save();
        // Cleanup is itself an edit; a failure waits for the next edit or
        // focus loss rather than retrying.
        self.autosave_due = None;
    }

    /// Whether the file on disk is still what was last loaded or saved.
    fn autosave_disk_unchanged(&self) -> bool {
        let Some(path) = self.doc.path.as_deref() else { return false };
        let known = self.file_tabs.get(self.active_file).and_then(|t| t.disk.bytes.as_deref());
        match std::fs::read(path) {
            Ok(bytes) => known == Some(bytes.as_slice()),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound && known.is_none(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delay_saves_once_and_keeps_external_changes() {
        let dir = std::env::temp_dir().join(format!("lightdown-autosave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.txt");
        std::fs::write(&path, "before\n").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.cfg.auto_save = "after_delay".into();
        a.paste_text("edit");
        assert!(a.autosave_due.is_some(), "an edit schedules a save");
        a.autosave_due = Some(std::time::Instant::now());
        a.autosave_poll();
        assert!(!a.doc.buf.modified);
        assert!(a.autosave_due.is_none());
        a.paste_text("again");
        std::fs::write(&path, "external\n").unwrap();
        a.autosave_due = Some(std::time::Instant::now());
        a.autosave_poll();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
        assert!(a.autosave_due.is_none());
        // No dialog pops up; the status bar says why.
        assert!(a.prompt.is_none());
        assert!(a.message.as_deref().unwrap().contains("changed on disk"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn untitled_and_wait_windows_are_never_saved() {
        let mut a = App::new(None);
        a.cfg.auto_save = "on_focus_loss".into();
        a.paste_text("text");
        a.focus_lost();
        assert!(a.prompt.is_none());
        assert!(a.doc.buf.modified);
        a.wait_mode = true;
        a.focus_lost();
        assert!(a.doc.buf.modified);
    }
    #[test]
    fn a_tab_with_a_conflict_is_skipped() {
        let dir = std::env::temp_dir().join(format!("lightdown-autosave-conflict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.txt");
        std::fs::write(&path, "before\n").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.cfg.auto_save = "on_focus_loss".into();
        a.paste_text("edit");
        a.file_tabs[a.active_file].disk.conflict = true;
        a.focus_lost();
        assert!(a.doc.buf.modified);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
