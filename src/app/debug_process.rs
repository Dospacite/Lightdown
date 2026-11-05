//! Attaching to a running process, starting a script under debugpy, and
//! writing debug configurations from templates.
use super::*;
use serde_json::{Value, json};
use std::os::unix::fs::MetadataExt;

#[derive(Clone, Debug, PartialEq)]
struct Proc {
    pid: u32,
    user: String,
    command: String,
}

/// User names by uid, from /etc/passwd.
fn user_names() -> std::collections::HashMap<u32, String> {
    let text = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    text.lines().filter_map(|l| {
        let mut f = l.split(':');
        let name = f.next()?;
        Some((f.nth(1)?.parse().ok()?, name.to_string()))
    }).collect()
}

/// This user's processes from /proc, newest first. Kernel threads, which
/// have no command line, and Lightdown itself are left out.
fn processes() -> Vec<Proc> {
    let uid = unsafe { libc::getuid() };
    let me = std::process::id();
    let user = user_names().remove(&uid).unwrap_or_else(|| uid.to_string());
    let mut result = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid == me || !entry.metadata().is_ok_and(|m| m.uid() == uid) { continue; }
        let raw = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let command: String = String::from_utf8_lossy(&raw).trim_end_matches('\0').replace('\0', " ").chars().take(500).collect();
        if command.trim().is_empty() { continue; }
        result.push(Proc { pid, user: user.clone(), command });
        if result.len() >= 4096 { break; }
    }
    result.sort_by(|a, b| b.pid.cmp(&a.pid));
    result
}

/// Whether an ELF file has a section with one of `names`, reading only its
/// headers and section-name table.
fn elf_has_section(path: &Path, names: &[&str]) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let read = || -> Option<bool> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut h = [0u8; 64];
        f.read_exact(&mut h).ok()?;
        if &h[..4] != b"\x7fELF" || h[4] != 2 || h[5] != 1 { return Some(false); } // 64-bit little-endian only
        let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
        let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let (shoff, shentsize, shnum, shstrndx) = (u64_at(&h, 0x28), u16_at(&h, 0x3a), u16_at(&h, 0x3c), u16_at(&h, 0x3e));
        if shentsize < 64 || shnum == 0 || shnum > 4096 || shstrndx >= shnum { return Some(false); }
        let mut table = vec![0u8; (shentsize * shnum) as usize];
        f.seek(SeekFrom::Start(shoff)).ok()?;
        f.read_exact(&mut table).ok()?;
        let header = |i: u64| &table[(i * shentsize) as usize..][..64];
        let strings = header(shstrndx);
        let (str_off, str_size) = (u64_at(strings, 0x18), u64_at(strings, 0x20).min(1 << 20));
        let mut names_table = vec![0u8; str_size as usize];
        f.seek(SeekFrom::Start(str_off)).ok()?;
        f.read_exact(&mut names_table).ok()?;
        Some((0..shnum).any(|i| {
            let at = u32::from_le_bytes(header(i)[..4].try_into().unwrap()) as usize;
            let name = names_table.get(at..).and_then(|s| s.split(|&b| b == 0).next()).unwrap_or(&[]);
            names.iter().any(|n| n.as_bytes() == name)
        }))
    };
    read().unwrap_or(false)
}

/// The debug type and attach arguments for a process.
fn attach_config(pid: u32, exe: &Path, command: &str) -> Value {
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let first = command.split_whitespace().next().unwrap_or("");
    let python = |s: &str| s.rsplit('/').next().is_some_and(|n| n.starts_with("python"));
    if python(name) || python(first) {
        json!({"type":"python","request":"attach","launch":{"processId":pid,"justMyCode":false}})
    } else if name == "node" || first.rsplit('/').next() == Some("node") {
        json!({"type":"node","request":"attach","launch":{"type":"pwa-node","processId":pid.to_string()}})
    } else if elf_has_section(exe, &[".go.buildinfo", ".gopclntab"]) {
        json!({"type":"go","request":"attach","launch":{"mode":"local","processId":pid}})
    } else {
        json!({"type":"cpp","request":"attach","launch":{"pid":pid,"program":exe}})
    }
}

/// Why attaching may have been refused, from the kernel's ptrace setting.
pub(super) fn ptrace_hint() -> String {
    let scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok().and_then(|s| s.trim().parse::<u8>().ok());
    let why = match scope {
        Some(0) => "kernel.yama.ptrace_scope is 0, so check that the process is yours and not already being debugged".to_string(),
        Some(1) => "kernel.yama.ptrace_scope is 1, so only a parent process may attach. Launch the program under the debugger instead, or allow attaching until reboot with `sudo sysctl kernel.yama.ptrace_scope=0`".to_string(),
        Some(n) => format!("kernel.yama.ptrace_scope is {n}, which needs administrator rights (2) or forbids attaching (3)"),
        None => "check that the process is yours and that ptrace is allowed".to_string(),
    };
    format!(". Attaching uses ptrace; {why}")
}

impl App {
    pub(super) fn debug_extra_command(&mut self, action: &str) {
        match action {
            "attach" => self.debug_process_menu(),
            "template" => self.debug_template_menu(),
            "debugpy" => self.debugpy_start(),
            _ => {}
        }
    }
    pub(super) fn debug_process_menu(&mut self) {
        let entries = processes().into_iter()
            .map(|p| Entry::new(&p.pid.to_string(), &format!("{}  {}", p.pid, p.command), &p.user))
            .collect();
        let mut menu = Menu::new("Attach to process", entries, "");
        menu.hint = "Type to filter by pid or command line".into();
        self.menu = Some((MenuKind::DebugProcess, menu));
    }
    pub(super) fn debug_process_pick(&mut self, key: &str) {
        let Some(p) = processes().into_iter().find(|p| p.pid.to_string() == key) else {
            self.message = Some("That process has exited".into());
            return;
        };
        if self.debugger.dap.is_some() || self.debugger.process.is_some() {
            self.message = Some("Stop the current debug session first".into());
            return;
        }
        let exe = std::fs::read_link(format!("/proc/{}/exe", p.pid)).unwrap_or_default();
        let config = attach_config(p.pid, &exe, &p.command);
        self.message = Some(format!("Attaching to {} ({})", p.pid, config["type"].as_str().unwrap_or("")));
        self.native_launch(config);
    }
    /// Starts the current Python file under `python -m debugpy --listen`
    /// and attaches to it over loopback.
    pub(super) fn debugpy_start(&mut self) {
        let Some(path) = self.doc.path.clone().filter(|_| self.doc.doc == Doc::Python) else {
            self.message = Some("Open a saved Python file first".into());
            return;
        };
        if self.debugger.dap.is_some() || self.debugger.process.is_some() {
            self.message = Some("Stop the current debug session first".into());
            return;
        }
        let python = self.cfg.python_for(&self.root);
        let mut argv = vec![json!(python), json!("-m"), json!("debugpy"), json!("--listen"), json!("127.0.0.1:{port}"), json!("--wait-for-client"), json!(path)];
        argv.extend(self.debugger.options["args"].as_array().cloned().unwrap_or_default());
        let config = json!({"type":"python","request":"attach","adapter":argv,"transport":"tcp","launch":{"justMyCode":true,"redirectOutput":true}});
        if !std::process::Command::new(&python).args(["-c", "import debugpy"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success()) {
            self.message = Some(format!("Install debugpy for {python} first: {python} -m pip install debugpy"));
            return;
        }
        self.native_start_value(Some(config));
    }
    pub(super) fn debug_template_menu(&mut self) {
        let entries = super::debug_config::TEMPLATES.iter().map(|(k, label)| Entry::new(k, label, "")).collect();
        let mut menu = Menu::new("Debug configuration template", entries, "");
        menu.hint = "Adds an entry to .lightdown/debug.json".into();
        self.menu = Some((MenuKind::DebugTemplate, menu));
    }
    pub(super) fn debug_template_pick(&mut self, key: &str) {
        let path = self.root.join(".lightdown/debug.json");
        let result = (|| -> Result<String, String> {
            let open_and_edited = self.doc.path.as_ref() == Some(&path) && self.doc.buf.modified
                || self.file_tabs.iter().filter_map(|t| t.state.as_ref()).any(|s| s.pane.path.as_ref() == Some(&path) && s.pane.buf.modified);
            if open_and_edited { return Err("Save .lightdown/debug.json first".into()); }
            let mut file = match std::fs::read(&path) {
                Ok(b) if b.len() > 1024 * 1024 => return Err("Debug configuration exceeds 1 MB".into()),
                Ok(b) => serde_json::from_slice::<Value>(&b).map_err(|e| format!("debug.json: {e}; fix it first"))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
                Err(e) => return Err(e.to_string()),
            };
            let bin = std::fs::read_to_string(self.root.join("Cargo.toml")).ok().and_then(|t| super::debug_config::cargo_package(&t));
            let entry = super::debug_config::template(key, bin.as_deref())?;
            let name = super::debug_config::add_configuration(&mut file, entry)?;
            std::fs::create_dir_all(self.root.join(".lightdown")).map_err(|e| e.to_string())?;
            let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())? + "\n";
            super::write_file(&path, text.as_bytes()).map_err(|e| e.to_string())?;
            Ok(name)
        })();
        match result {
            Ok(name) => {
                self.jump_to(path, 0, 0);
                self.message = Some(format!("Added \"{name}\". Debug: Select named configuration chooses it"));
            }
            Err(e) => self.message = Some(format!("Debug: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_own_processes_with_user_names() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let list = processes();
        let found = list.iter().find(|p| p.pid == child.id()).expect("child listed");
        assert_eq!(found.command, "sleep 30");
        assert!(!found.user.is_empty());
        assert!(!list.iter().any(|p| p.pid == std::process::id()));
        child.kill().unwrap();
        child.wait().unwrap();
    }
    #[test]
    fn processes_choose_their_adapter() {
        let sleep = Path::new("/usr/bin/sleep");
        assert_eq!(attach_config(5, Path::new("/usr/bin/python3.13"), "python3 app.py")["type"], "python");
        assert_eq!(attach_config(5, sleep, "/home/u/.venv/bin/python x.py")["launch"]["processId"], 5);
        assert_eq!(attach_config(5, Path::new("/usr/bin/node"), "node app.js")["launch"], json!({"type":"pwa-node","processId":"5"}));
        let native = attach_config(5, sleep, "sleep 1");
        assert_eq!((native["type"].as_str(), native["launch"]["pid"].as_u64()), (Some("cpp"), Some(5)));
        // Reading section names works on a real ELF file: our own test binary.
        let me = std::env::current_exe().unwrap();
        assert!(elf_has_section(&me, &[".text"]));
        assert!(!elf_has_section(&me, &[".go.buildinfo"]));
        assert!(!elf_has_section(Path::new("/etc/passwd"), &[".text"]));
    }
    #[test]
    fn ptrace_hint_names_the_setting() {
        assert!(ptrace_hint().contains("ptrace"));
    }
    #[test]
    #[ignore = "needs gdb 14 or newer and kernel.yama.ptrace_scope 1"]
    fn gdb_attach_refusal_explains_ptrace_scope() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let root = std::env::temp_dir().join(format!("ld-attach-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let mut a = App::new(Some(root.join("x.c")));
        a.native_launch(json!({"type":"gdb","request":"attach","launch":{"pid":child.id()}}));
        let start = std::time::Instant::now();
        while !a.debugger.output.contains("ptrace_scope") && start.elapsed().as_secs() < 20 {
            a.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(a.debugger.output.contains("kernel.yama.ptrace_scope is 1"), "{}", a.debugger.output);
        child.kill().unwrap();
        child.wait().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "needs a Python with debugpy; set LD_DEBUGPY_PYTHON to it"]
    fn debugpy_listen_stops_at_a_breakpoint() {
        let python = std::env::var("LD_DEBUGPY_PYTHON").unwrap_or_else(|_| "python3".into());
        let root = std::env::temp_dir().join(format!("ld-debugpy-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let script = root.join("app.py");
        std::fs::write(&script, "answer = 6 * 7\nprint('result', answer)\n").unwrap();
        let mut a = App::new(Some(script.clone()));
        a.resize(1200, 800, 1.0);
        a.cfg.project_mut(&root).python = python;
        let at = a.doc.buf.starts[1];
        a.doc.select(at, at);
        a.debug_command("breakpoint");
        a.debug_command("debugpy");
        let start = std::time::Instant::now();
        while !(a.debugger.paused && a.debugger.snapshot["variables"].as_array().is_some_and(|v| v.iter().any(|v| v["name"] == "answer"))) && start.elapsed().as_secs() < 30 {
            a.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(a.debugger.view.buf.text.contains("answer"), "{:?} {}", a.message, a.debugger.output);
        a.debug_command("continue");
        while a.debugger.dap.is_some() && start.elapsed().as_secs() < 30 { a.debug_poll(); std::thread::sleep(std::time::Duration::from_millis(10)); }
        assert!(a.debugger.output.contains("result 42"), "{}", a.debugger.output);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "needs a Python with debugpy; set LD_DEBUGPY_PYTHON to it"]
    fn python_template_finds_debugpy_adapter() {
        let python = std::env::var("LD_DEBUGPY_PYTHON").unwrap_or_else(|_| "python3".into());
        let root = std::env::temp_dir().join(format!("ld-debugpy-template-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let script = root.join("app.py");
        std::fs::write(&script, "answer = 6 * 7\nprint('result', answer)\n").unwrap();
        let mut a = App::new(Some(script.clone()));
        a.resize(1200, 800, 1.0);
        a.cfg.project_mut(&root).python = python;
        a.debug_template_pick("python");
        assert!(root.join(".lightdown/debug.json").is_file());
        a.open_tab(script);
        let at = a.doc.buf.starts[1];
        a.doc.select(at, at);
        a.debug_command("breakpoint");
        a.debug_command("native");
        let start = std::time::Instant::now();
        while !(a.debugger.paused && a.debugger.snapshot["variables"].as_array().is_some_and(|v| v.iter().any(|v| v["name"] == "answer"))) && start.elapsed().as_secs() < 30 {
            a.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(a.debugger.view.buf.text.contains("answer"), "{:?} {}", a.message, a.debugger.output);
        a.debug_command("stop");
        std::fs::remove_dir_all(root).unwrap();
    }
}
