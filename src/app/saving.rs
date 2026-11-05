//! Save policies and optional stdin/stdout formatters, away from the UI thread.
use super::*;
use crate::lsp;
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver};

pub(super) struct Formatting {
    tab: u64,
    version: u64,
    path: PathBuf,
    result: Receiver<Result<String, String>>,
}
#[derive(Default)]
pub(super) struct Saving {
    pending: Option<Formatting>,
    formatted: bool,
}
impl App {
    pub(super) fn prepare_save(&mut self) -> bool {
        if self.nb.is_some() {
            return true;
        }
        let Some(path) = self.doc.path.clone() else { return true };
        let rules = Rules::read(&path);
        let text = rules.normalize(&self.doc.buf.text);
        if rules.charset.as_deref().is_some_and(|s| !matches!(s, "utf-8" | "utf-8-bom")) {
            self.message = Some("Cannot save with this .editorconfig charset; use utf-8".into());
            return false;
        }
        if !std::mem::take(&mut self.saving.formatted) {
            let kinds = self.cfg.code_actions_on_save_for(&self.root);
            let command = if self.cfg.format_on_save_for(&self.root) {
                self.cfg.formatter_for(&self.root, &ext(&path)).filter(|s| !s.trim().is_empty()).map(str::to_string)
            } else { None };
            if !kinds.is_empty() || command.is_some() {
                if self.saving.pending.is_some() {
                    self.message = Some("Save preparation is still running".into());
                    return false;
                }
                let (tx, rx) = mpsc::channel();
                let wake = self.waker.clone();
                let dir = self.root.clone();
                let source = self.doc.buf.text.clone();
                // A synchronized running server can answer save actions at once.
                if !kinds.is_empty() { self.language_sync(); }
                self.saving.pending = Some(Formatting {
                    tab: self.file_tabs[self.active_file].id,
                    version: self.doc.buf.version,
                    path:path.clone(),
                    result: rx,
                });
                std::thread::spawn(move || {
                    let result = (|| {
                        let source = if kinds.is_empty() { source } else { lsp::save_actions(&dir,&path,&source,&kinds,wake.clone())? };
                        let text = Rules::read(&path).normalize(&source);
                        if let Some(command) = command { format_text(&dir,&command,&text) } else { Ok(text) }
                    })();
                    let _ = tx.send(result);
                    wake();
                });
                self.message = Some("Preparing save…".into());
                return false;
            }
        }
        self.apply_saved_text(&text);
        match rules.eol.as_deref() {
            Some("crlf") => self.doc.buf.eol = editor::Eol::Crlf,
            Some("lf") => self.doc.buf.eol = editor::Eol::Lf,
            Some("cr") => self.doc.buf.eol = editor::Eol::Cr,
            _ => {}
        }
        true
    }
    fn apply_saved_text(&mut self, text: &str) {
        if self.doc.buf.text == text {
            return;
        }
        let point = |at: usize| {
            let line = self.doc.buf.line_of(at);
            (line, self.doc.buf.text[self.doc.buf.starts[line]..at].chars().count())
        };
        let (cur, anchor) = (point(self.doc.buf.cur), point(self.doc.buf.anchor));
        self.doc.edit(0, self.doc.buf.text.len(), text, EditKind::Other);
        let offset = |(line, col): (usize, usize)| {
            let (s, e) = self.doc.buf.line(line.min(self.doc.buf.lines() - 1));
            self.doc.buf.text[s..e].char_indices().nth(col).map_or(e, |(i, _)| s + i)
        };
        self.doc.select(offset(anchor), offset(cur));
        self.doc.moved();
        self.edited();
    }
    pub(super) fn saving_poll(&mut self) {
        let Some(job) = &self.saving.pending else { return };
        let result = match job.result.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => Err("Formatter stopped without a result".into()),
        };
        let job = self.saving.pending.take().unwrap();
        if self.file_tabs[self.active_file].id != job.tab
            || self.doc.buf.version != job.version
            || self.doc.path.as_ref() != Some(&job.path)
        {
            self.message = Some("Document changed during formatting. Save again.".into());
            return;
        }
        match result {
            Ok(text) => {
                // Check the disk again before changing either the buffer or the file.
                if !self.disk_safe() {
                    return;
                }
                let text = Rules::read(&job.path).normalize(&text);
                self.apply_saved_text(&text);
                self.saving.formatted = true;
                self.save();
            }
            Err(e) => self.message = Some(format!("Not saved. Formatter failed: {e}")),
        }
        self.autosave_due = None;
        self.redraw = true;
    }
}
fn format_text(root: &Path, command: &str, text: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("timeout")
        .args(["-k", "2", "10", "bash", "-c", command])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut input = child.stdin.take().unwrap();
    let bytes = text.as_bytes().to_vec();
    let writer = std::thread::spawn(move || input.write_all(&bytes));
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    let written = writer.join().map_err(|_| "Cannot send text to formatter".to_string())?;
    if !result.status.success() {
        return Err(format!("{}: {}", result.status, String::from_utf8_lossy(&result.stderr).trim()));
    }
    written.map_err(|e| e.to_string())?;
    String::from_utf8(result.stdout).map_err(|e| e.to_string())
}

#[derive(Default, Debug)]
pub(crate) struct Rules {
    values: BTreeMap<String, String>,
    eol: Option<String>,
    charset: Option<String>,
}
impl Rules {
    pub(crate) fn read(path: &Path) -> Self {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let mut configs = Vec::new();
        for dir in path.parent().into_iter().flat_map(Path::ancestors) {
            let Ok(text) = std::fs::read_to_string(dir.join(".editorconfig")) else { continue };
            let root = text.lines().take_while(|l| !l.trim().starts_with('[')).any(|l| {
                l.split_once('=').is_some_and(|(k, v)| {
                    k.trim().eq_ignore_ascii_case("root") && v.trim().eq_ignore_ascii_case("true")
                })
            });
            configs.push((dir.to_path_buf(), text));
            if root {
                break;
            }
        }
        let mut rules = Self::default();
        for (dir, text) in configs.into_iter().rev() {
            let name = path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let mut applies = false;
            for line in text.lines().map(str::trim) {
                if line.starts_with(['#', ';']) || line.is_empty() {
                    continue;
                }
                if let Some(pattern) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                    let pattern = pattern.trim_start_matches('/');
                    let target =
                        if pattern.contains('/') { name.as_str() } else { name.rsplit('/').next().unwrap_or(&name) };
                    applies = glob(pattern, target);
                } else if applies && let Some((k, v)) = line.split_once('=') {
                    let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
                    if v == "unset" {
                        rules.values.remove(&k);
                    } else {
                        rules.values.insert(k, v);
                    }
                }
            }
        }
        rules.eol = rules.values.get("end_of_line").cloned();
        rules.charset = rules.values.get("charset").cloned();
        rules
    }
    pub(crate) fn cr(&self) -> bool {
        self.eol.as_deref() == Some("cr")
    }
    pub(crate) fn indent(&self) -> Option<String> {
        match self.values.get("indent_style").map(String::as_str) {
            Some("tab") => Some("\t".into()),
            Some("space") => Some(" ".repeat(
                self.values.get("indent_size").and_then(|s| s.parse::<usize>().ok()).unwrap_or(4).clamp(1, 16),
            )),
            _ => {
                self.values.get("indent_size").and_then(|s| s.parse::<usize>().ok()).map(|n| " ".repeat(n.clamp(1, 16)))
            }
        }
    }
    fn normalize(&self, text: &str) -> String {
        let trim = self.values.get("trim_trailing_whitespace").is_none_or(|v| v == "true");
        let mut text = text.replace("\r\n", "\n");
        if trim {
            text = text.split('\n').map(|l| l.trim_end_matches([' ', '\t'])).collect::<Vec<_>>().join("\n");
        }
        if self.values.get("insert_final_newline").is_none_or(|v| v == "true")
            && !text.is_empty()
            && !text.ends_with('\n')
        {
            text.push('\n');
        }
        match self.charset.as_deref() {
            Some("utf-8-bom") if !text.starts_with('\u{feff}') => text.insert(0, '\u{feff}'),
            Some("utf-8") => text = text.trim_start_matches('\u{feff}').to_string(),
            _ => {}
        }
        text
    }
}
/// EditorConfig globs, including recursive directories, classes and alternatives.
pub(super) fn glob(pattern: &str, text: &str) -> bool {
    if let Some(open) = pattern.find('{')
        && let Some(end) = pattern[open..].find('}')
    {
        let end = open + end;
        let inside = &pattern[open + 1..end];
        let choices: Vec<String> = if let Some((a, b)) = inside.split_once("..") {
            match (a.parse::<i32>(), b.parse::<i32>()) {
                (Ok(a), Ok(b)) if b >= a && b.saturating_sub(a) < 1000 => (a..=b).map(|n| n.to_string()).collect(),
                _ => vec![inside.to_string()],
            }
        } else {
            inside.split(',').map(str::to_string).collect()
        };
        return choices.iter().any(|part| glob(&format!("{}{part}{}", &pattern[..open], &pattern[end + 1..]), text));
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn matches(p: &[char], t: &[char], memo: &mut BTreeMap<(usize, usize), bool>) -> bool {
        let key = (p.len(), t.len());
        if let Some(v) = memo.get(&key) {
            return *v;
        }
        let yes = match p.first() {
            None => t.is_empty(),
            Some('*') => {
                let double = p.get(1) == Some(&'*');
                let n = if double { 2 } else { 1 };
                matches(&p[n..], t, memo)
                    || (double && p.get(n) == Some(&'/') && matches(&p[n + 1..], t, memo))
                    || (!t.is_empty() && (double || t[0] != '/') && matches(p, &t[1..], memo))
            }
            Some('?') => !t.is_empty() && t[0] != '/' && matches(&p[1..], &t[1..], memo),
            Some('[') => {
                if let Some(end) = p.iter().position(|&c| c == ']') {
                    let negate = p.get(1) == Some(&'!');
                    let start = if negate { 2 } else { 1 };
                    let mut hit = false;
                    if let Some(&c) = t.first() {
                        let mut i = start;
                        while i < end {
                            if i + 2 < end && p[i + 1] == '-' {
                                hit |= p[i] <= c && c <= p[i + 2];
                                i += 3;
                            } else {
                                hit |= p[i] == c;
                                i += 1;
                            }
                        }
                        c != '/' && hit != negate && matches(&p[end + 1..], &t[1..], memo)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            Some(c) => t.first() == Some(c) && matches(&p[1..], &t[1..], memo),
        };
        memo.insert(key, yes);
        yes
    }
    matches(&p, &t, &mut BTreeMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editorconfig_inherits_globs_unsets_and_endings() {
        let dir = std::env::temp_dir().join(format!("ld-config-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join(".editorconfig"),"root=true\n[*]\nindent_style=space\nindent_size=2\nend_of_line=crlf\n[*.{md,txt}]\ntrim_trailing_whitespace=false\n").unwrap();
        std::fs::write(
            dir.join("src/.editorconfig"),
            "[*.py]\nindent_size=4\nend_of_line=unset\ninsert_final_newline=false\n",
        )
        .unwrap();
        let rules = Rules::read(&dir.join("src/a.py"));
        assert_eq!(rules.indent().as_deref(), Some("    "));
        assert_eq!(rules.eol, None);
        assert_eq!(rules.normalize("foo  "), "foo");
        assert_eq!(Rules::read(&dir.join("a.md")).normalize("word  "), "word  \n");
        assert!(glob("src/**/test[0-9].{py,rs}", "src/a/test3.py"));
        assert!(glob("src/**/test[0-9].{py,rs}", "src/test3.rs"));
        assert!(!glob("*.py", "src/a.py"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn formatting_saves_as_one_undo_step_and_refuses_stale_output() {
        let dir = std::env::temp_dir().join(format!("ld-format-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.rs");
        std::fs::write(&path, "hello  ").unwrap();
        let mut app = App::new(Some(path.clone()));
        app.cfg.format_on_save = true;
        app.cfg.formatters.insert("rs".into(), "tr a-z A-Z".into());
        app.save();
        let until = std::time::Instant::now();
        while app.saving.pending.is_some() && until.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.saving_poll();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "HELLO\n");
        app.doc.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(app.doc.buf.text, "hello  ");
        app.cfg.formatters.insert("rs".into(), "sleep 0.05; cat".into());
        app.save();
        app.doc.edit(0, 0, "new", EditKind::Other);
        let until = std::time::Instant::now();
        while app.saving.pending.is_some() && until.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.saving_poll();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "HELLO\n");
        assert!(app.doc.buf.text.starts_with("new"));
        app.cfg.formatters.insert("rs".into(), "echo failure >&2; exit 2".into());
        app.save();
        let until = std::time::Instant::now();
        while app.saving.pending.is_some() && until.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.saving_poll();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "HELLO\n");
        assert!(app.message.as_deref().unwrap().contains("failure"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
