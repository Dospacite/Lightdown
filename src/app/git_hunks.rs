//! Saved-file hunk operations. A preview owns the exact index and disk snapshots.
use super::*;
use super::repository::{self as repo, Repo};
use std::io::Write;
use std::process::Stdio;

#[derive(Clone, Debug)]
pub(super) struct Hunk {
    pub path: PathBuf,
    pub line: usize,
    pub staged: bool,
    pub patch: String,
    index: String,
    disk: Vec<u8>,
}
fn patch(r: &Repo, path: &Path, staged: bool) -> Result<String, String> {
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--unified=0"];
    if staged { args.push("--cached"); }
    args.extend(["--", r.relative(path)?]);
    repo::run(&r.top, args)
}
pub(super) fn read(r: &Repo, path: PathBuf, line: usize, staged: bool) -> Result<Hunk, String> {
    let all = patch(r, &path, staged)?;
    let start = all.find("@@ ").ok_or("No saved text hunks here. Save edits first; add new files before staging hunks.")?;
    let header = &all[..start];
    let mut hunks = Vec::new();
    let mut offset = start;
    for block in all[start..].split_inclusive('\n') {
        if block.starts_with("@@ ") { hunks.push(offset); }
        offset += block.len();
    }
    let mut chosen = None;
    for (i, &at) in hunks.iter().enumerate() {
        let end = hunks.get(i + 1).copied().unwrap_or(all.len());
        let range = all[at..].split_whitespace().nth(2).unwrap_or("+1");
        let (n, count) = range[1..].split_once(',').unwrap_or((&range[1..], "1"));
        let n = n.parse::<usize>().unwrap_or(1).saturating_sub(1);
        let count = count.parse::<usize>().unwrap_or(1).max(1);
        if (n..n + count).contains(&line) { chosen = Some((n, at, end)); break; }
    }
    let (line, at, end) = chosen.ok_or("No hunk at this line. Use a changed line in the saved file.")?;
    Ok(Hunk {
        index: repo::run(&r.top, ["ls-files", "--stage", "--", r.relative(&path)?])?,
        disk: std::fs::read(&path).map_err(|e| e.to_string())?,
        path, line, staged, patch: format!("{header}{}", &all[at..end]),
    })
}
/// Select changed lines in new-file coordinates. In a replacement, removed
/// lines pair with added lines in order and extra removed lines belong to the
/// last added line; a pure deletion belongs to the lines beside it.
pub(super) fn selected(mut h: Hunk, first: usize, last: usize) -> Result<Hunk, String> {
    let at = h.patch.find("@@ ").ok_or("Missing hunk header")?;
    let end = h.patch[at..].find('\n').map(|n| at + n + 1).ok_or("Missing hunk body")?;
    let fields: Vec<_> = h.patch[at..end].split_whitespace().collect();
    if fields.len() < 3 { return Err("Malformed hunk header".into()); }
    let range = |s: &str| {
        let (n, count) = s[1..].split_once(',').unwrap_or((&s[1..], "1"));
        (n.parse::<usize>().unwrap_or(0), count.parse::<usize>().unwrap_or(1))
    };
    let ((old, old_count), (new, new_count)) = (range(fields[1]), range(fields[2]));
    // With no added lines, the new side names the line before the hunk.
    let start = if new_count == 0 { new } else { new.saturating_sub(1) };
    // A pure deletion is chosen from the line before or after it.
    let within = if new_count == 0 { first <= start && last + 1 >= start } else { first >= start && last < start + new_count };
    if !within { return Err("Select lines within one hunk".into()); }
    // Each row keeps any "\ No newline" marker that follows it.
    let (mut removed, mut added): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for row in h.patch[end..].split_inclusive('\n') {
        let list = match row.as_bytes().first() {
            Some(b'-') => &mut removed,
            Some(b'+') => &mut added,
            Some(b'\\') => {
                let last = if added.is_empty() { removed.last_mut() } else { added.last_mut() };
                if let Some(last) = last { last.push_str(row); }
                continue;
            }
            _ => continue,
        };
        list.push(row.to_string());
    }
    let groups = added.len().max(1);
    let (mut body, mut before, mut after, mut changes) = (String::new(), 0, 0, 0);
    for g in 0..groups {
        let chosen = new_count == 0 || (first..=last).contains(&(start + g));
        let pairs = removed.iter().enumerate().filter(|(i, _)| (*i).min(groups - 1) == g).map(|(_, r)| r);
        for row in pairs {
            if chosen { before += 1; changes += 1; body.push_str(row); }
            // Unstaging keeps removals out of the index; staging keeps the old line.
            else if !h.staged { before += 1; after += 1; body.push(' '); body.push_str(&row[1..]); }
        }
        if let Some(row) = added.get(g) {
            if chosen { after += 1; changes += 1; body.push_str(row); }
            else if h.staged { before += 1; after += 1; body.push(' '); body.push_str(&row[1..]); }
        }
    }
    if changes == 0 { return Err("Selection contains no changed lines".into()); }
    // A side with no lines names the line before; otherwise its first line.
    let side = |n: usize, count: usize, lines: usize| match (count, lines) {
        (0, 0) => n,
        (0, _) => n + 1,
        (_, 0) => n.saturating_sub(1),
        _ => n,
    };
    let (old, new) = (side(old, old_count, before), side(new, new_count, after));
    h.patch = format!("{}@@ -{old},{before} +{new},{after} @@\n{body}", &h.patch[..at]);
    Ok(h)
}
pub(super) fn apply(r: &Repo, h: &Hunk, revert: bool) -> Result<(), String> {
    if repo::run(&r.top, ["ls-files", "--stage", "--", r.relative(&h.path)?])? != h.index
        || std::fs::read(&h.path).map_err(|e| e.to_string())? != h.disk
    { return Err("File or index changed. Open the hunk again.".into()); }
    if revert && h.staged { return Err("Unstage the hunk before reverting it".into()); }
    let mut cmd = repo::command(&r.top);
    cmd.args(["apply", "--unidiff-zero", "--whitespace=nowarn"]);
    if !revert { cmd.arg("--cached"); }
    if revert || h.staged { cmd.arg("--reverse"); }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let mut input = child.stdin.take().unwrap();
    let bytes = h.patch.as_bytes().to_vec();
    let writer = std::thread::spawn(move || input.write_all(&bytes));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    writer.join().map_err(|_| "Patch writer failed")?.map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).into()); }
    Ok(())
}
impl App {
    pub(super) fn git_hunk_show(&mut self, line: usize, staged: bool) {
        if let Some(path) = self.doc.path.clone().filter(|_| !self.doc.read_only && self.nb.is_none()) {
            self.git_action(git_actions::Action::Hunk(path, line, staged));
        }
    }
    pub(super) fn git_hunk_button(&mut self, choice: u8) {
        if choice == 2 { self.git_actions.hunk = None; return; }
        if let Some((mut h, version)) = self.git_actions.hunk.clone() {
            if self.doc.path.as_ref() != Some(&h.path) || self.doc.buf.version != version || self.doc.buf.modified {
                self.message = Some("Save edits and reopen the hunk first".into());
                self.git_actions.hunk = None;
                return;
            }
            if choice == 3 {
                let (a, b) = self.doc.buf.sel();
                match selected(h, self.doc.buf.line_of(a), self.doc.buf.line_of(b.saturating_sub(1).max(a))) {
                    Ok(part) => h = part,
                    Err(e) => { self.message = Some(e); return; }
                }
            }
            self.git_action(git_actions::Action::ApplyHunk(h, choice == 1));
            self.git_actions.hunk = None;
        }
    }
    /// Whether the selection holds changed lines of the previewed hunk.
    fn git_hunk_selection_fits(&self, h: &Hunk) -> bool {
        let (a, b) = self.doc.buf.sel();
        a != b && selected(h.clone(), self.doc.buf.line_of(a), self.doc.buf.line_of(b - 1)).is_ok()
    }
    pub(super) fn draw_git_hunk(&mut self, c: &mut Canvas) {
        let Some((h, version)) = self.git_actions.hunk.clone() else { return };
        if self.doc.path.as_ref() != Some(&h.path) || self.doc.buf.version != version { self.git_actions.hunk = None; return; }
        let at = self.doc.buf.starts[h.line.min(self.doc.buf.lines() - 1)];
        let Some((_, y, _)) = self.doc.spot(at) else { return };
        let (x, w) = self.doc.column();
        let s = self.scale;
        let row = 22.0 * s;
        let lines: Vec<_> = h.patch.lines().skip_while(|l| !l.starts_with("@@ ")).collect();
        let bottom = if self.term_open { self.term_top() } else { self.body_bottom() };
        let top = self.title_h() + self.head_h();
        let height = ((lines.len().min(12) + 1) as f32 * row).min((bottom - top).max(row));
        let y = (y + height.min(row)).max(top).min(bottom - height);
        c.rect(x, y, w, height, theme::raised());
        c.frame(x, y, w, height, s, theme::edge());
        for (choice, label) in [if h.staged { "Unstage" } else { "Stage" }, "Revert", "Close", if h.staged { "Unstage lines" } else { "Stage lines" }].iter().enumerate() {
            if h.staged && choice == 1 { continue; }
            if choice == 3 && !self.git_hunk_selection_fits(&h) { continue; }
            let bw = (100.0 * s).min(w / 4.0);
            let rect = (x + choice as f32 * bw, y, bw, row);
            ui::button(c, &mut self.fonts.borrow_mut(), rect, label, None, ui::Variant::Ghost, ui::inside(rect, Some(self.pointer)), s);
            self.push_hit(rect, Hit::GitHunk(choice as u8), label, "");
        }
        c.clip(x, y + row, w, height - row);
        let mut fonts = self.fonts.borrow_mut();
        let face = fonts.mono();
        for (i, line) in lines.iter().take(12).enumerate() {
            let color = if line.starts_with('+') { theme::success() } else if line.starts_with('-') { theme::danger() } else { theme::dim() };
            ui::text(c, &mut fonts, x + 8.0 * s, y + (i + 2) as f32 * row - 5.0 * s, line, face, 12.0 * s, color);
        }
        c.unclip();
    }
}
