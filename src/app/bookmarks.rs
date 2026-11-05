//! Line bookmarks across the project: kept per project in the private
//! data folder, listed in a menu, and stepped through file by file.
use super::assist::MenuKind;
use super::*;
use crate::ui::{Entry, Menu};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Each file's bookmarked lines, counted from zero.
type Marks = BTreeMap<PathBuf, Vec<usize>>;

/// One bookmark as listed: its file, line and the line's text.
pub(super) struct Bookmark {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
}

fn store(root: &Path) -> PathBuf {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hash);
    config::data_dir().join("bookmarks").join(format!("{:016x}.json", hash.finish()))
}

fn read(root: &Path) -> Marks {
    std::fs::read(store(root)).ok().and_then(|s| serde_json::from_slice(&s).ok()).unwrap_or_default()
}

fn write(root: &Path, marks: &Marks) {
    if let Ok(bytes) = serde_json::to_vec(marks) {
        let _ = config::write_private(&store(root), &bytes);
    }
}

/// The text of each listed line, or an empty string past the end.
fn line_texts(text: &str, lines: &[usize]) -> Vec<String> {
    let all: Vec<&str> = text.split('\n').collect();
    lines.iter().map(|&l| all.get(l).map_or(String::new(), |s| s.trim().to_string())).collect()
}

impl App {
    /// Reads the opened file's bookmarks back.
    pub(super) fn bookmarks_load(&mut self) {
        let marks = self.doc.path.as_ref().and_then(|p| read(&self.root).remove(p)).unwrap_or_default();
        self.doc.keep_bookmarks(marks);
        self.doc.bookmarks_changed = false;
    }

    /// Stores the document's bookmarks if they changed.
    pub(super) fn bookmarks_sync(&mut self) {
        if !std::mem::take(&mut self.doc.bookmarks_changed) {
            return;
        }
        let Some(path) = self.doc.path.clone() else { return };
        let mut marks = read(&self.root);
        if self.doc.bookmarks.is_empty() {
            marks.remove(&path);
        } else {
            marks.insert(path, self.doc.bookmarks.clone());
        }
        write(&self.root, &marks);
    }

    /// The project's bookmarks in file and line order: the stored ones,
    /// with open files' as they stand, and the text of each line.
    pub(super) fn bookmarks_all(&self) -> Vec<Bookmark> {
        let mut marks = read(&self.root);
        let mut texts: BTreeMap<PathBuf, &str> = BTreeMap::new();
        let open = std::iter::once(&self.doc).chain(self.file_tabs.iter().filter_map(|t| t.state.as_ref().map(|s| &s.pane)));
        for pane in open {
            if let Some(path) = &pane.path {
                if pane.bookmarks.is_empty() {
                    marks.remove(path);
                } else {
                    marks.insert(path.clone(), pane.bookmarks.clone());
                }
                texts.insert(path.clone(), &pane.buf.text);
            }
        }
        let mut out = Vec::new();
        for (path, lines) in marks {
            let read_text;
            let text = match texts.get(&path) {
                Some(text) => *text,
                None => {
                    if std::fs::metadata(&path).map_or(true, |m| !m.is_file() || m.len() > 8 << 20) {
                        continue;
                    }
                    read_text = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
                    &read_text
                }
            };
            for (line, text) in lines.iter().zip(line_texts(text, &lines)) {
                out.push(Bookmark { path: path.clone(), line: *line, text });
            }
        }
        out
    }

    /// Opens a bookmark's file at the start of its line.
    pub(super) fn go_to_bookmark(&mut self, path: PathBuf, line: usize) {
        self.jump_to(path.clone(), 0, 0);
        if self.doc.path.as_deref() == Some(path.as_path()) && self.reader.is_none() {
            let at = self.doc.buf.starts[line.min(self.doc.buf.lines() - 1)];
            self.doc.select(at, at);
            self.doc.moved();
        }
    }

    /// The next bookmark in the project after the cursor, or the one
    /// before it, wrapping round from the last file to the first.
    fn bookmark_project_step(&mut self, back: bool) {
        let all = self.bookmarks_all();
        let here = (self.doc.path.clone().unwrap_or_default(), self.doc.buf.line_of(self.doc.buf.cur));
        let key = |b: &Bookmark| (b.path.clone(), b.line);
        let target = if back {
            all.iter().rev().find(|b| key(b) < here).or(all.last())
        } else {
            all.iter().find(|b| key(b) > here).or(all.first())
        };
        match target {
            Some(b) => {
                let (path, line) = (b.path.clone(), b.line);
                self.go_to_bookmark(path, line);
            }
            None => self.message = Some("No bookmarks in this project".into()),
        }
    }

    fn open_bookmarks(&mut self) {
        let all = self.bookmarks_all();
        if all.is_empty() {
            self.message = Some("No bookmarks in this project".into());
            return;
        }
        let entries = all
            .iter()
            .map(|b| {
                let place = format!("{}:{}", crate::tools::short(&self.root, &b.path), b.line + 1);
                let label = if b.text.is_empty() { "(blank line)" } else { &b.text };
                Entry::new(&format!("{}\n{}", b.line, b.path.display()), label, &place)
            })
            .collect();
        self.menu = Some((MenuKind::Bookmarks, Menu::new("Bookmarks", entries, "")));
        self.redraw = true;
    }

    pub(super) fn pick_bookmark(&mut self, key: &str) {
        let Some((line, path)) = key.split_once('\n') else { return };
        let Ok(line) = line.parse() else { return };
        self.go_to_bookmark(PathBuf::from(path), line);
    }

    /// The Bookmarks commands.
    pub(super) fn bookmark_command(&mut self, id: &str) {
        let in_text = self.reader.is_none() && self.doc.output_name.is_none();
        match id {
            "bookmark_toggle" | "bookmark_next" | "bookmark_previous" | "bookmark_clear" if !in_text => {
                self.message = Some("Bookmarks mark lines of text files".into())
            }
            "bookmark_toggle" => {
                self.doc.bookmark_toggle();
                if self.doc.path.is_none() {
                    self.message = Some("Bookmarks in an unsaved document are kept once it is saved".into());
                }
            }
            "bookmark_next" | "bookmark_previous" => {
                if !self.doc.bookmark_step(id == "bookmark_previous") {
                    self.message = Some("No bookmarks in this file".into());
                }
            }
            "bookmark_clear" => {
                let n = self.doc.bookmarks_clear();
                self.message = Some(match n {
                    0 => "No bookmarks in this file".into(),
                    1 => "Cleared 1 bookmark".into(),
                    n => format!("Cleared {n} bookmarks"),
                });
            }
            "bookmark_project_next" => self.bookmark_project_step(false),
            "bookmark_project_previous" => self.bookmark_project_step(true),
            "bookmark_list" => self.open_bookmarks(),
            _ => {}
        }
        self.bookmarks_sync();
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_bracket_and_occurrence_keys_reach_their_commands() {
        let mut a = App::new(None);
        a.open_output(String::new());
        a.doc.output_name = None;
        a.doc.read_only = false;
        a.doc.set_text("f(x) x\nx\n".into(), Doc::Code);
        let ctrl_alt = Mods { ctrl: true, alt: true, shift: false };
        a.focus = Focus::Doc;
        a.doc.select(1, 1);
        a.key(Key::Char('b'), ctrl_alt, None);
        assert_eq!(a.doc.buf.cur, 3, "Ctrl+Alt+B jumps to the matching bracket");
        a.doc.select(5, 6);
        a.key(Key::Char('l'), ctrl_alt, None);
        assert_eq!(a.doc.extra_cursors.len(), 2, "Ctrl+Alt+L selects every occurrence");
        a.doc.select(0, 0);
        a.key(Key::Char('k'), ctrl_alt, None);
        assert_eq!(a.doc.bookmarks, vec![0]);
        a.doc.select(a.doc.buf.starts[1], a.doc.buf.starts[1]);
        a.key(Key::Char(']'), ctrl_alt, None);
        assert_eq!(a.doc.buf.cur, 0, "Ctrl+Alt+] wraps round to the only mark");
    }

    #[test]
    fn bookmarks_are_kept_per_project_and_listed_across_files() {
        let dir = std::env::temp_dir().join(format!("ld-e4-marks-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let (a_path, b_path) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a_path, "one\ntwo\nthree\n").unwrap();
        std::fs::write(&b_path, "alpha\nbeta\n").unwrap();
        let mut a = App::new(Some(b_path.clone()));
        let at = a.doc.buf.starts[1];
        a.doc.select(at, at);
        a.bookmark_command("bookmark_toggle");
        let mut a = App::new(Some(a_path.clone()));
        a.doc.select(a.doc.buf.starts[2], a.doc.buf.starts[2]);
        a.bookmark_command("bookmark_toggle");
        // Deleting a line above moves the mark up with its line.
        a.doc.select(0, a.doc.buf.starts[1]);
        a.doc.insert("", EditKind::Other);
        a.edited();
        let all = a.bookmarks_all();
        let found: Vec<_> = all.iter().map(|b| (b.path.file_name().unwrap().to_str().unwrap(), b.line, b.text.as_str())).collect();
        assert_eq!(found, [("a.txt", 1, "three"), ("b.txt", 1, "beta")]);
        a.bookmark_command("bookmark_project_next");
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 1, "first the mark further down this file");
        a.bookmark_command("bookmark_project_next");
        assert_eq!(a.doc.path.as_deref(), Some(b_path.as_path()));
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 1);
        // A fresh window reads them back.
        let a = App::new(Some(a_path.clone()));
        assert_eq!(a.doc.bookmarks, vec![1]);
        let mut a = a;
        a.bookmark_command("bookmark_list");
        assert!(matches!(a.menu, Some((MenuKind::Bookmarks, _))));
        a.bookmark_command("bookmark_clear");
        assert!(App::new(Some(a_path)).doc.bookmarks.is_empty());
        let _ = std::fs::remove_file(store(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
