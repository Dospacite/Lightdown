//! Debug configuration selection and literal variable expansion. No shell is involved.
use serde_json::Value;
use std::path::Path;

pub(super) fn select(value: Value, name: Option<&str>) -> Result<Value, String> {
    let Some(configs) = value.get("configurations") else { return Ok(value) };
    let configs = configs.as_array().ok_or("configurations must be an array")?;
    if configs.is_empty() || configs.len() > 100 { return Err("Provide between 1 and 100 debug configurations".into()); }
    let wanted = name.or_else(|| value["default"].as_str());
    let config = if let Some(name) = wanted {
        configs.iter().find(|c| c["name"].as_str() == Some(name)).ok_or_else(|| format!("Unknown debug configuration: {name}"))?
    } else { &configs[0] };
    Ok(config.clone())
}

pub(super) fn expand(value: &mut Value, root: &Path, file: Option<&Path>) -> Result<(), String> {
    match value {
        Value::String(s) => {
            let mut result = String::new();
            let mut rest = s.as_str();
            while let Some(start) = rest.find("${") {
                result.push_str(&rest[..start]);
                let tail = &rest[start + 2..];
                let end = tail.find('}').ok_or("Unclosed debug variable")?;
                let key = &tail[..end];
                let replacement = match key {
                    "workspaceFolder" => root.to_string_lossy().into_owned(),
                    "file" => file.ok_or("Debug variable file requires a saved file")?.to_string_lossy().into_owned(),
                    "fileDirname" => file.and_then(Path::parent).ok_or("Debug variable fileDirname requires a saved file")?.to_string_lossy().into_owned(),
                    "relativeFile" => file.and_then(|f| f.strip_prefix(root).ok()).ok_or("Debug file must be within the workspace")?.to_string_lossy().into_owned(),
                    _ if key.starts_with("env:") => std::env::var(&key[4..]).map_err(|_| format!("Missing environment variable: {}", &key[4..]))?,
                    _ => return Err(format!("Unknown debug variable: {key}")),
                };
                result.push_str(&replacement);
                if result.len() > 1024 * 1024 { return Err("Expanded debug value exceeds 1 MB".into()); }
                rest = &tail[end + 1..];
            }
            result.push_str(rest);
            *s = result;
        }
        Value::Array(items) => for item in items { expand(item, root, file)?; },
        Value::Object(items) => for item in items.values_mut() { expand(item, root, file)?; },
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn named_default_and_legacy_configurations() {
        let value = json!({"default":"two","configurations":[{"name":"one"},{"name":"two"}]});
        assert_eq!(select(value.clone(), None).unwrap()["name"], "two");
        assert_eq!(select(value.clone(), Some("one")).unwrap()["name"], "one");
        assert!(select(value, Some("missing")).is_err());
        assert!(select(json!({"configurations":[]}), None).is_err());
        assert_eq!(select(json!({"launch":{}}), None).unwrap(), json!({"launch":{}}));
    }
    #[test]
    fn expands_nested_values_without_shell_or_recursive_expansion() {
        let mut value = json!({"program":"${file}","args":["${relativeFile}","${fileDirname}","${workspaceFolder}","$(echo untouched)"]});
        expand(&mut value, Path::new("/project"), Some(Path::new("/project/src/main.rs"))).unwrap();
        assert_eq!(value["args"], json!(["src/main.rs","/project/src","/project","$(echo untouched)"]));
        assert!(expand(&mut json!("${unknown}"), Path::new("/"), None).is_err());
        assert!(expand(&mut json!("${file}"), Path::new("/"), None).is_err());
        assert!(expand(&mut json!("${file"), Path::new("/"), None).is_err());
    }
}

/// The first executable called `name` on PATH.
fn on_path(name: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}
/// Runs a short probe, such as `gdb --version`, with a time limit.
fn probe(argv: &[&str]) -> Option<(bool, String)> {
    let mut child = std::process::Command::new(argv[0]).args(&argv[1..]).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                use std::io::Read;
                child.stdout.take()?.take(64 * 1024).read_to_string(&mut out).ok()?;
                return Some((status.success(), out));
            }
            Ok(None) if start.elapsed().as_secs() < 5 => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => { let _ = child.kill(); let _ = child.wait(); return None; }
        }
    }
}
/// GDB's major version from `gdb --version`, such as 14 for "GNU gdb (GDB) 14.2".
fn gdb_major(text: &str) -> Option<u32> {
    let line = text.lines().next()?;
    line.split_whitespace().rev().find_map(|w| w.split('.').next()?.parse().ok().filter(|_| w.contains('.')))
}
/// Debug types Lightdown can find an adapter for, and the adapters tried in order.
const KNOWN: &[(&[&str], &[&str])] = &[
    (&["lldb", "rust", "c", "cpp"], &["lldb-dap", "codelldb", "gdb"]),
    (&["lldb-dap"], &["lldb-dap"]),
    (&["codelldb"], &["codelldb"]),
    (&["gdb"], &["gdb"]),
    (&["go", "delve", "dlv"], &["dlv"]),
    (&["node", "pwa-node", "javascript", "js-debug"], &["js-debug"]),
    (&["python", "debugpy"], &["debugpy"]),
];
/// The adapter argv and transport for one adapter name, if it is installed.
fn adapter(name: &str, python: &str) -> Option<(Value, &'static str)> {
    use serde_json::json;
    match name {
        "lldb-dap" => ["lldb-dap", "lldb-vscode"].into_iter().find_map(on_path).map(|p| (json!([p]), "stdio")),
        "codelldb" => on_path("codelldb").map(|p| (json!([p, "--port", "{port}"]), "tcp")),
        "gdb" => {
            let gdb = on_path("gdb")?;
            let version = probe(&[&gdb.to_string_lossy(), "--version"]).and_then(|(_, out)| gdb_major(&out))?;
            (version >= 14).then(|| (json!([gdb, "-q", "-i", "dap"]), "stdio"))
        }
        "dlv" => on_path("dlv").map(|p| (json!([p, "dap", "--listen=127.0.0.1:{port}"]), "tcp")),
        "js-debug" => {
            if let Some(p) = ["js-debug-adapter", "dapDebugServer"].into_iter().find_map(on_path) { return Some((json!([p, "{port}", "127.0.0.1"]), "tcp")); }
            let script = on_path("dapDebugServer.js")?;
            Some((json!([on_path("node")?, script, "{port}", "127.0.0.1"]), "tcp"))
        }
        "debugpy" => probe(&[python, "-c", "import debugpy"]).filter(|(ok, _)| *ok).map(|_| (json!([python, "-m", "debugpy.adapter"]), "stdio")),
        _ => None,
    }
}
/// Fills in `adapter` and `transport` from PATH for a known `type` when the
/// configuration leaves `adapter` out.
pub(super) fn discover_adapter(value: &mut Value, python: &str) -> Result<(), String> {
    use serde_json::json;
    if value.get("adapter").is_some() { return Ok(()); }
    let kind = value["type"].as_str().ok_or("Set \"adapter\" to an argv array, or \"type\" to lldb, gdb, codelldb, go, node or python")?;
    let (_, names) = KNOWN.iter().find(|(kinds, _)| kinds.contains(&kind)).ok_or_else(|| format!("Unknown debug type {kind}; set \"adapter\" to an argv array"))?;
    let (argv, transport) = names.iter().find_map(|n| adapter(n, python)).ok_or_else(|| {
        let want = names.iter().map(|n| match *n {
            "gdb" => "gdb 14 or newer", "js-debug" => "js-debug's dapDebugServer", "debugpy" => "debugpy in the project's Python", n => n,
        }).collect::<Vec<_>>().join(" or ");
        format!("No debug adapter for type {kind} was found on PATH; install {want}, or set \"adapter\"")
    })?;
    value["adapter"] = argv;
    if value.get("transport").is_none() { value["transport"] = json!(transport); }
    Ok(())
}
/// The `cargo` argv for a configuration's `"cargo": {"args": [...]}`.
pub(super) fn cargo_command(cargo: &Value) -> Result<Vec<String>, String> {
    let args = cargo["args"].as_array().ok_or("cargo.args must be a string array, such as [\"build\", \"--bin\", \"app\"]")?;
    if args.is_empty() || args.len() > 64 { return Err("cargo.args needs 1 to 64 arguments".into()); }
    let mut argv = vec!["cargo".to_string()];
    for a in args { argv.push(a.as_str().ok_or("cargo.args must contain strings")?.to_string()); }
    if argv.iter().any(|a| a.starts_with("--message-format")) { return Err("Leave --message-format out of cargo.args; Lightdown sets it".into()); }
    argv.push("--message-format=json".into());
    Ok(argv)
}
/// The executable a `cargo --message-format=json` build produced, or the
/// compiler's errors when it produced none. Build scripts are skipped.
pub(super) fn cargo_executable(output: &str, ok: bool) -> Result<String, String> {
    let (mut found, mut errors) = (Vec::<String>::new(), String::new());
    for line in output.lines().filter(|l| l.starts_with('{')) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["reason"].as_str() {
            Some("compiler-artifact") if !v["target"]["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "custom-build")) => {
                if let Some(exe) = v["executable"].as_str() && !found.iter().any(|f| f == exe) { found.push(exe.into()); }
            }
            Some("compiler-message") if v["message"]["level"] == "error" && errors.len() < 20_000 => {
                errors += v["message"]["rendered"].as_str().unwrap_or("");
            }
            _ => {}
        }
    }
    if !ok {
        let plain: String = output.lines().filter(|l| !l.starts_with('{')).collect::<Vec<_>>().join("\n");
        return Err(format!("Cargo build failed\n{}", if errors.is_empty() { plain } else { errors }));
    }
    match found.len() {
        0 => Err("Cargo built no executable; add --bin NAME, --example NAME, or use test --no-run".into()),
        1 => Ok(found.remove(0)),
        _ => Err(format!("Cargo built {} executables; choose one with --bin NAME:\n{}", found.len(), found.join("\n"))),
    }
}
/// Templates for "Debug: Create configuration from template": key, label, entry.
pub(super) const TEMPLATES: [(&str, &str); 6] = [
    ("lldb", "Rust, C or C++ with lldb-dap"),
    ("gdb", "Rust, C or C++ with GDB 14 or newer"),
    ("go", "Go with Delve"),
    ("node", "Node with js-debug"),
    ("python", "Python with debugpy"),
    ("python-attach", "Python: attach to debugpy listening on port 5678"),
];
/// One configuration entry for a template. `cargo_bin` names the package's
/// binary when the project is a Cargo project.
pub(super) fn template(key: &str, cargo_bin: Option<&str>) -> Result<Value, String> {
    use serde_json::json;
    let native = |kind: &str| {
        let mut launch = json!({"cwd":"${workspaceFolder}","args":[]});
        if kind == "gdb" { launch["stopAtBeginningOfMainSubprogram"] = json!(true); }
        let mut entry = json!({"type":kind,"request":"launch","launch":launch});
        match cargo_bin {
            Some(bin) => { entry["name"] = json!(format!("Debug {bin}")); entry["cargo"] = json!({"args":["build","--bin",bin]}); }
            None => { entry["name"] = json!(format!("Debug program ({kind})")); entry["launch"]["program"] = json!("${workspaceFolder}/a.out"); }
        }
        entry
    };
    Ok(match key {
        "lldb" | "gdb" => native(key),
        "go" => json!({"name":"Debug Go package","type":"go","request":"launch","launch":{"mode":"debug","program":"${workspaceFolder}"}}),
        "node" => json!({"name":"Debug current Node file","type":"node","request":"launch","launch":{"type":"pwa-node","program":"${file}","cwd":"${workspaceFolder}","console":"internalConsole"}}),
        "python" => json!({"name":"Debug current Python file","type":"python","request":"launch","launch":{"program":"${file}","cwd":"${workspaceFolder}","console":"internalConsole","justMyCode":true}}),
        "python-attach" => json!({"name":"Attach to debugpy on 5678","type":"python","request":"attach","launch":{"connect":{"host":"127.0.0.1","port":5678},"justMyCode":true}}),
        _ => return Err(format!("Unknown debug template: {key}")),
    })
}
/// Adds `entry` to a debug.json value without replacing any existing entry.
/// A single-object file becomes the first named configuration. Returns the
/// name the entry was given, made unique with a number.
pub(super) fn add_configuration(file: &mut Value, mut entry: Value) -> Result<String, String> {
    use serde_json::json;
    if !file.is_object() { return Err("debug.json must contain an object".into()); }
    if file.get("configurations").is_none() {
        let mut old = std::mem::take(file);
        *file = if old.as_object().is_some_and(|o| o.is_empty()) { json!({"configurations":[]}) } else {
            if old.get("name").is_none() { old["name"] = json!("default"); }
            json!({"configurations":[old]})
        };
    }
    let list = file["configurations"].as_array_mut().ok_or("configurations must be an array")?;
    if list.len() >= 100 { return Err("debug.json already has 100 configurations".into()); }
    let base = entry["name"].as_str().unwrap_or("Debug").to_string();
    let mut name = base.clone();
    for n in 2.. { if !list.iter().any(|c| c["name"] == name.as_str()) { break; } name = format!("{base} {n}"); }
    entry["name"] = json!(name);
    list.push(entry);
    Ok(name)
}
/// The package name in a Cargo.toml, used as the template's binary.
pub(super) fn cargo_package(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') { in_package = line == "[package]"; continue; }
        if in_package && let Some(rest) = line.strip_prefix("name") && let Some(v) = rest.trim_start().strip_prefix('=') {
            return Some(v.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}
#[cfg(test)]
mod discovery_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn explicit_adapters_are_kept_and_unknown_types_explain_themselves() {
        let mut v = json!({"adapter":["custom"],"type":"lldb"});
        discover_adapter(&mut v, "python3").unwrap();
        assert_eq!(v["adapter"], json!(["custom"]));
        let err = discover_adapter(&mut json!({"type":"cobol"}), "python3").unwrap_err();
        assert!(err.contains("Unknown debug type cobol"), "{err}");
        assert!(discover_adapter(&mut json!({}), "python3").is_err());
        // A Python without debugpy reports what to install.
        let err = discover_adapter(&mut json!({"type":"python"}), "/nonexistent/python").unwrap_err();
        assert!(err.contains("debugpy"), "{err}");
    }
    #[test]
    fn gdb_versions_parse() {
        assert_eq!(gdb_major("GNU gdb (GDB) 18.1\nCopyright"), Some(18));
        assert_eq!(gdb_major("GNU gdb (Ubuntu 12.1-0ubuntu1~22.04) 12.1"), Some(12));
        assert_eq!(gdb_major("GNU gdb (GDB) Fedora Linux 14.2-1.fc40"), Some(14));
    }
    #[test]
    #[ignore = "needs gdb 14 or newer on PATH"]
    fn finds_gdb_on_path() {
        let mut v = json!({"type":"gdb"});
        discover_adapter(&mut v, "python3").unwrap();
        assert_eq!(v["transport"], "stdio");
        assert_eq!(v["adapter"][2], "-i");
    }
    #[test]
    fn cargo_builds_report_one_executable_or_errors() {
        let artifact = |kind: &str, exe: Option<&str>| json!({"reason":"compiler-artifact","target":{"kind":[kind]},"executable":exe}).to_string();
        let out = [artifact("custom-build", Some("/t/build-script-build")), artifact("lib", None), artifact("bin", Some("/t/debug/app")), json!({"reason":"build-finished","success":true}).to_string()].join("\n");
        assert_eq!(cargo_executable(&out, true).unwrap(), "/t/debug/app");
        let two = format!("{out}\n{}", artifact("bin", Some("/t/debug/other")));
        assert!(cargo_executable(&two, true).unwrap_err().contains("--bin NAME"));
        assert!(cargo_executable("", true).is_err());
        let failed = json!({"reason":"compiler-message","message":{"level":"error","rendered":"error[E0425]: cannot find value `x`\n --> src/main.rs:2:5\n"}}).to_string();
        let err = cargo_executable(&format!("{failed}\nerror: could not compile"), false).unwrap_err();
        assert!(err.contains("src/main.rs:2:5") && !err.contains("could not compile"), "{err}");
        assert_eq!(cargo_command(&json!({"args":["build","--bin","app"]})).unwrap(), ["cargo","build","--bin","app","--message-format=json"]);
        assert!(cargo_command(&json!({"args":["build","--message-format=short"]})).is_err());
        assert!(cargo_command(&json!({"args":"build"})).is_err());
    }
    #[test]
    fn templates_extend_files_without_replacing_entries() {
        for (key, _) in TEMPLATES { assert!(template(key, None).unwrap()["launch"].is_object()); }
        assert!(template("unknown", None).is_err());
        let rust = template("lldb", Some("app")).unwrap();
        assert_eq!(rust["cargo"]["args"], json!(["build","--bin","app"]));
        assert!(rust["launch"].get("program").is_none());
        let mut file = json!({"adapter":["x"],"launch":{}});
        assert_eq!(add_configuration(&mut file, rust.clone()).unwrap(), "Debug app");
        assert_eq!(add_configuration(&mut file, rust).unwrap(), "Debug app 2");
        assert_eq!(file["configurations"][0], json!({"name":"default","adapter":["x"],"launch":{}}));
        assert_eq!(file["configurations"].as_array().unwrap().len(), 3);
        let mut empty = json!({});
        add_configuration(&mut empty, template("go", None).unwrap()).unwrap();
        assert_eq!(empty["configurations"].as_array().unwrap().len(), 1);
        assert!(add_configuration(&mut json!([]), json!({})).is_err());
        assert_eq!(cargo_package("[workspace]\nname = \"no\"\n[package]\nname = \"app\"\nversion = \"0.1.0\"\n").as_deref(), Some("app"));
        assert_eq!(cargo_package("[workspace]\nmembers = []\n"), None);
    }
}
