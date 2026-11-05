//! Dialogs over the window for what needs a name or an answer: saving as,
//! making a file or folder, renaming, copying, exporting, and moving to
//! the trash. Each is a field, or a question, with buttons for its
//! choices; the keys are as before: Enter, Shift+Enter, Tab and Esc.

use super::*;
use crate::icons::{self, Icon};
use crate::ui::{Rect, Variant, px};

/// A control of a dialog.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum DialogHit {
    Field,
    Cancel,
    /// The main choice: Enter.
    Confirm,
    /// The other choice: Shift+Enter, as deleting for good.
    Alternate,
}

/// What a dialog says: its title, what it asks, and its buttons, the
/// main one last.
struct Words {
    title: String,
    note: String,
    confirm: &'static str,
    alternate: Option<&'static str>,
    icon: Icon,
}

impl App {
    fn dialog_words(&self, p: &Prompt) -> Words {
        let (title, note, confirm, icon) = match p.kind {
            PromptKind::TerminalFind => (
                "Find in terminal history",
                "Literal, case-sensitive text. F3 finds next; Shift+F3 finds previous.",
                "Find",
                Icon::Search,
            ),
            PromptKind::Reload => {
                return Words {
                    title: "File changed on disk".into(),
                    note: "Keep your edits, or reload and discard them. Save writes kept edits.".into(),
                    confirm: "Keep edits",
                    alternate: Some("Reload disk"),
                    icon: Icon::Alert,
                };
            }
            PromptKind::SearchInclude => (
                "Include paths",
                "Comma-separated globs, such as src/**,*.py. Empty includes all.",
                "Set",
                Icon::Search,
            ),
            PromptKind::SearchExclude => {
                ("Exclude paths", "Comma-separated globs, such as tests/**. Empty excludes none.", "Set", Icon::Search)
            }
            PromptKind::AgentWorktree => ("New agent worktree", "Create a branch and open an isolated project window. Use letters, digits, hyphens or underscores.", "Create", Icon::GitBranch),
            PromptKind::InlineEdit => (
                "Edit selection with the agent",
                "Describe the change. Tab keeps the proposed replacement; Esc discards it.",
                "Edit",
                Icon::SquarePen,
            ),
            PromptKind::ProjectReplace => (
                "Replace project matches",
                "Replacement for the last project search. Review the preview before applying.",
                "Preview",
                Icon::Replace,
            ),
            PromptKind::LspRename => {
                ("Rename symbol", "Changes stay unsaved in tabs for review.", "Rename", Icon::FileCode)
            }
            PromptKind::DebugInput => {
                let (title, note) = match self.debugger.input_action.as_str() {
                    "configuration" => ("Debug configuration", "Name from .lightdown/debug.json. Empty uses the configured default."),
                    "thread" => ("Select thread", "Numeric thread ID from the paused Debug tab."),
                    "condition" => ("Breakpoint condition", "Python expression at the current line. Empty removes the condition."),
                    "logpoint" => ("Logpoint expression", "Evaluate and print without stopping. Empty makes a regular breakpoint."),
                    "watch" => ("Toggle watch expression", "Python expression in the selected frame. Enter an existing expression to remove it."),
                    "evaluate" => ("Evaluate in paused frame", "Python expression; calls can change the running program."),
                    "args" => ("Script arguments", "JSON string array, for example [\"--verbose\", \"file name\"]."),
                    "stdin" => ("Script standard input", "JSON string, for example \"answer\\n\". Supplied from the start of the next run."),
                    _ => ("Debug pytest test", "Pytest node ID, for example tests/test_app.py::test_name. Saves required."),
                };
                (title, note, "Apply", Icon::Play)
            }
            PromptKind::GitStashMessage => ("Stash saved changes", "Include untracked files. Save open edits first.", "Stash", Icon::GitBranch),
            PromptKind::GitCompareRevisions => ("Compare revisions", "Enter two branch names or commit IDs separated by a space.", "Compare", Icon::GitBranch),
            PromptKind::GithubReviewLine => ("Line review comment", "Comment body", "Add locally", Icon::GitBranch),
            PromptKind::GithubReply => ("Reply to review thread", "Asks before sending to GitHub. Up to 60 KB.", "Reply", Icon::GitBranch),
            PromptKind::GithubReviewBody => ("Review body", "Sent with the pending line comments, after you confirm. Optional for an approval.", "Review", Icon::GitBranch),
            PromptKind::GitRebaseReword => ("Reword commit", "The new message for this commit. The rebase starts after Enter on the list.", "Reword", Icon::GitBranch),
            PromptKind::GitLogSearch => ("Search repository history", "Message text, author:name or path:src/file.rs. Latest 200 matching commits.", "Search", Icon::GitBranch),
            PromptKind::GitAmend => ("Amend HEAD", "Rewrites HEAD with this message and the staged tree. Review staged changes first.", "Amend", Icon::GitBranch),
            PromptKind::GitBranch => {
                ("Create branch", "Create and switch from HEAD. Save open edits first.", "Create", Icon::GitBranch)
            }
            PromptKind::GitCommit => (
                "Commit staged changes",
                "Review the message. Only the staged changes will be committed.",
                "Commit",
                Icon::GitBranch,
            ),
            PromptKind::SaveAs => ("Save as", "Type a path; Tab completes it.", "Save", Icon::File),
            PromptKind::ThemeImport => {
                ("Import theme", "A Zed theme or VS Code colour theme JSON; Tab completes.", "Import", Icon::Sun)
            }
            PromptKind::New => {
                ("New file", "A name ending in / makes a folder. Tab completes.", "Create", Icon::FilePlus)
            }
            PromptKind::NewFolder => {
                ("New folder", "Folders that are not there yet are made too.", "Create", Icon::FolderPlus)
            }
            PromptKind::Rename => {
                ("Rename or move", "Give a new name, or a folder to move it into.", "Rename", Icon::File)
            }
            PromptKind::Copy => ("Save a copy", "The copy is written; this file stays open.", "Save copy", Icon::Copy),
            PromptKind::Export => {
                ("Export as PDF", "An earlier export of this name is written over.", "Export", Icon::FileText)
            }
            PromptKind::ExportDocument => ("Export document", "HTML embeds local images and math. DOCX/EPUB need pandoc.", "Export", Icon::FileText),
            PromptKind::ExportTex => ("Export as LaTeX", "The LaTeX the PDF is built from.", "Export", Icon::FileCode),
            PromptKind::Trash => {
                let name = match &p.subject {
                    Some(path) => crate::tools::short(&self.root, path),
                    None => self.name(),
                };
                let slash = if p.subject.as_ref().is_some_and(|s| s.is_dir()) { "/" } else { "" };
                return Words {
                    title: format!("Move \"{name}{slash}\" to the trash?"),
                    note: "It can be restored from the trash. Deleting it permanently cannot be undone.".into(),
                    confirm: "Move to trash",
                    alternate: Some("Delete permanently"),
                    icon: Icon::Trash,
                };
            }
            PromptKind::Find | PromptKind::Replace => ("", "", "", Icon::Search),
        };
        Words { title: title.into(), note: note.into(), confirm, alternate: None, icon }
    }

    /// Where the dialog lies, as last drawn.
    pub(super) fn dialog_rect(&self) -> Option<Rect> {
        self.dialog_panel.filter(|_| self.prompt.as_ref().is_some_and(|p| !p.finds()))
    }

    /// A click on a dialog's control.
    pub(super) fn dialog_hit(&mut self, hit: DialogHit) {
        match hit {
            DialogHit::Field => {}
            DialogHit::Cancel => self.prompt = None,
            DialogHit::Confirm => self.submit(false),
            DialogHit::Alternate => self.submit(true),
        }
    }

    /// Draws the dialog for a prompt that is not a find.
    pub(super) fn draw_dialog(&mut self, c: &mut Canvas) {
        self.dialog_panel = None;
        let Some(p) = self.prompt.as_ref().filter(|p| !p.finds()) else { return };
        let words = self.dialog_words(p);
        let (text, cur, trash) = (p.text.clone(), p.cur, matches!(p.kind, PromptKind::Trash | PromptKind::Reload));
        let s = self.scale;
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let pad = px(20.0, s);
        let size = px(theme::UI_TEXT, s);
        let large = px(theme::LARGE + 1.0, s);
        let small = px(theme::SMALL, s);
        let pw = px(520.0, s).min(self.w - px(24.0, s)).round();
        let field_h = px(34.0, s);
        let btn_h = px(30.0, s);
        let error = self.message.clone().filter(|_| !trash || self.message.is_some());
        let mut ph = pad + px(22.0, s) + px(6.0, s) + px(18.0, s) + px(16.0, s) + btn_h + pad;
        if !trash {
            ph += field_h + px(14.0, s);
        }
        if error.is_some() {
            ph += px(22.0, s);
        }
        let x = ((self.w - pw) / 2.0).round();
        let y = (self.title_h() + px(48.0, s)).min((self.h - ph) / 2.0).max(px(8.0, s)).round();
        let r = (x, y, pw, ph);
        self.dialog_panel = Some(r);
        ui::surface(c, r, px(theme::RADIUS_2XL, s), s);

        // The title, with an icon, and what is asked.
        let mut pen = y + pad;
        let isz = px(theme::ICON, s);
        let ink = if trash { theme::danger() } else { theme::icon() };
        icons::draw(c, words.icon, x + pad, (pen + (px(22.0, s) - isz) / 2.0).round(), isz, ink);
        let tx = x + pad + isz + px(10.0, s);
        let title = ui::fit(fonts, font::UI_BOLD, large, &words.title, x + pw - pad - tx);
        ui::text(c, fonts, tx, ui::base(pen, px(22.0, s), large), &title, font::UI_BOLD, large, theme::fg());
        pen += px(22.0, s) + px(6.0, s);
        let note = ui::fit(fonts, font::UI, size, &words.note, pw - pad * 2.0);
        ui::text(c, fonts, x + pad, ui::base(pen, px(18.0, s), size), &note, font::UI, size, theme::dim());
        pen += px(18.0, s) + px(14.0, s);

        // The field, its end in view.
        if !trash {
            let f = (x + pad, pen, pw - pad * 2.0, field_h);
            ui::input(c, f, true, s);
            let fpad = px(10.0, s);
            let room = f.2 - fpad * 2.0;
            let before = crate::bidi::caret_x(fonts,font::UI,size,&text,cur.min(text.len()));
            let offset = (before - room + px(4.0, s)).max(0.0);
            c.clip(f.0 + 1.0, f.1, f.2 - 2.0, f.3);
            ui::text(c, fonts, f.0 + fpad - offset, ui::base(f.1, f.3, size), &text, font::UI, size, theme::fg());
            ui::remember_input(20,&text,font::UI,size,f,f.0+fpad-offset);
            ui::caret(c, f.0 + fpad - offset + before, f.1 + f.3 / 2.0, size * 1.35, s);
            c.unclip();
            self.push_hit(f, Hit::Dialog(DialogHit::Field), "", "");
            pen += field_h + px(14.0, s);
        }
        if let Some(error) = &error {
            let said = ui::fit(fonts, font::UI, small, error, pw - pad * 2.0 - px(20.0, s));
            let isz = px(13.0, s);
            icons::draw(c, Icon::Alert, x + pad, (pen + (px(16.0, s) - isz) / 2.0).round(), isz, theme::danger());
            ui::text(
                c,
                fonts,
                x + pad + isz + px(6.0, s),
                ui::base(pen, px(16.0, s), small),
                &said,
                font::UI,
                small,
                theme::danger(),
            );
            pen += px(22.0, s);
        }

        // The buttons, the main one at the right; the keys at the left.
        let by = pen + px(2.0, s);
        let mut bx = x + pw - pad;
        let mut buttons = vec![(
            words.confirm,
            if trash { Variant::Destructive } else { Variant::Primary },
            DialogHit::Confirm,
            "Enter",
        )];
        if let Some(alt) = words.alternate {
            buttons.push((alt, Variant::Outline, DialogHit::Alternate, "Shift+Enter"));
        }
        buttons.push(("Cancel", Variant::Ghost, DialogHit::Cancel, "Esc"));
        for (label, variant, hit, keys) in buttons {
            let bw = ui::button_width(fonts, label, false, s);
            bx -= bw;
            let br = (bx, by, bw, btn_h);
            ui::button(c, fonts, br, label, None, variant, ui::inside(br, Some(self.pointer)), s);
            self.push_hit(br, Hit::Dialog(hit), "", keys);
            bx -= px(8.0, s);
        }
        let hints: &[(&str, &str)] =
            if trash { &[("Enter", "trash"), ("Esc", "keep")] } else { &[("Tab", "complete"), ("Esc", "cancel")] };
        let mut hx = x + pad;
        for (keys, does) in hints {
            let need = ui::kbd_width(fonts, keys, s) + px(6.0, s) + fonts.width(font::UI, small, does) + px(14.0, s);
            if hx + need > bx {
                break;
            }
            hx += ui::kbd(c, fonts, hx, by + btn_h / 2.0, keys, s) + px(6.0, s);
            hx +=
                ui::text(c, fonts, hx, ui::base(by, btn_h, small), does, font::UI, small, theme::faint()) + px(14.0, s);
        }
    }
}
