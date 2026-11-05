//! The folder view: the project's files as a tree, shown in the sidebar in
//! place of the conversation. A folder is read when it is unfolded, and
//! again whenever something may have changed in it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::app::Key;
use crate::app::git::Change;
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::icons::{self, Icon};
use crate::ui::px;
use crate::{theme, ui};

/// How many of a folder's entries are read. One that holds more says so
/// rather than being listed whole.
pub const MOST: usize = 2000;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Folder,
    File,
    /// Stands for what a folder too big to list holds besides.
    More,
}

struct Row {
    path: PathBuf,
    name: String,
    depth: usize,
    kind: Kind,
}

pub struct Tree {
    root: PathBuf,
    /// Every folder of the window, each listed as a folder of its own when
    /// there are several; `root` is the first.
    roots: Vec<PathBuf>,
    /// The file open in the editor, which is marked.
    current: Option<PathBuf>,
    /// The folders that stand unfolded.
    unfolded: HashSet<PathBuf>,
    rows: Vec<Row>,
    sel: usize,
    /// The folders are to be read again before the rows are next used.
    stale: bool,
    /// The open file is to be selected once the rows are read.
    seek: bool,
    /// The selected row is to be scrolled into view.
    reveal: bool,
    /// How far down the list is scrolled, in pixels.
    scroll: f32,
    fonts: Rc<RefCell<Fonts>>,
    rect: (f32, f32, f32, f32),
    scale: f32,
    /// Where the pointer is, for lighting the row under it.
    pub hover: Option<(f32, f32)>,
    /// What git says has changed, by path, and the folders holding it.
    changes: HashMap<PathBuf, Change>,
    changed_dirs: HashMap<PathBuf, Change>,
    ignored: HashSet<PathBuf>,
    /// What a folder of several is called, when not by its own name.
    pub names: HashMap<PathBuf, String>,
}

/// What a folder holds, folders first, and whether there was more than is
/// read. The repository's own records are left out.
pub fn entries(dir: &Path) -> (Vec<(String, bool)>, bool) {
    let Ok(read) = std::fs::read_dir(dir) else { return (Vec::new(), false) };
    let mut read = read.flatten();
    let entry = |e: std::fs::DirEntry| {
        let name = e.file_name().into_string().ok()?;
        let kind = e.file_type().ok()?;
        let folder = kind.is_dir() || (kind.is_symlink() && e.path().is_dir());
        (name != ".git").then_some((name, folder))
    };
    let mut found: Vec<(String, bool)> = read.by_ref().take(MOST).filter_map(entry).collect();
    found.sort_by_cached_key(|(name, folder)| (!folder, name.to_lowercase(), name.clone()));
    (found, read.next().is_some())
}

impl Tree {
    pub fn new(fonts: Rc<RefCell<Fonts>>) -> Tree {
        Tree {
            root: PathBuf::new(),
            roots: Vec::new(),
            current: None,
            unfolded: HashSet::new(),
            rows: Vec::new(),
            sel: 0,
            stale: true,
            seek: false,
            reveal: false,
            scroll: 0.0,
            fonts,
            rect: (0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
            hover: None,
            changes: HashMap::new(),
            changed_dirs: HashMap::new(),
            ignored: HashSet::new(),
            names: HashMap::new(),
        }
    }

    /// Takes in what git says has changed.
    pub fn set_changes(&mut self, changes: HashMap<PathBuf, Change>) {
        // A folder shows the weightiest change it holds.
        let weight = |c: Change| match c {
            Change::Conflict => 3,
            Change::Modified | Change::Renamed | Change::Deleted => 2,
            Change::Added | Change::Untracked => 1,
        };
        self.changed_dirs.clear();
        for (path, &change) in &changes {
            for dir in path.ancestors().skip(1) {
                let held = self.changed_dirs.entry(dir.to_path_buf()).or_insert(change);
                if weight(change) > weight(*held) {
                    *held = change;
                }
            }
        }
        self.changes = changes;
    }

    pub fn set_ignored(&mut self, ignored: HashSet<PathBuf>) {
        self.ignored = ignored;
    }

    pub(crate) fn is_ignored(&self, path: &Path) -> bool {
        path.ancestors().any(|p| self.ignored.contains(p))
    }

    /// What git says of a file, or of a folder holding changes.
    fn change(&self, row: &Row) -> Option<Change> {
        match row.kind {
            Kind::File => self.changes.get(&row.path).copied(),
            Kind::Folder => self.changed_dirs.get(&row.path).copied(),
            _ => None,
        }
    }

    /// The folder the tree hangs from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Chooses the row under a point, for a menu of what can be done with
    /// it: its path, whether it is a folder, and whether that is unfolded.
    pub fn take(&mut self, x: f32, y: f32) -> Option<(PathBuf, bool, bool)> {
        let i = self.row_at(x, y).filter(|&i| self.rows[i].kind != Kind::More)?;
        self.sel = i;
        let row = &self.rows[i];
        let folder = row.kind == Kind::Folder;
        Some((row.path.clone(), folder, folder && self.unfolded.contains(&row.path)))
    }

    /// Whether the displayed tree has a file or folder selected.
    pub fn has_selection(&self) -> bool {
        self.rows.get(self.sel).is_some_and(|r| r.kind != Kind::More)
    }

    /// The chosen row's path, and whether it is a folder.
    pub fn selected(&mut self) -> Option<(PathBuf, bool)> {
        self.fresh();
        let row = self.rows.get(self.sel).filter(|r| r.kind != Kind::More)?;
        Some((row.path.clone(), row.kind == Kind::Folder))
    }

    /// Folds an unfolded folder, or unfolds a folded one.
    pub fn flip(&mut self, path: &Path) {
        let unfolded = self.unfolded.contains(path);
        self.fold(path, !unfolded);
    }

    /// Folds every folder.
    pub fn collapse(&mut self) {
        self.unfolded.clear();
        (self.stale, self.scroll) = (true, 0.0);
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) {
        (self.rect, self.scale) = ((x, y, w, h), scale);
    }

    /// Where the explorer lies; Source Control takes the same place.
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        self.rect
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        rw > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    /// Says which folders to show, each as a folder of its own when there
    /// are several, and which file is open in the editor. The way to that
    /// file is unfolded when it becomes the open one.
    pub fn show_roots(&mut self, roots: &[PathBuf], current: Option<&Path>) {
        if roots != self.roots {
            let first = roots.first().cloned().unwrap_or_default();
            // A folder added or taken away leaves the rest as they were.
            if self.roots.is_empty() || first != self.root || roots.len() == 1 || self.roots.len() == 1 {
                self.unfolded.clear();
                (self.current, self.sel, self.scroll) = (None, 0, 0.0);
            }
            if roots.len() > 1 {
                self.unfolded.extend(roots.iter().filter(|r| !self.roots.contains(r)).cloned());
            }
            (self.root, self.roots, self.stale) = (first, roots.to_vec(), true);
        }
        if current != self.current.as_deref() {
            self.current = current.map(Path::to_path_buf);
            let multi = self.roots.len() > 1;
            if let Some(root) = current.and_then(|p| self.roots.iter().filter(|r| p.starts_with(r)).max_by_key(|r| r.as_os_str().len())) {
                let above = current.into_iter().flat_map(|p| p.ancestors().skip(1));
                let held: Vec<PathBuf> = above.take_while(|d| *d != root).map(Path::to_path_buf).collect();
                if multi {
                    self.unfolded.insert(root.clone());
                }
                self.unfolded.extend(held);
            }
            (self.seek, self.stale) = (true, true);
        }
    }

    /// Whether the tree lists several folders, each as a row of its own.
    pub fn multi(&self) -> bool {
        self.roots.len() > 1
    }

    /// The window's folder a row's path is, if it is one of several.
    pub fn is_root(&self, path: &Path) -> bool {
        self.multi() && self.roots.iter().any(|r| r == path)
    }

    /// Has the folders read again: something may have changed in them.
    pub fn refresh(&mut self) {
        self.stale = true;
    }

    /// Lists a folder and, under each folder in it that is unfolded, what
    /// that one holds.
    fn gather(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let (found, more) = entries(dir);
        for (name, folder) in found {
            let path = dir.join(&name);
            let kind = if folder { Kind::Folder } else { Kind::File };
            rows.push(Row { path: path.clone(), name, depth, kind });
            if folder && self.unfolded.contains(&path) {
                self.gather(&path, depth + 1, rows);
            }
        }
        if more {
            let name = format!("\u{2026} and more than these {MOST}");
            rows.push(Row { path: dir.to_path_buf(), name, depth, kind: Kind::More });
        }
    }

    /// Reads the folders again if need be, keeping the selection on the
    /// file or folder it was on.
    fn fresh(&mut self) {
        if !std::mem::take(&mut self.stale) {
            return;
        }
        let held = self.rows.get(self.sel).map(|r| r.path.clone());
        let mut rows = Vec::new();
        if self.multi() {
            for root in &self.roots {
                let name = root.file_name().map_or_else(|| root.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
                rows.push(Row { path: root.clone(), name: self.names.get(root).cloned().unwrap_or(name), depth: 0, kind: Kind::Folder });
                if self.unfolded.contains(root) {
                    self.gather(root, 1, &mut rows);
                }
            }
        } else {
            self.gather(&self.root, 0, &mut rows);
        }
        self.rows = rows;
        let row_of = |path: &Path| self.rows.iter().position(|r| r.path == path);
        let sought = self.current.as_deref().filter(|_| std::mem::take(&mut self.seek)).and_then(row_of);
        self.reveal |= sought.is_some();
        let sel = sought.or_else(|| held.as_deref().and_then(row_of)).unwrap_or(self.sel);
        self.sel = sel.min(self.rows.len().saturating_sub(1));
    }

    fn fold(&mut self, path: &Path, unfold: bool) {
        if unfold {
            self.unfolded.insert(path.to_path_buf());
        } else {
            self.unfolded.remove(path);
        }
        self.stale = true;
    }

    // ---- geometry -------------------------------------------------------

    fn row_h(&self) -> f32 {
        (theme::ROW * self.scale).round()
    }

    /// Where the rows are listed: the top of the list and its height.
    fn span(&self) -> (f32, f32) {
        let (_, y, _, h) = self.rect;
        let top = y + (4.0 * self.scale).round();
        (top, (y + h - top).max(0.0))
    }

    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let (top, tall) = self.span();
        if !self.contains(x, y) || y < top || y >= top + tall {
            return None;
        }
        let i = ((y - top + self.scroll) / self.row_h()) as usize;
        (i < self.rows.len()).then_some(i)
    }

    /// Whether a point is on something a click would open or unfold.
    pub fn hit(&self, x: f32, y: f32) -> bool {
        self.row_at(x, y).is_some_and(|i| self.rows[i].kind != Kind::More)
    }

    fn clamp_scroll(&mut self) {
        let (rh, tall) = (self.row_h(), self.span().1);
        // Half a row of air under the last one.
        let all = self.rows.len() as f32 * rh + (rh / 2.0).round();
        self.scroll = self.scroll.min(all - tall).max(0.0);
    }

    pub fn scroll(&mut self, dy: f32) {
        self.scroll += dy;
        self.reveal = false;
        self.clamp_scroll();
    }

    pub fn scroll_lines(&mut self, lines: f32) {
        self.scroll(lines * self.row_h());
    }

    // ---- input ----------------------------------------------------------

    /// A key pressed while the tree has the keyboard. Returns the file to
    /// open, if the key chose one.
    pub fn key(&mut self, key: Key) -> Option<PathBuf> {
        self.fresh();
        let last = self.rows.len().checked_sub(1)?;
        let sel = self.sel.min(last);
        let Row { path, depth, kind, .. } = &self.rows[sel];
        let (path, depth, kind) = (path.clone(), *depth, *kind);
        let unfolded = kind == Kind::Folder && self.unfolded.contains(&path);
        let page = ((self.span().1 / self.row_h()) as usize).saturating_sub(1).max(1);
        self.reveal = true;
        match key {
            Key::Up => self.sel = sel.saturating_sub(1),
            Key::Down => self.sel = (sel + 1).min(last),
            Key::PageUp => self.sel = sel.saturating_sub(page),
            Key::PageDown => self.sel = (sel + page).min(last),
            Key::Home => self.sel = 0,
            Key::End => self.sel = last,
            Key::Enter if kind == Kind::File => return Some(path),
            Key::Enter if kind == Kind::Folder => self.fold(&path, !unfolded),
            Key::Right if kind == Kind::Folder && !unfolded => self.fold(&path, true),
            // Into a folder that is unfolded already, if it holds anything.
            Key::Right if self.rows.get(sel + 1).is_some_and(|r| r.depth > depth) => self.sel = sel + 1,
            Key::Left if unfolded => self.fold(&path, false),
            // Out of a folder: to the row this one hangs from.
            Key::Left => self.sel = self.rows[..sel].iter().rposition(|r| r.depth < depth).unwrap_or(sel),
            _ => {}
        }
        None
    }

    /// A click. On a folder it folds or unfolds it; on a file it returns
    /// the file, to be opened.
    pub fn press(&mut self, x: f32, y: f32) -> Option<PathBuf> {
        let i = self.row_at(x, y)?;
        let (path, kind) = (self.rows[i].path.clone(), self.rows[i].kind);
        match kind {
            Kind::File => {
                self.sel = i;
                Some(path)
            }
            Kind::Folder => {
                self.sel = i;
                let unfolded = self.unfolded.contains(&path);
                self.fold(&path, !unfolded);
                None
            }
            Kind::More => None,
        }
    }

    // ---- painting -------------------------------------------------------

    /// Draws the tree. The selection shows while it has the keyboard.
    pub fn draw(&mut self, c: &mut Canvas, focused: bool) {
        let (x, y, w, h) = self.rect;
        if w < 1.0 || h < 1.0 {
            return;
        }
        self.fresh();
        let s = self.scale;
        let size = px(theme::UI_TEXT, s);
        let small = px(theme::SMALL, s);
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());

        let (top, tall) = self.span();
        let rh = self.row_h();
        if std::mem::take(&mut self.reveal) {
            let at = self.sel as f32 * rh;
            self.scroll = self.scroll.max(at + rh - tall).min(at);
        }
        self.clamp_scroll();
        c.clip(x, top, w, tall);
        let step = px(12.0, s);
        let inset = px(6.0, s);
        let isz = px(theme::SMALL_ICON, s);
        let pad = px(10.0, s);
        let hot = self.hover.and_then(|(hx, hy)| self.row_at(hx, hy)).filter(|&i| self.rows[i].kind != Kind::More);
        let first = (self.scroll / rh) as usize;
        for (i, row) in self.rows.iter().enumerate().skip(first) {
            let ry = top + i as f32 * rh - self.scroll;
            if ry >= top + tall {
                break;
            }
            let open = row.kind == Kind::File && self.current.as_deref() == Some(&row.path);
            let chosen = focused && i == self.sel;
            let r = (x + inset, ry, (w - inset * 2.0).max(0.0), rh);
            ui::row_ground(c, r, chosen || open, hot == Some(i) && !chosen && !open, chosen, s);
            // Guides down from each folder that holds this row.
            for d in 0..row.depth {
                let gx = (x + pad + d as f32 * step + isz / 2.0).round();
                c.rect(gx, ry, 1.0, rh, theme::line());
            }
            let change = self.change(row);
            let tint = change.map(|c| match c {
                Change::Untracked | Change::Added => theme::success(),
                Change::Conflict | Change::Deleted => theme::danger(),
                Change::Modified | Change::Renamed => theme::warning(),
            });
            let (face, mut ink) = match row.kind {
                _ if open => (font::UI_MEDIUM, theme::bright()),
                // Each of a window's folders heads its own files.
                _ if row.depth == 0 && self.is_root(&row.path) => (font::UI_MEDIUM, theme::fg()),
                Kind::More => (font::UI_ITALIC, theme::faint()),
                // Hidden files are there, and stay out of the way.
                _ if row.name.starts_with('.') || self.is_ignored(&row.path) => (font::UI, theme::faint()),
                _ => (font::UI, theme::fg()),
            };
            if let Some(t) = tint.filter(|_| row.kind == Kind::File) {
                ink = t;
            }
            let left = x + pad + row.depth as f32 * step;
            let iy = (ry + (rh - isz) / 2.0).round();
            if row.kind == Kind::Folder {
                let icon = if self.unfolded.contains(&row.path) { Icon::ChevronDown } else { Icon::ChevronRight };
                icons::draw(c, icon, left, iy, isz, theme::icon());
            }
            // The change's letter at the right; a dot for a folder holding changes.
            let base = ui::base(ry, rh, size);
            let mut room = x + w - pad - left - isz - px(6.0, s);
            if let (Some(change), Some(tint)) = (change, tint) {
                if row.kind == Kind::Folder {
                    c.circle(x + w - pad - px(3.0, s), (ry + rh / 2.0).round(), px(3.0, s), tint);
                    room -= px(14.0, s);
                } else {
                    let mark = change.letter();
                    let mw = fonts.width(font::UI_MEDIUM, small, mark);
                    ui::text(c, fonts, x + w - pad - mw, ui::base(ry, rh, small), mark, font::UI_MEDIUM, small, tint);
                    room -= mw + px(8.0, s);
                }
            }
            // An icon theme that shows icons here puts one before the name.
            let mut text_x = left + isz + px(6.0, s);
            if crate::icon_theme::everywhere() && matches!(row.kind, Kind::File | Kind::Folder) {
                use crate::icon_theme::Kind as IconKind;
                let kind = match row.kind {
                    Kind::Folder if self.unfolded.contains(&row.path) => IconKind::FolderOpen,
                    Kind::Folder => IconKind::Folder,
                    _ => IconKind::File,
                };
                crate::icon_theme::draw(c, &row.name, kind, text_x, iy, isz, theme::icon());
                text_x += isz + px(6.0, s);
                room -= isz + px(6.0, s);
            }
            let name = ui::fit(fonts, face, size, &row.name, room);
            ui::text(c, fonts, text_x, base, &name, face, size, ink);
        }
        if self.rows.is_empty() {
            let said = "This folder is empty";
            ui::text(c, fonts, x + pad + px(4.0, s), ui::base(top, rh * 1.5, size), said, font::UI, size, theme::dim());
        }
        c.unclip();
        let all = self.rows.len() as f32 * rh;
        if all > tall && self.hover.is_some_and(|(hx, hy)| hx >= x && hx < x + w && hy >= y && hy < y + h) {
            ui::scrollbar(c, (x, top, w, tall), self.scroll / all, tall / all, false, s);
        }
    }

    /// The rows as they stand, each indented by its depth and with a `>`
    /// where the selection is.
    #[cfg(test)]
    pub fn listing(&mut self) -> Vec<String> {
        self.fresh();
        let line = |(i, r): (usize, &Row)| {
            let slash = if r.kind == Kind::Folder { "/" } else { "" };
            format!("{}{}{}{slash}", if i == self.sel { ">" } else { "" }, "  ".repeat(r.depth), r.name)
        };
        self.rows.iter().enumerate().map(line).collect()
    }

    /// The middle of a row, by its name, for clicking it.
    #[cfg(test)]
    pub fn point(&mut self, name: &str) -> (f32, f32) {
        self.fresh();
        let i = self.rows.iter().position(|r| r.name == name).unwrap();
        (self.rect.0 + self.rect.2 / 2.0, self.span().0 + (i as f32 + 0.5) * self.row_h() - self.scroll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-tree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in [".git", "src/deep", "Docs"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for file in [".env", "b.md", "A.md", "src/main.py", "src/deep/x.py"] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        dir
    }

    fn tree(dir: &Path) -> Tree {
        let mut t = Tree::new(Rc::new(RefCell::new(Fonts::new())));
        t.set_rect(0.0, 0.0, 300.0, 33.0 + 26.0 * 4.0, 1.0);
        t.show_roots(&[dir.to_path_buf()], None);
        t
    }

    #[test]
    fn folders_come_first_and_unfold() {
        let dir = folder("unfold");
        let mut t = tree(&dir);
        // Folders, then files, whatever their case; `.git` is not shown.
        assert_eq!(t.listing(), [">Docs/", "src/", ".env", "A.md", "b.md"]);
        // Right unfolds a folder and then steps into it; Left goes back out
        // and folds it again.
        t.key(Key::Down);
        t.key(Key::Right);
        assert_eq!(t.listing(), ["Docs/", ">src/", "  deep/", "  main.py", ".env", "A.md", "b.md"]);
        t.key(Key::Right);
        t.key(Key::Enter);
        assert_eq!(t.listing()[2..4], [">  deep/", "    x.py"]);
        t.key(Key::Down);
        assert_eq!(t.key(Key::Enter), Some(dir.join("src/deep/x.py")));
        t.key(Key::Left);
        t.key(Key::Left);
        assert_eq!(t.listing()[2..4], [">  deep/", "  main.py"]);
        t.key(Key::Left);
        t.key(Key::Left);
        assert_eq!(t.listing(), ["Docs/", ">src/", ".env", "A.md", "b.md"]);
        // A folder unfolded again is as it was left.
        t.key(Key::Enter);
        assert_eq!(t.listing().len(), 7);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn several_folders_each_head_their_own_files() {
        let (one, two) = (folder("multi-one"), folder("multi-two"));
        let mut t = tree(&one);
        t.show_roots(&[one.clone(), two.clone()], Some(&two.join("src/main.py")));
        let listing = t.listing();
        let name = |d: &Path| format!("{}/", d.file_name().unwrap().to_string_lossy());
        assert_eq!(listing[0], name(&one));
        assert!(t.is_root(&one) && t.is_root(&two) && !t.is_root(&one.join("src")));
        // The open file's folder and the way to it are unfolded.
        let at = listing.iter().position(|l| l.trim_start_matches('>') == name(&two)).unwrap();
        assert_eq!(listing[at + 1..at + 5], ["  Docs/", "  src/", "    deep/", ">    main.py"]);
        // Back to one folder, it is listed as before.
        t.show_roots(&[one.clone()], None);
        assert_eq!(t.listing()[0].trim_start_matches('>'), "Docs/");
        std::fs::remove_dir_all(&one).unwrap();
        std::fs::remove_dir_all(&two).unwrap();
    }

    #[test]
    fn the_tree_follows_the_disk_and_the_open_file() {
        let dir = folder("follow");
        let mut t = tree(&dir);
        t.key(Key::End);
        assert_eq!(t.listing()[4], ">b.md");
        // What appears on disk shows after a refresh, and the selection
        // stays with its file.
        std::fs::write(dir.join("a2.md"), "").unwrap();
        std::fs::remove_file(dir.join(".env")).unwrap();
        t.refresh();
        assert_eq!(t.listing(), ["Docs/", "src/", "A.md", "a2.md", ">b.md"]);
        // The file that is opened is unfolded to, selected and in view.
        t.show_roots(&[dir.to_path_buf()], Some(&dir.join("src/deep/x.py")));
        assert_eq!(t.listing()[..4], ["Docs/", "src/", "  deep/", ">    x.py"]);
        let mut px = vec![0u32; 300 * 200];
        t.draw(&mut Canvas::new(&mut px, 300, 200), true);
        let (x, y) = t.point("x.py");
        assert!(t.hit(x, y) && t.press(x, y) == Some(dir.join("src/deep/x.py")));
        // The wheel moves the list no further than it goes.
        t.scroll_lines(100.0);
        assert_eq!(t.scroll, t.row_h() * t.rows.len() as f32 + (t.row_h() / 2.0).round() - t.span().1);
        // Another project starts folded.
        t.show_roots(&[dir.join("src").to_path_buf()], Some(&dir.join("src/main.py")));
        assert_eq!(t.listing(), ["deep/", ">main.py"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
