//! Each language server's state, work progress and bounded log, for the status bar.
use super::*;
type Key = (PathBuf, String);
#[derive(Clone, PartialEq, Debug, Default)]
pub(super) enum State {
    #[default]
    Starting,
    Ready,
    Stopped,
    Crashed(String),
    Missing(String),
}
#[derive(Default)]
pub(super) struct Status {
    pub(super) state: HashMap<Key, State>,
    /// Work-done tokens in the order they began, with their title and current text.
    progress: HashMap<Key, Vec<(String, String, String)>>,
    logs: HashMap<Key, String>,
    pub(super) names: HashMap<Key, String>,
}
const LOG: usize = 64 * 1024;
/// Keeps the newest 64 KB, dropping whole lines from the front.
fn log_push(log: &mut String, text: &str) {
    log.extend(text.chars().take(4096));
    if !log.ends_with('\n') { log.push('\n'); }
    if log.len() > LOG {
        let mut cut = log.len() - LOG;
        while !log.is_char_boundary(cut) { cut += 1; }
        let cut = log[cut..].find('\n').map_or(cut, |n| cut + n + 1);
        log.drain(..cut);
    }
}
/// Report messages refine the title the begin message gave.
fn progress_text(value: &Value, title: &str) -> String {
    let mut out = title.to_string();
    if let Some(m) = value["message"].as_str().filter(|m| !m.is_empty()) { out += " · "; out += &m.chars().take(60).collect::<String>(); }
    if let Some(n) = value["percentage"].as_u64() { out += &format!(" {}%", n.min(100)); }
    out
}
/// Missing executables are reported apart from servers that started and then failed.
pub(super) fn failure(why: &str) -> State {
    if why.contains("os error 2") || why.contains("not found") { State::Missing(why.into()) } else { State::Crashed(why.into()) }
}
impl App {
    /// The status-bar label of the file's server and its colour.
    pub(in crate::app) fn language_status(&self) -> Option<(String, u32)> {
        let key = self.languages.key.as_ref()?;
        let st = &self.languages.status;
        let name = st.names.get(key).cloned().unwrap_or_else(|| key.1.clone());
        let state = st.state.get(key).cloned().unwrap_or_default();
        let work = st.progress.get(key).and_then(|p| p.last()).map(|(_, _, t)| t.clone());
        if let Some(work) = self.languages.installs.progress.get(&name) { return Some((format!("{name}: {work}"),if work.contains("failed:") { theme::danger() } else { theme::warning() })); }
        Some(match (state, work) {
            (State::Ready | State::Starting, Some(work)) => (format!("{name}: {work}"), theme::warning()),
            (State::Starting, None) => (format!("{name}: starting"), theme::warning()),
            (State::Ready, None) => (name, theme::success()),
            (State::Stopped, _) => (format!("{name}: stopped"), theme::dim()),
            (State::Crashed(_), _) => (format!("{name}: crashed"), theme::danger()),
            (State::Missing(_), _) if self.cfg.lsp_auto_install == "never" => {
                (format!("{name}: not installed"), theme::danger())
            }
            (State::Missing(_), _) => (format!("Install {name}"), theme::danger()),
        })
    }
    /// Takes log lines and progress out of the message stream. Returns whether `v` was consumed.
    pub(super) fn language_status_event(&mut self, key: &Key, v: &Value) -> bool {
        let st = &mut self.languages.status;
        if let Some(text) = v["clientLog"].as_str() {
            log_push(st.logs.entry(key.clone()).or_default(), text);
            return true;
        }
        if matches!(v["method"].as_str(), Some("window/logMessage" | "window/showMessage")) {
            let text = v["params"]["message"].as_str().unwrap_or("");
            log_push(st.logs.entry(key.clone()).or_default(), text);
            // Errors the server chooses to show reach the status bar too.
            if v["method"] == "window/showMessage" && v["params"]["type"].as_u64() == Some(1) {
                self.message = Some(format!("{}: {}", key.1, text.lines().next().unwrap_or("")));
            }
            return true;
        }
        if v["method"] == "$/progress" {
            let token = v["params"]["token"].to_string();
            let value = &v["params"]["value"];
            let list = st.progress.entry(key.clone()).or_default();
            let at = list.iter().position(|(t, ..)| *t == token);
            if value["kind"] == "end" {
                if let Some(at) = at { list.remove(at); }
            } else if let Some(at) = at {
                list[at].2 = progress_text(value, &list[at].1.clone());
            } else if list.len() < 64 {
                let title: String = value["title"].as_str().unwrap_or("Working").chars().take(60).collect();
                list.push((token, title.clone(), progress_text(value, &title)));
            }
            return true;
        }
        if v["clientCapabilities"].is_object() {
            st.state.insert(key.clone(), State::Ready);
        }
        if let Some(why) = v["clientError"].as_str() {
            st.progress.remove(key);
            log_push(st.logs.entry(key.clone()).or_default(), why);
            if st.state.get(key) != Some(&State::Stopped) { st.state.insert(key.clone(), failure(why)); }
        }
        false
    }
    pub(in crate::app) fn language_server_started(&mut self, key: &Key, command: &[String]) {
        let st = &mut self.languages.status;
        let name = command.first().map(|c| c.rsplit('/').next().unwrap_or(c).to_string()).unwrap_or_default();
        st.names.insert(key.clone(), name);
        st.state.insert(key.clone(), State::Starting);
        st.progress.remove(key);
    }
    pub(super) fn language_server_stopped(&mut self, key: &Key) {
        self.languages.status.state.insert(key.clone(), State::Stopped);
        self.languages.status.progress.remove(key);
    }
    /// The status item's menu: restart, stop, or read the log.
    pub(in crate::app) fn language_server_menu(&mut self) {
        let Some((label, _)) = self.language_status() else { return };
        let running = self.languages.key.as_ref().is_some_and(|k| self.languages.clients.contains_key(k));
        let mut entries = vec![Entry::new("server:restart", "Restart language server", "").keys(self.keys_of("lsp_restart"))];
        if self.cfg.lsp_auto_install != "never" {
            if let Some(s) = self.languages.key.as_ref().and_then(|k| crate::lsp_registry::for_language(&k.1).next()) {
                entries.insert(0,Entry::new(&format!("server:install:{}",s.name),&format!("Install {}",s.name),"Install or update in Lightdown's server directory"));
            }
        }
        entries.push(Entry::new("server:update", "Check language server updates", ""));
        if let Some(work) = self.languages.installs.progress.get("updates") { entries.push(Entry::new("",work,"")); }
        if running { entries.push(Entry::new("server:stop", "Stop language server", "")); }
        entries.push(Entry::new("server:server_log", "Show language server log", ""));
        let why = match self.languages.key.as_ref().and_then(|k| self.languages.status.state.get(k)) {
            Some(State::Crashed(w) | State::Missing(w)) => w.lines().next().unwrap_or("").to_string(),
            _ => String::new(),
        };
        let why = self.languages.key.as_ref().and_then(|k| self.languages.status.names.get(k)).and_then(|name| self.languages.installs.progress.get(name)).cloned().unwrap_or(why);
        let mut menu = Menu::new(&label, entries, "");
        if !why.is_empty() { menu.hint = why; }
        self.menu = Some((MenuKind::Language, menu));
    }
    pub(super) fn language_server_log(&mut self) {
        let log = self.languages.key.as_ref().and_then(|k| self.languages.status.logs.get(k)).filter(|l| !l.trim().is_empty());
        let text = log.map_or_else(|| "No language server log messages".to_string(), |l| {
            // The popup shows the newest lines; the full 64 KB stays in memory.
            let lines: Vec<_> = l.lines().collect();
            lines[lines.len().saturating_sub(200)..].join("\n")
        });
        self.language_hover_at(format!("```\n{text}\n```"), None);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_keep_the_newest_lines_and_progress_tracks_tokens() {
        let mut log = String::new();
        for i in 0..20000 { log_push(&mut log, &format!("line {i}")); }
        assert!(log.len() <= LOG);
        assert!(log.starts_with("line ") && log.ends_with("line 19999\n"));
        let begin = json!({"kind":"begin","title":"Indexing","percentage":0});
        assert_eq!(progress_text(&begin, "Indexing"), "Indexing 0%");
        let report = json!({"kind":"report","message":"12/40","percentage":45});
        assert_eq!(progress_text(&report, "Indexing"), "Indexing · 12/40 45%");
        assert!(matches!(failure("rust-analyzer: No such file or directory (os error 2)"), State::Missing(_)));
        assert!(matches!(failure("Language server exited"), State::Crashed(_)));
    }
    #[test]
    fn status_follows_start_progress_and_crash() {
        let mut a = App::new(None);
        let key = (PathBuf::from("/p"), "rust".to_string());
        a.languages.key = Some(key.clone());
        a.language_server_started(&key, &["/usr/bin/rust-analyzer".into()]);
        assert_eq!(a.language_status().unwrap().0, "rust-analyzer: starting");
        a.language_status_event(&key, &json!({"clientCapabilities":{}}));
        assert_eq!(a.language_status().unwrap(), ("rust-analyzer".into(), theme::success()));
        let p = |kind: &str, token: u64, pct: u64| json!({"method":"$/progress","params":{"token":token,"value":{"kind":kind,"title":"indexing","percentage":pct}}});
        assert!(a.language_status_event(&key, &p("begin", 1, 45)));
        a.language_status_event(&key, &p("begin", 2, 10));
        assert_eq!(a.language_status().unwrap().0, "rust-analyzer: indexing 10%");
        a.language_status_event(&key, &p("end", 2, 0));
        assert_eq!(a.language_status().unwrap().0, "rust-analyzer: indexing 45%");
        a.language_status_event(&key, &json!({"clientError":"Language server exited"}));
        assert_eq!(a.language_status().unwrap(), ("rust-analyzer: crashed".into(), theme::danger()));
        a.language_server_menu();
        assert!(matches!(a.menu, Some((MenuKind::Language, _))));
    }
}
