//! The window's contents, independent of the window system: the document
//! pane, the sidebar with the agent or the folder in it, what surrounds
//! them, and the routing of input between them.

mod agent_changes;
mod worktrees;
mod worktree_overview;
mod planning;
mod agent_context;
mod assist;
mod outside;
mod chrome;
mod citations;
mod compaction;
mod context;
mod counts;
mod convenience;
mod debugging;
mod debug_exceptions;
mod debug_hover;
mod debug_tree;
mod inline_values;
mod diagrams;
mod dialog;
mod disk;
mod document_export;
mod docs;
mod environments;
mod file_format;
mod project_env;
mod files;
mod find;
mod folder;
pub(crate) mod git;
mod git_actions;
mod integration;
mod merge_editor;
mod multibuffer;
mod rebase;
mod git_hunks;
mod file_blame;
mod source_control;
mod git_discard;
mod github;
mod ci_logs;
mod inline;
pub(crate) mod ime;
mod languages;
mod local_history;
mod problems;
mod selection;
mod user_commands;
mod grammar;
mod grammar_menu;
mod todos;
mod http_requests;
mod coverage_view;
mod link_updates;
pub(crate) mod links;
mod navigation;
mod notebook;
mod native_debug;
#[cfg(test)]
mod node_debug_tests;
mod debug_config;
mod debug_process;
mod debug_launch;
mod agent_checks;
mod notes;
mod palette;
mod modal_editing;
mod rebind;
mod keybindings;
mod recovery;
mod remote_projects;
mod port_list;
mod containers;
mod replace;
mod roots;
pub(crate) mod repository;
pub(crate) mod saving;
mod bookmarks;
mod run_configs;
mod script_cells;
mod search;
mod search_panel;
mod autosave;
mod session;
mod settings;
mod snippets;
mod config_editing;
mod spelling;
mod split;
mod suggest;
mod predictions;
mod copilot_predictions;
mod tabs;
mod tasks;
mod job_terminal;
mod task_config;
mod terminal;
mod shell_marks;
mod testing;
mod test_runners;
mod tests_panel;
mod panel_tabs;
mod typeset;
mod writing;
mod looks;
mod kernels;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crate::cells::{self, Tag};
use crate::chooser::Chooser;
use crate::config::{self, Config};
use crate::editor::{self, EditKind};
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::kernel::{Event, Kernel, Waker};
use crate::keys::{ACTIONS, Chord, Keymap, Meaning};
use crate::nb::Notebook;
use crate::outline::Outline;
use crate::pane::{self, Doc, Pane};
use crate::pdf::{self, Pages};
use crate::pics::Pics;
use crate::preview::Preview;
use crate::py::{self, Candidate};
use crate::reader::{Reader, Seek};
use crate::side::Side;
use crate::term::Term;
use crate::tree::Tree;
use crate::ui::{Entry, Menu, Picked, Popup};
use crate::ui::{Rect, px};
use crate::{agent, latex, reader};
use crate::{theme, ui};
use assist::{Assist, MenuKind};
use chrome::Hit;
use context::Target;
use settings::Settings;
use suggest::Suggesting;
use tabs::FileTab;
use typeset::Typeset;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Enter,
    Tab,
    Escape,
    /// A printable key, lowercased, for shortcuts.
    Char(char),
    /// A function key, by its number.
    F(u8),
    Other,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// What lies under the pointer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Zone {
    Text,
    Link,
    /// Something that can be clicked.
    Button,
    /// The edge between the document and the sidebar, which can be dragged.
    Divider,
    /// The top edge of the terminal, which can be dragged.
    RowDivider,
    Plain,
    Title,
    Close,
    Minimize,
    Maximize,
    Resize(Edge),
}

/// Which pane the keyboard goes to.
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Doc,
    Chat,
    Files,
    Term,
}

/// What the sidebar holds.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Agent,
    Files,
    /// The document built into a PDF. It takes no keys.
    Preview,
    /// The document's headings. It takes no keys.
    Outline,
    Debug,
    Problems,
    Hierarchy,
    Tests,
    Backlinks,
    Search,
}

impl Tab {
    /// Where the keyboard is while this tab has it.
    fn focus(self) -> Focus {
        match self {
            Tab::Agent => Focus::Chat,
            Tab::Files => Focus::Files,
            Tab::Search | Tab::Preview | Tab::Outline | Tab::Debug | Tab::Problems | Tab::Tests | Tab::Backlinks | Tab::Hierarchy => Focus::Doc,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    SaveAs,
    Reload,
    Find,
    /// Find, with a second field for what each match becomes.
    Replace,
    New,
    /// A new folder: the name is a folder's, slash or not.
    NewFolder,
    Rename,
    Copy,
    /// Asks whether the file is to go; nothing is typed.
    Trash,
    Export,
    ExportTex,
    ExportDocument,
    GitBranch,
    GitCommit,
    GithubReviewLine,
    GitLogSearch,
    GitStashMessage,
    GitCompareRevisions,
    GitAmend,
    GitRebaseReword,
    GithubReply,
    GithubReviewBody,
    DebugInput,
    LspRename,
    SearchInclude,
    SearchExclude,
    ProjectReplace,
    InlineEdit,
    TerminalFind,
    AgentWorktree,
    /// The path of a Zed or VS Code theme to convert.
    ThemeImport,
}

struct Prompt {
    kind: PromptKind,
    text: String,
    cur: usize,
    /// Where a search started, so each keystroke searches from the same spot.
    origin: usize,
    /// The file or folder of the folder view the prompt is about, where it
    /// is not the open document.
    subject: Option<PathBuf>,
    /// What matches become, for `Replace`, with its cursor, and whether
    /// that field has the keys.
    with: String,
    with_cur: usize,
    on_with: bool,
    /// A find has given the keys back to the document, and stays on show.
    away: bool,
}

impl Prompt {
    /// The field being typed in, and its cursor.
    fn field(&mut self) -> (&mut String, &mut usize) {
        if self.on_with { (&mut self.with, &mut self.with_cur) } else { (&mut self.text, &mut self.cur) }
    }

    /// Whether the prompt finds as it is typed in.
    fn finds(&self) -> bool {
        matches!(self.kind, PromptKind::Find | PromptKind::Replace)
    }
}

/// The names on offer for the word being typed.
struct Completion {
    query: py::Query,
    items: Vec<Candidate>,
    sel: usize,
    /// The first item shown, as last drawn.
    top: usize,
    /// The request to the interpreter whose answer belongs here.
    seq: u64,
}

pub struct App {
    pub doc: Pane,
    file_tabs: Vec<FileTab>,
    active_file: usize,
    closed_tabs: Vec<FileTab>,
    views: split::Views,
    /// Tabs also shown in a second view, after the others, and whether the
    /// tab's live view is drawn there rather than in its first.
    split_mirrors: Vec<(u64, bool)>,
    focus_mode: bool,
    preedit: Option<ime::Preedit>,
    ime_generation: u64,
    typewriter_mode: bool,
    tab_drag: Option<(u64, f32, f32)>,
    recovery: recovery::Recovery,
    recent: Vec<PathBuf>,
    disk_checked: std::time::Instant,
    history: navigation::History,
    task_state: tasks::Tasks,
    pending_pr_root: Option<PathBuf>,
    pending_pr_head: Option<String>,
    pending_pr_comments: Vec<serde_json::Value>,
    worktrees: worktrees::Worktrees,
    remote_projects: remote_projects::RemoteProjects,
    languages: languages::Languages,
    problems: problems::Problems,
    selection: selection::Selection,
    user_commands: user_commands::Commands,
    grammar: grammar::Grammar,
    todos: todos::Todos,
    http: http_requests::HttpRequests,
    coverage: coverage_view::CoverageView,
    /// Link changes waiting for Markdown: Update links to renamed headings.
    heading_link_updates: Vec<(PathBuf, String, String)>,
    scripts: script_cells::Scripts,
    tests: testing::Tests,
    mcp: crate::mcp::Servers,
    debugger: debugging::Debugging,
    debug_prep: Option<debug_launch::Prep>,
    saving: saving::Saving,
    file_tab_hits: Vec<(Rect, usize, bool)>,
    tab_close_armed: Option<usize>,
    nb: Option<Notebook>,
    /// The pages of a PDF, shown in the document's place.
    reader: Option<Reader>,
    /// The interpreter that completes names in Python files.
    helper: Option<Kernel>,
    comp: Option<Completion>,
    comp_seq: u64,
    /// Where the list of completions was last drawn, and each row's height.
    comp_box: Option<(f32, f32, f32, f32)>,
    comp_row: f32,
    /// A suggestion that has been asked for and not yet shown.
    suggest: Option<Suggesting>,
    predictions: predictions::State,
    copilot: copilot_predictions::State,
    suggest_due: Option<(std::time::Instant, u64, u64, usize)>,
    inline_preview: Option<(usize, String, usize, usize)>,
    /// Signatures and docstrings on show, or asked for.
    docs: docs::Docs,
    /// The search of the project's files under way.
    search: search::Search,
    /// What git says of the project.
    git: git::Git,
    /// The left sidebar's Source Control view.
    source: source_control::View,
    git_actions: git_actions::GitActions,
    /// The spell checker, once there is prose to check, and the misspelt
    /// word a menu is open on, with what it may be meant to be.
    speller: Option<crate::spell::Speller>,
    /// Typesets the mathematics of markdown, for every tab.
    maths: Rc<RefCell<crate::math::Maths>>,
    spell_fix: Option<(usize, usize, Vec<String>)>,
    ai: Assist,
    tree: Tree,
    outline: Outline,
    typeset: Typeset,
    /// The open terminals, the one shown, and whether the panel shows.
    terms: Vec<Term>,
    term_cur: usize,
    /// The two terminals shown side by side, left and right, when the
    /// panel is split. The one shown with the keys is one of them.
    term_panes: Option<[usize; 2]>,
    term_open: bool,
    /// A terminal with a program at work, asked once to close.
    term_close_armed: Option<usize>,
    /// The terminal's top edge is being dragged.
    term_resizing: bool,
    /// Right sidebar, containing the agent or PDF preview.
    side_open: bool,
    tab: Tab,
    files_open: bool,
    /// Which sidebar is visible when a narrow window can show only one.
    compact_files: bool,
    /// The sidebar's edge is being dragged.
    resizing: Option<Tab>,
    /// Where each control the window draws itself was last drawn.
    hits: Vec<(Rect, Hit)>,
    /// The right panel's tabs that did not fit in its head, and its ⋯.
    panel_hidden: Vec<Tab>,
    panel_more: Option<Rect>,
    /// Where the status bar's name for the kind of document was last drawn.
    kind_hit: Option<Rect>,
    /// The control the pointer rests on, since when, and whether its
    /// tooltip has shown.
    tip_hover: Option<(Rect, std::time::Instant, bool)>,
    /// What the palette lists, and whether that is to be listed anew.
    palette_mode: Option<palette::Mode>,
    palette_stale: bool,
    /// The project's files, for the palette.
    file_index: palette::FileIndex,
    /// The text and needle the find's matches were marked for, how many
    /// there are and which one is chosen.
    found_for: Option<(u64, String, replace::Options)>,
    search_options: replace::Options,
    autosave_due: Option<std::time::Instant>,
    config_checks: config_editing::ConfigChecks,
    found_count: (usize, Option<usize>),
    /// Where the dialog was last drawn.
    dialog_panel: Option<Rect>,
    focus: Focus,
    menu: Option<(MenuKind, Menu)>,
    /// A context menu, and what it is about.
    popup: Option<(Target, Popup)>,
    settings: Option<Settings>,
    keymap: Keymap,
    key_bindings: keybindings::KeyBindings,
    /// The shortcut, by its place in the list, that waits for new keys.
    rebinding: Option<usize>,
    pub cfg: Config,
    /// The directory of the project the document belongs to.
    root: PathBuf,
    /// The folders opened in this window, its roots; the one holding the
    /// file opened is its project. See `roots`.
    folders: Vec<PathBuf>,
    window: roots::Window,
    /// A file or folder copied, or cut when true, in the folder view, to be
    /// pasted into another folder there.
    file_clip: Option<(PathBuf, bool)>,
    fonts: Rc<RefCell<Fonts>>,
    pics: Rc<RefCell<Pics>>,
    waker: Waker,
    w: f32,
    h: f32,
    scale: f32,
    pointer: (f32, f32),
    /// The modifiers held, as the window system last said.
    pub mods: Mods,
    /// The pointer is over something that lights up under it, so that
    /// moving it means drawing again.
    lively: bool,
    prompt: Option<Prompt>,
    chooser: Option<Chooser>,
    message: Option<String>,
    quit_armed: bool,
    words: Option<counts::Counts>,
    pub csd: bool,
    pub maximized: bool,
    /// A tiling compositor places the window: minimizing and maximizing
    /// it mean nothing there.
    pub tiled: bool,
    // Requests for the window system, cleared by it once served.
    pub redraw: bool,
    pub exit: bool,
    pub wait_mode: bool,
    pub wait_cancelled: bool,
    pub copy: Option<String>,
    pub copy_html: Option<String>,
    pub paste: bool,
    /// Asks the window system for the clipboard's text, for `$CLIPBOARD`.
    pub clipboard_read: bool,
    snippet_wait: Option<snippets::Waiting>,
    pub paste_plain: bool,
    pub paste_table: bool,
    diagrams: diagrams::Diagrams,
    citations: citations::Citations,
    notes: notes::Notes,
    links: links::Links,
    document_exports: document_export::DocumentExports,
    looks: looks::State,
    /// Text selected with the pointer, for the primary selection, which
    /// the middle button pastes.
    pub primary: Option<String>,
    pub paste_primary: bool,
    pub retitle: bool,
}

const MD_EXT: [&str; 7] = ["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "mdx"];
const TEX_EXT: [&str; 5] = ["tex", "latex", "ltx", "sty", "cls"];

/// The kinds a document can be shown as, whatever its name: each with the
/// key of its row in the list, its name and what it means.
const KINDS: [(Doc, &str, &str, &str); 5] = [
    (Doc::Markdown, "markdown", "Markdown", "rendered where it stands"),
    (Doc::Latex, "latex", "LaTeX", "highlighted, built to PDF"),
    (Doc::Python, "python", "Python", "highlighted, with completion"),
    (Doc::Text, "text", "Plain text", "in the reading font"),
    (Doc::Code, "code", "Code", "in the monospace font"),
];

fn ext(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Whether a file is a picture that can be shown, by its first bytes.
pub(crate) fn is_picture(path: &Path) -> bool {
    crate::pics::probe(path).is_some()
}

/// How a file is shown, by its name. A file without one is markdown.
/// Which word-wrap setting a kind of document follows: code (including
/// Python and LaTeX source), markdown, or plain text.
fn wrap_kind(doc: Doc) -> usize {
    match doc {
        Doc::Code | Doc::Python | Doc::Latex | Doc::Cells => 0,
        Doc::Markdown => 1,
        Doc::Text => 2,
    }
}

fn doc_for(path: Option<&Path>) -> Doc {
    match path.map(ext).as_deref() {
        None => Doc::Markdown,
        Some(e) if MD_EXT.contains(&e) => Doc::Markdown,
        Some("txt" | "text" | "") => Doc::Text,
        Some("py" | "pyi" | "pyw") => Doc::Python,
        Some(e) if TEX_EXT.contains(&e) => Doc::Latex,
        Some("ipynb") => Doc::Cells,
        Some(_) => Doc::Code,
    }
}

impl App {
    pub fn new(path: Option<PathBuf>) -> App {
        let cfg = Config::load();
        theme::set_light(cfg.light);
        let fonts = Rc::new(RefCell::new(Fonts::new()));
        let pics = Rc::new(RefCell::new(Pics::new()));
        let waker: Waker = Arc::new(|| {});
        let mut app = App {
            doc: Pane::new(fonts.clone(), pics.clone()),
            file_tabs: vec![FileTab::preview()],
            active_file: 0,
            closed_tabs: Vec::new(),
            views: Default::default(),
            split_mirrors: Vec::new(),
            focus_mode: false,
            preedit: None,
            ime_generation: 0,
            typewriter_mode: false,
            tab_drag: None,
            recovery: recovery::Recovery::new(),
            recent: convenience::recent_paths(),
            disk_checked: std::time::Instant::now(),
            history: Default::default(),
            pending_pr_root: None,
            pending_pr_head: None,
            pending_pr_comments: Vec::new(),
            worktrees: Default::default(),
            remote_projects: Default::default(),
            languages: Default::default(),
            problems: Default::default(),
            selection: Default::default(),
            user_commands: Default::default(),
            grammar: Default::default(),
            todos: Default::default(),
            http: Default::default(),
            coverage: Default::default(),
            heading_link_updates: Vec::new(),
            scripts: Default::default(),
            tests: Default::default(),
            mcp: Default::default(),
            debugger: debugging::Debugging::new(fonts.clone(), pics.clone()),
            debug_prep: None,
            task_state: Default::default(),
            saving: Default::default(),
            file_tab_hits: Vec::new(),
            tab_close_armed: None,
            nb: None,
            reader: None,
            helper: None,
            comp: None,
            comp_seq: 0,
            comp_box: None,
            comp_row: 0.0,
            docs: docs::Docs::default(),
            search: search::Search::default(),
            git: git::Git::default(),
            source: Default::default(),
            git_actions: Default::default(),
            speller: None,
            maths: Rc::new(RefCell::new(crate::math::Maths::new(&cfg.latex, waker.clone()))),
            spell_fix: None,
            suggest: None,
            predictions: predictions::State::default(),
            copilot: copilot_predictions::State::default(),
            suggest_due: None,
            inline_preview: None,
            ai: Assist::new(Side::new(fonts.clone(), pics.clone())),
            tree: Tree::new(fonts.clone()),
            outline: Outline::new(fonts.clone()),
            typeset: Typeset::new(Preview::new(fonts.clone(), pics.clone())),
            terms: Vec::new(),
            term_cur: 0,
            term_panes: None,
            term_open: false,
            term_close_armed: None,
            term_resizing: false,
            side_open: cfg.sidebar,
            tab: Tab::Agent,
            files_open: cfg.files_sidebar,
            compact_files: cfg.files_sidebar && !cfg.sidebar,
            resizing: None,
            hits: Vec::new(),
            panel_hidden: Vec::new(),
            panel_more: None,
            kind_hit: None,
            tip_hover: None,
            palette_mode: None,
            palette_stale: false,
            file_index: palette::FileIndex::default(),
            found_for: None,
            search_options: Default::default(),
            autosave_due: None,
            config_checks: Default::default(),
            found_count: (0, None),
            dialog_panel: None,
            focus: Focus::Doc,
            menu: None,
            popup: None,
            settings: None,
            keymap: Keymap::new(&cfg.keys),
            key_bindings: keybindings::KeyBindings::load(&cfg),
            rebinding: None,
            cfg,
            root: PathBuf::new(),
            folders: Vec::new(),
            window: Default::default(),
            file_clip: None,
            fonts,
            pics,
            waker,
            w: 0.0,
            h: 0.0,
            scale: 1.0,
            pointer: (0.0, 0.0),
            mods: Mods::default(),
            lively: false,
            prompt: None,
            chooser: None,
            message: None,
            quit_armed: false,
            words: None,
            csd: false,
            maximized: false,
            tiled: false,
            redraw: true,
            exit: false,
            wait_mode: false,
            wait_cancelled: false,
            copy: None,
            copy_html: None,
            paste: false,
            clipboard_read: false,
            snippet_wait: None,
            paste_plain: false,
            paste_table: false,
            diagrams: Default::default(),
            citations: Default::default(),
            notes: Default::default(),
            links: Default::default(),
            document_exports: Default::default(),
            looks: Default::default(),
            primary: None,
            paste_primary: false,
            retitle: true,
        };
        match path {
            Some(dir) if dir.is_dir() => app.show_folder(dir),
            Some(file) if roots::is_workspace(&file) => app.open_workspace(&file),
            path => app.load(path),
        }
        if let Some(error) = app.key_bindings.error.take() { app.message = Some(format!("Cannot load keymap: {error}")); }
        app.looks_init();
        app
    }

    /// The project a file belongs to: the window's folder holding it, or
    /// else the file's own project.
    fn root_for(&mut self, path: Option<&Path>) -> PathBuf {
        if let Some(root) = path.and_then(|p| self.root_of(p)) {
            return root;
        }
        match path {
            // A window left without a file stays in the project it was in.
            None if self.folders.contains(&self.root) => self.root.clone(),
            None if !self.folders.is_empty() => self.folders[0].clone(),
            None if !self.root.as_os_str().is_empty() => self.root.clone(),
            path => {
                // A window of one folder follows a file out of it; one of
                // several keeps them.
                if !self.multi_root() && self.window.workspace.is_none() {
                    self.folders.clear();
                }
                config::project_root(path)
            }
        }
    }

    /// Makes a folder the window's project, with nothing open in it, and
    /// shows the folder in the sidebar.
    fn show_folder(&mut self, dir: PathBuf) {
        self.show_folders(vec![dir]);
    }

    /// Replaces the document with the contents of `path`. A path that does
    /// not exist yet becomes a new, empty document saved there.
    fn load(&mut self, path: Option<PathBuf>) {
        self.load_as(path, true);
    }

    /// Loads a file, a picture as the picture if `pictured`, or else as
    /// the text it is, as an SVG is to be edited.
    fn load_as(&mut self, path: Option<PathBuf>, pictured: bool) {
        self.file_tabs[self.active_file].pinned = false;
        let mut text = String::new();
        self.doc.numbers = true;
        self.doc.read_only = false;
        self.message = None;
        self.reader = None;
        // A PDF is shown as its pages, and a picture as itself; neither's
        // bytes are text to edit.
        let paged = path.as_deref().filter(|p| p.is_file() && (pdf::is_pdf(p) || (pictured && is_picture(p))));
        if let Some(p) = paged {
            self.doc.read_only = true;
            let (pages, what) = match pdf::is_pdf(p) {
                true => (Pages::open(p), "PDF"),
                false => (Pages::picture(p), "picture"),
            };
            match pages {
                Ok(pages) => self.reader = Some(Reader::new(pages, self.pics.clone(), self.waker.clone())),
                Err(why) => self.message = Some(format!("Cannot show the {what}: {why}")),
            }
        } else if let Some(p) = &path {
            match std::fs::read(p) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(s) => text = s,
                    Err(e) => {
                        text = String::from_utf8_lossy(e.as_bytes()).into_owned();
                        self.doc.read_only = true;
                        self.message = Some("Not valid UTF-8, so it opened read-only".into());
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    self.doc.read_only = true;
                    self.message = Some(format!("Cannot open: {e}"));
                }
            }
        }
        let mut eol = editor::Eol::detect(&text);
        if eol == editor::Eol::Lf && path.as_deref().is_some_and(|p| saving::Rules::read(p).cr()) {
            eol = editor::Eol::Cr;
        }
        text = text.replace("\r\n", "\n");
        if eol == editor::Eol::Cr {
            text = text.replace('\r', "\n");
        }
        let mut doc = if paged.is_some() { Doc::Text } else { doc_for(path.as_deref()) };
        let root = self.root_for(path.as_deref());
        // Threads belong to a project, or to a window of several folders;
        // another project starts afresh.
        if root != self.root && !(self.folders.contains(&root) && self.folders.contains(&self.root) && self.multi_root()) {
            self.stop();
            self.ai.images.clear();
            self.ai.thread = agent::Thread::new();
            self.show_thread();
        }
        self.root = root;
        if let Err(e) = self.cfg.ensure_project(&self.root) { self.message = Some(e); }
        self.doc.path = path;
        (self.nb, self.helper, self.comp) = (None, None, None);
        self.doc.decor.clear();
        self.doc.headed = false;
        if doc == Doc::Cells && !self.doc.read_only {
            match Notebook::open(&text, self.pics.clone()) {
                Ok((mut nb, source, raw)) => {
                    self.doc.set_text(source, Doc::Cells);
                    nb.attach(&mut self.doc, raw);
                    self.nb = Some(nb);
                    self.doc.headed = true;
                }
                // Not a notebook after all: show what is there, untouched.
                Err(e) => {
                    self.message = Some(e);
                    self.doc.read_only = true;
                    doc = Doc::Code;
                }
            }
        } else if doc == Doc::Cells {
            doc = Doc::Code;
        }
        if self.nb.is_none() {
            self.doc.set_text(text.replace(cells::MARK, ""), doc);
            self.doc.buf.eol = eol;
            self.bookmarks_load();
        }
        self.words = None;
        self.retitle = true;
        self.redraw = true;
        self.typeset_loaded();
        self.disk_remember();
        if let Some(path) = self.doc.path.clone() {
            self.remember_recent(&path);
        }
    }

    pub fn title(&self) -> String {
        let project = self.roots_label().map_or(String::new(), |p| format!(" | {p}"));
        format!("{}{}{project} | Lightdown", self.name(), if self.doc.buf.modified { " \u{2022}" } else { "" })
    }

    fn name(&self) -> String {
        let name = self.doc.path.as_deref().and_then(Path::file_name);
        name.map_or_else(
            || self.doc.output_name.clone().unwrap_or_else(|| "Untitled".into()),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// When an animation on show moves on to its next frame.
    pub fn next_frame(&self) -> Option<std::time::Instant> {
        let frame = self.pics.borrow().next_frame();
        [self.autosave_due, frame, self.docs_due(), self.tip_due(), self.mcp.due(), self.suggest_due.map(|s| s.0)].into_iter().flatten().min()
    }

    /// Sets what to call, from another thread, when something has happened
    /// that `poll` should take in: an image decoded, a cell's output.
    pub fn set_waker(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.waker = Arc::new(wake);
        crate::treesitter::set_waker(self.waker.clone());
        self.pics.borrow_mut().set_waker(self.waker.clone());
        self.maths.borrow_mut().set_waker(self.waker.clone());
        if self.side_open && self.tab == Tab::Agent {
            self.ai.catalog.refresh(&self.waker);
        }
    }

    /// The interpreter and the directory it runs in.
    fn python(&self) -> (String, PathBuf) {
        let dir = self.doc.path.as_deref().and_then(Path::parent).filter(|d| d.is_dir());
        (self.cfg.python_for(&self.root), dir.map_or_else(|| self.root.clone(), Path::to_path_buf))
    }

    /// Takes in what other threads have produced since the last call.
    pub fn poll(&mut self) {
        self.keymap_poll();
        self.autosave_poll();
        if crate::treesitter::completed() { self.doc.reparse(); self.redraw = true; }
        if let Some(message) = crate::treesitter::message() { self.message = Some(message); }
        self.recovery_poll();
        self.redraw = true;
        self.poll_file_tabs();
        self.disk_poll(false);
        self.tasks_poll();
        self.user_commands_poll();
        self.grammar_poll();
        self.worktrees_poll();
        self.remote_poll();
        self.language_poll();
        self.scripts_poll();
        self.tests_poll();
        self.debug_poll();
        self.debug_prep_poll();
        self.saving_poll();
        let (python, dir) = self.python();
        let (mut matches, mut finished, mut infos) = (None, Vec::new(), Vec::new());
        if let Some(nb) = &mut self.nb {
            nb.env_root = Some(self.root.clone());
            nb.poll(&mut self.doc, &python, &dir, &self.waker);
            matches = nb.matches.take();
            finished = nb.take_finished();
            infos = std::mem::take(&mut nb.infos);
            if let Some(notice) = nb.notice.take() {
                self.message = Some(notice);
            }
        }
        if !finished.is_empty() && matches!(self.menu, Some((MenuKind::Variables, _))) {
            self.open_variables();
        }
        self.assist_poll(&finished);
        self.suggest_poll();
        self.typeset_poll();
        self.term_poll();
        self.search_poll();
        self.index_poll();
        self.todos_poll();
        self.http_poll();
        self.coverage_poll();
        self.backlinks_poll();
        self.links_poll();
        self.document_export_poll();
        self.diagrams_poll();
        self.git_poll();
        self.git_actions_poll();
        self.review_notes_tick();
        self.git_marks_tick();
        self.spell_poll();
        if self.maths.borrow_mut().poll() {
            self.redraw = true;
            if let Some(nb) = &mut self.nb {
                nb.refresh_all(&mut self.doc);
            }
        }
        for ev in self.helper.as_mut().map_or(Vec::new(), Kernel::poll) {
            match ev {
                Event::Matches { id, items } => matches = Some((id, items)),
                Event::Info { id, info } => infos.push((id, info)),
                Event::Dead(_) => self.helper = None,
                _ => {}
            }
        }
        if let Some((seq, items)) = matches {
            self.offer(seq, items);
        }
        for (id, info) in infos {
            self.docs_answer(id, info);
        }
    }

    /// Waits for work still in progress. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub fn settle(&mut self) {
        #[cfg(feature = "dump")]
        { crate::treesitter::settle(); if crate::treesitter::completed() { self.doc.reparse(); } }
        self.writing_settle();
        if let Some(reader) = &mut self.reader {
            reader.settle();
        }
        self.pics.borrow_mut().wait();
        self.git_tick();
        self.git_settle();
        self.git_marks_tick();
        self.git_actions_settle();
        self.file_blame_settle();
        self.debug_settle();
        self.http_settle();
        self.coverage_settle();
        self.language_settle();
        self.tests_settle();
        if let Some(s) = &mut self.speller {
            s.settle();
        }
        self.maths.borrow_mut().settle();
        self.search_settle();
        self.index_settle();
        self.todos_settle();
        for t in &mut self.terms {
            t.settle();
        }
        self.user_commands_settle();
        self.grammar_settle();
        self.suggest_settle();
        self.assist_settle();
        self.typeset_settle();
        let (python, dir) = self.python();
        if let Some(nb) = &mut self.nb {
            nb.settle(&mut self.doc, &python, &dir, &self.waker);
            // Formulas the cells printed are set once they ran.
            self.maths.borrow_mut().settle();
            nb.refresh_all(&mut self.doc);
            self.maths.borrow_mut().settle();
            nb.refresh_all(&mut self.doc);
        }
        while self.comp.as_ref().is_some_and(|c| c.seq == self.comp_seq) && self.helper.is_some() {
            match self.helper.as_mut().and_then(Kernel::wait) {
                Some(Event::Matches { id, items }) if id == self.comp_seq => {
                    self.offer(id, items);
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        self.docs_settle();
    }

    // ---- geometry -------------------------------------------------------

    pub fn resize(&mut self, w: u32, h: u32, scale: f32) {
        (self.w, self.h, self.scale) = (w as f32, h as f32, scale);
        self.arrange();
        self.redraw = true;
    }

    /// Height of the title bar, which is the window's own where the
    /// compositor draws none.
    fn title_h(&self) -> f32 {
        if self.focus_mode { return 0.0; }
        (theme::TITLE * self.scale).round()
    }

    /// Height of the heads of panels: the editor's tabs, the explorer's
    /// head and the right panel's, whose bottom edges line up.
    fn head_h(&self) -> f32 {
        if self.focus_mode { return 0.0; }
        (theme::HEAD * self.scale).round()
    }

    /// Height of the status bar along the bottom.
    fn status_h(&self) -> f32 {
        if self.focus_mode { return 0.0; }
        (theme::STATUS * self.scale).round()
    }

    /// Where the panels end: the status bar's top edge.
    fn body_bottom(&self) -> f32 {
        self.h - self.status_h()
    }

    /// The widths of the explorer at the left and the right panel, each
    /// with the hairline on its inner edge, leaving room for the editor.
    fn sidebar_widths(&self) -> (f32, f32) {
        if self.focus_mode { return (0.0, 0.0); }
        if self.w < 640.0 * self.scale {
            if self.files_open && (self.compact_files || !self.side_open) {
                return (self.w, 0.0);
            }
            return (0.0, if self.side_open { self.w } else { 0.0 });
        }
        let mut left = if self.files_open { self.cfg.files_width * self.scale } else { 0.0 };
        let mut right = if self.side_open { self.cfg.sidebar_width * self.scale } else { 0.0 };
        // The editor keeps 320 pixels, or two fifths of a narrow window.
        let room = self.w - (320.0 * self.scale).min(self.w * 0.4);
        let total = left + right;
        if total > room {
            left *= room / total;
            right *= room / total;
        }
        (left.round(), right.round())
    }

    fn side_width(&self) -> f32 {
        self.sidebar_widths().1
    }

    fn files_width(&self) -> f32 {
        self.sidebar_widths().0
    }

    /// Left edge and width of the editor's column.
    fn document_area(&self) -> (f32, f32) {
        let (left, right) = self.sidebar_widths();
        if left + right >= self.w {
            return (left.min(self.w), 0.0);
        }
        (left, self.w - left - right)
    }

    /// Height of the terminal panel, its top edge included, or 0 while it
    /// is hidden. The editor keeps some room above it.
    fn term_height(&self) -> f32 {
        if self.focus_mode { return 0.0; }
        if !self.term_open {
            return 0.0;
        }
        let tall = (self.body_bottom() - self.head_h() - self.title_h()).max(0.0);
        let most = (tall - 120.0 * self.scale).max(tall.min(80.0 * self.scale));
        (self.cfg.terminal_height * self.scale).min(most).max(tall.min(80.0 * self.scale)).round()
    }

    /// The first row of the terminal panel, under its top edge.
    fn term_top(&self) -> f32 {
        self.body_bottom() - self.term_height() + 1.0
    }

    /// Toggles wrapping for the kind of file in view, and keeps the choice.
    pub(super) fn toggle_word_wrap(&mut self) {
        let kind = wrap_kind(self.doc.doc);
        self.cfg.word_wrap[kind] = !self.cfg.word_wrap[kind];
        let _ = self.cfg.save();
        let what = ["code", "markdown", "plain text"][kind];
        self.message = Some(format!("Word wrap {} for {what}", if self.cfg.word_wrap[kind] { "on" } else { "off" }));
        self.arrange();
        self.doc.moved();
        if self.nb.is_some() {
            self.message = Some("Notebook cells always wrap".into());
        }
    }

    /// Gives each pane its place in the window.
    fn arrange(&mut self) {
        self.views_tidy();
        let bottom = self.body_bottom();
        let y0 = self.title_h() + self.head_h();
        let term = self.term_height();
        let term_top = self.term_top();
        let (left, width) = self.document_area();
        self.doc.minimap = self.cfg.minimap.clone();
        self.doc.minimap_size = self.cfg.minimap_width;
        self.doc.word_wrap = self.cfg.word_wrap[wrap_kind(self.doc.doc)];
        self.doc.set_modal_editing(&self.cfg.modal_editing);
        self.doc.indent_guides = self.cfg.indent_guides;
        crate::shell_integration::set_enabled(self.cfg.shell_integration);
        self.doc.ruler = self.cfg.ruler_for(&self.root);
        self.doc.size = self.cfg.font_size_for(&self.root);
        self.doc.family = if self.cfg.serif { font::SERIF } else { font::SANS };
        let (dx, dy, dw, dh) = self.editor_rect(self.file_tabs[self.active_file].id);
        self.doc.set_rect(dx, dy, dw, dh, self.scale);
        self.doc.focus_paragraph = self.focus_mode;
        if self.doc.typewriter != self.typewriter_mode {
            self.doc.typewriter = self.typewriter_mode;
            self.doc.moved();
        }
        if let Some(reader) = &mut self.reader { reader.set_rect(dx, dy, dw, dh, self.scale); }
        for i in self.split_others() {
            let (x, y, w, h) = self.editor_rect(self.file_tabs[i].id);
            let state = self.file_tabs[i].state.as_mut().unwrap();
            state.pane.set_modal_editing(&self.cfg.modal_editing);
            state.pane.indent_guides = self.cfg.indent_guides;
            state.pane.minimap = self.cfg.minimap.clone();
            state.pane.minimap_size = self.cfg.minimap_width;
            state.pane.word_wrap = self.cfg.word_wrap[wrap_kind(state.pane.doc)];
            state.pane.ruler = self.cfg.ruler_for(&state.root);
            state.pane.size = self.cfg.font_size_for(&state.root);
            state.pane.family = self.doc.family;
            state.pane.set_rect(x, y, w, h, self.scale);
            if let Some(r) = &mut state.reader { r.set_rect(x, y, w, h, self.scale); }
        }
        let list = self.term_list_w();
        let tw = (width - list).max(0.0);
        let half_w = (tw / 2.0).round();
        for (i, t) in self.terms.iter_mut().enumerate() {
            t.size = (self.cfg.font_size_for(&self.root) * 0.82).round();
            let (x, w) = match self.term_panes {
                Some([l, _]) if l == i => (left, half_w),
                Some([_, r]) if r == i => (left + half_w, tw - half_w),
                _ => (left, tw),
            };
            t.set_rect(x, term_top, if term > 0.0 { w } else { 0.0 }, (bottom - term_top).max(0.0), self.scale);
            // The right pane of a split draws the rule between the two.
            t.divided = matches!(self.term_panes, Some([_, r]) if r == i);
        }
        // The explorer stays at the left while the right panel switches views.
        let (files_w, side_w) = self.sidebar_widths();
        let edge = |w: f32| if w > 0.0 && w < self.w { 1.0 } else { 0.0 };
        let fw = (files_w - edge(files_w)).max(0.0);
        let (sx, sw) =
            if side_w > 0.0 { (self.w - side_w + edge(side_w), side_w - edge(side_w)) } else { (self.w, 0.0) };
        let width = |tab: Tab| if self.tab == tab { sw } else { 0.0 };
        let (agent, preview, outline) = (width(Tab::Agent), width(Tab::Preview), width(Tab::Outline));
        let tall = (bottom - y0).max(0.0);
        self.ai.side.set_rect(sx, y0, agent, tall, self.scale);
        self.tree.set_rect(0.0, y0, fw, tall, self.scale);
        self.typeset.view.set_rect(sx, y0, preview, tall, self.scale);
        self.outline.set_rect(sx, y0, outline.max(width(Tab::Backlinks)).max(width(Tab::Hierarchy)), tall, self.scale);
        self.search.panel.rect = (sx, y0, width(Tab::Search), tall);
        self.problems.rect = (sx,y0,width(Tab::Problems),tall);
        self.tests.rect = (sx,y0,width(Tab::Tests),tall);
        self.debugger.view.set_rect(sx, y0, width(Tab::Debug), tall, self.scale);
    }

    fn divider_at(&self, x: f32, y: f32) -> Option<Tab> {
        if y < self.title_h() || y >= self.body_bottom() {
            return None;
        }
        let (left, right) = self.sidebar_widths();
        // The hairline between two panels, and a little either side of it.
        let reach = 4.0 * self.scale;
        let near = |edge: f32| edge > 0.0 && edge < self.w && (x - edge).abs() <= reach;
        if left > 0.0 && near(left - 0.5) {
            Some(Tab::Files)
        } else if right > 0.0 && near(self.w - right + 0.5) {
            Some(self.tab)
        } else {
            None
        }
    }

    fn on_divider(&self, x: f32, y: f32) -> bool {
        self.divider_at(x, y).is_some()
    }

    fn side_is_open(&self, tab: Tab) -> bool {
        match tab {
            Tab::Files => self.files_open,
            tab => self.side_open && self.tab == tab,
        }
    }

    /// Whether the keyboard is with either sidebar.
    fn in_side(&self) -> bool {
        (self.files_open && self.focus == Focus::Files)
            || (self.side_open && self.tab == Tab::Agent && self.focus == Focus::Chat)
    }

    /// Whether the keyboard is with the document.
    fn doc_has_keys(&self) -> bool {
        !self.in_side() && !(self.term_open && self.focus == Focus::Term)
    }

    /// Open and focus a sidebar, or return its keyboard focus to the document.
    fn toggle_side(&mut self, tab: Tab) {
        let has = self.side_is_open(tab) && self.focus == tab.focus();
        self.show_side(Some(tab));
        self.focus = if has { Focus::Doc } else { tab.focus() };
    }

    /// Shows the outline in the right sidebar, or closes it.
    fn toggle_outline(&mut self) {
        let open = self.side_open && self.tab == Tab::Outline && self.side_width() > 0.0;
        self.show_side(if open { None } else { Some(Tab::Outline) });
    }

    /// Puts the cursor at a place in the document, with the line in view.
    fn go_to(&mut self, at: usize) {
        if self.reader.is_some() {
            return;
        }
        let at = editor::floor(&self.doc.buf.text, at.min(self.doc.buf.text.len()));
        self.remember_jump();
        self.doc.select(at, at);
        self.doc.moved();
        self.focus = Focus::Doc;
        self.redraw = true;
    }

    fn show_files(&mut self, open: bool) {
        self.files_open = open;
        self.compact_files = open;
        if !open && self.focus == Focus::Files {
            self.focus = Focus::Doc;
        }
        if open {
            self.tree.show_roots(&self.roots(), self.doc.path.as_deref());
            self.tree.refresh();
        }
        if self.cfg.files_sidebar != open {
            self.cfg.files_sidebar = open;
            let _ = self.cfg.save();
        }
        self.comp = None;
        self.arrange();
    }

    /// Files open independently; agent and preview share the right sidebar.
    /// None closes only the right sidebar.
    fn show_side(&mut self, tab: Option<Tab>) {
        if tab == Some(Tab::Files) {
            return self.show_files(true);
        }
        let open = tab.is_some();
        self.tab = tab.unwrap_or(self.tab);
        self.side_open = open;
        self.compact_files = false;
        if self.focus == Focus::Chat && (!open || self.tab != Tab::Agent) {
            self.focus = Focus::Doc;
        }
        // Preview visibility is temporary; remember the agent's preference.
        if self.tab == Tab::Agent && self.cfg.sidebar != open {
            self.cfg.sidebar = open;
            let _ = self.cfg.save();
        }
        if tab == Some(Tab::Agent) {
            self.ai.catalog.refresh(&self.waker);
        }
        self.comp = None;
        self.arrange();
    }

    fn hide_sidebars(&mut self) {
        self.show_files(false);
        self.show_side(None);
        self.focus = Focus::Doc;
    }

    /// The window has the keyboard again. Whatever else was at work in the
    /// meantime may have changed the folder.
    pub fn attend(&mut self) {
        self.disk_poll(true);
        if self.files_open {
            self.tree.refresh();
            self.redraw = true;
        }
        // A PDF built again elsewhere is shown as it is now.
        if self.reader.as_mut().is_some_and(Reader::refresh) {
            self.redraw = true;
        }
    }

    /// Whether a list, a panel or a dialog stands over the window and has
    /// its input.
    fn overlaid(&self) -> bool {
        self.menu.is_some()
            || self.settings.is_some()
            || self.chooser.is_some()
            || self.popup.is_some()
            || self.prompt.as_ref().is_some_and(|p| !p.finds())
    }

    /// Whether what stands over the window dims it: the dialogs, the
    /// palette and the settings, but not a list dropping from a control.
    fn modal(&self) -> bool {
        let menu = self.menu.as_ref().is_some_and(|(_, m)| m.anchor.is_none());
        menu || self.settings.is_some() || self.chooser.is_some() || self.prompt.as_ref().is_some_and(|p| !p.finds())
    }

    pub fn zone(&self, x: f32, y: f32, ctrl: bool) -> Zone {
        if self.csd && !self.maximized {
            let m = 5.0 * self.scale;
            let (l, r, t, b) = (x < m, x >= self.w - m, y < m, y >= self.h - m);
            let edge = match (l, r, t, b) {
                (true, _, true, _) => Some(Edge::TopLeft),
                (_, true, true, _) => Some(Edge::TopRight),
                (true, _, _, true) => Some(Edge::BottomLeft),
                (_, true, _, true) => Some(Edge::BottomRight),
                (true, ..) => Some(Edge::Left),
                (_, true, ..) => Some(Edge::Right),
                (_, _, true, _) => Some(Edge::Top),
                (_, _, _, true) => Some(Edge::Bottom),
                _ => None,
            };
            if let Some(e) = edge {
                return Zone::Resize(e);
            }
        }
        let overlaid = self.overlaid();
        if let Some(hit) = self.hit_at(x, y).filter(|h| !overlaid || matches!(h, Hit::Dialog(_))) {
            return match hit {
                Hit::Close => Zone::Close,
                Hit::Minimize => Zone::Minimize,
                Hit::Maximize => Zone::Maximize,
                Hit::Find(find::FindHit::Field(_)) | Hit::Dialog(dialog::DialogHit::Field) => Zone::Text,
                _ => Zone::Button,
            };
        }
        if y < self.title_h() {
            return if self.csd { Zone::Title } else { Zone::Plain };
        }
        if overlaid {
            return Zone::Plain;
        }
        if self.find_rect().is_some_and(|r| ui::inside(r, Some((x, y)))) {
            return Zone::Plain;
        }
        if self.file_tab_hit(x, y).is_some() || self.notebook_hit(x, y).is_some() || self.doc.test_under(x, y).is_some()
        {
            return Zone::Button;
        }
        if let Some(zone) = self.view_divider_zone(x, y) {
            return zone;
        }
        if self.on_divider(x, y) {
            return Zone::Divider;
        }
        if let Some(t) = self.term_under(x, y) {
            if t.on_edge(x, y) {
                return Zone::RowDivider;
            }
            if t.contains(x, y) {
                return match () {
                    _ if ctrl && t.link_at(x, y).is_some() => Zone::Link,
                    _ if t.on_text(x, y) && !t.grabs_mouse(self.mods) => Zone::Text,
                    _ => Zone::Plain,
                };
            }
        }
        if self.tree.contains(x, y) {
            return if self.tree.hit(x, y) { Zone::Button } else { Zone::Plain };
        }
        if self.typeset.view.contains(x, y) {
            return if self.typeset.view.hit(x, y).is_some() { Zone::Button } else { Zone::Plain };
        }
        if self.outline.contains(x, y) {
            return if self.outline.hit(x, y) { Zone::Button } else { Zone::Plain };
        }
        let side = &self.ai.side;
        if side.contains(x, y) {
            let text = |p: &Pane| p.contains(x, y);
            return match side.hit(x, y) {
                Some(_) => Zone::Button,
                None if ctrl && text(&side.transcript) && side.transcript.link_at(x, y).is_some() => Zone::Link,
                None if side.transcript.on_bar(x, y) || side.input.on_bar(x, y) => Zone::Plain,
                None if text(&side.transcript) || text(&side.input) => Zone::Text,
                None => Zone::Plain,
            };
        }
        if let Some(reader) = self.reader.as_ref().filter(|r| r.contains(x, y)) {
            return if reader.link_at(x, y).is_some() {
                Zone::Link
            } else if reader.on_page(x, y) && !reader.picture() {
                Zone::Text
            } else {
                Zone::Plain
            };
        }
        if self.mirror_at(x, y).is_some() {
            return Zone::Text;
        }
        if self.split_others().into_iter().any(|i| self.file_tabs[i].state.as_ref().unwrap().pane.contains(x, y)) {
            return Zone::Text;
        }
        if self.welcomes() || !self.doc.contains(x, y) || self.doc.on_bar(x, y) {
            return Zone::Plain;
        }
        if ctrl && self.doc.link_at(x, y).is_some() { Zone::Link } else { Zone::Text }
    }

    // ---- scrolling ------------------------------------------------------

    /// The terminal, if it is under the pointer and nothing stands over it.
    fn pointed_term(&mut self) -> Option<&mut Term> {
        let (x, y) = self.pointer;
        let free = !self.overlaid() && self.term_open;
        let i = self.shown_terms().into_iter().find(|&i| self.terms[i].contains(x, y))?;
        self.terms.get_mut(i).filter(|_| free)
    }

    /// The pane under the pointer, which is the one the wheel scrolls.
    fn pointed(&mut self) -> Option<&mut Pane> {
        let (x, y) = self.pointer;
        let lists = self.tree.contains(x, y) || self.typeset.view.contains(x, y) || self.outline.contains(x, y);
        let term = self.term_under(x, y).is_some_and(|t| t.contains(x, y));
        if self.settings.is_some() || self.chooser.is_some() || lists || term {
            None
        } else if self.ai.side.contains(x, y) {
            let side = &mut self.ai.side;
            Some(if side.input.contains(x, y) { &mut side.input } else { &mut side.transcript })
        } else if let Some(i) = self.split_others().into_iter()
            .find(|&i| self.file_tabs[i].state.as_ref().unwrap().pane.contains(x, y))
        {
            Some(&mut self.file_tabs[i].state.as_mut().unwrap().pane)
        } else {
            Some(&mut self.doc)
        }
    }

    pub fn scroll(&mut self, dy: f32) {
        if self.docs_scroll(dy) {
            return;
        }
        self.comp = None;
        let (x, y) = self.pointer;
        let mods = self.mods;
        if let Some(t) = self.pointed_term() {
            t.scroll(dy, mods);
        } else if let Some((_, popup)) = &mut self.popup {
            popup.scroll(dy);
        } else if let Some((_, menu)) = &mut self.menu {
            menu.scroll(dy / (theme::CONTROL * self.scale));
        } else if let Some(chooser) = &mut self.chooser {
            chooser.scroll(dy);
        } else if self.settings.is_some() {
            self.settings_scroll(dy);
        } else if self.source_shown() && self.tree.contains(x, y) {
            self.source_scroll(dy);
        } else if self.tree.contains(x, y) {
            self.tree.scroll(dy);
        } else if self.settings.is_none() && self.typeset.view.contains(x, y) {
            self.typeset.view.scroll(dy);
        } else if self.search_panel_contains(x, y) {
            self.search.panel.scroll = (self.search.panel.scroll + dy).max(0.0);
        } else if self.tests.contains(x,y) {
            self.tests.scroll_by(dy);
        } else if self.problems.contains(x,y) {
            self.problems.scroll(dy);
        } else if self.debugger.view.contains(x, y) {
            self.debugger.view.scroll(dy);
        } else if self.settings.is_none() && self.outline.contains(x, y) {
            self.outline.scroll(dy);
        } else if let Some(reader) = self.reader.as_mut().filter(|r| r.contains(x, y) && self.settings.is_none()) {
            reader.scroll(dy);
        } else if let Some(j) = self.mirror_at(x, y).filter(|_| self.settings.is_none() && self.chooser.is_none()) {
            self.with_mirror(j, |pane| pane.scroll(dy));
        } else if let Some(pane) = self.pointed() {
            pane.scroll(dy);
        }
        self.redraw = true;
    }

    pub fn scroll_lines(&mut self, lines: f32) {
        if self.docs_scroll(lines * theme::CONTROL * self.scale) {
            return;
        }
        self.comp = None;
        let (x, y) = self.pointer;
        let mods = self.mods;
        if let Some(t) = self.pointed_term() {
            t.scroll_lines(lines, mods);
        } else if let Some((_, popup)) = &mut self.popup {
            popup.scroll(lines * theme::CONTROL * self.scale);
        } else if let Some((_, menu)) = &mut self.menu {
            menu.scroll(lines);
        } else if let Some(chooser) = &mut self.chooser {
            chooser.scroll_lines(lines);
        } else if self.settings.is_some() {
            self.settings_scroll(lines * theme::CONTROL * self.scale);
        } else if self.source_shown() && self.tree.contains(x, y) {
            self.source_scroll(lines * theme::ROW * self.scale);
        } else if self.tree.contains(x, y) {
            self.tree.scroll_lines(lines);
        } else if self.settings.is_none() && self.typeset.view.contains(x, y) {
            self.typeset.view.scroll_lines(lines);
        } else if self.search_panel_contains(x, y) {
            self.search.panel.scroll = (self.search.panel.scroll + lines * theme::ROW * self.scale).max(0.0);
        } else if self.tests.contains(x,y) {
            self.tests.scroll_by(lines*theme::ROW*self.scale);
        } else if self.problems.contains(x,y) {
            self.problems.scroll(lines*theme::ROW*self.scale);
        } else if self.debugger.view.contains(x, y) {
            self.debugger.view.scroll_lines(lines);
        } else if self.settings.is_none() && self.outline.contains(x, y) {
            self.outline.scroll_lines(lines);
        } else if let Some(reader) = self.reader.as_mut().filter(|r| r.contains(x, y) && self.settings.is_none()) {
            reader.scroll_lines(lines);
        } else if let Some(j) = self.mirror_at(x, y).filter(|_| self.settings.is_none() && self.chooser.is_none()) {
            self.with_mirror(j, |pane| pane.scroll_lines(lines));
        } else if let Some(pane) = self.pointed() {
            pane.scroll_lines(lines);
        }
        self.redraw = true;
    }

    /// Scrolls sideways what is under the pointer: the pages of a PDF, or
    /// unwrapped text.
    pub fn scroll_across(&mut self, dx: f32) {
        let (x, y) = self.pointer;
        if self.overlaid() {
            return;
        }
        if let Some(reader) = self.reader.as_mut().filter(|r| r.contains(x, y)) {
            reader.scroll_across(dx);
        } else {
            self.text_across(|pane| pane.scroll_horizontal(dx));
        }
        self.redraw = true;
    }

    pub fn scroll_across_lines(&mut self, lines: f32) {
        let (x, y) = self.pointer;
        if self.overlaid() {
            return;
        }
        if let Some(reader) = self.reader.as_mut().filter(|r| r.contains(x, y)) {
            reader.scroll_across_lines(lines);
        } else {
            // A step is two spaces' width of the body text.
            self.text_across(|pane| pane.scroll_horizontal(lines * pane.px() * 1.2));
        }
        self.redraw = true;
    }

    /// Scrolls sideways the editor view under the pointer, a second view
    /// included; other panes wrap and stay put.
    fn text_across(&mut self, f: impl FnOnce(&mut Pane)) {
        let (x, y) = self.pointer;
        if self.settings.is_some() || self.chooser.is_some() || self.ai.side.contains(x, y) {
            return;
        }
        if let Some(j) = self.mirror_at(x, y) {
            self.with_mirror(j, f);
        } else if let Some(pane) = self.pointed() {
            f(pane);
        }
    }

    pub fn zoom_by(&mut self, steps: i32) {
        let (x, y) = self.pointer;
        if let Some(t) = self.pointed_term() {
            t.zoom_by(steps);
        } else if let Some(reader) = self.reader.as_mut().filter(|r| r.contains(x, y)) {
            reader.zoom_by(steps);
        } else if let Some(pane) = self.pointed() {
            pane.zoom_by(steps);
        }
        self.redraw = true;
    }

    // ---- keyboard -------------------------------------------------------

    pub fn key(&mut self, key: Key, mods: Mods, text: Option<&str>) {
        // A modifier going down on its own is not input.
        if key == Key::Other && text.is_none_or(str::is_empty) {
            return;
        }
        self.redraw = true;
        self.tip_hover = None;
        if key == Key::Escape && self.preedit.is_some() { self.input_clear(); return; }
        if self.rebinding.is_some() {
            return self.rebind_key(key, mods);
        }
        if self.contextual_key(key, mods) { return; }
        if let Some((target, popup)) = &mut self.popup {
            match popup.key(key) {
                Picked::Key(k) => {
                    let target = target.clone();
                    self.popup = None;
                    self.context_pick(target, &k);
                }
                Picked::Close => self.popup = None,
                _ => {}
            }
            return self.collect();
        }
        if self.term_has_keys() && mods.ctrl && mods.shift && key == Key::Char('f') {
            return self.ask(PromptKind::TerminalFind);
        }
        if self.term_has_keys() && key == Key::F(3) {
            return self.terminal_find_step(mods.shift);
        }
        // The terminal takes the keys before they are read as shortcuts.
        let focus_key = mods.ctrl && mods.alt && !mods.shift && key == Key::Char('z');
        if self.term_has_keys() && self.preedit.is_some() { self.input_clear(); }
        if !self.focus_mode && !focus_key && self.term_has_keys() && self.term_key(key, mods, text) {
            return;
        }
        if self.user_command_key(key,mods) { return; }
        // From here on a shortcut is known by its default keys, wherever
        // the user has put it.
        let (key, mods, text) = match self.keymap.meaning(Chord::new(key, mods), |scope| self.in_scope(scope)) {
            Meaning::Itself => (key, mods, text),
            Meaning::Default(chord) => (chord.key, chord.mods, None),
            Meaning::Nothing => return,
        };
        self.dispatch(key, mods, text);
    }

    /// Does what a shortcut does, by its name, as if its keys were pressed.
    pub(super) fn act(&mut self, id: &str) {
        let Some(chord) = ACTIONS.iter().find(|a| a.id == id).and_then(|a| Chord::parse(a.keys[0])) else { return };
        self.redraw = true;
        self.dispatch(chord.key, chord.mods, None);
    }

    /// Carries out a key, a shortcut known by its default keys.
    fn dispatch(&mut self, key: Key, mods: Mods, text: Option<&str>) {
        if key == Key::Escape && self.preedit.is_some() { self.input_clear(); return; }
        if self.suggest_key(key, mods, text) {
            return self.collect();
        }
        if mods.alt && !mods.ctrl && !mods.shift && key == Key::Char('l') {
            self.toggle_find_selection(); return;
        }
        if self.search_panel_key(key, mods, text) { return; }
        if self.prompt_has_keys() {
            return self.prompt_key(key, mods, text);
        }
        if self.chooser.is_some() {
            return self.chooser_key(key, mods, text);
        }
        if let Some((kind, _)) = &self.menu {
            // The palette's own keys switch what it lists.
            if *kind == MenuKind::Rebase && self.rebase_key(key, mods) {
                return self.collect();
            }
            let Some((kind, menu)) = &mut self.menu else { return };
            if *kind == MenuKind::Palette && mods.ctrl && !mods.alt && matches!(key, Key::Char('p') | Key::Char('g')) {
                let prefix = match (key, mods.shift) {
                    (Key::Char('g'), _) => ":",
                    (_, true) => ">",
                    _ => "",
                };
                menu.set_typed(prefix);
                self.palette_update();
                return self.collect();
            }
            let (kind, picked) = (*kind, menu.key(key, mods, text));
            match picked {
                Picked::None => {}
                Picked::Close => self.menu = None,
                Picked::Key(k) => {
                    self.menu = None;
                    self.menu_pick(kind, k);
                }
                Picked::Remove(k) => self.menu_remove(kind, &k),
            }
            if matches!(self.menu, Some((MenuKind::Palette, _))) {
                self.palette_update();
            }
            self.search_update();
            return self.collect();
        }
        if self.settings.is_some() {
            return self.settings_key(key, mods, text);
        }
        if self.notebook_input_key(key, mods, text) {
            return;
        }
        if self.tests.typing && self.tests_key(key, mods, text) {
            return;
        }
        if self.debug_tree_key(key, mods) {
            return self.collect();
        }
        if self.view_key(key, mods) {
            return self.collect();
        }
        if (mods.ctrl && mods.alt && !mods.shift && key == Key::Char('z'))
            || (key == Key::Escape && self.focus_mode && !self.vim_takes(key, mods))
        {
            self.focus_mode = !self.focus_mode;
            self.focus = Focus::Doc;
            self.arrange();
            return;
        }
        if mods.ctrl && mods.alt && !mods.shift && key == Key::Char('w') {
            self.typewriter_mode = !self.typewriter_mode;
            self.arrange();
            return;
        }
        if mods.ctrl && mods.alt && !mods.shift && key == Key::Char('y') { return self.agent_hunk(None, true); }
        if mods.ctrl && mods.alt && !mods.shift && key == Key::Enter { return self.http_send(); }
        if mods.ctrl && mods.alt && !mods.shift && key == Key::Char('u') { return self.agent_hunk(None, false); }
        if mods.ctrl && mods.shift && mods.alt && key == Key::Char('t') {
            return self.reopen_closed_tab();
        }
        if mods.ctrl && key == Key::Tab {
            let count = self.file_tabs.len();
            self.activate_file((self.active_file + if mods.shift { count - 1 } else { 1 }) % count);
            return;
        }
        if mods.ctrl && mods.shift && !mods.alt && key == Key::Char('w') && self.focus == Focus::Term && self.term_open
        {
            return self.close_term(self.term_cur);
        }
        self.term_close_armed = None;
        if mods.ctrl && !mods.alt && key == Key::Char('w') {
            self.close_file_tab(self.active_file);
            return;
        }
        self.tab_close_armed = None;
        let armed = std::mem::take(&mut self.quit_armed);
        self.message = None;
        match key {
            Key::Left | Key::Right if mods.alt && mods.shift && !mods.ctrl => return self.selection_change(key == Key::Left),
            Key::Left | Key::Right if mods.alt && !mods.ctrl => return self.navigate(key == Key::Right),
            Key::Char('r') if mods.ctrl && mods.alt => return self.open_palette("#"),
            Key::Char('h') if mods.alt && mods.shift => return self.language_command(if mods.ctrl { "hierarchy_outgoing" } else { "hierarchy_incoming" }),
            Key::Char('h') if mods.ctrl && mods.alt => return self.git_command("git_hunk"),
            Key::Char('b') if mods.ctrl && mods.alt && self.focus == Focus::Doc => {
                return self.run_command(if mods.shift { "bracket_select" } else { "bracket_jump" });
            }
            Key::Char('l') if mods.ctrl && mods.alt && !mods.shift && self.focus == Focus::Doc => {
                return self.run_command("cursor_all");
            }
            Key::Char('d') if mods.ctrl && mods.alt && mods.shift && self.focus == Focus::Doc => {
                return self.run_command("cursor_skip");
            }
            Key::Char('k') if mods.ctrl && mods.alt && !mods.shift && self.focus == Focus::Doc => {
                return self.bookmark_command("bookmark_toggle");
            }
            Key::Char(']') if mods.ctrl && mods.alt && self.focus == Focus::Doc => return self.bookmark_command("bookmark_next"),
            Key::Char('[') if mods.ctrl && mods.alt && self.focus == Focus::Doc => {
                return self.bookmark_command("bookmark_previous");
            }
            Key::Char('z') if mods.alt && !mods.ctrl && !mods.shift => return self.run_command("word_wrap"),
            Key::F(7) => return self.run_configs(true),
            Key::F(9) => return self.debug_command("breakpoint"),
            Key::F(10) => return self.debug_command("next"),
            Key::F(11) => return self.debug_command(if mods.shift { "out" } else { "step" }),
            Key::F(5) if mods.ctrl => return self.debug_command("start"),
            Key::F(6) => return self.open_tasks(),
            Key::F(8) if mods.alt && !mods.ctrl => return self.git_command(if mods.shift { "git_previous_change" } else { "git_next_change" }),
            Key::F(8) => return self.next_error(mods.shift),
            Key::Char('y') if mods.ctrl && mods.shift && !mods.alt => return self.open_tests(),
            Key::Char(',') if mods.ctrl => return self.open_settings(),
            Key::F(1) => {
                self.comp = None;
                return self.open_menu(MenuKind::Keys);
            }
            Key::F(3) if self.prompt.as_ref().is_some_and(Prompt::finds) => return self.find_step(mods.shift),
            Key::Char('p') if mods.ctrl && mods.shift && !mods.alt => return self.open_palette(">"),
            Key::Char('p') if mods.ctrl && !mods.alt => return self.open_palette(""),
            Key::Char('g') if mods.ctrl && !mods.shift && !mods.alt => return self.open_palette(":"),
            Key::Char('r') if mods.ctrl && !mods.shift && !mods.alt && !self.in_side() && !self.vim_takes(key, mods) => {
                return self.open_palette("@");
            }
            Key::Char('.') if mods.ctrl && mods.alt => return self.language_command("actions"),
            Key::Char(' ') if mods.ctrl && mods.alt => return self.language_command("signature"),
            Key::Char('u') if mods.alt && mods.shift && !mods.ctrl => return self.language_command("hierarchy_supertypes"),
            Key::Char('d') if mods.alt && mods.shift && !mods.ctrl => return self.language_command("hierarchy_subtypes"),
            Key::F(12) if mods.alt && !mods.ctrl => return self.language_command(if mods.shift { "peek_references" } else { "peek_definition" }),
            Key::F(12) => return self.language_command(if mods.ctrl && mods.alt { "declaration" } else if mods.ctrl && mods.shift { "type_definition" } else if mods.ctrl { "implementation" } else if mods.shift { "references" } else { "definition" }),
            Key::F(2) if mods.ctrl => return self.language_command("rename"),
            Key::Char('i') if mods.ctrl && mods.alt => return self.ask(PromptKind::InlineEdit),
            Key::F(2) => return self.file_action("rename"),
            Key::F(5) => return self.file_action("preview"),
            Key::Char('p') if mods.ctrl && mods.alt => return self.file_action("export_pdf"),
            Key::Char('j') if mods.ctrl && mods.alt => return self.sync_forward(),
            Key::Char('f') if mods.alt => return self.file_action("file_menu"),
            Key::Char('n') if mods.ctrl && mods.alt => return self.file_action("new_file"),
            Key::Char('s') if mods.ctrl && mods.alt => return self.file_action("save_copy"),
            Key::Char('x') if mods.ctrl && mods.alt => return self.file_action("trash"),
            Key::Char('o') if mods.ctrl && mods.alt => return self.file_action("reveal"),
            Key::Char('c') if mods.ctrl && mods.alt => return self.file_action("copy_path"),
            Key::Char('m') if mods.ctrl && mods.alt => return self.file_action("doc_kind"),
            Key::Char('l') if mods.ctrl && mods.shift => return self.hide_sidebars(),
            Key::Char('l') if mods.ctrl => return self.toggle_side(Tab::Agent),
            Key::Char('e') if mods.ctrl && mods.shift => return self.explorer_toggle(),
            Key::Char('g') if mods.ctrl && mods.shift => return self.source_toggle(),
            Key::Char('`') if mods.ctrl && mods.alt => return self.show_term(false),
            Key::Char('t') if mods.ctrl && mods.shift && !mods.alt => return self.add_term(),
            Key::Char('f') if mods.ctrl && mods.shift && !mods.alt => return self.open_search(),
            Key::Char('h') if mods.ctrl && mods.shift && !mods.alt => return self.toggle_outline(),
            Key::Char('h') if mods.ctrl && !mods.alt => {
                self.focus = Focus::Doc;
                return self.ask(PromptKind::Replace);
            }
            Key::Char('j') if mods.ctrl && mods.shift && !mods.alt => return self.toggle_split(),
            Key::PageDown if mods.ctrl && mods.shift && self.term_open => return self.cycle_term(1),
            Key::PageUp if mods.ctrl && mods.shift && self.term_open => return self.cycle_term(-1),
            Key::Char('`') if mods.ctrl => return self.toggle_term(),
            _ => {}
        }
        // What reaches here from the terminal is the window's: saving
        // under another name, say. Nothing is typed into the document.
        if self.focus == Focus::Term && self.term_open {
            if key == Key::Char('s') && mods.ctrl && mods.shift {
                self.ask(PromptKind::SaveAs);
            }
            return self.collect();
        }
        if self.focus == Focus::Files && self.source_shown() {
            match key {
                Key::Char('q') if mods.ctrl => {
                    self.quit_armed = armed;
                    return self.close();
                }
                Key::Char('s') if mods.ctrl && !mods.shift => return self.save(),
                _ => return self.source_key(key, mods, text),
            }
        }
        if self.focus == Focus::Files && self.files_open {
            match key {
                Key::Char('q') if mods.ctrl => {
                    self.quit_armed = armed;
                    return self.close();
                }
                Key::Char('s') if mods.ctrl && !mods.shift => return self.save(),
                // Finding and opening are about the document.
                Key::Char('f' | 'o') if mods.ctrl && !mods.shift => self.focus = Focus::Doc,
                _ => return self.tree_key(key, mods),
            }
        }
        if self.focus == Focus::Chat && self.side_is_open(Tab::Agent) {
            match key {
                Key::Char('q') if mods.ctrl => {
                    self.quit_armed = armed;
                    self.close();
                }
                Key::Char('s') if mods.ctrl && !mods.shift => self.save(),
                Key::Char('v') if mods.ctrl => { self.paste = true; self.paste_plain = mods.shift; },
                // Finding and opening are about the document.
                Key::Char('f' | 'o') if mods.ctrl && !mods.shift => self.focus = Focus::Doc,
                _ => self.chat_key(key, mods, text),
            }
            if self.focus == Focus::Chat {
                return self.collect();
            }
        }
        if self.completion_key(key, mods) {
            return;
        }
        if self.peek_key(key, mods) {
            return;
        }
        if self.vim_takes(key, mods) {
            self.doc.key(key, mods, text);
            self.vim_request();
            self.edited();
            return self.collect();
        }
        if key == Key::Escape && self.git_actions.hunk.is_some() {
            self.git_actions.hunk = None;
            self.redraw = true;
            return;
        }
        if key == Key::Escape && self.docs_escape() {
            return;
        }
        // A find left on show closes with Esc from the document.
        if key == Key::Escape && self.prompt.is_some() {
            self.prompt = None;
            return;
        }
        match key {
            Key::Char('q') if mods.ctrl => {
                self.quit_armed = armed;
                self.close();
            }
            Key::Char('s') if mods.ctrl && mods.shift => self.ask(PromptKind::SaveAs),
            Key::Char('s') if mods.ctrl => self.save(),
            Key::Char('o') if mods.ctrl && !mods.shift => self.file_action("open"),
            Key::Char('f') if mods.ctrl => self.ask(PromptKind::Find),
            Key::Char('n') if mods.ctrl && !mods.shift => spawn_self(None),
            Key::Char('c') if mods.ctrl && mods.shift => self.copy_rich(),
            Key::Char('v') if mods.ctrl => { self.paste = true; self.paste_plain = mods.shift; },
            Key::Char('=' | '+' | '-' | '0') if mods.ctrl && self.reader.is_some() => {
                let reader = self.reader.as_mut().unwrap();
                match key {
                    Key::Char('-') => reader.zoom_by(-1),
                    Key::Char('0') => reader.zoom_reset(),
                    _ => reader.zoom_by(1),
                }
            }
            Key::Char('=' | '+') if mods.ctrl => self.doc.zoom_by(1),
            Key::Char('-') if mods.ctrl => self.doc.zoom_by(-1),
            Key::Char('0') if mods.ctrl => self.doc.zoom_reset(),
            Key::Char('\\') if mods.alt => self.ask_suggestion(),
            Key::Char(' ') if mods.ctrl && mods.shift => self.ask_suggestion(),
            Key::Char(' ') if mods.ctrl => self.complete(true),
            Key::Char('k') if mods.ctrl && !mods.shift && !mods.alt => self.show_docs(),
            _ if self.multibuffer_key(key, mods) => {}
            _ if self.notebook_key(key, mods) => {}
            _ if self.reader.is_some() => {
                let reader = self.reader.as_mut().unwrap();
                match key {
                    Key::Char('c') if mods.ctrl => self.copy = reader.selected(),
                    Key::Char('a') if mods.ctrl => reader.select_all(),
                    _ => {
                        reader.key(key, mods);
                    }
                }
            }
            _ => {
                let typed = matches!(key, Key::Char(_) | Key::Other | Key::Backspace)
                    && !mods.ctrl
                    && !mods.alt
                    && self.doc.vim_typing();
                self.doc.key(key, mods, text);
                self.vim_request();
                self.edited();
                if typed {
                    self.complete(false);
                    self.docs_typed(text);
                } else {
                    self.comp = None;
                    self.snippet_offer_choices();
                }
            }
        }
        self.collect();
    }

    /// Call after anything that may have changed the document.
    fn edited(&mut self) {
        self.multibuffer_tick();
        self.bookmarks_sync();
        if self.cfg.auto_save_for(&self.root) == "after_delay" {
            self.autosave_due = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
        }
        self.schedule_suggestion();
        if let Some(nb) = &mut self.nb {
            nb.sync(&mut self.doc);
        }
        self.pin_edited_tab();
        self.words = None;
        self.retitle = true;
    }

    /// Passes on what the panes asked for while handling input.
    fn collect(&mut self) {
        if let Some(i) = self.doc.test_request.take() {
            self.test_pick(&i.to_string());
        }
        if let Some(text) = self.doc.copy.take() {
            self.copy = Some(self.plain(&text));
        }
        if let Some(note) = self.doc.note.take() {
            self.message = Some(note);
        }
        self.redraw |= std::mem::take(&mut self.doc.redraw);
        let side = &mut self.ai.side;
        for pane in [&mut side.transcript, &mut side.input] {
            pane.note = None;
            self.redraw |= std::mem::take(&mut pane.redraw);
        }
        if let Some(text) = side.input.copy.take().or(side.transcript.copy.take()) {
            self.copy = Some(self.plain(&text));
        }
    }

    /// Copied text as other programs should see it. A notebook's cell
    /// headers become the `# %%` lines that scripts use for cells; a
    /// conversation's become the name of who is speaking.
    fn plain(&self, text: &str) -> String {
        if !text.contains(cells::MARK) {
            return text.to_string();
        }
        let line = |l: &str| match cells::parse(l) {
            Some((Tag::Code, _)) => "# %%".to_string(),
            Some((tag @ (Tag::Markdown | Tag::Raw), _)) => format!("# %% [{}]", tag.name()),
            Some((_, "")) => String::new(),
            Some((_, label)) => format!("{label}:"),
            None => l.to_string(),
        };
        text.split('\n').map(line).collect::<Vec<_>>().join("\n")
    }

    pub fn paste_text(&mut self, s: &str) {
        if self.paste_into_input(s) {
            return;
        }
        self.paste_document_text(s, false);
    }

    fn paste_document_text(&mut self, s: &str, plain: bool) {
        if let Some(left) = self.git_actions.compare_clipboard.take() {
            let rules = self.doc.path.clone().unwrap_or_else(|| PathBuf::from("selection"));
            self.git_action(git_actions::Action::TextDiff(rules, "Selection \u{2194} clipboard".into(), left, s.into()));
            return;
        }

        self.unsuggest();
        if self.search_panel_paste(s) {
        } else if let Some(p) = &mut self.prompt {
            let line = s.lines().next().unwrap_or("");
            let (text, cur) = p.field();
            text.insert_str(*cur, line);
            *cur += line.len();
            self.prompt_changed();
        } else if let Some(chooser) = &mut self.chooser {
            chooser.type_text(s.lines().next().unwrap_or("").trim());
        } else if let Some((MenuKind::Search, menu)) = &mut self.menu {
            menu.type_text(s.lines().next().unwrap_or(""));
            self.search_update();
        } else if self.source_paste(s) || self.settings_paste(s) || self.menu.is_some() || self.popup.is_some() {
        } else if let Some(t) =
            self.terms.get_mut(self.term_cur).filter(|_| self.focus == Focus::Term && self.term_open)
        {
            t.paste(s);
        } else if self.focus == Focus::Chat && self.side_is_open(Tab::Agent) {
            self.ai.side.input.paste_text(s);
        } else {
            self.comp = None;
            let (a, b) = self.doc.buf.sel();
            let linked = (!plain).then(|| writing::selection_link(self.doc.doc, &self.doc.buf.text[a..b], s)).flatten();
            self.doc.paste_text(linked.as_deref().unwrap_or(s));
            self.edited();
            self.docs_typed(None);
        }
        self.collect();
        self.redraw = true;
    }

    /// Keep unsaved work before closing; running terminal programs still ask.
    pub fn close(&mut self) {
        let running = self.terms.iter().any(|t| t.running() && !t.idle());
        self.unsuggest();
        if self.wait_mode && self.any_unsaved_tab() && !self.quit_armed {
            self.quit_armed = true;
            self.message = Some("Unsaved changes. Save before closing; closing again aborts the editor command.".into());
        } else if let Err(e) = self.recovery_save() {
            self.message = Some(format!("Could not keep unsaved work: {e}. Save your files before quitting."));
        } else if running && !self.quit_armed {
            self.quit_armed = true;
            self.message = Some("A program is running in the terminal. Closing again ends it".into());
        } else {
            self.ai.thread.save(&self.root);
            self.save_session();
            self.wait_cancelled = self.wait_mode && self.any_unsaved_tab();
            self.exit = true;
        }
        self.redraw = true;
    }

    // ---- completion -----------------------------------------------------

    /// The stretch of text completion looks at: the cell in a notebook, the
    /// whole file otherwise.
    fn scope(&self, off: usize) -> (usize, usize) {
        match self.doc.doc {
            Doc::Cells => self.doc.cell_body(self.doc.cell_at(off)),
            _ => (0, self.doc.buf.text.len()),
        }
    }

    /// Offers names for the word at the cursor. Unless `force`d, only where
    /// it would not be in the way.
    fn complete(&mut self, force: bool) {
        self.comp = None;
        let cur = self.doc.buf.cur;
        let wanted = force || self.cfg.autocomplete;
        if !wanted || !self.doc.extra_cursors.is_empty() || self.doc.buf.has_sel() || self.doc.read_only {
            return;
        }
        if self.doc.snippet.is_some() && !force {
            return;
        }
        if self.config_complete() { return; }
        if self.complete_citation() || self.complete_note() { return; }
        let snippet_query = py::query(&self.doc.buf.text, cur);
        let snippets = snippet_query.as_ref().map_or(Vec::new(), |q| self.snippet_items(&q.prefix));
        if self.language_complete(force) { return; }
        if self.doc.doc == Doc::Latex {
            self.complete_latex(force);
            if let Some(c) = &mut self.comp {
                c.items.splice(0..0, snippets);
            } else if let Some(query) = snippet_query.filter(|_| !snippets.is_empty()) {
                self.comp = Some(Completion { query, items: snippets, sel: 0, top: 0, seq: 0 });
            }
            return;
        }
        if !self.doc.python_at(cur) {
            if let Some(query) = snippet_query.filter(|_| !snippets.is_empty()) {
                self.comp = Some(Completion { query, items: snippets, sel: 0, top: 0, seq: 0 });
            }
            return;
        }
        // Words in comments and strings are prose, not names.
        if !force && cur > 0 && self.doc.attr_at(cur - 1) & (crate::md::DIM | crate::md::SOFT) != 0 {
            return;
        }
        let (s, e) = self.scope(cur);
        let text = &self.doc.buf.text;
        let Some(mut query) = py::query(&text[s..e], cur - s) else { return };
        query.start += s;
        let code: Vec<(usize, &str)> = match self.doc.doc {
            Doc::Cells => (0..self.doc.cells.len())
                .filter(|&i| self.doc.cells[i].tag == Tag::Code)
                .map(|i| self.doc.cell_body(i))
                .map(|(s, e)| (s, &text[s..e]))
                .collect(),
            _ => vec![(0, text.as_str())],
        };
        let mut items = snippets;
        items.extend(py::candidates(code.into_iter(), &query, 60));

        self.comp_seq += 1;
        let line = text[s..cur].matches('\n').count() + 1;
        let col = text[text[..cur].rfind('\n').map_or(0, |i| i + 1).max(s)..cur].chars().count();
        let (code, live, path, seq) = (text[s..e].to_string(), self.nb.is_some(), self.doc.path.clone(), self.comp_seq);
        if let Some(k) = self.python_kernel() {
            k.complete(seq, &code, line, col, live, path.as_deref());
        }
        self.comp = Some(Completion { query, items, sel: 0, top: 0, seq });
    }

    /// Offers commands, environments and labels in LaTeX. Everything is in
    /// the document or known already, so there is nothing to wait for.
    fn complete_latex(&mut self, force: bool) {
        let cur = self.doc.buf.cur;
        // Nothing is offered in a comment.
        if !force && cur > 0 && self.doc.attr_at(cur - 1) & crate::md::DIM != 0 {
            return;
        }
        let text = &self.doc.buf.text;
        let Some(query) = latex::query(text, cur, force) else { return };
        let items = latex::candidates(text, &query, 60);
        self.comp_seq += 1;
        self.comp = Some(Completion { query, items, sel: 0, top: 0, seq: self.comp_seq });
    }

    /// Adds the interpreter's answer to the names on offer.
    fn offer(&mut self, seq: u64, found: Vec<(String, String)>) {
        let Some(c) = self.comp.as_mut().filter(|c| c.seq == seq) else { return };
        let chosen = c.items.get(c.sel).filter(|_| c.sel > 0).map(|i| i.name.clone());
        let prefix = &c.query.prefix;
        let mut items: Vec<Candidate> = found
            .into_iter()
            .filter(|(name, _)| name.len() > prefix.len() && name.starts_with(prefix.as_str()))
            .map(|(name, kind)| Candidate { name, kind })
            .collect();
        for old in c.items.drain(..) {
            if !items.iter().any(|i| i.name == old.name) {
                items.push(old);
            }
        }
        items.truncate(60);
        c.sel = chosen.and_then(|n| items.iter().position(|i| i.name == n)).unwrap_or(0);
        c.items = items;
    }

    /// Keys that act on the list of completions while it shows.
    fn completion_key(&mut self, key: Key, mods: Mods) -> bool {
        let capture = matches!(key, Key::Tab | Key::Enter) && self.doc.vim_typing() && self.comp.is_some();
        let before = capture.then(|| (self.doc.buf.text.clone(), self.doc.buf.cur));
        let taken = self.completion_key_inner(key, mods);
        if let Some((before, cursor)) = before { self.doc.vim_completion(&before, cursor); }
        taken
    }

    fn completion_key_inner(&mut self, key: Key, mods: Mods) -> bool {
        let Some(c) = self.comp.as_mut().filter(|c| !c.items.is_empty()) else { return false };
        match key {
            Key::Up => c.sel = c.sel.checked_sub(1).unwrap_or(c.items.len() - 1),
            Key::Down => c.sel = (c.sel + 1) % c.items.len(),
            Key::Escape => self.comp = None,
            Key::Tab | Key::Enter if !mods.shift && !mods.ctrl && !mods.alt => {
                let (start, name, snippet) =
                    (c.query.start, c.items[c.sel].name.clone(), c.items[c.sel].kind == "snippet");
                let choice = c.items[c.sel].kind == "choice";
                self.comp = None;
                if !choice && self.config_completion_take(start, &name) { return true; }
                if !snippet && !choice && self.language_completion_take(start, &name) { return true; }
                if snippet && self.snippet_take(start, &name) {
                    return true;
                }
                let cur = self.doc.buf.cur;
                if self.doc.edit(start, cur, &name, EditKind::Other) {
                    self.doc.moved();
                }
                self.edited();
            }
            _ => return false,
        }
        true
    }

    /// The pointer over the list of completions chooses the row under it.
    fn completion_motion(&mut self, _x: f32, y: f32) {
        let (Some(c), Some((_, top, _, _))) = (self.comp.as_mut(), self.comp_box) else { return };
        let i = c.top + ((y - top) / self.comp_row.max(1.0)) as usize;
        if i < c.items.len() {
            c.sel = i;
        }
    }

    /// A click on the list of completions takes the one under it. Returns
    /// whether the click was there.
    fn completion_press(&mut self, x: f32, y: f32) -> bool {
        if !self.comp.as_ref().is_some_and(|c| !c.items.is_empty()) || !ui::inside(self.comp_box.unwrap(), Some((x, y)))
        {
            return false;
        }
        self.completion_motion(x, y);
        self.completion_key(Key::Tab, Mods::default());
        self.redraw = true;
        true
    }

    // ---- notebook -------------------------------------------------------

    /// Keys that act on notebook cells. Returns whether this was one.
    fn notebook_key(&mut self, key: Key, mods: Mods) -> bool {
        if self.nb.is_none() {
            if self.script_cells_here() && key == Key::Enter && (mods.ctrl || mods.shift) {
                self.script_command(if mods.shift { "next" } else { "run" });
                return true;
            }
            return false;
        }
        let at = self.doc.cell_at(self.doc.buf.cur);
        match key {
            Key::Enter if mods.ctrl && mods.shift => {
                for i in 0..self.doc.cells.len() {
                    self.run_cell(i);
                }
            }
            Key::Enter if mods.shift || mods.alt => {
                self.run_cell(at);
                if mods.alt || at + 1 == self.doc.cells.len() {
                    self.insert_cell(at, true, Tag::Code, "");
                } else {
                    let next = self.doc.cell_body(at + 1).0;
                    self.doc.select(next, next);
                }
            }
            Key::Enter if mods.ctrl => {
                self.run_cell(at);
            }
            Key::Char('a') if mods.ctrl && mods.shift => {
                self.insert_cell(at, false, Tag::Code, "");
            }
            Key::Char('b') if mods.ctrl && mods.shift => {
                self.insert_cell(at, true, Tag::Code, "");
            }
            Key::Char('m') if mods.ctrl && mods.shift => {
                let tag = if self.doc.cells[at].tag == Tag::Markdown { Tag::Code } else { Tag::Markdown };
                self.retag_cell(at, tag);
            }
            Key::Char('d') if mods.ctrl && mods.shift => {
                self.delete_cell(at);
            }
            Key::Char('.') if mods.ctrl => {
                if let Some(nb) = &mut self.nb {
                    nb.interrupt(&mut self.doc);
                    self.message = Some("Interrupted".into());
                }
            }
            Key::Char('r') if mods.ctrl && mods.shift => {
                if let Some(nb) = &mut self.nb {
                    nb.restart(&mut self.doc);
                    self.message = Some("Kernel restarted".into());
                }
            }
            Key::Char('o') if mods.ctrl && mods.shift => {
                if let Some(nb) = &mut self.nb {
                    nb.clear_outputs(&mut self.doc);
                    self.doc.buf.modified = true;
                }
            }
            _ => return false,
        }
        self.comp = None;
        self.edited();
        true
    }

    /// Runs a cell. Returns false if it is not code or cannot be run.
    pub fn run_cell(&mut self, index: usize) -> bool {
        let (python, dir) = self.python();
        let Some(nb) = &mut self.nb else { return false };
        nb.env_root = Some(self.root.clone());
        let ran = nb.run(&mut self.doc, index, &python, &dir, &self.waker);
        if ran {
            self.doc.buf.modified = true;
            self.message = nb.trouble.clone();
        }
        ran
    }

    /// Adds a cell after or before another and puts the cursor in it.
    pub fn insert_cell(&mut self, index: usize, below: bool, tag: Tag, source: &str) -> bool {
        let head = cells::header(tag, "");
        let source = source.replace(cells::MARK, "");
        let (at, text, cur) = if below {
            let at = self.doc.cell_body(index).1;
            (at, format!("\n{head}\n{source}"), at + head.len() + 2)
        } else {
            let at = self.doc.cell_span(index).0;
            (at, format!("{head}\n{source}\n"), at + head.len() + 1)
        };
        let done = self.doc.edit(at, at, &text, EditKind::Other);
        if done {
            self.doc.select(cur, cur);
        }
        done
    }

    /// Changes a cell's kind, keeping its text and the cursor.
    pub fn retag_cell(&mut self, index: usize, tag: Tag) -> bool {
        let cell = &self.doc.cells[index];
        if !cell.headed || cell.tag == tag {
            return cell.tag == tag;
        }
        let (s, e) = self.doc.buf.line(cell.line);
        let head = cells::header(tag, "");
        let moved = |o: usize| if o >= e { o + head.len() - (e - s) } else { o };
        let (cur, anchor) = (moved(self.doc.buf.cur), moved(self.doc.buf.anchor));
        let done = self.doc.edit(s, e, &head, EditKind::Other);
        if done {
            self.doc.select(anchor, cur);
        }
        done
    }

    pub fn delete_cell(&mut self, index: usize) -> bool {
        if self.doc.cells.len() == 1 {
            let (s, e) = self.doc.cell_body(index);
            return s == e || self.doc.edit(s, e, "", EditKind::Other);
        }
        let (mut s, e) = self.doc.cell_span(index);
        // The last cell takes the line break before it along.
        if index + 1 == self.doc.cells.len() {
            s -= 1;
        }
        let done = self.doc.edit(s, e, "", EditKind::Other);
        self.doc.moved();
        done
    }

    // ---- files ----------------------------------------------------------

    fn save(&mut self) {
        if self.doc.multi.is_some() {
            return self.multibuffer_save();
        }
        self.pin_edited_tab();
        self.git_stale();
        let Some(path) = self.doc.path.clone() else { return self.ask(PromptKind::SaveAs) };
        if self.doc.read_only {
            self.message = Some("This file is read-only".into());
            return;
        }
        self.unsuggest();
        if !self.disk_safe() || !self.prepare_save() {
            return;
        }
        let contents = self.contents();
        let previous = std::fs::metadata(&path).ok().filter(|m| m.len() <= 2 << 20)
            .and_then(|_| std::fs::read_to_string(&path).ok());
        match write_file(&path, contents.as_bytes()) {
            Ok(()) => {
                self.doc.buf.modified = false;
                self.disk_remember();
                self.message = Some("Saved".into());
                if self.doc.doc == Doc::Markdown && let Some(before) = &previous {
                    self.offer_heading_link_updates(&path, before, &contents);
                }
                if contents.len() <= 2 << 20 {
                    self.git_actions.history.push(local_history::record(path.clone(), previous, contents, self.waker.clone()));
                }
                self.tree.refresh();
                self.typeset_saved();
                self.watch_saved(&path);
                self.remote_saved(&path);
                if path == keybindings::path() { self.reload_keymap(); }
            }
            Err(e) => self.message = Some(format!("Cannot save: {e}")),
        }
        self.retitle = true;
    }

    /// The document as it is written to a file.
    fn contents(&self) -> String {
        match &self.nb {
            Some(nb) => nb.to_json(&self.doc),
            None => self.doc.buf.eol.apply(&self.doc.buf.text),
        }
    }

    /// Makes `path` the document's, as after saving it there or moving it.
    fn set_path(&mut self, path: PathBuf) {
        // A notebook stays one whatever it is called; other text is read
        // anew by its new name.
        let doc = doc_for(Some(&path));
        self.doc.path = Some(path.clone());
        if self.nb.is_none() && doc != Doc::Cells {
            self.doc.set_doc(doc);
            self.helper = None;
        }
        self.root = self.root_for(Some(&path));
        if let Err(e) = self.cfg.ensure_project(&self.root) { self.message = Some(e); }
        if let Some(reader) = &mut self.reader {
            reader.pages.path = path.clone();
        }
        self.doc.path = Some(path);
        self.disk_remember();
        self.retitle = true;
    }

    /// Opens a file in this window's preview tab, keeping edited tabs.
    fn open(&mut self, path: PathBuf) {
        self.open_tab(path);
    }

    /// Opens a folder as the project, the same way.
    fn open_folder(&mut self, dir: PathBuf) {
        self.remember_recent(&dir);
        if self.doc.buf.modified || self.doc.path.is_some() {
            spawn_self(Some(&dir));
            let name = dir.file_name().unwrap_or(dir.as_os_str()).to_string_lossy();
            self.message = Some(format!("{name} opened in a new window"));
        } else {
            self.show_folder(dir);
        }
    }

    /// Where a name typed for a file leads: from the file's folder, or
    /// without a file from the window's folder, never from wherever the
    /// program happened to be started.
    fn resolve(&self, input: &str) -> PathBuf {
        match &self.doc.path {
            Some(path) => pane::resolve(Some(path), input),
            None => pane::resolve(Some(&self.root.join("_")), input),
        }
    }

    /// The folder new files go in: the file's own, or else the window's.
    fn here(&self) -> PathBuf {
        let dir = self.doc.path.as_deref().and_then(Path::parent).filter(|d| !d.as_os_str().is_empty());
        dir.unwrap_or(&self.root).to_path_buf()
    }

    // ---- prompt ---------------------------------------------------------

    fn ask(&mut self, kind: PromptKind) {
        self.doc.vim.searching = None;
        if matches!(kind, PromptKind::Find | PromptKind::Replace) && !self.prompt.as_ref().is_some_and(Prompt::finds) {
            self.remember_jump();
        }
        self.comp = None;
        // A find on show takes the keys back, keeping what it finds unless
        // a word is selected to find instead.
        if matches!(kind, PromptKind::Find | PromptKind::Replace)
            && self.prompt.as_ref().is_some_and(Prompt::finds)
            && !(kind == PromptKind::Replace && (self.reader.is_some() || self.doc.read_only))
        {
            let (a, b) = self.doc.buf.sel();
            let picked = &self.doc.buf.text[a..b];
            let picked =
                (self.reader.is_none() && !picked.is_empty() && !picked.contains('\n')).then(|| picked.to_string());
            let p = self.prompt.as_mut().unwrap();
            if let Some(text) = picked {
                (p.cur, p.text) = (text.len(), text);
            }
            (p.kind, p.away, p.on_with) = (kind, false, false);
            return self.prompt_changed();
        }
        if kind == PromptKind::Find && self.reader.as_ref().is_some_and(Reader::picture) {
            self.message = Some("A picture has no text to find".into());
            return;
        }
        if kind == PromptKind::Replace && (self.reader.is_some() || self.doc.read_only) {
            self.message = Some("Nothing can be replaced in this file".into());
            return;
        }
        let text = match kind {
            PromptKind::Find | PromptKind::Replace => {
                let (a, b) = self.doc.buf.sel();
                let s = match &self.reader {
                    Some(reader) => reader.selected().unwrap_or_default(),
                    None => self.doc.buf.text[a..b].to_string(),
                };
                if s.contains('\n') { String::new() } else { s }
            }
            PromptKind::SaveAs | PromptKind::Rename => match &self.doc.path {
                Some(path) => path.to_string_lossy().into_owned(),
                None => format!("{}/", self.here().to_string_lossy().trim_end_matches('/')),
            },
            PromptKind::New | PromptKind::NewFolder | PromptKind::ThemeImport => {
                format!("{}/", self.here().to_string_lossy().trim_end_matches('/'))
            }
            PromptKind::TerminalFind => self.term().map_or(String::new(), |t| t.search.clone()),
            PromptKind::Copy => self.copy_name(),
            PromptKind::Export => self.export_name("pdf"),
            PromptKind::ExportTex => self.export_name("tex"),
            PromptKind::ExportDocument => self.export_name(&self.document_exports.format),
            PromptKind::Trash
            | PromptKind::Reload
            | PromptKind::GitBranch
            | PromptKind::GitCommit
            | PromptKind::GitStashMessage
            | PromptKind::GitCompareRevisions
            | PromptKind::GithubReviewLine
            | PromptKind::GitLogSearch
            | PromptKind::GitAmend
            | PromptKind::GitRebaseReword
            | PromptKind::GithubReply
            | PromptKind::GithubReviewBody
            | PromptKind::DebugInput
            | PromptKind::LspRename
            | PromptKind::InlineEdit
            | PromptKind::AgentWorktree
            | PromptKind::ProjectReplace
            | PromptKind::SearchInclude
            | PromptKind::SearchExclude => String::new(),
        };
        // A name is changed more often than its extension.
        let cur = match kind {
            PromptKind::Rename | PromptKind::Copy | PromptKind::Export | PromptKind::ExportTex | PromptKind::ExportDocument => {
                Path::new(&text).extension().map_or(text.len(), |e| text.len() - e.len() - 1)
            }
            _ => text.len(),
        };
        let origin = self.doc.buf.sel().0;
        let (with, with_cur, on_with) = (String::new(), 0, false);
        self.prompt = Some(Prompt { kind, cur, text, origin, subject: None, with, with_cur, on_with, away: false });
    }

    fn prompt_key(&mut self, key: Key, mods: Mods, text: Option<&str>) {
        let Some(p) = &mut self.prompt else { return };
        if !p.finds() {
            self.message = None;
        }
        let kind = p.kind;
        match key {
            Key::Escape => self.prompt = None,
            Key::Enter if kind == PromptKind::Replace => self.replace_key(mods),
            Key::Enter if kind == PromptKind::Find && self.doc.vim.searching.is_some() => {
                let back = self.doc.vim.searching.take() == Some(true);
                self.doc.vim.search_back = back;
                let origin = p.origin;
                self.find(if back { origin } else { editor::next(&self.doc.buf.text, origin) }, !back);
                self.vim_found();
            }
            Key::Enter => self.submit(mods.shift),
            _ if matches!(kind, PromptKind::Trash | PromptKind::Reload) => {}
            Key::Tab if kind == PromptKind::Replace => p.on_with = !p.on_with,
            Key::Tab
                if !matches!(
                    kind,
                    PromptKind::Find
                        | PromptKind::TerminalFind
                        | PromptKind::GitBranch
                        | PromptKind::GitCommit
                        | PromptKind::LspRename
                        | PromptKind::InlineEdit
                        | PromptKind::AgentWorktree
                        | PromptKind::ProjectReplace
                        | PromptKind::SearchInclude
                        | PromptKind::SearchExclude
                ) =>
            {
                let input = p.text.clone();
                if let Some(done) = self.complete_path(&input) {
                    let p = self.prompt.as_mut().unwrap();
                    p.cur = done.len();
                    p.text = done;
                }
            }
            Key::Char('v') if mods.ctrl => { self.paste = true; self.paste_plain = mods.shift; },
            _ => {
                let (text_, cur) = p.field();
                let edited = match key {
                    Key::Left => {
                        *cur = crate::bidi::visual_move(text_, *cur, false);
                        false
                    }
                    Key::Right => {
                        *cur = crate::bidi::visual_move(text_, *cur, true);
                        false
                    }
                    Key::Home => {
                        *cur = 0;
                        false
                    }
                    Key::End => {
                        *cur = text_.len();
                        false
                    }
                    Key::Backspace | Key::Delete => {
                        let (a, b) = match (key == Key::Delete, mods.ctrl) {
                            (true, _) => (*cur, editor::next(text_, *cur)),
                            (false, true) => (editor::word_left(text_, *cur), *cur),
                            (false, false) => (editor::prev(text_, *cur), *cur),
                        };
                        text_.replace_range(a..b, "");
                        *cur = a;
                        true
                    }
                    _ => match text.filter(|s| !mods.ctrl && !mods.alt && !s.chars().any(char::is_control)) {
                        Some(s) => {
                            text_.insert_str(*cur, s);
                            *cur += s.len();
                            true
                        }
                        None => false,
                    },
                };
                if edited {
                    self.prompt_changed();
                }
            }
        }
        self.collect();
    }

    fn prompt_changed(&mut self) {
        if let Some(Prompt { kind: PromptKind::Find | PromptKind::Replace, origin, on_with: false, .. }) = self.prompt {
            match self.reader.is_some() {
                true => self.find_in_pages(Seek::Here),
                false => self.find(origin, true),
            }
        }
    }

    fn submit(&mut self, back: bool) {
        let Some(p) = &self.prompt else { return };
        let input = if matches!(p.kind, PromptKind::ProjectReplace | PromptKind::TerminalFind) {
            p.text.clone()
        } else {
            p.text.trim().to_string()
        };
        match p.kind {
            PromptKind::TerminalFind => {
                self.prompt = None;
                if let Some(t) = self.term_mut() {
                    let (at, total) = t.find_history(&input, back);
                    self.message = Some(format!("Terminal match {at} of {total}"));
                }
                self.focus = Focus::Term;
            }
            PromptKind::Reload => self.disk_decide(back),
            PromptKind::Find if self.reader.is_some() => self.find_in_pages(if back { Seek::Back } else { Seek::Next }),
            PromptKind::Find | PromptKind::Replace => {
                let a = self.doc.buf.sel().0;
                self.find(if back { a } else { editor::next(&self.doc.buf.text, a) }, !back);
            }
            PromptKind::Trash => match p.subject.clone() {
                Some(path) => self.discard_path(&path, back),
                None => self.discard(back),
            },
            _ if input.is_empty()
                && !matches!(
                    p.kind,
                    PromptKind::ProjectReplace
                        | PromptKind::DebugInput
                        | PromptKind::TerminalFind
                        | PromptKind::SearchInclude
                        | PromptKind::SearchExclude
                ) =>
            {
                self.prompt = None
            }
            // A PDF has no text to write: it is copied, and the copy shown.
            PromptKind::SaveAs if self.reader.is_some() => {
                let path = self.target(&input);
                self.copy_to(&input);
                if self.prompt.is_none() {
                    let said = self.message.take();
                    self.load(Some(path));
                    self.message = said;
                }
            }
            PromptKind::SaveAs => {
                let path = self.resolve(&input);
                self.prompt = None;
                self.set_path(path);
                self.doc.read_only = false;
                self.save();
            }
            PromptKind::New => self.create(&input),
            PromptKind::NewFolder => self.create(&format!("{}/", input.trim_end_matches('/'))),
            PromptKind::Rename => match p.subject.clone() {
                Some(path) => self.rename_path(&path, &input),
                None => self.rename(&input),
            },
            PromptKind::Copy => self.copy_to(&input),
            PromptKind::Export => self.export_pdf(&input),
            PromptKind::ExportTex => self.export_tex(&input),
            PromptKind::ExportDocument => self.export_document(&input),
            PromptKind::SearchInclude | PromptKind::SearchExclude => {
                if p.kind == PromptKind::SearchInclude {
                    self.search_options.include = input;
                } else {
                    self.search_options.exclude = input;
                }
                self.prompt = None;
                self.open_search();
            }
            PromptKind::DebugInput => self.debug_input(&input),
            PromptKind::InlineEdit => self.inline_request(&input),
            PromptKind::AgentWorktree => { self.prompt = None; self.new_agent_worktree(&input); },
            PromptKind::ProjectReplace => {
                self.prompt = None;
                self.preview_project_replace(&input);
            }
            PromptKind::LspRename => self.language_rename(&input),
            PromptKind::ThemeImport => self.import_theme(&input),
            PromptKind::GithubReviewLine => self.add_pr_review_line(&input),
            PromptKind::GitRebaseReword => self.rebase_reworded(&input),
            PromptKind::GithubReply | PromptKind::GithubReviewBody => self.github_prompt(p.kind, &input),
            PromptKind::GitStashMessage | PromptKind::GitCompareRevisions | PromptKind::GitBranch | PromptKind::GitCommit | PromptKind::GitLogSearch | PromptKind::GitAmend => self.git_submit(p.kind, &input),
        }
    }

    /// Completes a partial path to the longest unambiguous prefix.
    fn complete_path(&self, input: &str) -> Option<String> {
        let (dir, stem) = input.rsplit_once('/').map_or(("", input), |(d, s)| (d, s));
        let base = if input.contains('/') { self.resolve(&format!("{dir}/")) } else { self.resolve(".") };
        let mut names: Vec<String> = std::fs::read_dir(base)
            .ok()?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let is_dir = e.file_type().ok()?.is_dir();
                name.starts_with(stem).then(|| if is_dir { name + "/" } else { name })
            })
            .collect();
        names.sort();
        let mut common = names.first()?.clone();
        for n in &names {
            let same = common.chars().zip(n.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
            common.truncate(same);
        }
        Some(if input.contains('/') { format!("{dir}/{common}") } else { common })
    }

    /// Selects the next match at or after `from` (or the previous one before
    /// it), wrapping around. Case matters only if the query has capitals.
    /// Finds what the prompt holds in the pages of a PDF.
    fn find_in_pages(&mut self, seek: Seek) {
        let (Some(p), Some(reader)) = (&self.prompt, &mut self.reader) else { return };
        if p.text.is_empty() || reader.find(&p.text, seek) {
            self.message = None;
        } else {
            self.message = Some("No match".into());
        }
    }

    /// Whether Vim mode in the document takes a key before the window's
    /// shortcuts do.
    fn vim_takes(&self, key: Key, mods: Mods) -> bool {
        self.focus == Focus::Doc && self.reader.is_none() && self.doc.vim_takes(key, mods)
    }

    /// Carries out what Vim mode asked of the window: searching with the
    /// find, and the Ex commands that write, close and open files.
    fn vim_request(&mut self) {
        if !self.doc.vim_typing() {
            self.comp = None;
            self.suggest_due = None;
        }
        let Some(request) = self.doc.vim.request.take() else { return };
        match request {
            pane::vim::Request::Command(id) => self.run_command(id),
            pane::vim::Request::Search(back) => {
                self.ask(PromptKind::Find);
                if let Some(p) = &mut self.prompt {
                    (p.text, p.cur) = (String::new(), 0);
                }
                self.find_marks();
                self.doc.vim.searching = Some(back);
            }
            pane::vim::Request::Word(word, back) => {
                self.ask(PromptKind::Find);
                if let Some(p) = &mut self.prompt {
                    (p.cur, p.text, p.away) = (word.len(), word, true);
                }
                self.doc.vim.search_back = back;
                let (a, b) = self.doc.buf.sel();
                self.find(if back { a } else { b.max(editor::next(&self.doc.buf.text, a)) }, !back);
                self.vim_found();
            }
            pane::vim::Request::Next(reverse) => {
                if !self.prompt.as_ref().is_some_and(Prompt::finds) {
                    self.message = Some("No search to repeat".into());
                    return;
                }
                self.find_step(reverse != self.doc.vim.search_back);
                self.vim_found();
            }
            pane::vim::Request::Paste => {
                self.paste = true;
                self.paste_plain = true;
            }
            pane::vim::Request::Ex(command) => {
                let (name, arg) = command.split_once(' ').map_or((command.as_str(), ""), |(a, b)| (a, b.trim()));
                match name {
                    "w" | "write" if arg.is_empty() => self.save(),
                    "w" | "write" => {
                        self.message = Some("Use :w without a name; Ctrl+Shift+S saves under another".into())
                    }
                    "q" | "quit" | "clo" | "close" | "buffer-close" | "bc" => self.close_file_tab(self.active_file),
                    "q!" | "quit!" => {
                        self.tab_close_armed = Some(self.active_file);
                        self.close_file_tab(self.active_file);
                    }
                    "wq" | "x" | "xit" => {
                        self.save();
                        if !self.doc.buf.modified {
                            self.close_file_tab(self.active_file);
                        }
                    }
                    "qa" | "qall" | "qa!" | "qall!" => self.close(),
                    "e" | "edit" | "o" | "open" if !arg.is_empty() => {
                        let path = self.resolve(arg);
                        self.open(path);
                    }
                    "format" | "fmt" => self.run_command("lsp_format"),
                    "o" | "open" => self.run_command("open"),
                    "bn" | "buffer-next" => self.run_command("next_tab"),
                    "bp" | "buffer-previous" => self.run_command("previous_tab"),
                    "noh" | "nohlsearch" => self.prompt = None,
                    _ => self.message = Some(format!("Not a Vim command here: :{command}")),
                }
            }
        }
        self.find_marks();
        self.doc.moved();
    }

    /// A search leaves the cursor at the start of the match, not selecting
    /// it, and gives the keys back to the document.
    fn vim_found(&mut self) {
        let a = self.doc.buf.sel().0;
        self.doc.select(a, a);
        if let Some(p) = &mut self.prompt {
            p.away = true;
        }
    }

    fn find(&mut self, from: usize, forward: bool) {
        let Some(p) = &self.prompt else { return };
        if p.text.is_empty() {
            return;
        }
        let found = match self.scoped_matches(&p.text) {
            Ok(v) => v,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        let hit = if forward {
            found.iter().find(|m| m.0 >= from).or(found.first())
        } else {
            found.iter().rev().find(|m| m.0 < from).or(found.last())
        };
        if let Some(&(a, b)) = hit {
            self.doc.select(a, b);
            self.message = None;
        } else {
            self.message = Some("No match".into());
        }
    }

    // ---- pointer --------------------------------------------------------

    pub fn press(&mut self, x: f32, y: f32, mods: Mods, clicks: u32) {
        self.key_bindings.map.cancel();
        if !self.modal() && self.file_blame_click(x, y) { return; }
        self.redraw = true;
        self.pointer = (x, y);
        self.tip_hover = None;
        self.debugger.tree.focus = false;
        // The controls the window draws itself come first; while something
        // stands over the window, only the dialog's.
        if self.input_press(x,y) { return; }
        let overlaid = self.overlaid();
        if let Some(hit) = self.hit_at(x, y).filter(|h| !overlaid || matches!(h, Hit::Dialog(_))) {
            if !matches!(hit, Hit::Find(_) | Hit::Dialog(_)) {
                if let Some(p) = self.prompt.as_mut().filter(|p| p.finds()) {
                    p.away = true;
                }
                self.comp = None;
                self.unsuggest();
            }
            self.do_hit(hit);
            return self.collect();
        }
        if !overlaid && (self.completion_press(x, y) || self.docs_press(x, y)) {
            return;
        }
        // A dialog goes when clicked outside of; the find stays, giving
        // the keys back to what is clicked.
        if let Some(r) = self.dialog_rect() {
            if !ui::inside(r, Some((x, y))) {
                self.prompt = None;
            }
            return;
        }
        if self.prompt.as_ref().is_some_and(Prompt::finds) {
            if self.find_rect().is_some_and(|r| ui::inside(r, Some((x, y)))) {
                return;
            }
            if let Some(p) = &mut self.prompt {
                p.away = true;
            }
        }
        self.search.panel.keys = false;
        self.comp = None;
        self.unsuggest();
        if !self.tests.contains(x, y) {
            self.tests.typing = false;
        }
        if !self.overlaid() {
            self.focus_split_at(x, y);
            if let Some(line) = self.doc.git_under(x, y) {
                self.git_hunk_show(line, mods.shift);
                return;
            }
            self.term_aim(x, y);
        }
        if self.rebinding.take().is_some() {
            self.open_menu(MenuKind::Keys);
        } else if let Some((target, popup)) = &mut self.popup {
            match popup.press(x, y) {
                Picked::Key(k) => {
                    let target = target.clone();
                    self.popup = None;
                    self.context_pick(target, &k);
                }
                Picked::Close => self.popup = None,
                _ => {}
            }
        } else if let Some((kind, menu)) = &mut self.menu {
            let (kind, picked) = (*kind, menu.press(x, y));
            match picked {
                Picked::Key(k) => {
                    self.menu = None;
                    self.menu_pick(kind, k);
                }
                Picked::Close => self.menu = None,
                _ => {}
            }
        } else if let Some(chooser) = &mut self.chooser {
            let chosen = chooser.press(x, y, mods.ctrl);
            self.chosen(chosen);
        } else if self.settings.is_some() {
            self.settings_press(x, y);
        } else if y < self.title_h() || y >= self.body_bottom() {
        } else if let Some((index, close)) = self.file_tab_hit(x, y) {
            if close {
                self.close_file_tab(index);
            } else {
                self.press_tab(index, x, y);
            }
        } else if self.view_divider_press(x, y, clicks) {
        } else if self.on_divider(x, y) {
            self.resizing = self.divider_at(x, y);
        } else if self.term_open && self.term().is_some_and(|t| t.on_edge(x, y)) {
            self.term_resizing = true;
        } else if mods.ctrl && self.term_open && self.term().is_some_and(|t| t.on_text(x, y)) && self.term_link(x, y) {
        } else if let Some(t) = self.terms.get_mut(self.term_cur).filter(|t| self.term_open && t.contains(x, y)) {
            self.focus = Focus::Term;
            t.press(x, y, mods, clicks, 0);
        } else if self.source_shown() && self.tree.contains(x, y) {
            self.source_click();
        } else if self.tree.contains(x, y) {
            self.focus = Focus::Files;
            if let Some(path) = self.tree.press(x, y) {
                self.pick(path, mods.ctrl);
            }
        } else if self.typeset.view.contains(x, y) {
            self.focus = Focus::Doc;
            if mods.ctrl && self.typeset.view.hit(x, y).is_none() && self.sync_inverse(x, y) {
            } else if let Some(hit) = self.typeset.view.hit(x, y) {
                self.preview_hit(hit);
            }
        } else if self.tests.contains(x,y) {
            self.tests_press(x,y);
        } else if self.problems.contains(x,y) {
            self.problems_press(x,y);
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Debug && self.debug_tree_press(x, y) {
        } else if self.outline.contains(x, y) {
            if let Some(at) = self.outline.press(x, y) {
                if self.tab == Tab::Hierarchy {
                    self.hierarchy_pick(at);
                } else if self.tab == Tab::Backlinks {
                    self.backlink_pick(at);
                } else if let Some(reader) = self.reader.as_mut() {
                    reader.go_bookmark(at);
                } else {
                    self.go_to(at);
                }
            }
        } else if self.ai.side.contains(x, y) {
            self.focus = Focus::Chat;
            let side = &mut self.ai.side;
            if let Some(hit) = side.hit(x, y) {
                self.chat_hit(hit);
            } else if side.input.contains(x, y) {
                side.transcript.select(0, 0);
                side.input.press(x, y, mods, clicks);
            } else if side.transcript.contains(x, y) {
                if let Some(url) = side.transcript.press(x, y, mods, clicks) {
                    self.follow(&url);
                }
            }
        } else if let Some(reader) = self.reader.as_mut().filter(|r| r.contains(x, y)) {
            self.focus = Focus::Doc;
            reader.press(x, y, mods, clicks);
        } else if self.multibuffer_press(x, y) {
        } else if self.notebook_press(x, y) {
        } else if self.doc.contains(x, y) && self.welcomes() {
            self.focus = Focus::Doc;
        } else if self.doc.contains(x, y) {
            self.focus = Focus::Doc;
            if let Some(url) = self.doc.press(x, y, mods, clicks) {
                self.follow(&url);
            }
            self.edited();
        }
        self.collect();
    }

    pub fn motion(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
        if let Some(tab) = self.resizing {
            if tab == Tab::Files {
                self.cfg.files_width = (x / self.scale).clamp(160.0, 1200.0);
            } else {
                self.cfg.sidebar_width = ((self.w - x) / self.scale).clamp(240.0, 1200.0);
            }
            self.arrange();
            self.redraw = true;
        }
        if self.term_resizing {
            self.cfg.terminal_height = ((self.body_bottom() - y) / self.scale).clamp(80.0, 2000.0);
            self.arrange();
            self.redraw = true;
        }
        self.view_divider_motion(x, y);
        self.hover(x, y);
        for i in self.shown_terms() {
            if self.terms[i].motion(x, y, self.mods) {
                self.redraw = true;
            }
        }
        self.doc.motion(x, y);
        if self.reader.as_mut().is_some_and(|r| r.motion(x, y)) {
            self.redraw = true;
        }
        self.ai.side.transcript.motion(x, y);
        self.ai.side.input.motion(x, y);
        self.docs_motion(x, y);
        self.collect();
    }

    pub fn release(&mut self) {
        self.drop_tab_in_view();
        if self.resizing.take().is_some() | std::mem::take(&mut self.term_resizing) {
            let _ = self.cfg.save();
        }
        let selecting = self.doc.selecting()
            || self.ai.side.transcript.selecting()
            || self.ai.side.input.selecting()
            || self.reader.as_ref().is_some_and(Reader::dragging)
            || self.terms.iter().any(Term::dragging);
        for t in &mut self.terms {
            t.release(0, self.mods);
        }
        self.doc.release();
        if let Some(address) = self.reader.as_mut().and_then(Reader::release) {
            // Only web and mail addresses get this far. Tests open nothing.
            let opened = cfg!(test) || std::process::Command::new("xdg-open").arg(&address).spawn().is_ok();
            self.message = Some(if opened { format!("Opened {address}") } else { "Cannot run xdg-open".into() });
        }
        self.ai.side.transcript.release();
        self.ai.side.input.release();
        if selecting {
            self.offer_primary();
        }
        self.collect();
    }

    fn follow(&mut self, url: &str) {
        if let Some(note) = url.strip_prefix("lightdown-note:") { self.follow_note(note); return; }
        if !url.contains("://") && !url.starts_with("mailto:") {
            let (file, fragment) = url.split_once('#').map_or((url, None), |(f, a)| (f, Some(a)));
            let path =
                if file.is_empty() { self.doc.path.clone() } else { Some(self.resolve(&pane::percent_decode(file))) };
            if let Some(path) = path.filter(|p| p.is_file()) {
                self.jump_to(path, 0, 0);
                if self.reader.is_none()
                    && let Some(fragment) = fragment
                {
                    let at = if let Some(line) = fragment
                        .strip_prefix('L')
                        .and_then(|l| l.split('-').next())
                        .and_then(|l| l.parse::<usize>().ok())
                    {
                        self.doc.buf.starts[line.saturating_sub(1).min(self.doc.buf.lines() - 1)]
                    } else {
                        let slug = |s: &str| {
                            s.to_lowercase()
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join("-")
                                .chars()
                                .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
                                .collect::<String>()
                        };
                        crate::outline::items(&self.doc.buf.text, self.doc.doc)
                            .iter()
                            .find(|i| slug(&i.title) == pane::percent_decode(fragment))
                            .map_or(0, |i| i.at)
                    };
                    self.doc.select(at, at);
                    self.doc.moved();
                }
                return;
            }
        }
        if std::process::Command::new("xdg-open").arg(url).spawn().is_err() {
            self.message = Some("Cannot run xdg-open".into());
        }
    }

    // ---- painting -------------------------------------------------------

    pub fn draw(&mut self, c: &mut Canvas) {
        ui::clear_input_caret();
        if self.w < 1.0 || self.h < 1.0 {
            c.clear(theme::bg());
            return;
        }
        self.looks_preview();
        self.pics.borrow_mut().poll();
        self.multibuffer_tick();
        self.arrange();
        self.hits.clear();
        ui::clear_tips();
        self.draw_frame(c);
        let overlaid = self.overlaid();
        let welcome = self.welcomes();
        self.doc.focused = !welcome && !self.prompt_has_keys() && !overlaid && self.doc_has_keys();
        self.spell_marks();
        self.grammar_marks();
        self.debug_marks();
        self.find_marks();
        self.config_refresh();
        self.doc.maths = (self.cfg.math && self.reader.is_none()).then(|| self.maths.clone());
        {
            let mut maths = self.maths.borrow_mut();
            if maths.engine != self.cfg.latex_for(&self.root) {
                maths.engine = self.cfg.latex_for(&self.root).into();
                maths.retry();
            }
        }
        if welcome {
            self.doc.visible.clear();
            self.draw_recent(c);
        } else {
            self.semantic_apply();
            match &mut self.reader {
                Some(reader) => reader.draw(c),
                None => self.doc.draw(c),
            }
            self.draw_split(c);
        }
        self.maths.borrow_mut().flush();
        // Lines that came into view have their words asked about now.
        self.spell_marks();
        self.grammar_marks();
        self.draw_notebook_controls(c);
        self.draw_git_conflicts(c);
        self.draw_merge_buttons(c);
        self.draw_git_hunk(c);
        if self.term_open && !self.focus_mode {
            let focused = self.focus == Focus::Term && !self.prompt_has_keys() && !overlaid;
            for i in self.shown_terms() {
                let t = &mut self.terms[i];
                t.set_focus(focused && i == self.term_cur);
                t.draw(c);
            }
            self.term_chrome(c);
        }
        if self.files_open && !self.focus_mode {
            self.tree.show_roots(&self.roots(), self.doc.path.as_deref());
            if self.cfg.source_control {
                self.draw_source_control(c);
            } else {
                self.tree.draw(c, self.focus == Focus::Files && !self.prompt_has_keys() && !overlaid);
            }
        }
        if !self.focus_mode && self.side_open && self.tab == Tab::Preview {
            let (busy, nothing) = (self.typeset.busy(), self.unbuildable());
            self.typeset.view.draw(c, busy, nothing);
            if std::mem::take(&mut self.typeset.view.again) {
                self.redraw = true;
            }
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Search {
            self.draw_search_panel(c);
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Problems {
            self.draw_problems(c);
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Tests {
            self.draw_tests(c);
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Debug {
            self.debugger.view.draw(c);
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Hierarchy {
            self.hierarchy_rows();
            self.outline.draw(c);
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Backlinks {
            self.outline.draw(c);
        } else if !self.focus_mode && self.side_open && self.tab == Tab::Outline {
            match self.reader.as_mut() {
                Some(reader) => {
                    let path = reader.pages.path.clone();
                    let current = reader.current_bookmark();
                    self.outline.show_bookmarks(&path, reader.bookmarks(), current);
                }
                None => {
                    self.outline.update(&self.doc.buf.text, self.doc.doc, self.doc.buf.cur);
                    if let Some(items) = self.language_symbols() {
                        self.outline.current = items.iter().rposition(|i| i.at <= self.doc.buf.cur);
                        self.outline.items = items;
                    }
                }
            }
            self.outline.draw(c);
        } else if !self.focus_mode && self.side_open {
            let view = self.side_view();
            self.ai.side.draw(c, &view);
        }
        if !self.focus_mode {
            self.draw_file_tabs(c);
            self.explorer_head(c);
            self.panel_head(c);
            self.title_bar(c);
            self.status_bar(c);
        } else { self.file_tab_hits.clear(); self.kind_hit = None; }
        self.draw_find(c);
        self.draw_language_inlays(c);
        self.draw_lightbulb(c);
        self.draw_peek(c);
        self.draw_completion(c);
        self.draw_docs(c);
        if self.modal() {
            // What lies under a dialog dims, as under T3 Code's.
            c.shade(0.0, 0.0, self.w, self.h, 0x000000, theme::scrim());
        }
        self.draw_dialog(c);
        self.draw_settings(c);
        if let Some(chooser) = &mut self.chooser {
            chooser.draw(c, &mut self.fonts.borrow_mut(), self.w, self.h, self.scale);
        }
        if let Some((_, menu)) = &mut self.menu {
            menu.draw(c, &mut self.fonts.borrow_mut(), self.w, self.h, self.scale);
        }
        if let Some((_, popup)) = &mut self.popup {
            popup.draw(c, &mut self.fonts.borrow_mut(), self.w, self.h, self.scale);
        }
        self.draw_preedit(c);
        self.draw_tooltip(c);
    }

    /// The list of completions, under the caret or above it: each name
    /// with a badge for what kind of thing it is, the part typed lit.
    fn draw_completion(&mut self, c: &mut Canvas) {
        self.comp_box = None;
        let bottom = self.body_bottom() - self.term_height();
        let Some(comp) = self.comp.as_mut().filter(|comp| !comp.items.is_empty()) else { return };
        let Some((x, y, h)) = self.doc.caret() else { return };
        let s = self.scale;
        let fonts = &mut self.fonts.borrow_mut();
        let mono = fonts.mono();
        let size = (self.doc.px() * theme::mono_scale()).round().max(px(13.0, s));
        let small = px(theme::SMALL, s);
        let row_h = (size * 1.65).round().max(px(24.0, s));
        let shown = comp.items.len().min(10);
        // The list keeps still while the choice moves within it.
        let top = comp.top.min(comp.sel).max(comp.sel.saturating_sub(shown - 1)).min(comp.items.len() - shown);
        comp.top = top;
        let items = &comp.items[top..top + shown];
        let badge = px(16.0, s);
        let mut widest = 0.0f32;
        for i in items {
            widest = widest.max(fonts.width(mono, size, &i.name) + fonts.width(font::UI, small, &i.kind));
        }
        let inset = px(4.0, s);
        let w = (widest + badge + px(44.0, s)).clamp(px(220.0, s), px(520.0, s)).round();
        let ph = row_h * shown as f32 + inset * 2.0;
        let bx = (x - badge - px(14.0, s)).clamp(0.0, (self.w - w).max(0.0)).round();
        let by = if y + h + ph > bottom { y - ph - px(2.0, s) } else { y + h + px(2.0, s) }.round();
        self.comp_box = Some((bx, by, w, ph));
        self.comp_row = row_h;
        ui::surface(c, (bx, by, w, ph), px(theme::RADIUS, s), s);
        let typed = comp.query.prefix.len();
        for (i, item) in items.iter().enumerate() {
            let r = (bx + inset, by + inset + row_h * i as f32, w - inset * 2.0, row_h);
            let chosen = top + i == comp.sel;
            ui::row_ground(c, r, chosen, false, false, s);
            let (letter, ink) = match item.kind.as_str() {
                "function" | "def" | "method" | "builtin" => ("f", theme::function()),
                "class" | "type" => ("C", theme::special()),
                "keyword" => ("k", theme::keyword()),
                "module" | "path" => ("m", theme::number()),
                "command" => ("\\", theme::keyword()),
                "environment" => ("e", theme::special()),
                "label" => ("#", theme::number()),
                "" => ("w", theme::faint()),
                _ => ("v", theme::string()),
            };
            let br = (r.0 + px(6.0, s), r.1 + ((row_h - badge) / 2.0).round(), badge, badge);
            c.round_rect(br.0, br.1, br.2, br.3, px(theme::RADIUS_SM, s), theme::wash(ink));
            let lw = fonts.width(font::UI_BOLD, small, letter);
            ui::text(
                c,
                fonts,
                (br.0 + (badge - lw) / 2.0).round(),
                ui::base(br.1, badge, small),
                letter,
                font::UI_BOLD,
                small,
                ink,
            );
            let nx = br.0 + badge + px(8.0, s);
            let marks: Vec<usize> = (0..typed.min(item.name.len())).collect();
            let kw = fonts.width(font::UI, small, &item.kind);
            let name = ui::fit(fonts, mono, size, &item.name, r.0 + r.2 - nx - kw - px(16.0, s));
            ui::text_marked(
                c,
                fonts,
                (nx, ui::base(r.1, row_h, size)),
                &name,
                mono,
                size,
                theme::fg(),
                &marks,
                theme::link(),
            );
            ui::text(
                c,
                fonts,
                r.0 + r.2 - px(8.0, s) - kw,
                ui::base(r.1, row_h, small),
                &item.kind,
                font::UI,
                small,
                theme::dim(),
            );
        }
    }

    /// What the right end of the status bar says about the document, and
    /// the name of its kind, which stands last.
    fn status(&mut self) -> (String, &'static str) {
        let buf = &self.doc.buf;
        let line = buf.line_of(buf.cur);
        let place = || format!("Ln {}, Col {}", line + 1, buf.text[buf.starts[line]..buf.cur].chars().count() + 1);
        if let Some(reader) = &self.reader {
            if reader.picture() {
                let (w, h) = reader.pages.sizes[0];
                return (format!("{w} \u{d7} {h}\t{}%", reader.percent()), "Picture");
            }
            return (format!("Page {} of {}", reader.page(), reader.pages.sizes.len()), "PDF");
        }
        let kind = KINDS.iter().find(|k| k.0 == self.doc.doc).map_or("Notebook", |k| k.2);
        let said = match (self.doc.doc, &self.nb) {
            (Doc::Cells, Some(nb)) => {
                let cell = format!("Cell {}/{}", self.doc.cell_at(buf.cur) + 1, self.doc.cells.len());
                let python = nb.kernel_label();
                let state = match (nb.busy(), &nb.version) {
                    _ if nb.input.is_some() => "waiting for input",
                    (true, _) => "busy",
                    (false, Some(_)) => "idle",
                    (false, None) => "not started",
                };
                format!("{cell}\t{python} {state}")
            }
            (Doc::Python | Doc::Code | Doc::Latex, _) => place(),
            (Doc::Markdown | Doc::Text | Doc::Cells, _) => {
                self.count_status()
            }
        };
        (said, kind)
    }

    /// Only editable text documents can switch their presentation kind,
    /// and an SVG between its picture and its text.
    pub(super) fn can_change_kind(&self) -> bool {
        let text = self.nb.is_none() && self.reader.is_none() && !self.doc.read_only;
        text || self.reader.as_ref().is_some_and(|r| r.pages.vector)
    }

    /// Whether the document is an SVG, which can be shown as its picture.
    fn is_svg(&self) -> bool {
        if let Some(reader) = &self.reader {
            return reader.pages.vector;
        }
        self.doc.path.as_deref().is_some_and(|p| matches!(ext(p).as_str(), "svg" | "svgz"))
    }

    /// The kinds a document can be shown as, to pick from.
    pub(super) fn kind_menu(&self) -> Menu {
        let entry = |(_, key, label, detail): &(Doc, &str, &str, &str)| Entry::new(key, label, detail);
        let mut entries: Vec<Entry> = KINDS.iter().map(entry).collect();
        if self.is_svg() {
            entries.insert(0, entry(&(Doc::Text, "picture", "Picture", "drawn as it looks")));
        }
        let current = match &self.reader {
            Some(_) => "picture",
            None => KINDS.iter().find(|k| k.0 == self.doc.doc).map_or("", |k| k.1),
        };
        let mut menu = Menu::new("Show as", entries, current);
        if let Some((x, y, ..)) = self.kind_hit {
            (menu.anchor, menu.rise) = (Some((x, y)), true);
        }
        menu
    }

    /// Reads the document as another kind, by the key of its row in
    /// `KINDS`. The file stays as it is; only how it is shown changes.
    pub(super) fn set_kind(&mut self, key: &str) {
        // An SVG goes between its picture and its text by reading the file
        // again as the one or the other.
        if key == "picture" {
            if self.reader.is_none() && self.is_svg() && self.can_change_kind() {
                if self.doc.buf.modified {
                    self.message = Some("Save the SVG to see it as a picture".into());
                } else {
                    self.load_as(self.doc.path.clone(), true);
                    self.message = Some("Shown as Picture".into());
                }
            }
            return;
        }
        let Some(&(doc, _, label, _)) = KINDS.iter().find(|k| k.1 == key) else { return };
        if self.reader.as_ref().is_some_and(|r| r.pages.vector) {
            self.load_as(self.doc.path.clone(), false);
        } else if !self.can_change_kind() || doc == self.doc.doc {
            return;
        }
        self.unsuggest();
        (self.comp, self.helper) = (None, None);
        self.doc.set_doc(doc);
        self.words = None;
        self.message = Some(format!("Shown as {label}"));
    }

    /// Opens the settings.
    fn open_settings(&mut self) {
        self.comp = None;
        self.menu = None;
        self.settings = Some(Settings::new());
    }


    /// Closes every tab but the one shown, keeping those with unsaved
    /// changes.
    fn close_other_tabs(&mut self) {
        let mut i = self.file_tabs.len();
        while i > 0 {
            i -= 1;
            if i != self.active_file && !self.tab_unsaved(i) {
                self.close_file_tab(i);
            }
        }
    }
}

/// Writes through a temporary file and a rename, so a crash mid-save cannot
/// truncate the document. Falls back to writing in place where the directory
/// does not allow that.
pub fn write_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let tmp = target.with_file_name(format!(".{name}.lightdown-tmp"));
    let attempt = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, &target)
    })();
    if attempt.is_err() {
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&target, data)?;
    }
    Ok(())
}

/// Opens another window by starting a second instance.
fn spawn_self(path: Option<&Path>) {
    if cfg!(test) {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cmd = std::process::Command::new(exe);
        if let Some(p) = path {
            cmd.arg(p);
        }
        let _ = cmd.spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
    const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
    const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

    fn app(text: &str) -> App {
        let mut a = App::new(None);
        a.resize(800, 600, 1.0);
        a.paste_text(text);
        a.key(Key::Home, CTRL, None);
        a
    }

    /// An app on a file that does not exist yet, so only its name matters.
    fn app_named(name: &str, text: &str) -> App {
        let dir = std::env::temp_dir().join(format!("lightdown-test-{}-{name}", std::process::id()));
        let mut a = App::new(Some(dir.join(name)));
        a.resize(800, 600, 1.0);
        a.paste_text(text);
        a
    }

    fn type_in(a: &mut App, s: &str) {
        for ch in s.chars() {
            match ch {
                '\n' => a.key(Key::Enter, NONE, None),
                _ => a.key(Key::Char(ch), NONE, Some(&ch.to_string())),
            }
        }
    }

    fn draw(a: &mut App) {
        let mut px = vec![0u32; 800 * 600];
        a.draw(&mut Canvas::new(&mut px, 800, 600));
    }

    #[test]
    fn focus_and_typewriter_modes_keep_the_caret_and_restore_chrome() {
        let mut a = App::new(None);
        a.resize(900, 1000, 1.0);
        a.paste_text("First paragraph.\n\nSecond paragraph.\n");
        let height = a.title_h();
        a.key(Key::Char('z'), Mods { ctrl: true, alt: true, shift: false }, None);
        assert!(a.focus_mode && a.title_h() == 0.0 && a.head_h() == 0.0);
        a.key(Key::Char('w'), Mods { ctrl: true, alt: true, shift: false }, None);
        assert!(a.doc.typewriter);
        for at in [0, a.doc.buf.text.len()] {
            a.doc.select(at, at);
            let (_, y, _) = a.doc.caret().unwrap();
            assert!((y-450.0).abs() < 2.0, "caret at {y}");
        }
        a.key(Key::Escape, NONE, None);
        assert!(!a.focus_mode && a.title_h() == height);
        for light in [false, true] {
            theme::set_light(light);
            assert!(theme::contrast(theme::dim(), theme::bg()) >= 4.5);
        }
        theme::set_light(false);
    }

    #[test]
    fn lists_continue_and_end() {
        let mut a = app("");
        type_in(&mut a, "- a\nb\n\n1. x\ny\n\n> q\nr");
        assert_eq!(a.doc.buf.text, "- a\n- b\n1. x\n2. y\n> q\n> r");
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "- a\n- b\n1. x\n2. y\n> q\n> ");
    }

    #[test]
    fn markers_show_only_around_the_cursor() {
        let mut a = app("a **bold** c\n\nend");
        let hidden = |a: &mut App| a.doc.layout(0).clusters.iter().filter(|c| c.w == 0.0).count();
        a.key(Key::End, CTRL, None);
        assert_eq!(hidden(&mut a), 2);
        a.key(Key::Home, CTRL, None);
        for _ in 0..5 {
            a.key(Key::Right, NONE, None);
        }
        assert_eq!(hidden(&mut a), 0);
        // Edit mode shows everything regardless of the cursor.
        a.key(Key::End, CTRL, None);
        a.key(Key::Char('e'), CTRL, None);
        assert_eq!(hidden(&mut a), 0);
    }

    #[test]
    fn tables_render_until_entered() {
        let mut a = app("| a | b |\n|---|---|\n| 1 | 2 |\n\nend");
        assert!(!a.doc.layout(0).grid && a.doc.layout(1).h > 0.0);
        a.key(Key::End, CTRL, None);
        assert!(a.doc.layout(0).grid && a.doc.layout(1).h == 0.0 && a.doc.layout(2).h == 0.0);
        // Moving up from below lands inside the table and opens its source.
        a.key(Key::Up, NONE, None);
        a.key(Key::Up, NONE, None);
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 2);
        assert!(!a.doc.layout(0).grid);
    }

    #[test]
    fn clicking_a_checkbox_ticks_it() {
        let mut a = app("text\n\n- [ ] task");
        draw(&mut a);
        let (li, top) = a.doc.visible[2];
        let (r, _) = a.doc.layout(li).checkbox.as_ref().map(|(r, o)| ((r.x, r.y), *o)).unwrap();
        let (x, y) = (a.doc.column().0 + r.0 + 3.0, top + r.1 + 3.0);
        a.press(x, y, NONE, 1);
        a.release();
        assert_eq!(a.doc.buf.text, "text\n\n- [x] task");
        assert_eq!(a.doc.buf.cur, 0);
    }

    #[test]
    fn emphasis_toggles_and_selection_replaces() {
        let mut a = app("one two");
        a.key(Key::Right, CTRL_SHIFT, None);
        a.key(Key::Char('b'), CTRL, None);
        assert_eq!(a.doc.buf.text, "**one** two");
        a.key(Key::Char('b'), CTRL, None);
        assert_eq!(a.doc.buf.text, "one two");
        type_in(&mut a, "1");
        assert_eq!(a.doc.buf.text, "1 two");
        a.key(Key::End, SHIFT, None);
        a.key(Key::Backspace, NONE, None);
        assert_eq!(a.doc.buf.text, "1");
    }

    /// A 40x20 grayscale PNG, white at the top fading to black.
    const PNG: [u8; 90] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 40, 0, 0, 0, 20, 8, 0, 0, 0, 0, 218, 45,
        32, 103, 0, 0, 0, 33, 73, 68, 65, 84, 40, 207, 99, 252, 207, 64, 28, 96, 249, 76, 172, 194, 79, 35, 210, 196,
        81, 207, 12, 82, 19, 25, 121, 137, 84, 72, 52, 0, 0, 5, 95, 17, 111, 148, 40, 173, 21, 0, 0, 0, 0, 73, 69, 78,
        68, 174, 66, 96, 130,
    ];

    #[test]
    fn images_show_under_their_line() {
        let dir = std::env::temp_dir().join(format!("lightdown-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a picture.png");
        std::fs::write(&file, PNG).unwrap();
        let url = file.to_string_lossy().replace(' ', "%20");
        let mut a = app(&format!("![pic]({url})\n\ntext ![gone](missing.png) ![far](https://example.com/x.png)"));

        // Away from the line there is only the picture, at its natural size.
        a.key(Key::End, CTRL, None);
        let lay = a.doc.layout(0);
        assert!(lay.rows.is_empty());
        assert_eq!((lay.images.len(), lay.images[0].w, lay.images[0].h), (1, 40.0, 20.0));
        // Images that cannot be loaded keep their alt text and take no space.
        assert!(a.doc.layout(2).images.is_empty() && a.doc.layout(2).h < lay.h);

        // It is drawn once decoded: white at the top, black at the bottom.
        let mut px = vec![0u32; 800 * 600];
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        a.settle();
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        let x = (a.doc.column().0 + lay.images[0].x) as usize + 20;
        let y = (a.doc.visible[0].1 + lay.images[0].y) as usize;
        assert!(px[y * 800 + x] & 0xff > 0xe0 && px[(y + 19) * 800 + x] & 0xff < 0x20);

        // Clicking it brings the cursor to its markdown, which then shows.
        a.press(x as f32, y as f32 + 5.0, NONE, 1);
        a.release();
        assert_eq!(a.doc.buf.cur, 0);
        assert!(!a.doc.layout(0).rows.is_empty() && a.doc.layout(0).images.len() == 1);
        // The keyboard reaches the line too.
        a.key(Key::End, CTRL, None);
        a.key(Key::Up, NONE, None);
        a.key(Key::Up, NONE, None);
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pdf_links_are_followed_and_bookmarks_fill_the_outline() {
        let dir = std::env::temp_dir().join(format!("lightdown-test-{}-links", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("linked.pdf");
        std::fs::write(&file, pdf::tests::linked()).unwrap();
        let mut a = App::new(Some(file));
        a.resize(1400, 900, 1.0);
        let mut px = vec![0u32; 1400 * 900];
        a.draw(&mut Canvas::new(&mut px, 1400, 900));
        a.settle();
        a.show_side(Some(Tab::Outline));
        a.draw(&mut Canvas::new(&mut px, 1400, 900));
        let titles: Vec<_> = a.outline.items.iter().map(|i| (i.depth, i.title.as_str())).collect();
        assert_eq!(titles, vec![(0, "Intro"), (1, "Detail"), (0, "Part B ›")]);
        assert_eq!(a.outline.current, Some(0));

        // A click on the web link hands its address to the desktop.
        let (wx, wy) = a.reader.as_ref().unwrap().screen_point(0, 150.0, 90.0);
        assert_eq!(a.zone(wx, wy, false), Zone::Link);
        a.press(wx, wy, NONE, 1);
        a.release();
        assert_eq!(a.message.as_deref(), Some("Opened https://example.com/a"));
        // The other leads by name to the second page.
        let (gx, gy) = a.reader.as_ref().unwrap().screen_point(0, 150.0, 190.0);
        a.press(gx, gy, NONE, 1);
        a.release();
        assert_eq!(a.status().0, "Page 2 of 2");
        // Bookmarks lead to their places, and the outline follows the view.
        a.reader.as_mut().unwrap().go_bookmark(0);
        assert_eq!(a.status().0, "Page 1 of 2");
        a.reader.as_mut().unwrap().go_bookmark(2);
        a.draw(&mut Canvas::new(&mut px, 1400, 900));
        assert_eq!(a.status().0, "Page 2 of 2");
        assert_eq!(a.outline.current, Some(2));
        // Scrolled away, the outline follows the top of the view again.
        a.reader.as_mut().unwrap().scroll(-10_000.0);
        a.draw(&mut Canvas::new(&mut px, 1400, 900));
        assert_eq!(a.outline.current, Some(0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pdfs_are_shown_as_pages() {
        let dir = std::env::temp_dir().join(format!("lightdown-test-{}-pdf", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("paper.pdf");
        std::fs::write(&file, pdf::tests::sample(3, 300, 400)).unwrap();
        let mut a = App::new(Some(file.clone()));
        a.resize(800, 600, 1.0);
        assert_eq!(a.status(), ("Page 1 of 3".to_string(), "PDF"));

        // The first page, at its printed size, down the middle: the square
        // in its corner is black, the rest of it white.
        let mut px = vec![0u32; 800 * 600];
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        a.settle();
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        let (left, top) = (200, (a.title_h() + a.head_h()) as usize + 16);
        assert_eq!((px[(top + 10) * 800 + left + 10], px[300 * 800 + 400]), (0xff0000, 0xffffff));

        // Its text is selected by a double click, a word at a time, and
        // copied. "Hello" stands 30 points in, 40 down to its baseline.
        let (hx, hy) = (left as f32 + 32.0 * 4.0 / 3.0, top as f32 + 36.0 * 4.0 / 3.0);
        a.press(hx, hy, NONE, 2);
        a.release();
        a.key(Key::Char('c'), CTRL, None);
        assert_eq!(a.copy.take().as_deref(), Some("Hello"));
        // Dragged, the selection follows the pointer down to the next line.
        a.press(hx - 2.0, hy, NONE, 1);
        a.motion(hx + 4.0, hy + 20.0 * 4.0 / 3.0);
        a.release();
        a.key(Key::Char('c'), CTRL, None);
        assert_eq!(a.copy.take().as_deref(), Some("Hello world\na"));
        assert_eq!(a.zone(hx, hy, false), Zone::Text);

        // Finding selects the match as it is typed; Enter goes on to the
        // next, round to the first.
        a.key(Key::Char('f'), CTRL, None);
        type_in(&mut a, "o");
        a.key(Key::Escape, NONE, None);
        a.key(Key::Char('c'), CTRL, None);
        assert_eq!(a.copy.take().as_deref(), Some("o"));
        a.key(Key::Char('f'), CTRL, None);
        let p = a.prompt.as_mut().unwrap();
        (p.text, p.cur) = (String::new(), 0);
        type_in(&mut a, "AGAIN");
        assert_eq!(a.message.as_deref(), Some("No match"));
        a.prompt.as_mut().unwrap().text = "again".into();
        a.key(Key::Enter, NONE, None);
        a.key(Key::Escape, NONE, None);
        a.key(Key::Char('c'), CTRL, None);
        assert_eq!(a.copy.take().as_deref(), Some("again"));

        // Zoomed past the window's width, the pages scroll sideways.
        a.motion(400.0, 300.0);
        for _ in 0..10 {
            a.key(Key::Char('='), CTRL, None);
        }
        let red = |a: &mut App| {
            let mut px = vec![0u32; 800 * 600];
            a.draw(&mut Canvas::new(&mut px, 800, 600));
            a.settle();
            a.draw(&mut Canvas::new(&mut px, 800, 600));
            px[((a.title_h() + a.head_h()) as usize + 20) * 800 + 30] == 0xff0000
        };
        a.key(Key::Home, NONE, None);
        a.scroll_across(-5000.0);
        assert!(red(&mut a));
        a.key(Key::Right, NONE, None);
        a.scroll_across(100.0);
        assert!(!red(&mut a));
        a.key(Key::Char('0'), CTRL, None);

        // Keys move through the pages and edit nothing.
        a.key(Key::End, CTRL, None);
        assert_eq!(a.status().0, "Page 3 of 3");
        type_in(&mut a, "x");
        a.key(Key::Home, NONE, None);
        assert!(a.status().0 == "Page 1 of 3" && a.doc.buf.text.is_empty() && !a.doc.buf.modified);

        // Saved under another name, it is copied whole, and the copy shown.
        a.key(Key::Char('s'), CTRL_SHIFT, None);
        a.prompt.as_mut().unwrap().text = "copy.pdf".into();
        a.key(Key::Enter, NONE, None);
        let copy = dir.join("copy.pdf");
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&file).unwrap());
        assert!(a.doc.path.as_ref() == Some(&copy) && a.reader.is_some());

        // Written again elsewhere, it is read again on coming back.
        std::fs::write(&copy, pdf::tests::sample(2, 300, 400)).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
        std::fs::File::options().write(true).open(&copy).unwrap().set_modified(later).unwrap();
        a.attend();
        assert_eq!(a.status().0, "Page 1 of 2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pictures_are_shown_as_themselves() {
        let dir = std::env::temp_dir().join(format!("lightdown-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The left half red, the right half clear.
        let half = |w: usize, h: usize| -> Vec<u8> {
            (0..w * h).flat_map(|i| if i % w < w / 2 { [255, 0, 0, 255] } else { [0, 0, 0, 0] }).collect()
        };
        let file = dir.join("red.png");
        std::fs::write(&file, crate::pics::tests::png(400, 200, &half(400, 200))).unwrap();
        let mut a = App::new(Some(file.clone()));
        a.resize(800, 600, 1.0);
        assert!(a.doc.read_only && a.message.is_none());
        assert_eq!(a.status(), ("400 \u{d7} 200\t100%".to_string(), "Picture"));

        // At its actual size, in the middle of the window; what it lets
        // through shows the checkerboard.
        let shot = |a: &mut App| {
            let mut px = vec![0u32; 800 * 600];
            a.draw(&mut Canvas::new(&mut px, 800, 600));
            a.settle();
            a.draw(&mut Canvas::new(&mut px, 800, 600));
            px
        };
        let mid = ((a.title_h() + a.head_h()) + a.body_bottom()) / 2.0;
        let at = |px: &[u32], x: f32| px[mid as usize * 800 + x as usize];
        let px = shot(&mut a);
        assert_eq!(at(&px, 100.0), theme::bg());
        assert_eq!(at(&px, 300.0), 0xff0000);
        assert!(![0xff0000, theme::bg()].contains(&at(&px, 500.0)));
        assert_eq!(a.zone(300.0, mid, false), Zone::Plain);

        // It has no text to find, and keys edit nothing.
        a.key(Key::Char('f'), CTRL, None);
        assert!(a.prompt.is_none() && a.message.as_deref() == Some("A picture has no text to find"));
        type_in(&mut a, "x");
        assert!(a.doc.buf.text.is_empty() && !a.doc.buf.modified);

        // Zoomed, it grows from the middle; dragged, it follows the pointer.
        a.motion(400.0, mid);
        for _ in 0..10 {
            a.key(Key::Char('='), CTRL, None);
        }
        assert_eq!(a.status().0, "400 \u{d7} 200\t259%");
        assert_ne!(at(&shot(&mut a), 450.0), 0xff0000);
        a.press(400.0, mid, NONE, 1);
        a.motion(500.0, mid);
        a.release();
        assert_eq!(at(&shot(&mut a), 450.0), 0xff0000);
        a.key(Key::Char('0'), CTRL, None);
        assert_eq!(a.status().0, "400 \u{d7} 200\t100%");

        // One too wide for the window is made smaller to fit it.
        let wide = dir.join("wide.png");
        std::fs::write(&wide, crate::pics::tests::png(2000, 10, &half(2000, 10))).unwrap();
        a.pick(wide.clone(), false);
        assert_eq!(a.status().0, "2000 \u{d7} 10\t38%");

        // Written again elsewhere, it is read again on coming back.
        std::fs::write(&wide, crate::pics::tests::png(20, 10, &half(20, 10))).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
        std::fs::File::options().write(true).open(&wide).unwrap().set_modified(later).unwrap();
        a.attend();
        assert_eq!(a.status().0, "20 \u{d7} 10\t100%");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn svgs_are_drawn_and_gifs_play() {
        let dir = std::env::temp_dir().join(format!("lightdown-svg-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("box.svg");
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="#00ff00"/></svg>"##;
        std::fs::write(&svg, source).unwrap();
        let mut a = App::new(Some(svg.clone()));
        a.resize(800, 600, 1.0);
        assert_eq!(a.status(), ("100 \u{d7} 50\t100%".to_string(), "Picture"));
        let mut px = vec![0u32; 800 * 600];
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        a.settle();
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        let mid = ((a.title_h() + a.head_h()) + a.body_bottom()) / 2.0;
        assert_eq!(px[mid as usize * 800 + 400], 0x00ff00);
        assert!(a.next_frame().is_none());

        // Shown as code, it is its text, to edit; and back as its picture.
        assert!(a.can_change_kind() && a.kind_menu().entries[0].key == "picture");
        a.set_kind("code");
        assert!(a.reader.is_none() && a.doc.buf.text == source && !a.doc.read_only);
        type_in(&mut a, "x");
        a.set_kind("picture");
        assert!(a.reader.is_none() && a.message.as_deref() == Some("Save the SVG to see it as a picture"));
        a.key(Key::Char('z'), CTRL, None);
        a.key(Key::Char('s'), CTRL, None);
        a.set_kind("picture");
        assert!(a.reader.is_some() && a.status().1 == "Picture");

        // An animated GIF asks to be drawn again when its frame is over.
        use gif::DisposalMethod::Keep;
        let anim = dir.join("blink.gif");
        let frames = [(0, 0, 2, vec![1, 1], 10, Keep), (0, 0, 2, vec![2, 2], 10, Keep)];
        std::fs::write(&anim, crate::pics::tests::gif(2, 1, &frames)).unwrap();
        a.pick(anim, false);
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        a.settle();
        a.draw(&mut Canvas::new(&mut px, 800, 600));
        let wait = a.next_frame().unwrap().saturating_duration_since(std::time::Instant::now());
        assert!(wait <= std::time::Duration::from_millis(100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_keeps_unsaved_work() {
        let mut a = app("text");
        a.close();
        assert!(a.exit);

    }

    #[test]
    fn vertical_movement_follows_wrapped_rows() {
        let long = "word ".repeat(80);
        let mut a = app(&format!("{long}\nshort"));
        let rows = a.doc.layout(0).rows.len();
        assert!(rows > 2);
        for _ in 0..rows {
            assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 0);
            a.key(Key::Down, NONE, None);
        }
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 1);
        a.key(Key::End, NONE, None);
        assert_eq!(a.doc.buf.cur, a.doc.buf.text.len());
    }

    #[test]
    fn python_indents_and_completes() {
        let mut a = app_named("a.py", "");
        a.cfg.autocomplete = false;
        type_in(&mut a, "def area(width):\nreturn width\nx = 1");
        assert_eq!(a.doc.buf.text, "def area(width):\n    return width\nx = 1");
        a.key(Key::Char('/'), CTRL, None);
        assert_eq!(a.doc.buf.text, "def area(width):\n    return width\n# x = 1");
        // Backspace in the indentation steps back a level.
        a.key(Key::Up, NONE, None);
        a.key(Key::Home, NONE, None);
        a.key(Key::Backspace, NONE, None);
        assert_eq!(a.doc.buf.text, "def area(width):\nreturn width\n# x = 1");

        a.cfg.autocomplete = true;
        a.key(Key::End, CTRL, None);
        type_in(&mut a, "\nwi");
        let names = |a: &App| a.comp.as_ref().map(|c| c.items.iter().map(|i| i.name.clone()).collect::<Vec<_>>());
        assert_eq!(names(&a).unwrap()[0], "width");
        a.key(Key::Tab, NONE, None);
        assert!(a.doc.buf.text.ends_with("\nwidth") && a.comp.is_none());
        // Nothing is offered inside a comment.
        type_in(&mut a, " # wi");
        assert!(a.comp.is_none());
    }

    #[test]
    fn the_status_bar_shows_and_changes_the_kind() {
        let mut a = app_named("notes.txt", "# Title");
        draw(&mut a);
        assert_eq!(a.status(), ("2 words · 7 chars · 1 min read".to_string(), "Plain text"));
        // The kind's name is a button that opens a list rising from it.
        let (x, y, w, h) = a.kind_hit.unwrap();
        let (x, y) = (x + w / 2.0, y + h / 2.0);
        assert_eq!(a.zone(x, y, false), Zone::Button);
        a.press(x, y, NONE, 1);
        assert!(matches!(&a.menu, Some((MenuKind::Kind, m)) if m.rise && m.entries.len() == KINDS.len()));
        type_in(&mut a, "markdown\n");
        assert!(a.menu.is_none() && a.doc.doc == Doc::Markdown && a.message.as_deref() == Some("Shown as Markdown"));
        draw(&mut a);
        assert_eq!(a.status().1, "Markdown");
        // The keyboard reaches it too; a notebook keeps its cells.
        a.key(Key::Char('m'), Mods { ctrl: true, shift: false, alt: true }, None);
        assert!(matches!(&a.menu, Some((MenuKind::Kind, _))));
        let mut n = app_named("n.ipynb", "");
        draw(&mut n);
        assert!(n.kind_hit.is_none() && n.status().1 == "Notebook");
        n.file_action("doc_kind");
        assert!(n.menu.is_none() && n.doc.doc == Doc::Cells);
    }

    #[test]
    fn latex_ends_environments_and_completes() {
        let mut a = app_named("paper.tex", "");
        a.cfg.autocomplete = false;
        assert_eq!(a.doc.doc, Doc::Latex);
        type_in(&mut a, "\\begin{itemize}\n\\item one");
        assert_eq!(a.doc.buf.text, "\\begin{itemize}\n    \\item one\n\\end{itemize}");
        // A selection back across lines is replaced as anywhere else.
        a.key(Key::End, CTRL, None);
        a.key(Key::Up, SHIFT, None);
        a.key(Key::Up, SHIFT, None);
        assert!(a.doc.buf.line_of(a.doc.buf.cur) == 0 && a.doc.buf.has_sel());
        a.key(Key::Enter, NONE, None);
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "\\begin{itemize}\n    \\item one\n\\end{itemize}");
        // Only an environment left open is ended.
        a.key(Key::End, CTRL, None);
        type_in(&mut a, "\n\\begin{a}\\end{a}\n");
        assert!(a.doc.buf.text.ends_with("\\end{itemize}\n\\begin{a}\\end{a}\n"));
        // Comments are `%`, and bold and italic are commands.
        a.key(Key::Up, NONE, None);
        a.key(Key::Char('/'), CTRL, None);
        assert!(a.doc.buf.text.ends_with("\n% \\begin{a}\\end{a}\n"));
        a.key(Key::End, CTRL, None);
        type_in(&mut a, "word");
        a.key(Key::Left, CTRL_SHIFT, None);
        a.key(Key::Char('b'), CTRL, None);
        assert!(a.doc.buf.text.ends_with("\n\\textbf{word}"));
        // Commands complete as they are typed.
        a.cfg.autocomplete = true;
        a.key(Key::End, NONE, None);
        type_in(&mut a, " \\sec");
        let names: Vec<String> = a.comp.as_ref().unwrap().items.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names[0], "\\section");
        a.key(Key::Tab, NONE, None);
        assert!(a.doc.buf.text.ends_with("\\textbf{word} \\section"));
    }

    #[test]
    fn notebook_cells_are_edited_as_one_text() {
        let mut a = app_named("n.ipynb", "");
        assert_eq!(a.doc.buf.text, "\u{1e}code\n");
        type_in(&mut a, "x = 1");
        a.key(Key::Char('b'), CTRL_SHIFT, None);
        type_in(&mut a, "# Notes");
        a.key(Key::Char('m'), CTRL_SHIFT, None);
        assert_eq!(a.doc.buf.text, "\u{1e}code\nx = 1\n\u{1e}markdown\n# Notes");
        assert_eq!(a.doc.buf.cur, a.doc.buf.text.len());
        // The cursor steps over the header between the cells.
        a.key(Key::Home, NONE, None);
        a.key(Key::Left, NONE, None);
        assert_eq!(&a.doc.buf.text[..a.doc.buf.cur], "\u{1e}code\nx = 1");
        a.key(Key::Right, NONE, None);
        assert!(a.doc.buf.text[a.doc.buf.cur..].starts_with("# Notes"));
        // Backspace at the start of a cell does not eat its header.
        a.key(Key::Backspace, NONE, None);
        assert_eq!(a.doc.cells.len(), 2);
        // Copying gives other programs `# %%` lines, not control characters.
        a.key(Key::Char('a'), CTRL, None);
        a.key(Key::Char('c'), CTRL, None);
        assert_eq!(a.copy.take().unwrap(), "# %%\nx = 1\n# %% [markdown]\n# Notes");
        // Deleting everything leaves one empty cell.
        a.key(Key::Backspace, NONE, None);
        assert_eq!(a.doc.buf.text, "\u{1e}code\n");
        a.key(Key::Char('z'), CTRL, None);
        a.key(Key::Char('z'), CTRL, None);
        a.key(Key::Char('d'), CTRL_SHIFT, None);
        assert_eq!(a.doc.buf.text, "\u{1e}code\nx = 1");
        draw(&mut a);
    }
}

/// Stable command ids accepted by keymap.json.
pub(crate) fn keymap_command_known(id: &str) -> bool {
    keybindings::commands().iter().any(|(known,_,_)| *known == id)
}
