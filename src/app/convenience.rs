//! References and recent paths use source line numbers and canonical files.
use super::*;
use serde_json::{Value, json};

pub(super) fn recent_paths() -> Vec<PathBuf> {
    let text = std::fs::read_to_string(config::data_dir().join("recent.json")).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .take(120)
        .collect()
}
fn encoded(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn github_remote(remote: &str) -> Option<String> {
    let path = remote
        .trim()
        .strip_prefix("git@github.com:")
        .or_else(|| remote.trim().strip_prefix("https://github.com/"))
        .or_else(|| remote.trim().strip_prefix("ssh://git@github.com/"))?
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
    {
        return None;
    }
    Some(format!("https://github.com/{path}"))
}
impl App {
    pub(super) fn remember_recent(&mut self, path: &Path) {
        let Ok(path) = path.canonicalize() else { return };
        let mut paths = recent_paths();
        paths.retain(|p| *p != path);
        paths.insert(0, path);
        paths.truncate(120);
        let value = json!(paths);
        let _ = config::write_private(&config::data_dir().join("recent.json"), value.to_string().as_bytes());
        self.recent = paths;
    }
    pub(super) fn open_recent(&mut self) {
        self.recent = recent_paths();
        let entries = self
            .recent
            .iter()
            .enumerate()
            .map(|(i, p)| {
                Entry::new(
                    &i.to_string(),
                    &p.file_name().unwrap_or(p.as_os_str()).to_string_lossy(),
                    &format!("{}  {}", if p.is_dir() { "Project" } else { "File" }, p.display()),
                )
            })
            .collect();
        self.menu = Some((MenuKind::Recent, Menu::new("Recent projects and files", entries, "")));
    }
    pub(super) fn pick_recent(&mut self, key: &str) {
        let Some(path) = key.parse::<usize>().ok().and_then(|i| self.recent.get(i)).cloned() else { return };
        if path.is_dir() {
            self.open_folder(path);
        } else {
            self.pick(path, false);
        }
    }
    /// A welcome view takes the empty editor's place, including its caret and gutter.
    pub(super) fn welcomes(&self) -> bool {
        self.doc.path.is_none()
            && self.doc.buf.text.is_empty()
            && !self.doc.buf.modified
            && !self.doc.read_only
            && self.doc.output_name.is_none()
            && self.reader.is_none()
            && self.nb.is_none()
            && self.split_other().is_none()
    }

    pub(super) fn draw_recent(&mut self, c: &mut Canvas) {
        if !self.welcomes() {
            return;
        }
        use crate::icons::{self, Icon};
        let (left, width) = self.document_area();
        let s = self.scale;
        let top = self.title_h() + self.head_h();
        let bottom = self.body_bottom() - self.term_height();
        if width < 140.0 * s || bottom - top < 100.0 * s {
            return;
        }
        // Clip the whole view, including labels and hit targets, to the editor.
        c.clip(left, top, width, bottom - top);
        let margin = (28.0 * s).min(width * 0.08);
        let x = left + margin;
        let w = (width - margin * 2.0).min(560.0 * s);
        let mut y = top + 40.0 * s;
        {
            let mut fonts = self.fonts.borrow_mut();
            ui::text(c, &mut fonts, x, y, "Start editing", font::UI_BOLD, theme::BODY * s, theme::fg());
            y += 24.0 * s;
            let note = ui::fit(
                &mut fonts,
                font::UI,
                theme::UI_TEXT * s,
                "Open a file or folder, or start typing a new document.",
                w,
            );
            ui::text(c, &mut fonts, x, y, &note, font::UI, theme::UI_TEXT * s, theme::dim());
        }
        y += 20.0 * s;
        let mut bx = x;
        let height = theme::CONTROL * s;
        for (id, label, icon, variant) in [
            ("open", "Open…", Icon::Folder, ui::Variant::Primary),
            ("new_file", "New file…", Icon::FilePlus, ui::Variant::Outline),
        ] {
            let fonts = self.fonts.clone();
            let mut fonts = fonts.borrow_mut();
            let bw = ui::button_width(&mut fonts, label, true, s).min(w);
            if bx > x && bx + bw > x + w {
                bx = x;
                y += height + 8.0 * s;
            }
            let r = (bx, y, bw, height);
            if y + height <= bottom - 8.0 * s {
                ui::button(
                    c,
                    &mut fonts,
                    r,
                    label,
                    Some(icon),
                    variant,
                    !self.overlaid() && ui::inside(r, Some(self.pointer)),
                    s,
                );
                let keys = self.keys_of(id);
                self.push_hit(r, Hit::Start(id), label, &keys);
            }
            bx += bw + 8.0 * s;
        }
        y += height + 36.0 * s;
        // A first launch has no recent paths. Do not show an empty section.
        if !self.recent.is_empty() && y + 56.0 * s <= bottom {
            {
                let mut fonts = self.fonts.borrow_mut();
                let label = ui::fit(&mut fonts, font::UI_BOLD, theme::LARGE * s, "Recent projects and files", w);
                ui::text(c, &mut fonts, x, y, &label, font::UI_BOLD, theme::LARGE * s, theme::fg());
            }
            y += 16.0 * s;
            let paths = self.recent.clone();
            for (i, p) in paths.iter().take(12).enumerate() {
                let r = (x, y, w, 48.0 * s);
                if r.1 + r.3 > bottom - 8.0 * s {
                    break;
                }
                let hot = !self.overlaid() && ui::inside(r, Some(self.pointer));
                if hot {
                    c.round_rect(r.0, r.1, r.2, r.3, theme::RADIUS_MD * s, theme::hover());
                }
                icons::centred(
                    c,
                    if p.is_dir() { Icon::Folder } else { Icon::File },
                    (x + 8.0 * s, y, 16.0 * s, r.3),
                    16.0 * s,
                    theme::icon(),
                );
                let mut fonts = self.fonts.borrow_mut();
                let tx = x + 36.0 * s;
                let room = (w - 44.0 * s).max(0.0);
                let name = p.file_name().unwrap_or(p.as_os_str()).to_string_lossy();
                let name = ui::fit(&mut fonts, font::UI_MEDIUM, theme::UI_TEXT * s, &name, room);
                let parent = p.parent().unwrap_or(p).to_string_lossy();
                let parent = ui::fit(&mut fonts, font::UI, theme::SMALL * s, &parent, room);
                ui::text(c, &mut fonts, tx, y + 19.0 * s, &name, font::UI_MEDIUM, theme::UI_TEXT * s, theme::fg());
                ui::text(c, &mut fonts, tx, y + 36.0 * s, &parent, font::UI, theme::SMALL * s, theme::dim());
                drop(fonts);
                self.push_hit(r, Hit::Recent(i), &p.to_string_lossy(), "");
                y += r.3;
            }
        }
        c.unclip();
    }
    pub(super) fn copy_reference(&mut self, github: bool) {
        let Some(path) = &self.doc.path else {
            self.message = Some("Save the file before copying a reference".into());
            return;
        };
        if self.reader.is_some() || self.nb.is_some() {
            self.message = Some("Line references require a source text file".into());
            return;
        }
        let (a, b) = self.doc.buf.sel();
        let first = self.doc.buf.line_of(a) + 1;
        let last =
            self.doc.buf.line_of(if b > a && self.doc.buf.starts.binary_search(&b).is_ok() { b - 1 } else { b }) + 1;
        if !github {
            self.copy = Some(format!(
                "{}:{first}{}",
                crate::tools::short(&self.root, path),
                if first == last { String::new() } else { format!("-{last}") }
            ));
            self.message = Some("Source reference copied".into());
            return;
        }
        let run = |args: &[&str]| -> Result<String, String> {
            let o = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .output()
                .map_err(|e| e.to_string())?;
            if !o.status.success() {
                return Err(String::from_utf8_lossy(&o.stderr).trim().into());
            }
            Ok(String::from_utf8_lossy(&o.stdout).trim().into())
        };
        let result = (|| -> Result<String, String> {
            if self.doc.buf.modified {
                return Err("Save or revert edits before copying a GitHub reference".into());
            }
            let top = PathBuf::from(run(&["rev-parse", "--show-toplevel"])?);
            let relative = path
                .strip_prefix(&top)
                .map_err(|_| "File is outside the repository")?
                .to_str()
                .ok_or("Path is not UTF-8")?;
            let head = run(&["rev-parse", "HEAD"])?;
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["show", &format!("{head}:{relative}")])
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err("File is not committed at HEAD".into());
            }
            let saved = String::from_utf8(output.stdout).map_err(|_| "Committed file is not UTF-8")?;
            if saved != self.contents() {
                return Err("Line numbers differ from HEAD. Commit the file before copying a GitHub link.".into());
            }
            let remote = run(&["remote", "get-url", "origin"])?;
            let base = github_remote(&remote).ok_or("Origin must point to a GitHub repository")?;
            Ok(format!(
                "{base}/blob/{head}/{}#L{first}{}",
                encoded(relative),
                if first == last { String::new() } else { format!("-L{last}") }
            ))
        })();
        match result {
            Ok(url) => {
                self.copy = Some(url);
                self.message = Some("GitHub reference copied".into());
            }
            Err(e) => self.message = Some(e),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_remotes_and_paths() {
        assert_eq!(github_remote("git@github.com:owner/repo.git").unwrap(), "https://github.com/owner/repo");
        assert!(github_remote("https://github.com.evil/repo").is_none());
        assert_eq!(encoded("src/a b#é.rs"), "src/a%20b%23%C3%A9.rs");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git").arg("-C").arg(root).args(args).status().unwrap().success());
    }
    #[test]
    fn source_references_use_selected_lines_and_github_committed_bytes() {
        let dir = std::env::temp_dir().join(format!("ld-reference-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        let path = dir.join("source file.py");
        std::fs::write(&path, "    alpha\n    beta\n    gamma\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "initial"]);
        git(&dir, &["remote", "add", "origin", "git@github.com:owner/repo.git"]);
        let mut a = App::new(Some(path.clone()));
        a.doc.select(0, a.doc.buf.starts[2]);
        a.copy_reference(false);
        assert_eq!(a.copy.as_deref(), Some("source file.py:1-2"));
        a.copy = None;
        a.copy_reference(true);
        let url = a.copy.take().expect("GitHub reference must accept leading indentation");
        assert!(url.ends_with("/source%20file.py#L1-L2"));
        a.doc.insert("# edit", EditKind::Other);
        a.copy_reference(true);
        assert!(a.copy.is_none());
        a.remember_recent(&path);
        a.open_recent();
        assert_eq!(a.recent.first(), Some(&path.canonicalize().unwrap()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod welcome_tests {
    use super::*;

    fn app(w: u32, h: u32, scale: f32) -> App {
        let mut a = App::new(None);
        a.cfg.sidebar = false;
        a.cfg.files_sidebar = false;
        a.show_side(None);
        a.show_files(false);
        a.focus = Focus::Doc;
        a.recent.clear();
        a.resize(w, h, scale);
        a
    }
    fn draw(a: &mut App) {
        let mut pixels = vec![0; (a.w * a.h) as usize];
        a.draw(&mut Canvas::new(&mut pixels, a.w as i32, a.h as i32));
    }
    #[test]
    fn empty_startup_replaces_the_editor_and_typing_restores_it() {
        let mut a = app(1072, 700, 1.0);
        draw(&mut a);
        assert!(a.welcomes());
        assert!(!a.doc.focused, "The welcome view must not paint an editor caret");
        assert!(a.doc.visible.is_empty(), "The welcome view must not paint source lines or gutter");
        assert!(!a.hits.iter().any(|(_, h)| matches!(h, Hit::Recent(_))));
        let r = a.hits.iter().find(|(_, h)| *h == Hit::Start("open")).unwrap().0;
        a.press(r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, Mods::default(), 1);
        assert!(a.chooser.is_some());
        a.key(Key::Escape, Mods::default(), None);
        a.key(Key::Char('h'), Mods::default(), Some("h"));
        draw(&mut a);
        assert_eq!(a.doc.buf.text, "h");
        assert!(!a.welcomes());
        assert!(a.doc.focused && !a.doc.visible.is_empty());
        assert!(!a.hits.iter().any(|(_, h)| matches!(h, Hit::Start(_))));
        a.open_output(String::new());
        assert!(!a.welcomes(), "An empty output tab is still a document view");
    }
    #[test]
    fn start_actions_and_recent_rows_stay_inside_the_editor_at_all_sizes() {
        for (w, h, scale) in [(1072, 700, 1.0), (320, 480, 1.0), (640, 360, 1.0), (1600, 1100, 2.0)] {
            let mut a = app(w, h, scale);
            a.recent = vec![PathBuf::from("/a/long/path/to/project"); 12];
            draw(&mut a);
            let (left, width) = a.document_area();
            let top = a.title_h() + a.head_h();
            let bottom = a.body_bottom() - a.term_height();
            let controls: Vec<_> = a.hits.iter().filter(|(_, h)| matches!(h, Hit::Start(_) | Hit::Recent(_))).collect();
            assert!(controls.len() >= 2);
            for (r, _) in controls {
                assert!(r.0 >= left && r.0 + r.2 <= left + width);
                assert!(r.1 >= top && r.1 + r.3 <= bottom);
            }
            // Drawing the start view must not touch another panel or the bars.
            let mut pixels = vec![0x123456; (w * h) as usize];
            a.draw_recent(&mut Canvas::new(&mut pixels, w as i32, h as i32));
            for (i, pixel) in pixels.iter().enumerate() {
                let (x, y) = ((i % w as usize) as f32, (i / w as usize) as f32);
                if x < left || x >= left + width || y < top || y >= bottom {
                    assert_eq!(*pixel, 0x123456);
                }
            }
        }
    }
    #[test]
    fn recent_rows_open_files_and_saved_empty_files_keep_the_editor() {
        let dir = std::env::temp_dir().join(format!("ld-welcome-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.md");
        std::fs::write(&path, "").unwrap();
        let mut a = app(1000, 700, 1.0);
        a.recent = vec![path.clone()];
        draw(&mut a);
        let r = a.hits.iter().find(|(_, h)| *h == Hit::Recent(0)).unwrap().0;
        a.press(r.0 + 20.0, r.1 + 20.0, Mods::default(), 1);
        draw(&mut a);
        assert_eq!(a.doc.path, Some(path.canonicalize().unwrap()));
        assert!(!a.welcomes());
        assert!(a.doc.focused);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
