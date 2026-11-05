//! Searching every file of the project for text, in the background. The
//! matches stream into the Search tab of the right panel as they are found.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::{App, MenuKind};
use crate::kernel::Waker;
use crate::ui::{Entry, Menu};

/// How many matches are listed at most, and how many files are read.
pub(super) const MOST: usize = 50_000;
const FILES: usize = 20_000;
/// Files larger than this are left out: they are data, not writing.
const LARGEST: u64 = 2 << 20;
/// Folders a walk leaves out when the project is not under git.
const SKIP: [&str; 7] = ["target", "node_modules", "__pycache__", "build", "dist", "venv", ".venv"];

/// A line holding the text searched for.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Found {
    pub path: PathBuf,
    /// The line, counted from 1, and where the match is in it, in bytes.
    pub line: usize,
    pub col: usize,
    pub len: usize,
    /// The line, or for a long line the part around the match, which
    /// starts `cut` bytes into the line.
    pub text: String,
    pub cut: usize,
}

/// Lines longer than this keep only the part around a match.
const LONG_LINE: usize = 400;

/// What the worker has found that the window has not taken yet.
#[derive(Default)]
struct Batch {
    number: u64,
    found: Vec<Found>,
    /// Whether it stopped at `MOST`, and whether it is done.
    more: bool,
    finished: bool,
}

/// The search under way, and what it has found.
#[derive(Default)]
pub(super) struct Search {
    /// What the last search was for.
    pub(super) asked: String,
    pub(super) panel: super::search_panel::SearchPanel,
    options: super::replace::Options,
    pub(super) replacements: Vec<(PathBuf, String, String)>,
    /// The number of the latest search; an older one stops when it sees
    /// this has moved on.
    current: Arc<AtomicU64>,
    /// What the worker has found since the window last looked.
    done: Arc<Mutex<Batch>>,
    pub(super) pending: bool,
    pub(super) more: bool,
}

/// The files of a project worth searching: those git knows of and does
/// not ignore, or else every file outside hidden and build folders.
pub(super) fn files(root: &Path) -> Vec<PathBuf> {
    let listed = std::process::Command::new("git")
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    if let Some(out) = listed {
        let names = out.stdout.split(|&b| b == 0).filter(|n| !n.is_empty());
        let paths = names.map(|n| root.join(String::from_utf8_lossy(n).as_ref()));
        return paths.filter(|p| p.is_file()).take(FILES).collect();
    }
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries.into_iter().rev() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = e.file_type() else { continue };
            if name.starts_with('.') || (kind.is_dir() && SKIP.contains(&name.as_ref())) {
                continue;
            }
            if kind.is_dir() {
                dirs.push(e.path());
            } else if kind.is_file() {
                out.push(e.path());
                if out.len() >= FILES {
                    return out;
                }
            }
        }
    }
    out.sort();
    out
}

/// Searches the files for `needle`; case matters only if it has capitals.
/// Gives up, returning None, once `stop` says the search is out of date.
#[cfg(test)]
pub(super) fn search(paths: &[PathBuf], needle: &str, stop: impl Fn() -> bool) -> Option<(Vec<Found>, bool)> {
    let mut found = Vec::new();
    let more = search_buffers(paths, needle, &Default::default(), &Default::default(), stop, |f| found.extend(f))?;
    Some((found, more))
}

/// The match as it is listed: the whole line, or the part around the
/// match of a long one.
fn found(path: &Path, line: usize, text: &str, col: usize, len: usize) -> Found {
    let (cut, text) = if text.len() > LONG_LINE {
        let a = crate::editor::floor(text, col.saturating_sub(100));
        let b = crate::editor::ceil(text, (col + len + 200).min(text.len()));
        (a, text[a..b].to_string())
    } else {
        (0, text.trim_end().to_string())
    };
    Found { path: path.to_path_buf(), line, col, len, text, cut }
}

/// Searches the files, reading open tabs from their buffers, and hands
/// what it finds to `publish` in batches. Returns whether it stopped at
/// `MOST`, or None once `stop` says the search is out of date.
fn search_buffers(
    paths: &[PathBuf],
    needle: &str,
    options: &super::replace::Options,
    buffers: &std::collections::HashMap<PathBuf, String>,
    stop: impl Fn() -> bool,
    mut publish: impl FnMut(Vec<Found>),
) -> Option<bool> {
    let pattern = options.pattern(needle).ok()?;
    let mut fresh = Vec::new();
    let mut total = 0;
    let mut since = std::time::Instant::now();
    for path in paths {
        if stop() {
            return None;
        }
        let bytes = if let Some(text) = buffers.get(path) {
            std::borrow::Cow::Borrowed(text.as_bytes())
        } else {
            if std::fs::metadata(path).map_or(true, |m| m.len() > LARGEST) {
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else { continue };
            std::borrow::Cow::Owned(bytes)
        };
        if bytes.len() > LARGEST as usize || bytes[..bytes.len().min(4096)].contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            for m in pattern.find_iter(line) {
                fresh.push(found(path, i + 1, line, m.start(), m.len()));
                total += 1;
                if total >= MOST {
                    publish(fresh);
                    return Some(true);
                }
            }
        }
        // Batches go out often enough to stream, rarely enough to be cheap.
        if fresh.len() >= 512 || (!fresh.is_empty() && since.elapsed().as_millis() >= 60) {
            publish(std::mem::take(&mut fresh));
            since = std::time::Instant::now();
        }
    }
    publish(fresh);
    Some(false)
}

/// The text of every open tab with a path, so searches and replacements
/// see unsaved edits. Notebooks are left to their files.
pub(super) fn open_buffers(app: &App) -> std::collections::HashMap<PathBuf, String> {
    let mut buffers = std::collections::HashMap::new();
    for (i, tab) in app.file_tabs.iter().enumerate() {
        let pane = if i == app.active_file { Some(&app.doc) } else { tab.state.as_ref().map(|s| &s.pane) };
        if let Some(pane) = pane.filter(|p| p.doc != crate::pane::Doc::Cells) {
            if let Some(path) = &pane.path {
                buffers.insert(path.clone(), pane.buf.text.clone());
            }
        }
    }
    buffers
}

impl App {
    /// Opens the Search tab with the keys in its query, starting from the
    /// text selected, if it is on one line.
    pub(super) fn open_search(&mut self) {
        self.comp = None;
        let (a, b) = self.doc.buf.sel();
        let picked = if self.reader.is_none() { &self.doc.buf.text[a..b] } else { "" };
        if !picked.is_empty() && !picked.contains('\n') {
            let picked = picked.to_string();
            self.search.panel.set_query(&picked);
        }
        if matches!(self.menu, Some((MenuKind::Search, _))) {
            self.menu = None;
        }
        self.show_side(Some(super::Tab::Search));
        self.search_panel_focus(0);
        self.search.asked.clear();
        self.search_update();
    }

    /// Starts a search for what has been typed, if it or the options changed.
    pub(super) fn search_update(&mut self) {
        let typed = self.search.panel.query.text.clone();
        if typed == self.search.asked && self.search.options == self.search_options {
            return;
        }
        self.search.asked = typed.clone();
        self.search.options = self.search_options.clone();
        let number = self.search.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.search.panel.clear_results();
        self.search.more = false;
        if let Err(e) = self.search_options.pattern(&typed) {
            self.search.panel.status = e;
            self.search.pending = false;
            return;
        }
        if typed.trim().chars().count() < 2 {
            self.search.panel.status.clear();
            self.search.pending = false;
            return;
        }
        self.search.pending = true;
        self.search.panel.status = "Searching\u{2026}".into();
        *self.search.done.lock().unwrap() = Batch { number, ..Default::default() };
        let (roots, current, done, waker) =
            (self.roots(), self.search.current.clone(), self.search.done.clone(), self.waker.clone());
        let options = self.search_options.clone();
        let buffers = open_buffers(self);
        std::thread::spawn(move || {
            let stale = || current.load(Ordering::SeqCst) != number;
            let mut paths = super::roots::files_in(&roots);
            paths.extend(buffers.keys().filter(|p| super::roots::holding(&roots, p).is_some()).cloned());
            paths.sort();
            paths.dedup();
            paths.retain(|p| options.accepts(&super::roots::short_in(&roots, p)));
            let publish = |found: Vec<Found>| {
                let mut batch = done.lock().unwrap();
                if batch.number == number {
                    batch.found.extend(found);
                }
                drop(batch);
                waker_call(&waker);
            };
            if let Some(more) = search_buffers(&paths, &typed, &options, &buffers, stale, publish) {
                let mut batch = done.lock().unwrap();
                if batch.number == number {
                    (batch.more, batch.finished) = (more, true);
                }
                drop(batch);
                waker_call(&waker);
            }
        });
    }

    /// Takes in what the search under way has found since the last look.
    pub(super) fn search_poll(&mut self) {
        if !self.search.pending {
            return;
        }
        let (found, more, finished) = {
            let mut batch = self.search.done.lock().unwrap();
            if batch.number != self.search.current.load(Ordering::SeqCst) || (batch.found.is_empty() && !batch.finished) {
                return;
            }
            (std::mem::take(&mut batch.found), batch.more, batch.finished)
        };
        self.search.pending = !finished;
        self.search.more = more;
        self.search.panel.append(found);
        self.search_panel_status();
        self.redraw = true;
    }

    /// Waits for the search under way. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn search_settle(&mut self) {
        let start = std::time::Instant::now();
        while self.search.pending && start.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.search_poll();
        }
    }

    /// Opens a line the search found, with its match selected.
    pub(super) fn search_pick(&mut self, key: &str) {
        let mut parts = key.split('\n');
        let (Some(path), Some(line), Some(col), Some(len)) = (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return;
        };
        let path = PathBuf::from(path);
        let (line, col, len): (usize, usize, usize) =
            (line.parse().unwrap_or(1), col.parse().unwrap_or(0), len.parse().unwrap_or(0));
        self.jump_to(path.clone(), 0, 0);
        if self.doc.path.as_deref() != Some(path.as_path()) || self.reader.is_some() {
            return;
        }
        let buf = &self.doc.buf;
        let (start, end) = buf.line(line.saturating_sub(1).min(buf.lines().saturating_sub(1)));
        let a = crate::editor::floor(&buf.text, (start + col).min(end));
        let b = crate::editor::floor(&buf.text, (a + len).min(end));
        self.doc.select(a, b);
        self.doc.moved();
    }
}

fn waker_call(waker: &Waker) {
    waker();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Key, Mods};

    #[test]
    fn buffers_override_disk_and_stream_every_match() {
        let path = PathBuf::from("/unsaved-project/example.txt");
        let buffers = std::collections::HashMap::from([(path.clone(), "needle ".repeat(300))]);
        let mut found = Vec::new();
        let more = search_buffers(&[path], "needle", &Default::default(), &buffers, || false, |f| found.extend(f)).unwrap();
        assert_eq!(found.len(), 300);
        // A long line keeps only the part around each match.
        assert!(found[299].text.len() < 400 && found[299].col == 299 * 7);
        assert!(!more);
        assert_eq!(found[1].col, 7);
    }


    #[test]
    fn lines_are_found_in_the_files_of_a_folder() {
        let dir = std::env::temp_dir().join(format!("ld-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("a.md"), "Alpha\nthe Needle here\n").unwrap();
        std::fs::write(dir.join("notes/b.txt"), "no\nno\nneedle again\n").unwrap();
        std::fs::write(dir.join("target/c.txt"), "needle in a build\n").unwrap();
        std::fs::write(dir.join("d.bin"), b"needle\0binary").unwrap();
        let paths = files(&dir);
        let (found, more) = search(&paths, "needle", || false).unwrap();
        assert!(!more);
        let mut got: Vec<_> =
            found.iter().map(|f| (f.path.strip_prefix(&dir).unwrap().to_owned(), f.line, f.col)).collect();
        got.sort();
        assert_eq!(got, [(PathBuf::from("a.md"), 2, 4), (PathBuf::from("notes/b.txt"), 3, 0)]);
        // Capitals make case matter.
        assert_eq!(search(&paths, "Needle", || false).unwrap().0.len(), 1);

        // In the window: the list fills as it is typed, and a pick opens
        // the file with the match selected.
        let mut a = App::new(Some(dir.join("a.md")));
        a.resize(900, 700, 1.0);
        let ctrl_shift = Mods { ctrl: true, shift: true, alt: false };
        a.key(Key::Char('f'), ctrl_shift, None);
        assert_eq!(a.tab, super::super::Tab::Search);
        for ch in "again".chars() {
            a.key(Key::Char(ch), Mods::default(), Some(&ch.to_string()));
        }
        a.search_settle();
        assert_eq!(a.search.panel.results.len(), 1);
        assert_eq!(a.search.panel.results[0].line, 3);
        a.key(Key::Enter, Mods::default(), None);
        assert_eq!(a.doc.path.as_deref(), Some(dir.join("notes/b.txt").as_path()));
        let (s, e) = a.doc.buf.sel();
        assert_eq!(&a.doc.buf.text[s..e], "again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

impl App {
    pub(super) fn search_option(&mut self, id: &str) {
        match id {
            "regex" => self.search_options.regex = !self.search_options.regex,
            "word" => self.search_options.word = !self.search_options.word,
            "case" => self.search_options.case = !self.search_options.case,
            "preserve_case" => self.search_options.preserve_case = !self.search_options.preserve_case,
            "include" => {
                self.ask(super::PromptKind::SearchInclude);
                return;
            }
            "exclude" => {
                self.ask(super::PromptKind::SearchExclude);
                return;
            }
            "review" => {
                self.review_project_replacements();
                return;
            }
            "apply" => {
                self.pick_project_replace("apply");
                return;
            }
            "replace" => {
                self.ask(super::PromptKind::ProjectReplace);
                return;
            }
            _ => return,
        }
        self.found_for = None;
        self.prompt_changed();
        self.search_update();
        self.message = Some(format!(
            "Search: regex {}, whole word {}, match case {}, preserve case {}",
            self.search_options.regex, self.search_options.word, self.search_options.case, self.search_options.preserve_case
        ));
    }
    /// Plans the replacement of the matches listed for the last project
    /// query, and lists the plans to review.
    pub(super) fn preview_project_replace(&mut self, with: &str) {
        if self.search.asked.is_empty() {
            self.message = Some("Search the project before replacing".into());
            return;
        }
        self.search.panel.replace.text = with.into();
        self.search.panel.replace.cur = with.len();
        self.search.panel.show_replace = true;
        if self.search.panel.results.is_empty() || self.search.pending {
            // Nothing listed yet: search here and now, so the plan covers
            // every match of the query.
            self.search.current.fetch_add(1, Ordering::SeqCst);
            self.search.pending = false;
            let buffers = open_buffers(self);
            let roots = self.roots();
            let mut paths = super::roots::files_in(&roots);
            paths.extend(buffers.keys().filter(|p| super::roots::holding(&roots, p).is_some()).cloned());
            paths.sort();
            paths.dedup();
            paths.retain(|p| self.search_options.accepts(&super::roots::short_in(&roots, p)));
            let mut found = Vec::new();
            let more = search_buffers(&paths, &self.search.asked, &self.search_options, &buffers, || false, |f| found.extend(f));
            self.search.more = more.unwrap_or(false);
            self.search.panel.clear_results();
            self.search.panel.append(found);
            self.search.options = self.search_options.clone();
            self.search_panel_status();
        }
        if self.plan_replacements(with, |_, _| true) {
            self.review_project_replacements();
        }
    }

    /// Plans, into `search.replacements`, the replacement of the listed
    /// matches that `keep` accepts, by path and index into the results.
    /// Open tabs are planned from their buffers. Says why and returns
    /// false if nothing can be planned.
    pub(super) fn plan_replacements(&mut self, with: &str, keep: impl Fn(&Path, usize) -> bool) -> bool {
        self.search.replacements.clear();
        let pattern = match self.search_options.pattern(&self.search.asked) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some(e);
                return false;
            }
        };
        let buffers = open_buffers(self);
        let mut wanted: std::collections::BTreeMap<&Path, std::collections::HashSet<(usize, usize)>> = Default::default();
        for (i, f) in self.search.panel.results.iter().enumerate() {
            if keep(&f.path, i) {
                wanted.entry(&f.path).or_default().insert((f.line, f.col));
            }
        }
        let mut plans = Vec::new();
        for (path, at) in wanted {
            let text = match buffers.get(path) {
                Some(text) => text.clone(),
                None => {
                    if std::fs::metadata(path).map_or(true, |m| m.len() > LARGEST) {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(path) else { continue };
                    text
                }
            };
            if text.contains('\0') {
                continue;
            }
            let out = replace_lines(&text, &pattern, &self.search_options, with, |line, col| at.contains(&(line, col)));
            if out != text {
                plans.push((path.to_path_buf(), text, out));
            }
        }
        if plans.is_empty() {
            self.message = Some("Nothing to replace. Search again if the files changed".into());
            return false;
        }
        self.search.replacements = plans;
        true
    }
    pub(super) fn review_project_replacements(&mut self) {
        self.review_replacements("Review project replacements");
    }
    /// Lists planned edits: one row applies them all, the others show a
    /// file's diff.
    pub(super) fn review_replacements(&mut self, title: &str) {
        let mut entries = vec![Entry::new(
            "apply",
            "Apply replacements as unsaved edits",
            &format!("{} files. Save each after review.", self.search.replacements.len()),
        )];
        entries.extend(
            self.search
                .replacements
                .iter()
                .enumerate()
                .map(|(i, (p, _, _))| Entry::new(&i.to_string(), &self.root_short(p), "Preview diff")),
        );
        self.menu = Some((MenuKind::SearchReplace, Menu::new(title, entries, "")));
    }
    pub(super) fn pick_project_replace(&mut self, key: &str) {
        if key != "apply" {
            if let Some((_path, before, after)) =
                key.parse::<usize>().ok().and_then(|i| self.search.replacements.get(i))
            {
                let text = super::repository::diff(before, after, 3).unwrap_or_else(|e| e);
                self.open_output(text);
            }
            return;
        }
        // Refuse anything changed since the plan before touching any buffer:
        // an open tab must still hold the planned text, and a file not
        // open, or open without edits, must still hold it on disk.
        let lf = |s: &str| s.replace("\r\n", "\n");
        for (path, text, _) in &self.search.replacements {
            let mut open = None;
            for (i, t) in self.file_tabs.iter().enumerate() {
                let p = if i == self.active_file { Some(&self.doc) } else { t.state.as_ref().map(|s| &s.pane) };
                if let Some(p) = p.filter(|p| p.path.as_ref() == Some(path)) {
                    open = Some((p.buf.modified, p.read_only || p.doc == crate::pane::Doc::Cells, lf(&p.buf.text)));
                }
            }
            if open.as_ref().is_some_and(|o| o.1) {
                self.message = Some("Close notebooks and read-only tabs before project replacement".into());
                return;
            }
            if open.as_ref().is_some_and(|o| o.2 != lf(text)) {
                self.message = Some("A replacement target was edited. Search again.".into());
                return;
            }
            if !open.as_ref().is_some_and(|o| o.0) && std::fs::read_to_string(path).ok().map(|d| lf(&d)) != Some(lf(text)) {
                self.message = Some("A replacement target changed on disk. Search again.".into());
                return;
            }
            if super::doc_for(Some(path)) == crate::pane::Doc::Cells
                || crate::pdf::is_pdf(path)
                || super::is_picture(path)
            {
                self.message = Some("Exclude notebooks and pictures before project replacement".into());
                return;
            }
        }
        let plans = std::mem::take(&mut self.search.replacements);
        let n = plans.len();
        for (path, _, out) in plans {
            self.open_tab(path);
            let out = out.replace("\r\n", "\n");
            // Only the changed middle is replaced, as one step to undo, so
            // the caret and folds elsewhere stay where they were.
            let (a, b, ins) = changed_middle(&self.doc.buf.text, &out);
            if self.doc.edit(a, b, ins, crate::editor::EditKind::Other) {
                self.doc.buf.commit();
                self.doc.select(a, a + ins.len());
                self.doc.moved();
            }
            self.edited();
        }
        self.message = Some(format!("Replaced in {n} files. Review and save the open tabs."));
    }
}
#[cfg(test)]
mod replacement_tests {
    use super::super::replace::Options;
    use super::*;
    #[test]
    fn patterns_share_unicode_boundaries_capture_expansion_and_path_filters() {
        let o = Options {
            regex: true,
            word: true,
            include: "src/**,*.py".into(),
            exclude: "**/generated/**".into(),
            ..Default::default()
        };
        assert_eq!(o.matches("café cafés CAFÉ", r"(café)").unwrap(), [(0, 5), (13, 18)]);
        assert_eq!(o.replacement("foo=1 foo=2", "foo=(\\d)", "bar=$1").unwrap(), "bar=1 bar=2");
        assert!(o.pattern("[").is_err());
        assert!(o.accepts("src/a.rs"));
        assert!(!o.accepts("src/generated/a.rs"));
        assert!(!o.accepts("README.md"));
        assert_eq!(Options::default().replacement("x", "x", "$1").unwrap(), "$1");
    }
    #[test]
    fn project_replacement_refuses_stale_disk_and_preserves_unsaved_review() {
        let root = std::env::temp_dir().join(format!("ld-replace-project-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let file = root.join("a.txt");
        std::fs::write(&file, "one one\r\n").unwrap();
        let mut a = App::new(Some(file.clone()));
        a.search.asked = "one".into();
        a.ask(super::super::PromptKind::ProjectReplace);
        a.submit(false);
        assert_eq!(a.search.replacements[0].2, " \n", "planned from the open buffer");
        a.ask(super::super::PromptKind::ProjectReplace);
        a.prompt.as_mut().unwrap().text = "  ".into();
        a.submit(false);
        assert_eq!(a.search.replacements[0].2, "     \n");
        a.preview_project_replace("two");
        assert_eq!(a.search.replacements.len(), 1);
        std::fs::write(&file, "changed\r\n").unwrap();
        a.pick_project_replace("apply");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed\r\n");
        assert!(!a.doc.buf.modified);
        std::fs::write(&file, "one one\r\n").unwrap();
        a.preview_project_replace("two");
        a.pick_project_replace("apply");
        assert_eq!(a.doc.buf.text, "two two\n");
        assert!(a.doc.buf.modified);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one one\r\n");
        a.doc.key(crate::app::Key::Char('z'), crate::app::Mods { ctrl: true, ..Default::default() }, None);
        assert_eq!(a.doc.buf.text, "one one\n");
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Replaces, line by line as project search matches, the matches that
/// `keep` accepts by line (from 1) and byte column, keeping line endings.
pub(super) fn replace_lines(
    text: &str,
    pattern: &regex::Regex,
    options: &super::replace::Options,
    with: &str,
    keep: impl Fn(usize, usize) -> bool,
) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, piece) in text.split_inclusive('\n').enumerate() {
        let body = piece.strip_suffix('\n').map_or(piece, |b| b.strip_suffix('\r').unwrap_or(b));
        let mut last = 0;
        for caps in pattern.captures_iter(body) {
            let m = caps.get(0).unwrap();
            if !keep(i + 1, m.start()) {
                continue;
            }
            out.push_str(&body[last..m.start()]);
            out.push_str(&options.expand(&caps, with));
            last = m.end();
        }
        out.push_str(&piece[last..]);
    }
    out
}

/// The part of `before` that differs from `after`: its range, and what
/// stands there in `after`.
pub(super) fn changed_middle<'a>(before: &str, after: &'a str) -> (usize, usize, &'a str) {
    let mut a = before.bytes().zip(after.bytes()).take_while(|(x, y)| x == y).count();
    while !before.is_char_boundary(a) || !after.is_char_boundary(a) {
        a -= 1;
    }
    let most = before.len().min(after.len()) - a;
    let mut z = before.bytes().rev().zip(after.bytes().rev()).take(most).take_while(|(x, y)| x == y).count();
    while !before.is_char_boundary(before.len() - z) || !after.is_char_boundary(after.len() - z) {
        z -= 1;
    }
    (a, before.len() - z, &after[a..after.len() - z])
}

#[cfg(test)]
mod panel_replacement_tests {
    use super::*;

    #[test]
    fn lines_replace_only_kept_matches_and_keep_endings() {
        let o = super::super::replace::Options { preserve_case: true, ..Default::default() };
        let re = o.pattern("cat").unwrap();
        let text = "cat Cat\r\nno\ncat";
        assert_eq!(replace_lines(text, &re, &o, "dog", |_, _| true), "dog Dog\r\nno\ndog");
        assert_eq!(replace_lines(text, &re, &o, "dog", |l, c| (l, c) == (1, 4)), "cat Dog\r\nno\ncat");
        let r = super::super::replace::Options { regex: true, ..Default::default() };
        assert_eq!(replace_lines("a=1", &r.pattern(r"(\w)=(\d)").unwrap(), &r, "$2=$1", |_, _| true), "1=a");
    }

    #[test]
    fn the_changed_middle_is_small_and_on_char_boundaries() {
        assert_eq!(changed_middle("one two three", "one 2 three"), (4, 7, "2"));
        assert_eq!(changed_middle("aé", "aè"), (1, 3, "è"));
        assert_eq!(changed_middle("same", "same"), (4, 4, ""));
        assert_eq!(changed_middle("ab", "abab"), (2, 2, "ab"));
    }
}
