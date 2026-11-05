//! Test runners: how each is found in a project, the commands that list and
//! run its tests, and the parsers that read what those commands print.
//!
//! Everything here is pure so that recorded output can test it. Commands
//! are shell text run by `testing`, which may write a structured report
//! to the file named by `$ld_report`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Status {
    Idle,
    Running,
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Cargo,
    Nextest,
    Pytest,
    Jest,
    Vitest,
    Go,
    Ctest,
}

/// One runner found in a project.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Runner {
    pub kind: Kind,
    /// How the runner is started: the interpreter for pytest, the package
    /// manager's launcher for Jest and Vitest, the build folder for CTest.
    pub launch: String,
}

/// A test as discovery reports it.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Found {
    pub id: String,
    pub group: String,
    pub path: Option<PathBuf>,
    /// The declaration's line, from 0, when known.
    pub line: Option<usize>,
    /// It stands for a file whose tests are known only once it runs.
    pub whole_file: bool,
}

/// A test's result as a run reports it.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct Outcome {
    pub id: String,
    pub status: Option<Status>,
    /// Seconds.
    pub duration: Option<f64>,
    pub message: Option<String>,
    /// Where it failed, with the line from 0.
    pub at: Option<(PathBuf, usize)>,
    /// What was expected and what was found, or Rust's left and right.
    pub values: Option<(String, String)>,
    /// Jest and Vitest only: the file it belongs to, and the line it is
    /// declared on, for tests first seen in a run.
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Cargo => "cargo test",
            Kind::Nextest => "cargo nextest",
            Kind::Pytest => "pytest",
            Kind::Jest => "Jest",
            Kind::Vitest => "Vitest",
            Kind::Go => "go test",
            Kind::Ctest => "CTest",
        }
    }
    /// Rust names its values left and right; others expected and actual.
    pub fn value_names(self) -> (&'static str, &'static str) {
        if matches!(self, Kind::Cargo | Kind::Nextest) { ("left", "right") } else { ("expected", "actual") }
    }
}

pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn package_launcher(root: &Path, tool: &str) -> String {
    if root.join("pnpm-lock.yaml").is_file() {
        format!("pnpm exec {tool}")
    } else if root.join("yarn.lock").is_file() {
        format!("yarn {tool}")
    } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        format!("bun x --no-install {tool}")
    } else {
        format!("npx --no-install {tool}")
    }
}

/// The folder CTest runs in: the project itself or a build folder in it.
fn ctest_dir(root: &Path) -> Option<String> {
    if root.join("CTestTestfile.cmake").is_file() {
        return Some(".".into());
    }
    let mut dirs: Vec<String> = ["build", "cmake-build-debug", "cmake-build-release", "out/build"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Ok(read) = std::fs::read_dir(root) {
        let mut more: Vec<String> = read
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("build") || n.starts_with("cmake-build"))
            .collect();
        more.sort();
        dirs.extend(more);
    }
    dirs.into_iter().find(|d| root.join(d).join("CTestTestfile.cmake").is_file())
}

fn pytest_project(root: &Path) -> bool {
    let has = |name: &str, needle: &str| {
        std::fs::read_to_string(root.join(name)).is_ok_and(|s| needle.is_empty() || s.contains(needle))
    };
    has("pytest.ini", "")
        || has("conftest.py", "")
        || has("pyproject.toml", "pytest")
        || has("setup.cfg", "[tool:pytest]")
        || has("tox.ini", "[pytest]")
        || root.join("tests").join("conftest.py").is_file()
        || std::fs::read_dir(root.join("tests")).into_iter().chain(std::fs::read_dir(root)).flatten().flatten().any(|e| {
            e.file_name().to_str().is_some_and(|n| n.starts_with("test_") && n.ends_with(".py"))
        })
}

/// The runners a project uses, in the order the panel lists them.
/// `installed` says whether a program is on the PATH.
pub(crate) fn detect(root: &Path, python: &str, installed: &dyn Fn(&str) -> bool) -> Vec<Runner> {
    let mut out = Vec::new();
    if root.join("Cargo.toml").is_file() {
        let kind = if installed("cargo-nextest") { Kind::Nextest } else { Kind::Cargo };
        out.push(Runner { kind, launch: String::new() });
    }
    if pytest_project(root) {
        out.push(Runner { kind: Kind::Pytest, launch: python.into() });
    }
    if let Some(pkg) = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    {
        let uses = |name: &str| {
            ["dependencies", "devDependencies"].iter().any(|k| pkg[k].get(name).is_some())
                || pkg.get(name).is_some()
                || pkg["scripts"]["test"].as_str().is_some_and(|s| s.split_whitespace().any(|w| w == name))
        };
        if uses("jest") {
            out.push(Runner { kind: Kind::Jest, launch: package_launcher(root, "jest") });
        }
        if uses("vitest") {
            out.push(Runner { kind: Kind::Vitest, launch: package_launcher(root, "vitest") });
        }
    }
    if root.join("go.mod").is_file() {
        out.push(Runner { kind: Kind::Go, launch: "go".into() });
    }
    if let Some(dir) = ctest_dir(root) {
        out.push(Runner { kind: Kind::Ctest, launch: dir });
    }
    out
}

/// Whether a program is on the PATH.
pub(crate) fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

// ---- commands ---------------------------------------------------------------

pub(crate) fn discover_command(r: &Runner) -> String {
    match r.kind {
        Kind::Cargo => "cargo test --color=never -- --list --format=terse".into(),
        Kind::Nextest => "cargo nextest list --color=never --message-format json > \"$ld_report\"".into(),
        Kind::Pytest => format!("{} -m pytest --collect-only -q --color=no", quote(&r.launch)),
        Kind::Jest => format!("{} --listTests --ci", r.launch),
        Kind::Vitest => format!("{} list --json > \"$ld_report\"", r.launch),
        Kind::Go => "go list -f '{{.ImportPath}}\t{{.Dir}}' ./... && echo '@@LD-GO-TESTS@@' && go test -list . ./...".into(),
        Kind::Ctest => format!("ctest --test-dir {} --show-only=json-v1 > \"$ld_report\"", quote(&r.launch)),
    }
}

/// Escapes a name for an anchored regular expression. CTest's dialect has
/// no non-capturing groups, so plain groups are used for all of them.
fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if "\\.+*?()|[]{}^$/".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn anchored(names: &[&str]) -> String {
    format!("^({})$", names.iter().map(|n| regex_escape(n)).collect::<Vec<_>>().join("|"))
}

/// The file and the full name of a Jest or Vitest test ID.
pub(crate) fn js_parts(id: &str) -> (&str, &str) {
    id.split_once("::").unwrap_or((id, ""))
}

/// The command that runs these tests, or all of the runner's tests when
/// `all` is set. `ids` are IDs from discovery.
pub(crate) fn run_command(r: &Runner, ids: &[String], all: bool) -> String {
    let quoted = |list: &[String]| list.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ");
    match r.kind {
        Kind::Cargo if all => "cargo test --no-fail-fast --color=never -- --color=never".into(),
        Kind::Cargo => format!("cargo test --no-fail-fast --color=never -- --exact --color=never {}", quoted(ids)),
        Kind::Nextest => {
            let filter = if all {
                String::new()
            } else {
                format!(" -E {}", quote(&ids.iter().map(|i| format!("test(={i})")).collect::<Vec<_>>().join(" | ")))
            };
            format!("cargo nextest run --no-fail-fast --color=never --hide-progress-bar --failure-output=final --success-output=never{filter}")
        }
        Kind::Pytest => format!(
            "{} -m pytest -v --tb=short --color=no -rfE --durations=0 --durations-min=0{}",
            quote(&r.launch),
            if all { String::new() } else { format!(" -- {}", quoted(ids)) }
        ),
        Kind::Jest | Kind::Vitest => {
            let mut files: Vec<&str> = Vec::new();
            let mut names: Vec<&str> = Vec::new();
            for id in ids {
                let (file, name) = js_parts(id);
                if !files.contains(&file) {
                    files.push(file);
                }
                // A whole file has no name, and runs all of its tests.
                if !name.is_empty() {
                    names.push(name);
                }
            }
            let whole = ids.iter().any(|i| js_parts(i).1.is_empty());
            let pattern = if all || whole || names.is_empty() {
                String::new()
            } else {
                format!(" -t {}", quote(&anchored(&names)))
            };
            let files = if all { String::new() } else { format!(" -- {}", files.iter().map(|f| quote(f)).collect::<Vec<_>>().join(" ")) };
            if r.kind == Kind::Jest {
                format!(
                    "{} --ci --json --outputFile=\"$ld_report\" --testLocationInResults{pattern}{}",
                    r.launch,
                    if all { String::new() } else { format!(" --runTestsByPath{files}") }
                )
            } else {
                format!("{} run --reporter=json --outputFile=\"$ld_report\"{pattern}{files}", r.launch)
            }
        }
        Kind::Go => {
            if all {
                return "go test -json ./...".into();
            }
            let mut by_package: Vec<(&str, Vec<&str>)> = Vec::new();
            for id in ids {
                let (package, name) = id.rsplit_once("::").unwrap_or(("./...", id));
                match by_package.iter_mut().find(|(p, _)| *p == package) {
                    Some((_, names)) => names.push(name),
                    None => by_package.push((package, vec![name])),
                }
            }
            // Each package gets its own run, and every one of them runs.
            let lines: Vec<String> = by_package
                .iter()
                .map(|(p, names)| format!("go test -json -run {} {} || ld_go_exit=1", quote(&anchored(names)), quote(p)))
                .collect();
            format!("ld_go_exit=0\n{}\n(exit $ld_go_exit)", lines.join("\n"))
        }
        Kind::Ctest => {
            let names: Vec<&str> = ids.iter().map(String::as_str).collect();
            format!(
                "ctest --test-dir {} --output-on-failure{}",
                quote(&r.launch),
                if all { String::new() } else { format!(" -R {}", quote(&anchored(&names))) }
            )
        }
    }
}

/// The command that runs a runner's tests under a coverage tool.
pub(crate) fn coverage_command(r: &Runner) -> Option<String> {
    match r.kind {
        Kind::Cargo => Some("cargo llvm-cov --no-fail-fast --lcov --output-path lcov.info".into()),
        Kind::Nextest => Some("cargo llvm-cov nextest --no-fail-fast --lcov --output-path lcov.info".into()),
        Kind::Pytest => {
            let python = quote(&r.launch);
            Some(format!(
                "{python} -m coverage run -m pytest -v --tb=short --color=no -rfE --durations=0 --durations-min=0\nld_coverage_exit=$?\n{python} -m coverage json -o coverage.json || exit 1\n(exit \"$ld_coverage_exit\")"
            ))
        }
        _ => None,
    }
}

/// The runner that can debug one of its tests, if any.
pub(crate) fn debuggable(kind: Kind) -> bool {
    kind == Kind::Pytest
}

// ---- discovery --------------------------------------------------------------

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

/// Reads what a runner's discovery printed. `log` is what the command
/// printed; `report` is the structured report it wrote, if any.
pub(crate) fn parse_discovery(r: &Runner, root: &Path, log: &str, report: &str) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let mut push = |f: Found| {
        if !out.iter().any(|o| o.id == f.id) {
            out.push(f);
        }
    };
    match r.kind {
        Kind::Cargo => {
            for id in log.lines().filter_map(|l| l.strip_suffix(": test")) {
                let group = id.rsplit_once("::").map_or("(crate root)", |(m, _)| m).to_string();
                push(Found { id: id.into(), group, path: None, line: None, whole_file: false });
            }
        }
        Kind::Nextest => {
            let v = json_in(report).unwrap_or_default();
            if let Some(suites) = v["rust-suites"].as_object() {
                for suite in suites.values() {
                    if let Some(cases) = suite["testcases"].as_object() {
                        for id in cases.keys() {
                            let group = id.rsplit_once("::").map_or("(crate root)", |(m, _)| m).to_string();
                            push(Found { id: id.clone(), group, path: None, line: None, whole_file: false });
                        }
                    }
                }
            }
        }
        Kind::Pytest => {
            for id in log.lines().map(str::trim).filter(|l| l.contains("::") && !l.contains(' ')) {
                let file = id.split("::").next().unwrap_or(id);
                let path = root.join(file);
                let line = std::fs::read_to_string(&path).ok().and_then(|s| source_line(&s, id));
                push(Found { id: id.into(), group: file.into(), path: path.is_file().then_some(path), line, whole_file: false });
            }
        }
        Kind::Jest => {
            for line in log.lines().map(str::trim).filter(|l| l.starts_with('/') || l.contains(".test.") || l.contains(".spec.")) {
                let path = root.join(line);
                if !path.is_file() {
                    continue;
                }
                let rel = relative(root, &path);
                push(Found { id: rel.clone(), group: rel, path: Some(path), line: None, whole_file: true });
            }
        }
        Kind::Vitest => {
            let v = json_in(report).unwrap_or_default();
            for t in v.as_array().into_iter().flatten() {
                let (Some(name), Some(file)) = (t["name"].as_str(), t["file"].as_str()) else { continue };
                let path = root.join(file);
                let rel = relative(root, &path);
                let full = name.replace(" > ", " ");
                let line = std::fs::read_to_string(&path).ok().and_then(|s| js_line(&s, name.rsplit(" > ").next().unwrap_or(name)));
                push(Found { id: format!("{rel}::{full}"), group: rel, path: Some(path), line, whole_file: false });
            }
        }
        Kind::Go => {
            let (dirs, list) = log.split_once("@@LD-GO-TESTS@@").unwrap_or(("", log));
            let dirs: HashMap<&str, &str> = dirs.lines().filter_map(|l| l.split_once('\t')).collect();
            let mut names: Vec<&str> = Vec::new();
            for line in list.lines() {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() == 1 && ["Test", "Example", "Fuzz"].iter().any(|p| fields[0].starts_with(p)) {
                    names.push(fields[0]);
                } else if fields.len() >= 2 && matches!(fields[0], "ok" | "?" | "FAIL") {
                    let package = fields[1];
                    let dir = dirs.get(package).map(PathBuf::from);
                    for name in names.drain(..) {
                        let (path, line) = dir.as_deref().and_then(|d| go_line(d, name)).map_or((None, None), |(p, l)| (Some(p), Some(l)));
                        push(Found { id: format!("{package}::{name}"), group: package.into(), path, line, whole_file: false });
                    }
                }
            }
        }
        Kind::Ctest => {
            let v = json_in(report).unwrap_or_default();
            let graph = &v["backtraceGraph"];
            for t in v["tests"].as_array().into_iter().flatten() {
                let Some(name) = t["name"].as_str() else { continue };
                let node = t["backtrace"].as_u64().map(|i| &graph["nodes"][i as usize]);
                let path = node
                    .and_then(|n| n["file"].as_u64())
                    .and_then(|i| graph["files"][i as usize].as_str())
                    .map(|f| root.join(r.launch.as_str()).join(f));
                let line = node.and_then(|n| n["line"].as_u64()).map(|l| (l as usize).saturating_sub(1));
                let group = path.as_ref().map_or_else(|| "CTest".into(), |p| relative(root, p));
                push(Found { id: name.into(), group, path, line, whole_file: false });
            }
        }
    }
    out
}

/// The JSON value in a report, which may follow other text.
fn json_in(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str(text.trim()).ok().or_else(|| {
        let start = text.find(['{', '['])?;
        serde_json::from_str(text[start..].trim()).ok()
    })
}

/// The line of a Rust or Python test's declaration.
pub(crate) fn source_line(text: &str, id: &str) -> Option<usize> {
    let name = id.rsplit("::").next()?.split('[').next()?;
    text.lines().position(|l| {
        let s = l.trim_start();
        let s = s.strip_prefix("pub ").unwrap_or(s);
        let s = s.strip_prefix("async ").unwrap_or(s);
        s.strip_prefix("def ").or_else(|| s.strip_prefix("fn ")).is_some_and(|r| r.strip_prefix(name).is_some_and(|s| s.starts_with('(') || s.starts_with('<')))
    })
}

/// The line of a JavaScript test called `title`.
pub(crate) fn js_line(text: &str, title: &str) -> Option<usize> {
    let quoted = ['\'', '"', '`'].map(|q| format!("{q}{title}{q}"));
    text.lines().position(|l| {
        let s = l.trim_start();
        ["it(", "test(", "it.", "test."].iter().any(|p| s.starts_with(p)) && quoted.iter().any(|q| s.contains(q.as_str()))
    })
}

/// The file and line of a Go test function in a package folder.
fn go_line(dir: &Path, name: &str) -> Option<(PathBuf, usize)> {
    let needle = format!("func {name}(");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with("_test.go")).collect();
    files.sort();
    files.into_iter().find_map(|p| {
        let text = std::fs::read_to_string(&p).ok()?;
        let line = text.lines().position(|l| l.starts_with(&needle))?;
        Some((p, line))
    })
}

/// Finds the files that declare Rust tests, reading each file once. A name
/// declared in several files goes to the one whose path best matches its
/// module path.
pub(crate) fn locate_rust(root: &Path, files: &[PathBuf], ids: &[String]) -> HashMap<String, (PathBuf, usize)> {
    let mut wanted: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in ids {
        wanted.entry(id.rsplit("::").next().unwrap_or(id)).or_default().push(id);
    }
    let mut seen: HashMap<&str, Vec<(PathBuf, usize)>> = HashMap::new();
    for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == "rs")) {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for (n, line) in text.lines().enumerate() {
            let s = line.trim_start();
            let s = s.strip_prefix("pub ").unwrap_or(s);
            let s = s.strip_prefix("async ").unwrap_or(s);
            let Some(rest) = s.strip_prefix("fn ") else { continue };
            let name: &str = &rest[..rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len())];
            if let Some((key, _)) = wanted.get_key_value(name) {
                seen.entry(key).or_default().push((path.clone(), n));
            }
        }
    }
    let mut out = HashMap::new();
    for (name, ids) in wanted {
        let Some(places) = seen.get(name) else { continue };
        for id in ids {
            let modules: Vec<&str> = id.split("::").collect();
            let score = |p: &Path| {
                let rel = relative(root, p);
                modules.iter().filter(|m| rel.split(['/', '.']).any(|part| part == **m)).count()
            };
            let best = places.iter().map(|(p, _)| score(p)).max().unwrap_or(0);
            let top: Vec<_> = places.iter().filter(|(p, _)| score(p) == best).collect();
            if top.len() == 1 {
                out.insert(id.to_string(), top[0].clone());
            }
        }
    }
    out
}

// ---- results ----------------------------------------------------------------

/// Reads a run's results. `ids` are the tests that ran, so a runner that
/// says nothing of one can still be read; tests it found that were not
/// asked for are returned too (Jest files that are run whole).
pub(crate) fn parse_results(r: &Runner, root: &Path, ids: &[String], log: &str, report: &str) -> Vec<Outcome> {
    match r.kind {
        Kind::Cargo => ids.iter().map(|id| cargo_outcome(root, id, log)).collect(),
        Kind::Nextest => nextest_results(root, ids, log),
        Kind::Pytest => pytest_results(root, ids, log),
        Kind::Jest | Kind::Vitest => js_results(root, report, log),
        Kind::Go => go_results(root, log),
        Kind::Ctest => ctest_results(log),
    }
}

/// A Rust test's panic: its message, place and assert_eq! values.
fn rust_panic(root: &Path, id: &str, text: &str) -> (Option<String>, Option<(PathBuf, usize)>, Option<(String, String)>) {
    // "thread 'name' panicked at", with the thread's number after the
    // name since Rust 1.88.
    let head = format!("thread '{id}'");
    let rest = text.match_indices(&head).find_map(|(at, _)| {
        let after = &text[at + head.len()..];
        let after = match after.strip_prefix(" (") {
            Some(n) => &n[n.find(')')? + 1..],
            None => after,
        };
        after.strip_prefix(" panicked at ")
    });
    let Some(rest) = rest else { return (None, None, None) };
    let (place, body) = if let Some(old) = rest.strip_prefix('\'') {
        // Before Rust 1.73: panicked at 'message', src/lib.rs:4:5
        let end = old.find("', ").unwrap_or(old.len());
        let after = old.get(end + 3..).unwrap_or("");
        (after.lines().next().unwrap_or(""), old[..end].to_string())
    } else {
        let (first, more) = rest.split_once('\n').unwrap_or((rest, ""));
        let body: Vec<&str> = more.lines().take_while(|l| !l.is_empty() && !l.starts_with("note:") && !l.starts_with("stack backtrace")).collect();
        (first.trim_end_matches(':'), body.join("\n"))
    };
    let mut parts = place.rsplitn(3, ':');
    let (_col, line, file) = (parts.next(), parts.next(), parts.next());
    let at = match (file, line.and_then(|l| l.trim().parse::<usize>().ok())) {
        (Some(f), Some(l)) => Some((root.join(f.trim()), l.saturating_sub(1))),
        _ => None,
    };
    let value = |key: &str| {
        body.lines().find_map(|l| l.trim_start().strip_prefix(key)).map(|v| {
            let v = v.trim();
            // Old panics put each value in backticks and end the first with a comma.
            let v = v.strip_suffix(',').unwrap_or(v);
            v.strip_prefix('`').and_then(|v| v.strip_suffix('`')).unwrap_or(v).to_string()
        })
    };
    let values = value("left:").zip(value("right:"));
    let message = body.lines().next().map(|l| l.trim().to_string()).filter(|m| !m.is_empty());
    (message, at, values)
}

fn cargo_outcome(root: &Path, id: &str, log: &str) -> Outcome {
    let line = log.lines().rev().find_map(|l| l.strip_prefix(&format!("test {id} ... ")));
    let status = line.map(|s| {
        if s.starts_with("ok") {
            Status::Passed
        } else if s.starts_with("ignored") {
            Status::Skipped
        } else {
            Status::Failed
        }
    });
    let mut o = Outcome { id: id.into(), status, ..Default::default() };
    if status == Some(Status::Failed) {
        (o.message, o.at, o.values) = rust_panic(root, id, log);
    }
    o
}

fn nextest_results(root: &Path, ids: &[String], log: &str) -> Vec<Outcome> {
    let mut by_id: HashMap<String, Outcome> = HashMap::new();
    for line in log.lines() {
        let s = line.trim_start();
        // "PASS [   0.004s] crate mod::test", or "TRY 2 FAIL [...] ..."
        let s = s.strip_prefix("TRY ").map(|t| t.split_once(' ').map_or(t, |(_, r)| r.trim_start())).unwrap_or(s);
        let Some((word, rest)) = s.split_once(' ') else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('[') else { continue };
        let Some((time, rest)) = rest.split_once(']') else { continue };
        let Ok(duration) = time.trim().trim_end_matches('s').parse::<f64>() else { continue };
        let Some(id) = rest.split_whitespace().nth(1) else { continue };
        let status = match word {
            "PASS" => Status::Passed,
            "SKIP" => Status::Skipped,
            w if w.chars().all(|c| c.is_ascii_uppercase() || c == '-') => Status::Failed,
            _ => continue,
        };
        by_id.insert(id.into(), Outcome { id: id.into(), status: Some(status), duration: Some(duration), ..Default::default() });
    }
    for o in by_id.values_mut().filter(|o| o.status == Some(Status::Failed)) {
        (o.message, o.at, o.values) = rust_panic(root, &o.id.clone(), log);
    }
    let mut out: Vec<Outcome> = ids.iter().map(|id| by_id.remove(id).unwrap_or(Outcome { id: id.clone(), ..Default::default() })).collect();
    out.extend(by_id.into_values());
    out
}

/// Splits `left == right` where `==` is outside quotes and brackets.
fn split_equals(s: &str) -> Option<(String, String)> {
    let b = s.as_bytes();
    let (mut depth, mut quote) = (0i32, None::<u8>);
    let mut i = 0;
    while i + 3 < b.len() {
        let c = b[i];
        match quote {
            Some(_) if c == b'\\' => i += 1,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                b'\'' | b'"' => quote = Some(c),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b' ' if depth == 0 && &b[i..i + 4] == b" == " => {
                    return Some((s[..i].trim().to_string(), s[i + 4..].trim().to_string()));
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn pytest_results(root: &Path, ids: &[String], log: &str) -> Vec<Outcome> {
    let mut out: Vec<Outcome> = ids.iter().map(|id| Outcome { id: id.clone(), ..Default::default() }).collect();
    let find = |out: &mut Vec<Outcome>, id: &str| -> usize {
        match out.iter().position(|o| o.id == id) {
            Some(i) => i,
            None => {
                out.push(Outcome { id: id.into(), ..Default::default() });
                out.len() - 1
            }
        }
    };
    // Sections of the short tracebacks, by the heading pytest gives them.
    let mut sections: HashMap<String, Vec<&str>> = HashMap::new();
    let mut current: Option<String> = None;
    let mut in_durations = false;
    for line in log.lines() {
        if line.starts_with("====") {
            current = None;
            in_durations = line.contains("slowest") && line.contains("durations");
            continue;
        }
        if line.starts_with("____") && line.ends_with("____") {
            current = Some(line.trim_matches(|c| c == '_' || c == ' ').to_string());
            continue;
        }
        if let Some(c) = &current {
            sections.entry(c.clone()).or_default().push(line);
            continue;
        }
        if in_durations {
            // 0.10s call     test_a.py::test_x
            let mut f = line.split_whitespace();
            if let (Some(t), Some(_), Some(id)) = (f.next(), f.next(), f.next())
                && let Ok(t) = t.trim_end_matches('s').parse::<f64>()
            {
                let i = find(&mut out, id);
                out[i].duration = Some(out[i].duration.unwrap_or(0.0) + t);
            }
            continue;
        }
        for (word, status) in [("PASSED", Status::Passed), ("FAILED", Status::Failed), ("ERROR", Status::Failed), ("SKIPPED", Status::Skipped), ("XFAIL", Status::Skipped), ("XPASS", Status::Passed)] {
            if let Some(head) = line.strip_prefix(&format!("{word} ")) {
                // The short summary: FAILED test_a.py::test_x - assert 1 == 2
                let (id, message) = head.split_once(" - ").unwrap_or((head, ""));
                if id.contains("::") {
                    let i = find(&mut out, id.trim());
                    out[i].status = Some(status);
                    if !message.is_empty() {
                        out[i].message = Some(message.trim().to_string());
                    }
                }
            } else if let Some(at) = line.find(&format!(" {word}")).filter(|_| line.contains("::") && !line.starts_with(' ')) {
                // The verbose line: test_a.py::test_x PASSED [ 50%]
                let id = &line[..at];
                if !id.contains(' ') {
                    let i = find(&mut out, id);
                    if out[i].status.is_none() || word != "ERROR" {
                        out[i].status = Some(status);
                    }
                }
            }
        }
    }
    for o in &mut out {
        let Some((file, rest)) = o.id.split_once("::") else { continue };
        let heading = rest.replace("::", ".");
        let Some(lines) = sections.get(&heading).or_else(|| sections.get(rest.rsplit("::").next().unwrap_or(rest))) else { continue };
        let frames: Vec<(String, usize)> = lines
            .iter()
            .filter(|l| !l.starts_with(' ') && !l.starts_with("E "))
            .filter_map(|l| {
                let mut p = l.splitn(3, ':');
                let (f, n) = (p.next()?, p.next()?.parse::<usize>().ok()?);
                Some((f.to_string(), n))
            })
            .collect();
        let frame = frames.iter().rev().find(|(f, _)| f == file).or(frames.first());
        o.at = frame.map(|(f, n)| (root.join(f), n.saturating_sub(1)));
        let errors: Vec<&str> = lines.iter().filter_map(|l| l.strip_prefix("E ")).map(str::trim).collect();
        if o.message.is_none() {
            o.message = errors.first().map(|s| s.to_string());
        }
        o.values = errors.iter().find_map(|e| {
            let e = e.strip_prefix("AssertionError: ").unwrap_or(e);
            split_equals(e.strip_prefix("assert ")?)
        });
        // pytest writes `assert actual == expected`, so the left is what was found.
        o.values = o.values.take().map(|(actual, expected)| (expected, actual));
    }
    out
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn js_results(root: &Path, report: &str, log: &str) -> Vec<Outcome> {
    let Some(v) = json_in(report) else { return Vec::new() };
    let mut out = Vec::new();
    for file in v["testResults"].as_array().into_iter().flatten() {
        let Some(name) = file["name"].as_str() else { continue };
        let path = root.join(name);
        let rel = relative(root, &path);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let results = file["assertionResults"].as_array().cloned().unwrap_or_default();
        if results.is_empty() {
            // The file did not run: a syntax error, or no tests in it.
            let message = file["message"].as_str().map(strip_ansi).unwrap_or_else(|| log.lines().last().unwrap_or("").into());
            let failed = file["status"].as_str() == Some("failed");
            out.push(Outcome {
                id: rel.clone(),
                status: Some(if failed { Status::Failed } else { Status::Skipped }),
                message: message.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()),
                file: Some(path.clone()),
                ..Default::default()
            });
            continue;
        }
        for t in results {
            let full = t["fullName"].as_str().map(str::to_string).unwrap_or_else(|| {
                let mut parts: Vec<String> = t["ancestorTitles"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect();
                parts.push(t["title"].as_str().unwrap_or("").into());
                parts.join(" ")
            });
            let status = match t["status"].as_str() {
                Some("passed") => Status::Passed,
                Some("failed") => Status::Failed,
                _ => Status::Skipped,
            };
            let failure = t["failureMessages"].as_array().and_then(|m| m.first()).and_then(|m| m.as_str()).map(strip_ansi);
            let at = failure.as_deref().and_then(|f| {
                let needle = format!("{}:", path.display());
                let i = f.find(&needle)? + needle.len();
                let n: String = f[i..].chars().take_while(char::is_ascii_digit).collect();
                Some((path.clone(), n.parse::<usize>().ok()?.saturating_sub(1)))
            });
            let value = |key: &str| failure.as_deref().and_then(|f| f.lines().find_map(|l| l.trim().strip_prefix(key)).map(|v| v.trim().to_string()));
            let line = t["location"]["line"].as_u64().map(|l| (l as usize).saturating_sub(1)).or_else(|| js_line(&text, t["title"].as_str().unwrap_or("")));
            out.push(Outcome {
                id: format!("{rel}::{full}"),
                status: Some(status),
                duration: t["duration"].as_f64().map(|ms| ms / 1000.0),
                message: failure.as_deref().and_then(|f| f.lines().find(|l| !l.trim().is_empty())).map(|l| l.trim().to_string()),
                at: at.or_else(|| (status == Status::Failed).then(|| line.map(|l| (path.clone(), l))).flatten()),
                values: value("Expected:").zip(value("Received:")),
                file: Some(path.clone()),
                line,
            });
        }
    }
    out
}

fn go_results(root: &Path, log: &str) -> Vec<Outcome> {
    let mut out: Vec<Outcome> = Vec::new();
    let mut failed_packages: Vec<(String, String)> = Vec::new();
    let mut outputs: HashMap<String, String> = HashMap::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let (Some(action), Some(package)) = (v["Action"].as_str(), v["Package"].as_str()) else { continue };
        let Some(test) = v["Test"].as_str() else {
            if action == "fail" {
                failed_packages.push((package.into(), outputs.remove(package).unwrap_or_default()));
            } else if action == "output" {
                outputs.entry(package.into()).or_default().push_str(v["Output"].as_str().unwrap_or(""));
            }
            continue;
        };
        // Subtests count toward their parent.
        if test.contains('/') {
            if action == "output" {
                let top = test.split('/').next().unwrap_or(test);
                outputs.entry(format!("{package}::{top}")).or_default().push_str(v["Output"].as_str().unwrap_or(""));
            }
            continue;
        }
        let id = format!("{package}::{test}");
        match action {
            "output" => outputs.entry(id).or_default().push_str(v["Output"].as_str().unwrap_or("")),
            "pass" | "fail" | "skip" => {
                let status = match action {
                    "pass" => Status::Passed,
                    "fail" => Status::Failed,
                    _ => Status::Skipped,
                };
                let mut o = Outcome { id: id.clone(), status: Some(status), duration: v["Elapsed"].as_f64(), ..Default::default() };
                if status == Status::Failed {
                    let text = outputs.get(&id).cloned().unwrap_or_default();
                    // "    sum_test.go:12: got 3, want 4"
                    if let Some((file, line, message)) = text.lines().find_map(|l| {
                        let l = l.trim();
                        let (file, rest) = l.split_once(".go:")?;
                        let (n, message) = rest.split_once(':')?;
                        Some((format!("{file}.go"), n.parse::<usize>().ok()?, message.trim().to_string()))
                    }) {
                        let dir = go_dir(root, package, &file);
                        o.at = Some((dir.join(&file), line.saturating_sub(1)));
                        o.values = message.split_once("want").and_then(|(got, want)| {
                            let got = got.trim().trim_end_matches(',').trim().strip_prefix("got")?;
                            Some((want.trim_start_matches([':', ' ']).trim().to_string(), got.trim_start_matches([':', ' ']).trim().to_string()))
                        });
                        o.message = Some(message);
                    }
                }
                out.push(o);
            }
            _ => {}
        }
    }
    // A package that does not build fails every test asked of it.
    for (package, text) in failed_packages {
        if !out.iter().any(|o| o.id.starts_with(&format!("{package}::"))) {
            out.push(Outcome {
                id: format!("{package}::"),
                status: Some(Status::Failed),
                message: text.lines().find(|l| !l.trim().is_empty() && !l.starts_with("FAIL")).map(|l| l.trim().to_string()),
                ..Default::default()
            });
        }
    }
    out
}

/// A Go package's folder: found from the file a failure names.
fn go_dir(root: &Path, package: &str, file: &str) -> PathBuf {
    let mut parts: Vec<&str> = package.split('/').collect();
    while !parts.is_empty() {
        for skip in 0..parts.len() {
            let dir = root.join(parts[skip..].join("/"));
            if dir.join(file).is_file() {
                return dir;
            }
        }
        parts.clear();
    }
    root.to_path_buf()
}

fn ctest_results(log: &str) -> Vec<Outcome> {
    let mut out: Vec<Outcome> = Vec::new();
    let mut lines = log.lines().peekable();
    while let Some(line) = lines.next() {
        // "1/2 Test #1: one ..............   Passed    0.01 sec"
        let Some(i) = line.find(" Test ") else { continue };
        let rest = line[i + 6..].trim_start();
        let Some((_, rest)) = rest.strip_prefix('#').and_then(|r| r.split_once(": ")) else { continue };
        let Some(dots) = rest.find(" .") else { continue };
        let name = &rest[..dots];
        let tail = rest[dots..].trim_start_matches([' ', '.']).trim_start_matches('*');
        let status = if tail.starts_with("Passed") {
            Status::Passed
        } else if tail.starts_with("Not Run") || tail.starts_with("Skipped") || tail.starts_with("Disabled") {
            Status::Skipped
        } else {
            Status::Failed
        };
        let duration = tail.split_whitespace().rev().nth(1).and_then(|t| t.parse::<f64>().ok());
        let word = tail.split("  ").next().unwrap_or(tail).trim().to_string();
        let mut o = Outcome { id: name.into(), status: Some(status), duration, ..Default::default() };
        if status == Status::Failed {
            let detail = lines.peek().filter(|l| !l.contains(" Test ") && !l.trim().is_empty()).map(|l| l.trim().to_string());
            o.message = Some(detail.map_or(word.clone(), |d| format!("{word}: {d}")));
        }
        out.push(o);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ld-runners-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn detects_every_runner_a_project_uses() {
        let root = scratch("detect");
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(root.join("package.json"), r#"{"devDependencies":{"jest":"29","vitest":"1"}}"#).unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(root.join("go.mod"), "module m").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build/CTestTestfile.cmake"), "").unwrap();
        std::fs::write(root.join("test_x.py"), "").unwrap();
        let found = detect(&root, "python3", &|_| false);
        let kinds: Vec<Kind> = found.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [Kind::Cargo, Kind::Pytest, Kind::Jest, Kind::Vitest, Kind::Go, Kind::Ctest]);
        assert_eq!(found[2].launch, "pnpm exec jest");
        assert_eq!(found[5].launch, "build");
        // nextest is preferred when installed.
        assert_eq!(detect(&root, "python3", &|p| p == "cargo-nextest")[0].kind, Kind::Nextest);
        std::fs::remove_file(root.join("pnpm-lock.yaml")).unwrap();
        std::fs::write(root.join("yarn.lock"), "").unwrap();
        assert_eq!(detect(&root, "python3", &|_| false)[3].launch, "yarn vitest");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cargo_lists_runs_and_reads_assert_eq_values() {
        let root = Path::new("/p");
        let r = Runner { kind: Kind::Cargo, launch: String::new() };
        let found = parse_discovery(&r, root, "app::tests::adds: test\nmain: test\nbench_x: benchmark\n\n2 tests, 1 benchmark", "");
        assert_eq!(found.iter().map(|f| (f.id.as_str(), f.group.as_str())).collect::<Vec<_>>(), [("app::tests::adds", "app::tests"), ("main", "(crate root)")]);
        let log = "running 2 tests\ntest app::tests::adds ... FAILED\ntest main ... ignored\n\nfailures:\n\n---- app::tests::adds stdout ----\n\nthread 'app::tests::adds' (4242) panicked at src/app.rs:12:9:\nassertion `left == right` failed: sums differ\n  left: [1, 2]\n right: [1, 3]\nnote: run with `RUST_BACKTRACE=1`\n";
        let out = parse_results(&r, root, &["app::tests::adds".into(), "main".into()], log, "");
        assert_eq!(out[0].status, Some(Status::Failed));
        assert_eq!(out[0].message.as_deref(), Some("assertion `left == right` failed: sums differ"));
        assert_eq!(out[0].at, Some((PathBuf::from("/p/src/app.rs"), 11)));
        assert_eq!(out[0].values, Some(("[1, 2]".into(), "[1, 3]".into())));
        assert_eq!(out[1].status, Some(Status::Skipped));
        // Before Rust 1.73.
        let old = "thread 'main' panicked at 'assertion failed: `(left == right)`\n  left: `1`,\n right: `2`', src/main.rs:3:5\n";
        let (_, at, values) = rust_panic(root, "main", old);
        assert_eq!(at, Some((PathBuf::from("/p/src/main.rs"), 2)));
        assert_eq!(values, Some(("1".into(), "2".into())));
        assert!(run_command(&r, &["a b".into()], false).ends_with("--exact --color=never 'a b'"));
    }

    #[test]
    fn nextest_lists_and_reads_durations() {
        let root = Path::new("/p");
        let r = Runner { kind: Kind::Nextest, launch: String::new() };
        let report = r#"{"rust-build-meta":{},"test-count":2,"rust-suites":{"lib":{"binary-id":"lib","testcases":{"m::a":{"ignored":false},"m::b":{"ignored":true}}}}}"#;
        assert_eq!(parse_discovery(&r, root, "", report).len(), 2);
        let log = "    Starting 2 tests across 1 binary\n        PASS [   0.004s] lib m::a\n        FAIL [   1.250s] lib m::b\n--- STDERR:              lib m::b ---\nthread 'm::b' panicked at src/m.rs:9:5:\nboom\n";
        let out = parse_results(&r, root, &["m::a".into(), "m::b".into()], log, "");
        assert_eq!((out[0].status, out[0].duration), (Some(Status::Passed), Some(0.004)));
        assert_eq!((out[1].status, out[1].message.as_deref()), (Some(Status::Failed), Some("boom")));
        assert!(run_command(&r, &["m::a".into()], false).contains("-E 'test(=m::a)'"));
    }

    #[test]
    fn pytest_reads_status_duration_place_and_values() {
        let root = scratch("pytest");
        std::fs::write(root.join("test_a.py"), "def test_ok():\n    pass\n\ndef test_sum():\n    assert add(1, 2) == 4\n").unwrap();
        let r = Runner { kind: Kind::Pytest, launch: "python3".into() };
        let found = parse_discovery(&r, &root, "test_a.py::test_ok\ntest_a.py::test_sum\n\n2 tests collected in 0.01s", "");
        assert_eq!((found[1].group.as_str(), found[1].line), ("test_a.py", Some(3)));
        let log = "test_a.py::test_ok PASSED                                     [ 50%]\ntest_a.py::test_sum FAILED                                    [100%]\n\n=================================== FAILURES ===================================\n___________________________________ test_sum ___________________________________\ntest_a.py:5: in test_sum\n    assert add(1, 2) == 4\nE   assert 3 == 4\nE    +  where 3 = add(1, 2)\n============================== slowest durations ===============================\n0.20s call     test_a.py::test_sum\n0.01s setup    test_a.py::test_sum\n0.00s call     test_a.py::test_ok\n=========================== short test summary info ============================\nFAILED test_a.py::test_sum - assert 3 == 4\n========================= 1 failed, 1 passed in 0.03s ==========================\n";
        let out = parse_results(&r, &root, &["test_a.py::test_ok".into(), "test_a.py::test_sum".into()], log, "");
        assert_eq!(out[0].status, Some(Status::Passed));
        let sum = &out[1];
        assert_eq!(sum.status, Some(Status::Failed));
        assert!((sum.duration.unwrap() - 0.21).abs() < 1e-9);
        assert_eq!(sum.message.as_deref(), Some("assert 3 == 4"));
        assert_eq!(sum.at, Some((root.join("test_a.py"), 4)));
        assert_eq!(sum.values, Some(("4".into(), "3".into())));
        assert_eq!(split_equals("'a == b' == [1, 2]"), Some(("'a == b'".into(), "[1, 2]".into())));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn jest_lists_files_and_reads_json_results() {
        let root = scratch("jest");
        std::fs::write(root.join("sum.test.js"), "describe('sum', () => {\n  it('adds', () => {\n    expect(sum(1, 2)).toBe(4);\n  });\n});\n").unwrap();
        let r = Runner { kind: Kind::Jest, launch: "npx --no-install jest".into() };
        let found = parse_discovery(&r, &root, &format!("{}\n", root.join("sum.test.js").display()), "");
        assert_eq!(found.len(), 1);
        assert!(found[0].whole_file);
        let file = root.join("sum.test.js");
        let report = serde_json::json!({"testResults":[{"name":file,"status":"failed","assertionResults":[
            {"fullName":"sum adds","title":"adds","ancestorTitles":["sum"],"status":"failed","duration":7,
             "failureMessages":[format!("Error: \u{1b}[2mexpect(\u{1b}[22mreceived).toBe(expected)\n\nExpected: \u{1b}[32m4\u{1b}[39m\nReceived: \u{1b}[31m3\u{1b}[39m\n    at Object.<anonymous> ({}:3:23)", file.display())],
             "location":{"line":2,"column":3}}]}]}).to_string();
        let out = parse_results(&r, &root, &["sum.test.js".into()], "", &report);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "sum.test.js::sum adds");
        assert_eq!(out[0].duration, Some(0.007));
        assert_eq!(out[0].at, Some((file.clone(), 2)));
        assert_eq!(out[0].values, Some(("4".into(), "3".into())));
        assert_eq!(out[0].line, Some(1));
        let cmd = run_command(&r, &["sum.test.js::sum adds".into()], false);
        assert!(cmd.contains("-t '^(sum adds)$'") && cmd.contains("--runTestsByPath -- 'sum.test.js'"), "{cmd}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vitest_lists_tests_with_their_lines() {
        let root = scratch("vitest");
        std::fs::write(root.join("a.test.ts"), "import { test } from 'vitest'\ntest('works', () => {})\n").unwrap();
        let r = Runner { kind: Kind::Vitest, launch: "npx --no-install vitest".into() };
        let report = format!("[{{\"name\":\"works\",\"file\":\"{}\"}},{{\"name\":\"suite > nested\",\"file\":\"{}\"}}]", root.join("a.test.ts").display(), root.join("a.test.ts").display());
        let found = parse_discovery(&r, &root, "", &report);
        assert_eq!(found[0].id, "a.test.ts::works");
        assert_eq!(found[0].line, Some(1));
        assert_eq!(found[1].id, "a.test.ts::suite nested");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn go_lists_by_package_and_reads_json_events() {
        let root = scratch("go");
        std::fs::create_dir_all(root.join("calc")).unwrap();
        std::fs::write(root.join("calc/sum_test.go"), "package calc\n\nfunc TestSum(t *testing.T) {\n\tt.Errorf(\"got 3, want 4\")\n}\n").unwrap();
        let r = Runner { kind: Kind::Go, launch: "go".into() };
        let log = format!("m/calc\t{}\n@@LD-GO-TESTS@@\nTestSum\nExampleSum\nok  \tm/calc\t0.002s\n?   \tm/empty\t[no test files]\n", root.join("calc").display());
        let found = parse_discovery(&r, &root, &log, "");
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].id.as_str(), found[0].line), ("m/calc::TestSum", Some(2)));
        let run = [
            r#"{"Action":"run","Package":"m/calc","Test":"TestSum"}"#,
            r#"{"Action":"output","Package":"m/calc","Test":"TestSum","Output":"    sum_test.go:4: got 3, want 4\n"}"#,
            r#"{"Action":"fail","Package":"m/calc","Test":"TestSum","Elapsed":0.01}"#,
            r#"{"Action":"pass","Package":"m/calc","Test":"ExampleSum","Elapsed":0}"#,
        ]
        .join("\n");
        let out = parse_results(&r, &root, &[], &run, "");
        assert_eq!(out[0].status, Some(Status::Failed));
        assert_eq!(out[0].at, Some((root.join("calc/sum_test.go"), 3)));
        assert_eq!(out[0].values, Some(("4".into(), "3".into())));
        assert_eq!(out[1].status, Some(Status::Passed));
        assert!(run_command(&r, &["m/calc::TestSum".into()], false).contains("go test -json -run '^(TestSum)$' 'm/calc'"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ctest_lists_declarations_and_reads_results() {
        let root = Path::new("/p");
        let r = Runner { kind: Kind::Ctest, launch: "build".into() };
        let report = r#"{"kind":"ctestInfo","version":{"major":1,"minor":0},"backtraceGraph":{"commands":["add_test"],"files":["/p/CMakeLists.txt"],"nodes":[{"file":0},{"command":0,"file":0,"line":7,"parent":0}]},"tests":[{"name":"unit.one","backtrace":1},{"name":"two"}]}"#;
        let found = parse_discovery(&r, root, "", report);
        assert_eq!((found[0].path.as_deref(), found[0].line), (Some(Path::new("/p/CMakeLists.txt")), Some(6)));
        assert_eq!(found[1].group, "CTest");
        let log = "Test project /p/build\n    Start 1: unit.one\n1/2 Test #1: unit.one .........................   Passed    0.01 sec\n    Start 2: two\n2/2 Test #2: two ..............................***Failed    0.20 sec\nexpected 4\n";
        let out = parse_results(&r, root, &[], log, "");
        assert_eq!((out[0].status, out[0].duration), (Some(Status::Passed), Some(0.01)));
        assert_eq!(out[1].status, Some(Status::Failed));
        assert_eq!(out[1].message.as_deref(), Some("Failed: expected 4"));
        assert!(run_command(&r, &["unit.one".into()], false).ends_with("-R '^(unit\\.one)$'"));
    }

    #[test]
    fn rust_tests_are_located_by_module_path() {
        let root = scratch("locate");
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::write(root.join("src/app/a.rs"), "#[test]\nfn same() {}\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "\n\n#[test]\nfn same() {}\nfn only() {}\n").unwrap();
        let files = vec![root.join("src/app/a.rs"), root.join("src/b.rs")];
        let found = locate_rust(&root, &files, &["app::a::tests::same".into(), "b::tests::same".into(), "b::only".into()]);
        assert_eq!(found["app::a::tests::same"], (root.join("src/app/a.rs"), 1));
        assert_eq!(found["b::tests::same"], (root.join("src/b.rs"), 3));
        assert_eq!(found["b::only"].1, 4);
        std::fs::remove_dir_all(root).unwrap();
    }
}
