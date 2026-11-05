//! Coverage shading in the gutter, from the project's newest report. The
//! report is read on a thread; marks are worked out again only when the
//! file, its text or the report changes.
use super::*;
use crate::coverage::{self, Report};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Instant, SystemTime};

#[derive(Default)]
pub(super) struct CoverageView {
    shown: bool,
    root: PathBuf,
    report: Option<Report>,
    loading: Option<(Receiver<Result<Report, String>>, bool)>,
    checked: Option<Instant>,
    /// What the marks were made for: the file, its text's version, whether
    /// it is edited, and when the report was written.
    key: Option<(PathBuf, u64, bool, Option<SystemTime>)>,
}

impl App {
    pub(super) fn coverage_toggle(&mut self) {
        if self.coverage.shown {
            self.coverage = Default::default();
            self.doc.coverage_marks.clear();
            self.message = Some("Coverage hidden".into());
            self.redraw = true;
            return;
        }
        self.coverage.shown = true;
        self.coverage.root = self.root.clone();
        self.coverage_load(true);
    }

    pub(super) fn coverage_reload(&mut self) {
        self.coverage.shown = true;
        self.coverage.root = self.root.clone();
        self.coverage_load(true);
    }

    fn coverage_load(&mut self, announce: bool) {
        let Some((path, _)) = coverage::find(&self.root) else {
            if announce {
                self.message = Some(
                    "No coverage report yet. Write lcov.info or coverage.json in the project, as cargo llvm-cov --lcov --output-path lcov.info or coverage json do; it shows once written".into(),
                );
            }
            return;
        };
        let (tx, rx) = channel();
        let (wake, root) = (self.waker.clone(), self.root.clone());
        std::thread::spawn(move || {
            let _ = tx.send(coverage::read(&root, &path));
            wake();
        });
        self.coverage.loading = Some((rx, announce));
    }

    pub(super) fn coverage_poll(&mut self) {
        if !self.coverage.shown {
            return;
        }
        if self.coverage.root != self.root {
            self.coverage = CoverageView { shown: true, root: self.root.clone(), ..Default::default() };
        }
        if let Some((rx, announce)) = &self.coverage.loading {
            let announce = *announce;
            match rx.try_recv() {
                Ok(Ok(report)) => {
                    self.coverage.loading = None;
                    self.coverage.checked = Some(Instant::now());
                    self.coverage.report = Some(report);
                    self.coverage.key = None;
                    if announce {
                        self.coverage_announce();
                    }
                }
                Ok(Err(e)) => {
                    self.coverage.loading = None;
                    self.coverage.checked = Some(Instant::now());
                    self.message = Some(format!("Coverage: {e}"));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.coverage.loading = None,
            }
        }
        // A test run that writes a new report is picked up within seconds.
        if self.coverage.loading.is_none() && self.coverage.checked.is_none_or(|t| t.elapsed().as_secs() >= 2) {
            self.coverage.checked = Some(Instant::now());
            let current = self.coverage.report.as_ref().map(|r| (r.path.clone(), r.written));
            if let Some((path, written)) = coverage::find(&self.root)
                && current != Some((path, Some(written)))
            {
                self.coverage_load(false);
            }
        }
        self.coverage_marks();
    }

    fn coverage_announce(&mut self) {
        let Some(report) = &self.coverage.report else { return };
        let (hit, all) = report.totals();
        let percent = |hit: usize, all: usize| if all == 0 { 0 } else { (hit * 100 + all / 2) / all };
        let name = report.path.strip_prefix(&self.root).unwrap_or(&report.path).display().to_string();
        let files = report.files.len();
        let mut said = format!("Coverage from {name}: {}% of {all} lines in {files} file{}", percent(hit, all), if files == 1 { "" } else { "s" });
        if let Some(lines) = self.coverage_lines() {
            let ran = lines.iter().filter(|(_, r)| *r).count();
            said += &format!("; this file {}%", percent(ran, lines.len()));
        } else if self.doc.path.as_ref().is_some_and(|p| report.files.contains_key(p)) {
            said += "; this file changed after the report";
        }
        self.message = Some(said);
    }

    fn coverage_marks(&mut self) {
        let written = self.coverage.report.as_ref().and_then(|r| r.written);
        let key = self.doc.path.clone().map(|p| (p, self.doc.buf.version, self.doc.buf.modified, written));
        if key == self.coverage.key {
            return;
        }
        self.coverage.key = key;
        self.doc.coverage_marks = self.coverage_lines().unwrap_or_default();
        self.redraw = true;
    }

    /// The current file's measured lines, unless it has moved on from the
    /// report: edited, or written since.
    fn coverage_lines(&self) -> Option<Vec<(usize, bool)>> {
        let report = self.coverage.report.as_ref()?;
        let path = self.doc.path.as_ref()?;
        if self.doc.buf.modified || self.nb.is_some() || self.reader.is_some() {
            return None;
        }
        let lines = report.files.get(path).or_else(|| report.files.get(&path.canonicalize().ok()?))?;
        let saved = std::fs::metadata(path).ok()?.modified().ok()?;
        if report.written.is_some_and(|w| saved > w) {
            return None;
        }
        Some(lines.clone())
    }

    #[cfg(any(test, feature = "dump"))]
    pub(super) fn coverage_settle(&mut self) {
        let start = Instant::now();
        while self.coverage.loading.is_some() && start.elapsed().as_secs() < 10 {
            self.coverage_poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.coverage_poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_follow_the_report_and_hide_for_edited_or_newer_files() {
        let root = std::env::temp_dir().join(format!("ld-coverage-app-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let path = root.join("lib.rs");
        std::fs::write(&path, "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("lcov.info"), "SF:lib.rs\nDA:1,4\nDA:2,0\nend_of_record\n").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(1000, 800, 1.0);
        a.coverage_toggle();
        a.coverage_settle();
        assert_eq!(a.doc.coverage_marks, vec![(0, true), (1, false)]);
        assert!(a.message.as_deref().unwrap().contains("50% of 2 lines") && a.message.as_deref().unwrap().contains("this file 50%"), "{:?}", a.message);
        a.paste_text("x");
        a.coverage_poll();
        assert!(a.doc.coverage_marks.is_empty());
        a.doc.buf.undo();
        a.doc.buf.modified = false;
        a.coverage_poll();
        assert_eq!(a.doc.coverage_marks.len(), 2);
        // Saving after the report leaves its marks behind.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "fn a() {}\n").unwrap();
        a.coverage.key = None;
        a.coverage_poll();
        assert!(a.doc.coverage_marks.is_empty());
        a.coverage_toggle();
        assert!(!a.coverage.shown && a.doc.coverage_marks.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
