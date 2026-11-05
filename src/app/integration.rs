//! Repository operation state, including linked worktrees, and its item in
//! the status bar.
use super::chrome::Hit;
use super::repository as repo;
use super::*;
use crate::icons::Icon;
use std::path::Path;

/// The merge, rebase, cherry-pick or revert in progress, if any.
pub(super) fn state(root: &Path) -> Option<&'static str> {
    const FILES: [(&str, &str); 5] = [
        ("rebase-merge", "rebase"), ("rebase-apply", "rebase"),
        ("MERGE_HEAD", "merge"), ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert"),
    ];
    let mut args = vec!["rev-parse"];
    for (file, _) in FILES { args.extend(["--git-path", file]); }
    let paths = repo::run(root, args).ok()?;
    paths.lines().zip(FILES).find_map(|(path, (_, kind))| {
        let path = Path::new(path.trim());
        (if path.is_absolute() { path.to_path_buf() } else { root.join(path) }).exists().then_some(kind)
    })
}

pub(super) fn title(kind: &str) -> String {
    let mut c = kind.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

impl App {
    /// The operation in progress, which leads to Continue, Skip and Abort.
    /// Returns where the next item starts.
    pub(super) fn draw_operation_item(&mut self, c: &mut Canvas, fonts: &mut Fonts, x: f32, right: f32, gap: f32) -> f32 {
        let Some(kind) = self.git.status.as_ref().and_then(|s| s.operation.clone()) else { return x };
        let label = format!("{} in progress", title(&kind));
        let w = fonts.width(font::UI, ui::px(theme::SMALL, self.scale), &label) + ui::px(30.0, self.scale);
        if x + w >= right { return x; }
        let hit = Some((Hit::GitOperation, "Continue, skip or abort", String::new()));
        let end = self.status_item(c, fonts, x + w, &label, Some((Icon::GitBranch, theme::warning())), theme::warning(), hit);
        end + w + gap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn titles_name_the_operation() {
        assert_eq!(title("cherry-pick"), "Cherry-pick");
        assert_eq!(title("rebase"), "Rebase");
    }
}
