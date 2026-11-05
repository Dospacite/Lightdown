//! The three-way merge editor: a conflicted file's index stages side by
//! side above its working text, which is the result.
use super::chrome::Hit;
use super::git_actions::{Action, Answer, Choice, menu};
use super::repository::{self as repo, Repo};
use super::*;
use crate::merge::{CHOICES, Merge, SHOWN};
use std::path::Path;

/// The palette command of each choice, in the choices' numbering.
const COMMANDS: [&str; 5] = ["git_keep_current", "git_keep_incoming", "git_keep_both", "git_keep_base", "git_keep_reverse"];
/// Button labels, shorter than the choices' names.
const SHORT: [&str; 5] = ["Ours", "Theirs", "Ours + theirs", "Base", "Theirs + ours"];

/// The stages of a conflicted file merged with `git merge-file`, and the
/// columns' titles. Runs on a worker.
pub(super) fn load(r: &Repo, path: &Path) -> Result<Answer, String> {
    let name = r.relative(path)?;
    let listed = repo::run(&r.top, ["ls-files", "-u", "-z", "--", name])?;
    let mut ids: [Option<String>; 3] = Default::default();
    for entry in listed.split('\0').filter(|e| !e.is_empty()) {
        let Some((meta, _)) = entry.split_once('\t') else { continue };
        let fields: Vec<&str> = meta.split(' ').collect();
        if let [_, id, stage] = fields[..]
            && let Ok(stage @ 1..=3) = stage.parse::<usize>()
        {
            ids[stage - 1] = Some(id.to_string());
        }
    }
    if ids.iter().all(Option::is_none) {
        return Err("This file has no unmerged index stages".into());
    }
    let (Some(ours), Some(theirs)) = (ids[1].clone(), ids[2].clone()) else {
        return Err("One side deleted this file. Keep or remove it from Git: Source control".into());
    };
    // A file both sides added has no base: it merges against empty text.
    let base = match ids[0].clone() {
        Some(id) => id,
        None => repo::run(&r.top, ["hash-object", "-w", "-t", "blob", "/dev/null"])?.trim().to_string(),
    };
    let out = repo::output(&r.top, ["merge-file", "-p", "--diff3", "--object-id", "-L", "ours", "-L", "base", "-L", "theirs", &ours, &base, &theirs])?;
    // Its status is the number of conflicts; only a failure is negative or
    // above 127.
    if out.status.code().is_none_or(|n| !(0..=127).contains(&n)) {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    if out.stdout.contains(&0) {
        return Err("Binary file has no merge editor".into());
    }
    let merged = String::from_utf8(out.stdout).map_err(|_| "File is not UTF-8 text".to_string())?;
    let head = repo::run(&r.top, ["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
    let labels = match super::integration::state(&r.top) {
        // A rebase replays your commits onto the other branch.
        Some("rebase") => ["Ours \u{b7} the branch rebased onto".into(), "Base".into(), "Theirs \u{b7} your commit being replayed".into()],
        Some("cherry-pick") => [format!("Ours \u{b7} {head}"), "Base".into(), "Theirs \u{b7} the picked commit".into()],
        Some("revert") => [format!("Ours \u{b7} {head}"), "Base \u{b7} the reverted commit".into(), "Theirs \u{b7} its parent".into()],
        _ => [format!("Ours \u{b7} {head}"), "Base".into(), "Theirs \u{b7} the merged branch".into()],
    };
    Ok(Answer::MergeEditor(path.to_path_buf(), merged, labels))
}

/// The conflicted files, each leading to the merge editor, after an
/// operation stopped on conflicts; or none.
pub(super) fn conflicts_menu(r: &Repo, title: &str) -> Option<Answer> {
    let listed = repo::run(&r.top, ["diff", "--name-only", "--diff-filter=U", "-z"]).ok()?;
    let choices: Vec<Choice> = listed.split('\0').filter(|n| !n.is_empty()).map(|name| {
        let path = r.top.join(name);
        Choice::new(format!("Open merge editor: {name}"), "Accept ours, base or theirs per conflict, then Git: Complete merge", Action::MergeEditor(path))
    }).collect();
    (!choices.is_empty()).then(|| menu(title, choices))
}

impl App {
    pub(super) fn merge_editor_show(&mut self, path: PathBuf, merged: String, labels: [String; 3]) {
        if self.doc.path.as_ref() != Some(&path) { self.open_tab(path.clone()); }
        if self.doc.path.as_ref() != Some(&path) || self.reader.is_some() || self.nb.is_some() || self.doc.read_only {
            self.message = Some("Open the conflicted file as text to merge it".into());
            return;
        }
        let mut merge = Merge::new(&merged, &self.doc.buf.text, labels);
        merge.keys = COMMANDS.map(|id| self.keys_of(id));
        let count = merge.heads.len();
        self.doc.merge_open(merge);
        self.arrange();
        if let Some(range) = self.doc.merge_range(0) { self.go_to(range.start); }
        self.message = Some(if count == 0 {
            "No conflicts remain between the stages. Review the result, then Git: Complete merge".into()
        } else {
            format!("{count} conflicts. Accept a side for each or edit the result, then Git: Complete merge")
        });
    }

    /// A choice for conflict `k`, or for the conflict at the cursor.
    pub(super) fn merge_editor_accept(&mut self, k: Option<usize>, choice: u8) {
        let Some(k) = k.or_else(|| self.doc.merge_current()) else {
            self.message = Some("No unresolved conflicts left in the result".into());
            return;
        };
        self.unsuggest();
        match self.doc.merge_accept(k, choice) {
            Ok(()) => {
                self.edited();
                let left = (0..self.doc.merge.as_ref().map_or(0, |m| m.heads.len())).filter(|&i| self.doc.merge_range(i).is_some()).count();
                self.message = Some(if left == 0 {
                    "All conflicts accepted. Review the result, then Git: Complete merge".into()
                } else { format!("Accepted {}. {left} conflicts left", CHOICES[choice as usize].to_lowercase()) });
            }
            Err(e) => self.message = Some(e),
        }
    }

    pub(super) fn merge_editor_close(&mut self) {
        if self.doc.merge.is_none() { self.message = Some("No merge editor is open in this tab".into()); return; }
        self.doc.merge_close();
        self.arrange();
    }

    /// Closes the merge editor of a file once it is staged.
    pub(super) fn merge_editor_done(&mut self, path: &Path) {
        if self.doc.path.as_deref() == Some(path) { self.doc.merge_close(); }
        for tab in &mut self.file_tabs {
            if let Some(state) = tab.state.as_mut().filter(|s| s.pane.path.as_deref() == Some(path)) {
                state.pane.merge_close();
            }
        }
        self.arrange();
    }

    /// The choice buttons on each conflict heading in view.
    pub(super) fn draw_merge_buttons(&mut self, c: &mut Canvas) {
        if self.doc.merge.is_none() || self.reader.is_some() || self.nb.is_some() { return; }
        let s = self.scale;
        let (x, _, w, _) = self.doc.merge.as_ref().unwrap().panel;
        let keys = self.doc.merge.as_ref().unwrap().keys.clone();
        let version = self.doc.buf.version;
        let fonts = self.fonts.clone();
        for (k, top, row) in self.doc.merge_heads() {
            if self.doc.merge_range(k).is_none() { continue; }
            let mut right = x + w - px(4.0, s);
            for &choice in SHOWN.iter().rev() {
                let label = SHORT[choice as usize];
                let bw = ui::button_width(&mut fonts.borrow_mut(), label, false, s).min(w / 6.0);
                let r = ((right - bw).round(), top + 1.0, bw, row - 2.0);
                if r.0 < x + w * 0.35 { break; }
                ui::button(c, &mut fonts.borrow_mut(), r, label, None, ui::Variant::Ghost, ui::inside(r, Some(self.pointer)), s);
                self.push_hit(r, Hit::MergeAccept(k, choice, version), &format!("Accept {}", CHOICES[choice as usize].to_lowercase()), &keys[choice as usize]);
                right = r.0 - px(2.0, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A repository stopped on a real merge conflict in `a.txt`.
    pub(crate) fn conflicted(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ld-merge-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@e"], &["config", "user.name", "T"]] { git(&dir, args); }
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\n").unwrap();
        git(&dir, &["add", "."]); git(&dir, &["commit", "-qm", "base"]);
        git(&dir, &["switch", "-qc", "topic"]);
        std::fs::write(dir.join("a.txt"), "one\nTWO topic\nthree\nfour\nfive\nsix\nSEVEN topic\n").unwrap();
        git(&dir, &["commit", "-qam", "topic"]);
        git(&dir, &["switch", "-q", "main"]);
        std::fs::write(dir.join("a.txt"), "one\nTWO main\nthree\nfour\nfive\nsix\nSEVEN main\n").unwrap();
        git(&dir, &["commit", "-qam", "main"]);
        git(&dir, &["merge", "topic"]);
        dir
    }

    #[test]
    fn stages_load_with_base_and_each_conflict_is_accepted_in_the_result() {
        let dir = conflicted("accept");
        let path = dir.join("a.txt");
        let Answer::MergeEditor(_, merged, labels) = load(&Repo::open(&dir).unwrap(), &path).unwrap() else { panic!() };
        assert!(merged.contains("||||||| base\ntwo\n"));
        assert!(labels[0].contains("main"));
        let mut a = App::new(Some(path.clone()));
        a.resize(1100, 800, 1.0);
        a.merge_editor_show(path.clone(), merged, labels);
        let m = a.doc.merge.as_ref().unwrap();
        assert_eq!(m.heads.len(), 2);
        // The columns take the top of the pane and the result the rest.
        assert!(m.panel.3 > 0.0 && a.doc.bounds().1 > m.panel.1 + m.panel.3 - 1.0);
        let mut px = vec![0u32; 1100 * 800];
        a.draw(&mut Canvas::new(&mut px, 1100, 800));
        let base = a.hits.iter().find(|(_, h)| *h == Hit::MergeAccept(0, 3, a.doc.buf.version)).unwrap().0;
        a.press(base.0 + 2.0, base.1 + 2.0, Mods::default(), 1);
        assert!(a.doc.buf.text.starts_with("one\ntwo\nthree\n"));
        // The other conflict is still found and takes theirs then ours.
        a.merge_editor_accept(None, 4);
        assert!(a.doc.buf.text.ends_with("SEVEN topic\nSEVEN main\n"));
        assert!(a.doc.merge_current().is_none());
        // The result can be edited freely; completing saves and stages it.
        a.doc.select(0, 0);
        a.paste_text("edited ");
        a.git_command("git_complete_merge");
        a.git_actions_settle();
        assert!(a.doc.merge.is_none());
        assert!(git(&dir, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("edited one\ntwo\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn complete_merge_refuses_while_markers_remain() {
        let dir = conflicted("refuse");
        let path = dir.join("a.txt");
        let mut a = App::new(Some(path.clone()));
        a.resize(1100, 800, 1.0);
        a.git_command("git_merge_editor");
        a.git_actions_settle();
        assert!(a.doc.merge.is_some());
        a.git_command("git_complete_merge");
        assert!(a.message.as_deref().is_some_and(|m| m.contains("conflict markers")));
        assert!(!git(&dir, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
        // Next and previous conflict move the result's cursor, and the
        // columns follow it.
        a.git_command("git_conflict");
        let first = a.doc.merge_current();
        a.doc.select(a.doc.buf.text.len() - 1, a.doc.buf.text.len() - 1);
        a.git_command("git_conflict_previous");
        assert_eq!(a.doc.merge_current(), Some(1));
        assert_eq!(first, Some(0));
        a.git_command("git_merge_close");
        assert!(a.doc.merge.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
