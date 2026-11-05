//! The terminal at work in the window: showing and hiding its panel, the
//! keys that reach the shell, and what the shell has done meanwhile.

use std::path::Path;

use super::{App, Focus, Key, Mods};
use crate::keys::Chord;
use crate::term::{Link, Term};

/// A path as the shell reads it, quoted whatever it holds.
pub(super) fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

impl App {
    pub(super) fn terminal_find_step(&mut self, previous: bool) {
        if let Some(t) = self.term_mut() {
            let query = t.search.clone();
            if query.is_empty() {
                return self.ask(super::PromptKind::TerminalFind);
            }
            let (at, total) = t.find_history(&query, previous);
            self.message = Some(format!("Terminal match {at} of {total}"));
            self.redraw = true;
        } else {
            self.message = Some("Open a terminal first".into());
        }
    }

    /// The terminal shown in the panel, if any is open.
    pub(super) fn term(&self) -> Option<&Term> {
        self.terms.get(self.term_cur)
    }

    pub(super) fn term_mut(&mut self) -> Option<&mut Term> {
        self.terms.get_mut(self.term_cur)
    }

    /// The terminals on show: the two side by side, or the one.
    pub(super) fn shown_terms(&self) -> Vec<usize> {
        match self.term_panes {
            Some(pair) => pair.to_vec(),
            None if self.term_cur < self.terms.len() => vec![self.term_cur],
            None => Vec::new(),
        }
    }

    /// The terminal on show under a point, or whose top edge it is on.
    pub(super) fn term_under(&self, x: f32, y: f32) -> Option<&Term> {
        if !self.term_open {
            return None;
        }
        let shown = self.shown_terms();
        shown.into_iter().map(|i| &self.terms[i]).find(|t| t.contains(x, y) || t.on_edge(x, y))
    }

    /// Makes the terminal on show under a point the one with the keys.
    pub(super) fn term_aim(&mut self, x: f32, y: f32) {
        if !self.term_open {
            return;
        }
        if let Some(i) = self.shown_terms().into_iter().find(|&i| self.terms[i].contains(x, y))
            && i != self.term_cur
        {
            self.terms[self.term_cur].set_focus(false);
            self.term_cur = i;
        }
    }

    /// Shows a terminal in the pane of the one with the keys, and gives
    /// it the keys.
    pub(super) fn show_in_pane(&mut self, index: usize) {
        if let Some(pair) = &mut self.term_panes
            && !pair.contains(&index)
        {
            let slot = if pair[0] == self.term_cur { 0 } else { 1 };
            pair[slot] = index;
        }
        self.term_cur = index;
    }

    /// Shows two terminals side by side, starting a second if there is
    /// only one; or one again.
    pub(super) fn toggle_split(&mut self) {
        if self.term_panes.take().is_some() {
            return self.show_term(true);
        }
        if self.terms.is_empty() {
            self.show_term(true);
        }
        let first = self.term_cur;
        let second = if self.terms.len() < 2 {
            let dir = self.terms[first].cwd();
            self.new_term(&dir);
            self.term_cur
        } else {
            (first + 1) % self.terms.len()
        };
        self.term_panes = Some([first, second]);
        self.terms[first].set_focus(false);
        self.term_cur = second;
        self.show_term(true);
    }

    /// Whether keys go to the terminal.
    pub(super) fn term_has_keys(&self) -> bool {
        self.focus == Focus::Term
            && self.term_open
            && self.term().is_some()
            && self.prompt.is_none()
            && !self.overlaid()
    }

    /// What each terminal's tab says: its number, and the program's
    /// title, shortened to the last folder where it names one, as shells
    /// do with `user@host:~/some/folder`.
    pub(super) fn term_names(&self) -> Vec<String> {
        let name = |(i, t): (usize, &Term)| {
            let title = t.title();
            let path = title.rsplit_once(':').map_or(title.as_str(), |(_, p)| p).trim();
            let short = match Path::new(path).file_name() {
                Some(last) if path.starts_with(['/', '~']) => last.to_string_lossy().into_owned(),
                _ if path == "~" || path == "/" => path.to_string(),
                _ if title.is_empty() => "shell".to_string(),
                _ => title,
            };
            format!("{}  {short}", i + 1)
        };
        self.terms.iter().enumerate().map(name).collect()
    }

    /// Starts another shell in a folder and makes it the one shown.
    fn new_term(&mut self, dir: &Path) {
        if let Some(t) = self.term_mut() {
            t.set_focus(false);
        }
        self.terms.push(Term::new_in(dir, &self.root, self.fonts.clone(), self.waker.clone()));
        self.show_in_pane(self.terms.len() - 1);
    }

    /// Shows the terminal and gives it the keyboard, starting a shell in
    /// the project if there is none; or hides it, the shells running on.
    pub(super) fn show_term(&mut self, open: bool) {
        self.comp = None;
        if open {
            self.unsuggest();
            if self.terms.is_empty() {
                let root = self.root.clone();
                self.new_term(&root);
            }
            self.focus = Focus::Term;
        } else if self.focus == Focus::Term {
            self.focus = Focus::Doc;
        }
        self.term_open = open;
        self.arrange();
        self.redraw = true;
    }

    /// Goes to the terminal, or from it back to the document.
    pub(super) fn toggle_term(&mut self) {
        if self.term_open && self.focus == Focus::Term {
            self.focus = Focus::Doc;
        } else {
            self.show_term(true);
        }
    }

    /// Opens another terminal in the project, beside those already open.
    pub(super) fn add_term(&mut self) {
        if self.ask_terminal_root() {
            return;
        }
        let root = self.root.clone();
        self.new_term(&root);
        self.show_term(true);
    }

    /// Shows the terminal at an index and gives it the keyboard.
    pub(super) fn switch_term(&mut self, index: usize) {
        if index >= self.terms.len() {
            return;
        }
        if index != self.term_cur
            && let Some(t) = self.term_mut()
        {
            t.set_focus(false);
        }
        self.show_in_pane(index);
        self.term_close_armed = None;
        self.show_term(true);
    }

    /// Goes to the next terminal, or with a negative step the previous.
    pub(super) fn cycle_term(&mut self, step: isize) {
        let count = self.terms.len() as isize;
        if count > 0 {
            self.switch_term((self.term_cur as isize + step).rem_euclid(count) as usize);
        }
    }

    /// Closes a terminal, ending its shell. A program at work in it needs
    /// asking twice.
    pub(super) fn close_term(&mut self, index: usize) {
        let Some(t) = self.terms.get(index) else { return };
        if t.running() && !t.idle() && self.term_close_armed != Some(index) {
            self.term_close_armed = Some(index);
            self.message = Some("A program is running in this terminal. Closing it again ends it".into());
            return;
        }
        self.remove_term(index);
    }

    /// Takes a terminal out of the panel; the panel closes with the last.
    fn remove_term(&mut self, index: usize) {
        self.terms.remove(index);
        self.term_close_armed = None;
        // Split, the other pane's terminal is shown alone.
        self.term_panes = match self.term_panes {
            Some(pair) if pair.contains(&index) => {
                let other = if pair[0] == index { pair[1] } else { pair[0] };
                // Numbered as before the removal, as the lines below expect.
                self.term_cur = other;
                None
            }
            Some(pair) => Some(pair.map(|i| if i > index { i - 1 } else { i })),
            None => None,
        };
        if self.term_cur > index || self.term_cur >= self.terms.len() {
            self.term_cur = self.term_cur.saturating_sub(1);
        }
        if self.terms.is_empty() {
            if self.term_open {
                self.show_term(false);
            }
        } else {
            self.arrange();
        }
        self.redraw = true;
    }

    pub(super) fn restart_term(&mut self) {
        if let Some(t) = self.term_mut() {
            t.restart();
        }
        self.focus = Focus::Term;
    }

    /// Gets a shell ready to be typed at in a folder: the one shown, taken
    /// there with `cd` if it is waiting for a command, or else a new one
    /// started there.
    fn shell_in(&mut self, dir: &Path) {
        match self.terms.get_mut(self.term_cur) {
            Some(t) if t.running() && t.idle() => {
                // The space before it keeps it out of the shell's history.
                t.run(&format!(" cd -- {}", quote(dir)));
            }
            // A shell that could not start is replaced.
            Some(t) if !t.running() => *t = Term::new_in(dir, &self.root, self.fonts.clone(), self.waker.clone()),
            _ => self.new_term(dir),
        }
        self.show_term(true);
    }

    pub(super) fn selected_text_range(&self) -> (usize, usize) {
        if self.doc.buf.has_sel() { self.doc.buf.sel() }
        else { self.doc.buf.line(self.doc.buf.line_of(self.doc.buf.cur)) }
    }
    pub(super) fn terminal_run_selection(&mut self) {
        let (a, b) = self.selected_text_range();
        let text = self.doc.buf.text[a..b].to_string();
        self.show_term(true);
        if let Some(t) = self.term_mut() { t.paste(&text); t.key(Key::Enter, Mods::default(), None); }
        self.message = Some("Sent text to terminal".into());
    }

    /// Opens the terminal in a folder.
    pub(super) fn term_at(&mut self, dir: &Path) {
        self.shell_in(dir);
    }

    /// Runs a command in the terminal, in a folder, as if it were typed.
    pub(super) fn term_run(&mut self, dir: &Path, command: &str) {
        self.shell_in(dir);
        if let Some(t) = self.term_mut() {
            t.run(command);
        }
    }

    /// A key typed while the terminal has the keyboard. Returns false for
    /// the keys that are the window's even there: the terminal's own
    /// shortcuts, and Ctrl+Shift with a key, which programs do not tell
    /// from Ctrl with it.
    pub(super) fn term_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        let chord = Chord::new(key, mods);
        let own = [
            "back",
            "forward",
            "project_tasks",
            "next_error",
            "previous_error",
            "terminal",
            "terminal_hide",
            "terminal_new",
            "terminal_close",
            "terminal_next",
            "terminal_previous",
            "terminal_split",
        ];
        if own.iter().any(|id| self.keymap.has(id, chord)) {
            return false;
        }
        let Some(t) = self.terms.get_mut(self.term_cur) else { return false };
        match key {
            Key::Char('c') if mods.ctrl && mods.shift => self.copy = t.selected(),
            Key::Char('v') if mods.ctrl && mods.shift => self.paste = true,
            Key::PageUp | Key::PageDown if mods.shift && !mods.ctrl && !mods.alt => t.page(key == Key::PageUp),
            Key::Home | Key::End if mods.shift && !mods.ctrl && !mods.alt => t.scroll_edge(key == Key::Home),
            Key::Up | Key::Down if mods.ctrl && mods.shift && !mods.alt => return self.shell_mark_key(key == Key::Up),
            _ if mods.ctrl && mods.shift => return false,
            _ => {
                t.key(key, mods, text);
            }
        }
        self.term_close_armed = None;
        self.message = None;
        true
    }

    /// Opens what was printed in the terminal at a point, if it can be
    /// opened. Returns whether there was something.
    pub(super) fn term_link(&mut self, x: f32, y: f32) -> bool {
        let Some(link) = self.term().and_then(|t| t.link_at(x, y)) else { return false };
        match link {
            Link::Url(url) => self.follow(&url),
            Link::File(path, line, col) => {
                self.jump_to(path.clone(), 0, 0);
                if let Some(line) = line.filter(|_| self.doc.path.as_deref() == Some(path.as_path())) {
                    let buf = &self.doc.buf;
                    let (start, end) = buf.line(line.saturating_sub(1).min(buf.lines().saturating_sub(1)));
                    let col = col.unwrap_or(1).saturating_sub(1);
                    let at = buf.text[start..end].char_indices().nth(col).map_or(end, |(i, _)| start + i);
                    self.doc.select(at, at);
                    self.doc.moved();
                }
            }
        }
        true
    }

    /// Takes in what the shells have done.
    pub(super) fn term_poll(&mut self) {
        let mut gone = Vec::new();
        for (i, t) in self.terms.iter_mut().enumerate() {
            let news = t.poll();
            if let Some(text) = news.clipboard {
                self.copy = Some(text);
            }
            if news.gone && t.job_name.is_none() {
                gone.push(i);
            }
        }
        for &i in gone.iter().rev() {
            self.remove_term(i);
        }
        if !gone.is_empty() {
            self.message = Some(if self.terms.is_empty() {
                "The terminal's shell has ended".into()
            } else {
                "A terminal's shell has ended".into()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::Canvas;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    fn draw(a: &mut App) {
        let mut px = vec![0u32; 900 * 700];
        a.draw(&mut Canvas::new(&mut px, 900, 700));
    }

    #[test]
    fn the_terminal_takes_the_keys_and_gives_them_back() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("text");
        a.key(Key::Char('`'), CTRL, None);
        assert!(a.term_open && a.focus == Focus::Term);
        draw(&mut a);
        // The document is shorter by the panel, which sits under it.
        let t = a.term().unwrap();
        assert!(t.contains(10.0, 700.0 - 40.0) && !a.doc.contains(10.0, 700.0 - 40.0));
        a.term_mut().unwrap().settle();
        // What is typed goes to the shell, not into the document.
        for ch in "echo hi-there".chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
        a.key(Key::Enter, NONE, None);
        a.term_mut().unwrap().settle();
        assert_eq!(a.doc.buf.text, "text");
        assert!(a.term().unwrap().screen_text().contains("\nhi-there"));
        // Ctrl+C is the shell's; the toggle hands the keyboard back.
        a.key(Key::Char('c'), CTRL, Some("\x03"));
        assert!(a.copy.is_none());
        a.key(Key::Char('j'), CTRL, None);
        assert!(a.term_open && a.focus == Focus::Doc);
        a.key(Key::Char('x'), NONE, Some("x"));
        assert_eq!(a.doc.buf.text, "textx");
        // A click on the panel gives it the keyboard again; Hide hides it.
        let (x, y) = (20.0, 700.0 - 60.0);
        a.press(x, y, NONE, 1);
        a.release();
        assert!(a.focus == Focus::Term);
        a.key(Key::Char('`'), Mods { ctrl: true, shift: false, alt: true }, None);
        assert!(!a.term_open && a.focus == Focus::Doc && !a.terms.is_empty());
        // The shell ending closes the panel.
        a.key(Key::Char('`'), CTRL, None);
        a.term_mut().unwrap().settle();
        a.term_mut().unwrap().run("exit");
        let start = std::time::Instant::now();
        while !a.terms.is_empty() && start.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            a.poll();
        }
        assert!(a.terms.is_empty() && !a.term_open && a.focus == Focus::Doc);
    }

    fn type_line(a: &mut App, line: &str) {
        for ch in line.chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
        a.key(Key::Enter, NONE, None);
    }

    #[test]
    fn several_terminals_are_kept_apart() {
        const CS: Mods = Mods { ctrl: true, shift: true, alt: false };
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.key(Key::Char('`'), CTRL, None);
        a.term_mut().unwrap().settle();
        type_line(&mut a, "echo first-one");
        a.term_mut().unwrap().settle();
        // Ctrl+Shift+T opens a second shell, which takes the keyboard.
        a.key(Key::Char('t'), CS, None);
        assert_eq!((a.terms.len(), a.term_cur), (2, 1));
        assert!(a.term_open && a.focus == Focus::Term);
        draw(&mut a);
        a.term_mut().unwrap().settle();
        type_line(&mut a, "echo second-one");
        a.term_mut().unwrap().settle();
        assert!(a.term().unwrap().screen_text().contains("\nsecond-one"));
        assert!(!a.term().unwrap().screen_text().contains("\nfirst-one"));
        // The list beside them names both; a click on the first goes to it.
        draw(&mut a);
        let r =
            a.hits.iter().find(|(_, h)| *h == super::super::Hit::TermTab(0)).expect("a row for the first terminal").0;
        a.press(r.0 + 5.0, r.1 + 8.0, NONE, 1);
        a.release();
        assert_eq!(a.term_cur, 0);
        assert!(a.term().unwrap().screen_text().contains("\nfirst-one"));
        // Ctrl+Shift+PgDn cycles; Ctrl+Shift+W closes the one shown.
        a.key(Key::PageDown, CS, None);
        assert_eq!(a.term_cur, 1);
        a.key(Key::Char('w'), CS, None);
        assert_eq!((a.terms.len(), a.term_cur), (1, 0));
        assert!(a.term_open && a.term().unwrap().screen_text().contains("\nfirst-one"));
        // One shell ending leaves the others; the last one closes the panel.
        a.key(Key::Char('t'), CS, None);
        draw(&mut a);
        a.term_mut().unwrap().settle();
        a.term_mut().unwrap().run("exit");
        let start = std::time::Instant::now();
        while a.terms.len() > 1 && start.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            a.poll();
        }
        assert_eq!((a.terms.len(), a.term_cur), (1, 0));
        assert!(a.term_open);
        a.key(Key::Char('w'), CS, None);
        assert!(a.terms.is_empty() && !a.term_open && a.focus == Focus::Doc);
    }

    #[test]
    fn two_terminals_side_by_side() {
        const CS: Mods = Mods { ctrl: true, shift: true, alt: false };
        let mut a = App::new(None);
        a.resize(1000, 700, 1.0);
        a.key(Key::Char('j'), CS, None);
        assert_eq!((a.terms.len(), a.term_panes, a.term_cur), (2, Some([0, 1]), 1));
        draw(&mut a);
        let (left, right) = (&a.terms[0], &a.terms[1]);
        let y = 700.0 - 60.0;
        assert!(left.contains(10.0, y) && !left.contains(990.0, y));
        assert!(right.contains(990.0, y) && !right.contains(10.0, y));
        // A click on the left pane gives it the keys.
        a.press(20.0, y, NONE, 1);
        a.release();
        assert_eq!((a.term_cur, a.focus == Focus::Term), (0, true));
        // A third terminal takes the place of the one with the keys.
        a.key(Key::Char('t'), CS, None);
        assert_eq!((a.term_panes, a.term_cur), (Some([2, 1]), 2));
        // Closing a shown one leaves the other alone, numbered anew.
        a.key(Key::Char('w'), CS, None);
        assert_eq!((a.terms.len(), a.term_panes, a.term_cur), (2, None, 1));
        a.key(Key::Char('j'), CS, None);
        assert_eq!(a.term_panes, Some([1, 0]));
        a.key(Key::Char('j'), CS, None);
        assert_eq!((a.term_panes, a.term_cur), (None, 0));
    }

    #[test]
    fn paths_are_quoted_for_the_shell() {
        assert_eq!(quote(Path::new("/a b/it's")), "'/a b/it'\\''s'");
    }
}

impl App {
    pub(super) fn agent_terminal(&mut self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        if name == "start_terminal" {
            if self.terms.iter().filter(|t| t.agent_owner.is_some()).count() >= 8 { return Err("At most eight agent command terminals may run".into()); }
            let command = args["command"].as_str().filter(|c| !c.trim().is_empty() && !c.contains('\0')).ok_or("Expected a shell command")?;
            self.checkpoint_command()?;
            let root = self.root.clone();
            self.new_term(&root);
            self.show_term(true);
            let owner = self.ai.thread.id.clone();
            let t = self.term_mut().ok_or("Terminal could not start")?;
            t.agent_owner = Some(owner);
            let id = t.agent_id;
            t.run(command);
            self.focus = Focus::Chat;
            return Ok(format!("Started visible terminal {id}. Use read_terminal with this ID to read output or stop_terminal to end it. The user can see and close this terminal."));
        }
        let id = args["id"].as_u64().ok_or("Expected terminal id")?;
        let i = self.terms.iter().position(|t| t.agent_id == id && t.agent_owner.as_deref() == Some(&self.ai.thread.id)).ok_or("This thread has no such command terminal; it may have exited or been closed")?;
        match name {
            "read_terminal" => Ok(self.terms[i].recent_text(20_000)),
            "stop_terminal" => { self.remove_term(i); self.refresh_agent_changes(); Ok(format!("Stopped terminal {id}")) }
            _ => Err("Unknown terminal tool".into()),
        }
    }
    pub(super) fn show_agent_terminal(&mut self) {
        if let Some(i) = self.terms.iter().position(|t| t.agent_owner.as_deref() == Some(&self.ai.thread.id)) {
            self.switch_term(i);
        } else { self.message = Some("This thread has no running command terminals".into()); }
    }
}
#[cfg(test)]
mod agent_terminal_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn visible_commands_have_stable_ids_and_cannot_read_user_shells() {
        let dir = std::env::temp_dir().join(format!("ld-visible-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let mut a = App::new(Some(dir.clone()));
        a.resize(900, 700, 1.0);
        a.agent_terminal("start_terminal", &json!({"command":"printf 'watcher-ready\\n'; sleep 30"})).unwrap();
        assert!(a.term_open);
        let id = a.term().unwrap().agent_id;
        a.term_mut().unwrap().settle();
        assert!(a.agent_terminal("read_terminal", &json!({"id":id})).unwrap().contains("watcher-ready"));
        a.add_term();
        let user = a.term().unwrap().agent_id;
        assert!(a.agent_terminal("read_terminal", &json!({"id":user})).is_err());
        a.agent_terminal("stop_terminal", &json!({"id":id})).unwrap();
        assert_eq!(a.terms.len(), 1);
        assert_eq!(a.term().unwrap().agent_id, user);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
