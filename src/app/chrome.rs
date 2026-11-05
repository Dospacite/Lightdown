//! The window's frame: the title bar with its layout toggles and command
//! centre, the status bar, the heads of the explorer and the right panel,
//! the terminal's toolbar and list, the hairlines between panels and the
//! tooltips of every control. Everything here that can be clicked is in
//! one table of hits, drawn and answered together.

use std::time::{Duration, Instant};

use super::assist::MenuKind;
use super::*;
use crate::icons::{self, Icon};
use crate::ui::{Rect, px};

/// How long the pointer rests on a control before its tooltip shows.
const TIP_DELAY: Duration = Duration::from_millis(550);

/// Something App draws that can be clicked.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Hit {
    // The title bar.
    ToggleFiles,
    Project,
    FileMenu,
    Palette,
    ToggleTerminal,
    TogglePanel,
    Settings,
    Minimize,
    Maximize,
    Close,
    // The status bar.
    Kind,
    FileIndent,
    FileEol,
    FileEncoding,
    Mode,
    Position,
    AgentBusy,
    Branch,
    GitOperation,
    GitHunk(u8),
    SidebarView(bool),
    SourceMessage,
    SourceRow(usize),
    SourceButton(usize, super::source_control::Button),
    SourceCommit,
    SourceAmend,
    SourceDraft,
    SourceRefresh,
    SourceMenu,
    Conflict(usize, u8, u64),
    MergeAccept(usize, u8, u64),
    Remote,
    LanguageServer,
    Peek(usize),
    Lightbulb,
    ProjectEnv,
    /// The notebook's kernel, in the status bar.
    Kernel,
    Recent(usize),
    Start(&'static str),
    ViewZoom,
    // The editor's actions, at the right of its tabs.
    EditMode,
    Preview,
    RunAll,
    RestartKernel,
    RunFile,
    More,
    // The explorer's head.
    NewFile,
    NewFolder,
    Collapse,
    Refresh,
    // The right panel's head.
    View(Tab),
    ViewOverflow,
    /// One of the Tests tab's actions, by its command.
    Tests(&'static str),
    Threads,
    NewThread,
    Build,
    OpenPdf,
    ClosePanel,
    // The terminal's toolbar and list.
    TermNew,
    TermSplit,
    TermRestart,
    TermKill,
    TermHide,
    TermTab(usize),
    TermClose(usize),
    // The find widget and dialogs.
    Find(super::find::FindHit),
    Dialog(super::dialog::DialogHit),
    /// The Search tab: what a control does, and the file or match it is for.
    Search(u8, usize),
}

impl App {
    // ---- the table of hits ------------------------------------------------

    /// Remembers a control drawn at `r`, and what its tooltip says.
    pub(super) fn push_hit(&mut self, r: Rect, hit: Hit, label: &str, keys: &str) {
        self.hits.push((r, hit));
        if !label.is_empty() {
            ui::tip(r, label, keys);
        }
    }

    /// The control under a point, if any.
    pub(super) fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| ui::inside(*r, Some((x, y)))).map(|(_, h)| *h)
    }

    /// Whether the pointer is over a control, so it lights up.
    pub(super) fn hot(&self, r: Rect) -> bool {
        !self.overlaid() && ui::inside(r, Some(self.pointer))
    }

    /// Carries out a click on a control.
    pub(super) fn do_hit(&mut self, hit: Hit) {
        self.tip_hover = None;
        match hit {
            Hit::Recent(i) => self.pick_recent(&i.to_string()),
            Hit::ViewZoom => self.view_zoom(),
            Hit::Start(id) => self.file_action(id),
            Hit::ToggleFiles => {
                let open = self.files_open && self.files_width() > 0.0;
                if open {
                    self.show_files(false);
                } else {
                    self.toggle_side(Tab::Files);
                }
            }
            Hit::Project => self.file_action("open"),
            Hit::FileMenu | Hit::More => {
                self.file_action("file_menu");
                // The list drops from the control that opened it.
                let at = self.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r);
                if let (Some((_, menu)), Some(r)) = (&mut self.menu, at) {
                    let x = if hit == Hit::More { r.0 + r.2 - menu.width * self.scale } else { r.0 };
                    menu.anchor = Some((x, r.1 + r.3));
                }
            }
            Hit::Palette => self.open_palette(""),
            Hit::ToggleTerminal => self.show_term(!self.term_open),
            Hit::TogglePanel => {
                if self.side_open && self.side_width() > 0.0 {
                    self.show_side(None);
                } else {
                    self.toggle_side(self.tab);
                }
            }
            Hit::Settings => self.open_settings(),
            Hit::Minimize | Hit::Maximize | Hit::Close => {}
            Hit::FileIndent | Hit::FileEol | Hit::FileEncoding => self.open_format_menu(hit),
            Hit::Kind => self.file_action("doc_kind"),
            Hit::Kernel => self.open_kernels(),
            Hit::Mode => {
                self.doc.edit_mode = !self.doc.edit_mode;
                self.doc.redraw = true;
            }
            Hit::Position => self.open_palette(":"),
            Hit::SidebarView(source) => self.source_view(source),
            Hit::SourceMessage => {
                (self.source.typing, self.focus) = (true, Focus::Files);
                self.source.cur = self.source.message.len();
            }
            Hit::SourceRow(i) => {
                (self.source.typing, self.focus) = (false, Focus::Files);
                self.source_open(i);
            }
            Hit::SourceButton(i, button) => self.source_button(i, button),
            Hit::SourceCommit => self.source_commit(),
            Hit::SourceAmend => self.source_amend(),
            Hit::SourceDraft => self.source_draft(),
            Hit::SourceRefresh => self.git_stale(),
            Hit::SourceMenu => self.git_command("git_changes"),
            Hit::GitHunk(choice) => self.git_hunk_button(choice),
            Hit::Branch => self.git_command("git_branches"),
            Hit::GitOperation => self.git_command("git_operation"),
            Hit::Remote => self.open_palette(self.execution_commands()),
            Hit::LanguageServer => self.language_server_menu(),
            Hit::Peek(i) => self.peek_open(Some(i)),
            Hit::Lightbulb => self.language_command("actions"),
            Hit::ProjectEnv => self.reload_env_loader(),
            Hit::MergeAccept(k, choice, version) => {
                if version == self.doc.buf.version {
                    self.merge_editor_accept(Some(k), choice);
                }
            }
            Hit::Conflict(i, choice, version) => {
                if version == self.doc.buf.version {
                    self.git_resolve(i, choice);
                }
            }
            Hit::AgentBusy => self.toggle_side(Tab::Agent),
            Hit::EditMode => {
                self.doc.edit_mode = !self.doc.edit_mode;
                self.focus = Focus::Doc;
            }
            Hit::Preview => self.toggle_preview(),
            Hit::RunAll => self.act("run_all"),
            Hit::RestartKernel => self.act("restart"),
            Hit::RunFile => self.run_file(),
            Hit::NewFile | Hit::NewFolder => {
                let root = self.tree.root().to_path_buf();
                self.ask(if hit == Hit::NewFile { PromptKind::New } else { PromptKind::NewFolder });
                if let Some(p) = &mut self.prompt {
                    p.text = format!("{}/", root.to_string_lossy().trim_end_matches('/'));
                    p.cur = p.text.len();
                }
            }
            Hit::Collapse => self.tree.collapse(),
            Hit::Refresh => {
                self.tree.refresh();
                self.git_stale();
            }
            Hit::View(tab) => {
                if tab == Tab::Preview {
                    if !(self.side_open && self.tab == Tab::Preview) {
                        self.toggle_preview();
                    }
                } else if tab == Tab::Backlinks {
                    self.open_backlinks();
                } else if tab == Tab::Search {
                    self.show_side(Some(tab));
                    self.search_panel_focus(0);
                } else {
                    self.show_side(Some(tab));
                    self.focus = tab.focus();
                }
            }
            Hit::ViewOverflow => self.panel_tabs_menu(true),
            Hit::Tests(id) => self.tests_command(id),
            Hit::Threads => self.open_menu(MenuKind::Thread),
            Hit::NewThread => {
                self.new_thread();
                self.focus = Focus::Chat;
            }
            Hit::Build => self.preview_hit(crate::preview::Hit::Build),
            Hit::OpenPdf => self.preview_hit(crate::preview::Hit::Open),
            Hit::ClosePanel => self.show_side(None),
            Hit::TermNew => self.add_term(),
            Hit::TermSplit => self.toggle_split(),
            Hit::TermRestart => self.restart_term(),
            Hit::TermKill => self.close_term(self.term_cur),
            Hit::TermHide => self.show_term(false),
            Hit::TermTab(i) => self.switch_term(i),
            Hit::TermClose(i) => self.close_term(i),
            Hit::Search(action, index) => self.search_panel_hit(action, index),
            Hit::Find(h) => self.find_hit(h),
            Hit::Dialog(h) => self.dialog_hit(h),
        }
        self.redraw = true;
    }

    // ---- the frame ----------------------------------------------------------

    /// Paints the canvas under every panel and the hairlines between them.
    pub(super) fn draw_frame(&mut self, c: &mut Canvas) {
        c.clear(theme::bg());
        if self.focus_mode { return; }
        let (top, bottom) = (self.title_h(), self.body_bottom());
        let (left, right) = self.sidebar_widths();
        let line = theme::line();
        // Under the title bar and over the status bar.
        c.line(0.0, top - 1.0, self.w, 1.0, line);
        c.line(0.0, bottom, self.w, 1.0, line);
        if left > 0.0 && left < self.w {
            c.line(left - 1.0, top, 1.0, bottom - top, line);
        }
        if right > 0.0 && right < self.w {
            c.line(self.w - right, top, 1.0, bottom - top, line);
        }
        // Under the heads of the panels, which line up.
        let head = top + self.head_h() - 1.0;
        c.line(0.0, head, self.w, 1.0, line);
        if self.term_open && self.term_height() > 0.0 {
            let (x, w) = self.document_area();
            c.line(x, self.term_top() - 1.0, w, 1.0, line);
        }
    }

    // ---- the title bar ----------------------------------------------------

    /// The title bar: the explorer's toggle and where the file is at the
    /// left, the command centre in the middle, and the other panels'
    /// toggles and the settings at the right. Where the compositor draws
    /// no frame, it is the window's title bar, with its buttons.
    pub(super) fn title_bar(&mut self, c: &mut Canvas) {
        let s = self.scale;
        let (w, h) = (self.w, self.title_h());
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let size = px(theme::UI_TEXT, s);
        let b = ui::base(0.0, h - 1.0, size);
        let btn = px(theme::CONTROL, s);
        let by = ((h - 1.0 - btn) / 2.0).round();
        let gap = px(4.0, s);
        let compact = w < 640.0 * s;

        // The left: the explorer's toggle, the project, the file.
        let mut x = px(8.0, s);
        let r = (x, by, btn, btn);
        let on = self.files_open && self.files_width() > 0.0;
        ui::icon_button(c, r, Icon::PanelLeft, self.hot(r), on, s);
        self.push_hit(r, Hit::ToggleFiles, "Explorer", &self.keys_of("files"));
        x += btn + px(6.0, s);

        // The right, from the window's edge inwards.
        let mut right = w - px(8.0, s);
        if self.csd {
            let buttons = [
                (Icon::X, Hit::Close, "Close"),
                (Icon::Maximize, Hit::Maximize, "Maximize"),
                (Icon::Minus, Hit::Minimize, "Minimize"),
            ];
            for (icon, hit, label) in buttons.into_iter().take(if self.tiled { 1 } else { 3 }) {
                right -= btn;
                let r = (right, by, btn, btn);
                let hot = self.hot(r);
                if hot && hit == Hit::Close {
                    c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS, s), theme::destructive());
                    icons::centred(c, icon, r, px(theme::ICON, s), 0xffffff);
                } else {
                    ui::icon_button(c, r, icon, hot, false, s);
                }
                self.push_hit(r, hit, label, "");
                right -= gap;
            }
            right -= px(8.0, s);
        }
        let toggles = [
            (Icon::Settings, Hit::Settings, "Settings", "settings", self.settings.is_some()),
            (
                Icon::PanelRight,
                Hit::TogglePanel,
                "Agent and side panel",
                "sidebar",
                self.side_open && self.side_width() > 0.0,
            ),
            (Icon::PanelBottom, Hit::ToggleTerminal, "Terminal", "terminal", self.term_open),
        ];
        for (icon, hit, label, id, on) in toggles {
            if compact && hit == Hit::ToggleTerminal {
                continue;
            }
            right -= btn;
            let r = (right, by, btn, btn);
            ui::icon_button(c, r, icon, self.hot(r), on, s);
            let keys = self.keys_of(id);
            self.push_hit(r, hit, label, &keys);
            right -= gap;
            if hit == Hit::Settings {
                right -= px(6.0, s);
            }
        }

        // The command centre, in the middle of the window where it fits.
        let palette_keys = self.keys_of("quick_open");
        let want = px(440.0, s);
        let left_end = x + px(160.0, s);
        let room = right - px(12.0, s) - left_end;
        let cw = want.min(room).round();
        let crumbs_end = if cw >= px(180.0, s) && !compact {
            let cx = ((w - cw) / 2.0).max(left_end).min(right - px(12.0, s) - cw).round();
            let ch = px(28.0, s);
            let r = (cx, ((h - 1.0 - ch) / 2.0).round(), cw, ch);
            let hot = self.hot(r);
            let fill = if hot { theme::hover() } else { theme::bg() };
            c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS, s), fill);
            c.round_ring(
                r.0,
                r.1,
                r.2,
                r.3,
                px(theme::RADIUS, s),
                1.0,
                if hot { theme::field() } else { theme::edge() },
            );
            let isz = px(theme::SMALL_ICON, s);
            icons::draw(c, Icon::Search, r.0 + px(10.0, s), (r.1 + (ch - isz) / 2.0).round(), isz, theme::icon());
            let kw = ui::kbd_width(fonts, &palette_keys, s);
            ui::kbd(c, fonts, r.0 + r.2 - px(8.0, s) - kw, r.1 + ch / 2.0, &palette_keys, s);
            let label = format!("Search {}", self.project_name());
            let room = r.2 - px(40.0, s) - kw - px(16.0, s);
            let label = ui::fit(fonts, font::UI, size, &label, room);
            ui::text(c, fonts, r.0 + px(32.0, s), ui::base(r.1, ch, size), &label, font::UI, size, theme::faint());
            self.push_hit(r, Hit::Palette, "Go to a file, or > for commands", &palette_keys);
            cx - px(12.0, s)
        } else {
            right -= btn;
            let r = (right, by, btn, btn);
            ui::icon_button(c, r, Icon::Search, self.hot(r), false, s);
            self.push_hit(r, Hit::Palette, "Go to a file, or > for commands", &palette_keys);
            right - px(8.0, s)
        };

        // The project and where the file is in it.
        let slip = px(26.0, s);
        let sy = ((h - 1.0 - slip) / 2.0).round();
        let spad = px(8.0, s);
        if !compact {
            let project = ui::fit(fonts, font::UI_MEDIUM, size, &self.project_name(), px(180.0, s));
            let pw = fonts.width(font::UI_MEDIUM, size, &project) + spad * 2.0;
            if x + pw < crumbs_end {
                let r = (x, sy, pw, slip);
                if self.hot(r) {
                    c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::hover());
                }
                ui::text(c, fonts, x + spad, b, &project, font::UI_MEDIUM, size, theme::fg());
                self.push_hit(r, Hit::Project, "Open a file or folder", &self.keys_of("open"));
                x += pw;
                let slash = "/";
                ui::text(c, fonts, x + px(2.0, s), b, slash, font::UI, size, theme::faint());
                x += fonts.width(font::UI, size, slash) + px(4.0, s);
            }
        }
        let (dir, name) = self.place();
        let unsaved = self.doc.buf.modified;
        let dot = if unsaved { px(14.0, s) } else { 0.0 };
        let room = (crumbs_end - x - spad * 2.0 - dot).max(0.0);
        let dir =
            if !compact && fonts.width(font::UI, size, &format!("{dir}{name}")) <= room { dir } else { String::new() };
        let name = ui::fit(fonts, font::UI_MEDIUM, size, &name, room);
        if !name.is_empty() {
            let (dw, nw) = (fonts.width(font::UI, size, &dir), fonts.width(font::UI_MEDIUM, size, &name));
            let r = (x, sy, dw + nw + spad * 2.0 + dot, slip);
            let open = self.menu.as_ref().is_some_and(|(kind, _)| *kind == MenuKind::File);
            if open {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::active());
            } else if self.hot(r) {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::hover());
            }
            ui::text(c, fonts, x + spad, b, &dir, font::UI, size, theme::dim());
            ui::text(c, fonts, x + spad + dw, b, &name, font::UI_MEDIUM, size, theme::fg());
            if unsaved {
                let d = px(7.0, s);
                c.circle(x + spad + dw + nw + px(9.0, s), ((h - 1.0) / 2.0).round(), d / 2.0, theme::warning());
            }
            let label = if unsaved { "File actions (unsaved changes)" } else { "File actions" };
            self.push_hit(r, Hit::FileMenu, label, &self.keys_of("file_menu"));
        }
        if self.csd && !self.maximized {
            c.frame(0.0, 0.0, self.w, self.h, 1.0, theme::edge());
        }
    }

    /// The project's name, as the title bar and palette call it.
    pub(super) fn project_name(&self) -> String {
        if let Some(label) = self.roots_label() {
            return label;
        }
        let root = if self.root.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            self.root.clone()
        };
        root.file_name().map_or_else(|| root.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
    }

    // ---- the status bar -----------------------------------------------------

    /// One item of the status bar, from the right edge leftwards: text with
    /// an optional icon, lit under the pointer if it can be clicked.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn status_item(
        &mut self,
        c: &mut Canvas,
        fonts: &mut Fonts,
        right: f32,
        label: &str,
        icon: Option<(Icon, u32)>,
        ink: u32,
        hit: Option<(Hit, &str, String)>,
    ) -> f32 {
        let s = self.scale;
        let (top, h) = (self.body_bottom() + 1.0, self.status_h() - 1.0);
        let size = px(theme::SMALL, s);
        let pad = px(6.0, s);
        let isz = px(13.0, s);
        let iw = if icon.is_some() { isz + px(5.0, s) } else { 0.0 };
        let w = fonts.width(font::UI, size, label) + iw + pad * 2.0;
        let r = ((right - w).round(), top + px(2.0, s), w.round(), h - px(4.0, s));
        if hit.is_some() && self.hot(r) {
            c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_SM, s), theme::hover());
        }
        let mut x = r.0 + pad;
        if let Some((icon, color)) = icon {
            icons::draw(c, icon, x, (top + (h - isz) / 2.0).round(), isz, color);
            x += iw;
        }
        ui::text(c, fonts, x, ui::base(top, h, size), label, font::UI, size, ink);
        if let Some((hit, tip, keys)) = hit {
            self.push_hit(r, hit, tip, &keys);
        }
        r.0
    }

    /// Vim mode's mode, or the command line while one is typed, for the
    /// status bar. Returns where it begins, if it is shown.
    fn vim_status_item(&mut self, c: &mut Canvas, fonts: &mut Fonts, right: f32) -> Option<f32> {
        let label = self.doc.vim_status().filter(|_| self.reader.is_none())?;
        let ink = if label.starts_with(':') { theme::fg() } else { theme::dim() };
        let x = self.status_item(c, fonts, right, &label, None, ink, None);
        if label.starts_with(':') && self.focus == Focus::Doc && !self.overlaid() && !self.prompt_has_keys() {
            let size = px(theme::SMALL,self.scale);
            let at = crate::bidi::caret_x(fonts,font::UI,size,&label,label.len());
            ui::caret(c,x+px(6.0,self.scale)+at,self.body_bottom()+self.status_h()/2.0,size*1.2,self.scale);
        }
        Some(x)
    }

    /// The status bar: the branch and what just happened at the left;
    /// facts about the document at the right, each a control where it
    /// leads somewhere.
    pub(super) fn status_bar(&mut self, c: &mut Canvas) {
        let s = self.scale;
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let size = px(theme::SMALL, s);
        let (top, h) = (self.body_bottom() + 1.0, self.status_h() - 1.0);
        let b = ui::base(top, h, size);
        let gap = px(2.0, s);

        // The right: the kind, the mode, where the cursor is.
        let mut right = self.w - px(6.0, s);
        let (said, kind) = self.status();
        self.kind_hit = None;
        if self.can_change_kind() {
            let keys = self.keys_of("doc_kind");
            let x =
                self.status_item(c, fonts, right, kind, None, theme::fg(), Some((Hit::Kind, "Show the file as", keys)));
            self.kind_hit = self.hits.last().map(|(r, _)| *r);
            right = x - gap;
        } else {
            right = self.status_item(c, fonts, right, kind, None, theme::dim(), None) - gap;
        }
        if self.doc.doc == Doc::Markdown && self.reader.is_none() {
            let (label, tip) = if self.doc.edit_mode {
                ("Source", "Showing all markdown syntax")
            } else {
                ("Live", "Rendering markdown in place")
            };
            let keys = self.keys_of("edit_mode");
            let icon = Some((if self.doc.edit_mode { Icon::Code } else { Icon::Eye }, theme::icon()));
            right = self.status_item(c, fonts, right, label, icon, theme::dim(), Some((Hit::Mode, tip, keys))) - gap;
        }
        if let Some(x) = self.vim_status_item(c, fonts, right) {
            right = x - gap;
        }
        for part in said.split('\t').rev() {
            let clickable = part.starts_with("Ln ");
            let kernel = self.nb.is_some() && !part.starts_with("Cell ") && !clickable;
            let hit = if kernel {
                Some((Hit::Kernel, "Choose the kernel", String::new()))
            } else {
                clickable.then(|| (Hit::Position, "Go to line", self.keys_of("goto_line")))
            };
            let ink = if part.ends_with("busy") { theme::warning() } else { theme::dim() };
            right = self.status_item(c, fonts, right, part, None, ink, hit) - gap;
        }

        right = self.file_format_status(c, fonts, right);
        right = self.language_status_item(c, fonts, right, gap);

        // The left: the branch, the agent at work, and what just happened.
        let mut x = px(6.0, s);
        let place = |fonts: &mut Fonts, label: &str| fonts.width(font::UI, size, label) + px(30.0, s);
        if let Some(remote) = crate::remote::active(&self.root) {
            let label = remote.label();
            let w = place(fonts, &label);
            if x + w < right {
                let end = self.status_item(c, fonts, x + w, &label, Some((Icon::Terminal, theme::icon())), theme::dim(), Some((Hit::Remote, "Remote project commands", String::new())));
                x = end + w + gap;
            }
        }
        x = self.project_env_item(c, fonts, x, right);
        if let Some(branch) = self.git.status.as_ref().and_then(|st| st.branch.clone()) {
            let tracking = &self.git.status.as_ref().unwrap().ahead_behind;
            let branch = if tracking.is_empty() { branch } else { format!("{branch} · {tracking}") };
            let w = place(fonts, &branch);
            if x + w < right {
                let end = self.status_item(
                    c,
                    fonts,
                    x + w,
                    &branch,
                    Some((Icon::GitBranch, theme::icon())),
                    theme::dim(),
                    Some((Hit::Branch, "Switch, create or delete branches", String::new())),
                );
                x = end + w + gap;
            }
        }
        x = self.draw_operation_item(c, fonts, x, right, gap);
        if self.ai.busy() && !(self.side_open && self.tab == Tab::Agent && self.side_width() > 0.0) {
            let label = if self.ai.waiting() { "Agent needs you" } else { "Agent working" };
            let ink = if self.ai.waiting() { theme::warning() } else { theme::info() };
            let w = place(fonts, label);
            if x + w < right {
                let hit = Some((Hit::AgentBusy, "Show the agent", self.keys_of("sidebar")));
                let end = self.status_item(c, fonts, x + w, label, Some((Icon::Dot, ink)), ink, hit);
                x = end + w + gap;
            }
        }
        if let Some(label) = self.copilot_status() {
            let label = ui::fit(fonts, font::UI, size, &label, ((right-x)*0.45).max(0.0));
            if !label.is_empty() { let w = place(fonts, &label); x = self.status_item(c, fonts, x+w, &label, None, theme::dim(), None) + w + gap; }
        }
        let status = self.key_bindings.map.hint.clone().or_else(|| self.task_status()).or_else(|| self.message.clone()).or_else(|| self.github_status());
        let (said, ink, icon) = match &status {
            Some(m) => (
                m.clone(),
                self.watch_badge().map_or(theme::fg(), |ok| if ok { theme::success() } else { theme::danger() }),
                Some(Icon::Info),
            ),
            None if self.doc.buf.modified => ("Unsaved changes".to_string(), theme::warning(), None),
            None => (String::new(), theme::dim(), None),
        };
        if !said.is_empty() {
            let isz = px(13.0, s);
            let mut tx = x + px(6.0, s);
            if let Some(icon) = icon {
                icons::draw(c, icon, tx, (top + (h - isz) / 2.0).round(), isz, theme::icon());
                tx += isz + px(5.0, s);
            }
            let said = ui::fit(fonts, font::UI, size, &said, (right - tx - px(8.0, s)).max(0.0));
            ui::text(c, fonts, tx, b, &said, font::UI, size, ink);
        }
    }

    /// The file's language server and its work, at the left of the
    /// document facts. A click offers restart, stop and the log.
    fn language_status_item(&mut self, c: &mut Canvas, fonts: &mut Fonts, right: f32, gap: f32) -> f32 {
        let Some((label, ink)) = self.language_status() else { return right };
        let label = ui::fit(fonts, font::UI, px(theme::SMALL, self.scale), &label, px(260.0, self.scale));
        let hit = Some((Hit::LanguageServer, "Language server: restart, stop or show its log", String::new()));
        self.status_item(c, fonts, right, &label, Some((Icon::Dot, ink)), theme::dim(), hit) - gap
    }

    // ---- the heads of panels ------------------------------------------------

    /// The explorer's head: the folder's name and what can be made in it.
    pub(super) fn explorer_head(&mut self, c: &mut Canvas) {
        let (left, _) = self.sidebar_widths();
        if left <= 0.0 {
            return;
        }
        let s = self.scale;
        let w = left - if left < self.w { 1.0 } else { 0.0 };
        let (y, h) = (self.title_h(), self.head_h() - 1.0);
        c.clip(0.0, y, w, h);
        // The two views as tabs, in place of the folder's name.
        let after = self.sidebar_views(c, y, h, w) + px(4.0, s);
        let btn = px(theme::SMALL_CONTROL, s);
        let by = (y + (h - btn) / 2.0).round();
        let mut right = w - px(6.0, s);
        if self.cfg.source_control {
            self.source_head_actions(c, by, right, after);
            c.unclip();
            return;
        }
        let actions = [
            (Icon::Refresh, Hit::Refresh, "Refresh", ""),
            (Icon::ChevronsDownUp, Hit::Collapse, "Collapse folders", ""),
            (Icon::FolderPlus, Hit::NewFolder, "New folder", ""),
            (Icon::FilePlus, Hit::NewFile, "New file", "new_file"),
        ];
        for (icon, hit, label, id) in actions {
            right -= btn;
            if right < after {
                break;
            }
            let r = (right, by, btn, btn);
            ui::icon_button(c, r, icon, self.hot(r), false, s);
            let keys = if id.is_empty() { String::new() } else { self.keys_of(id) };
            self.push_hit(r, hit, label, &keys);
            right -= px(2.0, s);
        }
        c.unclip();
    }

    /// The right panel's head: its views as tabs, and what the one shown
    /// can do.
    pub(super) fn panel_head(&mut self, c: &mut Canvas) {
        let (_, right_w) = self.sidebar_widths();
        if right_w <= 0.0 {
            return;
        }
        let s = self.scale;
        let x0 = self.w - right_w + if right_w < self.w { 1.0 } else { 0.0 };
        let w = self.w - x0;
        let (y, h) = (self.title_h(), self.head_h() - 1.0);
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        c.clip(x0, y, w, h);
        let size = px(theme::SMALL, s);
        let btn = px(theme::SMALL_CONTROL, s);
        let by = (y + (h - btn) / 2.0).round();

        // The actions at the right.
        let mut right = x0 + w - px(6.0, s);
        let mut actions = vec![(Icon::X, Hit::ClosePanel, "Close the panel", "sidebar_hide")];
        match self.tab {
            Tab::Agent => {
                actions.push((Icon::SquarePen, Hit::NewThread, "New thread", "new_thread"));
                actions.push((Icon::History, Hit::Threads, "Threads", "threads"));
            }
            Tab::Tests => {
                if self.tests.busy() {
                    actions.push((Icon::Stop, Hit::Tests("tests_stop"), "Tests: Stop tests", "tests_stop"));
                } else {
                    actions.push((Icon::Refresh, Hit::Tests("tests_refresh"), "Tests: Refresh tests", "tests_refresh"));
                    actions.push((Icon::Gauge, Hit::Tests("tests_coverage"), "Tests: Run all with coverage", "tests_coverage"));
                    actions.push((Icon::RotateCcw, Hit::Tests("tests_failed"), "Tests: Rerun failed tests", "tests_failed"));
                    actions.push((Icon::PlayAll, Hit::Tests("tests_all"), "Tests: Run all tests", "tests_all"));
                }
            }
            Tab::Preview => {
                if self.typeset.view.pdf.is_some() {
                    actions.push((Icon::External, Hit::OpenPdf, "Open in the PDF viewer", ""));
                }
                if self.unbuildable().is_none() && !self.typeset.busy() {
                    actions.push((Icon::Play, Hit::Build, "Build", "preview"));
                }
            }
            _ => {}
        }
        for (icon, hit, label, id) in actions {
            right -= btn;
            let r = (right, by, btn, btn);
            ui::icon_button(c, r, icon, self.hot(r), false, s);
            let keys = if id.is_empty() { String::new() } else { self.keys_of(id) };
            self.push_hit(r, hit, label, &keys);
            right -= px(2.0, s);
        }

        // The views, as tabs. Those that do not fit are under ⋯.
        let views = self.panel_views();
        let mut tab_w = |label: &str| (fonts.width(font::UI_MEDIUM, size, label) + px(12.0, s) + px(5.0, s) + px(16.0, s)).round() + px(2.0, s);
        let widths: Vec<f32> = views.iter().map(|v| tab_w(v.2)).collect();
        let active = views.iter().position(|v| v.0 == self.tab);
        let (shown, more) = super::panel_tabs::fit(&widths, active, right - x0 - px(12.0, s), btn + px(2.0, s));
        self.panel_hidden = (0..views.len()).filter(|i| !shown.contains(i)).map(|i| views[i].0).collect();
        self.panel_more = None;
        if more {
            right -= btn;
            let r = (right, by, btn, btn);
            ui::icon_button(c, r, Icon::Ellipsis, self.hot(r), false, s);
            self.push_hit(r, Hit::ViewOverflow, "View: More panel tabs", &self.keys_of("panel_tabs"));
            self.panel_more = Some(r);
            right -= px(2.0, s);
        }
        let views: Vec<_> = shown.into_iter().map(|i| views[i]).collect();
        let mut x = x0 + px(8.0, s);
        let th = px(24.0, s);
        let ty = (y + (h - th) / 2.0).round();
        let isz = px(12.0, s);
        for (tab, icon, label, id) in views {
            let tw = fonts.width(font::UI_MEDIUM, size, label);
            let width = (tw + isz + px(5.0, s) + px(16.0, s)).round();
            let width = if self.tab == tab { width.min((right - px(4.0, s) - x).max(0.0)) } else { width };
            let r = (x, ty, width, th);
            if r.0 + r.2 > right - px(4.0, s) {
                break;
            }
            let on = self.tab == tab;
            let hot = self.hot(r);
            if on {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::active());
            } else if hot {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), theme::hover());
            }
            let ink = if on || hot { theme::fg() } else { theme::dim() };
            icons::draw(
                c,
                icon,
                r.0 + px(8.0, s),
                (ty + (th - isz) / 2.0).round(),
                isz,
                if on { theme::fg() } else { theme::icon() },
            );
            let fitted = ui::fit(fonts, font::UI_MEDIUM, size, label, (r.2 - isz - px(20.0, s)).max(0.0));
            ui::text(
                c,
                fonts,
                r.0 + px(8.0, s) + isz + px(5.0, s),
                ui::base(ty, th, size),
                &fitted,
                font::UI_MEDIUM,
                size,
                ink,
            );
            let keys = self.keys_of(id);
            self.push_hit(r, Hit::View(tab), label, &keys);
            x += r.2 + px(2.0, s);
        }
        c.unclip();
    }

    // ---- the terminal's controls --------------------------------------------

    /// The width of the list of terminals at the right of the panel, which
    /// shows while more than one is open.
    pub(super) fn term_list_w(&self) -> f32 {
        let (_, w) = self.document_area();
        // Two side by side are both in view already.
        let listed = self.terms.len() > 2 || (self.terms.len() == 2 && self.term_panes.is_none());
        if listed && w > 420.0 * self.scale { px(150.0, self.scale) } else { 0.0 }
    }

    /// The terminal's toolbar, floating at the top right of the panel, and
    /// the list of terminals beside it.
    pub(super) fn term_chrome(&mut self, c: &mut Canvas) {
        if !self.term_open || self.terms.is_empty() || self.term_height() <= 0.0 {
            return;
        }
        let s = self.scale;
        let (x, w) = self.document_area();
        let top = self.term_top();
        let bottom = self.body_bottom();
        let list = self.term_list_w();
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();

        if list > 0.0 {
            let lx = x + w - list;
            c.rect(lx, top, list, bottom - top, theme::bg());
            c.line(lx, top, 1.0, bottom - top, theme::line());
            let names = self.term_names();
            let rh = px(26.0, s);
            let size = px(theme::SMALL, s);
            let isz = px(13.0, s);
            c.clip(lx + 1.0, top, list - 1.0, bottom - top);
            for (i, name) in names.iter().enumerate() {
                let r = (lx + px(5.0, s), top + px(5.0, s) + rh * i as f32, list - px(10.0, s), rh);
                if r.1 + rh > bottom {
                    break;
                }
                let on = i == self.term_cur || self.term_panes.is_some_and(|p| p.contains(&i));
                let hot = self.hot(r);
                ui::row_ground(c, r, i == self.term_cur, hot && i != self.term_cur, false, s);
                let ink = if on { theme::fg() } else { theme::dim() };
                icons::draw(c, Icon::Terminal, r.0 + px(7.0, s), (r.1 + (rh - isz) / 2.0).round(), isz, theme::icon());
                let close = (r.0 + r.2 - rh, r.1, rh, rh);
                let room = r.2 - px(30.0, s) - if hot { rh } else { 0.0 };
                let label = ui::fit(fonts, font::UI, size, name, room);
                ui::text(c, fonts, r.0 + px(26.0, s), ui::base(r.1, rh, size), &label, font::UI, size, ink);
                if hot {
                    let ch = self.hot(close);
                    ui::icon_button(
                        c,
                        (close.0 + px(3.0, s), close.1 + px(3.0, s), rh - px(6.0, s), rh - px(6.0, s)),
                        Icon::X,
                        ch,
                        false,
                        s,
                    );
                    self.push_hit(close, Hit::TermClose(i), "Close the terminal", &self.keys_of("terminal_close"));
                }
                self.push_hit(r, Hit::TermTab(i), "", "");
            }
            c.unclip();
        }

        // The toolbar.
        let btn = px(24.0, s);
        let gap = px(2.0, s);
        let split = if self.term_panes.is_some() { "Show one terminal" } else { "Split the terminal" };
        let tools = [
            (Icon::Plus, Hit::TermNew, "New terminal", "terminal_new"),
            (Icon::Columns, Hit::TermSplit, split, "terminal_split"),
            (Icon::RotateCcw, Hit::TermRestart, "Restart the shell", ""),
            (Icon::Trash, Hit::TermKill, "Close the terminal", "terminal_close"),
            (Icon::X, Hit::TermHide, "Hide the panel", "terminal_hide"),
        ];
        let bw = btn * tools.len() as f32 + gap * (tools.len() + 1) as f32 + px(9.0, s);
        let bx = (x + w - list - px(8.0, s) - bw).round();
        let byy = top + px(6.0, s);
        if bx < x + px(40.0, s) {
            return;
        }
        let r = (bx, byy, bw, btn + gap * 2.0);
        c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS, s), theme::bg());
        c.round_ring(r.0, r.1, r.2, r.3, px(theme::RADIUS, s), 1.0, theme::edge());
        let mut tx = bx + gap;
        for (i, (icon, hit, label, id)) in tools.into_iter().enumerate() {
            if i == tools.len() - 1 {
                c.line(tx + px(4.0, s), byy + px(7.0, s), 1.0, btn - px(10.0, s), theme::line());
                tx += px(9.0, s);
            }
            let br = (tx, byy + gap, btn, btn);
            let on = hit == Hit::TermSplit && self.term_panes.is_some();
            ui::icon_button(c, br, icon, self.hot(br), on, s);
            let keys = if id.is_empty() { String::new() } else { self.keys_of(id) };
            self.push_hit(br, hit, label, &keys);
            tx += btn + gap;
        }
    }

    // ---- tooltips -------------------------------------------------------

    /// The pointer moved: a tooltip waits anew for it to rest on a control.
    pub(super) fn tip_motion(&mut self, x: f32, y: f32) {
        let under = if self.overlaid() { None } else { ui::tip_at(x, y).map(|(r, ..)| r) };
        match (under, self.tip_hover) {
            (Some(r), Some((held, _, _))) if r == held => {}
            (Some(r), _) => {
                self.tip_hover = Some((r, Instant::now(), false));
                self.redraw = true;
            }
            (None, Some((_, _, shown))) => {
                self.tip_hover = None;
                self.redraw |= shown;
            }
            (None, None) => {}
        }
    }

    /// When a tooltip waiting to show is due.
    pub(super) fn tip_due(&self) -> Option<Instant> {
        self.tip_hover.filter(|t| !t.2).map(|(_, since, _)| since + TIP_DELAY)
    }

    /// Draws the tooltip of the control the pointer rests on, once it has
    /// rested long enough.
    pub(super) fn draw_tooltip(&mut self, c: &mut Canvas) {
        let Some((r, since, _)) = self.tip_hover else { return };
        if self.overlaid() || since.elapsed() < TIP_DELAY {
            return;
        }
        let Some((_, label, keys)) = ui::tip_at(r.0 + r.2 / 2.0, r.1 + r.3 / 2.0) else { return };
        let fonts = self.fonts.clone();
        ui::tooltip(c, &mut fonts.borrow_mut(), r, &label, &keys, self.w, self.h, self.scale);
        if let Some(t) = &mut self.tip_hover {
            t.2 = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(a: &mut App) {
        let mut px = vec![0; (a.w * a.h) as usize];
        a.draw(&mut Canvas::new(&mut px, a.w as i32, a.h as i32));
    }

    #[test]
    fn controls_name_themselves_once_the_pointer_rests() {
        let mut a = App::new(None);
        a.resize(1000, 700, 1.0);
        draw(&mut a);
        let r = a.hits.iter().find(|(_, h)| *h == Hit::ToggleTerminal).unwrap().0;
        let (x, y) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
        a.motion(x, y);
        // It waits for the pointer to rest, and asks to be woken for it.
        assert!(a.tip_due().is_some_and(|due| due > Instant::now()));
        draw(&mut a);
        assert!(!a.tip_hover.unwrap().2);
        let t = a.tip_hover.as_mut().unwrap();
        t.1 = t.1.checked_sub(TIP_DELAY).unwrap();
        draw(&mut a);
        assert!(a.tip_hover.unwrap().2 && a.tip_due().is_none());
        assert_eq!(ui::tip_at(x, y).map(|t| t.1).as_deref(), Some("Terminal"));
        // A click puts it away, and does what the control does.
        a.press(x, y, Mods::default(), 1);
        assert!(a.tip_hover.is_none() && a.term_open);
    }

    #[test]
    fn the_status_bar_leads_to_the_line_and_the_mode() {
        let mut a = App::new(None);
        a.resize(1000, 700, 1.0);
        a.paste_text("# Title\n\ntext");
        draw(&mut a);
        let at = |a: &App, hit: Hit| a.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| (r.0 + 2.0, r.1 + 2.0));
        let (x, y) = at(&a, Hit::Mode).unwrap();
        a.press(x, y, Mods::default(), 1);
        assert!(a.doc.edit_mode);
        let (x, y) = at(&a, Hit::Kind).unwrap();
        a.press(x, y, Mods::default(), 1);
        assert!(matches!(a.menu, Some((MenuKind::Kind, _))));
    }
}
