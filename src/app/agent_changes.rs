//! Agent edit checkpoints. Restore only when the current contents still match the agent's result.
use super::*;
use crate::tools;
use std::collections::{BTreeMap, VecDeque};
use std::os::unix::fs::PermissionsExt;

#[derive(Clone, PartialEq)]
struct Saved {
    bytes: Option<Arc<[u8]>>,
    mode: u32,
}
impl Saved {
    fn read(path: &Path) -> Result<Self, String> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                Err(format!("Checkpoint refuses symbolic link {}", path.display()))
            }
            Ok(meta) if meta.is_file() => Ok(Self {
                bytes: Some(std::fs::read(path).map_err(|e| e.to_string())?.into()),
                mode: meta.permissions().mode(),
            }),
            Ok(_) => Err(format!("Not a regular file: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self { bytes: None, mode: 0o644 }),
            Err(e) => Err(e.to_string()),
        }
    }
    fn restore(&self, path: &Path) -> Result<(), String> {
        if let Some(bytes) = &self.bytes {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            super::write_file(path, bytes).map_err(|e| e.to_string())?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(self.mode)).map_err(|e| e.to_string())?;
        } else if path.exists() {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
#[derive(Clone)]
struct Buffer {
    tab: u64,
    text: String,
    modified: bool,
    cur: usize,
    anchor: usize,
    notebook: Option<String>,
}
impl PartialEq for Buffer {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.modified == other.modified && self.notebook == other.notebook
    }
}
#[derive(Clone)]
struct Change {
    before: Saved,
    after: Saved,
    buffer_before: Option<Buffer>,
    buffer_after: Option<Buffer>,
    kept: bool,
    reviewed: Option<String>,
}
#[derive(Clone)]
pub(super) struct Checkpoint {
    thread: String,
    item: usize,
    title: String,
    root: PathBuf,
    changes: BTreeMap<PathBuf, Change>,
    command_before: Option<BTreeMap<PathBuf, Saved>>,
}
#[derive(Default)]
pub(super) struct Reviews {
    turns: VecDeque<Checkpoint>,
    active: bool,
    selected: Option<PathBuf>,
    selected_turn: Option<usize>,
}
impl Reviews {
    fn current(&self) -> Option<&Checkpoint> {
        self.turns.get(self.selected_turn.unwrap_or_else(|| self.turns.len().saturating_sub(1)))
    }
    fn current_mut(&mut self) -> Option<&mut Checkpoint> {
        let i = self.selected_turn.unwrap_or_else(|| self.turns.len().saturating_sub(1));
        self.turns.get_mut(i)
    }
}

// Build products and dependencies are outside source checkpoints. Explicit file edits to
// ignored files still get their own checkpoint. Never traverse symlinks or Git metadata.
fn source_files(root: &Path) -> Result<BTreeMap<PathBuf, Saved>, String> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Saved>, size: &mut usize) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() || name == ".git" {
                continue;
            }
            if meta.is_dir() {
                if !["target", "node_modules", ".venv", "venv", "__pycache__", ".cache"].contains(&name.as_ref()) {
                    walk(&path, out, size)?;
                }
            } else if meta.is_file() {
                *size += meta.len() as usize;
                if *size > 64 << 20 || out.len() >= 10000 {
                    return Err(
                        "Source checkpoint exceeds 64 MB or 10000 files; use file edit tools for this project".into()
                    );
                }
                out.insert(path.clone(), Saved::read(&path)?);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out, &mut 0)?;
    Ok(out)
}
impl App {
    fn agent_buffer(&self, path: &Path) -> Option<Buffer> {
        let untitled_tab = self
            .ai
            .reviews
            .current()
            .and_then(|t| t.changes.get(path))
            .and_then(|c| c.buffer_before.as_ref())
            .map(|b| b.tab);
        let untitled_active = path == Path::new(crate::tools::OPEN_DOC)
            && !self.doc.read_only
            && self.doc.path.is_none()
            && untitled_tab.is_none_or(|id| id == self.file_tabs[self.active_file].id);
        let pane_nb = if self.doc.path.as_deref() == Some(path) || untitled_active {
            Some((&self.doc, self.nb.as_ref(), self.file_tabs[self.active_file].id))
        } else {
            self.file_tabs
                .iter()
                .filter_map(|t| t.state.as_ref().map(|s| (s, t.id)))
                .find(|(s, id)| {
                    s.pane.path.as_deref() == Some(path)
                        || path == Path::new(crate::tools::OPEN_DOC)
                            && !s.pane.read_only
                            && s.pane.path.is_none()
                            && Some(*id) == untitled_tab
                })
                .map(|(s, id)| (&s.pane, s.notebook.as_ref(), id))
        };
        pane_nb.map(|(p, nb, tab)| Buffer {
            tab,
            text: p.buf.text.clone(),
            modified: p.buf.modified,
            cur: p.buf.cur,
            anchor: p.buf.anchor,
            notebook: nb.map(|n| n.to_json(p)),
        })
    }
    pub(super) fn checkpoint_active(&self) -> bool { self.ai.reviews.active }
    pub(super) fn begin_agent_turn(&mut self) {
        self.ai.reviews.active = true;
        self.ai.reviews.selected_turn = None;
        self.ai.reviews.turns.push_back(Checkpoint {
            thread: self.ai.thread.id.clone(),
            item: self.ai.thread.items.len(),
            title: crate::tools::clip_line(&self.ai.side.input.buf.text, 60),
            root: self.root.clone(),
            changes: BTreeMap::new(),
            command_before: None,
        });
        while self.ai.reviews.turns.len() > 10 {
            self.ai.reviews.turns.pop_front();
        }
    }
    fn ensure_agent_turn(&mut self) {
        if !self.ai.reviews.active || self.ai.reviews.current().is_none_or(|c| c.root != self.root) {
            self.begin_agent_turn();
        }
    }
    pub(super) fn checkpoint_file(&mut self, path: &Path) -> Result<(), String> {
        self.ensure_agent_turn();
        if self.ai.reviews.current().unwrap().changes.contains_key(path) {
            return Ok(());
        }
        let before = if path == Path::new(crate::tools::OPEN_DOC) {
            Saved { bytes: None, mode: 0o644 }
        } else {
            Saved::read(path)?
        };
        let buffer = self.agent_buffer(path);
        self.ai.reviews.current_mut().unwrap().changes.insert(
            path.into(),
            Change {
                before: before.clone(),
                after: before,
                buffer_before: buffer.clone(),
                buffer_after: buffer,
                kept: false,
                reviewed: None,
            },
        );
        Ok(())
    }
    pub(super) fn checkpoint_command(&mut self) -> Result<(), String> {
        self.ensure_agent_turn();
        if self.ai.reviews.current().unwrap().command_before.is_none() {
            let files = source_files(&self.root)?;
            let paths: Vec<_> = files.keys().cloned().collect();
            for path in paths {
                self.checkpoint_file(&path)?;
            }
            self.ai.reviews.current_mut().unwrap().command_before = Some(files);
        }
        Ok(())
    }
    pub(super) fn refresh_agent_changes(&mut self) {
        let Some(checkpoint) = self.ai.reviews.current() else { return };
        if checkpoint.root != self.root {
            return;
        }
        if let Some(before) = &checkpoint.command_before {
            match source_files(&self.root) {
                Ok(after) => {
                    let added: Vec<_> = after.keys().filter(|p| !before.contains_key(*p)).cloned().collect();
                    for path in added {
                        self.ai.reviews.current_mut().unwrap().changes.entry(path).or_insert_with(|| {
                            let empty = Saved { bytes: None, mode: 0o644 };
                            Change {
                                before: empty.clone(),
                                after: empty,
                                buffer_before: None,
                                buffer_after: None,
                                kept: false,
                reviewed: None,
                            }
                        });
                    }
                }
                Err(why) => {
                    self.message = Some(format!("Cannot scan command changes: {why}"));
                }
            }
        }
        let paths: Vec<_> = self.ai.reviews.current().unwrap().changes.keys().cloned().collect();
        for path in paths {
            let buffer = self.agent_buffer(&path);
            let saved = if path == Path::new(crate::tools::OPEN_DOC) {
                Ok(Saved { bytes: None, mode: 0o644 })
            } else {
                Saved::read(&path)
            };
            if let Ok(saved) = saved {
                let change = self.ai.reviews.current_mut().unwrap().changes.get_mut(&path).unwrap();
                change.after = if saved == change.before { change.before.clone() } else { saved };
                change.buffer_after = buffer;
            }
        }
    }
    /// Whether the agent turn in progress has changed any file.
    pub(super) fn agent_turn_changed(&self) -> bool {
        self.ai.reviews.active && self.ai.reviews.current().is_some_and(|c| {
            c.root == self.root && c.changes.values().any(|c| c.before != c.after || c.buffer_before != c.buffer_after)
        })
    }
    pub(super) fn finish_agent_turn(&mut self) {
        if let Some(c) = self.ai.reviews.current_mut() {
            c.command_before = None;
            c.changes.retain(|_, c| c.before != c.after || c.buffer_before != c.buffer_after);
        }
        self.ai.reviews.active = false;
    }
    pub(super) fn open_agent_turns(&mut self) {
        let entries = self
            .ai
            .reviews
            .turns
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, c)| c.root == self.root && c.thread == self.ai.thread.id)
            .map(|(i, c)| {
                Entry::new(
                    &i.to_string(),
                    &format!("Turn {}: {}", i + 1, if c.title.is_empty() { "Agent turn" } else { &c.title }),
                    &format!("{} changed files", c.changes.len()),
                )
            })
            .collect();
        self.menu = Some((MenuKind::AgentTurns, Menu::new("Agent turn checkpoints", entries, "")));
    }
    pub(super) fn pick_agent_turn(&mut self, key: &str) {
        let Some(index) =
            key.parse::<usize>().ok().filter(|&i| self.ai.reviews.turns.get(i).is_some_and(|c| c.root == self.root && c.thread == self.ai.thread.id))
        else {
            return;
        };
        self.ai.reviews.selected_turn = Some(index);
        self.open_agent_changes();
    }
    pub(super) fn rollback_latest_agent_turn(&mut self) {
        self.ai.reviews.selected_turn = None;
        self.rollback_agent_turn();
    }
    pub(super) fn open_agent_changes(&mut self) {
        if self.ai.busy() {
            self.message = Some("Finish or stop the agent before reviewing changes".into());
            return;
        }
        let mut entries: Vec<Entry> = self
            .ai
            .reviews
            .current()
            .filter(|c| c.root == self.root && c.thread == self.ai.thread.id)
            .map(|c| {
                c.changes
                    .iter()
                    .filter(|(_, c)| !c.kept && (c.before != c.after || c.buffer_before != c.buffer_after))
                    .map(|(p, _)| {
                        Entry::new(
                            &p.to_string_lossy(),
                            &crate::tools::short(&self.root, p),
                            "View diff, Keep or Revert",
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !entries.is_empty() {
            entries.insert(
                0,
                Entry::new("action:rollback", "Roll back this turn", "Restore all files in this checkpoint"),
            );
        }
        self.menu = Some((MenuKind::AgentChanges, Menu::new("Agent changes", entries, "")));
    }
    pub(super) fn pick_agent_change(&mut self, key: &str) {
        if key == "action:rollback" {
            self.rollback_agent_turn();
            return;
        }
        if key == "hunks" {
            self.open_agent_hunks();
            return;
        }
        if ["keep", "revert", "diff"].contains(&key) {
            let Some(path) = self.ai.reviews.selected.clone() else { return };
            if key == "diff" {
                let Some(c) = self.ai.reviews.current().and_then(|t| t.changes.get(&path)).cloned() else { return };
                let before = c
                    .buffer_before
                    .as_ref()
                    .map(|b| b.notebook.as_deref().unwrap_or(&b.text))
                    .map(str::as_bytes)
                    .or(c.before.bytes.as_deref())
                    .unwrap_or_default();
                let after = c
                    .buffer_after
                    .as_ref()
                    .map(|b| b.notebook.as_deref().unwrap_or(&b.text))
                    .map(str::as_bytes)
                    .or(c.after.bytes.as_deref())
                    .unwrap_or_default();
                match (std::str::from_utf8(before), std::str::from_utf8(after)) {
                    (Ok(a), Ok(b)) => match repository::text_diff(&path, a, b, 3) {
                        Ok(text) => self.open_output(text),
                        Err(e) => self.message = Some(e),
                    },
                    _ => self.open_output(format!("Binary change: {} bytes to {} bytes", before.len(), after.len())),
                }
            } else if key == "keep" {
                if let Some(c) = self.ai.reviews.current_mut().and_then(|t| t.changes.get_mut(&path)) {
                    c.kept = true;
                }
                self.open_agent_changes();
            } else {
                match self.restore_agent_files(&[path]) {
                    Ok(()) => self.open_agent_changes(),
                    Err(e) => self.message = Some(e),
                }
            }
            return;
        }
        let path = PathBuf::from(key);
        self.ai.reviews.selected = Some(path.clone());
        self.menu = Some((
            MenuKind::AgentChange,
            Menu::new(
                &format!("Agent: {}", crate::tools::short(&self.root, &path)),
                vec![
                    Entry::new("hunks", "Review hunks in editor", "Keep or undo each added and removed block"),
                    Entry::new("diff", "View diff", "Before and after this turn"),
                    Entry::new("keep", "Keep", "Accept this file"),
                    Entry::new("revert", "Revert", "Restore this file before the turn"),
                ],
                "diff",
            ),
        ));
    }
    fn restore_agent_files(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        if self.ai.busy() {
            return Err("Stop the agent before rolling back".into());
        }
        let c = self.ai.reviews.current().filter(|c| c.root == self.root && c.thread == self.ai.thread.id).ok_or("No checkpoint for this project")?;
        let changes: Vec<_> = paths.iter().filter_map(|p| c.changes.get(p).map(|c| (p.clone(), c.clone()))).collect();
        // Validate every file before writing any, including unsaved editor contents.
        for (path, c) in &changes {
            if path != Path::new(crate::tools::OPEN_DOC) {
                let located = crate::tools::locate(&self.root, &path.to_string_lossy())?;
                if located != *path || Saved::read(path)? != c.after {
                    return Err(format!("{} changed since the agent edited it; rollback refused", path.display()));
                }
            }
            let current = self.agent_buffer(path);
            if current.is_none() && c.buffer_before.as_ref().is_some_and(|b| b.modified) {
                return Err(format!("Reopen {} to restore its unsaved checkpoint", path.display()));
            }
            if current.as_ref().map(|b| (&b.text, &b.notebook))
                != c.buffer_after.as_ref().map(|b| (&b.text, &b.notebook))
            {
                // A clean buffer may have opened/reloaded since the edit. Compare its saved representation.
                if current.as_ref().is_some_and(|b| {
                    b.modified
                        || b.notebook.as_deref().unwrap_or(&b.text).replace("\r\n", "\n").as_bytes()
                            != c.after.bytes.as_deref().unwrap_or_default()
                }) {
                    return Err(format!("{} has later editor changes; rollback refused", path.display()));
                }
            }
        }
        for (path, c) in &changes {
            if path != Path::new(crate::tools::OPEN_DOC) {
                c.before.restore(path)?;
            }
            for i in 0..self.file_tabs.len() {
                let tab_id = self.file_tabs[i].id;
                let (pane, nb) = if i == self.active_file {
                    (&mut self.doc, &mut self.nb)
                } else {
                    let s = self.file_tabs[i].state.as_mut().unwrap();
                    (&mut s.pane, &mut s.notebook)
                };
                if pane.path.as_deref() != Some(path)
                    && !(path == Path::new(crate::tools::OPEN_DOC)
                        && pane.path.is_none()
                        && !pane.read_only
                        && c.buffer_before.as_ref().is_some_and(|b| b.tab == tab_id))
                {
                    continue;
                }
                let b = c.buffer_before.clone().unwrap_or_else(|| Buffer {
                    tab: tab_id,
                    text: String::from_utf8_lossy(c.before.bytes.as_deref().unwrap_or_default()).replace("\r\n", "\n"),
                    notebook: None,
                    cur: 0,
                    anchor: 0,
                    modified: c.before.bytes.is_none(),
                });
                if let Some(json) = b.notebook.as_ref().or_else(|| (pane.doc == Doc::Cells).then_some(&b.text)) {
                    let (mut restored, text, tags) = Notebook::open(json, self.pics.clone())?;
                    pane.set_text(text, Doc::Cells);
                    restored.attach(pane, tags);
                    *nb = Some(restored);
                } else {
                    pane.edit(0, pane.buf.text.len(), &b.text, EditKind::Other);
                }
                pane.select(editor::floor(&pane.buf.text, b.anchor), editor::floor(&pane.buf.text, b.cur));
                pane.buf.modified = b.modified;
                pane.moved();
            }
            if let Some(c) = self.ai.reviews.current_mut().and_then(|t| t.changes.get_mut(path)) {
                c.kept = true;
                c.after = c.before.clone();
                c.buffer_after = c.buffer_before.clone();
            }
        }
        let tabs: Vec<_> = self
            .tab_files()
            .iter()
            .enumerate()
            .filter(|(_, file)| file.as_ref().is_some_and(|(p, _)| changes.iter().any(|(path, _)| path == p)))
            .map(|(i, _)| i)
            .collect();
        for tab in tabs {
            self.disk_remember_tab(tab);
        }
        self.disk_poll(true);
        self.tree.refresh();
        self.edited();
        self.message = Some(format!("Restored {} files from the checkpoint", changes.len()));
        Ok(())
    }
    pub(super) fn rollback_agent_turn(&mut self) {
        let paths: Vec<_> = self
            .ai
            .reviews
            .current()
            .filter(|c| c.root == self.root && c.thread == self.ai.thread.id)
            .map(|c| {
                c.changes
                    .iter()
                    .filter(|(_, c)| c.before != c.after || c.buffer_before != c.buffer_after)
                    .map(|(p, _)| p.clone())
                    .collect()
            })
            .unwrap_or_default();
        if paths.is_empty() {
            self.message = Some("No agent changes to roll back".into());
            return;
        }
        match self.restore_agent_files(&paths) {
            Ok(()) => {
                let index = self.ai.reviews.selected_turn.take().unwrap_or(self.ai.reviews.turns.len() - 1);
                self.ai.reviews.turns.remove(index);
                self.ai.reviews.active = false;
            }
            Err(e) => self.message = Some(e),
        }
    }
}

#[derive(Clone, Debug)]
struct Hunk {
    old: std::ops::Range<usize>,
    new: std::ops::Range<usize>,
}
fn line_range(text: &str, start: usize, count: usize) -> std::ops::Range<usize> {
    let offsets: Vec<_> = text.split_inclusive('\n').scan(0, |n, l| { let at = *n; *n += l.len(); Some(at) }).collect();
    let first = if count == 0 { start } else { start.saturating_sub(1) };
    offsets.get(first).copied().unwrap_or(text.len())..offsets.get(first + count).copied().unwrap_or(text.len())
}
fn hunks(before: &str, after: &str) -> Result<(String, Vec<Hunk>), String> {
    let diff = repository::diff(before, after, 0)?;
    let mut out = String::new();
    let mut hunks = Vec::new();
    for line in diff.lines().skip(4) {
        if line.starts_with("@@ ") {
            let parts: Vec<_> = line.split_whitespace().collect();
            let range = |part: &str, text: &str| -> Result<_, String> {
                let (a, b) = part[1..].split_once(',').unwrap_or((&part[1..], "1"));
                Ok(line_range(text, a.parse().map_err(|_| "Invalid diff range")?, b.parse().map_err(|_| "Invalid diff count")?))
            };
            out += &format!("{line}    Keep / Undo\n");
            hunks.push(Hunk { old: range(parts[1], before)?, new: range(parts[2], after)? });
        } else {
            out += line;
            out.push('\n');
        }
    }
    Ok((out, hunks))
}
impl Change {
    fn text(&self, before: bool) -> Result<String, String> {
        let (buffer, saved) = if before { (&self.buffer_before, &self.before) } else { (&self.buffer_after, &self.after) };
        if let Some(b) = buffer {
            if b.notebook.is_some() { return Err("Use per-file review for notebook changes".into()); }
            return Ok(b.text.clone());
        }
        String::from_utf8(saved.bytes.as_deref().unwrap_or_default().to_vec()).map_err(|_| "Use per-file review for binary changes".into())
    }
}
impl App {
    /// Review hunks in editor: every text file of the turn, its changes
    /// as excerpts to edit, keep or revert.
    pub(super) fn open_agent_hunks(&mut self) {
        self.multibuffer_review();
    }
    /// Keeps or reverts the review's hunk `index`, or the one at the cursor.
    pub(super) fn agent_hunk(&mut self, index: Option<usize>, keep: bool) {
        let review = self.doc.multi.as_ref().is_some_and(|m| m.kind == crate::multibuffer::Kind::Review);
        let cell = index.or_else(|| self.doc.excerpt_at(self.doc.buf.cur).map(|c| c.0));
        match cell.filter(|_| review) {
            Some(cell) => self.review_excerpt(cell, keep),
            None => self.message = Some("Open Review hunks in editor and put the cursor in a hunk".into()),
        }
    }
    /// Each text file the current turn changed, with what it is reviewed
    /// against: the file before the turn, with the hunks kept since.
    pub(super) fn review_baselines(&self) -> Result<Vec<(PathBuf, String)>, String> {
        if self.ai.busy() { return Err("Finish the agent turn before reviewing hunks".into()); }
        let turn = self.ai.reviews.current().ok_or("No agent changes to review")?;
        let mut out: Vec<_> = turn.changes.iter().filter(|(p, _)| p.as_path() != Path::new(tools::OPEN_DOC))
            .filter_map(|(p, c)| Some((p.clone(), c.reviewed.clone().map(Ok).unwrap_or_else(|| c.text(true)).ok()?)))
            .collect();
        if let Some(selected) = &self.ai.reviews.selected { out.sort_by_key(|(p, _)| p != selected); }
        Ok(out)
    }
    /// Keeps a hunk: lines `old` of the file's baseline, which must still
    /// be `removed`, become `kept`, so it leaves the review.
    pub(super) fn review_keep(&mut self, path: &Path, old: (usize, usize), removed: &[String], kept: &[String]) -> Result<(), String> {
        let turn = self.ai.reviews.current_mut().ok_or("Checkpoint expired")?;
        let key = turn.changes.keys().find(|k| k.as_path() == path || k.canonicalize().ok().as_deref() == Some(path)).cloned();
        let c = key.and_then(|k| turn.changes.get_mut(&k)).ok_or("Checkpoint expired")?;
        let base = c.reviewed.clone().map(Ok).unwrap_or_else(|| c.text(true))?;
        let mut lines: Vec<&str> = base.split('\n').collect();
        if lines.get(old.0..old.1).is_none_or(|l| l.len() != removed.len() || l.iter().zip(removed).any(|(a, b)| a != b)) {
            return Err("This file was reviewed elsewhere since; open the review again".into());
        }
        lines.splice(old.0..old.1, kept.iter().map(String::as_str));
        c.reviewed = Some(lines.join("\n"));
        Ok(())
    }
}
/// The changes between two texts by lines: lines of `before` replaced,
/// lines of `after` written, and the lines removed.
pub(super) fn line_hunks(before: &str, after: &str) -> Result<Vec<((usize, usize), (usize, usize), Vec<String>)>, String> {
    let lines = |text: &str, r: &std::ops::Range<usize>| {
        let first = text[..r.start].matches('\n').count();
        (first, first + text[r.clone()].split_inclusive('\n').count())
    };
    Ok(hunks(before, after)?.1.iter().map(|h| {
        let removed = before[h.old.clone()].split_inclusive('\n').map(|l| l.strip_suffix('\n').unwrap_or(l).to_string()).collect();
        (lines(before, &h.old), lines(after, &h.new), removed)
    }).collect())
}
#[cfg(test)]
mod hunk_tests {
    use super::*;
    #[test]
    fn independent_hunks_and_missing_final_newline() {
        let before = "α\nb\nc\nd\ne\nlast";
        let after = "α\nB\nc\nd\ne\nlast\nnew";
        let (_, hs) = hunks(before, after).unwrap();
        assert_eq!(hs.len(), 2);
        let mut restored = after.to_string();
        for h in hs.iter().rev() { restored.replace_range(h.new.clone(), &before[h.old.clone()]); }
        assert_eq!(restored, before);
        let (_, hs) = hunks("", "hello").unwrap();
        assert_eq!(hs[0].new, 0..5);
        assert_eq!(hs[0].old, 0..0);
    }
    #[test]
    fn fork_restores_later_turns_and_edit_restores_before_message() {
        let dir = std::env::temp_dir().join(format!("lightdown-rewind-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let path = dir.join("file.txt");
        std::fs::write(&path, "zero").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(900, 700, 1.0);
        for text in ["one", "two", "three"] {
            a.begin_agent_turn();
            a.ai.thread.items.push(agent::Item::User { text: text.into(), context: String::new(), images: Vec::new() });
            a.checkpoint_file(&path).unwrap();
            std::fs::write(&path, text).unwrap();
            a.doc.set_text(text.into(), Doc::Text);
            a.refresh_agent_changes(); a.finish_agent_turn();
        }
        let old_id = a.ai.thread.id.clone();
        a.pick_agent_history("fork:0");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        assert_ne!(a.ai.thread.id, old_id);
        assert_eq!(a.ai.thread.items.len(), 1);
        a.pick_agent_history("edit:0");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "zero");
        assert!(a.ai.thread.items.is_empty());
        assert_eq!(a.ai.side.input.buf.text, "one");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hunks_keep_and_revert_in_an_editable_review_and_saving_refuses_later_disk_edits() {
        let dir = std::env::temp_dir().join(format!("lightdown-hunks-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let path = dir.join("file.txt");
        let before = "one\ntwo\nthree\nfour\nfive\nsix\n";
        let after = "ONE\ntwo\nthree\nfour\nfive\nSIX\n";
        std::fs::write(&path, before).unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(900, 700, 1.0);
        a.begin_agent_turn();
        a.checkpoint_file(&path).unwrap();
        std::fs::write(&path, after).unwrap();
        a.doc.set_text(after.into(), Doc::Text);
        a.disk_remember();
        a.refresh_agent_changes();
        a.finish_agent_turn();
        a.open_agent_hunks();
        assert_eq!(a.doc.multi.as_ref().map(|m| m.excerpts.len()), Some(2));
        a.agent_hunk(Some(0), true);
        a.agent_hunk(Some(0), false);
        assert_eq!(a.doc.multi.as_ref().map(|m| m.excerpts.len()), Some(0));
        a.key(Key::Char('s'), Mods { ctrl: true, ..Default::default() }, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ONE\ntwo\nthree\nfour\nfive\nsix\n");
        // A saved revert is a later edit, so the next turn starts from here.
        std::fs::write(&path, before).unwrap();
        a.activate_file(0);
        a.doc.set_text(before.into(), Doc::Text);
        a.disk_remember();
        a.begin_agent_turn();
        a.checkpoint_file(&path).unwrap();
        std::fs::write(&path, after).unwrap();
        a.doc.set_text(after.into(), Doc::Text);
        a.disk_remember();
        a.refresh_agent_changes();
        a.finish_agent_turn();
        a.open_agent_hunks();
        std::fs::write(&path, "later").unwrap();
        a.agent_hunk(Some(0), false);
        a.key(Key::Char('s'), Mods { ctrl: true, ..Default::default() }, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "later");
        assert!(a.message.as_deref().unwrap().contains("changed on disk"));
        std::fs::remove_dir_all(dir).unwrap();
    }

}

impl App {
    pub(super) fn open_agent_history(&mut self, fork: bool) {
        if self.ai.busy() { self.message = Some("Finish or stop the agent first".into()); return; }
        let entries = self.ai.thread.items.iter().enumerate().filter_map(|(i, item)| {
            if let agent::Item::User { text, .. } = item {
                Some(Entry::new(&format!("{}:{i}", if fork { "fork" } else { "edit" }), &tools::clip_line(text, 80), if fork { "Fork after this turn and restore its files" } else { "Restore before this turn and edit its message" }))
            } else { None }
        }).collect();
        self.menu = Some((MenuKind::AgentHistory, Menu::new("Conversation turns", entries, "")));
    }
    // Combine checkpoints and validate every final snapshot before restoring any file.
    fn restore_conversation(&mut self, from: usize) -> Result<(), String> {
        if from >= self.ai.thread.items.len() { return Ok(()); }
        let turns: Vec<_> = self.ai.reviews.turns.iter().filter(|c| c.root == self.root && c.thread == self.ai.thread.id && c.item >= from).cloned().collect();
        let users: Vec<_> = self.ai.thread.items.iter().enumerate().skip(from).filter(|(_, i)| matches!(i, agent::Item::User { .. })).map(|(i, _)| i).collect();
        if users.iter().any(|i| !turns.iter().any(|c| c.item == *i)) {
            return Err("A checkpoint for this turn is unavailable. Only the last ten turns in this window can restore files; queued follow-ups share their parent turn.".into());
        }
        let mut combined = Checkpoint { thread: self.ai.thread.id.clone(), item: from, title: String::new(), root: self.root.clone(), changes: BTreeMap::new(), command_before: None };
        for turn in turns {
            for (p, c) in turn.changes {
                combined.changes.entry(p).and_modify(|old: &mut Change| {
                    old.after = c.after.clone(); old.buffer_after = c.buffer_after.clone();
                }).or_insert(c);
            }
        }
        let paths = combined.changes.keys().cloned().collect::<Vec<_>>();
        let selected = self.ai.reviews.selected_turn.take();
        self.ai.reviews.turns.push_back(combined);
        let result = self.restore_agent_files(&paths);
        self.ai.reviews.turns.pop_back();
        self.ai.reviews.selected_turn = selected;
        result
    }
    pub(super) fn pick_agent_history(&mut self, key: &str) {
        let result = (|| {
            if self.ai.busy() { return Err("Finish or stop the agent first".into()); }
            if !self.ai.side.input.buf.text.is_empty() || !self.ai.images.is_empty() { return Err("Send or clear the current draft first".into()); }
            let (action, i) = key.split_once(':').ok_or("Invalid turn")?;
            let i: usize = i.parse().map_err(|_| "Invalid turn")?;
            let Some(agent::Item::User { text, images, .. }) = self.ai.thread.items.get(i).cloned() else { return Err("Turn no longer exists".into()); };
            let fork = action == "fork";
            let cut = if fork { self.ai.thread.items.iter().enumerate().skip(i + 1).find(|(_, i)| matches!(i, agent::Item::User { .. })).map_or(self.ai.thread.items.len(), |(i, _)| i) } else { i };
            self.restore_conversation(cut)?;
            self.ai.thread.save(&self.root);
            if fork {
                let mut thread = agent::Thread::new();
                thread.title = format!("Fork: {}", self.ai.thread.title);
                thread.items = self.ai.thread.items[..cut].to_vec();
                thread.backend = self.ai.thread.backend.clone();
                let checkpoints: Vec<_> = self.ai.reviews.turns.iter().filter(|c| c.thread == self.ai.thread.id && c.item < cut).cloned().map(|mut c| { c.thread = thread.id.clone(); c }).collect();
                self.ai.reviews.turns.extend(checkpoints);
                while self.ai.reviews.turns.len() > 10 { self.ai.reviews.turns.pop_front(); }
                self.ai.thread = thread;
            } else {
                self.ai.thread.items.truncate(cut);
                self.ai.thread.session.clear();
                self.ai.thread.summary.clear();
                self.ai.thread.summarized = 0;
                self.ai.side.input.set_text(text, Doc::Text);
                self.ai.images = images;
                self.ai.reviews.turns.retain(|c| c.thread != self.ai.thread.id || c.item < cut);
            }
            self.ai.thread.filled = 0;
            self.ai.proposed_plan = None;
            self.ai.reviews.selected_turn = None;
            self.ai.thread.save(&self.root);
            self.show_thread();
            self.show_side(Some(Tab::Agent));
            self.focus = Focus::Chat;
            self.message = Some(if fork { "Thread forked; files restored to that turn" } else { "Files restored. Edit the message and press Enter to resend" }.into());
            Ok::<_, String>(())
        })();
        if let Err(e) = result { self.message = Some(e); }
    }
}
