//! Multibuffer tabs: excerpts of many files in one editable document. Edits
//! go to each file's buffer, the open tab's if the file is open, or else a
//! buffer of the multibuffer's own; edits made elsewhere come back.
use super::*;
use crate::multibuffer::{self as mb, Kind, Multi, Source};

/// Files larger than this are not shown in excerpts.
const LARGEST: u64 = 2 << 20;

impl App {
    /// The inactive tab that has a file open as text.
    fn multibuffer_tab(&self, path: &Path) -> Option<usize> {
        (0..self.file_tabs.len()).filter(|&i| i != self.active_file).find(|&i| {
            self.file_tabs[i].state.as_ref().is_some_and(|s| {
                s.pane.path.as_deref() == Some(path)
                    && s.reader.is_none()
                    && s.notebook.is_none()
                    && s.pane.multi.is_none()
            })
        })
    }

    /// Adds a file to a multibuffer being built and gives its text: an open
    /// tab's, unsaved edits included, or else the file's from disk.
    fn multibuffer_load(&self, m: &mut Multi, path: &Path) -> Option<String> {
        let path = path.canonicalize().ok()?;
        if let Some(s) = m.source(&path) {
            return Some(s.text.clone());
        }
        if doc_for(Some(&path)) == Doc::Cells || pdf::is_pdf(&path) || is_picture(&path) {
            return None;
        }
        let open = (self.doc.path.as_deref() == Some(&path) && self.reader.is_none() && self.nb.is_none())
            .then_some(&self.doc)
            .or_else(|| self.multibuffer_tab(&path).and_then(|i| self.file_tabs[i].state.as_ref()).map(|s| &s.pane));
        let source = match open {
            Some(pane) => Source {
                path: path.clone(),
                text: pane.buf.text.clone(),
                version: pane.buf.version,
                hidden: None,
                touched: false,
                disk: 0,
            },
            None => {
                if std::fs::metadata(&path).ok()?.len() > LARGEST {
                    return None;
                }
                let raw = String::from_utf8(std::fs::read(&path).ok()?).ok()?;
                let disk = mb::disk_hash(Some(raw.as_bytes()));
                let eol = editor::Eol::detect(&raw);
                let text = raw.replace("\r\n", "\n");
                let text = if eol == editor::Eol::Cr { text.replace('\r', "\n") } else { text };
                let mut buf = editor::Buffer::new(text.clone());
                buf.eol = eol;
                Source { path: path.clone(), text, version: buf.version, hidden: Some(buf), touched: false, disk }
            }
        };
        let text = source.text.clone();
        m.sources.push(source);
        Some(text)
    }

    /// Opens a multibuffer in a tab of its own.
    pub(super) fn open_multibuffer(&mut self, m: Multi) {
        if m.excerpts.is_empty() {
            self.message = Some(format!("{}: nothing to show in the editor", m.kind.name()));
            return;
        }
        let shown = m.render();
        let (title, count) = (m.title.clone(), m.excerpts.len());
        self.open_output(String::new());
        self.doc.read_only = false;
        self.doc.numbers = true;
        self.doc.fill = true;
        self.doc.multi = Some(Box::new(m));
        self.doc.set_text(shown, Doc::Cells);
        self.doc.output_name = Some(title);
        self.multibuffer_decorate();
        if let Some(m) = &mut self.doc.multi {
            m.seen = self.doc.buf.version;
        }
        self.focus = Focus::Doc;
        self.message = Some(format!("{count} excerpts; Ctrl+S saves the files edited here"));
    }

    // ---- building -------------------------------------------------------

    /// Builds a multibuffer from lines of interest, by file, in order.
    fn multibuffer_from(&self, kind: Kind, title: &str, groups: Vec<(PathBuf, Vec<(usize, String)>)>) -> Multi {
        let mut m = Multi::new(kind, title, &self.root);
        for (path, hits) in groups {
            let Some(text) = self.multibuffer_load(&mut m, &path) else { continue };
            let path = path.canonicalize().unwrap_or(path);
            let lines: Vec<usize> = hits.iter().map(|h| h.0).collect();
            let first = m.excerpts.len();
            m.add_file(&path, &text, &lines, mb::CONTEXT);
            for x in &mut m.excerpts[first..] {
                x.notes = hits
                    .iter()
                    .filter(|h| (x.start..x.end).contains(&h.0) && !h.1.is_empty())
                    .map(|h| format!("{}: {}", h.0 + 1, h.1))
                    .collect();
            }
        }
        m
    }

    /// Search: Open results in editor.
    pub(super) fn multibuffer_search(&mut self) {
        let results = &self.search.panel.results;
        if results.is_empty() {
            self.message = Some("Search the project first: no results to open".into());
            return;
        }
        let mut groups: Vec<(PathBuf, Vec<(usize, String)>)> = Vec::new();
        for f in results {
            match groups.last_mut() {
                Some((p, hits)) if *p == f.path => hits.push((f.line - 1, String::new())),
                _ => groups.push((f.path.clone(), vec![(f.line - 1, String::new())])),
            }
        }
        let asked = self.search.asked.clone();
        let mut m = self.multibuffer_from(Kind::Search, &format!("Search: {asked}"), groups);
        m.pattern = self.search_options.pattern(&asked).ok();
        self.open_multibuffer(m);
    }

    /// The problems with a file, by path, for a multibuffer.
    fn problem_groups(&self) -> Vec<(PathBuf, Vec<(usize, String)>)> {
        let mut groups: Vec<(PathBuf, Vec<(usize, String)>)> = Vec::new();
        for p in self.problem_list() {
            let Some(path) = p.path else { continue };
            let hit = (p.line.saturating_sub(1), format!("{}: {}", p.source, p.message.replace('\n', " ")));
            match groups.last_mut() {
                Some((q, hits)) if *q == path => hits.push(hit),
                _ => groups.push((path, vec![hit])),
            }
        }
        groups
    }

    /// Problems: Open in editor.
    pub(super) fn multibuffer_problems(&mut self) {
        let groups = self.problem_groups();
        let mut m = self.multibuffer_from(Kind::Problems, "Problems", groups);
        m.stamp = self.problems_stamp();
        self.open_multibuffer(m);
    }

    fn problems_stamp(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.problem_groups().hash(&mut h);
        h.finish()
    }

    /// Code: Find all references in editor, from a language server's list.
    pub(super) fn multibuffer_locations(&mut self, title: &str) {
        let mut groups: Vec<(PathBuf, Vec<(usize, String)>)> = Vec::new();
        for v in &self.languages.locations {
            let Some(path) = v["uri"].as_str().or(v["targetUri"].as_str()).and_then(crate::lsp::path) else { continue };
            let line = v.pointer("/range/start/line").or(v.pointer("/targetSelectionRange/start/line"));
            let line = line.and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;
            match groups.iter_mut().find(|(p, _)| *p == path) {
                Some((_, hits)) => hits.push((line, String::new())),
                None => groups.push((path, vec![(line, String::new())])),
            }
        }
        let m = self.multibuffer_from(Kind::References, title, groups);
        self.open_multibuffer(m);
    }

    /// Review hunks in editor: each change of the agent's latest turn as an
    /// excerpt of the file as it is now, its removed lines under it.
    pub(super) fn multibuffer_review(&mut self) {
        let baselines = match self.review_baselines() {
            Ok(b) => b,
            Err(e) => return self.message = Some(e),
        };
        let mut m = Multi::new(Kind::Review, "Review: agent changes", &self.root);
        for (path, baseline) in baselines {
            let Some(text) = self.multibuffer_load(&mut m, &path) else { continue };
            let path = path.canonicalize().unwrap_or(path);
            let Ok(hunks) = super::agent_changes::line_hunks(&baseline, &text) else { continue };
            let added: Vec<(usize, usize)> = hunks.iter().map(|h| h.1).collect();
            let all = mb::lines(&text);
            for ((s, e), (old, new, removed)) in mb::hunk_ranges(&added, mb::CONTEXT, all.len()).into_iter().zip(hunks)
            {
                if s >= e || m.excerpts.len() >= mb::MOST {
                    continue;
                }
                let lines = all[s..e].iter().map(|l| l.to_string()).collect();
                let hunk = Some(mb::Hunk { removed, added: new, old });
                m.excerpts.push(mb::Excerpt {
                    path: path.clone(),
                    start: s,
                    end: e,
                    lines,
                    hits: Vec::new(),
                    notes: Vec::new(),
                    hunk,
                });
            }
        }
        if m.excerpts.is_empty() {
            self.message = Some("No hunks left to review".into());
            return;
        }
        self.open_multibuffer(m);
    }

    /// Keeps or reverts the change an excerpt of the review shows. A kept
    /// change leaves the review; a reverted one is an edit of the file, to
    /// save or undo like the others.
    pub(super) fn review_excerpt(&mut self, cell: usize, keep: bool) {
        self.multibuffer_tick();
        let Some(mut m) = self.doc.multi.take() else { return };
        let result = self.review_excerpt_in(&mut m, cell, keep);
        self.doc.multi = Some(m);
        match result {
            Ok(()) => {
                self.multibuffer_rebuild(None);
                let left =
                    self.doc.multi.as_ref().map_or(0, |m| m.excerpts.iter().filter(|x| x.hunk.is_some()).count());
                self.message = Some(format!("{} hunk; {left} left", if keep { "Kept" } else { "Reverted" }));
            }
            Err(e) => self.message = Some(e),
        }
    }

    fn review_excerpt_in(&mut self, m: &mut Multi, cell: usize, keep: bool) -> Result<(), String> {
        let x = m.excerpts.get(cell).cloned().ok_or("No such hunk")?;
        let h = x.hunk.clone().ok_or("This excerpt is not a change to review")?;
        let k = m.sources.iter().position(|s| s.path == x.path).ok_or("The file is no longer shown")?;
        let (text, _) = self.multibuffer_current(&m.sources[k]).ok_or("The file is no longer open")?;
        if keep {
            let all = mb::lines(&text);
            let kept: Vec<String> =
                all[h.added.0.min(all.len())..h.added.1.min(all.len())].iter().map(|l| l.to_string()).collect();
            self.review_keep(&x.path, h.old, &h.removed, &kept)?;
            let delta = kept.len() as isize - (h.old.1 - h.old.0) as isize;
            for y in m.excerpts.iter_mut().filter(|y| y.path == x.path) {
                if let Some(o) = y.hunk.as_mut().filter(|o| o.old.0 >= h.old.1 && o.old != h.old) {
                    o.old = ((o.old.0 as isize + delta) as usize, (o.old.1 as isize + delta) as usize);
                }
            }
            m.excerpts.remove(cell);
            return Ok(());
        }
        let (s, e, ins) = mb::source_edit(&text, h.added.0, h.added.1, &h.removed);
        let (new, version, undo) =
            self.multibuffer_apply(m, k, (s, e, ins), EditKind::Other).ok_or("The file refused the edit")?;
        m.excerpts.remove(cell);
        m.follow(&x.path, &new);
        let source = &mut m.sources[k];
        (source.version, source.touched) = (version, true);
        m.record(vec![(x.path.clone(), undo)]);
        Ok(())
    }

    // ---- keeping the excerpts and the files together ---------------------

    /// Carries edits between the multibuffer shown and its files. Call
    /// after input and before drawing.
    pub(super) fn multibuffer_tick(&mut self) {
        self.multibuffer_hand_over();
        let Some(mut m) = self.doc.multi.take() else { return };
        let mut rebuild = false;
        let mut target = None;
        if let Some(redo) = m.request.take() {
            target = self.multibuffer_undo(&mut m, redo);
        }
        if self.doc.buf.version != m.seen {
            rebuild |= self.multibuffer_push(&mut m);
        }
        rebuild |= self.multibuffer_pull(&mut m);
        if m.kind == Kind::Problems {
            let stamp = self.problems_stamp();
            if stamp != m.stamp {
                m.stamp = stamp;
                rebuild |= self.multibuffer_refresh_problems(&mut m);
            }
        }
        let modified = m.sources.iter().any(|s| s.touched && self.multibuffer_modified(s));
        self.doc.multi = Some(m);
        if rebuild || target.is_some() {
            self.multibuffer_rebuild(target);
        }
        self.doc.buf.modified = modified;
    }

    fn multibuffer_modified(&self, s: &Source) -> bool {
        match &s.hidden {
            Some(b) => b.modified,
            None => self
                .multibuffer_tab(&s.path)
                .is_some_and(|i| self.file_tabs[i].state.as_ref().unwrap().pane.buf.modified),
        }
    }

    /// The text a file has now, and its version.
    fn multibuffer_current(&self, s: &Source) -> Option<(String, u64)> {
        if let Some(b) = &s.hidden {
            return Some((b.text.clone(), b.version));
        }
        let i = self.multibuffer_tab(&s.path)?;
        let pane = &self.file_tabs[i].state.as_ref()?.pane;
        Some((pane.buf.text.clone(), pane.buf.version))
    }

    /// Takes edits made in the multibuffer to the files. Returns whether
    /// the multibuffer must be built again, as when a file refused one.
    fn multibuffer_push(&mut self, m: &mut Multi) -> bool {
        if self.doc.cells.len() != m.excerpts.len() {
            return true;
        }
        let mut step = Vec::new();
        let mut rebuild = false;
        for i in 0..m.excerpts.len() {
            let (a, b) = self.doc.cell_body(i);
            let new: Vec<String> = self.doc.buf.text[a..b].split('\n').map(str::to_string).collect();
            if new == m.excerpts[i].lines {
                continue;
            }
            let path = m.excerpts[i].path.clone();
            let Some(k) = m.sources.iter().position(|s| s.path == path) else { continue };
            let Some((text, version)) = self.multibuffer_current(&m.sources[k]) else {
                rebuild = true;
                continue;
            };
            // Changed elsewhere too: what is there wins.
            if version != m.sources[k].version {
                rebuild = true;
                continue;
            }
            let before = m.excerpts.clone();
            let Some((s, e, ins)) = m.take(i, new, &text) else { continue };
            let kind = if s == e && ins.chars().count() == 1 && ins != "\n" {
                EditKind::Type
            } else if ins.is_empty() && text[s..e].chars().count() == 1 {
                EditKind::Delete
            } else {
                EditKind::Other
            };
            let applied = self.multibuffer_apply(m, k, (s, e, ins), kind);
            match applied {
                Some((text, version, undo)) => {
                    let source = &mut m.sources[k];
                    (source.text, source.version, source.touched) = (text, version, true);
                    step.push((path, undo));
                }
                None => {
                    m.excerpts = before;
                    rebuild = true;
                }
            }
        }
        m.record(step);
        m.seen = self.doc.buf.version;
        rebuild
    }

    /// Makes an edit in a file's buffer: its tab's, or the multibuffer's
    /// own. Gives the text, version and undo length after, unless refused.
    fn multibuffer_apply(
        &mut self,
        m: &mut Multi,
        k: usize,
        (s, e, ins): (usize, usize, String),
        kind: EditKind,
    ) -> Option<(String, u64, usize)> {
        if let Some(buf) = &mut m.sources[k].hidden {
            buf.replace(s, e, &ins, kind);
            return Some((buf.text.clone(), buf.version, buf.undo_len()));
        }
        let t = self.multibuffer_tab(&m.sources[k].path)?;
        let pane = &mut self.file_tabs[t].state.as_mut().unwrap().pane;
        let done = pane.edit(s, e, &ins, kind);
        if let Some(note) = pane.note.take() {
            self.message = Some(note);
        }
        done.then(|| (pane.buf.text.clone(), pane.buf.version, pane.buf.undo_len()))
    }

    /// Follows edits made to the files elsewhere. Returns whether any came.
    fn multibuffer_pull(&mut self, m: &mut Multi) -> bool {
        let mut changed = false;
        for k in 0..m.sources.len() {
            let path = m.sources[k].path.clone();
            let open = self.multibuffer_tab(&path);
            if open.is_none() && m.sources[k].hidden.is_none() {
                // Its tab closed, edits kept or not: the file is on disk.
                let mut probe = Multi::new(m.kind, "", &self.root);
                let Some(text) = self.multibuffer_load(&mut probe, &path) else { continue };
                let Some(fresh) = probe.sources.pop() else { continue };
                m.follow(&path, &text);
                let source = &mut m.sources[k];
                (source.version, source.hidden, source.disk) = (fresh.version, fresh.hidden, fresh.disk);
                changed = true;
                continue;
            }
            let Some((text, version)) = self.multibuffer_current(&m.sources[k]) else { continue };
            if version != m.sources[k].version {
                m.follow(&path, &text);
                m.sources[k].version = version;
                changed = true;
            }
        }
        changed
    }

    /// A file a multibuffer holds a buffer for has since opened in a tab:
    /// the tab takes the buffer's unsaved edits, and the multibuffer the tab.
    fn multibuffer_hand_over(&mut self) {
        for i in 0..self.file_tabs.len() {
            let pane = if i == self.active_file {
                &mut self.doc
            } else {
                match self.file_tabs[i].state.as_mut() {
                    Some(s) => &mut s.pane,
                    None => continue,
                }
            };
            let Some(mut m) = pane.multi.take() else { continue };
            for s in &mut m.sources {
                if s.hidden.is_none() {
                    continue;
                }
                let Some(t) = (0..self.file_tabs.len()).filter(|&t| t != self.active_file && t != i).find(|&t| {
                    self.file_tabs[t].state.as_ref().is_some_and(|st| {
                        st.pane.path.as_deref() == Some(&s.path) && st.notebook.is_none() && st.reader.is_none()
                    })
                }) else {
                    continue;
                };
                let hidden = s.hidden.take().unwrap();
                let tab = &mut self.file_tabs[t].state.as_mut().unwrap().pane;
                if hidden.modified && !tab.buf.modified && tab.buf.text != hidden.text {
                    let len = tab.buf.text.len();
                    tab.edit(0, len, &hidden.text, EditKind::Other);
                    tab.buf.eol = hidden.eol;
                }
            }
            let pane =
                if i == self.active_file { &mut self.doc } else { &mut self.file_tabs[i].state.as_mut().unwrap().pane };
            pane.multi = Some(m);
        }
        // The active tab, opened just now, may be such a file.
        if let Some(path) = self.doc.path.clone().filter(|_| self.doc.multi.is_none() && self.nb.is_none()) {
            for t in 0..self.file_tabs.len() {
                if t == self.active_file {
                    continue;
                }
                let Some(st) = self.file_tabs[t].state.as_mut() else { continue };
                let Some(m) = st.pane.multi.as_mut() else { continue };
                let Some(s) = m.source_mut(&path) else { continue };
                let Some(hidden) = s.hidden.take() else { continue };
                if hidden.modified && !self.doc.buf.modified && self.doc.buf.text != hidden.text {
                    let len = self.doc.buf.text.len();
                    self.doc.edit(0, len, &hidden.text, EditKind::Other);
                    self.doc.buf.eol = hidden.eol;
                    self.message = Some("Unsaved edits from a multibuffer are in this tab".into());
                }
                // Read again from the tab once the multibuffer is shown.
                s.version = 0;
            }
        }
    }

    /// Undoes or redoes the multibuffer's last step in its files. Gives the
    /// place of the change, to put the cursor at.
    fn multibuffer_undo(&mut self, m: &mut Multi, redo: bool) -> Option<(PathBuf, usize)> {
        let Some(step) = (if redo { m.redo.pop() } else { m.undo.pop() }) else {
            self.message = Some(if redo { "Nothing to redo here" } else { "Nothing to undo here" }.into());
            return None;
        };
        let mut place = None;
        let mut refused = Vec::new();
        for (path, n) in step.iter().rev() {
            let Some(k) = m.sources.iter().position(|s| s.path == *path) else { continue };
            let fits = |len: usize, can: bool| if redo { can && len + 1 == *n } else { len == *n };
            let at = if let Some(buf) = &mut m.sources[k].hidden {
                if !fits(buf.undo_len(), buf.can_redo()) {
                    None
                } else if redo {
                    buf.redo().map(|c| c.pos)
                } else {
                    buf.undo().map(|c| c.pos)
                }
            } else if let Some(t) = self.multibuffer_tab(path) {
                let pane = &mut self.file_tabs[t].state.as_mut().unwrap().pane;
                if fits(pane.buf.undo_len(), pane.buf.can_redo()) { pane.history_step(redo) } else { None }
            } else {
                None
            };
            match at {
                Some(at) => place = Some((path.clone(), at)),
                None => refused.push(mb::short(&self.root, path)),
            }
        }
        if !refused.is_empty() {
            self.message = Some(format!("Edited since; undo there instead: {}", refused.join(", ")));
        }
        if redo {
            m.undo.push(step)
        } else {
            m.redo.push(step)
        }
        place
    }

    /// Shows the multibuffer's excerpts again, keeping the cursor on the
    /// same line of the same file, or putting it at `target`.
    fn multibuffer_rebuild(&mut self, target: Option<(PathBuf, usize)>) {
        let Some(m) = self.doc.multi.as_ref() else { return };
        let place = match target {
            Some((path, at)) => m.source(&path).map(|s| {
                let at = editor::floor(&s.text, at.min(s.text.len()));
                let line = s.text[..at].matches('\n').count();
                let col = at - s.text[..at].rfind('\n').map_or(0, |i| i + 1);
                (path.clone(), line, col)
            }),
            None => self.doc.excerpt_at(self.doc.buf.cur).and_then(|(cell, off)| {
                let (path, line) = m.locate(cell, off)?;
                let col = self.doc.buf.cur - self.doc.buf.starts[self.doc.buf.line_of(self.doc.buf.cur)];
                Some((path.to_path_buf(), line, col))
            }),
        };
        let text = m.render();
        self.doc.sync(&text);
        self.multibuffer_decorate();
        let Some(m) = self.doc.multi.as_ref() else { return };
        if let Some((path, line, col)) = place
            && let Some((cell, off)) = m.find(&path, line)
            && let Some(at) = self.doc.excerpt_offset(cell, off, col)
        {
            self.doc.select(at, at);
        } else {
            let at = editor::floor(&self.doc.buf.text, self.doc.buf.cur.min(self.doc.buf.text.len()));
            self.doc.select(at, at);
        }
        if let Some(m) = &mut self.doc.multi {
            m.seen = self.doc.buf.version;
        }
    }

    /// Sets what the excerpts show besides their text: notes and removed
    /// lines under them, matches, and lines a reviewed change added.
    pub(super) fn multibuffer_decorate(&mut self) {
        use std::hash::{Hash, Hasher};
        let Some(mut m) = self.doc.multi.take() else { return };
        self.doc.decor.clear();
        m.marks.clear();
        m.added.clear();
        if self.doc.cells.len() != m.excerpts.len() {
            self.doc.multi = Some(m);
            return;
        }
        for (i, cell) in self.doc.cells.iter().enumerate() {
            let x = &m.excerpts[i];
            let mut output: Vec<crate::layout::OutItem> = Vec::new();
            if let Some(h) = &x.hunk {
                let (a, b) = h.added;
                for l in a.max(x.start)..b.min(x.end) {
                    m.added.push(cell.line + 1 + l - x.start);
                }
                if !h.removed.is_empty() {
                    let text = h.removed.iter().map(|l| format!("\u{2212} {l}")).collect::<Vec<_>>().join("\n");
                    output.push(crate::layout::OutItem::Text { text, dim: false, error: true, attrs: Vec::new() });
                }
            }
            if !x.notes.is_empty() {
                output.push(crate::layout::OutItem::Text {
                    text: x.notes.join("\n"),
                    dim: false,
                    error: m.kind == Kind::Problems,
                    attrs: Vec::new(),
                });
            }
            if !output.is_empty() {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (&x.notes, x.hunk.as_ref().map(|h| &h.removed)).hash(&mut h);
                self.doc.decor.insert(cell.id, crate::pane::Decor { label: String::new(), output, stamp: h.finish() });
            }
            if let Some(re) = &m.pattern {
                let (a, b) = self.doc.cell_body(i);
                let body = &self.doc.buf.text[a..b];
                m.marks
                    .extend(re.find_iter(body).filter(|f| f.start() < f.end()).map(|f| (a + f.start(), a + f.end())));
            }
        }
        self.doc.multi = Some(m);
        self.doc.redraw = true;
    }

    /// Problems changed: excerpts take the problems in their lines, new
    /// excerpts show those outside them, and excerpts left without any go
    /// unless the cursor is in one. Returns whether the excerpts changed.
    fn multibuffer_refresh_problems(&mut self, m: &mut Multi) -> bool {
        let groups = self.problem_groups();
        let cursor = self.doc.excerpt_at(self.doc.buf.cur).map(|c| c.0);
        let mut found: Vec<(PathBuf, Vec<(usize, String)>)> = Vec::new();
        for (path, hits) in groups {
            let path = path.canonicalize().unwrap_or(path);
            if m.source(&path).is_none() && self.multibuffer_load(m, &path).is_none() {
                continue;
            }
            found.push((path, hits));
        }
        let mut keep = vec![false; m.excerpts.len()];
        for (i, x) in m.excerpts.iter_mut().enumerate() {
            let hits: Vec<&(usize, String)> = found
                .iter()
                .filter(|(p, _)| *p == x.path)
                .flat_map(|(_, h)| h)
                .filter(|h| (x.start..x.end).contains(&h.0))
                .collect();
            x.hits = hits.iter().map(|h| h.0).collect();
            x.notes = hits.iter().map(|h| format!("{}: {}", h.0 + 1, h.1)).collect();
            keep[i] = !hits.is_empty() || cursor == Some(i);
        }
        let mut k = keep.iter();
        m.excerpts.retain(|_| *k.next().unwrap());
        for (path, hits) in &found {
            let Some(text) = m.source(path).map(|s| s.text.clone()) else { continue };
            let outside: Vec<usize> = hits.iter().map(|h| h.0).filter(|&l| m.find(path, l).is_none()).collect();
            let first = m.excerpts.len();
            m.add_file(path, &text, &outside, mb::CONTEXT);
            for x in &mut m.excerpts[first..] {
                x.notes = hits
                    .iter()
                    .filter(|h| (x.start..x.end).contains(&h.0))
                    .map(|h| format!("{}: {}", h.0 + 1, h.1))
                    .collect();
            }
        }
        let order: Vec<&PathBuf> = found.iter().map(|(p, _)| p).collect();
        m.excerpts.sort_by_key(|x| (order.iter().position(|p| **p == x.path).unwrap_or(usize::MAX), x.start));
        m.sources.retain(|s| s.touched || m.excerpts.iter().any(|x| x.path == s.path));
        true
    }

    // ---- commands -------------------------------------------------------

    /// Ctrl+S in a multibuffer: saves every file it edited.
    pub(super) fn multibuffer_save(&mut self) {
        self.multibuffer_tick();
        let Some(mut m) = self.doc.multi.take() else { return };
        let (mut saved, mut failed) = (0, Vec::new());
        let conflict = || std::io::Error::other("changed on disk since it was read; reload it in its tab");
        for s in m.sources.iter_mut().filter(|s| s.touched) {
            let now = mb::disk_hash(std::fs::read(&s.path).ok().as_deref());
            let result = if let Some(buf) = &mut s.hidden {
                if !buf.modified {
                    continue;
                }
                if now != s.disk {
                    Err(conflict())
                } else {
                    let bytes = buf.eol.apply(&buf.text);
                    write_file(&s.path, bytes.as_bytes())
                        .map(|_| (buf.modified, s.disk) = (false, mb::disk_hash(Some(bytes.as_bytes()))))
                }
            } else if let Some(t) = self.multibuffer_tab(&s.path) {
                let known = mb::disk_hash(self.file_tabs[t].disk.bytes.as_deref());
                let pane = &mut self.file_tabs[t].state.as_mut().unwrap().pane;
                if !pane.buf.modified {
                    continue;
                }
                let done = if known != now {
                    Err(conflict())
                } else {
                    write_file(&s.path, pane.buf.eol.apply(&pane.buf.text).as_bytes())
                        .map(|_| pane.buf.modified = false)
                };
                if done.is_ok() {
                    self.disk_remember_tab(t);
                }
                done
            } else {
                continue;
            };
            match result {
                Ok(()) => saved += 1,
                Err(e) => failed.push(format!("{}: {e}", mb::short(&self.root, &s.path))),
            }
        }
        self.doc.multi = Some(m);
        self.doc.buf.modified = !failed.is_empty();
        self.tree.refresh();
        self.retitle = true;
        self.message = Some(if failed.is_empty() {
            format!("Saved {saved} file{}", if saved == 1 { "" } else { "s" })
        } else {
            format!("Cannot save {}", failed.join("; "))
        });
    }

    /// Keys of a multibuffer: Alt+Enter opens the file at the cursor, and
    /// Alt+PageUp and Alt+PageDown grow the excerpt.
    pub(super) fn multibuffer_key(&mut self, key: Key, mods: Mods) -> bool {
        if self.doc.multi.is_none() || !mods.alt || mods.ctrl || mods.shift {
            return false;
        }
        match key {
            Key::Enter => self.multibuffer_command("open"),
            Key::PageUp => self.multibuffer_command("expand_up"),
            Key::PageDown => self.multibuffer_command("expand_down"),
            _ => return false,
        }
        true
    }

    /// The palette's Multibuffer commands, by what follows `multibuffer_`.
    pub(super) fn multibuffer_command(&mut self, id: &str) {
        match id {
            "search" => return self.multibuffer_search(),
            "problems" => return self.multibuffer_problems(),
            "references" => return self.language_command("references_editor"),
            _ => {}
        }
        self.multibuffer_tick();
        let Some((cell, off)) = self.doc.excerpt_at(self.doc.buf.cur) else {
            self.message = Some("Put the cursor in a multibuffer's excerpt first".into());
            return;
        };
        match id {
            "open" => self.multibuffer_open(cell, Some(off)),
            "expand_up" => self.multibuffer_expand(cell, true),
            "expand_down" => self.multibuffer_expand(cell, false),
            _ => {}
        }
    }

    /// Opens an excerpt's file at a line of it, or at its first line of
    /// interest.
    fn multibuffer_open(&mut self, cell: usize, off: Option<usize>) {
        let Some(m) = &self.doc.multi else { return };
        let Some(x) = m.excerpts.get(cell) else { return };
        let path = x.path.clone();
        let line = match off {
            Some(off) => x.start + off.min(x.end - x.start - 1),
            None => x.hunk.as_ref().map(|h| h.added.0).or(x.hits.first().copied()).unwrap_or(x.start),
        };
        let col = off.map_or(0, |_| self.doc.buf.cur - self.doc.buf.starts[self.doc.buf.line_of(self.doc.buf.cur)]);
        self.open_tab(path.clone());
        if self.doc.path.as_deref() == Some(path.as_path()) && self.reader.is_none() {
            let (a, b) = self.doc.buf.line(line.min(self.doc.buf.lines() - 1));
            let at = editor::floor(&self.doc.buf.text, (a + col).min(b));
            self.doc.select(at, at);
            self.doc.moved();
        }
    }

    fn multibuffer_expand(&mut self, cell: usize, up: bool) {
        let Some(mut m) = self.doc.multi.take() else { return };
        let grown = m.expand(cell, up, mb::STEP);
        self.doc.multi = Some(m);
        if grown {
            self.multibuffer_rebuild(None);
        } else {
            self.message = Some(format!("The excerpt already reaches the file's {}", if up { "start" } else { "end" }));
        }
    }

    /// A click on an excerpt's header: its label opens the file, and its
    /// controls expand the excerpt or keep or revert a reviewed change.
    pub(super) fn multibuffer_press(&mut self, x: f32, y: f32) -> bool {
        if self.doc.multi.is_none() || self.welcomes() {
            return false;
        }
        let Some((cell, control)) = self.doc.header_hit(x, y) else { return false };
        self.focus = Focus::Doc;
        self.multibuffer_tick();
        let review = self.doc.multi.as_ref().is_some_and(|m| m.excerpts.get(cell).is_some_and(|x| x.hunk.is_some()));
        match (control, review) {
            (None, _) => self.multibuffer_open(cell, None),
            (Some(0), true) => self.review_excerpt(cell, true),
            (Some(1), true) => self.review_excerpt(cell, false),
            (Some(k), true) => self.multibuffer_expand(cell, k == 2),
            (Some(k), false) => self.multibuffer_expand(cell, k == 0),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_in(dir: &Path) -> App {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.root = dir.to_path_buf();
        a
    }

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-multibuffer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let text = (0..30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n") + "\n";
        std::fs::write(dir.join("a.rs"), &text).unwrap();
        std::fs::write(dir.join("b.py"), "x = 1\ny = 2\nz = 3\n").unwrap();
        dir.canonicalize().unwrap()
    }

    fn groups(dir: &Path) -> Vec<(PathBuf, Vec<(usize, String)>)> {
        vec![
            (dir.join("a.rs"), vec![(5, String::new()), (20, String::new())]),
            (dir.join("b.py"), vec![(1, String::new())]),
        ]
    }

    fn type_at(a: &mut App, at: usize, s: &str) {
        a.doc.select(at, at);
        for ch in s.chars() {
            a.key(Key::Char(ch), Mods::default(), Some(&ch.to_string()));
        }
        a.multibuffer_tick();
    }

    #[test]
    fn edits_reach_closed_files_and_ctrl_s_saves_them() {
        let dir = project("closed");
        let mut a = app_in(&dir);
        let m = a.multibuffer_from(Kind::Search, "Search: line", groups(&dir));
        a.open_multibuffer(m);
        assert_eq!(a.doc.cells.len(), 3, "{:?}", a.message);
        assert_eq!(a.doc.output_name.as_deref(), Some("Search: line"));
        // Line 6 of a.rs is the second of the first excerpt.
        let at = a.doc.excerpt_offset(0, 2, 0).unwrap();
        type_at(&mut a, at, "new ");
        assert!(a.doc.buf.modified);
        let m = a.doc.multi.as_ref().unwrap();
        let hidden = m.source(&dir.join("a.rs")).unwrap().hidden.as_ref().unwrap();
        assert!(hidden.text.contains("\nnew line 5\n") && hidden.modified);
        assert_eq!(std::fs::read_to_string(dir.join("a.rs")).unwrap().matches("new").count(), 0);
        // Typing is one step to undo; undo goes to the file's buffer.
        assert_eq!(m.undo.len(), 1);
        a.key(Key::Char('z'), Mods { ctrl: true, ..Default::default() }, None);
        a.multibuffer_tick();
        assert!(!a.doc.buf.text.contains("new line"));
        a.key(Key::Char('y'), Mods { ctrl: true, ..Default::default() }, None);
        a.multibuffer_tick();
        assert!(a.doc.buf.text.contains("new line 5"));
        a.key(Key::Char('s'), Mods { ctrl: true, ..Default::default() }, None);
        assert!(std::fs::read_to_string(dir.join("a.rs")).unwrap().contains("\nnew line 5\n"));
        assert!(!a.doc.buf.modified);
        assert!(a.message.as_deref().unwrap().starts_with("Saved 1 file"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_open_tab_shares_its_buffer_both_ways() {
        let dir = project("open");
        let mut a = app_in(&dir);
        a.open_tab(dir.join("a.rs"));
        a.file_tabs[a.active_file].pinned = true;
        let m = a.multibuffer_from(Kind::References, "References", groups(&dir));
        assert!(m.source(&dir.join("a.rs")).unwrap().hidden.is_none());
        a.open_multibuffer(m);
        let at = a.doc.excerpt_offset(0, 2, 0).unwrap();
        type_at(&mut a, at, "X");
        let tab = a.multibuffer_tab(&dir.join("a.rs")).unwrap();
        let pane = &a.file_tabs[tab].state.as_ref().unwrap().pane;
        assert!(pane.buf.text.contains("\nXline 5\n") && pane.buf.modified);
        // An edit in the tab moves the excerpts after it, and they follow.
        let multi = a.active_file;
        a.activate_file(tab);
        a.doc.select(0, 0);
        a.doc.insert("top\n", EditKind::Other);
        a.activate_file(multi);
        a.multibuffer_tick();
        let m = a.doc.multi.as_ref().unwrap();
        assert_eq!((m.excerpts[0].start, m.excerpts[0].end), (4, 9));
        assert!(a.doc.buf.text.contains("Xline 5"));
        assert_eq!(a.doc.excerpt_at(a.doc.excerpt_offset(1, 0, 0).unwrap()), Some((1, 0)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn excerpts_expand_from_keys_and_open_their_files() {
        let dir = project("expand");
        let mut a = app_in(&dir);
        let m = a.multibuffer_from(Kind::Search, "Search", groups(&dir));
        a.open_multibuffer(m);
        let at = a.doc.excerpt_offset(0, 0, 0).unwrap();
        a.doc.select(at, at);
        a.key(Key::PageUp, Mods { alt: true, ..Default::default() }, None);
        let m = a.doc.multi.as_ref().unwrap();
        assert_eq!((m.excerpts[0].start, m.excerpts[0].end), (0, 8));
        assert_eq!(a.doc.excerpt_at(a.doc.buf.cur).map(|c| c.0), Some(0));
        a.key(Key::Enter, Mods { alt: true, ..Default::default() }, None);
        assert_eq!(a.doc.path.as_deref(), Some(dir.join("a.rs").as_path()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
