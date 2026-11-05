//! Mathematics in markdown, typeset by TeX: each formula is built into a
//! page of its own, and the page drawn where the formula's source stands.
//! Formulas are built in batches on another thread; one that TeX cannot
//! build stays as source.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use crate::kernel::Waker;
use crate::tex::{self, Engine};

/// A formula, as asked for: its TeX, and whether it is displayed on a
/// line of its own.
pub type Ask = (String, bool);

/// A formula built: the image cache's name for its page, its size in
/// points, and how far its baseline is below its top.
#[derive(Clone, Debug, PartialEq)]
pub struct Formula {
    pub key: PathBuf,
    pub w: f32,
    pub h: f32,
    pub ascent: f32,
}

#[derive(Clone, Debug, PartialEq)]
enum State {
    Pending,
    Ready(Formula),
    Failed,
}

pub struct Maths {
    states: HashMap<Ask, State>,
    /// Formulas asked for since the last batch was sent.
    queue: Vec<Ask>,
    tx: Option<Sender<(Vec<Ask>, String)>>,
    done: Arc<Mutex<Vec<(Ask, State)>>>,
    waker: Waker,
    /// The engine to build with, as the settings name it.
    pub engine: String,
}

/// TeX's border around each formula's page, in points.
const BORDER: f32 = 1.0;

/// The document that builds a batch: a tight page for each formula.
pub fn document(batch: &[Ask]) -> String {
    let mut out = String::from(
        "\\documentclass[10pt]{article}\n\\usepackage{amsmath,amssymb}\n\
         \\usepackage[active,tightpage]{preview}\n\\setlength\\PreviewBorder{1pt}\n\\pagestyle{empty}\n\
         \\begin{document}\n",
    );
    for (tex, display) in batch {
        let style = if *display { "\\displaystyle " } else { "" };
        out += &format!("\\begin{{preview}}${style}{}$\\end{{preview}}\n", tex.trim());
    }
    out += "\\end{document}\n";
    out
}

/// The height and depth of each formula, in points, from what the
/// preview package writes in the log: `Preview: Snippet 1 ended.(H+DxW).`
pub fn snippets(log: &str) -> Vec<(f32, f32)> {
    let sp = |s: &str| s.trim().parse::<f32>().ok().map(|v| v / 65536.0);
    let mut out = Vec::new();
    for line in log.lines() {
        let Some(rest) = line.split_once("ended.(").map(|(_, r)| r) else { continue };
        let Some((hd, _)) = rest.split_once('x') else { continue };
        let Some((h, d)) = hd.split_once('+') else { continue };
        if let (Some(h), Some(d)) = (sp(h), sp(d)) {
            out.push((h, d));
        }
    }
    out
}

/// Builds a batch in a folder of its own. Returns each formula's state,
/// or None if the build failed as a whole.
fn build(engine: &Engine, batch: &[Ask]) -> Option<Vec<State>> {
    let mut hash = DefaultHasher::new();
    batch.hash(&mut hash);
    let dir = crate::config::cache_dir().join("math").join(format!("{:016x}", hash.finish()));
    let (source, pdf) = (dir.join("m.tex"), dir.join("m.pdf"));
    if !pdf.is_file() {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&source, document(batch)).ok()?;
        let mut cmd = engine.command(&source);
        if matches!(engine, Engine::Tectonic(_)) {
            cmd.arg("--keep-logs");
        }
        let out = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().ok()?;
        if !out.success() {
            let _ = std::fs::remove_file(&pdf);
            return None;
        }
    }
    let pages = crate::pdf::Pages::open(&pdf).ok()?;
    if pages.sizes.len() != batch.len() {
        return None;
    }
    let log = std::fs::read_to_string(dir.join("m.log")).unwrap_or_default();
    let dims = snippets(&log);
    let k = 72.0 / 72.27;
    Some(
        (0..batch.len())
            .map(|n| {
                let (w, h) = pages.sizes[n];
                let ascent = dims.get(n).map_or(h * 0.75, |&(above, _)| (above + BORDER) * k);
                State::Ready(Formula { key: crate::pdf::clear_key(&pages.key(n)), w, h, ascent })
            })
            .collect(),
    )
}

/// Builds batches as they come; a batch that fails is built again a
/// formula at a time, so one mistake spoils only itself.
fn worker(rx: std::sync::mpsc::Receiver<(Vec<Ask>, String)>, done: Arc<Mutex<Vec<(Ask, State)>>>, waker: Waker) {
    for (batch, choice) in rx {
        let Some(engine) = tex::find(&choice) else {
            done.lock().unwrap().extend(batch.into_iter().map(|a| (a, State::Failed)));
            waker();
            continue;
        };
        let states = match build(&engine, &batch) {
            Some(states) => states,
            None if batch.len() == 1 => vec![State::Failed],
            None => batch
                .iter()
                .map(|one| build(&engine, std::slice::from_ref(one)).and_then(|s| s.into_iter().next()))
                .map(|s| s.unwrap_or(State::Failed))
                .collect(),
        };
        done.lock().unwrap().extend(batch.into_iter().zip(states));
        waker();
    }
}

impl Maths {
    pub fn new(engine: &str, waker: Waker) -> Maths {
        Maths {
            states: HashMap::new(),
            queue: Vec::new(),
            tx: None,
            done: Arc::new(Mutex::new(Vec::new())),
            waker,
            engine: engine.to_string(),
        }
    }

    /// Has the window woken when formulas are built.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = waker;
    }

    /// The formula built, asking for it to be built if it has not been.
    pub fn get(&mut self, tex: &str, display: bool) -> Option<&Formula> {
        let ask = (tex.to_string(), display);
        if !self.states.contains_key(&ask) {
            self.states.insert(ask.clone(), State::Pending);
            self.queue.push(ask.clone());
        }
        match self.states.get(&ask) {
            Some(State::Ready(f)) => Some(f),
            _ => None,
        }
    }

    /// Sends what has been asked for to be built.
    pub fn flush(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.queue);
        let tx = self.tx.get_or_insert_with(|| {
            let (tx, rx) = channel();
            let (done, waker) = (self.done.clone(), self.waker.clone());
            std::thread::spawn(move || worker(rx, done, waker));
            tx
        });
        let _ = tx.send((batch, self.engine.clone()));
    }

    /// Takes in what has been built. Returns whether anything was.
    pub fn poll(&mut self) -> bool {
        let done = std::mem::take(&mut *self.done.lock().unwrap());
        let any = !done.is_empty();
        self.states.extend(done);
        any
    }

    /// Forgets formulas that failed, to try them again: after the engine
    /// changed, say.
    pub fn retry(&mut self) {
        self.states.retain(|_, s| *s != State::Failed);
    }

    /// Waits for what is being built. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub fn settle(&mut self) {
        self.flush();
        let start = std::time::Instant::now();
        while self.states.values().any(|s| *s == State::Pending) && start.elapsed().as_secs() < 120 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            self.poll();
        }
    }
}

/// Whether an engine can be had to build formulas with.
#[cfg(test)]
fn possible(choice: &str) -> bool {
    tex::find(choice).is_some()
}

/// Where a built formula's PDF is, for tests.
#[cfg(test)]
fn pdf_of(key: &std::path::Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let bytes = key.as_os_str().as_bytes();
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes.split(|&b| b == 0).next().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_is_one_document() {
        let doc = document(&[("x^2".into(), false), ("\\frac{a}{b}".into(), true)]);
        assert!(doc.contains("\\begin{preview}$x^2$\\end{preview}"));
        assert!(doc.contains("\\begin{preview}$\\displaystyle \\frac{a}{b}$\\end{preview}"));
        let log = "Preview: Fontsize 10pt\nPreview: Snippet 1 ended.(491520+163840x1966080).\n\
                   Preview: Snippet 2 ended.(655360+0x655360).\n";
        assert_eq!(snippets(log), [(7.5, 2.5), (10.0, 0.0)]);
    }

    /// Built with the engine there is, where there is one.
    /// An engine to build with in tests: one on the `PATH`, or else the
    /// Tectonic Lightdown set up for the user.
    fn engine() -> Option<String> {
        let own = PathBuf::from(std::env::var_os("HOME")?).join(".local/share/lightdown/bin/tectonic");
        if possible("auto") { Some("auto".into()) } else { own.is_file().then(|| own.to_string_lossy().into_owned()) }
    }

    #[test]
    fn formulas_are_built() {
        let Some(engine) = engine() else { return };
        let mut m = Maths::new(&engine, Arc::new(|| {}));
        assert!(m.get("e^{i\\pi} + 1 = 0", false).is_none());
        m.get("\\frac{1", true);
        m.settle();
        let f = m.get("e^{i\\pi} + 1 = 0", false).cloned().unwrap();
        assert!(f.w > f.h && f.h > 5.0 && f.ascent > 0.0 && f.ascent < f.h, "{f:?}");
        assert!(pdf_of(&f.key).is_file());
        // The broken one stays source; the good one is not spoilt by it.
        assert!(m.get("\\frac{1", true).is_none());
        assert_eq!(m.states.get(&("\\frac{1".to_string(), true)), Some(&State::Failed));
    }
}

/// Build a formula only when an HTML export requests it, then keep vector paths.
pub fn svg(tex: &str, display: bool, choice: &str) -> Result<String, String> {
    let engine = tex::find(choice).ok_or("Mathematics in HTML needs a TeX engine")?;
    let batch = vec![(tex.to_string(), display)];
    let states = build(&engine, &batch).ok_or("Cannot typeset mathematics for HTML")?;
    let Some(State::Ready(formula)) = states.first() else { return Err("Cannot typeset mathematics for HTML".into()) };
    crate::pdf::formula_svg(&formula.key).ok_or("Cannot convert this formula to vector SVG".into())
}
