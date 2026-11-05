//! Explicit SSH or container execution for a project. No network work or
//! container start at startup.
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
/// Where project commands run: an SSH host's folder, or with `container`
/// set, a dev container's workspace folder (and `host` empty).
#[derive(Clone, Debug)]
pub struct Remote {
    pub host: String,
    pub path: PathBuf,
    /// Push each saved file to the SSH host.
    pub push_on_save: bool,
    pub container: Option<crate::container::Target>,
}
fn registry() -> &'static Mutex<BTreeMap<PathBuf, Remote>> { static ACTIVE: OnceLock<Mutex<BTreeMap<PathBuf, Remote>>> = OnceLock::new(); ACTIVE.get_or_init(Default::default) }
pub fn active(root: &Path) -> Option<Remote> { registry().lock().unwrap().get(root).cloned() }
pub fn activate(root: &Path, remote: Option<Remote>) { let mut all = registry().lock().unwrap(); if let Some(r) = remote { all.insert(root.into(), r); } else { all.remove(root); } }
pub fn quote(s: &str) -> String { format!("'{}'", s.replace('\'', "'\\''")) }
impl Remote {
    pub fn read(root: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(root.join(".lightdown/remote.json")).map_err(|e| format!("Read .lightdown/remote.json: {e}"))?;
        Self::parse(&serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())?)
    }
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let host = v["host"].as_str().ok_or("Expected SSH host")?;
        let path = v["path"].as_str().ok_or("Expected absolute remote path")?;
        if host.is_empty() || host.starts_with('-') || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b)) || host.matches('@').count() > 1 { return Err("Use an SSH config host alias or user@host; configure ports and keys in ~/.ssh/config".into()); }
        if !path.starts_with('/') || path.chars().any(char::is_control) || Path::new(path).components().any(|p| matches!(p, std::path::Component::ParentDir)) { return Err("Remote path must be absolute, without .. or control characters".into()); }
        let push_on_save = v.get("pushOnSave").map(|b| b.as_bool().ok_or("pushOnSave must be true or false")).transpose()?.unwrap_or(false);
        Ok(Self { host: host.into(), path: path.into(), push_on_save, container: None })
    }
    /// Execution in a dev container's workspace folder.
    pub fn container(target: crate::container::Target, workspace: PathBuf) -> Self {
        Self { host: String::new(), path: workspace, push_on_save: false, container: Some(target) }
    }
    /// What the status bar shows.
    pub fn label(&self) -> String {
        match &self.container {
            Some(c) => format!("Container {}", c.name),
            None => format!("SSH {}", self.host),
        }
    }
    /// What the agent is told about it.
    pub fn describe(&self) -> String {
        match &self.container {
            Some(c) => format!("Container execution is active in {} ({}) at {}. The project folder is mounted there, so file tools and commands see the same files; no sync is needed.", c.name, c.engine, self.path.display()),
            None => format!("Remote execution is active on {}:{}. File tools edit the local mirror; run_command and terminals execute remotely. Ask the user to push local changes before running tests and pull remote changes afterward. Remote writes are outside local checkpoints.", self.host, self.path.display()),
        }
    }
    pub fn command(&self, command: &str, terminal: bool) -> Command {
        if let Some(target) = &self.container {
            return target.exec(&self.path, command, terminal);
        }
        let mut cmd = Command::new("ssh");
        cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "StrictHostKeyChecking=yes", if terminal { "-tt" } else { "-T" }, "--", &self.host]);
        cmd.arg(format!("cd -- {} && exec sh -c {}", quote(&self.path.to_string_lossy()), quote(command)));
        cmd
    }
    pub fn sync(&self, root: &Path, pull: bool) -> Command {
        let mut cmd = Command::new("rsync");
        cmd.args(["--archive", "--timeout=60", "--safe-links", "--protect-args", "--exclude=.git/", "--exclude=.lightdown/remote.json", "--exclude=target/", "--exclude=node_modules/", "--exclude=.venv/", "--exclude=venv/", "--exclude=__pycache__/", "-e", "ssh -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=yes", "--"]);
        let remote = format!("{}:{}/", self.host, self.path.to_string_lossy().trim_end_matches('/'));
        let local = format!("{}/", root.to_string_lossy().trim_end_matches('/'));
        if pull { cmd.args([remote, local]); } else { cmd.args([local, remote]); }
        cmd
    }
    /// Copies just `files`, relative to `root`, to the same places remotely.
    pub fn push_files(&self, root: &Path, files: &[PathBuf]) -> Command {
        let mut cmd = Command::new("rsync");
        cmd.args(["--archive", "--relative", "--timeout=60", "--safe-links", "--protect-args", "-e", "ssh -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=yes", "--"]);
        cmd.args(files.iter().map(|f| Path::new(".").join(f)));
        cmd.arg(format!("{}:{}/", self.host, self.path.to_string_lossy().trim_end_matches('/')));
        cmd.current_dir(root);
        cmd
    }
    /// `ssh -N -L` forwarding `local` on this machine to `port` on the host.
    pub fn forward(&self, local: u16, port: u16) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(["-N", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "StrictHostKeyChecking=yes", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=30", "-L"]);
        cmd.arg(format!("127.0.0.1:{local}:127.0.0.1:{port}")).args(["--", &self.host]);
        cmd
    }
    /// Translate only protocol URI/path fields and URI keys, never source text.
    pub fn map_lsp(&self, value: &Value, root: &Path, to_remote: bool) -> Value {
        fn mapped(text: &str, from: &Path, to: &Path) -> String {
            if let Some(path) = crate::lsp::path(text) {
                return path.strip_prefix(from).map_or_else(|_| text.to_string(), |rel| crate::lsp::uri(&to.join(rel)));
            }
            if text.starts_with('/') { return Path::new(text).strip_prefix(from).map_or_else(|_| text.to_string(), |rel| to.join(rel).to_string_lossy().into_owned()); }
            text.to_string()
        }
        fn visit(value: &Value, from: &Path, to: &Path) -> Value {
            match value {
                Value::Array(values) => Value::Array(values.iter().map(|v| visit(v, from, to)).collect()),
                Value::Object(values) => Value::Object(values.iter().map(|(key, v)| {
                    let key_out = if key.starts_with("file://") { mapped(key, from, to) } else { key.clone() };
                    let val = if ["uri", "rootUri", "targetUri", "oldUri", "newUri", "documentUri", "rootPath"].contains(&key.as_str()) {
                        v.as_str().map_or_else(|| visit(v, from, to), |s| Value::String(mapped(s, from, to)))
                    } else { visit(v, from, to) };
                    (key_out, val)
                }).collect()),
                _ => value.clone(),
            }
        }
        let (from, to) = if to_remote { (root, self.path.as_path()) } else { (self.path.as_path(), root) };
        visit(value, from, to)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn ssh_is_noninteractive_and_quotes_remote_commands() {
        let r = Remote::parse(&json!({"host":"user@host", "path":"/srv/a project's files"})).unwrap();
        let c = r.command("printf '%s' \"hello\"", false);
        let args = c.get_args().map(|s| s.to_string_lossy().to_string()).collect::<Vec<_>>();
        assert_eq!(c.get_program(), "ssh");
        assert!(args.contains(&"BatchMode=yes".into()) && args.contains(&"StrictHostKeyChecking=yes".into()));
        assert!(args.last().unwrap().contains("'\\''"));
        assert!(Remote::parse(&json!({"host":"-oProxyCommand=bad", "path":"/srv"})).is_err());
        assert!(Remote::parse(&json!({"host":"host", "path":"/srv/../etc"})).is_err());
        let args = r.sync(Path::new("/mirror"), true).get_args().map(|s| s.to_string_lossy().to_string()).collect::<Vec<_>>();
        assert!(args.contains(&"--protect-args".into()) && !args.contains(&"--delete".into()));
        let push = r.push_files(Path::new("/mirror"), &[PathBuf::from("-n.rs"), PathBuf::from("src/a b.rs")]);
        let args = push.get_args().map(|s| s.to_string_lossy().to_string()).collect::<Vec<_>>();
        assert_eq!(&args[args.len() - 3..], ["./-n.rs", "./src/a b.rs", "user@host:/srv/a project's files/"]);
        assert_eq!(push.get_current_dir(), Some(Path::new("/mirror")));
        let forward = r.forward(18080, 8080).get_args().map(|s| s.to_string_lossy().to_string()).collect::<Vec<_>>();
        assert!(forward.contains(&"127.0.0.1:18080:127.0.0.1:8080".into()) && forward.contains(&"ExitOnForwardFailure=yes".into()));
        assert_eq!(forward.last().unwrap(), "user@host");
        assert!(Remote::parse(&json!({"host":"h", "path":"/srv", "pushOnSave":"yes"})).is_err());
        assert!(Remote::parse(&json!({"host":"h", "path":"/srv", "pushOnSave":true})).unwrap().push_on_save);
    }
    #[test]
    fn language_server_uris_and_edit_keys_round_trip_without_changing_text() {
        let r = Remote::parse(&json!({"host":"host", "path":"/remote/project"})).unwrap();
        let root = Path::new("/local/mirror");
        let input = json!({"params":{"textDocument":{"uri":"file:///local/mirror/a%20b.rs"}},"changes":{"file:///local/mirror/a.rs":[{"newText":"file:///local/mirror/literal"}]},"uri":"file:///local/mirror-other/no.rs"});
        let remote = r.map_lsp(&input, root, true);
        assert_eq!(remote["params"]["textDocument"]["uri"], "file:///remote/project/a%20b.rs");
        assert_eq!(remote["uri"], input["uri"]);
        assert_eq!(remote["changes"]["file:///remote/project/a.rs"][0]["newText"], "file:///local/mirror/literal");
        assert_eq!(r.map_lsp(&remote, root, false), input);
    }
}
