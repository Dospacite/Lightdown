//! Project environments from direnv, mise and Nix. Loading one runs the
//! project's own code, so it happens only for projects the user has
//! chosen, a choice kept in Lightdown's data folder and never read from
//! the project. The loader runs on a thread of its own with a time limit;
//! what it gives is kept until its source files change.

use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Direnv,
    Mise,
    Nix,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Direnv => "direnv",
            Tool::Mise => "mise",
            Tool::Nix => "Nix",
        }
    }

    /// The files whose change means loading again.
    fn sources(self) -> &'static [&'static str] {
        match self {
            Tool::Direnv => &[".envrc"],
            Tool::Mise => &["mise.toml", ".mise.toml", ".tool-versions", "mise.local.toml", ".mise.local.toml"],
            Tool::Nix => &["flake.nix", "shell.nix", "flake.lock"],
        }
    }

    fn program(self) -> &'static str {
        match self {
            Tool::Direnv => "direnv",
            Tool::Mise => "mise",
            Tool::Nix => "nix",
        }
    }

    /// How long a load may take: Nix may build or download a shell.
    fn limit(self) -> Duration {
        Duration::from_secs(if self == Tool::Nix { 300 } else { 60 })
    }
}

/// Variables to set, and to unset where the value is `None`.
pub type Vars = Vec<(String, Option<String>)>;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Loading(Tool),
    Ready(Tool, Vars),
    Failed(Tool, String),
}

/// The tool a project's files ask for, in this order: direnv's `.envrc`
/// (which often loads the others itself), mise, then Nix. Tools that are
/// not installed are passed over.
pub fn detect(root: &Path, installed: impl Fn(&str) -> bool) -> Option<Tool> {
    let has = |names: &[&str]| names.iter().any(|n| root.join(n).is_file());
    let wants = [
        (Tool::Direnv, has(&[".envrc"])),
        (Tool::Mise, has(&Tool::Mise.sources()[..3])),
        (Tool::Nix, has(&["flake.nix", "shell.nix"])),
    ];
    wants.into_iter().find(|(tool, wanted)| *wanted && installed(tool.program())).map(|(tool, _)| tool)
}

/// Whether a program is on PATH.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

fn command(tool: Tool, root: &Path) -> Command {
    let mut cmd = Command::new(tool.program());
    match tool {
        Tool::Direnv => cmd.args(["export", "json"]),
        Tool::Mise => cmd.args(["env", "--json"]),
        Tool::Nix => {
            cmd.args(["--extra-experimental-features", "nix-command flakes", "print-dev-env", "--json"]);
            if !root.join("flake.nix").is_file() {
                cmd.args(["--file", "shell.nix"]);
            }
            &mut cmd
        }
    };
    cmd.current_dir(root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd
}

/// Variables of `nix develop`'s that belong to the build sandbox, not to
/// programs run by hand.
const NIX_SKIP: &[&str] = &[
    "BASHOPTS", "HOME", "NIX_BUILD_TOP", "NIX_ENFORCE_PURITY", "NIX_LOG_FD", "NIX_REMOTE", "PPID", "SHELL",
    "SHELLOPTS", "SSL_CERT_FILE", "TEMP", "TEMPDIR", "TERM", "TMP", "TMPDIR", "TZ", "UID", "PWD", "OLDPWD",
    "SHLVL", "builder", "out", "outputs", "stdenv", "shellHook", "dontAddDisableDepTrack",
];

/// What a tool printed, as variables. `path` is the PATH Nix's is put in
/// front of.
pub fn parse(tool: Tool, stdout: &str, path: Option<&str>) -> Result<Vars, String> {
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    let v: Value = serde_json::from_str(stdout).map_err(|e| format!("{} printed what is not JSON: {e}", tool.name()))?;
    let mut out = Vec::new();
    match tool {
        Tool::Direnv | Tool::Mise => {
            let Some(map) = v.as_object() else { return Err(format!("{} printed no variables", tool.name())) };
            for (key, value) in map {
                match value {
                    Value::String(s) => out.push((key.clone(), Some(s.clone()))),
                    Value::Null if tool == Tool::Direnv => out.push((key.clone(), None)),
                    _ => {}
                }
            }
        }
        Tool::Nix => {
            let Some(map) = v["variables"].as_object() else { return Err("nix printed no variables".into()) };
            for (key, var) in map {
                let (Some("exported"), Some(value)) = (var["type"].as_str(), var["value"].as_str()) else { continue };
                if NIX_SKIP.contains(&key.as_str()) {
                    continue;
                }
                let value = match path.filter(|p| key == "PATH" && !p.is_empty()) {
                    Some(old) => format!("{value}:{old}"),
                    None => value.to_string(),
                };
                out.push((key.clone(), Some(value)));
            }
        }
    }
    Ok(out)
}

/// Text without its terminal colour sequences.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
        } else if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// The last lines of what a tool said went wrong.
fn complaint(tool: Tool, stderr: &str, status: Option<i32>) -> String {
    let stderr = plain(stderr);
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let said = lines[lines.len().saturating_sub(3)..].join(" ");
    let said: String = said.chars().take(400).collect();
    match (said.is_empty(), status) {
        (true, Some(code)) => format!("{} exited with status {code}", tool.name()),
        (true, None) => format!("{} was stopped", tool.name()),
        (false, _) => said,
    }
}

/// Runs a loader to its end, or stops it at its time limit.
fn load(tool: Tool, root: &Path) -> Result<Vars, String> {
    let mut child = command(tool, root).spawn().map_err(|e| format!("{}: {e}", tool.program()))?;
    let pipe = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(r) = r {
                let _ = r.take(32 << 20).read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = pipe(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let end = Instant::now() + tool.limit();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} took longer than {} s", tool.name(), tool.limit().as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    // direnv says a blocked .envrc on stderr, and prints nothing.
    if !status.success() || (tool == Tool::Direnv && stderr.contains("is blocked")) {
        return Err(complaint(tool, &stderr, status.code()));
    }
    parse(tool, &stdout, std::env::var("PATH").ok().as_deref())
}

// ---- the choice of projects ---------------------------------------------

fn chosen_file() -> PathBuf {
    crate::config::data_dir().join("environments.json")
}

fn chosen() -> &'static Mutex<BTreeSet<PathBuf>> {
    static CHOSEN: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    CHOSEN.get_or_init(|| {
        let text = std::fs::read_to_string(chosen_file()).unwrap_or_default();
        let list: Vec<PathBuf> = serde_json::from_str(&text).unwrap_or_default();
        Mutex::new(list.into_iter().collect())
    })
}

/// Whether the user has chosen to load this project's environment.
pub fn enabled(root: &Path) -> bool {
    chosen().lock().unwrap().contains(root)
}

/// Chooses to load a project's environment, or not, and remembers it.
pub fn set_enabled(root: &Path, on: bool) -> std::io::Result<()> {
    let mut set = chosen().lock().unwrap();
    if on {
        set.insert(root.to_path_buf());
    } else {
        set.remove(root);
        cache().lock().unwrap().remove(root);
    }
    let text = serde_json::to_string_pretty(&*set).unwrap_or_default();
    crate::config::write_private(&chosen_file(), text.as_bytes())
}

// ---- loaded environments ------------------------------------------------

struct Entry {
    tool: Tool,
    stamp: Vec<Option<(SystemTime, u64)>>,
    state: State,
    checked: Instant,
    /// Bumped on each load, so that a load overtaken by another is dropped.
    run: u64,
    /// A load is under way.
    busy: bool,
    /// The state changed since the window last looked.
    news: bool,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

static WAKE: OnceLock<crate::kernel::Waker> = OnceLock::new();

/// What to call when a load ends, so the window shows it.
pub fn set_waker(waker: crate::kernel::Waker) {
    let _ = WAKE.set(waker);
}

fn stamp(tool: Tool, root: &Path) -> Vec<Option<(SystemTime, u64)>> {
    let of = |n: &&str| std::fs::metadata(root.join(n)).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
    tool.sources().iter().map(of).collect()
}

fn start(root: &Path, tool: Tool, map: &mut HashMap<PathBuf, Entry>) {
    let run = map.get(root).map_or(0, |e| e.run + 1);
    let state = match map.get(root).map(|e| &e.state) {
        // Until the new one is in, the old environment is still used.
        Some(State::Ready(..)) => map[root].state.clone(),
        _ => State::Loading(tool),
    };
    let entry = Entry { tool, stamp: stamp(tool, root), state, checked: Instant::now(), run, busy: true, news: true };
    map.insert(root.to_path_buf(), entry);
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        let state = match load(tool, &root) {
            Ok(vars) => State::Ready(tool, vars),
            Err(e) => State::Failed(tool, e),
        };
        if let Some(e) = cache().lock().unwrap().get_mut(&root).filter(|e| e.run == run) {
            (e.state, e.busy, e.news) = (state, false, true);
        }
        if let Some(wake) = WAKE.get() {
            wake();
        }
    });
}

/// Starts loading a chosen project's environment if it has not been
/// loaded or its files have changed since, looking at most once a second.
fn refresh(root: &Path, force: bool) {
    if !enabled(root) {
        return;
    }
    let mut map = cache().lock().unwrap();
    if let Some(e) = map.get_mut(root) {
        if !force && e.checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        e.checked = Instant::now();
        if !force && (e.busy || stamp(e.tool, root) == e.stamp) {
            return;
        }
    }
    match detect(root, on_path) {
        Some(tool) => start(root, tool, &mut map),
        None => {
            map.remove(root);
        }
    }
}

/// Loads a chosen project's environment again.
pub fn reload(root: &Path) {
    refresh(root, true);
}

/// The variables of a chosen project's environment, as last loaded.
pub fn vars(root: &Path) -> Vars {
    refresh(root, false);
    match cache().lock().unwrap().get(root).map(|e| &e.state) {
        Some(State::Ready(_, vars)) => vars.clone(),
        _ => Vec::new(),
    }
}

/// How a chosen project's environment stands, and whether that is news
/// since the last call.
pub fn status(root: &Path) -> Option<(State, bool)> {
    refresh(root, false);
    let mut map = cache().lock().unwrap();
    let e = map.get_mut(root)?;
    // A reload under way shows as loading, though the old values hold.
    let state = if e.busy { State::Loading(e.tool) } else { e.state.clone() };
    Some((state, std::mem::take(&mut e.news)))
}

/// Puts an environment in place as if it had been loaded.
#[cfg(test)]
pub fn set_loaded(root: &Path, vars: Vars) {
    chosen().lock().unwrap().insert(root.to_path_buf());
    let entry = Entry {
        tool: Tool::Direnv,
        stamp: stamp(Tool::Direnv, root),
        state: State::Ready(Tool::Direnv, vars),
        checked: Instant::now(),
        run: 0,
        busy: false,
        news: false,
    };
    cache().lock().unwrap().insert(root.to_path_buf(), entry);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_installed_tool_whose_files_are_there_is_used() {
        let dir = std::env::temp_dir().join(format!("ld-envl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(detect(&dir, |_| true), None);
        std::fs::write(dir.join("shell.nix"), "").unwrap();
        std::fs::write(dir.join(".tool-versions"), "").unwrap();
        assert_eq!(detect(&dir, |_| true), Some(Tool::Mise));
        assert_eq!(detect(&dir, |p| p == "nix"), Some(Tool::Nix));
        assert_eq!(detect(&dir, |_| false), None);
        std::fs::write(dir.join(".envrc"), "").unwrap();
        assert_eq!(detect(&dir, |_| true), Some(Tool::Direnv));
        assert_eq!(command(Tool::Nix, &dir).get_args().last().unwrap(), "shell.nix");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_the_tools_print_is_read() {
        let direnv = r#"{"DIRENV_DIR":"-/p","FOO":"bar baz","GONE":null}"#;
        let vars = parse(Tool::Direnv, direnv, None).unwrap();
        assert!(vars.contains(&("FOO".into(), Some("bar baz".into()))));
        assert!(vars.contains(&("GONE".into(), None)));
        assert_eq!(parse(Tool::Direnv, "", None).unwrap(), vec![]);
        let mise = r#"{"PATH":"/home/u/.local/share/mise/installs/node/22/bin:/usr/bin","NODE_ENV":"dev"}"#;
        assert_eq!(parse(Tool::Mise, mise, None).unwrap().len(), 2);
        // As `nix print-dev-env --json` prints it, shortened.
        let nix = r#"{"bashFunctions":{"genericBuild":"..."},"variables":{
            "PATH":{"type":"exported","value":"/nix/store/abc-cargo/bin"},
            "RUST_SRC_PATH":{"type":"exported","value":"/nix/store/def-rust-src"},
            "HOME":{"type":"exported","value":"/homeless-shelter"},
            "TMPDIR":{"type":"exported","value":"/tmp/nix-shell"},
            "buildInputs":{"type":"var","value":""},
            "outputs":{"type":"array","value":["out"]}}}"#;
        let vars = parse(Tool::Nix, nix, Some("/usr/bin")).unwrap();
        assert_eq!(vars.len(), 2, "{vars:?}");
        assert!(vars.contains(&("PATH".into(), Some("/nix/store/abc-cargo/bin:/usr/bin".into()))));
        assert!(parse(Tool::Mise, "not json", None).unwrap_err().contains("not JSON"));
        assert_eq!(complaint(Tool::Nix, "\nwarning: x\nerror: flake has no devShell\n", Some(1)), "warning: x error: flake has no devShell");
        assert_eq!(complaint(Tool::Mise, "", Some(2)), "mise exited with status 2");
        assert_eq!(complaint(Tool::Direnv, "\x1b[31mdirenv: error .envrc is blocked\x1b[0m\n", Some(1)), "direnv: error .envrc is blocked");
    }

    /// Needs direnv installed; skipped quietly where it is not.
    #[test]
    fn direnv_loads_an_allowed_envrc_and_reports_a_blocked_one() {
        if !on_path("direnv") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ld-direnv-{}", std::process::id()));
        let home = dir.join("home");
        let project = dir.join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(".envrc"), "export LD_FROM_ENVRC=\"$((40 + 2))\"\n").unwrap();
        let env = |cmd: &mut Command| {
            cmd.env("HOME", &home).env("XDG_DATA_HOME", home.join("data")).env("XDG_CONFIG_HOME", home.join("cfg"));
            cmd.env("XDG_CACHE_HOME", home.join("cache")).env_remove("DIRENV_DIR").env_remove("DIRENV_FILE");
        };
        let run = || {
            let mut cmd = command(Tool::Direnv, &project);
            env(&mut cmd);
            let out = cmd.output().unwrap();
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            (out.status.success() && !stderr.contains("is blocked"), stdout.into_owned(), stderr.into_owned())
        };
        let (ok, _, stderr) = run();
        assert!(!ok && complaint(Tool::Direnv, &stderr, None).contains("direnv allow"), "{stderr}");
        let mut allow = Command::new("direnv");
        allow.arg("allow").current_dir(&project);
        env(&mut allow);
        assert!(allow.status().unwrap().success());
        let (ok, stdout, stderr) = run();
        assert!(ok, "{stderr}");
        let vars = parse(Tool::Direnv, &stdout, None).unwrap();
        assert!(vars.contains(&("LD_FROM_ENVRC".into(), Some("42".into()))), "{vars:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
