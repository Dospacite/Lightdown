//! Defer explorer operations until registered will-file requests have returned.
use super::*;
#[derive(Clone)]
pub(in crate::app) enum Resume {
    Create(String),
    Rename(PathBuf, String),
    Delete(PathBuf, bool),
    Paste(PathBuf, (PathBuf, bool)),
    Duplicate(PathBuf),
}
struct PendingFiles {
    resume: Resume,
    waiting: std::collections::HashSet<u64>,
    edits: Vec<Value>,
    started: std::time::Instant,
}
#[derive(Default)]
pub(super) struct Files {
    pending: Option<PendingFiles>,
    registrations: HashMap<(PathBuf, String), HashMap<String, Value>>,
    bypass: bool,
}
fn filter_matches(options: &Value, path: &Path, folder: bool) -> bool {
    options["filters"].as_array().is_none_or(|filters| {
        filters.iter().any(|f| {
            if f["scheme"].as_str().is_some_and(|s| s != "file") {
                return false;
            }
            if f["pattern"]["matches"] == "file" && folder || f["pattern"]["matches"] == "folder" && !folder {
                return false;
            }
            let glob = f["pattern"]["glob"].as_str().unwrap_or("**/*");
            let mut pattern = String::from("^");
            let mut chars = glob.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '*' if chars.peek() == Some(&'*') => {
                        chars.next();
                        if chars.peek() == Some(&'/') {
                            chars.next();
                            pattern.push_str("(?:.*/)?");
                        } else {
                            pattern.push_str(".*");
                        }
                    }
                    '*' => pattern.push_str("[^/]*"),
                    '?' => pattern.push_str("[^/]"),
                    '{' => pattern.push_str("(?:"),
                    '}' => pattern.push(')'),
                    ',' => pattern.push('|'),
                    c => pattern.push_str(&regex::escape(&c.to_string())),
                }
            }
            pattern.push('$');
            let mut builder = regex::RegexBuilder::new(&pattern);
            builder.case_insensitive(f["pattern"]["options"]["ignoreCase"] == true);
            builder.build().is_ok_and(|r| {
                r.is_match(&path.to_string_lossy())
                    || path.file_name().is_some_and(|n| r.is_match(&n.to_string_lossy()))
            })
        })
    })
}
impl App {
    fn language_file_support(&self, key: &(PathBuf, String), method: &str, path: &Path, folder: bool) -> bool {
        let short = method.strip_prefix("workspace/").unwrap_or(method).trim_end_matches("Files");
        self.languages
            .capabilities
            .get(key)
            .and_then(|c| c["workspace"]["fileOperations"].get(short))
            .is_some_and(|v| filter_matches(v, path, folder))
            || self.languages.files.registrations.get(key).is_some_and(|entries| {
                entries.values().any(|r| r["method"] == method && filter_matches(&r["registerOptions"], path, folder))
            })
    }
    /// True means the operation is deferred or could not safely start.
    pub(in crate::app) fn language_before_file(
        &mut self,
        kind: &str,
        old: &Path,
        new: Option<&Path>,
        resume: Resume,
    ) -> bool {
        if self.languages.files.bypass {
            return false;
        }
        if self.languages.files.pending.is_some() {
            self.message = Some("Waiting for language server file edits".into());
            return true;
        }
        let method = format!("workspace/will{kind}Files");
        let folder = match &resume {
            Resume::Create(input) => input.ends_with('/'),
            Resume::Duplicate(path) => path.is_dir(),
            Resume::Paste(_, clip) => clip.0.is_dir(),
            _ => old.is_dir(),
        };
        let keys: Vec<_> = self
            .languages
            .clients
            .keys()
            .filter(|k| self.language_file_support(k, &method, old, folder))
            .cloned()
            .collect();
        if keys.is_empty() {
            return false;
        }
        if !self.language_snapshot() {
            return true;
        }
        let params = if let Some(new) = new {
            json!({"files":[{"oldUri":lsp::uri(old),"newUri":lsp::uri(new)}]})
        } else {
            json!({"files":[{"uri":lsp::uri(old)}]})
        };
        let mut waiting = std::collections::HashSet::new();
        for key in keys {
            self.languages.next += 1;
            let id = self.languages.next;
            self.languages.clients[&key].request(id, &method, params.clone());
            waiting.insert(id);
        }
        self.languages.files.pending =
            Some(PendingFiles { resume, waiting, edits: Vec::new(), started: std::time::Instant::now() });
        self.message = Some("Updating imports before file operation…".into());
        true
    }
    pub(in crate::app) fn language_after_file(&mut self, kind: &str, old: &Path, new: Option<&Path>, folder: bool) {
        let method = format!("workspace/did{kind}Files");
        let params = if let Some(new) = new {
            json!({"files":[{"oldUri":lsp::uri(old),"newUri":lsp::uri(new)}]})
        } else {
            json!({"files":[{"uri":lsp::uri(old)}]})
        };
        for (key, client) in &self.languages.clients {
            if self.language_file_support(key, &method, old, folder) {
                client.notify(&method, params.clone());
            }
        }
        if kind == "Rename" || kind == "Delete" {
            self.languages.opened.retain(|p, _| !p.starts_with(old));
        }
    }
    pub(super) fn language_file_event(&mut self, key: &(PathBuf, String), v: &Value) -> bool {
        if v["method"] == "client/registerCapability" {
            let entries = self.languages.files.registrations.entry(key.clone()).or_default();
            for r in v["params"]["registrations"].as_array().into_iter().flatten() {
                if let Some(id) = r["id"].as_str() {
                    entries.insert(id.into(), r.clone());
                }
            }
            return true;
        }
        if v["method"] == "client/unregisterCapability" {
            if let Some(entries) = self.languages.files.registrations.get_mut(key) {
                for r in v["params"]["unregisterations"].as_array().into_iter().flatten() {
                    if let Some(id) = r["id"].as_str() {
                        entries.remove(id);
                    }
                }
            }
            return true;
        }
        if v.get("method").is_some() {
            return false;
        }
        let Some(pending) = self.languages.files.pending.as_mut() else { return false };
        if !v["id"].as_u64().is_some_and(|id| pending.waiting.remove(&id)) {
            return false;
        }
        if !v["error"].is_null() {
            self.languages.files.pending = None;
            self.message = Some(format!("File operation cancelled: {}", v["error"]));
            return true;
        }
        if v["result"].is_object() {
            pending.edits.push(v["result"].clone());
        }
        if pending.waiting.is_empty() {
            let pending = self.languages.files.pending.take().unwrap();
            let mut changes = Vec::new();
            for edit in pending.edits {
                if let Some(d) = edit["documentChanges"].as_array() {
                    changes.extend(d.clone());
                } else {
                    for (uri, e) in edit["changes"].as_object().into_iter().flatten() {
                        changes.push(json!({"textDocument":{"uri":uri},"edits":e}));
                    }
                }
            }
            if !changes.is_empty() {
                if let Err(e) = self.language_apply(&json!({"documentChanges":changes})) {
                    self.message = Some(format!("File operation cancelled: {e}"));
                    return true;
                }
            }
            self.languages.files.bypass = true;
            match pending.resume {
                Resume::Create(input) => self.create(&input),
                Resume::Rename(old, input) => self.rename_path(&old, &input),
                Resume::Delete(path, permanent) => self.discard_path(&path, permanent),
                Resume::Paste(dir, clip) => {
                    if self.file_clip.as_ref() == Some(&clip) {
                        self.paste_files(&dir);
                    } else {
                        self.message = Some("File operation cancelled: clipboard changed".into());
                    }
                }
                Resume::Duplicate(path) => self.duplicate(&path),
            }
            self.languages.files.bypass = false;
        }
        true
    }
    pub(super) fn language_files_tick(&mut self) {
        if self.languages.files.pending.as_ref().is_some_and(|p| p.started.elapsed().as_secs() > 10) {
            self.languages.files.pending = None;
            self.message = Some("File operation cancelled: language server timed out".into());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registrations_filter_language_and_folder_globs() {
        let options = json!({"filters":[{"scheme":"file","pattern":{"glob":"**/*.{rs,ts}","matches":"file"}}]});
        assert!(filter_matches(&options, Path::new("/p/src/a.rs"), false));
        assert!(!filter_matches(&options, Path::new("/p/a.py"), false));
        assert!(!filter_matches(&options, Path::new("/p/a.rs"), true));
    }
}
