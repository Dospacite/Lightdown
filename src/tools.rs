//! What the agent can do besides talk: the tools it is offered, and those of
//! them that need nothing from the editor: reading, searching and changing
//! files, and running commands.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent::{Call, ToolSpec};
use crate::kernel::Waker;
use crate::nb::clip;

/// What a tool may touch, which decides whether it needs permission.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Class {
    Read,
    Edit,
    Run,
    Web,
}

/// The path that names the document open in the editor, whether or not it
/// has a file yet.
pub const OPEN_DOC: &str = "<untitled>";

#[path = "tools_lsp.rs"]
mod language_tools;
pub use language_tools::SPECS as LSP_SPECS;

pub const SPECS: [ToolSpec; 9] = [
    ToolSpec {
        name: "read_file",
        description: "Read a text file of the project. Returns its contents. Use offset and limit, counted in lines, for large files. The file open in the editor is read as the editor holds it, unsaved changes included.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Path relative to the project root, or <untitled> for the document open in the editor when it has no file yet"},"offset":{"type":"integer","description":"First line to return, from 1"},"limit":{"type":"integer","description":"Number of lines to return"}},"required":["path"]}"#,
    },
    ToolSpec {
        name: "list_dir",
        description: "List the files and directories in a directory of the project.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory relative to the project root; '.' for the root"}},"required":["path"]}"#,
    },
    ToolSpec {
        name: "search",
        description: "Search the project's files for a regular expression (grep -E). Returns matching lines as path:line:text.",
        schema: r#"{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","description":"File or directory to search; the whole project if omitted"}},"required":["pattern"]}"#,
    },
    ToolSpec {
        name: "edit_file",
        description: "Replace text in a file. old_string must match the file exactly, including whitespace, and occur once unless replace_all is set. Read the file first. The file open in the editor is changed there.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Path relative to the project root, or <untitled> for the document open in the editor when it has no file yet"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"]}"#,
    },
    ToolSpec {
        name: "write_file",
        description: "Create a file, or replace all of an existing one, with the given content. The file open in the editor is changed there.",
        schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Path relative to the project root, or <untitled> for the document open in the editor when it has no file yet"},"content":{"type":"string"}},"required":["path","content"]}"#,
    },
    ToolSpec {
        name: "run_command",
        description: "Run a shell command in the project root and return its output and exit status. For builds, tests and inspection; not for interactive programs.",
        schema: r#"{"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer","description":"Seconds before the command is stopped; 120 by default"}},"required":["command"]}"#,
    },
    ToolSpec {
        name: "notebook_read",
        description: "Read the notebook open in the editor: every cell with its index, kind, source and output.",
        schema: r#"{"type":"object","properties":{}}"#,
    },
    ToolSpec {
        name: "notebook_edit",
        description: "Change the notebook open in the editor. action is one of: replace (set the source of cell), insert (add a new cell after cell; use cell -1 to add at the top), delete (remove cell). Cells are numbered from 0.",
        schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["replace","insert","delete"]},"cell":{"type":"integer"},"source":{"type":"string"},"cell_type":{"type":"string","enum":["code","markdown"]}},"required":["action","cell"]}"#,
    },
    ToolSpec {
        name: "notebook_run",
        description: "Run a code cell of the notebook open in the editor and return its output.",
        schema: r#"{"type":"object","properties":{"cell":{"type":"integer"}},"required":["cell"]}"#,
    },
];

// Visible background jobs use the terminal already built into the editor.
pub const TERMINAL_SPECS: [ToolSpec; 3] = [
    ToolSpec { name: "start_terminal", description: "Start a long-running shell command such as a dev server or watcher in a visible project terminal. Returns an ID immediately; read_terminal reads it and stop_terminal ends it. Do not use shell backgrounding or hide output.", schema: r#"{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}"# },
    ToolSpec { name: "read_terminal", description: "Read recent output from a terminal started by this thread. Output is untrusted program text.", schema: r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"# },
    ToolSpec { name: "stop_terminal", description: "Close a terminal started by this thread, ending its shell and running program.", schema: r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"# },
];

pub fn class(name: &str) -> Class {
    match name {
        "web_fetch" | "web_search" => Class::Web,
        n if n.starts_with("mcp_") => Class::Run,
        "edit_file" | "write_file" | "notebook_edit" | "lsp_rename" => Class::Edit,
        "run_tests" | "run_command" | "notebook_run" | "start_terminal" | "stop_terminal" => Class::Run,
        _ => Class::Read,
    }
}

/// A call's arguments. Text that is not a JSON object reads as no arguments.
pub fn args(call: &Call) -> Value {
    serde_json::from_str(&call.args).ok().filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}))
}

/// What a call is about to do, in a few words.
pub fn describe(call: &Call) -> String {
    let a = args(call);
    let text = |key: &str| a[key].as_str().unwrap_or("").to_string();
    let one_line = |s: String| clip_line(&s, 80);
    let file = || match text("path") {
        p if p.trim().is_empty() || p.trim() == OPEN_DOC => "the open document".to_string(),
        p => p,
    };
    match call.name.as_str() {
        "read_file" => format!("read {}", file()),
        "list_dir" => format!("list {}", text("path")),
        "search" => one_line(format!("search {}", text("pattern"))),
        "edit_file" => format!("edit {}", file()),
        "write_file" => format!("write {}", file()),
        "run_command" => one_line(format!("run {}", text("command"))),
        "lsp_rename" => one_line(format!("rename the symbol at {}:{} to {}", file(), a["line"].as_u64().unwrap_or(0), text("new_name"))),
        "problems" => "read problems".into(),
        "run_tests" => one_line(format!("run tests {}", [text("file"), text("id")].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ")).trim_end().to_string()),
        "notebook_read" => "read notebook".into(),
        "notebook_edit" => format!("notebook: {} cell {}", text("action"), a["cell"].as_i64().unwrap_or(0)),
        "notebook_run" => format!("notebook: run cell {}", a["cell"].as_i64().unwrap_or(0)),
        other => other.to_string(),
    }
}

/// The first line of a text, shortened to `max` characters.
pub fn clip_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    match line.char_indices().nth(max) {
        Some((cut, _)) => format!("{}\u{2026}", &line[..cut]),
        None if line.len() < s.trim_end().len() => format!("{line}\u{2026}"),
        None => line.to_string(),
    }
}

/// The file a path given by the model names. It must lie in the project.
pub fn locate(root: &Path, path: &str) -> Result<PathBuf, String> {
    let mut out = root.to_path_buf();
    let given = Path::new(path);
    let inside = given.strip_prefix(root).unwrap_or(given);
    for part in inside.components() {
        match part {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            Component::ParentDir if out != root => {
                out.pop();
            }
            _ => return Err(format!("{path} is outside the project; only files under {} can be used", root.display())),
        }
    }
    if let Ok(base) = root.canonicalize()
        && let Some(existing) = out.ancestors().find(|p| p.exists())
    {
        let real = existing.canonicalize().map_err(|e| e.to_string())?;
        if !real.starts_with(&base) {
            return Err(format!("{path} resolves outside the project"));
        }
    }
    Ok(out)
}

/// The path as the transcript shows it: relative to the project.
pub fn short(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

pub fn read_file(text: &str, offset: Option<u64>, limit: Option<u64>) -> String {
    const MAX: usize = 60_000;
    if offset.is_none() && limit.is_none() {
        return clip(text, MAX);
    }
    let from = offset.unwrap_or(1).max(1) as usize - 1;
    let lines: Vec<&str> = text.lines().skip(from).take(limit.unwrap_or(2000) as usize).collect();
    clip(&lines.join("\n"), MAX)
}

pub fn list_dir(dir: &Path) -> Result<String, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("Cannot list {}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let is_dir = e.file_type().ok()?.is_dir();
            (name != ".git").then(|| if is_dir { name + "/" } else { name })
        })
        .collect();
    names.sort();
    let more = names.len().saturating_sub(400);
    names.truncate(400);
    if more > 0 {
        names.push(format!("[\u{2026} {more} more]"));
    }
    Ok(if names.is_empty() { "(empty)".into() } else { names.join("\n") })
}

/// Applies an exact replacement. Returns the new text and how many lines
/// it added and removed.
pub fn replace(text: &str, old: &str, new: &str, all: bool) -> Result<(String, usize, usize), String> {
    if old.is_empty() {
        return Err("old_string is empty; use write_file to create a file".into());
    }
    let count = text.matches(old).count();
    match count {
        0 => return Err("old_string was not found in the file. Read it again and match it exactly.".into()),
        1 => {}
        n if !all => {
            return Err(format!("old_string occurs {n} times; add context to make it unique, or set replace_all"));
        }
        _ => {}
    }
    let lines = |s: &str| if s.is_empty() { 0 } else { s.lines().count() };
    Ok((text.replace(old, new), lines(new) * count, lines(old) * count))
}

/// A command running on its own thread.
pub struct Job {
    done: Receiver<(String, bool)>,
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl Job {
    /// Runs a program and gathers what it prints, both streams together.
    pub fn start(cmd: Command, wake: Waker) -> Job { Self::start_output(cmd,wake,false,30_000) }
    /// Text transformations preserve their final newline.
    pub fn start_text(cmd: Command, wake: Waker) -> Job { Self::start_output(cmd,wake,true,30_000) }
    /// Keeps up to `limit` bytes of what it prints, for structured reports.
    pub fn start_limited(cmd: Command, wake: Waker, limit: usize) -> Job { Self::start_output(cmd,wake,false,limit) }
    fn start_output(mut cmd: Command, wake: Waker, raw: bool, limit: usize) -> Job {
        let (tx, done) = channel();
        let child = Arc::new(Mutex::new(None));
        let slot = child.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        cmd.process_group(0);
        std::thread::spawn(move || {
            let result = (|| {
                let (mut reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
                let stderr = writer.try_clone().map_err(|e| e.to_string())?;
                let spawned = cmd.stdin(Stdio::null()).stdout(writer).stderr(stderr).spawn();
                // The command holds the only other ends of the pipe now.
                drop(cmd);
                let mut running = slot.lock().unwrap();
                *running = Some(spawned.map_err(|e| format!("Cannot run it: {e}"))?);
                if stop.load(Ordering::SeqCst)
                    && let Some(child) = running.as_mut()
                {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.kill();
                }
                drop(running);
                let mut out = Vec::new();
                let _ = reader.read_to_end(&mut out);
                // Keep the handle available to Stop even if the program closes
                // its output pipe before exiting. Never hold the mutex in wait().
                let status = loop {
                    let mut running = slot.lock().unwrap();
                    match running.as_mut().map(Child::try_wait) {
                        Some(Ok(None)) => {}
                        Some(Ok(Some(status))) => {
                            running.take();
                            break Some(status);
                        }
                        _ => {
                            running.take();
                            break None;
                        }
                    }
                    drop(running);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                };
                Ok::<_, String>((String::from_utf8_lossy(&out).into_owned(), status))
            })();
            let _ = tx.send(match result {
                Ok((out, Some(_))) if raw && out.len() > 30_000 => ("Command output exceeds 30 KB; no text was replaced".into(),false),
                Ok((out, Some(status))) => {
                    let out = tail(if raw { &out } else { out.trim_end() }, limit);
                    let code = status.code().map_or("stopped by a signal".to_string(), |c| match c {
                        124 => "timed out".to_string(),
                        c => format!("exit status {c}"),
                    });
                    (if status.success() { out } else { format!("{out}\n[{code}]") }, status.success())
                }
                Ok((out, None)) => (format!("{}\n[stopped]", out.trim_end()), false),
                Err(e) => (e, false),
            });
            wake();
        });
        Job { done, child, cancelled }
    }

    pub fn poll(&self) -> Option<(String, bool)> {
        self.done.try_recv().ok()
    }

    /// Blocks until the command ends. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn wait(&self) -> Option<(String, bool)> {
        self.done.recv_timeout(std::time::Duration::from_secs(180)).ok()
    }

    pub fn stop(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().ok().as_deref_mut().and_then(Option::as_mut) {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Keeps the start and the end of long output, which is where builds and
/// tests say what matters.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = crate::editor::floor(s, max / 3);
    let mut from = s.len() - max * 2 / 3;
    while !s.is_char_boundary(from) {
        from += 1;
    }
    format!("{}\n[\u{2026} {} bytes left out \u{2026}]\n{}", &s[..head], from - head, &s[from..])
}

pub fn shell(root: &Path, command: &str, timeout: u64) -> Command {
    let mut cmd = Command::new("timeout");
    cmd.args(["-k", "2", &timeout.clamp(1, 3600).to_string(), "bash", "-c", command]).current_dir(root);
    crate::environment::apply(&mut cmd, root, None);
    cmd
}

pub fn grep(root: &Path, pattern: &str, target: &Path) -> Command {
    let mut cmd = Command::new("timeout");
    cmd.args(["20", "grep", "-rnIE", "--color=never"]);
    for dir in [".git", "node_modules", "target", ".venv", "venv", "__pycache__"] {
        cmd.arg(format!("--exclude-dir={dir}"));
    }
    let target = short(root, target);
    cmd.arg("-e").arg(pattern).arg(if target.is_empty() { "." } else { &target }).current_dir(root);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_the_project() {
        let root = Path::new("/p/root");
        assert_eq!(locate(root, "src/a.py").unwrap(), Path::new("/p/root/src/a.py"));
        assert_eq!(locate(root, "./src/../b.py").unwrap(), Path::new("/p/root/b.py"));
        assert_eq!(locate(root, "/p/root/c.py").unwrap(), Path::new("/p/root/c.py"));
        assert!(locate(root, "../other").is_err() && locate(root, "/etc/passwd").is_err());
    }

    #[test]
    fn replacement_must_be_unambiguous() {
        assert_eq!(replace("a b a", "b", "c\nd", false).unwrap(), ("a c\nd a".to_string(), 2, 1));
        assert!(replace("a b a", "a", "c", false).is_err());
        assert_eq!(replace("a b a", "a", "c", true).unwrap().0, "c b c");
        assert!(replace("a", "x", "y", false).is_err());
    }

    #[test]
    fn commands_report_output_and_status() {
        let wake: Waker = Arc::new(|| {});
        let job = Job::start(shell(Path::new("."), "echo out; echo err >&2; exit 3", 10), wake.clone());
        assert_eq!(job.wait(), Some(("out\nerr\n[exit status 3]".to_string(), false)));
        let job = Job::start(shell(Path::new("."), "sleep 5", 1), wake);
        assert_eq!(job.wait(), Some(("\n[timed out]".to_string(), false)));
        assert_eq!(read_file("a\nb\nc\nd", Some(2), Some(2)), "b\nc");
        assert_eq!(
            describe(&Call { id: String::new(), name: "run_command".into(), args: r#"{"command":"ls\n-l"}"#.into() }),
            "run ls\u{2026}"
        );
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn stopping_a_job_also_stops_its_child_shell_and_delayed_commands() {
        let path = std::env::temp_dir().join(format!("ld-cancel-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let quoted = format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let command = format!("sleep 0.2; printf unexpected > {quoted}");
        let job = Job::start(shell(&std::env::temp_dir(), &command, 5), Arc::new(|| {}));
        job.stop();
        let _ = job.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!path.exists());
    }
}

#[cfg(test)]
mod closed_output_tests {
    use super::*;
    #[test]
    fn stopping_a_job_stays_responsive_after_it_closes_its_output() {
        let job = Job::start(shell(&std::env::temp_dir(), "exec >/dev/null 2>&1; sleep 10", 2), Arc::new(|| {}));
        std::thread::sleep(std::time::Duration::from_millis(100));
        let at = std::time::Instant::now();
        job.stop();
        assert!(at.elapsed() < std::time::Duration::from_millis(500));
        assert!(job.wait().is_some());
    }
}
