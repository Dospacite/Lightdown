//! Bounded project task definitions. Loading configuration never executes it.
use std::path::Path;
use serde_json::Value;
use super::terminal;
fn quote(s: &str) -> String { terminal::quote(Path::new(s)) }
/// A working folder as commands see it: relative to the project when inside
/// it, so the same command works over SSH and in a container.
pub(super) fn cwd_arg(root: &Path, cwd: &Path) -> String {
    match cwd.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
        Ok(rel) => quote(&format!("./{}", rel.to_string_lossy())),
        Err(_) => quote(&cwd.to_string_lossy()),
    }
}
/// One task from `.lightdown/tasks.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TaskDef {
    pub name: String,
    /// Its own command, after the dependencies that are not background
    /// tasks, in order. Each step must succeed before the next starts.
    pub command: String,
    /// Background tasks, here or among its dependencies, that must be
    /// ready before `command` starts.
    pub needs: Vec<String>,
    /// Project-relative globs; a matching save reruns the task.
    pub watch: Vec<String>,
    /// For a background task, the pattern that says it has started.
    pub ready: Option<String>,
}
/// Names and complete commands, for callers that run a task as one process.
/// Background dependencies are left out: those must already be running.
pub(super) fn load(root: &Path) -> Result<Vec<(String, String)>, String> {
    Ok(load_defs(root)?.into_iter().map(|t| (t.name, t.command)).collect())
}
pub(super) fn load_defs(root: &Path) -> Result<Vec<TaskDef>, String> {
    let path = root.join(".lightdown/tasks.json");
    if !path.exists() { return Ok(Vec::new()); }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.len() > 256_000 { return Err("tasks.json exceeds 256 KB".into()); }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let entries = value["tasks"].as_array().ok_or("Expected tasks array")?;
    if entries.len() > 128 { return Err("At most 128 tasks".into()); }
    let mut names = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().filter(|s| !s.is_empty()).ok_or("Task needs a name")?;
        if names.contains(&name) { return Err(format!("Duplicate task: {name}")); }
        names.push(name);
    }
    fn background(entry: &Value) -> Result<Option<String>, String> {
        let Some(value) = entry.get("background") else { return Ok(None) };
        let object = value.as_object().ok_or("background must be an object with readyPattern")?;
        if let Some(key) = object.keys().find(|k| *k != "readyPattern") { return Err(format!("Unknown background key: {key}")); }
        let pattern = object.get("readyPattern").and_then(Value::as_str).filter(|p| !p.is_empty()).ok_or("background needs a readyPattern string")?;
        ready_pattern(pattern).map_err(|e| format!("readyPattern: {e}"))?;
        Ok(Some(pattern.to_string()))
    }
    fn watch(entry: &Value) -> Result<Vec<String>, String> {
        let Some(value) = entry.get("watch") else { return Ok(Vec::new()) };
        let globs = value.as_array().filter(|a| !a.is_empty() && a.len() <= 32).ok_or("watch must be an array of 1 to 32 globs")?;
        globs.iter().map(|g| {
            let g = g.as_str().filter(|g| !g.is_empty() && g.len() <= 256).ok_or("watch must contain glob strings")?;
            if g.starts_with('/') || g.split('/').any(|part| part == "..") { return Err(format!("watch glob must stay in the project: {g}")); }
            Ok(g.to_string())
        }).collect()
    }
    fn compile(root: &Path, entries: &[Value], names: &[&str], index: usize, stack: &mut Vec<usize>) -> Result<(String, Vec<String>), String> {
        if stack.contains(&index) { return Err(format!("Task dependency cycle at {}", names[index])); }
        if stack.len() >= 32 { return Err("Task dependency depth exceeds 32".into()); }
        stack.push(index);
        let entry = &entries[index];
        let mut commands = Vec::new();
        let mut needs: Vec<String> = Vec::new();
        if let Some(deps) = entry.get("dependsOn") {
            for dep in deps.as_array().ok_or("dependsOn must be an array")? {
                let dep = dep.as_str().ok_or("dependsOn must contain names")?;
                let i = names.iter().position(|n| *n == dep).ok_or_else(|| format!("Unknown dependency: {dep}"))?;
                let (command, more) = compile(root, entries, names, i, stack)?;
                for need in more { if !needs.contains(&need) { needs.push(need); } }
                if background(&entries[i])?.is_some() {
                    if !needs.iter().any(|n| n == dep) { needs.push(dep.to_string()); }
                } else {
                    commands.push(format!("({command})"));
                }
                if commands.iter().map(String::len).sum::<usize>() > 256_000 { return Err("Expanded task exceeds 256 KB".into()); }
            }
        }
        let command = match (entry.get("argv"), entry.get("shell")) {
            (Some(argv), None) => {
                let argv = argv.as_array().filter(|a| !a.is_empty()).ok_or("argv must be a nonempty string array")?;
                let args = argv.iter().map(|a| a.as_str().map(quote).ok_or("argv must contain strings")).collect::<Result<Vec<_>, _>>()?;
                args.join(" ")
            }
            (None, Some(shell)) => format!("sh -c {}", quote(shell.as_str().ok_or("shell must be a string")?)),
            _ => return Err("Task needs exactly one of argv or shell".into()),
        };
        let cwd = root.join(entry.get("cwd").map(|v| v.as_str().ok_or("cwd must be a string")).transpose()?.unwrap_or("."));
        if !cwd.is_dir() { return Err(format!("Task cwd does not exist: {}", cwd.display())); }
        let mut env = Vec::new();
        if let Some(vars) = entry.get("env") {
            for (key, value) in vars.as_object().ok_or("env must be an object")? {
                if key.is_empty() || !key.bytes().enumerate().all(|(i,c)| c == b'_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit()) { return Err("Invalid env name".into()); }
                env.push(quote(&format!("{key}={}", value.as_str().ok_or("env values must be strings")?)));
            }
        }
        if let Some(pattern) = entry.get("problemMatcher") {
            matcher(pattern.as_str().ok_or("problemMatcher must be a regex string")?)?;
        }
        if entry.get("readyPattern").is_some() { return Err("readyPattern belongs inside background".into()); }
        background(entry)?;
        watch(entry)?;
        commands.push(format!("cd -- {} && env -- {} sh -c {}", cwd_arg(root, &cwd), env.join(" "), quote(&command)));
        stack.pop();
        Ok((commands.join(" && "), needs))
    }
    names.iter().enumerate().map(|(i, name)| {
        let (command, needs) = compile(root, entries, &names, i, &mut Vec::new()).map_err(|e| format!("{name}: {e}"))?;
        Ok(TaskDef { name: name.to_string(), command, needs, watch: watch(&entries[i])?, ready: background(&entries[i])? })
    }).collect()
}
/// A background task's ready pattern, with the same bounds as matchers.
pub(super) fn ready_pattern(pattern: &str) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(pattern).size_limit(1_000_000).build().map_err(|e| e.to_string())
}
fn matcher(pattern: &str) -> Result<regex::Regex, String> {
    let regex = regex::RegexBuilder::new(pattern).size_limit(1_000_000).build().map_err(|e| e.to_string())?;
    for name in ["file", "line"] {
        if !regex.capture_names().flatten().any(|n| n == name) { return Err(format!("problemMatcher needs named group {name}")); }
    }
    Ok(regex)
}
pub(super) fn locations(root: &Path, name: &str, output: &str) -> Vec<(String, usize, usize, String)> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(root.join(".lightdown/tasks.json")) else { return out };
    if text.len() > 256_000 { return out; }
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return out };
    let Some(entry) = value["tasks"].as_array().and_then(|a| a.iter().find(|e| e["name"].as_str() == Some(name))) else { return out };
    let Some(pattern) = entry["problemMatcher"].as_str() else { return out };
    let Ok(regex) = matcher(pattern) else { return out };
    for line in output.lines().take(20_000) {
        let Some(c) = regex.captures(line) else { continue };
        let Some(number) = c.name("line").and_then(|m| m.as_str().parse::<usize>().ok()).filter(|n| *n > 0) else { continue };
        let column = c.name("column").and_then(|m| m.as_str().parse::<usize>().ok()).unwrap_or(1).max(1);
        let Some(file) = c.name("file") else { continue };
        let cwd = root.join(entry["cwd"].as_str().unwrap_or("."));
        let file = cwd.join(file.as_str()).to_string_lossy().into_owned();
        let message = c.name("message").map_or(line, |m| m.as_str());
        let severity = c.name("severity").map_or("", |m| m.as_str());
        out.push((file, number, column, format!("{severity} {message}").trim().to_string()));
        if out.len() >= 512 { break; }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matcher_requires_location_groups_and_bounds_compilation() {
        assert!(matcher(r"(?P<file>.+)@(?P<line>\d+): (?P<message>.+)").is_ok());
        assert!(matcher("missing groups").is_err());
        assert!(matcher("[").is_err());
    }
    #[test]
    fn dependencies_are_ordered_and_argv_stays_literal() {
        let root = std::env::temp_dir().join(format!("ld-task-config-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let path = root.join(".lightdown/tasks.json");
        std::fs::write(&path, r#"{"tasks":[{"name":"prepare","shell":"printf ready > marker"},{"name":"run","argv":["printf","%s","$(touch unexpected)"],"dependsOn":["prepare"]}]}"#).unwrap();
        let tasks = load(&root).unwrap();
        let out = crate::tools::Job::start(crate::tools::shell(&root, &tasks[1].1, 5), std::sync::Arc::new(|| {})).wait().unwrap();
        assert!(out.1 && out.0.contains("$(touch unexpected)"));
        assert!(root.join("marker").exists());
        assert!(!root.join("unexpected").exists());
        std::fs::write(&path, r#"{"tasks":[{"name":"cycle","argv":["true"],"dependsOn":["cycle"]}]}"#).unwrap();
        assert!(load(&root).unwrap_err().contains("cycle"));
        for (bad, why) in [
            (r#"{"name":"bad","argv":["true"],"background":true}"#, "background must be an object"),
            (r#"{"name":"bad","argv":["true"],"background":{}}"#, "readyPattern"),
            (r#"{"name":"bad","argv":["true"],"background":{"readyPattern":"("}}"#, "readyPattern:"),
            (r#"{"name":"bad","argv":["true"],"background":{"readyPattern":"x","other":1}}"#, "Unknown background key"),
            (r#"{"name":"bad","argv":["true"],"readyPattern":"x"}"#, "inside background"),
            (r#"{"name":"bad","argv":["true"],"watch":[]}"#, "watch must be"),
            (r#"{"name":"bad","argv":["true"],"watch":["../x"]}"#, "stay in the project"),
            (r#"{"name":"bad","argv":["true"],"watch":"src"}"#, "watch must be"),
        ] {
            std::fs::write(&path, format!(r#"{{"tasks":[{bad}]}}"#)).unwrap();
            let e = load(&root).unwrap_err();
            assert!(e.contains(why) && e.starts_with("bad: "), "{e}");
        }
        std::fs::write(&path, r#"{"tasks":[
            {"name":"db","argv":["true"],"background":{"readyPattern":"ready"}},
            {"name":"migrate","argv":["true"],"dependsOn":["db"]},
            {"name":"serve","argv":["true"],"dependsOn":["migrate"],"watch":["src/**/*.rs"],"background":{"readyPattern":"listening"}},
            {"name":"e2e","argv":["true"],"dependsOn":["serve","migrate"]}]}"#).unwrap();
        let defs = load_defs(&root).unwrap();
        assert_eq!(defs[0].ready.as_deref(), Some("ready"));
        assert_eq!(defs[1].needs, ["db"]);
        assert_eq!(defs[2].needs, ["db"]);
        assert_eq!(defs[2].watch, ["src/**/*.rs"]);
        assert_eq!(defs[3].needs, ["db", "serve"]);
        assert_eq!(defs[3].command.matches("cd -- ").count(), 2);
        std::fs::write(&path, r#"{"tasks":[{"name":"match","argv":["true"],"problemMatcher":"(?P<file>.+)@(?P<line>[0-9]+): (?P<severity>error|warning): (?P<message>.+)"}]}"#).unwrap();
        load(&root).unwrap();
        let matches = locations(&root, "match", "a.py@12: warning: fixture\na.py@0: error: invalid");
        assert_eq!(matches.len(), 1);
        assert_eq!((matches[0].1, matches[0].2), (12, 1));
        assert_eq!(matches[0].3, "warning fixture");
        std::fs::remove_dir_all(root).unwrap();
    }
}
