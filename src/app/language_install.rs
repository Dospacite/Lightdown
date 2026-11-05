//! Installer jobs report through the same wake/poll loop as language clients.
use super::*;
use crate::lsp_registry as registry;
#[derive(Default)]
pub(super) struct Installs {
    jobs: HashMap<String, std::sync::mpsc::Receiver<(String, bool, bool)>>,
    pub(super) progress: HashMap<String, String>,
}
impl App {
    pub(super) fn language_install_missing(&mut self, key: &(PathBuf, String), command: &[String]) -> bool {
        if crate::remote::active(&self.root).is_some() {
            return false;
        }
        let Some(s) = registry::for_language(&key.1).next() else { return false };
        if command.first().is_some_and(|c| Path::new(c).is_file() || registry::local(&self.root, c).is_some()) {
            return false;
        }
        // Custom commands are never replaced by a default installation.
        if command.first().is_none_or(|c| c != s.binary) {
            return false;
        }
        self.languages.status.names.insert(key.clone(), s.name.into());
        self.languages
            .status
            .state
            .insert(key.clone(), status::State::Missing(format!("Install {} from language status", s.name)));
        self.languages.failed.insert(key.clone());
        if self.cfg.lsp_auto_install == "always" && !matches!(s.install, registry::Install::External) {
            self.language_install_start(s.name);
        }
        true
    }
    pub(super) fn language_install_start(&mut self, name: &str) {
        let Some(s) = registry::named(name) else { return };
        if self.languages.installs.jobs.contains_key(name) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.languages.installs.progress.insert(name.into(), "Starting installation".into());
        self.languages.installs.jobs.insert(name.into(), rx);
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            let result = registry::install(s, |p| {
                let _ = tx.send((p.into(), false, false));
                wake();
            });
            let (text, ok) = match result {
                Ok(v) => (format!("Installed {} {v}", s.name), true),
                Err(e) => (format!("{} installation failed: {e}", s.name), false),
            };
            let _ = tx.send((text, true, ok));
            wake();
        });
    }
    pub(super) fn language_install_poll(&mut self) {
        let events: Vec<_> = self
            .languages
            .installs
            .jobs
            .iter()
            .flat_map(|(name, rx)| rx.try_iter().map(|v| (name.clone(), v)))
            .collect();
        for (name, (text, done, ok)) in events {
            self.languages.installs.progress.insert(name.clone(), text.clone());
            if done {
                self.languages.installs.jobs.remove(&name);
                if name == "updates" {
                    self.language_hover_at(text.clone(), None);
                }
                self.message = Some(text);
                if ok {
                    self.languages.installs.progress.remove(&name);
                    let ids: Vec<_> = registry::named(&name).unwrap().languages.iter().map(|s| s.to_string()).collect();
                    self.languages.failed.retain(|(_, id)| !ids.contains(id));
                    self.language_sync();
                }
            }
        }
    }
    pub(super) fn language_updates(&mut self) {
        let name = "updates";
        if self.languages.installs.jobs.contains_key(name) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.languages.installs.jobs.insert(name.into(), rx);
        self.message = Some("Checking installed language server versions…".into());
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            let mut lines = Vec::new();
            for s in registry::SERVERS.iter().filter(|s| registry::installed(s).is_some()) {
                let v: Value = std::fs::read(registry::directory(s).join("version.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                let latest = registry::latest(s).unwrap_or_else(|e| e);
                lines.push(format!(
                    "{}: installed {}, latest {latest}",
                    s.name,
                    v["version"].as_str().unwrap_or("unknown")
                ));
            }
            let text = if lines.is_empty() {
                "No Lightdown-managed language servers installed".into()
            } else {
                lines.join("\n")
            };
            let _ = tx.send((text, true, false));
            wake();
        });
    }
}
