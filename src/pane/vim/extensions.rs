//! Register-backed macros and commands shared by the modal keymaps.
use super::*;

impl Pane {
    pub(crate) fn set_modal_editing(&mut self, mode: &str) {
        let enabled = mode != "none";
        let helix = mode == "helix";
        if self.vim.enabled == enabled && self.vim.helix.enabled == helix {
            return;
        }
        if matches!(self.vim.mode, Mode::Insert | Mode::Replace) {
            self.vim_leave_insert();
        }
        self.vim_normal();
        self.vim.pending.clear();
        self.vim.command = None;
        self.vim.enabled = enabled;
        self.vim.helix = helix::Helix::default();
        self.vim.helix.enabled = helix;
        self.redraw = true;
    }

    pub(super) fn vim_macro_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if self.vim.enabled && self.vim.macro_depth == 0 && !self.vim.replaying {
            let stop = self.vim.mode == Mode::Normal
                && self.vim.pending.is_empty()
                && self.vim.command.is_none()
                && !mods.ctrl
                && !mods.alt
                && Self::vim_char(key, mods, text, false) == Some('q');
            if stop && let Some(reg) = self.vim.macro_register.take() {
                let value = std::mem::take(&mut self.vim.macro_text);
                self.vim.registers.insert(reg, (value, false));
                self.redraw = true;
                return true;
            }
            if self.vim.macro_register.is_some() {
                let c = match key {
                    Key::Escape => Some('\u{1b}'),
                    Key::Enter => Some('\n'),
                    Key::Tab => Some('\t'),
                    Key::Backspace => Some('\u{8}'),
                    Key::Char(c) if mods.ctrl && !mods.alt && c.is_ascii() => char::from_u32(c as u32 & 31),
                    _ => Self::vim_char(key, mods, text, true),
                };
                if let Some(c) = c {
                    self.vim.macro_text.push(c);
                }
            }
        }
        self.vim_key_inner(key, mods, text)
    }

    pub(super) fn vim_extended(&mut self, command: &str, count: Option<usize>) -> bool {
        let n = count.unwrap_or(1).max(1);
        match command {
            "gv" => {
                if let Some((mode, a, b)) = self.vim.last_visual {
                    let boundary = |p: usize| {
                        let mut p = p.min(self.buf.text.len());
                        while !self.buf.text.is_char_boundary(p) {
                            p -= 1;
                        }
                        p
                    };
                    self.vim.mode = mode;
                    self.vim.anchor = boundary(a);
                    self.vim.cursor = boundary(b);
                    self.vim_show();
                }
            }
            "g;" | "g," => {
                let len = self.vim.changes.len();
                if len > 0 {
                    self.vim.change_index = if command == "g;" {
                        self.vim.change_index.saturating_sub(n)
                    } else {
                        (self.vim.change_index + n).min(len - 1)
                    };
                    let mut at = self.vim.changes[self.vim.change_index].min(self.buf.text.len());
                    while !self.buf.text.is_char_boundary(at) {
                        at -= 1;
                    }
                    self.vim_go(at);
                }
            }
            "zt" | "zz" | "zb" | "z\n" | "z." | "z-" => {
                if let Some(line) = count {
                    self.vim_go(self.buf.starts[line.saturating_sub(1).min(self.buf.lines() - 1)]);
                }
                let line = self.buf.line_of(self.buf.cur);
                if matches!(command, "z\n" | "z." | "z-") {
                    self.vim_go(first_non_blank(&self.buf.text, self.buf.starts[line]));
                }
                self.top = line;
                self.top_off = 0.0;
                let offset = match command {
                    "zz" | "z." => self.h / 2.0,
                    "zb" | "z-" => self.h - self.height(line),
                    _ => 0.0,
                };
                self.scroll(-offset.max(0.0));
            }
            _ if command.starts_with('q') => {
                let Some(reg) = command.chars().nth(1).filter(char::is_ascii_alphabetic) else {
                    return true;
                };
                self.vim.macro_text = if reg.is_ascii_uppercase() {
                    self.vim.registers.get(&reg.to_ascii_lowercase()).map(|r| r.0.clone()).unwrap_or_default()
                } else {
                    String::new()
                };
                self.vim.macro_register = Some(reg.to_ascii_lowercase());
            }
            _ if command.starts_with('@') => {
                let reg = command.chars().nth(1).unwrap_or('@');
                let reg = if reg == '@' { self.vim.last_macro.unwrap_or('@') } else { reg.to_ascii_lowercase() };
                if self.vim.macro_depth >= 32 {
                    self.note = Some("Macro recursion limit reached".into());
                    return true;
                }
                let Some((body, _)) = self.vim.registers.get(&reg).cloned() else {
                    return true;
                };
                self.vim.last_macro = Some(reg);
                if self.vim.macro_depth == 0 {
                    self.vim.macro_steps = 0;
                }
                self.vim.macro_depth += 1;
                let checkpoint = self.buf.checkpoint();
                'play: for _ in 0..n {
                    for c in body.chars() {
                        self.vim.macro_steps += 1;
                        if self.vim.macro_steps > 10_000 {
                            self.note = Some("Macro instruction limit reached".into());
                            break 'play;
                        }
                        let (key, mods, text) = match c {
                            '\u{1b}' => (Key::Escape, Mods::default(), None),
                            '\n' => (Key::Enter, Mods::default(), None),
                            '\t' => (Key::Tab, Mods::default(), None),
                            '\u{8}' => (Key::Backspace, Mods::default(), None),
                            c if c.is_ascii_control() => (
                                Key::Char(char::from_u32(c as u32 + 96).unwrap()),
                                Mods { ctrl: true, ..Mods::default() },
                                None,
                            ),
                            c => (Key::Char(c.to_ascii_lowercase()), Mods::default(), Some(c.to_string())),
                        };
                        self.key(key, mods, text.as_deref());
                    }
                }
                self.vim.macro_depth -= 1;
                self.buf.finish(checkpoint);
            }
            _ => return false,
        }
        true
    }

    pub(super) fn vim_note_change(&mut self) {
        if self.vim.changes.last() != Some(&self.buf.cur) {
            self.vim.changes.push(self.buf.cur);
            if self.vim.changes.len() > 100 {
                self.vim.changes.remove(0);
            }
        }
        self.vim.change_index = self.vim.changes.len();
    }

    pub(super) fn vim_scroll(&mut self, key: Key, count: Option<usize>) {
        let rows = (self.h / (self.px() * theme::LINE_HEIGHT)).max(2.0) as usize;
        let Key::Char(c) = key else {
            return;
        };
        let down = matches!(c, 'e' | 'd' | 'f');
        let n = match c {
            'e' | 'y' => count.unwrap_or(1),
            'd' | 'u' => count.unwrap_or(rows / 2),
            _ => rows.saturating_sub(2).max(1) * count.unwrap_or(1),
        };
        if !matches!(c, 'e' | 'y') {
            let to = vertical(&self.buf.text, self.buf.cur, n, down, column(&self.buf.text, self.buf.cur));
            self.vim_go(to);
        }
        self.scroll_lines(if down { n as f32 } else { -(n as f32) });
        let first = self.vim_screen_line(Motion::ScreenTop, 1);
        let last = self.vim_screen_line(Motion::ScreenBottom, 1);
        let line = self.buf.line_of(self.buf.cur);
        if line < first || line > last {
            self.vim_go(at_column(
                &self.buf.text,
                self.buf.starts[line.clamp(first, last)],
                column(&self.buf.text, self.buf.cur),
            ));
        }
        self.vim_settle();
    }

    /// Record a completion's actual edit instead of replaying its popup keys.
    pub(crate) fn vim_completion(&mut self, before: &str, cursor: usize) {
        if !self.vim.enabled || self.vim.mode != Mode::Insert || self.vim.replaying || before == self.buf.text {
            return;
        }
        let after = &self.buf.text;
        let mut start = before.bytes().zip(after.bytes()).take_while(|(a, b)| a == b).count();
        while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
            start -= 1;
        }
        let mut suffix =
            before[start..].bytes().rev().zip(after[start..].bytes().rev()).take_while(|(a, b)| a == b).count();
        while !before.is_char_boundary(before.len() - suffix) || !after.is_char_boundary(after.len() - suffix) {
            suffix -= 1;
        }
        if start > cursor || before.len() - suffix < cursor {
            return;
        }
        let back = before[start..cursor].chars().count();
        let forward = before[cursor..before.len() - suffix].chars().count();
        let payload = serde_json::json!([back, forward, &after[start..after.len() - suffix]]).to_string();
        let stroke = (Key::Other, Mods { ctrl: true, alt: true, shift: false }, Some(payload));
        self.vim.inserted.push(stroke.clone());
        if let Some(recording) = &mut self.vim.recording {
            recording.push(stroke);
        }
    }
}

impl Pane {
    pub(super) fn vim_global(&mut self, command: &str, (l0, l1): (usize, usize)) {
        let inverse = command.starts_with('v') || command.starts_with("g!");
        let body = if command.starts_with("g!") { &command[2..] } else { &command[1..] };
        let Some(delim) = body.chars().next() else { return };
        let mut pattern = String::new();
        let mut escaped = false;
        let mut end = None;
        for (i, c) in body[delim.len_utf8()..].char_indices() {
            if escaped {
                if c != delim {
                    pattern.push('\\');
                }
                pattern.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == delim {
                end = Some(i + 2 * delim.len_utf8());
                break;
            } else {
                pattern.push(c);
            }
        }
        let Some(end) = end else {
            self.note = Some("Write :g/pattern/command".into());
            return;
        };
        let re = match regex::Regex::new(&pattern) {
            Ok(re) => re,
            Err(e) => {
                self.note = Some(e.to_string());
                return;
            }
        };
        let action = body[end..].trim();
        if !matches!(action, "d" | "delete" | "y" | "yank" | "p" | "print" | "") && !action.starts_with('s') {
            self.note = Some("Global supports d, y, p and s commands".into());
            return;
        }
        let hits: Vec<_> = (l0..=l1)
            .filter(|&l| {
                let (a, b) = self.buf.line(l);
                re.is_match(&self.buf.text[a..b]) != inverse
            })
            .collect();
        if matches!(action, "p" | "print" | "" | "y" | "yank") {
            let text = hits
                .iter()
                .map(|&l| {
                    let (a, b) = self.buf.line(l);
                    &self.buf.text[a..b]
                })
                .collect::<Vec<_>>()
                .join("\n");
            if matches!(action, "y" | "yank") {
                self.vim_store(None, format!("{text}\n"), true, true);
            } else {
                self.note = Some(text);
            }
            return;
        }
        let checkpoint = self.buf.checkpoint();
        for l in hits.into_iter().rev() {
            if action.starts_with('s') {
                self.vim_substitute(action, (l, l));
            } else {
                let (a, b) = self.buf.line(l);
                self.vim_apply(Op::Delete, (a, b, true), None, 1);
            }
        }
        self.buf.finish(checkpoint);
        self.vim_settle();
    }
}
