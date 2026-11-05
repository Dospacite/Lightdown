//! Reusable per-user server registry. Installation never touches system directories.
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug)]
pub enum Install {
    Github(&'static str),
    Npm(&'static [&'static str]),
    Pip(&'static str),
    Go(&'static str),
    Jdtls,
    External,
}
#[derive(Clone, Copy, Debug)]
pub struct Server {
    pub name: &'static str,
    pub languages: &'static [&'static str],
    pub binary: &'static str,
    pub args: &'static [&'static str],
    pub install: Install,
}
macro_rules! server {
    ($name:literal, [$($lang:literal),+], $bin:literal, [$($arg:literal),*], $install:expr) => {
        Server { name:$name, languages:&[$($lang),+], binary:$bin, args:&[$($arg),*], install:$install }
    };
}
pub const SERVERS: &[Server] = &[
    server!("rust-analyzer", ["rust"], "rust-analyzer", [], Install::Github("rust-lang/rust-analyzer")),
    server!(
        "typescript-language-server",
        ["typescript", "typescriptreact", "javascript", "javascriptreact"],
        "typescript-language-server",
        ["--stdio"],
        Install::Npm(&["typescript-language-server", "typescript"])
    ),
    server!("gopls", ["go"], "gopls", [], Install::Go("golang.org/x/tools/gopls")),
    server!("clangd", ["c", "cpp"], "clangd", [], Install::Github("clangd/clangd")),
    server!("basedpyright", ["python"], "basedpyright-langserver", ["--stdio"], Install::Pip("basedpyright")),
    server!("pyright", ["python"], "pyright-langserver", ["--stdio"], Install::Npm(&["pyright"])),
    server!("ruff", ["python"], "ruff", ["server"], Install::Pip("ruff")),
    server!("lua-language-server", ["lua"], "lua-language-server", [], Install::Github("LuaLS/lua-language-server")),
    server!("zls", ["zig"], "zls", [], Install::Github("zigtools/zls")),
    server!(
        "bash-language-server",
        ["shellscript"],
        "bash-language-server",
        ["start"],
        Install::Npm(&["bash-language-server"])
    ),
    server!(
        "yaml-language-server",
        ["yaml"],
        "yaml-language-server",
        ["--stdio"],
        Install::Npm(&["yaml-language-server"])
    ),
    server!(
        "vscode-json-language-server",
        ["json", "jsonc"],
        "vscode-json-language-server",
        ["--stdio"],
        Install::Npm(&["vscode-langservers-extracted"])
    ),
    server!(
        "vscode-css-language-server",
        ["css", "scss", "less"],
        "vscode-css-language-server",
        ["--stdio"],
        Install::Npm(&["vscode-langservers-extracted"])
    ),
    server!(
        "vscode-html-language-server",
        ["html"],
        "vscode-html-language-server",
        ["--stdio"],
        Install::Npm(&["vscode-langservers-extracted"])
    ),
    server!("marksman", ["markdown"], "marksman", ["server"], Install::Github("artempyanykh/marksman")),
    server!("taplo", ["toml"], "taplo", ["lsp", "stdio"], Install::Github("tamasfe/taplo")),
    server!("texlab", ["latex", "bibtex"], "texlab", [], Install::Github("latex-lsp/texlab")),
    server!(
        "dockerfile-language-server",
        ["dockerfile"],
        "docker-langserver",
        ["--stdio"],
        Install::Npm(&["dockerfile-language-server-nodejs"])
    ),
    server!("intelephense", ["php"], "intelephense", ["--stdio"], Install::Npm(&["intelephense"])),
    server!("ruby-lsp", ["ruby"], "ruby-lsp", [], Install::External),
    server!("jdtls", ["java"], "jdtls", [], Install::Jdtls),
    server!(
        "kotlin-language-server",
        ["kotlin"],
        "kotlin-language-server",
        [],
        Install::Github("fwcd/kotlin-language-server")
    ),
    server!("sourcekit-lsp", ["swift"], "sourcekit-lsp", [], Install::External),
    server!("haskell-language-server", ["haskell"], "haskell-language-server-wrapper", ["--lsp"], Install::External),
];
pub fn language(path: &Path) -> &str {
    if path.file_name().is_some_and(|n| n == "Dockerfile" || n.to_string_lossy().starts_with("Dockerfile.")) {
        return "dockerfile";
    }
    match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "py" | "pyi" => "python",
        "sh" | "bash" | "zsh" => "shellscript",
        "yml" | "yaml" => "yaml",
        "md" | "markdown" => "markdown",
        "tex" | "sty" | "cls" => "latex",
        "bib" => "bibtex",
        "rb" => "ruby",
        "kt" | "kts" => "kotlin",
        "hs" | "lhs" => "haskell",
        "htm" => "html",
        ext => ext,
    }
}
pub fn for_language(id: &str) -> impl Iterator<Item = &'static Server> {
    SERVERS.iter().filter(move |s| s.languages.contains(&id))
}
pub fn named(name: &str) -> Option<&'static Server> {
    SERVERS.iter().find(|s| s.name == name)
}
pub fn directory(s: &Server) -> PathBuf {
    crate::config::data_dir().join("servers").join(s.name)
}
fn runnable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
pub fn local(root: &Path, binary: &str) -> Option<PathBuf> {
    [root.join(".venv/bin"), root.join("venv/bin"), root.join("node_modules/.bin")]
        .into_iter()
        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))
        .map(|p| p.join(binary))
        .find(|p| runnable(p))
}
pub fn installed(s: &Server) -> Option<PathBuf> {
    let dir = directory(s);
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.join("version.json")).ok()?).ok()?;
    let path = dir.join(manifest["executable"].as_str()?);
    runnable(&path).then_some(path)
}
pub fn command(root: &Path, s: &Server) -> Option<Vec<String>> {
    let p = local(root, s.binary).or_else(|| installed(s))?;
    Some(std::iter::once(p.to_string_lossy().into_owned()).chain(s.args.iter().map(|s| s.to_string())).collect())
}
fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{}: {e}", cmd.get_program().to_string_lossy()))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).chars().take(4000).collect());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().into())
}
fn fetch(url: &str, target: Option<&Path>) -> Result<String, String> {
    let mut c = Command::new("curl");
    c.args([
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "20",
        "--max-time",
        "300",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "-A",
        "Lightdown",
    ]);
    if let Some(p) = target {
        c.arg("--output").arg(p);
    }
    run(c.arg(url))
}
/// Deterministic Linux asset selection; never accept source archives or another architecture.
pub fn asset<'a>(s: &Server, assets: &'a [Value], arch: &str) -> Option<&'a Value> {
    let aliases: &[&str] = match arch {
        "x86_64" => &["x86_64", "x64", "amd64"],
        "aarch64" => &["aarch64", "arm64"],
        _ => return None,
    };
    assets
        .iter()
        .filter(|a| {
            let n = a["name"].as_str().unwrap_or("").to_ascii_lowercase();
            let linux = n.contains("linux") || (s.name == "kotlin-language-server" && n == "server.zip");
            let archive = n.ends_with(".gz")
                || n.ends_with(".zip")
                || n.ends_with(".tar.xz")
                || (s.name == "marksman" && !n.contains('.'));
            linux
                && archive
                && (aliases.iter().any(|a| n.contains(a))
                    || (arch == "x86_64"
                        && ((n.starts_with("clangd-linux-") && !n.contains("arm64") && !n.contains("aarch64"))
                            || n == "marksman-linux"
                            || n == "server.zip")))
                && !n.contains("musl")
                && !n.contains("sha256")
                && !n.contains("checksum")
                && !n.ends_with(".sig")
        })
        .min_by_key(|a| a["name"].as_str().unwrap_or("").to_string())
}
fn release(s: &Server, repo: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(&fetch(&format!("https://api.github.com/repos/{repo}/releases/latest"), None)?)
        .map_err(|e| e.to_string())?;
    if asset(s, v["assets"].as_array().ok_or("Release has no assets")?, std::env::consts::ARCH).is_none() {
        return Err(format!("No Linux {} release asset for {}", std::env::consts::ARCH, s.name));
    }
    Ok(v)
}
fn find_binary(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if e.file_type().ok()?.is_dir() {
            if let Some(p) = find_binary(&p, name) {
                return Some(p);
            }
        } else if e.file_name() == name {
            return Some(p);
        }
    }
    None
}
fn unpack(file: &Path, name: &str, dir: &Path, binary: &str) -> Result<(), String> {
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(file).map_err(|e| e.to_string())?))
            .unpack(dir)
            .map_err(|e| e.to_string())?;
    } else if name.ends_with(".tar.xz") {
        run(Command::new("tar").arg("-xJf").arg(file).arg("-C").arg(dir))?;
    } else if name.ends_with(".zip") {
        run(Command::new("unzip").arg("-q").arg(file).arg("-d").arg(dir))?;
    } else if name.ends_with(".gz") {
        let mut input = flate2::read::GzDecoder::new(std::fs::File::open(file).map_err(|e| e.to_string())?);
        std::io::copy(&mut input, &mut std::fs::File::create(dir.join(binary)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    } else {
        std::fs::copy(file, dir.join(binary)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// Runs on a worker. Staging plus rename preserves a working installation on failure.
pub fn install(s: &Server, progress: impl Fn(&str)) -> Result<String, String> {
    let dest = directory(s);
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let stage = dest.with_file_name(format!(".{}-{}", s.name, std::process::id()));
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        progress("Checking version");
        let version = match s.install {
            Install::Github(repo) => {
                let v = release(s, repo)?;
                let a = asset(s, v["assets"].as_array().unwrap(), std::env::consts::ARCH).unwrap();
                progress("Downloading release");
                let file = stage.join("download");
                fetch(a["browser_download_url"].as_str().ok_or("Missing asset URL")?, Some(&file))?;
                progress("Unpacking release");
                unpack(&file, a["name"].as_str().unwrap(), &stage, s.binary)?;
                std::fs::remove_file(file).map_err(|e| e.to_string())?;
                v["tag_name"].as_str().unwrap_or("unknown").to_string()
            }
            Install::Npm(packages) => {
                progress("Installing npm packages");
                run(Command::new("npm")
                    .args(["install", "--no-audit", "--no-fund", "--prefix"])
                    .arg(&stage)
                    .args(packages))?;
                let v: Value = serde_json::from_slice(
                    &std::fs::read(stage.join("node_modules").join(packages[0]).join("package.json"))
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                v["version"].as_str().unwrap_or("unknown").into()
            }
            Install::Pip(package) => {
                progress("Creating Python environment");
                run(Command::new("python3").args(["-m", "venv"]).arg(stage.join("venv")))?;
                progress("Installing pip package");
                run(Command::new(stage.join("venv/bin/python")).args(["-m", "pip", "install", "--upgrade", package]))?;
                run(Command::new(stage.join("venv/bin/python")).args([
                    "-c",
                    &format!("import importlib.metadata;print(importlib.metadata.version('{package}'))"),
                ]))?
            }
            Install::Go(module) => {
                progress("Building Go language server");
                std::fs::create_dir_all(stage.join("bin")).map_err(|e| e.to_string())?;
                run(Command::new("go").env("GOBIN", stage.join("bin")).args(["install", &format!("{module}@latest")]))?;
                run(Command::new(stage.join("bin").join(s.binary)).arg("version"))?
            }
            Install::Jdtls => {
                let base = "https://download.eclipse.org/jdtls/snapshots";
                let name = fetch(&format!("{base}/latest.txt"), None)?;
                if !name.starts_with("jdt-language-server-") || name.contains('/') || !name.ends_with(".tar.gz") {
                    return Err("Invalid JDT LS release name".into());
                }
                progress("Downloading JDT LS, requires Java 21+");
                let file = stage.join("download");
                fetch(&format!("{base}/{name}"), Some(&file))?;
                unpack(&file, &name, &stage, s.binary)?;
                std::fs::remove_file(file).map_err(|e| e.to_string())?;
                name
            }
            Install::External => {
                return Err(format!("Install {} with your language toolchain, then restart the server", s.name));
            }
        };
        let exe = match s.install {
            Install::Npm(_) => stage.join("node_modules/.bin").join(s.binary),
            Install::Pip(_) => stage.join("venv/bin").join(s.binary),
            _ => find_binary(&stage, s.binary).ok_or("Release contains no server executable")?,
        };
        let relative = exe.strip_prefix(&stage).map_err(|e| e.to_string())?.to_path_buf();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        // venv launchers embed their absolute path. Rewrite it before moving staging.
        if matches!(s.install, Install::Pip(_)) {
            for e in std::fs::read_dir(stage.join("venv/bin")).map_err(|e| e.to_string())?.flatten() {
                if let Ok(text) = std::fs::read_to_string(e.path()) {
                    if text.starts_with("#!") {
                        std::fs::write(
                            e.path(),
                            text.replace(&stage.to_string_lossy().to_string(), &dest.to_string_lossy()),
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        std::fs::write(stage.join("version.json"), json!({"version":version,"executable":relative}).to_string())
            .map_err(|e| e.to_string())?;
        let backup = dest.with_extension("previous");
        if backup.exists() {
            std::fs::remove_dir_all(&backup).map_err(|e| e.to_string())?;
        }
        if dest.exists() {
            std::fs::rename(&dest, &backup).map_err(|e| e.to_string())?;
        }
        if let Err(e) = std::fs::rename(&stage, &dest) {
            let _ = std::fs::rename(&backup, &dest);
            return Err(e.to_string());
        }
        let _ = std::fs::remove_dir_all(backup);
        Ok(version)
    })();
    if stage.exists() {
        let _ = std::fs::remove_dir_all(stage);
    }
    result
}
pub fn latest(s: &Server) -> Result<String, String> {
    match s.install {
        Install::Github(repo) => Ok(release(s, repo)?["tag_name"].as_str().unwrap_or("unknown").into()),
        Install::Npm(p) => run(Command::new("npm").args(["view", p[0], "version"])),
        Install::Pip(p) => {
            let v: Value = serde_json::from_str(&fetch(&format!("https://pypi.org/pypi/{p}/json"), None)?)
                .map_err(|e| e.to_string())?;
            Ok(v["info"]["version"].as_str().unwrap_or("unknown").into())
        }
        Install::Go(module) => {
            let v: Value = serde_json::from_str(&run(Command::new("go").args([
                "list",
                "-m",
                "-json",
                &format!("{module}@latest"),
            ]))?)
            .map_err(|e| e.to_string())?;
            Ok(v["Version"].as_str().unwrap_or("unknown").into())
        }
        Install::Jdtls => fetch("https://download.eclipse.org/jdtls/snapshots/latest.txt", None),
        Install::External => Err("Managed by the language toolchain".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_covers_languages_and_selects_linux_assets() {
        for ext in [
            "rs",
            "ts",
            "tsx",
            "js",
            "go",
            "c",
            "py",
            "lua",
            "zig",
            "sh",
            "yml",
            "json",
            "css",
            "html",
            "md",
            "toml",
            "tex",
            "Dockerfile",
            "php",
            "rb",
            "java",
            "kt",
            "swift",
            "hs",
        ] {
            assert!(
                for_language(language(Path::new(ext))).next().is_some()
                    || for_language(language(&PathBuf::from(format!("a.{ext}")))).next().is_some(),
                "{ext}"
            );
        }
        let a = json!([{"name":"rust-analyzer-linux-x64.vsix"},{"name":"rust-analyzer-aarch64-unknown-linux-gnu.gz"},{"name":"rust-analyzer-x86_64-apple-darwin.gz"},{"name":"rust-analyzer-x86_64-unknown-linux-gnu.gz"},{"name":"rust-analyzer-x86_64-unknown-linux-musl.gz"}]);
        assert_eq!(
            asset(named("rust-analyzer").unwrap(), a.as_array().unwrap(), "x86_64").unwrap()["name"],
            "rust-analyzer-x86_64-unknown-linux-gnu.gz"
        );
        assert!(asset(named("rust-analyzer").unwrap(), a.as_array().unwrap(), "riscv64").is_none());
        let clang = json!([{"name":"clangd-linux-arm64-20.zip"},{"name":"clangd-linux-20.zip"},{"name":"clangd-windows-20.zip"}]);
        assert_eq!(
            asset(named("clangd").unwrap(), clang.as_array().unwrap(), "x86_64").unwrap()["name"],
            "clangd-linux-20.zip"
        );
        let names: std::collections::HashSet<_> = SERVERS.iter().map(|s| s.name).collect();
        assert_eq!(names.len(), SERVERS.len());
    }
    #[test]
    #[ignore = "downloads real servers into scratch XDG_DATA_HOME"]
    fn live_install_servers() {
        assert!(std::env::var("XDG_DATA_HOME").unwrap().contains("scratch"));
        for name in ["rust-analyzer", "marksman", "bash-language-server", "ruff"] {
            let s = named(name).unwrap();
            let v = install(s, |p| eprintln!("{name}: {p}")).unwrap();
            assert!(!v.is_empty());
            assert!(installed(s).is_some());
            let out = Command::new(installed(s).unwrap()).arg("--version").output().unwrap();
            assert!(out.status.success(), "{name}: {:?}", out);
        }
    }
}
