//! Dev containers as an execution target. Reading a config never runs
//! anything; containers start only from an explicit command. docker or
//! podman run as processes.
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// The documented subset of `devcontainer.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// The file it came from.
    pub source: PathBuf,
    pub image: Option<String>,
    /// A Dockerfile and build context, as host paths inside the project.
    pub build: Option<(PathBuf, PathBuf)>,
    pub workspace: PathBuf,
    pub env: Vec<(String, String)>,
    pub forward: Vec<u16>,
    /// An argv; a string command becomes `sh -c`.
    pub post_create: Option<Vec<String>>,
    pub host_network: bool,
    pub user: Option<String>,
    /// Keys read but not supported, for the log.
    pub ignored: Vec<String>,
}

/// Where a running container is reached.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub engine: String,
    pub name: String,
    pub user: Option<String>,
}

/// The config file for `root`: `.lightdown/container.json` first, then
/// `.devcontainer/devcontainer.json`.
pub fn find(root: &Path) -> Option<PathBuf> {
    [root.join(".lightdown/container.json"), root.join(".devcontainer/devcontainer.json")].into_iter().find(|p| p.is_file())
}

/// JSON with comments and trailing commas, as devcontainer.json allows, made plain.
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    let mut string = false;
    while let Some((i, c)) = chars.next() {
        if string {
            out.push(c);
            if c == '\\' {
                if let Some((_, n)) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                string = false;
            }
            continue;
        }
        match c {
            '"' => {
                string = true;
                out.push(c);
            }
            '/' if chars.peek().is_some_and(|p| p.1 == '/') => {
                while chars.peek().is_some_and(|p| p.1 != '\n') {
                    chars.next();
                }
            }
            '/' if chars.peek().is_some_and(|p| p.1 == '*') => {
                chars.next();
                let mut last = ' ';
                for (_, c) in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                out.push(' ');
            }
            // Drop a comma that only whitespace or comments separate from a closing bracket.
            ',' if strip_jsonc_lead(&text[i + 1..]).starts_with(['}', ']']) => {}
            _ => out.push(c),
        }
    }
    out
}

fn strip_jsonc_lead(mut rest: &str) -> &str {
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("//") {
            rest = r.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.split_once("*/").map_or("", |(_, r)| r);
        } else {
            return rest;
        }
    }
}

fn inside(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    if !out.starts_with(root) {
        return Err(format!("{} is outside the project", path.display()));
    }
    Ok(out)
}

fn word(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Reads the config for `root`.
pub fn read(root: &Path) -> Result<Config, String> {
    let path = find(root).ok_or("Add .devcontainer/devcontainer.json or .lightdown/container.json")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() > 256_000 {
        return Err("Container config exceeds 256 KB".into());
    }
    let value: Value = serde_json::from_str(&strip_jsonc(&text)).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(root, &path, &value)
}

pub fn parse(root: &Path, source: &Path, v: &Value) -> Result<Config, String> {
    let object = v.as_object().ok_or("Expected a JSON object")?;
    let dir = source.parent().unwrap_or(root);
    let string = |key: &str| -> Result<Option<String>, String> {
        v.get(key).map(|s| s.as_str().map(str::to_string).ok_or(format!("{key} must be a string"))).transpose()
    };
    let mut cfg = Config { source: source.to_path_buf(), ..Default::default() };
    cfg.image = string("image")?;
    if let Some(image) = &cfg.image
        && !word(image)
    {
        return Err("image must be an image name".into());
    }
    if let Some(build) = v.get("build") {
        let file = build["dockerfile"].as_str().ok_or("build needs a dockerfile")?;
        let context = build.get("context").map(|c| c.as_str().ok_or("build.context must be a string")).transpose()?.unwrap_or(".");
        if let Some(key) = build.as_object().ok_or("build must be an object")?.keys().find(|k| !["dockerfile", "context"].contains(&k.as_str())) {
            cfg.ignored.push(format!("build.{key}"));
        }
        cfg.build = Some((inside(root, &dir.join(file))?, inside(root, &dir.join(context))?));
    }
    if cfg.image.is_some() == cfg.build.is_some() {
        return Err("Set exactly one of image or build.dockerfile".into());
    }
    let name = root.file_name().map_or("project".into(), |n| n.to_string_lossy().into_owned());
    let workspace = string("workspaceFolder")?.unwrap_or(format!("/workspaces/{name}"));
    let workspace = PathBuf::from(workspace.replace("${localWorkspaceFolderBasename}", &name));
    if !workspace.is_absolute() || workspace.components().any(|c| c == Component::ParentDir) || workspace.to_string_lossy().contains([':', '\n', ',']) {
        return Err("workspaceFolder must be an absolute container path".into());
    }
    cfg.workspace = workspace;
    if let Some(env) = v.get("containerEnv") {
        for (key, value) in env.as_object().ok_or("containerEnv must be an object")? {
            if key.is_empty() || !key.bytes().enumerate().all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit()) {
                return Err(format!("Invalid containerEnv name: {key}"));
            }
            cfg.env.push((key.clone(), value.as_str().ok_or("containerEnv values must be strings")?.to_string()));
        }
    }
    if let Some(ports) = v.get("forwardPorts") {
        for port in ports.as_array().filter(|a| a.len() <= 32).ok_or("forwardPorts must be an array of up to 32 ports")? {
            let port = port.as_u64().filter(|p| (1..=65535).contains(p)).ok_or("forwardPorts must contain port numbers")?;
            cfg.forward.push(port as u16);
        }
    }
    cfg.post_create = match v.get("postCreateCommand") {
        None => None,
        Some(Value::String(s)) => Some(vec!["sh".into(), "-c".into(), s.clone()]),
        Some(Value::Array(a)) if !a.is_empty() => {
            Some(a.iter().map(|s| s.as_str().map(str::to_string).ok_or("postCreateCommand must hold strings")).collect::<Result<_, _>>()?)
        }
        Some(_) => return Err("postCreateCommand must be a string or a string array".into()),
    };
    cfg.host_network = v.get("hostNetwork").map(|b| b.as_bool().ok_or("hostNetwork must be true or false")).transpose()?.unwrap_or(false);
    if let Some(args) = v.get("runArgs") {
        let args: Vec<&str> = args.as_array().ok_or("runArgs must be an array")?.iter().map(|a| a.as_str().unwrap_or("")).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i] {
                "--network=host" | "--net=host" => cfg.host_network = true,
                "--network" | "--net" if args.get(i + 1) == Some(&"host") => {
                    cfg.host_network = true;
                    i += 1;
                }
                other => cfg.ignored.push(format!("runArgs {other}")),
            }
            i += 1;
        }
    }
    cfg.user = string("remoteUser")?;
    if let Some(user) = &cfg.user
        && !word(user)
    {
        return Err("remoteUser must be a user name".into());
    }
    let known = ["name", "image", "build", "workspaceFolder", "containerEnv", "forwardPorts", "postCreateCommand", "runArgs", "hostNetwork", "remoteUser", "$schema"];
    cfg.ignored.extend(object.keys().filter(|k| !known.contains(&k.as_str())).cloned());
    Ok(cfg)
}

/// docker if installed, else podman.
pub fn engine() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    ["docker", "podman"].into_iter().find(|e| std::env::split_paths(&path).any(|d| d.join(e).is_file())).map(str::to_string)
}

fn hash(text: &str) -> u64 {
    // FNV-1a: stable across runs, unlike the standard hasher.
    text.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// The container's name for this project.
pub fn name(root: &Path) -> String {
    let base: String = root.file_name().map_or("project".into(), |n| n.to_string_lossy().to_lowercase()).chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect();
    format!("lightdown-{base}-{:08x}", hash(&root.to_string_lossy()) as u32)
}

impl Config {
    /// Changes here recreate the container.
    pub fn fingerprint(&self, root: &Path) -> String {
        format!("{:016x}", hash(&format!("{root:?}{self:?}")))
    }
    pub fn tag(&self, root: &Path) -> String {
        format!("{}-image", name(root))
    }
}

fn cmd(engine: &str, args: &[&str]) -> Command {
    let mut c = Command::new(engine);
    c.args(args);
    c
}

/// `true running`, `false stopped`, or `None` without a container, plus its fingerprint label.
pub fn inspect(engine: &str, name: &str) -> Command {
    cmd(engine, &["container", "inspect", "--format", "{{.State.Running}} {{index .Config.Labels \"lightdown.config\"}}", "--", name])
}

pub fn image_exists(engine: &str, image: &str) -> Command {
    cmd(engine, &["image", "inspect", "--format", "{{.Id}}", "--", image])
}

pub fn pull(engine: &str, image: &str) -> Command {
    cmd(engine, &["pull", "--", image])
}

pub fn build(engine: &str, cfg: &Config, root: &Path) -> Command {
    let (file, context) = cfg.build.clone().unwrap_or_default();
    let mut c = cmd(engine, &["build", "--tag", &cfg.tag(root), "--file"]);
    c.arg(file).arg("--").arg(context);
    c
}

/// Creates the container, idle until commands are run in it.
pub fn run(engine: &str, cfg: &Config, root: &Path, image: &str) -> Command {
    let name = name(root);
    let mut c = cmd(engine, &["run", "--detach", "--init", "--name", &name, "--label"]);
    c.arg(format!("lightdown.project={}", root.display()));
    c.arg("--label").arg(format!("lightdown.config={}", cfg.fingerprint(root)));
    if cfg.host_network {
        c.args(["--network", "host"]);
    } else {
        for port in &cfg.forward {
            c.arg("--publish").arg(format!("127.0.0.1:{port}:{port}"));
        }
    }
    c.arg("--volume").arg(format!("{}:{}", root.display(), cfg.workspace.display()));
    c.arg("--workdir").arg(&cfg.workspace);
    for (k, v) in &cfg.env {
        c.arg("--env").arg(format!("{k}={v}"));
    }
    c.args(["--entrypoint", "sh", "--", image, "-c", "trap 'exit 0' TERM INT; while sleep 3600; do :; done"]);
    c
}

pub fn start(engine: &str, name: &str) -> Command {
    cmd(engine, &["start", "--", name])
}

pub fn stop(engine: &str, name: &str) -> Command {
    cmd(engine, &["stop", "--time", "2", "--", name])
}

pub fn remove(engine: &str, name: &str) -> Command {
    cmd(engine, &["rm", "--force", "--", name])
}

/// Each command run in the container carries a tag, so that what a killed
/// client leaves behind can be found and ended.
pub fn new_tag() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let tag = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let mut issued = issued().lock().unwrap();
    if issued.len() >= 4096 {
        issued.remove(0);
    }
    issued.push(tag.clone());
    tag
}

fn issued() -> &'static std::sync::Mutex<Vec<String>> {
    static ISSUED: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    ISSUED.get_or_init(Default::default)
}

/// Tags handed out since the last call.
pub fn take_issued() -> Vec<String> {
    std::mem::take(&mut *issued().lock().unwrap())
}

impl Target {
    /// `command` through `sh -c` in `dir` inside the container; `-t` for terminals.
    pub fn exec(&self, dir: &Path, command: &str, terminal: bool) -> Command {
        let mut c = cmd(&self.engine, &["exec", "--interactive"]);
        if terminal {
            c.arg("--tty");
        }
        if let Some(user) = &self.user {
            c.args(["--user", user]);
        }
        c.arg("--env").arg(format!("LIGHTDOWN_EXEC={}", new_tag()));
        c.arg("--workdir").arg(dir).args(["--", &self.name, "sh", "-c", command]);
        c
    }
    /// Ends processes left by clients whose tags are in `gone`.
    pub fn sweep(&self, gone: &BTreeSet<String>) -> Command {
        let list: Vec<&str> = gone.iter().map(String::as_str).collect();
        let script = format!(
            "gone=' {} '; for p in /proc/[0-9]*; do t=$(tr '\\0' '\\n' < $p/environ 2>/dev/null | sed -n 's/^LIGHTDOWN_EXEC=//p'); [ -n \"$t\" ] && case \"$gone\" in *\" $t \"*) kill -9 ${{p#/proc/}} 2>/dev/null;; esac; done; true",
            list.join(" ")
        );
        cmd(&self.engine, &["exec", "--user", "0", "--", &self.name, "sh", "-c", &script])
    }
}

/// Tags of container commands whose clients still run on this machine.
pub fn live_tags() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return out };
    for entry in dir.flatten() {
        if !entry.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(line) = std::fs::read(entry.path().join("cmdline")) else { continue };
        for arg in line.split(|&b| b == 0) {
            if let Some(tag) = arg.strip_prefix(b"LIGHTDOWN_EXEC=") {
                out.insert(String::from_utf8_lossy(tag).into_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn args(c: &Command) -> Vec<String> {
        c.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }
    #[test]
    fn comments_and_trailing_commas_are_allowed() {
        let text = "{\n // a comment\n \"image\": \"alpine\", /* block */\n \"forwardPorts\": [3000, 5173,],\n \"url\": \"http://x//y\",\n}";
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(v["forwardPorts"], json!([3000, 5173]));
        assert_eq!(v["url"], "http://x//y");
    }
    #[test]
    fn the_supported_subset_is_validated() {
        let root = Path::new("/home/me/My App");
        let source = root.join(".devcontainer/devcontainer.json");
        let v = json!({"name":"x","build":{"dockerfile":"Dockerfile","context":".."},"containerEnv":{"MODE":"dev"},
            "forwardPorts":[8000],"postCreateCommand":"pip install -e .","runArgs":["--network=host","--cap-add=SYS_PTRACE"],
            "customizations":{}});
        let cfg = parse(root, &source, &v).unwrap();
        assert_eq!(cfg.build, Some((root.join(".devcontainer/Dockerfile"), root.to_path_buf())));
        assert_eq!(cfg.workspace, Path::new("/workspaces/My App"));
        assert!(cfg.host_network);
        assert_eq!(cfg.post_create.as_deref().unwrap()[2], "pip install -e .");
        assert_eq!(cfg.ignored, ["runArgs --cap-add=SYS_PTRACE", "customizations"]);
        for bad in [
            json!({}),
            json!({"image":"a","build":{"dockerfile":"D"}}),
            json!({"image":"--privileged"}),
            json!({"build":{"dockerfile":"../../../etc/D"}}),
            json!({"image":"a","workspaceFolder":"relative"}),
            json!({"image":"a","forwardPorts":[70000]}),
            json!({"image":"a","containerEnv":{"A B":"x"}}),
            json!({"image":"a","postCreateCommand":{"a":"b"}}),
        ] {
            assert!(parse(root, &source, &bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn commands_mount_the_project_and_tag_execs() {
        let root = Path::new("/p/app");
        let mut cfg = parse(root, &root.join(".lightdown/container.json"), &json!({"image":"python:3-slim","forwardPorts":[8000],"containerEnv":{"A":"b c"}})).unwrap();
        let run = args(&run("docker", &cfg, root, "python:3-slim"));
        let name = name(root);
        assert!(name.starts_with("lightdown-app-"));
        assert!(run.windows(2).any(|w| w == ["--volume", "/p/app:/workspaces/app"]));
        assert!(run.windows(2).any(|w| w == ["--publish", "127.0.0.1:8000:8000"]));
        assert!(run.windows(2).any(|w| w == ["--env", "A=b c"]));
        cfg.host_network = true;
        let run = args(&super::run("podman", &cfg, root, "python:3-slim"));
        assert!(run.windows(2).any(|w| w == ["--network", "host"]) && !run.contains(&"--publish".to_string()));
        let target = Target { engine: "docker".into(), name: name.clone(), user: Some("dev".into()) };
        let exec = args(&target.exec(Path::new("/workspaces/app/src"), "ls; pwd", true));
        assert!(exec.contains(&"--tty".into()) && exec.iter().any(|a| a.starts_with("LIGHTDOWN_EXEC=")));
        assert_eq!(&exec[exec.len() - 4..], [name.as_str(), "sh", "-c", "ls; pwd"]);
        assert!(exec.windows(2).any(|w| w == ["--workdir", "/workspaces/app/src"]));
        assert!(exec.windows(2).any(|w| w == ["--user", "dev"]));
    }
    #[test]
    fn live_tags_come_from_client_command_lines() {
        let tag = new_tag();
        let mut child = Command::new("sh").args(["-c", "sleep 5; :"]).arg(format!("LIGHTDOWN_EXEC={tag}")).spawn().unwrap();
        let start = std::time::Instant::now();
        while !live_tags().contains(&tag) && start.elapsed().as_secs() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let seen = live_tags().contains(&tag);
        let _ = child.kill();
        let _ = child.wait();
        assert!(seen);
    }
}
