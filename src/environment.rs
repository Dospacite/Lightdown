//! Project interpreters and literal dotenv values. Values are never evaluated by a shell;
//! environments from direnv, mise and Nix come from `env_loaders`, for projects that chose them.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn dotenv(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(root.join(".env")) else { return out };
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if key.is_empty()
            || !key
                .bytes()
                .enumerate()
                .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit())
        {
            continue;
        }
        let value = value.trim();
        let value = if value.starts_with(['\'', '"']) {
            let quote = value.as_bytes()[0] as char;
            let Some(end) = value[1..].find(quote).map(|i| i + 1) else { continue };
            &value[1..end]
        } else {
            value.split_once(" #").map_or(value, |(v, _)| v).trim_end()
        };
        out.insert(key.into(), value.into());
    }
    out
}

pub fn apply(cmd: &mut Command, root: &Path, python: Option<&str>) {
    // direnv, mise or Nix, where the user chose them for this project.
    let loaded = crate::env_loaders::vars(root);
    for (key, value) in &loaded {
        match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }
    // Explicit session values win over the project's defaults; `.env`
    // wins over what was loaded.
    for (key, value) in dotenv(root) {
        if std::env::var_os(&key).is_none() || loaded.iter().any(|(k, _)| *k == key) {
            cmd.env(key, value);
        }
    }
    if let Some(bin) = python.map(Path::new).filter(|p| p.is_absolute()).and_then(Path::parent) {
        let mut paths = vec![bin.to_path_buf()];
        let configured = cmd.get_envs().find(|(k, _)| *k == "PATH").and_then(|(_, v)| v.map(|s| s.to_os_string()));
        paths.extend(std::env::split_paths(&configured.or_else(|| std::env::var_os("PATH")).unwrap_or_default()));
        if let Ok(path) = std::env::join_paths(paths) {
            cmd.env("PATH", path);
        }
        if let Some(venv) = bin.parent().filter(|p| p.join("pyvenv.cfg").exists()) {
            cmd.env("VIRTUAL_ENV", venv);
        }
    }
}

pub fn interpreters(root: &Path) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), "Automatic".into()), ("python3".into(), "System Python".into())];
    let mut add = |path: PathBuf, label: String| {
        if path.is_file() {
            let path = path.to_string_lossy().into_owned();
            if !out.iter().any(|(p, _)| *p == path) {
                out.push((path, label));
            }
        }
    };
    for name in [".venv", "venv"] {
        add(root.join(name).join("bin/python"), format!("Project {name}"));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    // Poetry names its cached environments after the project. uv normally uses .venv.
    let project = root.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    for entry in std::fs::read_dir(home.join(".cache/pypoetry/virtualenvs")).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().to_lowercase().starts_with(&format!("{project}-")) {
            add(entry.path().join("bin/python"), format!("Poetry: {}", entry.file_name().to_string_lossy()));
        }
    }
    for prefix in [
        std::env::var_os("CONDA_PREFIX").map(PathBuf::from),
        Some(home.join("miniconda3")),
        Some(home.join("anaconda3")),
    ]
    .into_iter()
    .flatten()
    {
        add(prefix.join("bin/python"), format!("Conda: {}", prefix.display()));
    }
    if let Ok(envs) = std::fs::read_to_string(home.join(".conda/environments.txt")) {
        for prefix in envs.lines().filter(|s| !s.is_empty()) {
            add(Path::new(prefix).join("bin/python"), format!("Conda: {prefix}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dotenv_is_literal_and_environment_reaches_commands() {
        let dir = std::env::temp_dir().join(format!("ld-env-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".venv/bin")).unwrap();
        std::fs::write(dir.join(".venv/bin/python"), "").unwrap();
        std::fs::write(
            dir.join(".env"),
            "export LD_FIXTURE='hello # world'\nLD_LITERAL=$(whoami) # comment\n9INVALID=no\n",
        )
        .unwrap();
        let env = dotenv(&dir);
        assert_eq!(env["LD_LITERAL"], "$(whoami)");
        assert!(!env.contains_key("9INVALID"));
        let mut cmd = Command::new("printenv");
        apply(&mut cmd, &dir, Some(dir.join(".venv/bin/python").to_str().unwrap()));
        let out = String::from_utf8(cmd.output().unwrap().stdout).unwrap();
        assert!(out.contains("LD_FIXTURE=hello # world"));
        assert!(out.contains(&format!("PATH={}/.venv/bin:", dir.display())));
        assert!(interpreters(&dir).iter().any(|(_, label)| label == "Project .venv"));
        // A loaded environment lies under `.env`, and may unset values.
        crate::env_loaders::set_loaded(
            &dir,
            vec![
                ("LD_LOADED".into(), Some("from direnv".into())),
                ("LD_FIXTURE".into(), Some("loaded".into())),
                ("HOME".into(), None),
            ],
        );
        let mut cmd = Command::new("printenv");
        apply(&mut cmd, &dir, None);
        let out = String::from_utf8(cmd.output().unwrap().stdout).unwrap();
        assert!(out.contains("LD_LOADED=from direnv") && out.contains("LD_FIXTURE=hello # world"), "{out}");
        assert!(!out.lines().any(|l| l.starts_with("HOME=")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
