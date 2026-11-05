//! The TeX toolchain, which Lightdown runs but does not hold: finding an
//! engine, building a document into a PDF, and fetching Tectonic for
//! whoever has no TeX at all.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use crate::config;
use crate::kernel::Waker;
use crate::tools::Job;

/// The engines that can be asked for by name; `auto` takes the first of
/// them that is installed.
pub const CHOICES: [&str; 6] = ["auto", "tectonic", "latexmk", "xelatex", "lualatex", "pdflatex"];

/// The Tectonic release fetched when GitHub cannot say which is the latest.
const TECTONIC: &str = "0.17.0";

/// A program that turns a `.tex` file into a PDF.
#[derive(Clone, PartialEq, Debug)]
pub enum Engine {
    /// Fetches the packages a document needs and runs as often as needed.
    Tectonic(PathBuf),
    /// Runs the other engines as often as needed.
    Latexmk(PathBuf),
    /// `pdflatex`, `xelatex` or `lualatex`, run once and again if the
    /// document asks for it.
    Latex(PathBuf),
}

/// Where a program of this name is found on the `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Lightdown's own copy of Tectonic, once it has been set up.
pub fn own_tectonic() -> PathBuf {
    config::data_dir().join("bin/tectonic")
}

fn tectonic() -> Option<PathBuf> {
    Some(own_tectonic()).filter(|p| p.is_file()).or_else(|| which("tectonic"))
}

/// The engine to build with: the one chosen, if it is installed. A path
/// names a program, known by its name.
pub fn find(choice: &str) -> Option<Engine> {
    let latex = |name: &str| which(name).map(Engine::Latex);
    if choice.contains('/') {
        let path = PathBuf::from(choice);
        let name = path.file_name()?.to_string_lossy().into_owned();
        return path.is_file().then(|| match name.as_str() {
            n if n.starts_with("tectonic") => Engine::Tectonic(path),
            n if n.starts_with("latexmk") => Engine::Latexmk(path),
            _ => Engine::Latex(path),
        });
    }
    match choice {
        "tectonic" => tectonic().map(Engine::Tectonic),
        "latexmk" => which("latexmk").map(Engine::Latexmk),
        "xelatex" | "lualatex" | "pdflatex" => latex(choice),
        _ => tectonic()
            .map(Engine::Tectonic)
            .or_else(|| which("latexmk").map(Engine::Latexmk))
            .or_else(|| ["xelatex", "lualatex", "pdflatex"].into_iter().find_map(latex)),
    }
}

/// What the settings say about an engine: where it comes from, or that it
/// is missing.
pub fn describe(choice: &str) -> String {
    match (choice, find(choice)) {
        ("auto", Some(e)) => format!("uses {}", e.name()),
        ("auto", None) => "none installed".into(),
        (path, Some(_)) if path.contains('/') => "this program".into(),
        (_, Some(Engine::Tectonic(p))) if p == own_tectonic() => "Lightdown's own".into(),
        (_, Some(_)) => "installed".into(),
        (_, None) => "not installed".into(),
    }
}

impl Engine {
    pub fn name(&self) -> String {
        let (Engine::Tectonic(p) | Engine::Latexmk(p) | Engine::Latex(p)) = self;
        p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())
    }

    /// The command that builds `file` once, in its own folder, where the
    /// PDF is written.
    pub fn command(&self, file: &Path) -> Command {
        let mut cmd = match self {
            Engine::Tectonic(p) => {
                let mut cmd = Command::new(p);
                cmd.args(["--color", "never", "--chatter", "minimal", "--synctex"]);
                cmd
            }
            Engine::Latexmk(p) => {
                let mut cmd = Command::new(p);
                cmd.args(["-pdf", "-synctex=1", "-interaction=nonstopmode", "-halt-on-error", "-file-line-error"]);
                cmd
            }
            Engine::Latex(p) => {
                let mut cmd = Command::new(p);
                cmd.args(["-synctex=1", "-interaction=nonstopmode", "-halt-on-error", "-file-line-error"]);
                cmd
            }
        };
        let dir = file.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        cmd.arg(file.file_name().unwrap_or_default()).current_dir(dir);
        cmd
    }
}

/// Where a build's PDF is written: beside its source.
pub fn pdf_of(source: &Path) -> PathBuf {
    source.with_extension("pdf")
}

/// Something the engine said went wrong.
#[derive(Clone, PartialEq, Debug)]
pub struct Problem {
    /// The file and line it is about, where the engine says.
    pub file: String,
    pub line: Option<usize>,
    pub text: String,
}

/// Reads the errors out of what an engine printed. Engines say them as
/// `file.tex:12: message`, Tectonic after `error: `, and TeX itself on
/// lines that start with `!`.
pub fn problems(log: &str) -> Vec<Problem> {
    let mut found: Vec<Problem> = Vec::new();
    let mut bare = Vec::new();
    for line in log.lines().map(str::trim_end) {
        let said = line.strip_prefix("error: ").unwrap_or(line);
        if let Some(at) = said.find(".tex:") {
            let rest = &said[at + 5..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if let Some(text) = rest[digits..].strip_prefix(": ").filter(|_| digits > 0) {
                let file = said[..at + 4].trim_start_matches("./").to_string();
                let p = Problem { file, line: rest[..digits].parse().ok(), text: text.trim().to_string() };
                if !found.contains(&p) {
                    found.push(p);
                }
                continue;
            }
        }
        if let Some(text) = line.strip_prefix("! ") {
            bare.push(Problem { file: String::new(), line: None, text: text.to_string() });
        }
    }
    if found.is_empty() {
        found = bare;
        found.dedup();
    }
    found
}

/// The last thing a failed build said, for when it names no error.
fn last_words(log: &str) -> String {
    let said = |l: &&str| !l.trim().is_empty() && !l.starts_with("Fontconfig") && !l.starts_with('[');
    let line = log.lines().rev().find(said).unwrap_or("The build failed");
    line.trim().trim_start_matches("error: ").to_string()
}

/// How a build went.
#[derive(Debug, Default)]
pub struct Outcome {
    pub ok: bool,
    pub pdf: Option<PathBuf>,
    pub problems: Vec<Problem>,
    /// Something to say besides: the copy made, or why not.
    pub note: Option<String>,
}

/// A build under way: the engine, and perhaps the engine again.
pub struct Build {
    engine: Engine,
    pub source: PathBuf,
    /// Where to copy the PDF once it is built.
    copy_to: Option<PathBuf>,
    job: Job,
    pass: u8,
}

impl Build {
    pub fn start(engine: Engine, source: PathBuf, copy_to: Option<PathBuf>, wake: Waker) -> Build {
        let job = Job::start(engine.command(&source), wake);
        Build { engine, source, copy_to, job, pass: 1 }
    }

    /// Takes in a step that has finished, and starts the next. Returns how
    /// the build went once there is nothing left to do.
    pub fn poll(&mut self, wake: &Waker) -> Option<Outcome> {
        let done = self.job.poll()?;
        self.next(done, wake)
    }

    /// Blocks until the build is over. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn wait(&mut self, wake: &Waker) -> Outcome {
        loop {
            let Some(done) = self.job.wait() else {
                return Outcome { problems: vec![problem("The build took too long")], ..Outcome::default() };
            };
            if let Some(outcome) = self.next(done, wake) {
                return outcome;
            }
        }
    }

    fn next(&mut self, (out, ok): (String, bool), wake: &Waker) -> Option<Outcome> {
        let pdf = pdf_of(&self.source);
        if !ok || !pdf.is_file() {
            let mut problems = problems(&out);
            if problems.is_empty() {
                problems.push(problem(&last_words(&out)));
            }
            return Some(Outcome { problems, ..Outcome::default() });
        }
        // References settle on a later run.
        let again = out.contains("Rerun to get") || out.contains("Label(s) may have changed");
        if matches!(self.engine, Engine::Latex(_)) && again && self.pass < 3 {
            self.job = Job::start(self.engine.command(&self.source), wake.clone());
            self.pass += 1;
            return None;
        }
        let note = self.copy_to.as_ref().filter(|to| **to != pdf).map(|to| match std::fs::copy(&pdf, to) {
            Ok(_) => format!("Exported to {}", to.display()),
            Err(e) => format!("Cannot export: {e}"),
        });
        Some(Outcome { ok: true, pdf: Some(pdf), problems: Vec::new(), note })
    }
}

fn problem(text: &str) -> Problem {
    Problem { file: String::new(), line: None, text: text.to_string() }
}

/// Fetching Tectonic, under way on a thread of its own.
pub struct Setup {
    done: Receiver<Result<String, String>>,
    /// What it is doing now, to show.
    pub stage: Arc<Mutex<String>>,
}

impl Setup {
    /// Downloads the latest Tectonic for this machine into Lightdown's own
    /// folder, where it needs no permission to install. The TeX packages
    /// themselves are fetched by Tectonic as documents need them.
    pub fn start(wake: Waker) -> Setup {
        let (tx, done) = channel();
        let stage = Arc::new(Mutex::new("Asking GitHub for the latest Tectonic".to_string()));
        let said = stage.clone();
        std::thread::spawn(move || {
            let step = |s: String| {
                *said.lock().unwrap() = s;
                wake();
            };
            let _ = tx.send(fetch(&step));
            wake();
        });
        Setup { done, stage }
    }

    /// The version set up, or why not, once it is over.
    pub fn poll(&self) -> Option<Result<String, String>> {
        self.done.try_recv().ok()
    }

    /// Blocks until it is over. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn wait(&self) -> Result<String, String> {
        self.done.recv().unwrap_or_else(|_| Err("The setup stopped".into()))
    }
}

/// The name of the Tectonic download for this machine.
fn target() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64-unknown-linux-musl",
        "arm" => "arm-unknown-linux-musleabihf",
        "x86" => "i686-unknown-linux-gnu",
        _ => "x86_64-unknown-linux-musl",
    }
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let out = cmd.stdin(std::process::Stdio::null()).output().map_err(|e| format!("Cannot run it: {e}"))?;
    let err = String::from_utf8_lossy(&out.stderr);
    match out.status.success() {
        true => Ok(out.stdout),
        false => Err(err.trim().lines().last().unwrap_or("it failed").to_string()),
    }
}

fn fetch(step: &dyn Fn(String)) -> Result<String, String> {
    let api = "https://api.github.com/repos/tectonic-typesetting/tectonic/releases/latest";
    let release = run(Command::new("curl").args(["-fsSL", "--max-time", "30", "-A", "lightdown", api]));
    let release: serde_json::Value = release.ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let name = format!("{}.tar.gz", target());
    let asset = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(&name)));
    let (url, bytes) = match asset {
        Some(a) => (a["browser_download_url"].as_str().unwrap_or_default().to_string(), a["size"].as_u64()),
        None => (
            format!(
                "https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%40{TECTONIC}/tectonic-{TECTONIC}-{name}"
            ),
            None,
        ),
    };
    let size = bytes.map_or(String::new(), |b| format!(", {:.0} MB", b as f64 / 1e6));
    step(format!("Downloading Tectonic{size}"));
    let bin = own_tectonic();
    let dir = bin.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("Cannot make {}: {e}", dir.display()))?;
    let archive = dir.join(".tectonic-download.tar.gz");
    let got = run(Command::new("curl").args(["-fsSL", "--max-time", "900", "-o"]).arg(&archive).arg(&url));
    if let Err(e) = got {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("Cannot download Tectonic: {e}"));
    }
    step("Unpacking Tectonic".into());
    let unpacked = run(Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(dir).arg("tectonic"));
    let _ = std::fs::remove_file(&archive);
    unpacked.map_err(|e| format!("Cannot unpack Tectonic: {e}"))?;
    let version = run(Command::new(&bin).arg("--version")).map_err(|e| format!("Tectonic does not run: {e}"))?;
    Ok(String::from_utf8_lossy(&version).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_read_from_the_log() {
        let tectonic = "note: Running TeX ...\nerror: b.tex:4: Undefined control sequence\n\
            error: something bad happened inside XeTeX; its output follows:\n! Undefined control sequence.\nl.4 \\foo";
        let found = problems(tectonic);
        assert_eq!(found, [Problem { file: "b.tex".into(), line: Some(4), text: "Undefined control sequence".into() }]);
        let latex =
            "./ch/a.tex:12: LaTeX Error: File `x.sty' not found.\n./ch/a.tex:12: LaTeX Error: File `x.sty' not found.";
        assert_eq!(problems(latex).len(), 1);
        assert_eq!((problems(latex)[0].file.as_str(), problems(latex)[0].line), ("ch/a.tex", Some(12)));
        // Without a file and line, TeX's own `!` lines are what is left.
        assert_eq!(problems("x\n! Emergency stop.\n")[0].text, "Emergency stop.");
        assert_eq!(last_words("one\nerror: two\n\n[exit status 1]"), "two");
    }

    /// A stand-in for an engine: a script that writes what it is given.
    fn engine(dir: &Path, script: &str) -> Engine {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-latex");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Engine::Latex(path)
    }

    #[test]
    fn builds_report_errors_and_copy_the_pdf() {
        let dir = std::env::temp_dir().join(format!("lightdown-tex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("a.tex");
        std::fs::write(&source, "x").unwrap();
        let wake: Waker = Arc::new(|| {});

        let failing = engine(&dir, "echo './a.tex:3: Undefined control sequence.'; exit 1");
        let out = Build::start(failing, source.clone(), None, wake.clone()).wait(&wake);
        assert!(!out.ok && out.pdf.is_none() && out.problems[0].line == Some(3));

        // The second run is asked for by the first, and counted.
        let script = "echo run >> runs; [ $(wc -l < runs) -lt 2 ] && echo 'Rerun to get cross-references right'; \
            echo pdf > a.pdf";
        let copy_to = Some(dir.join("out.pdf"));
        let out = Build::start(engine(&dir, script), source.clone(), copy_to, wake.clone()).wait(&wake);
        assert!(out.ok && out.pdf == Some(dir.join("a.pdf")) && out.problems.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("runs")).unwrap().lines().count(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("out.pdf")).unwrap(), "pdf\n");
        assert!(out.note.unwrap().starts_with("Exported to"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
