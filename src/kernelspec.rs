//! Jupyter kernelspecs: finding the kernels installed on the machine and
//! in the project, and choosing one for a notebook or a script.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// An installed Jupyter kernel, as its `kernel.json` describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// The directory's name, which notebooks refer to it by.
    pub name: String,
    pub display_name: String,
    /// The language as the spec states it, lower case: `python`, `r`, `julia`, `typescript`.
    pub language: String,
    pub argv: Vec<String>,
    /// The directory holding `kernel.json`; none for the interpreter's own ipykernel.
    pub dir: Option<PathBuf>,
    /// The whole `kernel.json`, for the bridge.
    pub json: Value,
}

impl Spec {
    /// What the status bar and lists call it.
    pub fn label(&self) -> String {
        if self.display_name.is_empty() { self.name.clone() } else { self.display_name.clone() }
    }

    /// The `kernelspec` a notebook saved with this kernel records.
    pub fn metadata(&self) -> Value {
        json!({"display_name": self.label(), "language": self.language, "name": self.name})
    }
}

/// The directory a Python interpreter lives under: `<prefix>/bin/python`.
/// A bare name is looked up on the `PATH`. Links are not followed, so a
/// virtual environment's prefix is the environment.
pub fn prefix(python: &str) -> Option<PathBuf> {
    let path = if python.contains('/') {
        PathBuf::from(python)
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(python)).find(|p| p.is_file())?
    };
    Some(path.parent()?.parent()?.to_path_buf())
}

/// The `site-packages` directories of an interpreter's prefix, and the
/// user's own for a system interpreter.
fn site_packages(prefix: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut lib_dirs = vec![prefix.join("lib")];
    if prefix == Path::new("/usr") || prefix == Path::new("/usr/local") {
        if let Some(home) = std::env::var_os("HOME") {
            lib_dirs.push(PathBuf::from(home).join(".local/lib"));
        }
    }
    for lib in lib_dirs {
        let Ok(entries) = std::fs::read_dir(&lib) else { continue };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("python3") {
                out.push(e.path().join("site-packages"));
                out.push(e.path().join("dist-packages"));
            }
        }
    }
    out
}

/// Whether an interpreter has a package installed, judged by its files so
/// that nothing has to be started.
pub fn has_package(python: &str, package: &str) -> bool {
    let Some(prefix) = prefix(python) else { return false };
    site_packages(&prefix).iter().any(|s| s.join(package).join("__init__.py").is_file())
}

/// Where kernelspecs are looked for, in order of precedence: `$JUPYTER_PATH`,
/// the user's data directory, the project's environments, the interpreter's
/// prefix and the system's directories.
pub fn search_dirs(python: &str, root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(paths) = std::env::var_os("JUPYTER_PATH") {
        dirs.extend(std::env::split_paths(&paths).filter(|p| !p.as_os_str().is_empty()).map(|p| p.join("kernels")));
    }
    if let Some(data) = std::env::var_os("JUPYTER_DATA_DIR") {
        dirs.push(PathBuf::from(data).join("kernels"));
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    if let Some(data) = data_home {
        dirs.push(data.join("jupyter/kernels"));
    }
    for env in [".venv", "venv"] {
        dirs.push(root.join(env).join("share/jupyter/kernels"));
    }
    if let Some(prefix) = prefix(python) {
        dirs.push(prefix.join("share/jupyter/kernels"));
    }
    dirs.push("/usr/local/share/jupyter/kernels".into());
    dirs.push("/usr/share/jupyter/kernels".into());
    let mut seen = Vec::new();
    dirs.retain(|d| {
        let new = !seen.contains(d);
        seen.push(d.clone());
        new
    });
    dirs
}

/// Reads one kernelspec directory.
pub fn read(dir: &Path) -> Option<Spec> {
    let text = std::fs::read_to_string(dir.join("kernel.json")).ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    let argv: Vec<String> = json["argv"].as_array()?.iter().filter_map(|a| a.as_str().map(str::to_string)).collect();
    if argv.is_empty() {
        return None;
    }
    Some(Spec {
        name: dir.file_name()?.to_string_lossy().to_lowercase(),
        display_name: json["display_name"].as_str().unwrap_or("").to_string(),
        language: json["language"].as_str().unwrap_or("").to_lowercase(),
        argv,
        dir: Some(dir.to_path_buf()),
        json,
    })
}

/// Every kernelspec in `dirs`; a name found earlier hides the same name later.
pub fn scan(dirs: &[PathBuf]) -> Vec<Spec> {
    let mut specs: Vec<Spec> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<Spec> = entries.flatten().filter_map(|e| read(&e.path())).collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        for spec in found {
            if !specs.iter().any(|s| s.name == spec.name) {
                specs.push(spec);
            }
        }
    }
    specs
}

/// The interpreter's own ipykernel, as `jupyter_client` would offer it
/// without a `kernel.json`.
pub fn ipykernel(python: &str) -> Spec {
    let argv =
        vec![python.to_string(), "-m".into(), "ipykernel_launcher".into(), "-f".into(), "{connection_file}".into()];
    let json = json!({"argv": argv, "display_name": "Python 3 (ipykernel)", "language": "python",
        "metadata": {"debugger": true}});
    Spec {
        name: "python3".into(),
        display_name: "Python 3 (ipykernel)".into(),
        language: "python".into(),
        argv,
        dir: None,
        json,
    }
}

/// The kernels to choose from for a project: the interpreter's ipykernel
/// first when it has one, then every kernelspec found.
pub fn discover(python: &str, root: &Path) -> Vec<Spec> {
    let mut specs = scan(&search_dirs(python, root));
    if has_package(python, "ipykernel") {
        specs.retain(|s| s.name != "python3");
        specs.insert(0, ipykernel(python));
    }
    specs
}

/// The language a notebook is written in, from its metadata.
pub fn notebook_language(metadata: &Value) -> String {
    let lang = metadata["kernelspec"]["language"].as_str().or_else(|| metadata["language_info"]["name"].as_str());
    lang.unwrap_or("python").to_lowercase()
}

/// What runs a notebook's cells, or a script's.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Lightdown's own Python helper.
    Builtin,
    Jupyter(Spec),
}

impl Choice {
    pub fn label(&self) -> String {
        match self {
            Choice::Builtin => "Built-in Python".into(),
            Choice::Jupyter(spec) => spec.label(),
        }
    }
    pub fn language(&self) -> String {
        match self {
            Choice::Builtin => "python".into(),
            Choice::Jupyter(spec) => spec.language.clone(),
        }
    }
}

/// The kernel a notebook gets by default. Python notebooks run on the
/// interpreter's ipykernel when it has one, so the project's packages are
/// there; on a kernel named in the metadata when it is another; else on the
/// built-in helper. Other languages need a kernel for them.
pub fn choose(metadata: &Value, specs: &[Spec], python_has_ipykernel: bool) -> Result<Choice, String> {
    if metadata["lightdown"]["kernel"] == "builtin" {
        return Ok(Choice::Builtin);
    }
    let language = notebook_language(metadata);
    let named = metadata["kernelspec"]["name"].as_str().map(str::to_lowercase);
    let named = named.and_then(|n| specs.iter().find(|s| s.name == n));
    if language == "python" {
        if let Some(spec) = named.filter(|s| s.name != "python3" && s.language == "python") {
            return Ok(Choice::Jupyter(spec.clone()));
        }
        if python_has_ipykernel {
            if let Some(spec) = specs.iter().find(|s| s.name == "python3" && s.dir.is_none()) {
                return Ok(Choice::Jupyter(spec.clone()));
            }
        }
        return Ok(Choice::Builtin);
    }
    if let Some(spec) = named.filter(|s| s.language == language) {
        return Ok(Choice::Jupyter(spec.clone()));
    }
    match specs.iter().find(|s| s.language == language || (language == "javascript" && s.language == "typescript")) {
        Some(spec) => Ok(Choice::Jupyter(spec.clone())),
        None => Err(format!("No Jupyter kernel for {language} is installed. Notebook: Select kernel lists what is.")),
    }
}

/// The language of a source file whose `%%` cells can run on a kernel, and
/// the comment its cell markers start with.
pub fn script_language(path: &Path) -> Option<(&'static str, &'static str)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "py" => ("python", "#"),
        "r" => ("r", "#"),
        "jl" => ("julia", "#"),
        "ts" | "mts" => ("typescript", "//"),
        "js" | "mjs" => ("javascript", "//"),
        _ => return None,
    })
}

/// The kernel for a script's language: the first installed one for it.
/// Python scripts follow the notebook rule.
pub fn for_language(language: &str, specs: &[Spec], python_has_ipykernel: bool) -> Result<Choice, String> {
    let meta = json!({"kernelspec": {"language": language}});
    choose(&meta, specs, python_has_ipykernel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_dir(root: &Path, name: &str, language: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let json = json!({"argv": ["x", "-f", "{connection_file}"], "display_name": name.to_uppercase(), "language": language});
        std::fs::write(dir.join("kernel.json"), json.to_string()).unwrap();
    }

    #[test]
    fn specs_are_found_and_earlier_dirs_win() {
        let tmp = std::env::temp_dir().join(format!("ld-kernelspecs-{}", std::process::id()));
        let (a, b) = (tmp.join("a/kernels"), tmp.join("b/kernels"));
        spec_dir(&a, "ir", "R");
        spec_dir(&b, "ir", "python");
        spec_dir(&b, "julia-1.10", "julia");
        std::fs::create_dir_all(b.join("broken")).unwrap();
        std::fs::write(b.join("broken/kernel.json"), "{").unwrap();
        let specs = scan(&[a.clone(), b.clone()]);
        assert_eq!(specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["ir", "julia-1.10"]);
        assert_eq!(specs[0].language, "r");
        assert_eq!(specs[0].metadata()["display_name"], "IR");
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn search_dirs_include_the_project_and_the_interpreter() {
        let dirs = search_dirs("/opt/env/bin/python", Path::new("/p"));
        assert!(dirs.contains(&PathBuf::from("/p/.venv/share/jupyter/kernels")));
        assert!(dirs.contains(&PathBuf::from("/opt/env/share/jupyter/kernels")));
        assert_eq!(dirs.last().unwrap(), Path::new("/usr/share/jupyter/kernels"));
    }

    #[test]
    fn notebooks_choose_by_language_and_name() {
        let r = Spec {
            name: "ir".into(),
            display_name: "R".into(),
            language: "r".into(),
            argv: vec!["R".into()],
            dir: Some("/k/ir".into()),
            json: json!({}),
        };
        let other_py = Spec { name: "env2".into(), language: "python".into(), ..r.clone() };
        let own = ipykernel("/v/bin/python");
        let specs = vec![own.clone(), r.clone(), other_py.clone()];
        let py = json!({"kernelspec": {"name": "python3", "language": "python"}});
        assert_eq!(choose(&py, &specs, true), Ok(Choice::Jupyter(own)));
        assert_eq!(choose(&py, &specs[1..], false), Ok(Choice::Builtin));
        assert_eq!(choose(&json!({}), &specs[1..], false), Ok(Choice::Builtin));
        let env2 = json!({"kernelspec": {"name": "env2", "language": "python"}});
        assert_eq!(choose(&env2, &specs, true), Ok(Choice::Jupyter(other_py)));
        let rnb = json!({"kernelspec": {"name": "something", "language": "R"}});
        assert_eq!(choose(&rnb, &specs, true), Ok(Choice::Jupyter(r)));
        assert!(choose(&json!({"language_info": {"name": "julia"}}), &specs, true).is_err());
        let builtin = json!({"kernelspec": {"name": "python3"}, "lightdown": {"kernel": "builtin"}});
        assert_eq!(choose(&builtin, &specs, true), Ok(Choice::Builtin));
    }

    #[test]
    fn scripts_name_their_language() {
        assert_eq!(script_language(Path::new("a.R")), Some(("r", "#")));
        assert_eq!(script_language(Path::new("a.ts")), Some(("typescript", "//")));
        assert_eq!(script_language(Path::new("a.md")), None);
    }
}
