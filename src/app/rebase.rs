//! Interactive rebase as a native todo list. Git is driven through
//! `GIT_SEQUENCE_EDITOR`, which copies the list in, and `GIT_EDITOR`, which
//! writes reworded messages and keeps squashed ones.
use super::git_actions::{Action, Answer, Choice, menu};
use super::repository::{self as repo, Repo};
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Verb { Pick, Reword, Squash, Fixup, Drop }

impl Verb {
    fn word(self) -> &'static str {
        match self { Verb::Pick => "pick", Verb::Reword => "reword", Verb::Squash => "squash", Verb::Fixup => "fixup", Verb::Drop => "drop" }
    }
    fn detail(self) -> &'static str {
        match self {
            Verb::Pick => "Keep as it is",
            Verb::Reword => "Keep with a new message",
            Verb::Squash => "Meld into the commit above, joining the messages",
            Verb::Fixup => "Meld into the commit above, keeping its message",
            Verb::Drop => "Leave out",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Step { pub verb: Verb, pub id: String, pub subject: String, pub message: Option<String> }

/// The commits after `base`, oldest first, as they will be replayed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Todo { pub root: PathBuf, pub base: String, pub head: String, pub steps: Vec<Step>, pub selected: usize }

impl Todo {
    /// What git reads from the sequence editor.
    pub fn script(&self) -> String {
        self.steps.iter().map(|s| format!("{} {} {}\n", s.verb.word(), s.id, s.subject)).collect()
    }

    /// Why the list cannot be run as it is.
    pub fn problem(&self) -> Option<&'static str> {
        let kept: Vec<&Step> = self.steps.iter().filter(|s| s.verb != Verb::Drop).collect();
        if kept.is_empty() { return Some("Every commit is dropped; keep at least one"); }
        if matches!(kept[0].verb, Verb::Squash | Verb::Fixup) { return Some("The first kept commit has nothing above it to meld into"); }
        if self.steps.iter().any(|s| s.verb == Verb::Reword && s.message.as_deref().is_none_or(|m| m.trim().is_empty())) {
            return Some("A reworded commit needs a message");
        }
        None
    }

    /// Moves the selected step up or down one place.
    pub fn shift(&mut self, up: bool) {
        let i = self.selected;
        let j = if up { i.checked_sub(1) } else { (i + 1 < self.steps.len()).then_some(i + 1) };
        if let Some(j) = j { self.steps.swap(i, j); self.selected = j; }
    }
}

/// Where the list, the messages and the editor live while a rebase runs.
fn folder(r: &Repo) -> Result<PathBuf, String> {
    let path = PathBuf::from(repo::run(&r.top, ["rev-parse", "--git-path", "lightdown-rebase"])?.trim());
    Ok(if path.is_absolute() { path } else { r.top.join(path) })
}

/// Rewords from the file named for the commit git is replaying, and keeps
/// any other message as git proposes it.
const EDITOR: &str = r#"#!/bin/sh
set -f
dir=$(dirname "$0")
done_file=$(git rev-parse --git-path rebase-merge/done)
last=$(tail -n 1 "$done_file" 2>/dev/null)
file=$1
set -- $last
set +f
case "$1" in
r|reword)
    [ -n "$2" ] || exit 0
    for f in "$dir"/msg-*; do
        case "${f##*/msg-}" in "$2"*) cp "$f" "$file"; exit 0;; esac
    done;;
esac
exit 0
"#;

fn quoted(path: &Path) -> Result<String, String> {
    let s = path.to_str().ok_or("Non-UTF-8 Git folder")?;
    if s.contains('\'') { return Err("The Git folder's path has a quote in it".into()); }
    Ok(format!("'{s}'"))
}

/// Uses the rebase's editor when it has one, so later rewords still apply
/// after Continue; otherwise messages are kept as git proposes them.
pub(super) fn editors(r: &Repo, command: &mut Command) {
    let script = folder(r).ok().map(|d| d.join("editor.sh")).filter(|p| p.exists());
    match script.and_then(|p| quoted(&p).ok()) {
        Some(editor) => command.env("GIT_EDITOR", editor),
        None => command.env("GIT_EDITOR", "true"),
    };
}

/// Removes the rebase's files once nothing is in progress.
pub(super) fn tidy(r: &Repo) {
    if super::integration::state(&r.top).is_none() && let Ok(dir) = folder(r) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The bases to choose from: where the branch left its upstream, then
/// recent commits.
pub(super) fn bases(r: &Repo) -> Result<Answer, String> {
    let mut choices = Vec::new();
    if let Ok(base) = repo::run(&r.top, ["merge-base", "HEAD", "@{upstream}"]) {
        choices.push(Choice::new("Commits not yet on the upstream branch", base.trim().get(..8).unwrap_or(""), Action::RebaseTodo(base.trim().into())));
    }
    let log = repo::run(&r.top, ["log", "-n", "50", "--skip", "1", "--format=%H%x00%h %s%x00%as \u{b7} %an"])?;
    for line in log.lines() {
        let f: Vec<&str> = line.splitn(3, '\0').collect();
        if let [id, title, detail] = f[..] {
            choices.push(Choice::new(format!("After {title}"), detail, Action::RebaseTodo(id.into())));
        }
    }
    if choices.is_empty() { return Err("No earlier commits to rebase onto".into()); }
    Ok(menu("Interactive rebase: rewrite the commits after", choices))
}

/// The commits after a base, for the list.
pub(super) fn todo(r: &Repo, base: &str) -> Result<Answer, String> {
    if super::integration::state(&r.top).is_some() { return Err("Finish or abort the Git operation in progress first".into()); }
    if !repo::run(&r.top, ["status", "--porcelain=v1"])?.trim().is_empty() {
        return Err("Commit or stash changes before an interactive rebase".into());
    }
    let range = format!("{base}..HEAD");
    if repo::run(&r.top, ["rev-list", "--merges", "--count", &range])?.trim() != "0" {
        return Err("The range has merge commits; interactive rebase here replays straight history only".into());
    }
    let head = repo::run(&r.top, ["rev-parse", "HEAD"])?.trim().to_string();
    let log = repo::run(&r.top, ["log", "--reverse", "-n", "500", "--format=%H%x00%s", &range])?;
    let steps: Vec<Step> = log.lines().filter_map(|l| l.split_once('\0'))
        .map(|(id, subject)| Step { verb: Verb::Pick, id: id.into(), subject: subject.into(), message: None }).collect();
    if steps.is_empty() { return Err("No commits after that base".into()); }
    Ok(Answer::RebaseTodo(Todo { root: r.top.clone(), base: base.into(), head, steps, selected: 0 }))
}

/// Runs the rebase with the list. Refuses a moved HEAD or changed files.
pub(super) fn run(r: &Repo, todo: &Todo) -> Result<Answer, String> {
    if let Some(problem) = todo.problem() { return Err(problem.into()); }
    if repo::run(&r.top, ["rev-parse", "HEAD"])?.trim() != todo.head {
        return Err("HEAD moved since the list was made. Start the interactive rebase again.".into());
    }
    if super::integration::state(&r.top).is_some() || !repo::run(&r.top, ["status", "--porcelain=v1"])?.trim().is_empty() {
        return Err("The working tree changed. Commit or stash changes first.".into());
    }
    let dir = folder(r)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).map_err(|e| e.to_string());
    write("todo", &todo.script())?;
    for step in todo.steps.iter().filter(|s| s.verb == Verb::Reword) {
        write(&format!("msg-{}", step.id), &format!("{}\n", step.message.as_deref().unwrap_or("").trim_end()))?;
    }
    let editor = dir.join("editor.sh");
    write("editor.sh", EDITOR)?;
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let out = repo::command(&r.top)
        .env("GIT_SEQUENCE_EDITOR", format!("cp {}", quoted(&dir.join("todo"))?))
        .env("GIT_EDITOR", quoted(&editor)?)
        .args(["rebase", "-i", "--no-autosquash", &todo.base])
        .output().map_err(|e| format!("Cannot run Git: {e}"))?;
    if !out.status.success() {
        let e = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
        return super::merge_editor::conflicts_menu(r, "Rebase stopped on conflicts").ok_or(e);
    }
    tidy(r);
    let kept = todo.steps.iter().filter(|s| s.verb != Verb::Drop).count();
    Ok(Answer::Message(format!("Rebased: {kept} of {} commits replayed", todo.steps.len()), false))
}

/// The commands for an operation in progress, and its conflicted files.
pub(super) fn operation_menu(r: &Repo) -> Result<Answer, String> {
    let kind = super::integration::state(&r.top).ok_or("No Git operation in progress")?;
    let mut choices = vec![Choice::new("Continue", "Stage resolved files first", Action::Operation("continue".into()))];
    if kind != "merge" {
        choices.push(Choice::new("Skip", "Leave out the commit being applied", Action::Confirm(format!("Skip this commit of the {kind}?"), Box::new(Action::Operation("skip".into())))));
    }
    choices.push(Choice::new("Abort", "Put the branch back as it was before", Action::Confirm(format!("Abort the {kind}?"), Box::new(Action::Operation("abort".into())))));
    if let Some(Answer::Menu(_, files)) = super::merge_editor::conflicts_menu(r, "") { choices.extend(files); }
    Ok(menu(&format!("{} in progress", super::integration::title(kind)), choices))
}

impl App {
    pub(super) fn rebase_show(&mut self, todo: Todo) {
        let entries = todo.steps.iter().enumerate().map(|(i, s)| {
            let subject = if s.verb == Verb::Reword { s.message.as_deref().unwrap_or("") } else { &s.subject };
            Entry::new(&i.to_string(), &format!("{:<6}  {}  {subject}", s.verb.word(), &s.id[..s.id.len().min(8)]), s.verb.detail())
        }).collect();
        let mut menu = Menu::new(&format!("Interactive rebase \u{b7} {} commits", todo.steps.len()), entries, &todo.selected.to_string());
        menu.width = 720.0;
        menu.tick = false;
        menu.hint = "Oldest first. Enter starts the rebase".into();
        menu.footer = vec![
            ("Alt+\u{2191}\u{2193}".into(), "move".into()),
            ("p r s f d".into(), "pick, reword, squash, fixup, drop".into()),
            ("Enter".into(), "start".into()),
        ];
        self.git_actions.rebase = Some(todo);
        self.menu = Some((MenuKind::Rebase, menu));
        self.redraw = true;
    }

    /// The list's own keys. Others go to the menu.
    pub(super) fn rebase_key(&mut self, key: Key, mods: Mods) -> bool {
        let Some(mut todo) = self.git_actions.rebase.clone() else { return false };
        if let Some((_, menu)) = &mut self.menu
            && let Some(i) = menu.chosen().and_then(|e| e.key.parse().ok())
        {
            todo.selected = i;
        }
        let verb = match key {
            Key::Up | Key::Down if mods.alt && !mods.ctrl => { todo.shift(key == Key::Up); None }
            Key::Char(c) if !mods.ctrl && !mods.alt => match c {
                'p' => Some(Verb::Pick), 'r' => Some(Verb::Reword), 's' => Some(Verb::Squash),
                'f' => Some(Verb::Fixup), 'd' => Some(Verb::Drop), _ => return true,
            },
            _ => return false,
        };
        if let Some(verb) = verb {
            let step = &mut todo.steps[todo.selected];
            step.verb = verb;
            if verb == Verb::Reword {
                let message = step.message.clone().unwrap_or_else(|| step.subject.clone());
                self.git_actions.rebase = Some(todo);
                self.menu = None;
                self.ask(PromptKind::GitRebaseReword);
                if let Some(p) = &mut self.prompt { (p.cur, p.text) = (message.len(), message); }
                return true;
            }
        }
        self.rebase_show(todo);
        true
    }

    pub(super) fn rebase_reworded(&mut self, message: &str) {
        self.prompt = None;
        let Some(mut todo) = self.git_actions.rebase.clone() else { return };
        if message.trim().is_empty() { self.message = Some("A reworded commit needs a message".into()); }
        let step = &mut todo.steps[todo.selected];
        if message.trim().is_empty() { step.verb = Verb::Pick; step.message = None; } else { step.message = Some(message.trim().into()); }
        self.rebase_show(todo);
    }

    /// Enter on the list: confirm, then run it.
    pub(super) fn rebase_pick(&mut self) {
        let Some(todo) = self.git_actions.rebase.clone() else { return };
        if todo.root != self.root { self.git_actions.rebase = None; return; }
        if let Some(problem) = todo.problem() {
            self.message = Some(problem.into());
            return self.rebase_show(todo);
        }
        if self.git_dirty_tabs(None) {
            self.message = Some("Save or close unsaved tabs before rebasing".into());
            return self.rebase_show(todo);
        }
        let changed = todo.steps.iter().filter(|s| s.verb != Verb::Pick).count();
        self.git_choices("Rewrite history?", vec![
            Choice::new(format!("Rebase {} commits", todo.steps.len()), format!("{changed} changed. This rewrites their IDs; pushed commits need a force push"), Action::RebaseRun(todo)),
            Choice::new("Back to the list", "Change the list before running it", Action::RebaseEdit),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn repository(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ld-rebase-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@e"], &["config", "user.name", "T"]] { git(&dir, args); }
        for (i, subject) in ["base", "one", "two", "three", "four"].iter().enumerate() {
            std::fs::write(dir.join(format!("{subject}.txt")), format!("{i}\n")).unwrap();
            git(&dir, &["add", "."]); git(&dir, &["commit", "-qm", subject]);
        }
        dir
    }

    fn subjects(dir: &Path) -> Vec<String> {
        git(dir, &["log", "--reverse", "--format=%s"]).lines().map(String::from).collect()
    }

    #[test]
    fn the_list_checks_itself_and_moves_steps() {
        let step = |verb, id: &str| Step { verb, id: id.into(), subject: id.into(), message: None };
        let mut t = Todo { root: PathBuf::new(), base: "b".into(), head: "h".into(), steps: vec![step(Verb::Fixup, "a"), step(Verb::Pick, "b")], selected: 0 };
        assert!(t.problem().is_some());
        t.shift(false);
        assert_eq!((t.selected, t.steps[1].id.as_str()), (1, "a"));
        assert_eq!(t.problem(), None);
        assert_eq!(t.script(), "pick b b\nfixup a a\n");
        t.steps[0].verb = Verb::Reword;
        assert!(t.problem().is_some());
    }

    #[test]
    fn a_real_rebase_reorders_rewords_squashes_and_drops() {
        let dir = repository("run");
        let r = Repo::open(&dir).unwrap();
        let base = git(&dir, &["rev-parse", "HEAD~4"]).trim().to_string();
        let Answer::RebaseTodo(mut t) = todo(&r, &base).unwrap() else { panic!() };
        assert_eq!(t.steps.iter().map(|s| s.subject.as_str()).collect::<Vec<_>>(), ["one", "two", "three", "four"]);
        // Four goes first, one is reworded and three is melded into two.
        t.selected = 3; t.shift(true); t.shift(true); t.shift(true);
        t.steps[1].verb = Verb::Reword; t.steps[1].message = Some("one, reworded".into());
        t.steps[3].verb = Verb::Fixup;
        let Answer::Message(..) = run(&r, &t).unwrap() else { panic!() };
        assert_eq!(subjects(&dir), ["base", "four", "one, reworded", "two"]);
        assert!(dir.join("three.txt").exists());
        assert!(!folder(&r).unwrap().exists());
        // A moved HEAD is refused.
        let Answer::RebaseTodo(mut t) = todo(&r, &base).unwrap() else { panic!() };
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "later"]);
        t.steps[0].verb = Verb::Drop;
        assert!(run(&r, &t).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_conflict_stops_the_rebase_and_continue_keeps_later_rewords() {
        let dir = repository("conflict");
        std::fs::write(dir.join("one.txt"), "changed by four\n").unwrap();
        git(&dir, &["commit", "-qam", "five"]);
        let r = Repo::open(&dir).unwrap();
        let base = git(&dir, &["rev-parse", "HEAD~5"]).trim().to_string();
        let Answer::RebaseTodo(mut t) = todo(&r, &base).unwrap() else { panic!() };
        // "five" before "one" conflicts: it changes a file not yet added.
        t.selected = 4; for _ in 0..4 { t.shift(true); }
        t.steps[4].verb = Verb::Reword; t.steps[4].message = Some("four, reworded".into());
        let Answer::Menu(title, choices) = run(&r, &t).unwrap() else { panic!("expected the conflicts") };
        assert!(title.contains("conflicts") && !choices.is_empty());
        assert_eq!(super::super::integration::state(&dir), Some("rebase"));
        // Resolve by keeping theirs, then continue with the rebase's editor.
        git(&dir, &["checkout", "--theirs", "--", "one.txt"]);
        git(&dir, &["add", "one.txt"]);
        let mut command = repo::command(&dir);
        editors(&r, &mut command);
        let ok = command.args(["rebase", "--continue"]).output().unwrap();
        if !ok.status.success() {
            // "one" now conflicts with the file "five" made; keep "one"'s.
            git(&dir, &["checkout", "--theirs", "--", "one.txt"]);
            git(&dir, &["add", "one.txt"]);
            let mut command = repo::command(&dir);
            editors(&r, &mut command);
            assert!(command.args(["rebase", "--continue"]).output().unwrap().status.success());
        }
        assert_eq!(super::super::integration::state(&dir), None);
        assert_eq!(subjects(&dir).last().map(String::as_str), Some("four, reworded"));
        tidy(&r);
        assert!(!folder(&r).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
