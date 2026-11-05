//! Commands that use the marks a shell puts on its prompts and commands:
//! going from prompt to prompt, taking the last command's output and
//! running the last command again.

use super::App;

impl App {
    /// Ctrl+Shift+Up or Down in the terminal: to the previous or next
    /// prompt. Always taken, so the window does not act on it instead.
    pub(super) fn shell_mark_key(&mut self, up: bool) -> bool {
        self.shell_mark_command(if up { "terminal_command_previous" } else { "terminal_command_next" });
        true
    }

    pub(super) fn shell_mark_command(&mut self, id: &str) {
        let Some(t) = self.term_mut() else {
            self.message = Some("Open a terminal first".into());
            return;
        };
        if !t.has_marks() {
            self.message = Some("This shell does not mark its commands; see Shell integration in the README".into());
            return;
        }
        let said = match id {
            "terminal_command_previous" | "terminal_command_next" => {
                (!t.command_jump(id == "terminal_command_previous")).then(|| "No more commands that way".to_string())
            }
            "terminal_output_copy" | "terminal_output_select" => match t.select_last_output() {
                Some(text) if text.trim().is_empty() => Some("The last command printed nothing".into()),
                Some(text) if id == "terminal_output_copy" => {
                    let lines = text.lines().count();
                    self.copy = Some(text);
                    Some(format!("Copied {lines} line{} of output", if lines == 1 { "" } else { "s" }))
                }
                Some(_) => None,
                None => Some("No command has finished yet".into()),
            },
            _ => match t.last_command_line() {
                None => Some("No command has run yet".into()),
                Some(_) if !t.idle() => Some("A program is still running in the terminal".into()),
                Some(line) => {
                    t.run(&line);
                    None
                }
            },
        };
        self.message = said;
        self.redraw = true;
    }
}
