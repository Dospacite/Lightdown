//! Percent cells run unsaved code in persistent project kernels: Python on
//! the built-in helper or a Jupyter kernel, other languages on theirs.
use super::*;
use crate::kernelspec::{self, Choice};
use std::collections::HashMap;
#[derive(Default)]
pub(super) struct Scripts {
    /// One kernel per project and language.
    sessions: HashMap<(PathBuf, String), Session>,
    next: u64,
    /// Kernels chosen for a language's scripts, by project.
    pub(super) chosen: HashMap<(PathBuf, String), Choice>,
}
struct Session {
    kernel: Kernel,
    label: String,
    busy: Option<u64>,
    output: String,
    variables: Vec<(String, String, String)>,
}
/// The cells of a Python script.
#[cfg(test)]
fn ranges(text: &str) -> Vec<(usize, usize, bool)> {
    ranges_in(text, "python", "#")
}
/// The cells of a script whose markers are `<comment> %%`; markers inside
/// strings stay part of the string.
pub(super) fn ranges_in(text: &str, language: &str, comment: &str) -> Vec<(usize, usize, bool)> {
    let mut spans = Vec::new();
    if language == "python" {
        crate::py::spans(text, 0, &mut spans);
    } else {
        crate::syntax::spans(text, 0, language, &mut spans);
    }
    let marker = format!("{comment} %%");
    let mut marks = vec![(0, false)];
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed.strip_prefix(marker.as_str()).is_some_and(|r| r.is_empty() || r.starts_with([' ', '[']))
            && !spans.iter().any(|s| s.start as usize <= at && s.end as usize > at && s.attr & crate::md::SOFT != 0)
        {
            marks.push((at, trimmed.contains("[markdown]")));
        }
        at += line.len();
    }
    let mut out = Vec::new();
    for (i, &(start, markdown)) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map_or(text.len(), |m| m.0);
        if start < end {
            out.push((start, end, markdown));
        }
    }
    out
}
/// Text without the terminal's colour escapes, for the plain log.
fn strip_escapes(s: &str) -> String {
    crate::nb::ansi(s).0
}
/// The language of the current file's cells and its marker comment, when
/// its cells can run: Python files always, others once they have a marker.
fn script_of(doc: &Pane) -> Option<(&'static str, &'static str)> {
    if doc.doc == Doc::Python {
        return Some(("python", "#"));
    }
    let (language, comment) = kernelspec::script_language(doc.path.as_deref()?)?;
    let marker = format!("{comment} %%");
    doc.buf.text.lines().any(|l| l.trim_end().strip_prefix(marker.as_str()).is_some_and(|r| r.is_empty() || r.starts_with([' ', '['])))
        .then_some((language, comment))
}
impl App {
    /// Whether Ctrl+Enter in this file runs a percent cell.
    pub(super) fn script_cells_here(&self) -> bool {
        self.nb.is_none() && self.reader.is_none() && script_of(&self.doc).is_some()
    }
    fn script_key(&self) -> (PathBuf, String) {
        let language = script_of(&self.doc).map_or("python", |s| s.0);
        (self.root.clone(), language.to_string())
    }
    pub(super) fn scripts_busy(&self) -> bool {
        self.scripts.sessions.iter().any(|((root, _), s)| root == &self.root && s.busy.is_some())
    }
    pub(super) fn scripts_forget(&mut self, key: &(PathBuf, String)) {
        self.scripts.sessions.remove(key);
    }
    pub(super) fn scripts_reset(&mut self) {
        let root = self.root.clone();
        self.scripts.sessions.retain(|(r, _), _| r != &root);
    }
    /// The kernel the current file's cells run on: the one chosen, else the
    /// first installed for the language (Python: the interpreter's ipykernel,
    /// else the built-in helper).
    pub(super) fn script_choice(&self, language: &str) -> Result<Choice, String> {
        if let Some(choice) = self.scripts.chosen.get(&(self.root.clone(), language.to_string())) {
            return Ok(choice.clone());
        }
        let python = self.cfg.python_for(&self.root);
        let specs = kernelspec::discover(&python, &self.root);
        kernelspec::for_language(language, &specs, kernelspec::has_package(&python, "ipykernel"))
    }
    pub(super) fn script_command(&mut self, action: &str) {
        let key = self.script_key();
        let what = if key.1 == "python" { "Python".to_string() } else { key.1.clone() };
        if action == "output" {
            let text = self.scripts.sessions.get(&key).map_or(format!("No {what} cells have run."), |s| s.output.clone());
            self.open_output(text);
            return;
        }
        if action == "interrupt" {
            if let Some(s) = self.scripts.sessions.get_mut(&key) {
                s.kernel.interrupt();
            }
            return;
        }
        if action == "restart" {
            self.scripts.sessions.remove(&key);
            self.message = Some(format!("{what} cell kernel restarted"));
            return;
        }
        let Some((language, comment)) = script_of(&self.doc).filter(|_| self.reader.is_none()) else {
            self.message = Some("Open a Python source file, or a script with % cells, to run percent cells".into());
            return;
        };
        let cur = self.doc.buf.cur;
        let spans = ranges_in(&self.doc.buf.text, language, comment);
        let Some(&(mut a, mut b, markdown)) =
            spans.iter().find(|&&(a, b, _)| a <= cur && (cur < b || b == self.doc.buf.text.len()))
        else {
            return;
        };
        if action == "selection" {
            (a, b) = self.selected_text_range();
        }
        if markdown && action != "selection" {
            self.message = Some("This is a markdown cell".into());
            return;
        }
        if !self.scripts.sessions.contains_key(&key) {
            let python = self.cfg.python_for(&self.root);
            let root = self.root.clone();
            let started = match self.script_choice(language) {
                Ok(Choice::Builtin) => Kernel::spawn_in(&python, &root, &root, self.waker.clone())
                    .map(|k| (k, "Built-in Python".to_string()))
                    .map_err(|e| e.to_string()),
                Ok(Choice::Jupyter(spec)) if !kernelspec::has_package(&python, "jupyter_client") => {
                    if language == "python" {
                        Kernel::spawn_in(&python, &root, &root, self.waker.clone())
                            .map(|k| (k, "Built-in Python".to_string()))
                            .map_err(|e| e.to_string())
                    } else {
                        Err(format!(
                            "{} needs jupyter_client in {python}: run `{python} -m pip install jupyter_client`",
                            spec.label()
                        ))
                    }
                }
                Ok(Choice::Jupyter(spec)) => Kernel::spawn_jupyter(&python, &spec, &root, &root, self.waker.clone())
                    .map(|k| (k, spec.label()))
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            match started {
                Ok((kernel, label)) => {
                    self.scripts.sessions.insert(
                        key.clone(),
                        Session { kernel, label, busy: None, output: String::new(), variables: Vec::new() },
                    );
                }
                Err(e) => {
                    self.message = Some(e);
                    return;
                }
            }
        }
        let s = self.scripts.sessions.get_mut(&key).unwrap();
        if s.busy.is_some() {
            self.message = Some(format!("A {what} cell is running"));
            return;
        }
        self.scripts.next += 1;
        let n = self.scripts.next;
        let code = self.doc.buf.text[a..b].to_string();
        s.output += &format!(
            "\n{}:{}\n>>> {}\n",
            self.doc.path.as_deref().unwrap_or(Path::new("untitled")).display(),
            self.doc.buf.line_of(a) + 1,
            code.trim()
        );
        s.kernel.execute(n, &code);
        s.busy = Some(n);
        self.message = Some(format!("Running {what} cell on {}…", s.label));
        if action == "next" && b < self.doc.buf.text.len() {
            self.doc.select(b, b);
            self.doc.moved();
        }
    }
    pub(super) fn scripts_poll(&mut self) {
        for ((root, language), s) in &mut self.scripts.sessions {
            for ev in s.kernel.poll() {
                match ev {
                    Event::Stream { text, .. } => {
                        s.output += &strip_escapes(&text);
                        if !text.ends_with('\n') {
                            s.output.push('\n');
                        }
                    }
                    Event::Show { data, update: false, .. } => {
                        let text = crate::kernel::plain(&data);
                        let image = ["image/png", "image/jpeg", "image/svg+xml"].iter().any(|m| data.contains_key(*m));
                        s.output += &if image && text.is_empty() { "[image]".to_string() } else { text };
                        s.output.push('\n');
                    }
                    Event::Error { trace, .. } => {
                        s.output += &strip_escapes(&trace.join("\n"));
                        s.output.push('\n');
                    }
                    Event::Input { prompt, .. } => {
                        // Scripts have no place to type; the cell reads an empty line.
                        s.kernel.input("");
                        s.output += &format!("{prompt}(input is not read in script cells; an empty line was sent)\n");
                    }
                    Event::Timing { elapsed_ms, .. } => s.output += &format!("{:.3}s\n", elapsed_ms / 1000.0),
                    Event::Variables(items) => s.variables = items,
                    Event::Done { ok, .. } => {
                        s.busy = None;
                        if root == &self.root {
                            let what = if language == "python" { "Python" } else { language.as_str() };
                            self.message = Some(format!(
                                "{what} cell {}. Python: Show cell output opens its log.",
                                if ok { "passed" } else { "failed" }
                            ));
                        }
                    }
                    Event::Dead(why) | Event::Missing(why) => {
                        s.busy = None;
                        s.variables.clear();
                        s.output += &format!("Kernel exited: {why}\n");
                    }
                    _ => {}
                }
            }
            if s.output.len() > 200_000 {
                let at = editor::ceil(&s.output, s.output.len() - 160_000);
                s.output.drain(..at);
            }
        }
    }
    pub(super) fn script_variables(&mut self) {
        let entries = self
            .scripts
            .sessions
            .get(&self.script_key())
            .map(|s| s.variables.iter().map(|(n, t, shape)| Entry::new(n, n, &format!("{t} {shape}"))).collect())
            .unwrap_or_default();
        self.menu = Some((MenuKind::Variables, Menu::new("Python cell variables", entries, "")));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percent_cells_use_unicode_byte_offsets() {
        let text = "x='é'\n# %% next\ny=2\n# %% [markdown]\n# Heading\n";
        let r = ranges(text);
        assert_eq!(r.len(), 3);
        assert_eq!(&text[r[1].0..r[1].1], "# %% next\ny=2\n");
        assert!(r[2].2);
    }
}
#[cfg(test)]
mod execution_tests {
    use super::*;
    #[test]
    fn cells_share_a_kernel_load_dotenv_and_leave_the_script_unchanged() {
        let root = std::env::temp_dir().join(format!("ld-script-cells-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".env"), "CELL_FIXTURE=loaded\n").unwrap();
        let file = root.join("main.py");
        let text = "# %%\nimport os\nx=40\nprint(os.environ['CELL_FIXTURE'])\n# %%\nx + 2\n";
        std::fs::write(&file, text).unwrap();
        let mut a = App::new(Some(file.clone()));
        a.script_command("run");
        let until = std::time::Instant::now();
        while a.scripts_busy() && until.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            a.scripts_poll();
        }
        let key = (root.clone(), "python".to_string());
        assert!(a.scripts.sessions[&key].output.contains("loaded"));
        let at = text.rfind("# %%").unwrap();
        a.doc.select(at, at);
        a.script_command("run");
        while a.scripts_busy() && until.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            a.scripts_poll();
        }
        assert!(a.scripts.sessions[&key].output.contains("42"));
        assert!(a.scripts.sessions[&key].variables.iter().any(|(n, _, _)| n == "x"));
        let start = text.find("x=40").unwrap();
        a.doc.select(start, start + 4);
        a.script_command("selection");
        let until = std::time::Instant::now();
        while a.scripts_busy() && until.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            a.scripts_poll();
        }
        assert!(!a.scripts_busy());
        assert!(a.scripts.sessions[&key].output.contains(">>> x=40"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), text);
        assert!(!a.doc.buf.modified);
        assert_eq!(ranges("s='''\n# %%\n'''\n# %%\nx=1").len(), 2);
        a.scripts_reset();
        assert!(!a.scripts_busy());
        std::fs::remove_dir_all(root).unwrap();
    }
}
