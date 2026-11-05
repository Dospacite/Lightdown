//! The command palette: one box for getting anywhere. What is typed is a
//! file of the project to open; after `>`, a command to run; after `:`, a
//! line to go to; after `@`, a heading or definition of the document.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::assist::MenuKind;
use super::*;
use crate::icons::Icon;
use crate::keys::Scope;

/// What the palette lists, by the first character typed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Mode {
    Files,
    Commands,
    Line,
    Symbols,
    ProjectSymbols,
    Buffers,
}

impl Mode {
    fn of(typed: &str) -> Mode {
        match typed.chars().next() {
            Some('>') => Mode::Commands,
            Some(':') => Mode::Line,
            Some('@') => Mode::Symbols,
            Some('#') => Mode::ProjectSymbols,
            Some('%') => Mode::Buffers,
            _ => Mode::Files,
        }
    }
}

type Indexed = (Vec<PathBuf>, Vec<(PathBuf, crate::outline::Item)>);

/// The project's files, listed in the background and kept a while.
#[derive(Default)]
pub(super) struct FileIndex {
    /// The window's roots the files were listed from.
    roots: Vec<PathBuf>,
    pub(super) paths: Vec<PathBuf>,
    pub(super) symbols: Vec<(PathBuf, crate::outline::Item)>,
    at: Option<Instant>,
    pending: Option<Arc<Mutex<Option<Indexed>>>>,
}

/// Commands that the keybinds do not list, with what they are called.
pub(super) const EXTRA: &[(&str, &str)] = &[
    ("diagrams_render", "Markdown: Render document diagrams"),
    ("citations_refresh", "Writing: Refresh project bibliography"),
    ("citation_show", "Writing: Show citation at cursor"),
    ("export_html", "File: Export markdown as HTML"),
    ("export_docx", "File: Export markdown as DOCX"),
    ("export_epub", "File: Export markdown as EPUB"),
    ("check_links", "Markdown: Check local links and anchors"),
    ("backlinks", "Markdown: Show or refresh backlinks"),
    ("note_follow", "Markdown: Follow or create wikilink"),
    ("table_align", "Markdown table: Align columns"),
    ("folder_add", "File: Add folder to window\u{2026}"),
    ("folder_remove", "File: Remove folder from window"),
    ("workspace_save", "File: Save folders as workspace"),
    ("agent_root", "Agent: Choose the folder it works in"),
    ("table_row_insert", "Markdown table: Insert row below"),
    ("table_row_delete", "Markdown table: Delete row"),
    ("table_column_insert", "Markdown table: Insert column after"),
    ("table_column_delete", "Markdown table: Delete column"),
    ("table_sort", "Markdown table: Sort by current column"),
    ("table_next", "Markdown table: Next cell"),
    ("table_previous", "Markdown table: Previous cell"),
    ("table_csv", "Markdown table: Paste CSV as table"),

    ("paste_link", "Edit: Paste URL over selection as link"),
    ("paste_image", "Markdown: Paste image into assets"),
    ("terminal_command_previous", "Terminal: Previous command"),
    ("terminal_command_next", "Terminal: Next command"),
    ("terminal_output_copy", "Terminal: Copy last command output"),
    ("terminal_output_select", "Terminal: Select last command output"),
    ("terminal_rerun", "Terminal: Rerun last command"),
    ("terminal_find", "Terminal: Find in history"),
    ("terminal_find_next", "Terminal: Next history match"),
    ("terminal_find_previous", "Terminal: Previous history match"),
    ("local_history", "File: Local history"),
    ("http_stop", "HTTP: Stop request"),
    ("coverage_toggle", "Tests: Show or hide coverage in the gutter"),
    ("md_heading_links", "Markdown: Update links to renamed headings"),
    ("coverage_reload", "Tests: Reload coverage report"),
    ("project_todos", "Go to: Project TODO comments"),
    ("recent", "File: Recent projects and files"),
    ("bookmark_project_next", "Bookmarks: Next across project"),
    ("bookmark_project_previous", "Bookmarks: Previous across project"),
    ("bookmark_list", "Bookmarks: List all"),
    ("bookmark_clear", "Bookmarks: Clear in file"),
    ("copy_reference", "File: Copy source line reference"),
    ("copy_github", "File: Copy GitHub link to source lines"),
    ("minimap", "View: Toggle minimap"),
    ("sticky_headers", "View: Toggle sticky headers"),
    ("file_indent", "File: Set indentation for this file"),
    ("file_reindent", "File: Reindent with this unit"),
    ("file_eol", "File: Change line endings"),
    ("file_encoding", "File: Change encoding"),
    ("file_bom", "File: Add or remove the byte order mark"),
    ("indent_guides", "View: Toggle indent guides"),
    ("ruler", "View: Cycle column ruler 80, 100 or off"),
    ("grammar_check", "Writing: Check grammar and style"),
    ("grammar_clear", "Writing: Clear grammar and style marks"),
    ("project_settings", "Preferences: Reload project settings"),
    ("vim_mode", "Preferences: Toggle Vim mode"),
    ("helix_mode", "Preferences: Toggle Helix mode"),
    ("helix_buffers", "View: Choose open tab"),
    ("config_create", "Preferences: Create project config file"),
    ("editor_right", "View: Add editor view to the right"),
    ("editor_below", "View: Add editor view below"),
    ("editor_mirror", "View: Show this file in a second view, or close it"),
    ("cancel_input", "Edit: Cancel input composition"),
    ("counts", "Text: Show word, character and reading counts"),
    ("text_sort", "Text: Sort lines"),
    ("text_unique", "Text: Remove duplicate lines"),
    ("text_upper", "Text: Uppercase selection or word"),
    ("text_lower", "Text: Lowercase selection or word"),
    ("text_snake", "Text: Convert to snake_case"),
    ("text_camel", "Text: Convert to camelCase"),
    ("text_pascal", "Text: Convert to PascalCase"),
    ("text_kebab", "Text: Convert to kebab-case"),
    ("text_title", "Text: Convert to Title Case"),
    ("text_join", "Text: Join lines"),
    ("agent_remove_images", "Agent: Remove screenshots from draft"),
    ("theme_select", "Theme: Select theme"),
    ("theme_import", "Theme: Import Zed or VS Code theme\u{2026}"),
    ("theme_mode_system", "Theme: Follow the desktop's dark or light preference"),
    ("icon_theme_select", "Theme: Select icon theme"),
    ("font_buffer_select", "Font: Select buffer font"),
    ("font_ui_select", "Font: Select interface font"),
    ("debug_attach", "Debug: Attach to process"),
    ("debug_template", "Debug: Create configuration from template"),
    ("debug_debugpy", "Debug: Start Python script with debugpy"),
    ("debug_native", "Debug: Start native adapter"),
    ("debug_exceptions", "Debug: Select exception breakpoints"),
    ("debug_raised", "Debug: Toggle break on raised exceptions"),
    ("debug_uncaught", "Debug: Toggle break on uncaught exceptions"),
    ("debug_inline_values", "Debug: Toggle inline values"),
    ("debug_variables", "Debug: Browse variables"),
    ("debug_thread", "Debug: Select thread by ID"),
    ("debug_configuration", "Debug: Select named configuration"),
    ("debug_condition", "Debug: Set breakpoint condition"),
    ("debug_logpoint", "Debug: Set logpoint expression"),
    ("debug_watch", "Debug: Toggle watch expression"),
    ("debug_evaluate", "Debug: Evaluate in paused frame"),
    ("debug_args", "Debug: Set script arguments"),
    ("debug_stdin", "Debug: Set script standard input"),
    ("debug_pytest", "Debug: Run one pytest test"),
    ("debug_continue", "Debug: Continue"),
    ("debug_frames", "Debug: Inspect stack frame"),
    ("debug_stop", "Debug: Stop"),
    ("debug_show", "Debug: Show variables and call stack"),
    ("mcp_refresh", "Agent: Refresh MCP tool lists"),
    ("mcp_connect", "Agent: Connect project MCP servers"),
    ("mcp_disconnect", "Agent: Disconnect MCP servers"),
    ("editor_split", "View: Toggle two files side by side"),
    ("editor_other", "View: Focus other editor"),
    ("find_selection", "Search: Toggle find in selection"),
    ("search_regex", "Search: Toggle regular expressions"),
    ("search_word", "Search: Toggle whole words"),
    ("search_preserve_case", "Search: Toggle preserve case"),
    ("search_case", "Search: Toggle match case"),
    ("search_include", "Search: Set included paths"),
    ("search_exclude", "Search: Set excluded paths"),
    ("search_review", "Search: Review pending project replacements"),
    ("search_apply", "Search: Apply pending project replacements"),
    ("search_replace", "Search: Replace project matches"),
    ("multibuffer_search", "Search: Open results in editor"),
    ("multibuffer_problems", "Problems: Open in editor"),
    ("multibuffer_references", "Code: Find all references in editor"),
    ("multibuffer_open", "Multibuffer: Open file at cursor"),
    ("multibuffer_expand_up", "Multibuffer: Expand excerpt upward"),
    ("multibuffer_expand_down", "Multibuffer: Expand excerpt downward"),
    ("pycell_selection", "Python: Run selection in kernel"),
    ("terminal_selection", "Terminal: Run selected text"),
    ("pycell_run", "Python: Run current percent cell"),
    ("pycell_next", "Python: Run cell and advance"),
    ("pycell_output", "Python: Show cell output"),
    ("pycell_interrupt", "Python: Interrupt cell kernel"),
    ("pycell_restart", "Python: Restart cell kernel"),
    ("panel_tabs", "View: Choose right panel tab"),
    ("tests_filter", "Tests: Pick a test to run"),
    ("tests_filter_focus", "Tests: Filter tests"),
    ("tests_refresh", "Tests: Refresh tests"),
    ("tests_cargo", "Tests: Discover cargo tests"),
    ("tests_pytest", "Tests: Discover pytest tests"),
    ("tests_all", "Tests: Run all tests"),
    ("tests_file", "Tests: Run tests in this file"),
    ("tests_cursor", "Tests: Run test at cursor"),
    ("tests_debug", "Tests: Debug test at cursor"),
    ("tests_failed", "Tests: Rerun failed tests"),
    ("tests_coverage", "Tests: Run all with coverage"),
    ("tests_compare", "Tests: Compare expected and actual"),
    ("tests_output", "Tests: Show output"),
    ("tests_stop", "Tests: Stop tests"),
    ("agent_summarize", "Agent: Summarize older turns"),
    ("ts_install", "Code: Install language grammar"),
    ("ts_list", "Code: List installed grammars"),
    ("ts_remove", "Code: Remove grammar"),
    ("fold", "Code: Fold or unfold current block"),
    ("unfold", "Code: Unfold all blocks"),
    ("lsp_hover", "Code: Hover documentation"),
    ("lsp_ask_fix", "Code: Ask agent to fix diagnostic"),
    ("lsp_inline_diagnostics", "Code: Show or hide inline diagnostics"),
    ("lsp_diagnostics", "Code: Show language diagnostics"),
    ("lsp_format_selection", "Code: Format selection"),
    ("lsp_format", "Code: Format document with language server"),
    ("lsp_inlays", "Code: Show or hide inlay hints"),
    ("lsp_stop", "Code: Stop language server"),
    ("lsp_server_log", "Code: Show language server log"),
    ("lsp_lenses", "Code: Run code lens"),
    ("lsp_update", "Code: Update language servers"),
    ("lsp_restart", "Code: Restart language server"),
    ("task_watch", "Task: Toggle watch on save for tests or cargo check"),
    ("agent_attach", "Agent: Attach file, folder or symbol"),
    ("agent_prompts", "Agent: Saved project prompts"),
    ("agent_terminals", "Agent: Show running command terminals"),
    ("agent_terminal", "Agent: Ask about terminal output"),
    ("agent_task", "Agent: Ask about failed task"),
    ("agent_build", "Agent: Ask about failed PDF build"),
    ("notebook_variables", "Notebook: Variable explorer"),
    ("notebook_kernel", "Notebook: Select kernel"),
    ("python_environment", "Python: Select project interpreter"),
    ("env_loader", "Environment: Use direnv/mise/Nix for this project"),
    ("env_reload", "Environment: Reload project environment"),
    ("agent_steer", "Agent: Queue follow-up"),
    ("agent_unqueue", "Agent: Return queued follow-ups to draft"),
    ("copilot_sign_in", "Copilot: Sign in"),
    ("copilot_sign_out", "Copilot: Sign out"),
    ("copilot_install", "Copilot: Install language server"),
    ("suggest_auto", "Suggestion: Toggle edit predictions"),
    ("suggest_next_edit", "Suggestion: Suggest next matching edit"),
    ("agent_plan", "Agent: Toggle plan mode"),
    ("agent_plan_approve", "Agent: Approve plan"),
    ("agent_plan_edit", "Agent: Edit proposed plan"),
    ("agent_edit_message", "Agent: Edit and resend earlier message"),
    ("remote_connect", "Remote: Connect project over SSH"),
    ("remote_stop", "Remote: Stop connection or sync"),
    ("remote_disconnect", "Remote: Disconnect project"),
    ("remote_pull", "Remote: Pull files into local mirror"),
    ("remote_push", "Remote: Push saved files to remote"),
    ("remote_push_on_save", "Remote: Toggle push on save"),
    ("remote_forward", "Remote: Forward port…"),
    ("ports_show", "Ports: Show detected ports"),
    ("container_start", "Container: Start or connect"),
    ("container_stop", "Container: Stop"),
    ("container_rebuild", "Container: Rebuild"),
    ("container_log", "Container: Show log"),
    ("agent_worktree", "Agent: New worktree window"),
    ("agent_worktree_overview", "Agent: Worktree overview"),
    ("agent_worktrees", "Agent: Review and merge worktrees"),
    ("agent_fork", "Agent: Fork thread after a turn"),
    ("agent_turns", "Agent: Turn checkpoints"),
    ("agent_hunks", "Agent: Review file hunks in editor"),
    ("agent_changes", "Agent: Review changed files"),
    ("agent_rollback", "Agent: Roll back latest turn"),
    ("git_hunk_staged", "Git: Show staged hunk at cursor"),
    ("git_stage_lines", "Git: Stage selected lines"),
    ("git_unstage_lines", "Git: Unstage selected lines"),
    ("git_hunk_stage", "Git: Stage or unstage previewed hunk"),
    ("git_hunk_revert", "Git: Revert previewed hunk"),
    ("diff_file", "Diff: Compare file with…"),
    ("diff_tabs", "Diff: Compare with open tab"),
    ("diff_clipboard", "Diff: Compare selection with clipboard"),
    ("diff_columns", "Diff: Toggle side-by-side and unified view"),
    ("git_changes", "Git: Source control, stage and commit"),
    ("git_diff", "Git: Diff file against HEAD"),
    ("git_diff_staged", "Git: Diff file against staged"),
    ("git_branches", "Git: Switch, create or delete branches"),
    ("git_fetch", "Git: Fetch all remotes"),
    ("git_merge_branch", "Git: Merge branch…"),
    ("git_merge_no_ff", "Git: Merge branch with no fast-forward…"),
    ("git_rebase", "Git: Rebase onto…"),
    ("git_rebase_interactive", "Git: Interactive rebase…"),
    ("git_pull_rebase", "Git: Pull with rebase"),
    ("git_pull_merge", "Git: Pull with merge"),
    ("git_continue", "Git: Continue"),
    ("git_skip", "Git: Skip"),
    ("git_abort", "Git: Abort"),
    ("git_pull", "Git: Pull with fast-forward only"),
    ("git_push", "Git: Push branch"),
    ("git_stash", "Git: Stash saved changes"),
    ("git_stashes", "Git: List stashes"),
    ("git_stash_message", "Git: Stash saved changes with message"),
    ("git_stash_file", "Git: Stash current file"),
    ("git_compare_revisions", "Git: Compare branches or commits"),
    ("git_stash_pop", "Git: Restore latest stash"),
    ("git_keep_current", "Git: Keep current conflict change"),
    ("git_keep_incoming", "Git: Keep incoming conflict change"),
    ("git_keep_both", "Git: Keep both conflict changes"),
    ("git_merge_editor", "Git: Open merge editor"),
    ("git_merge_close", "Git: Close merge editor"),
    ("git_complete_merge", "Git: Complete merge"),
    ("git_conflict_previous", "Git: Resolve previous conflict"),
    ("git_keep_base", "Git: Keep base conflict change"),
    ("git_keep_reverse", "Git: Keep incoming then current conflict changes"),
    ("git_conflict", "Git: Resolve next conflict"),
    ("git_discard_all", "Git: Discard all unstaged changes"),
    ("git_file_blame", "Git: Toggle blame for file"),
    ("git_blame", "Git: Toggle inline blame"),
    ("github_add_review_line", "GitHub: Add line comment to pending review"),
    ("github_review_comment", "GitHub: Submit review as Comment"),
    ("github_review_approve", "GitHub: Submit review as Approve"),
    ("github_review_changes", "GitHub: Submit review as Request changes"),
    ("github_list", "GitHub: List pull requests"),
    ("github_ci_logs", "GitHub: Show failed CI logs"),
    ("github_status", "GitHub: Refresh current PR CI status"),
    ("github_hide_comments", "GitHub: Hide review comments beside lines"),
    ("github_draft", "GitHub: Ask agent to draft pull request description"),
    ("github_create", "GitHub: Create draft pull request from current buffer"),
    ("git_log", "Git: Repository history"),
    ("git_log_search", "Git: Search repository history"),
    ("git_amend", "Git: Amend HEAD message and staged changes"),
    ("git_line_history", "Git: History of selected lines"),
    ("git_history", "Git: File history"),
    ("git_review", "Git: Choose branch review base"),
    ("format_on_save", "File: Toggle format on save"),
    ("run_configs", "Run: Choose configuration"),
    ("user_commands", "User: List project commands"),
    ("user_command_stop", "User: Stop command"),
    ("task_output", "Task: Show output"),
    ("task_errors", "Task: Show errors"),
    ("task_stop_job", "Task: Stop job…"),
    ("task_stop", "Task: Stop"),
    ("task_rerun", "Task: Rerun last task"),
    ("theme", "Preferences: Switch between dark and light"),
    ("prose_font", "Preferences: Switch the reading font, sans or serif"),
    ("checkpoint", "File: Keep a recovery checkpoint now"),
    ("close_saved", "File: Close saved tabs"),
    ("close_others", "File: Close the other tabs"),
    ("copy_relative", "File: Copy the path within the project"),
    ("collapse", "Explorer: Collapse every folder"),
    ("show_problems", "View: Show problems"),
    ("show_outline", "View: Show the outline"),
    ("show_preview", "View: Show the PDF preview"),
];

/// An icon for a command, by the group its name starts with.
fn command_icon(id: &str, name: &str) -> Icon {
    let group = name.split(':').next().unwrap_or("");
    match (id, group) {
        ("theme", _) if crate::theme::light() => Icon::Moon,
        ("theme", _) => Icon::Sun,
        ("settings", _) | ("prose_font", _) => Icon::Settings,
        ("keys", _) => Icon::Keyboard,
        ("find" | "replace" | "search" | "quick_open", _) => Icon::Search,
        ("goto_line", _) => Icon::Hash,
        ("symbols", _) => Icon::Braces,
        (_, "File") => Icon::File,
        (_, "Terminal") => Icon::Terminal,
        (_, "Agent") => Icon::Message,
        (_, "Notebook") => Icon::Notebook,
        (_, "Explorer") => Icon::Folder,
        (_, "View") => Icon::PanelRight,
        (_, "Suggestion") => Icon::SquarePen,
        (_, "LaTeX" | "Preview") => Icon::FileText,
        _ => Icon::Command,
    }
}

impl App {
    /// Opens the palette with `typed` in it: nothing for files, `>` for
    /// commands, `:` for a line, `@` for a heading or definition.
    pub(super) fn open_palette(&mut self, typed: &str) {
        self.comp = None;
        self.unsuggest();
        self.index_files(typed == "#");
        let mut menu = Menu::new("", Vec::new(), "");
        menu.width = 620.0;
        menu.footer = vec![
            ("\u{2191}\u{2193}".into(), "navigate".into()),
            ("Enter".into(), "open".into()),
            (">".into(), "commands".into()),
            (":".into(), "line".into()),
            ("@".into(), "symbol".into()),
        ];
        menu.set_typed(typed);
        self.menu = Some((MenuKind::Palette, menu));
        self.palette_mode = None;
        self.palette_update();
    }

    /// Lists what the palette offers for what has been typed.
    pub(super) fn palette_update(&mut self) {
        let Some((MenuKind::Palette, menu)) = &self.menu else { return };
        let typed = menu.typed().to_string();
        let mode = Mode::of(&typed);
        let changed = self.palette_mode != Some(mode);
        if mode == Mode::ProjectSymbols { self.language_workspace_symbols(typed.get(1..).unwrap_or("")); }
        let total = self.doc.buf.lines();
        let entries = match mode {
            Mode::Line => Some(self.line_entries(&typed[1..], total)),
            Mode::Buffers if changed => Some(self.modal_buffer_entries()),
            Mode::Files if changed || self.palette_stale => Some(self.file_entries()),
            Mode::Commands if changed => Some(self.command_entries()),
            Mode::Symbols if changed => Some(self.symbol_entries()),
            Mode::ProjectSymbols if changed || self.palette_stale => Some(self.project_symbol_entries()),
            _ => None,
        };
        self.palette_stale = false;
        self.palette_mode = Some(mode);
        let project = self.project_name();
        let Some((_, menu)) = &mut self.menu else { return };
        menu.prefix = if mode == Mode::Files { 0 } else { 1 };
        menu.paths = mode == Mode::Files;
        menu.live = mode == Mode::Line;
        (menu.hint, menu.icon) = match mode {
            Mode::Files => (format!("Go to a file in {project}"), Icon::Search),
            Mode::Commands => ("Run a command".into(), Icon::ChevronRight),
            Mode::Buffers => ("Go to an open tab".into(), Icon::File),
            Mode::Line => (format!("Go to a line, 1 to {total}"), Icon::Hash),
            Mode::Symbols => ("Go to a heading or definition".into(), Icon::Braces),
            Mode::ProjectSymbols => ("Go to a symbol in the project".into(), Icon::Braces),
        };
        if let Some(entries) = entries {
            let keep = menu.typed().to_string();
            menu.set_entries(entries);
            menu.set_typed(&keep);
        }
    }

    /// The entries of a line to go to, as `12` or `12:5`.
    fn line_entries(&self, typed: &str, total: usize) -> Vec<Entry> {
        let mut parts = typed.trim().splitn(2, ':');
        let line = parts.next().and_then(|l| l.trim().parse::<usize>().ok());
        let col = parts.next().and_then(|c| c.trim().parse::<usize>().ok());
        match line {
            Some(n) if n >= 1 && self.reader.is_none() => {
                let n = n.min(total);
                let label = match col {
                    Some(c) => format!("Go to line {n}, column {c}"),
                    None => format!("Go to line {n}"),
                };
                vec![
                    Entry::new(&format!("l\n{n}\n{}", col.unwrap_or(1)), &label, &format!("of {total}"))
                        .icon(Icon::Hash),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// The project's files, the open ones first.
    pub(super) fn file_entries(&self) -> Vec<Entry> {
        let root = &self.root;
        let roots = self.roots();
        // In a window of several folders, a path found in more than one of
        // them is shown after its folder's name.
        let mut seen = std::collections::HashMap::<String, usize>::new();
        if roots.len() > 1 {
            for p in &self.file_index.paths {
                *seen.entry(super::roots::short_in(&roots, p)).or_default() += 1;
            }
        }
        let entry = |path: &Path| {
            let mut short = if roots.len() > 1 { super::roots::short_in(&roots, path) } else { crate::tools::short(root, path) };
            if seen.get(&short).is_some_and(|&n| n > 1) {
                short = self.root_short(path);
            }
            let (dir, name) =
                short.rsplit_once('/').map_or((String::new(), short.clone()), |(d, n)| (d.to_string(), n.to_string()));
            let mut e = Entry::new(&format!("f\n{}", path.to_string_lossy()), &name, &dir);
            e.file = Some(name.clone());
            e
        };
        let open: Vec<PathBuf> = self.tab_files().into_iter().flatten().map(|(p, _)| p).collect();
        let mut out: Vec<Entry> = open.iter().map(|p| entry(p)).collect();
        for e in &mut out {
            e.keys.clear();
        }
        out.extend(self.file_index.paths.iter().filter(|p| !open.contains(p)).map(|p| entry(p)));
        out
    }

    /// The commands that make sense here, with their keys.
    fn command_entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for a in ACTIONS {
            let fits = match a.scope {
                Scope::Notebook => self.nb.is_some(),
                _ => true,
            } && match a.id {
                "preview" | "export_pdf" => self.buildable(),
                "export_tex" => self.latex_exportable(),
                "sync" => matches!(self.doc.doc, Doc::Latex | Doc::Markdown),
                "doc_kind" => self.can_change_kind(),
                "edit_mode" => self.doc.doc == Doc::Markdown && self.reader.is_none(),
                "terminal_close" | "terminal_next" | "terminal_previous" => !self.terms.is_empty(),
                "rename" | "trash" | "reveal" | "copy_path" => self.doc.path.is_some(),
                "quick_open" | "palette" => false,
                _ => true,
            };
            if fits {
                let keys = self.keys_of(a.id);
                out.push(Entry::new(&format!("c\n{}", a.id), a.name, "").keys(keys).icon(command_icon(a.id, a.name)));
            }
        }
        for (id, name) in EXTRA.iter().chain(super::keybindings::COMMANDS) {
            out.push(Entry::new(&format!("c\n{id}"), name, "").keys(self.keys_of(id)).icon(command_icon(id, name)));
        }
        out.extend(self.user_command_entries());
        out
    }

    /// The document's headings or definitions, indented by depth.
    fn symbol_entries(&self) -> Vec<Entry> {
        if self.reader.is_some() {
            return Vec::new();
        }
        let items = self.language_symbols().unwrap_or_else(|| crate::outline::items(&self.doc.buf.text, self.doc.doc));
        items
            .iter()
            .map(|item| {
                let line = self.doc.buf.line_of(item.at) + 1;
                let label = format!("{}{}", "  ".repeat(item.depth.min(6)), item.title);
                Entry::new(&format!("s\n{}", item.at), &label, &format!("line {line}")).icon(Icon::Braces)
            })
            .collect()
    }

    fn project_symbol_entries(&self) -> Vec<Entry> {
        if let Some(entries) = self.language_workspace_entries() { return entries; }
        let open = self.tab_files();
        let mut symbols: Vec<_> = self
            .file_index
            .symbols
            .iter()
            .filter(|(p, _)| !open.iter().flatten().any(|(path, _)| path == p))
            .cloned()
            .collect();
        for (i, tab) in self.file_tabs.iter().enumerate() {
            let pane = if i == self.active_file { &self.doc } else { &tab.state.as_ref().unwrap().pane };
            let Some(path) = &pane.path else { continue };
            symbols
                .extend(crate::outline::items(&pane.buf.text, pane.doc).into_iter().map(|item| (path.clone(), item)));
        }
        symbols
            .into_iter()
            .map(|(path, item)| {
                Entry::new(
                    &format!("p\n{}\n{}", path.to_string_lossy(), item.at),
                    &item.title,
                    &crate::tools::short(&self.root, &path),
                )
                .icon(Icon::Braces)
            })
            .collect()
    }

    /// Does what the palette's chosen entry says.
    pub(super) fn palette_pick(&mut self, key: &str) {
        let mut parts = key.splitn(3, '\n');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("p"), Some(path), Some(at)) => {
                self.jump_to(PathBuf::from(path), at.parse().unwrap_or(0), at.parse().unwrap_or(0))
            }
            (Some("w"), Some(key), _) => self.language_workspace_pick(key),
            (Some("f"), Some(path), _) => self.pick(PathBuf::from(path), false),
            (Some("t"), Some(tab), _) => { if let Ok(tab)=tab.parse::<usize>() { self.activate_file(tab); } },
            (Some("c"), Some(id), _) => self.run_command(id),
            (Some("l"), Some(line), col) => {
                let line: usize = line.parse().unwrap_or(1);
                let col: usize = col.and_then(|c| c.parse().ok()).unwrap_or(1);
                let buf = &self.doc.buf;
                let (start, end) = buf.line(line.saturating_sub(1).min(buf.lines().saturating_sub(1)));
                let at = buf.text[start..end].char_indices().nth(col.saturating_sub(1)).map_or(end, |(i, _)| start + i);
                self.go_to(at);
            }
            (Some("s"), Some(at), _) => {
                let at: usize = at.parse().unwrap_or(0);
                self.go_to(at);
            }
            _ => {}
        }
    }

    /// Runs a command by its name: a shortcut's, or one of the palette's own.
    pub(super) fn run_command(&mut self, id: &str) {
        if self.keymap_command(id) { return; }
        match id {
            "diagrams_render" => self.diagrams_tick(true),
            "citations_refresh" => self.refresh_citations(),
            "citation_show" => self.show_docs(),
            "export_html" | "export_docx" | "export_epub" => self.ask_document_export(&id[7..]),
            "check_links" => self.links_tick(true),
            "backlinks" => self.open_backlinks(),
            "note_follow" => self.follow_note_at_cursor(),
            "table_csv" => { self.focus = Focus::Doc; self.paste_table = true; self.paste = true; }
            id if id.starts_with("table_") => { self.doc.table_tool(&id[6..]); self.edited(); self.collect(); }
            "paste_link" | "paste_image" => { self.focus = Focus::Doc; self.paste = true; }
            "terminal_command_previous" | "terminal_command_next" | "terminal_output_copy" | "terminal_output_select"
            | "terminal_rerun" => self.shell_mark_command(id),
            "terminal_find" => self.ask(PromptKind::TerminalFind),
            "terminal_find_next" => self.terminal_find_step(false),
            "terminal_find_previous" => self.terminal_find_step(true),
            "local_history" => self.git_command("local_history"),
            "recent" => self.open_recent(),
            "folder_add" | "folder_remove" | "workspace_save" | "agent_root" => self.roots_command(id),
            id if id.starts_with("bookmark_") => self.bookmark_command(id),
            id if id.starts_with("multibuffer_") => self.multibuffer_command(&id[12..]),
            "copy_reference" => self.copy_reference(false),
            "copy_github" => self.copy_reference(true),
            "file_indent" | "file_reindent" | "file_eol" | "file_encoding" | "file_bom" => self.file_format_command(id),
            "minimap" => {
                self.cfg.minimap = if self.cfg.minimap != "off" { "off" } else { "on" }.into();
                let _ = self.cfg.save(); self.arrange(); self.doc.redraw = true;
            }
            "word_wrap" => self.toggle_word_wrap(),
            "vim_mode" | "helix_mode" => {
                let mode = if id == "helix_mode" { "helix" } else { "vim" };
                self.cfg.modal_editing = if self.cfg.modal_editing == mode { "none" } else { mode }.into();
                self.doc.set_modal_editing(&self.cfg.modal_editing);
                self.doc.redraw = true;
                let _ = self.cfg.save();
            }
            "indent_guides" => {
                self.cfg.indent_guides = !self.cfg.indent_guides;
                let _ = self.cfg.save();
            }
            "grammar_check" => self.grammar_check(),
            "grammar_clear" => self.grammar_clear(),
            "sticky_headers" => { self.doc.sticky = !self.doc.sticky; self.doc.redraw = true; }
            "ruler" => {
                self.cfg.ruler = match self.cfg.ruler {
                    80 => 100,
                    100 => 0,
                    _ => 80,
                };
                let _ = self.cfg.save();
            }
            "config_create" => self.config_create_list(),
            id if id.starts_with("config_create:") => self.config_create(&id[14..]),
            "project_settings" => {
                self.message = Some(match self.cfg.load_project(&self.root) {
                    Ok(()) => "Project settings reloaded".into(),
                    Err(e) => e,
                });
                self.arrange();
            }
            "cancel_input" => self.input_clear(),
            "counts" => self.show_counts(),
            id if id.starts_with("text_") => {
                self.doc.text_tool(&id[5..]);
                self.edited();
                self.collect();
            }
            "agent_remove_images" => self.ai.images.clear(),
            "debug_show" => self.debug_command("show"),
            id if id.starts_with("debug_") => self.debug_command(&id[6..]),
            "mcp_refresh" => { if !self.ai.busy() { self.mcp.refresh(); self.message = Some("Refreshing MCP tools…".into()); } },
            "mcp_connect" => self.mcp_connect(),
            "mcp_disconnect" => self.mcp_disconnect(),
            "editor_right" => self.split_add(false),
            "editor_below" => self.split_add(true),
            "editor_split" => self.split_editor(),
            "editor_other" => self.focus_next_view(),
            "editor_mirror" => self.mirror_toggle(false),
            id if id.starts_with("pycell_") => self.script_command(&id[7..]),
            "panel_tabs" => self.open_panel_tabs(),
            id if id.starts_with("tests_") => self.tests_command(id),
            "agent_summarize" => {
                if !self.ai.busy() && !self.maybe_summarize(true, false) {
                    self.message = Some("Keep at least two recent turns before summarizing older turns".into());
                }
            }
            "inline_edit" => self.ask(PromptKind::InlineEdit),
            "bracket_jump" => self.doc.jump_bracket(false),
            "bracket_select" => self.doc.jump_bracket(true),
            "cursor_all" => self.doc.select_all_occurrences(),
            "cursor_skip" => self.doc.skip_occurrence(),
            "selection_expand" => self.selection_change(false),
            "selection_shrink" => self.selection_change(true),
            "cursor_next" => self.doc.add_next_cursor(),
            "cursor_above" => self.doc.add_line_cursor(false),
            "cursor_below" => self.doc.add_line_cursor(true),
            "ts_install" => self.grammars_menu(super::grammar_menu::Mode::Install),
            "ts_list" => self.grammars_menu(super::grammar_menu::Mode::List),
            "ts_remove" => self.grammars_menu(super::grammar_menu::Mode::Remove),
            "fold" => self.doc.fold_toggle(),
            "unfold" => self.doc.unfold_all(),
            "find_selection" => self.toggle_find_selection(),
            id if id.starts_with("search_") => self.search_option(&id[7..]),
            id if id.starts_with("lsp_") => self.language_command(&id[4..]),
            "terminal_selection" => self.terminal_run_selection(),
            "task_watch" => self.toggle_watch(),
            "agent_attach" => self.open_attachments(),
            "agent_prompts" => self.open_saved_prompts(),
            "agent_terminals" => self.show_agent_terminal(),
            "agent_terminal" => self.ask_terminal(),
            "agent_task" => self.ask_task(),
            "agent_build" => self.ask_build(),
            "notebook_variables" => self.open_variables(),
            "notebook_kernel" => self.open_kernels(),
            "python_environment" => self.open_environment(),
            "env_loader" => self.toggle_env_loader(),
            "env_reload" => self.reload_env_loader(),
            "agent_steer" => self.send(),
            "agent_unqueue" => self.return_followups(),
            "suggest_auto" => { self.cfg.auto_suggest = !self.cfg.auto_suggest; self.unsuggest(); let _ = self.cfg.save(); self.message = Some(format!("Edit predictions {}", if self.cfg.auto_suggest { "on" } else { "off" })); },
            id if id.starts_with("copilot_") => self.copilot_action(&id[8..]),
            "suggest_next_edit" => self.suggest_next_edit(),
            "agent_plan" => self.toggle_plan(),
            "agent_plan_approve" => self.approve_plan(),
            "agent_plan_edit" => self.edit_plan(),
            "agent_edit_message" => self.open_agent_history(false),
            id if id.starts_with("remote_") => self.remote_action(&id[7..]),
            id if id.starts_with("container_") => self.container_action(&id[10..]),
            "ports_show" => self.show_ports(),
            "agent_worktree" => self.ask(PromptKind::AgentWorktree),
            "agent_worktree_overview" => self.worktree_overview(),
            "agent_worktrees" => self.list_agent_worktrees(),
            "agent_fork" => self.open_agent_history(true),
            "agent_turns" => self.open_agent_turns(),
            "agent_hunks" => self.open_agent_hunks(),
            "agent_hunk_keep" => self.agent_hunk(None, true),
            "agent_hunk_undo" => self.agent_hunk(None, false),
            "agent_changes" => self.open_agent_changes(),
            "agent_rollback" => self.rollback_latest_agent_turn(),
            id if id.starts_with("github_") => self.github_command(id),
            "diff_file" | "diff_tabs" | "diff_clipboard" => self.git_command(id),
            "diff_columns" => self.doc.toggle_comparison(),
            id if id.starts_with("git_") => self.git_command(id),
            "format_on_save" => {
                self.cfg.format_on_save = !self.cfg.format_on_save;
                let _ = self.cfg.save();
                self.message = Some(format!("Format on save {}", if self.cfg.format_on_save { "on" } else { "off" }));
            }
            "run_configs" => self.run_configs(false),
            "run_default" => self.run_configs(true),
            id if id.starts_with("user_command:") => self.user_command_run(&id[13..]),
            "user_command_stop" => self.user_command_stop(),
            "http_send" => self.http_send(),
            "http_stop" => self.http_stop(),
            "coverage_toggle" => self.coverage_toggle(),
            "md_heading_links" => self.review_heading_link_updates(),
            "coverage_reload" => self.coverage_reload(),
            "user_commands" => self.user_command_list(),
            "project_todos" => self.open_todos(),
            "task_output" => self.show_task_output(),
            "show_problems" => self.show_side(Some(Tab::Problems)),
            "task_errors" => self.open_errors(),
            "task_stop_job" => self.stop_job_menu(),
            "task_stop" => self.stop_task(),
            "task_rerun" => self.rerun_task(),
            "project_symbols" => self.open_palette("#"),
            "threads" => self.open_menu(MenuKind::Thread),
            "model" => self.open_menu(MenuKind::Model),
            "reasoning" => self.open_menu(MenuKind::Reasoning),
            "new_thread" => {
                self.toggle_side(Tab::Agent);
                self.new_thread();
                self.focus = Focus::Chat;
            }
            "context" => {
                self.cfg.context = !self.cfg.context;
                let _ = self.cfg.save();
            }
            "settings" => self.open_settings(),
            "keys" => self.open_menu(MenuKind::Keys),
            "goto_line" => self.open_palette(":"),
            "symbols" => self.open_palette("@"),
            "helix_buffers" => self.open_palette("%"),
            "quick_open" => self.open_palette(""),
            "palette" => self.open_palette(">"),
            "theme" => self.toggle_theme(),
            "theme_select" => self.open_themes(),
            "theme_import" => self.ask(PromptKind::ThemeImport),
            "theme_mode_system" => self.set_theme_mode("system"),
            "icon_theme_select" => self.open_icon_themes(),
            "font_buffer_select" => self.open_fonts(true),
            "font_ui_select" => self.open_fonts(false),
            "prose_font" => {
                self.cfg.serif = !self.cfg.serif;
                let _ = self.cfg.save();
                self.message = Some(format!(
                    "Reading in {}",
                    if self.cfg.serif { "Libron, a serif" } else { "Inter, a sans-serif" }
                ));
            }
            "checkpoint" => {
                self.unsuggest();
                self.message = Some(match self.recovery_save() {
                    Ok(()) => "Unsaved work checkpointed".into(),
                    Err(e) => format!("Recovery checkpoint failed: {e}"),
                });
            }
            "close_saved" => self.close_saved_tabs(),
            "close_others" => self.close_other_tabs(),
            "copy_relative" => {
                if let Some(path) = &self.doc.path {
                    self.copy = Some(crate::tools::short(&self.root, path));
                    self.message = Some("Path copied".into());
                }
            }
            "collapse" => self.tree.collapse(),
            "show_outline" => self.show_side(Some(Tab::Outline)),
            "show_preview" => self.toggle_preview(),
            _ => self.act(id),
        }
    }

    /// Lists the project's files in the background, unless that was done
    /// lately; `now` lists them whatever.
    pub(super) fn index_files(&mut self, now: bool) {
        let fresh = self.file_index.at.is_some_and(|at| at.elapsed() < Duration::from_secs(20));
        let roots = self.roots();
        if self.file_index.roots == roots && fresh && !now || self.file_index.pending.is_some() {
            return;
        }
        let done = Arc::new(Mutex::new(None));
        self.file_index.pending = Some(done.clone());
        if self.file_index.roots != roots {
            self.file_index.paths.clear();
            self.file_index.symbols.clear();
        }
        self.file_index.roots = roots.clone();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let paths = super::roots::files_in(&roots);
            let mut symbols = Vec::new();
            let mut remaining = 64 << 20;
            let notebook_pics = Rc::new(RefCell::new(Pics::new()));
            for path in &paths {
                let kind = doc_for(Some(path));
                if !matches!(kind, Doc::Markdown | Doc::Latex | Doc::Python | Doc::Code | Doc::Cells) {
                    continue;
                }
                if let Ok(meta) = std::fs::metadata(path)
                    && meta.len() <= 2 << 20
                    && meta.len() <= remaining
                    && let Ok(text) = std::fs::read_to_string(path)
                {
                    remaining -= meta.len();
                    let text = if kind == Doc::Cells {
                        match Notebook::open(&text, notebook_pics.clone()) {
                            Ok((_, source, _)) => source,
                            Err(_) => continue,
                        }
                    } else {
                        text.replace("\r\n", "\n")
                    };
                    let items = crate::treesitter::project_symbols(path, &text).unwrap_or_else(|| crate::outline::lexical_items(&text, kind));
                    symbols.extend(items.into_iter().map(|item| (path.clone(), item)));
                }
            }
            *done.lock().unwrap() = Some((paths, symbols));
            waker();
        });
    }

    /// Takes in the files listed, and shows them if the palette is open.
    pub(super) fn index_poll(&mut self) {
        let Some(pending) = &self.file_index.pending else { return };
        let Some((paths, symbols)) = pending.lock().unwrap().take() else { return };
        if self.file_index.roots != self.roots() {
            self.file_index.pending = None;
            self.index_files(true);
            return;
        }
        self.file_index.pending = None;
        self.file_index.at = Some(Instant::now());
        self.file_index.paths = paths;
        self.file_index.symbols = symbols;
        if self.doc.doc == Doc::Markdown { self.complete(false); }
        if let Some((MenuKind::Attach, menu)) = &self.menu {
            let typed = menu.typed().to_string();
            self.attachments_menu(&typed);
        }
        if matches!(self.menu, Some((MenuKind::Palette, _))) {
            self.palette_stale = true;
            self.palette_update();
        }
    }

    /// Waits for the files to be listed. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn index_settle(&mut self) {
        let start = Instant::now();
        while self.file_index.pending.is_some() && start.elapsed().as_secs() < 10 {
            std::thread::sleep(Duration::from_millis(5));
            self.index_poll();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };

    fn typed(a: &mut App, s: &str) {
        for ch in s.chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
    }

    #[test]
    fn the_palette_opens_files_runs_commands_and_goes_to_lines() {
        let dir = std::env::temp_dir().join(format!("ld-palette-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("notes.md"), "# Title\n\none\ntwo\n## Part\nthree\n").unwrap();
        std::fs::write(dir.join("src/main.py"), "print(1)\n").unwrap();
        let mut a = App::new(Some(dir.join("notes.md")));
        a.resize(1000, 700, 1.0);

        // Ctrl+P lists the files; typing narrows them; Enter opens one.
        a.key(Key::Char('p'), Mods { ctrl: true, ..NONE }, None);
        a.index_settle();
        assert!(matches!(a.menu, Some((MenuKind::Palette, _))));
        typed(&mut a, "mpy");
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.doc.path.as_deref(), Some(dir.join("src/main.py").as_path()));

        // `>` lists commands, `:` goes to a line, `@` to a heading.
        a.pick(dir.join("notes.md"), false);
        a.key(Key::Char('p'), Mods { ctrl: true, shift: true, alt: false }, None);
        typed(&mut a, "toggle edit");
        let Some((_, menu)) = &mut a.menu else { panic!() };
        assert_eq!(menu.chosen().map(|e| e.key.clone()).as_deref(), Some("c\nedit_mode"));
        a.key(Key::Enter, NONE, None);
        assert!(a.doc.edit_mode);
        a.key(Key::Char('g'), Mods { ctrl: true, ..NONE }, None);
        typed(&mut a, "4");
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 3);
        a.open_palette("@");
        typed(&mut a, "part");
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;
    #[test]
    fn symbols_include_disk_files_and_unsaved_tab_definitions() {
        let dir = std::env::temp_dir().join(format!("ld-symbols-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def original(): pass\n").unwrap();
        std::fs::write(dir.join("b.rs"), "pub fn other() {}\n").unwrap();
        let mut app = App::new(Some(dir.join("a.py")));
        app.doc.edit(0, app.doc.buf.text.len(), "def unsaved(): pass\n", EditKind::Other);
        app.open_palette("#");
        app.index_settle();
        let entries = app.project_symbol_entries();
        assert!(entries.iter().any(|e| e.label == "unsaved()"));
        assert!(!entries.iter().any(|e| e.label == "original()"));
        let key = entries.iter().find(|e| e.label == "other()").unwrap().key.clone();
        app.menu = None;
        app.palette_pick(&key);
        assert_eq!(app.doc.path.as_ref(), Some(&dir.join("b.rs")));
        app.navigate(false);
        assert!(app.doc.buf.text.contains("unsaved"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
