//! What a folder's window held when it was closed: its tabs, with their
//! cursors, and its terminals' folders, brought back when the folder is
//! opened again.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::App;
use crate::config;
use crate::term::Term;

/// Where the sessions of every folder are kept.
fn file() -> PathBuf {
    config::data_dir().join("sessions.json")
}

fn load_all() -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(file()).unwrap_or_default();
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => map,
        _ => Default::default(),
    }
}

impl App {
    /// Writes down what the window holds, if it has a folder open.
    pub(super) fn save_session(&self) {
        let Some(folder) = self.window.workspace.as_ref().or(self.folders.first()) else { return };
        let tabs: Vec<Value> = self
            .tab_files()
            .into_iter()
            .flatten()
            .map(|(path, cur)| json!({ "path": path.to_string_lossy(), "cursor": cur }))
            .collect();
        // The tab shown, counted among those with files.
        let files = self.tab_files();
        let active = files[..self.active_file.min(files.len())].iter().filter(|t| t.is_some()).count();
        let active = if files.get(self.active_file).is_some_and(Option::is_some) { active } else { 0 };
        let views = self.views_json();
        let terms: Vec<Value> = self.terms.iter().map(|t| json!(t.cwd().to_string_lossy())).collect();
        let mut all = load_all();
        let key = folder.to_string_lossy().into_owned();
        all.insert(
            key,
            json!({
                "tabs": tabs,
                "active": active,
                "views": views,
                "terminals": terms,
                "terminal": self.term_cur,
                "terminal_open": self.term_open,
                "folders": self.folders.iter().map(|f| f.to_string_lossy()).collect::<Vec<_>>(),
            }),
        );
        let _ = config::write_private(&file(), serde_json::to_string_pretty(&Value::Object(all)).unwrap().as_bytes());
    }

    /// Opens again what the folder's window last held.
    pub(super) fn restore_session(&mut self, folder: &Path) {
        let all = load_all();
        let key = self.window.workspace.as_deref().unwrap_or(folder);
        let Some(s) = all.get(key.to_string_lossy().as_ref()) else { return };
        // A folder opened alone brings back the folders it was left with.
        if self.folders.len() == 1 && self.window.workspace.is_none() {
            let saved = s["folders"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from);
            let more: Vec<PathBuf> = saved.filter(|d| d.is_dir() && !self.folders.contains(d)).collect();
            if !more.is_empty() {
                self.folders.extend(more);
                self.name_tree_roots();
                self.root = self.root_for(None);
            }
        }
        let mut opened = 0;
        for tab in s["tabs"].as_array().into_iter().flatten() {
            let Some(path) = tab["path"].as_str().map(PathBuf::from).filter(|p| p.is_file()) else { continue };
            self.open_tab(path);
            if self.doc.path.is_none() {
                continue;
            }
            let cur = tab["cursor"].as_u64().unwrap_or(0) as usize;
            let cur = crate::editor::floor(&self.doc.buf.text, cur.min(self.doc.buf.text.len()));
            (self.doc.buf.cur, self.doc.buf.anchor) = (cur, cur);
            self.doc.moved();
            // Each file keeps a tab of its own.
            self.file_tabs[self.active_file].pinned = true;
            opened += 1;
        }
        if opened > 0 {
            let active = s["active"].as_u64().unwrap_or(0) as usize;
            self.activate_file(active.min(self.file_tabs.len() - 1));
            self.focus = super::Focus::Doc;
        }
        self.views_restore(s);
        self.arrange();
        let dirs: Vec<PathBuf> =
            s["terminals"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).collect();
        for dir in dirs.iter().filter(|d| d.is_dir()) {
            self.terms.push(Term::new_in(dir, &self.root, self.fonts.clone(), self.waker.clone()));
        }
        if !self.terms.is_empty() {
            self.term_cur = (s["terminal"].as_u64().unwrap_or(0) as usize).min(self.terms.len() - 1);
            if s["terminal_open"].as_bool().unwrap_or(false) {
                let focus = self.focus;
                self.show_term(true);
                self.focus = focus;
            }
        }
        self.message = (opened > 0).then(|| "The folder's tabs were opened again".into());
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_opens_with_its_last_tabs() {
        let dir = std::env::temp_dir().join(format!("ld-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.md"), "first file\nwith two lines\n").unwrap();
        std::fs::write(dir.join("b.py"), "print('b')\n").unwrap();
        let dir = dir.canonicalize().unwrap();
        let mut a = App::new(Some(dir.clone()));
        a.resize(900, 700, 1.0);
        a.pick(dir.join("a.md"), false);
        a.doc.select(15, 15);
        a.file_tabs[a.active_file].pinned = true;
        a.pick(dir.join("b.py"), false);
        a.activate_file(0);
        a.terms.push(Term::new(&dir.join("sub"), a.fonts.clone(), a.waker.clone()));
        a.split_editor();
        a.save_session();

        let mut b = App::new(Some(dir.clone()));
        b.resize(900, 700, 1.0);
        assert_eq!(b.file_tabs.len(), 2);
        assert!(b.split_other().is_some());
        assert_eq!(b.doc.path.as_deref(), Some(dir.join("a.md").as_path()));
        assert_eq!(b.doc.buf.cur, 15);
        assert_eq!(b.terms.len(), 1);
        assert_eq!(b.terms[0].cwd(), dir.join("sub"));
        assert!(!b.term_open);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
