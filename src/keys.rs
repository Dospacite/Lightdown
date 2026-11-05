//! Which keys do what. Every shortcut has a default; the keymap lets the
//! user move one to other keys. The rest of the program still thinks in
//! the defaults: a key pressed is first turned into the default key of
//! whatever it is assigned to, so no handler needs to know of the keymap.

pub mod contextual;
mod bases;

use std::collections::BTreeMap;

use crate::app::{Key, Mods};

/// A key together with the modifiers held.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Chord {
    pub key: Key,
    pub mods: Mods,
}

impl Chord {
    /// Shift is part of what a symbol is (`?` is Shift+`/` on one keyboard
    /// and a key of its own on another), so for symbols it does not count.
    pub fn new(key: Key, mut mods: Mods) -> Chord {
        if matches!(key, Key::Char(c) if !c.is_alphanumeric() && c != ' ') {
            mods.shift = false;
        }
        Chord { key, mods }
    }

    /// Reads a chord as the settings file writes it, as in `ctrl+shift+s`.
    pub fn parse(code: &str) -> Option<Chord> {
        let (mut mods, mut rest) = (Mods::default(), code);
        loop {
            let (flag, after) = if let Some(r) = rest.strip_prefix("ctrl+") {
                (&mut mods.ctrl, r)
            } else if let Some(r) = rest.strip_prefix("shift+") {
                (&mut mods.shift, r)
            } else if let Some(r) = rest.strip_prefix("alt+") {
                (&mut mods.alt, r)
            } else {
                break;
            };
            // What is left after `ctrl+` may be the `+` key itself.
            if after.is_empty() {
                break;
            }
            (*flag, rest) = (true, after);
        }
        let mut chars = rest.chars();
        let key = match rest {
            "escape" => Key::Escape,
            "backspace" => Key::Backspace,
            "delete" => Key::Delete,
            "home" => Key::Home,
            "end" => Key::End,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "enter" => Key::Enter,
            "tab" => Key::Tab,
            "space" => Key::Char(' '),
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            _ => match (chars.next()?, chars.as_str()) {
                (c, "") => Key::Char(c),
                ('f', n) => Key::F(n.parse().ok().filter(|n| (1..=12).contains(n))?),
                _ => return None,
            },
        };
        Some(Chord::new(key, mods))
    }

    fn write(&self, ctrl: &str, shift: &str, alt: &str, upper: bool) -> String {
        let mut out = String::new();
        for (held, name) in [(self.mods.ctrl, ctrl), (self.mods.shift, shift), (self.mods.alt, alt)] {
            if held {
                out += name;
            }
        }
        match self.key {
            Key::Escape => out += if upper { "Esc" } else { "escape" },
            Key::Backspace => out += if upper { "Backspace" } else { "backspace" },
            Key::Delete => out += if upper { "Delete" } else { "delete" },
            Key::Home => out += if upper { "Home" } else { "home" },
            Key::End => out += if upper { "End" } else { "end" },
            Key::Left => out += if upper { "Left" } else { "left" },
            Key::Right => out += if upper { "Right" } else { "right" },
            Key::Up => out += if upper { "Up" } else { "up" },
            Key::Down => out += if upper { "Down" } else { "down" },
            Key::Enter => out += if upper { "Enter" } else { "enter" },
            Key::Tab => out += if upper { "Tab" } else { "tab" },
            Key::PageUp => out += if upper { "PgUp" } else { "pageup" },
            Key::PageDown => out += if upper { "PgDn" } else { "pagedown" },
            Key::Char(' ') => out += if upper { "Space" } else { "space" },
            Key::Char(c) if upper => out.extend(c.to_uppercase()),
            Key::Char(c) => out.push(c),
            Key::F(n) => out += &format!("{}{n}", if upper { "F" } else { "f" }),
            _ => out += "?",
        }
        out
    }

    /// The chord as the settings file holds it.
    pub fn code(&self) -> String {
        self.write("ctrl+", "shift+", "alt+", false)
    }

    /// The chord as it is shown.
    pub fn name(&self) -> String {
        self.write("Ctrl+", "Shift+", "Alt+", true)
    }

    /// Whether a shortcut may be put on this chord: it must not be a key
    /// that types or moves.
    pub fn assignable(&self) -> bool {
        match self.key {
            Key::Left | Key::Right | Key::Up | Key::Down => self.mods.alt,
            Key::F(_) => true,
            Key::Char(_) | Key::Tab | Key::PageUp | Key::PageDown => self.mods.ctrl || self.mods.alt,
            Key::Enter => self.mods.ctrl || self.mods.alt || self.mods.shift,
            _ => false,
        }
    }
}

/// When a shortcut applies.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scope {
    /// Wherever text can be typed.
    Always,
    /// While no prompt, list or panel is open.
    Main,
    /// While the keyboard is with the document.
    Doc,
    /// In a notebook.
    Notebook,
    /// While the keyboard is with the agent in the sidebar.
    Chat,
}

impl Scope {
    /// Whether two shortcuts with these scopes could both be meant by one
    /// key press.
    #[cfg(test)]
    fn meets(self, other: Scope) -> bool {
        let doc = |s: Scope| matches!(s, Scope::Doc | Scope::Notebook);
        !((doc(self) && other == Scope::Chat) || (self == Scope::Chat && doc(other)))
    }
}

pub struct Action {
    /// The name the settings file knows the shortcut by.
    pub id: &'static str,
    pub name: &'static str,
    pub scope: Scope,
    /// The default keys. The first is what handlers look for.
    pub keys: &'static [&'static str],
}

const fn action(id: &'static str, name: &'static str, scope: Scope, keys: &'static [&'static str]) -> Action {
    Action { id, name, scope, keys }
}

pub const ACTIONS: &[Action] = &[
    action("find_selection", "Search: Toggle find in selection", Scope::Always, &["alt+l"]),
    action("bracket_jump", "Edit: Jump to matching bracket", Scope::Doc, &["ctrl+alt+b"]),
    action("bracket_select", "Edit: Select to matching bracket", Scope::Doc, &["ctrl+alt+shift+b"]),
    action("cursor_all", "Edit: Select all occurrences", Scope::Doc, &["ctrl+alt+l"]),
    action("cursor_skip", "Edit: Skip occurrence", Scope::Doc, &["ctrl+alt+shift+d"]),
    action("bookmark_toggle", "Bookmarks: Toggle bookmark", Scope::Doc, &["ctrl+alt+k"]),
    action("bookmark_next", "Bookmarks: Next in file", Scope::Doc, &["ctrl+alt+]"]),
    action("bookmark_previous", "Bookmarks: Previous in file", Scope::Doc, &["ctrl+alt+["]),
    action("selection_expand", "Edit: Expand selection", Scope::Doc, &["alt+shift+right"]),
    action("selection_shrink", "Edit: Shrink selection", Scope::Doc, &["alt+shift+left"]),
    action("lsp_actions", "Code: Code actions and quick fixes", Scope::Doc, &["ctrl+alt+."]),
    action("lsp_signature", "Code: Signature help", Scope::Doc, &["ctrl+alt+space"]),
    action("lsp_definition", "Code: Go to definition", Scope::Doc, &["f12"]),
    action("lsp_implementation", "Code: Go to implementation", Scope::Doc, &["ctrl+f12"]),
    action("lsp_type_definition", "Code: Go to type definition", Scope::Doc, &["ctrl+shift+f12"]),
    action("lsp_declaration", "Code: Go to declaration", Scope::Doc, &["ctrl+alt+f12"]),
    action("lsp_peek_definition", "Code: Peek definition", Scope::Doc, &["alt+f12"]),
    action("lsp_peek_references", "Code: Peek references", Scope::Doc, &["alt+shift+f12"]),
    action("lsp_hierarchy_incoming", "Code: Incoming calls", Scope::Doc, &["alt+shift+h"]),
    action("lsp_hierarchy_outgoing", "Code: Outgoing calls", Scope::Doc, &["ctrl+alt+shift+h"]),
    action("lsp_hierarchy_supertypes", "Code: Supertypes", Scope::Doc, &["alt+shift+u"]),
    action("lsp_hierarchy_subtypes", "Code: Subtypes", Scope::Doc, &["alt+shift+d"]),
    action("lsp_references", "Code: Find references", Scope::Doc, &["shift+f12"]),
    action("lsp_rename", "Code: Rename symbol", Scope::Doc, &["ctrl+f2"]),
    action("http_send", "HTTP: Send request at cursor", Scope::Doc, &["ctrl+alt+enter"]),
    action("agent_hunk_keep", "Agent: Keep current hunk", Scope::Doc, &["ctrl+alt+y"]),
    action("agent_hunk_undo", "Agent: Undo current hunk", Scope::Doc, &["ctrl+alt+u"]),
    action("inline_edit", "Agent: Edit selected text inline", Scope::Doc, &["ctrl+alt+i"]),
    action("cursor_next", "Edit: Add next matching selection", Scope::Doc, &["ctrl+d"]),
    action("cursor_above", "Edit: Add cursor above", Scope::Doc, &["ctrl+alt+up"]),
    action("cursor_below", "Edit: Add cursor below", Scope::Doc, &["ctrl+alt+down"]),
    action("quick_open", "Go to: File", Scope::Main, &["ctrl+p"]),
    action("palette", "Go to: Command", Scope::Main, &["ctrl+shift+p"]),
    action("goto_line", "Go to: Line", Scope::Doc, &["ctrl+g"]),
    action("symbols", "Go to: Heading or definition", Scope::Doc, &["ctrl+r"]),
    action("back", "Go to: Back", Scope::Main, &["alt+left"]),
    action("forward", "Go to: Forward", Scope::Main, &["alt+right"]),
    action("project_symbols", "Go to: Symbol in project", Scope::Doc, &["ctrl+alt+r"]),
    action("project_tasks", "Task: Run a project task", Scope::Main, &["f6"]),
    action("next_error", "Problems: Next problem", Scope::Main, &["f8"]),
    action("previous_error", "Problems: Previous problem", Scope::Main, &["shift+f8"]),
    action("tests_list", "Tests: Show tests", Scope::Main, &["ctrl+shift+y"]),
    action("move_line_up", "Edit: Move lines up", Scope::Doc, &["alt+up"]),
    action("move_line_down", "Edit: Move lines down", Scope::Doc, &["alt+down"]),
    action("delete_line", "Edit: Delete lines", Scope::Doc, &["ctrl+shift+k"]),
    action("duplicate_line", "Edit: Duplicate lines", Scope::Doc, &["ctrl+alt+d"]),
    action("run_default", "Run: Start default configuration", Scope::Main, &["f7"]),
    action("debug_start", "Debug: Start Python script", Scope::Main, &["ctrl+f5"]),
    action("debug_breakpoint", "Debug: Toggle breakpoint on current line", Scope::Main, &["f9"]),
    action("debug_next", "Debug: Step over", Scope::Main, &["f10"]),
    action("debug_step", "Debug: Step into", Scope::Main, &["f11"]),
    action("debug_out", "Debug: Step out", Scope::Main, &["shift+f11"]),
    action("git_source_view", "Git: Go to source control and back", Scope::Main, &["ctrl+shift+g"]),
    action("git_next_change", "Git: Next change", Scope::Main, &["alt+f8"]),
    action("git_previous_change", "Git: Previous change", Scope::Main, &["alt+shift+f8"]),
    action("git_hunk", "Git: Show saved hunk at cursor", Scope::Main, &["ctrl+alt+h"]),
    action("save", "File: Save", Scope::Main, &["ctrl+s"]),
    action("save_as", "File: Save as", Scope::Main, &["ctrl+shift+s"]),
    action("open", "File: Open a file or folder", Scope::Main, &["ctrl+o"]),
    action("focus_mode", "View: Toggle focus mode", Scope::Main, &["ctrl+alt+z"]),
    action("typewriter_mode", "View: Toggle typewriter mode", Scope::Main, &["ctrl+alt+w"]),
    action("minimap", "View: Toggle minimap", Scope::Main, &[]),
    action("word_wrap", "View: Toggle word wrap", Scope::Main, &["alt+z"]),
    action("reopen_tab", "File: Reopen closed tab", Scope::Main, &["ctrl+alt+shift+t"]),
    action("close_tab", "File: Close the tab", Scope::Main, &["ctrl+w"]),
    action("next_tab", "View: Next tab", Scope::Main, &["ctrl+tab"]),
    action("view_split_right", "View: Split right", Scope::Main, &["ctrl+\\"]),
    action("view_split_left", "View: Split left", Scope::Main, &["ctrl+alt+\\"]),
    action("view_split_down", "View: Split down", Scope::Main, &["ctrl+alt+-"]),
    action("view_split_up", "View: Split up", Scope::Main, &["ctrl+alt+="]),
    action("view_close", "View: Close view", Scope::Main, &["ctrl+alt+shift+w"]),
    action("view_zoom", "View: Zoom focused view", Scope::Main, &["ctrl+alt+shift+z"]),
    action("view_focus_left", "View: Focus view to the left", Scope::Main, &["ctrl+alt+shift+left"]),
    action("view_focus_right", "View: Focus view to the right", Scope::Main, &["ctrl+alt+shift+right"]),
    action("view_focus_up", "View: Focus view above", Scope::Main, &["ctrl+alt+shift+up"]),
    action("view_focus_down", "View: Focus view below", Scope::Main, &["ctrl+alt+shift+down"]),
    action("view_move_left", "View: Move view left", Scope::Main, &["ctrl+alt+left"]),
    action("view_move_right", "View: Move view right", Scope::Main, &["ctrl+alt+right"]),
    action("view_move_up", "View: Move view up", Scope::Main, &["ctrl+alt+pageup"]),
    action("view_move_down", "View: Move view down", Scope::Main, &["ctrl+alt+pagedown"]),
    action("previous_tab", "View: Previous tab", Scope::Main, &["ctrl+shift+tab"]),
    action("new_window", "Window: New window", Scope::Doc, &["ctrl+n"]),
    action("file_menu", "File: Show what can be done with it", Scope::Main, &["alt+f"]),
    action("new_file", "File: New file or folder", Scope::Main, &["ctrl+alt+n"]),
    action("save_copy", "File: Save a copy", Scope::Main, &["ctrl+alt+s"]),
    action("rename", "File: Rename or move", Scope::Main, &["f2"]),
    action("trash", "File: Move to the trash", Scope::Main, &["ctrl+alt+x"]),
    action("reveal", "File: Show in the file manager", Scope::Main, &["ctrl+alt+o"]),
    action("copy_path", "File: Copy the path", Scope::Main, &["ctrl+alt+c"]),
    action("preview", "Preview: Build and preview (markdown, LaTeX)", Scope::Main, &["f5"]),
    action("export_pdf", "File: Export as PDF", Scope::Main, &["ctrl+alt+p"]),
    action("sync", "PDF: Show the source line in the PDF", Scope::Main, &["ctrl+alt+j"]),
    action("doc_kind", "File: Show as markdown, LaTeX, code\u{2026}", Scope::Main, &["ctrl+alt+m"]),
    action("find", "Edit: Find", Scope::Main, &["ctrl+f"]),
    action("replace", "Edit: Find and replace", Scope::Main, &["ctrl+h"]),
    action("search", "Search: Find in the project's files", Scope::Main, &["ctrl+shift+f"]),
    action("quit", "Window: Quit", Scope::Main, &["ctrl+q"]),
    action("settings", "Preferences: Settings", Scope::Main, &["ctrl+,"]),
    action("keys", "Preferences: Keybinds", Scope::Main, &["f1"]),
    action("zoom_in", "View: Zoom in", Scope::Doc, &["ctrl+=", "ctrl++"]),
    action("zoom_out", "View: Zoom out", Scope::Doc, &["ctrl+-"]),
    action("zoom_reset", "View: Reset the zoom", Scope::Doc, &["ctrl+0"]),
    action("undo", "Edit: Undo", Scope::Main, &["ctrl+z"]),
    action("redo", "Edit: Redo", Scope::Main, &["ctrl+shift+z", "ctrl+y"]),
    action("select_all", "Edit: Select all", Scope::Main, &["ctrl+a"]),
    action("copy_rich", "Markdown: Copy as rich text", Scope::Doc, &["ctrl+shift+c"]),
    action("copy", "Edit: Copy", Scope::Main, &["ctrl+c"]),
    action("cut", "Edit: Cut", Scope::Main, &["ctrl+x"]),
    action("paste_plain", "Edit: Paste plain text", Scope::Always, &["ctrl+shift+v"]),
    action("paste", "Edit: Paste", Scope::Always, &["ctrl+v"]),
    action("edit_mode", "Markdown: Toggle edit mode (show all syntax)", Scope::Doc, &["ctrl+e"]),
    action("bold", "Edit: Bold (markdown, LaTeX)", Scope::Doc, &["ctrl+b"]),
    action("italic", "Edit: Italic (markdown, LaTeX)", Scope::Doc, &["ctrl+i"]),
    action("comment", "Edit: Toggle comment", Scope::Doc, &["ctrl+/"]),
    action("complete", "Edit: Complete the name", Scope::Doc, &["ctrl+space"]),
    action("docs", "Edit: Show the documentation (Python)", Scope::Doc, &["ctrl+k"]),
    action("suggest", "Suggestion: Ask for one at the cursor", Scope::Doc, &["alt+\\", "ctrl+shift+space"]),
    action("run", "Notebook: Run the cell", Scope::Notebook, &["ctrl+enter"]),
    action("run_next", "Notebook: Run the cell and go on", Scope::Notebook, &["shift+enter"]),
    action("run_add", "Notebook: Run the cell and add one", Scope::Notebook, &["alt+enter"]),
    action("run_all", "Notebook: Run all cells", Scope::Notebook, &["ctrl+shift+enter"]),
    action("cell_above", "Notebook: Add a cell above", Scope::Notebook, &["ctrl+shift+a"]),
    action("cell_below", "Notebook: Add a cell below", Scope::Notebook, &["ctrl+shift+b"]),
    action("cell_kind", "Notebook: Switch between code and markdown", Scope::Notebook, &["ctrl+shift+m"]),
    action("cell_delete", "Notebook: Delete the cell", Scope::Notebook, &["ctrl+shift+d"]),
    action("interrupt", "Notebook: Interrupt the kernel", Scope::Notebook, &["ctrl+."]),
    action("restart", "Notebook: Restart the kernel", Scope::Notebook, &["ctrl+shift+r"]),
    action("clear_outputs", "Notebook: Clear all outputs", Scope::Notebook, &["ctrl+shift+o"]),
    action("sidebar", "Agent: Go to the agent and back", Scope::Main, &["ctrl+l"]),
    action("sidebar_hide", "View: Hide the side panels", Scope::Main, &["ctrl+shift+l"]),
    action("files", "Explorer: Go to the explorer and back", Scope::Main, &["ctrl+shift+e"]),
    action("outline", "View: Toggle the outline", Scope::Main, &["ctrl+shift+h"]),
    action("terminal", "Terminal: Go to the terminal and back", Scope::Main, &["ctrl+`", "ctrl+j"]),
    action("terminal_hide", "Terminal: Hide the panel", Scope::Main, &["ctrl+alt+`"]),
    action("terminal_new", "Terminal: New terminal", Scope::Main, &["ctrl+shift+t"]),
    action("terminal_close", "Terminal: Close the terminal", Scope::Main, &["ctrl+shift+w"]),
    action("terminal_next", "Terminal: Next terminal", Scope::Main, &["ctrl+shift+pagedown"]),
    action("terminal_previous", "Terminal: Previous terminal", Scope::Main, &["ctrl+shift+pageup"]),
    action("terminal_split", "Terminal: Split, or show one", Scope::Main, &["ctrl+shift+j"]),
    action("threads", "Agent: Threads", Scope::Chat, &["ctrl+t"]),
    action("new_thread", "Agent: New thread", Scope::Chat, &["ctrl+shift+n"]),
    action("model", "Agent: Choose the model", Scope::Chat, &["ctrl+m"]),
    action("reasoning", "Agent: Choose the reasoning", Scope::Chat, &["ctrl+r"]),
    action("context", "Agent: Send IDE context or not", Scope::Chat, &["ctrl+k"]),
];

/// Keys that mean what they mean because of where they are pressed. They
/// are listed for reference and cannot be moved.
pub const FIXED: &[(&str, &str)] = &[
    ("Suggestion: take it", "Tab"),
    ("Suggestion: take its next word", "Ctrl+Right"),
    ("Suggestion: dismiss it", "Esc"),
    ("Completion list: choose", "Up, Down"),
    ("Completion list: accept", "Tab, Enter"),
    ("Agent: send the message", "Enter"),
    ("Agent: new line in the message", "Shift+Enter"),
    ("Agent: stop, or back to the document", "Esc"),
    ("Agent: allow, always allow, decline a tool", "Enter, A, Esc"),
    ("Folder: choose, unfold and fold", "Up, Down, Right, Left"),
    ("Folder: open the file, or in a new window", "Enter, Ctrl+Enter"),
    ("Folder: back to the document", "Esc"),
    ("Terminal: copy, paste", "Ctrl+Shift+C, Ctrl+Shift+V"),
    ("Terminal: scroll the history", "Shift+PgUp, Shift+PgDn, Shift+Home, Shift+End"),
    ("Terminal: previous, next command", "Ctrl+Shift+Up, Ctrl+Shift+Down"),
    ("Context menu: open it", "Right click"),
];

/// What a key press comes to once the keymap has been consulted.
#[derive(PartialEq, Debug)]
pub enum Meaning {
    /// Nothing is assigned to it: it means what it always did.
    Itself,
    /// It was the default for something that has been moved elsewhere.
    Nothing,
    /// It stands for this default chord.
    Default(Chord),
}

pub struct Keymap {
    /// The chords of each action, in the order of `ACTIONS`.
    binds: Vec<Vec<Chord>>,
}

fn defaults(a: &Action) -> Vec<Chord> {
    a.keys.iter().filter_map(|k| Chord::parse(k)).collect()
}

impl Keymap {
    /// The defaults, with the user's changes laid over them.
    pub fn new(changes: &BTreeMap<String, Vec<String>>) -> Keymap {
        let chords = |a: &Action| match changes.get(a.id) {
            Some(codes) => codes.iter().filter_map(|c| Chord::parse(c)).collect(),
            None => defaults(a),
        };
        Keymap { binds: ACTIONS.iter().map(chords).collect() }
    }

    /// Turns a key press into the default chord of the action it is
    /// assigned to. `active` says which scopes apply right now.
    pub fn meaning(&self, chord: Chord, active: impl Fn(Scope) -> bool) -> Meaning {
        let live = || (0..ACTIONS.len()).filter(|&i| active(ACTIONS[i].scope));
        if let Some(i) = live().find(|&i| self.binds[i].contains(&chord)) {
            let first = defaults(&ACTIONS[i])[0];
            return if first == chord { Meaning::Itself } else { Meaning::Default(first) };
        }
        let moved = live().any(|i| defaults(&ACTIONS[i]).contains(&chord));
        if moved { Meaning::Nothing } else { Meaning::Itself }
    }

    /// Whether a chord is one of an action's keys.
    pub fn has(&self, id: &str, chord: Chord) -> bool {
        ACTIONS.iter().position(|a| a.id == id).is_some_and(|i| self.binds[i].contains(&chord))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(code: &str) -> Chord {
        Chord::parse(code).unwrap()
    }

    #[test]
    fn chords_are_read_and_written() {
        for code in [
            "ctrl+s",
            "ctrl+shift+z",
            "alt+\\",
            "ctrl++",
            "f1",
            "shift+enter",
            "ctrl+shift+space",
            "ctrl+,",
            "ctrl+shift+pageup",
        ] {
            assert_eq!(chord(code).code(), code);
        }
        assert_eq!(chord("ctrl+shift+s").name(), "Ctrl+Shift+S");
        assert_eq!(
            (chord("alt+\\").name(), chord("ctrl+space").name(), chord("f12").name()),
            ("Alt+\\".into(), "Ctrl+Space".into(), "F12".into())
        );
        // Shift does not count on a symbol, so `/` is the same key everywhere.
        let shifted = Mods { ctrl: true, shift: true, alt: false };
        assert_eq!(Chord::new(Key::Char('/'), shifted), chord("ctrl+/"));
        assert_ne!(Chord::new(Key::Char('s'), shifted), chord("ctrl+s"));
        assert!(Chord::parse("ctrl+").is_none() && Chord::parse("f13").is_none() && Chord::parse("ctrl+sx").is_none());
        assert!(chord("ctrl+j").assignable() && chord("f5").assignable() && chord("shift+enter").assignable());
        assert!(!Chord::new(Key::Char('j'), Mods::default()).assignable());
        assert!(!Chord::new(Key::Left, Mods { ctrl: true, shift: false, alt: false }).assignable());
    }

    #[test]
    fn every_default_is_sound() {
        for (i, a) in ACTIONS.iter().enumerate() {
            let own = defaults(a);
            assert!(own.len() == a.keys.len() && own.iter().all(Chord::assignable), "{}", a.id);
            // No two shortcuts that could both apply share a key.
            for b in ACTIONS[i + 1..].iter().filter(|b| b.scope.meets(a.scope)) {
                assert!(!defaults(b).iter().any(|c| own.contains(c)), "{} and {}", a.id, b.id);
            }
        }
    }

    #[test]
    fn keys_move_between_actions() {
        let all = |_: Scope| true;
        let map = Keymap::new(&BTreeMap::new());
        assert_eq!(map.meaning(chord("ctrl+s"), all), Meaning::Itself);
        assert_eq!(map.meaning(chord("ctrl+y"), all), Meaning::Default(chord("ctrl+shift+z")));

        // Find moved to Ctrl+U: the new key finds, the old one does nothing.
        let map = Keymap::new(&BTreeMap::from([("find".to_string(), vec!["ctrl+u".to_string()])]));
        assert_eq!(map.meaning(chord("ctrl+u"), all), Meaning::Default(chord("ctrl+f")));
        assert_eq!(map.meaning(chord("ctrl+f"), all), Meaning::Nothing);
        assert!(map.has("find", chord("ctrl+u")) && !map.has("find", chord("ctrl+f")));

        // A key means its action only where the action applies, so one key
        // can serve the notebook and the sidebar alike.
        let moved = |id: &str| (id.to_string(), vec!["ctrl+u".to_string()]);
        let map = Keymap::new(&BTreeMap::from([moved("run_next"), moved("threads")]));
        let in_chat = |s: Scope| matches!(s, Scope::Always | Scope::Main | Scope::Chat);
        let in_notebook = |s: Scope| !matches!(s, Scope::Chat);
        assert_eq!(map.meaning(chord("shift+enter"), in_chat), Meaning::Itself);
        assert_eq!(map.meaning(chord("shift+enter"), in_notebook), Meaning::Nothing);
        assert_eq!(map.meaning(chord("ctrl+u"), in_notebook), Meaning::Default(chord("shift+enter")));
        assert_eq!(map.meaning(chord("ctrl+u"), in_chat), Meaning::Default(chord("ctrl+t")));

        // An action left without keys.
        let map = Keymap::new(&BTreeMap::from([("threads".to_string(), Vec::new())]));
        assert_eq!(map.meaning(chord("ctrl+t"), in_chat), Meaning::Nothing);
    }
}
