//! Explicit argv launch entries reuse the task runner's cancellation and output.
use super::*;
use serde_json::Value;
fn quote(s: &str) -> String { terminal::quote(Path::new(s)) }
pub(super) fn compile(root: &Path, value: &Value, tasks: &[(String, String)]) -> Result<(String, String), String> {
    let name = value["name"].as_str().filter(|s| !s.is_empty()).ok_or("Run entry needs a name")?;
    let command = value["command"].as_str().filter(|s| !s.is_empty()).ok_or("Run entry needs a command")?;
    let mut argv = vec![quote(command)];
    if let Some(args) = value.get("args") {
        for arg in args.as_array().ok_or("args must be a string array")? {
            argv.push(quote(arg.as_str().ok_or("args must contain strings")?));
        }
    }
    let cwd = root.join(value.get("cwd").map(|v| v.as_str().ok_or("cwd must be a string")).transpose()?.unwrap_or("."));
    if !cwd.is_dir() { return Err(format!("Run cwd does not exist: {}", cwd.display())); }
    let mut env = Vec::new();
    if let Some(vars) = value.get("env") {
        for (k, v) in vars.as_object().ok_or("env must be an object")? {
            if k.is_empty() || k.contains(['=', '\0']) { return Err("Invalid environment name".into()); }
            env.push(quote(&format!("{k}={}", v.as_str().ok_or("env values must be strings")?)));
        }
    }
    let pre = if let Some(pre) = value.get("preLaunchTask") {
        let label = pre.as_str().ok_or("preLaunchTask must name a task")?;
        let command = tasks.iter().find(|t| t.0 == label).map(|t| &t.1).ok_or_else(|| format!("Unknown pre-launch task: {label}"))?;
        format!("({command}) && ")
    } else { String::new() };
    Ok((name.into(), format!("{pre}cd -- {} && exec env -- {} {}", super::task_config::cwd_arg(root, &cwd), env.join(" "), argv.join(" "))))
}
impl App {
    pub(super) fn run_configs(&mut self, default: bool) {
        let result = (|| -> Result<(Vec<(String, String)>, Option<String>), String> {
            let text = std::fs::read_to_string(self.root.join(".lightdown/run.json")).map_err(|e| format!(".lightdown/run.json: {e}"))?;
            let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            super::task_config::load(&self.root).map_err(|e| format!("tasks.json: {e}"))?;
            let tasks = self.project_tasks();
            let entries = value["configurations"].as_array().ok_or("Expected configurations array")?;
            if entries.len() > 128 { return Err("At most 128 run configurations".into()); }
            let mut out = Vec::new();
            for entry in entries {
                let item = if entry.get("compound").is_some() {
                    let name = entry["name"].as_str().filter(|n| !n.is_empty()).ok_or("Compound needs a name")?;
                    (name.to_string(), String::new())
                } else { compile(&self.root, entry, &tasks)? };
                if out.iter().any(|(name, _)| *name == item.0) { return Err("Duplicate run configuration name".into()); }
                out.push(item);
            }
            let mut compounds = std::collections::BTreeMap::new();
            for entry in entries.iter().filter(|e| e.get("compound").is_some()) {
                let names = entry["compound"].as_array().filter(|a| !a.is_empty() && a.len() <= 16).ok_or("compound must contain 1 to 16 configuration names")?;
                let mut members = Vec::new();
                for name in names {
                    let name = name.as_str().ok_or("compound members must be names")?;
                    let member = out.iter().find(|(n, command)| n == name && !command.is_empty()).ok_or_else(|| format!("Unknown or nested compound member: {name}"))?;
                    if members.contains(member) { return Err(format!("Duplicate compound member: {name}")); }
                    members.push(member.clone());
                }
                compounds.insert(entry["name"].as_str().unwrap().to_string(), members);
            }
            self.set_run_compounds(compounds);
            Ok((out, value["default"].as_str().map(str::to_string)))
        })();
        match result {
            Ok((entries, chosen)) => self.offer_run_configs(entries, if default { Some(chosen.unwrap_or_default()) } else { None }),
            Err(e) => self.message = Some(format!("Run: {e}")),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn argv_environment_cwd_and_prelaunch_are_literal_and_ordered() {
        let root = std::env::temp_dir().join(format!("ld-run-config-{}", std::process::id()));
        std::fs::create_dir_all(root.join("child")).unwrap();
        let value = json!({"name":"demo", "command":"python3", "args":["-c", "import os,sys; print(sys.argv[1], os.environ['TEST_VALUE'], os.getcwd())", "$(echo injection)"], "env":{"TEST_VALUE":"a b ' c"}, "cwd":"child", "preLaunchTask":"prepare"});
        let (_, cmd) = compile(&root, &value, &[("prepare".into(), "printf ready > marker".into())]).unwrap();
        let out = crate::tools::Job::start(crate::tools::shell(&root, &cmd, 5), Arc::new(|| {})).wait().unwrap();
        assert!(out.1 && out.0.contains("$(echo injection) a b ' c") && out.0.contains("/child"), "{out:?}");
        assert!(root.join("marker").exists());
        let (_, cmd) = compile(&root, &value, &[("prepare".into(), "false".into())]).unwrap();
        assert!(!crate::tools::Job::start(crate::tools::shell(&root, &cmd, 5), Arc::new(|| {})).wait().unwrap().1);
        assert!(compile(&root, &value, &[]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
