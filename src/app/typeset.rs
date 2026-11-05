//! Building the document into a PDF, at work in the window: LaTeX as it
//! is, markdown by way of LaTeX. The PDF is previewed in the sidebar or
//! exported to a file, and a TeX engine is set up for those who have none.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{App, Focus, PromptKind, Tab};
use crate::config;
use crate::pane::Doc;
use crate::pdf::Pages;
use crate::preview::{Hit, Preview};
use crate::tex::{self, Build, Setup};
use crate::{export, tools};

/// What a build is for.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Purpose {
    Preview,
    /// Writing the PDF to this file.
    Export(PathBuf),
}

/// The builds of this window.
pub(super) struct Typeset {
    pub view: Preview,
    problem_file: Option<PathBuf>,
    problem_root: PathBuf,
    build: Option<(Build, Purpose, Instant)>,
    /// A build asked for while another ran, to start when it ends.
    again: Option<Purpose>,
    setup: Option<Setup>,
    /// A build that waits for a TeX engine to be set up.
    waiting: Option<Purpose>,
    /// The document has been previewed in this window, so saving it
    /// builds it again.
    live: bool,
    markdown_map: Option<crate::markdown_sync::Map>,
}

impl Typeset {
    pub fn new(view: Preview) -> Typeset {
        Typeset { view, problem_file: None, problem_root: PathBuf::new(), build: None, again: None, setup: None, waiting: None, live: false, markdown_map: None }
    }

    pub fn busy(&self) -> bool {
        self.build.is_some() || self.setup.is_some()
    }
}

/// Where a markdown document is written as LaTeX and built: a folder of
/// its own among the caches, so that the document's folder stays as it is.
fn workshop(doc: Option<&Path>, root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    doc.unwrap_or(root).hash(&mut h);
    config::cache_dir().join("build").join(format!("{:016x}", h.finish()))
}

impl App {
    pub(super) fn pdf_problems(&self) -> Vec<super::problems::Problem> {
        if self.typeset.problem_root != self.root { return Vec::new(); }
        self.typeset.view.problems.iter().map(|p| {
            let path = if p.file.is_empty() { self.typeset.problem_file.clone() } else {
                Some(self.typeset.problem_file.as_deref().and_then(Path::parent).unwrap_or(&self.root).join(&p.file))
            };
            super::problems::Problem { path,line:p.line.unwrap_or(1),col:1,message:p.text.clone(),source:"PDF" }
        }).collect()
    }

    /// Whether the document is one that is built into a PDF.
    pub(super) fn buildable(&self) -> bool {
        !self.doc.read_only
            && self.reader.is_none()
            && self.nb.is_none()
            && matches!(self.doc.doc, Doc::Markdown | Doc::Latex)
    }

    pub(super) fn latex_exportable(&self) -> bool {
        self.buildable() && self.doc.doc == Doc::Markdown
    }

    /// Why the preview has nothing to build, if it has not.
    pub(super) fn unbuildable(&self) -> Option<&'static str> {
        (!self.buildable()).then_some("PDF preview is available for editable Markdown and LaTeX documents.")
    }

    /// Builds the document and shows its pages in the sidebar.
    pub(super) fn preview(&mut self) {
        if !self.buildable() {
            self.message = Some("Only markdown and LaTeX are previewed".into());
            return;
        }
        self.typeset.live = true;
        if !(self.side_open && self.tab == Tab::Preview) {
            self.show_side(Some(Tab::Preview));
            self.focus = Focus::Doc;
        }
        self.build(Purpose::Preview);
    }

    /// The top bar's Preview: opens the preview, building if there is
    /// nothing to show yet, or closes it.
    pub(super) fn toggle_preview(&mut self) {
        if self.side_open && self.tab == Tab::Preview && self.side_width() > 0.0 {
            return self.show_side(None);
        }
        let view = &self.typeset.view;
        let shown = view.pages.is_some() || !view.problems.is_empty() || view.no_engine;
        if self.buildable() && !shown && !self.typeset.busy() {
            return self.preview();
        }
        self.show_side(Some(Tab::Preview));
        self.focus = Focus::Doc;
        self.typeset.live |= self.buildable();
    }

    /// Starts building the document, or asks for another build once the
    /// one under way is over.
    pub(super) fn build(&mut self, purpose: Purpose) {
        if !self.buildable() {
            return;
        }
        let Some(engine) = tex::find(self.cfg.latex_for(&self.root)) else {
            self.typeset.view.no_engine = true;
            self.typeset.waiting = Some(purpose);
            if !(self.side_open && self.tab == Tab::Preview) {
                self.show_side(Some(Tab::Preview));
                self.focus = Focus::Doc;
            }
            self.message = Some(match self.cfg.latex_for(&self.root) {
                "auto" => "No TeX engine is installed. The preview can set one up".to_string(),
                chosen => format!("{chosen} is not installed. Choose another engine in the settings"),
            });
            return;
        };
        self.typeset.view.no_engine = false;
        if self.typeset.build.is_some() {
            self.typeset.again = Some(purpose);
            return;
        }
        self.typeset.markdown_map = None;
        let source = match self.doc.doc {
            Doc::Latex => {
                let Some(path) = self.doc.path.clone() else {
                    self.ask(PromptKind::SaveAs);
                    self.message = Some("Save the file to build it".into());
                    return;
                };
                if self.doc.buf.modified {
                    // Saved for this build, not for one of its own.
                    let live = std::mem::replace(&mut self.typeset.live, false);
                    self.save();
                    self.typeset.live = live;
                    if self.doc.buf.modified {
                        return;
                    }
                }
                path
            }
            _ => {
                let name = self.doc.path.as_deref().and_then(Path::file_stem);
                let name = name.map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
                let dir = workshop(self.doc.path.as_deref(), &self.root);
                let source = dir.join(format!("{name}.tex"));
                let tex = export::latex_mapped(&self.doc.buf.text, self.doc.path.as_deref(), &name);
                if let Some(path) = &self.doc.path {
                    self.typeset.markdown_map = Some(crate::markdown_sync::Map::new(path.clone(), source.clone(), self.doc.buf.text.clone(), &tex));
                }
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&source, tex)) {
                    self.message = Some(format!("Cannot build: {e}"));
                    return;
                }
                source
            }
        };
        let copy_to = match &purpose {
            Purpose::Export(to) => Some(to.clone()),
            Purpose::Preview => None,
        };
        let first = matches!(engine, tex::Engine::Tectonic(_)) && !first_built();
        let view = &mut self.typeset.view;
        view.state = match first {
            true => "Building. A first build fetches the TeX packages it needs".into(),
            false => format!("Building with {}\u{2026}", engine.name()),
        };
        let build = Build::start(engine, source, copy_to, self.waker.clone());
        self.typeset.build = Some((build, purpose, Instant::now()));
    }

    /// Takes in a build or a setup that has moved on.
    pub(super) fn typeset_poll(&mut self) {
        if let Some(setup) = &self.typeset.setup {
            self.typeset.view.state = setup.stage.lock().unwrap().clone();
            if let Some(done) = setup.poll() {
                self.typeset.setup = None;
                self.set_up(done);
            }
        }
        let Some((build, ..)) = &mut self.typeset.build else { return };
        if let Some(outcome) = build.poll(&self.waker) {
            let (_, purpose, start) = self.typeset.build.take().unwrap();
            self.built(outcome, purpose, start);
        }
    }

    /// Waits for builds and setups under way. For tests and headless use.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn typeset_settle(&mut self) {
        while let Some(setup) = self.typeset.setup.take() {
            let done = setup.wait();
            self.set_up(done);
        }
        while let Some((mut build, purpose, start)) = self.typeset.build.take() {
            let outcome = build.wait(&self.waker);
            self.built(outcome, purpose, start);
        }
    }

    fn built(&mut self, out: tex::Outcome, purpose: Purpose, start: Instant) {
        if out.ok {
            if let (Some(map), Some(pdf)) = (self.typeset.markdown_map.take(), out.pdf.as_deref()) {
                if let Err(e) = map.save(pdf) { self.message = Some(format!("Could not save PDF source map: {e}")); }
            }
        } else { self.typeset.markdown_map = None; }
        let latex = self.doc.doc == Doc::Latex;
        self.typeset.problem_file = self.doc.path.clone();
        self.typeset.problem_root = self.root.clone();
        let view = &mut self.typeset.view;
        // An error in the document itself names no file. One in the LaTeX
        // made from markdown is the markdown's, at a line that cannot be
        // gone to.
        let doc = self.doc.path.clone().filter(|_| latex);
        view.problems = out.problems;
        for p in &mut view.problems {
            match &doc {
                Some(doc) if !p.file.is_empty() && doc.ends_with(&p.file) => p.file.clear(),
                Some(_) => {}
                None => (p.file, p.line) = (String::new(), None),
            }
        }
        // What the copy came to is said in the status bar, not the preview.
        view.note = None;
        if out.ok {
            view.pdf = out.pdf.clone();
            // Pages that cannot be read leave the last ones in place.
            match out.pdf.as_deref().map(Pages::open) {
                Some(Ok(pages)) => view.pages = Some(pages),
                Some(Err(why)) => view.note = Some(format!("Cannot show the pages: {why}")),
                None => {}
            }
            let pages = view.pages.as_ref().filter(|_| view.note.is_none()).map_or(0, |p| p.sizes.len());
            let secs = start.elapsed().as_secs_f32();
            view.state = match pages {
                0 => format!("Built in {secs:.1} s"),
                1 => format!("1 page, built in {secs:.1} s"),
                n => format!("{n} pages, built in {secs:.1} s"),
            };
        } else {
            view.state = "The build failed".into();
        }
        let first = view.problems.first().map(|p| match p.line {
            Some(line) => format!("line {line}: {}", p.text),
            None => p.text.clone(),
        });
        let exported = match &purpose {
            Purpose::Export(to) => Some(format!("Exported to {}", tools::short(&self.root, to))),
            Purpose::Preview => None,
        };
        self.message = match (out.ok, exported) {
            (true, Some(said)) => Some(out.note.filter(|n| n.starts_with("Cannot")).unwrap_or(said)),
            (true, None) => None,
            (false, Some(_)) => Some(format!("Cannot export: {}", first.unwrap_or_default())),
            (false, None) => Some(format!("The build failed: {}", first.unwrap_or_default())),
        };
        self.tree.refresh();
        self.redraw = true;
        if let Some(again) = self.typeset.again.take() {
            self.build(again);
        }
    }

    /// Fetches Tectonic, and builds what waited for it once it is there.
    pub(super) fn start_setup(&mut self) {
        if self.typeset.setup.is_none() {
            let setup = Setup::start(self.waker.clone());
            self.typeset.view.state = setup.stage.lock().unwrap().clone();
            self.typeset.setup = Some(setup);
        }
        self.show_side(Some(Tab::Preview));
        self.focus = Focus::Doc;
    }

    fn set_up(&mut self, done: Result<String, String>) {
        match done {
            Ok(version) => {
                self.cfg.latex = "auto".into();
                let _ = self.cfg.save();
                self.maths.borrow_mut().retry();
                self.typeset.view.no_engine = false;
                self.typeset.view.state = format!("{version} is set up");
                self.message = Some(format!("{version} is set up"));
                if let Some(purpose) = self.typeset.waiting.take() {
                    self.build(purpose);
                }
            }
            Err(why) => {
                self.typeset.view.state = "The setup failed".into();
                self.message = Some(why);
            }
        }
    }

    /// A click on a control of the preview.
    pub(super) fn preview_hit(&mut self, hit: Hit) {
        match hit {
            Hit::Build => self.preview(),
            Hit::Setup => self.start_setup(),
            Hit::Open => {
                if let Some(pdf) = self.typeset.view.pdf.clone() {
                    if std::process::Command::new("xdg-open").arg(&pdf).spawn().is_err() {
                        self.message = Some("Cannot run xdg-open".into());
                    }
                }
            }
            Hit::Problem(i) => {
                let Some(p) = self.typeset.view.problems.get(i) else { return };
                if let (Some(line), true) = (p.line, p.file.is_empty()) {
                    let line = line.saturating_sub(1).min(self.doc.buf.lines() - 1);
                    let at = self.doc.buf.starts[line];
                    self.doc.select(at, at);
                    self.doc.moved();
                    self.focus = Focus::Doc;
                } else if p.line.is_some() {
                    self.message = Some(format!("That is in {}", p.file));
                }
            }
        }
    }

    /// Shows in the preview where the line the cursor is on went in the
    /// PDF, by what TeX wrote down of it.
    pub(super) fn sync_forward(&mut self) {
        if !matches!(self.doc.doc, Doc::Latex | Doc::Markdown) {
            self.message = Some("Build a markdown or LaTeX document first".into());
            return;
        }
        let Some(path) = self.doc.path.clone() else { return };
        let pdf = self.typeset.view.pdf.clone().unwrap_or_else(|| tex::pdf_of(&path));
        let Some(sync) = crate::synctex::Sync::open(&pdf) else {
            self.message = Some("Build the document first (F5) to find places in its PDF".into());
            return;
        };
        let mut line = self.doc.buf.line_of(self.doc.buf.cur) + 1;
        let mut path = path;
        if self.doc.doc == Doc::Markdown {
            let Some(map) = crate::markdown_sync::Map::open(&pdf).filter(|m| m.text == self.doc.buf.text) else {
                self.message = Some("Markdown changed since the PDF build. Build again (F5).".into());
                return;
            };
            let Some(mapped) = map.forward(line) else { return };
            line = mapped;
            path = map.generated;
        }
        let Some((page, rect)) = sync.forward(&path, line) else {
            self.message = Some("That line is not in the PDF".into());
            return;
        };
        if !(self.side_open && self.tab == Tab::Preview) {
            self.preview();
        }
        self.typeset.view.show_spot(page, rect);
        self.redraw = true;
    }

    /// Goes to the line of the source that a place in the preview came
    /// from. Returns whether the point was on a page.
    pub(super) fn sync_inverse(&mut self, x: f32, y: f32) -> bool {
        let Some((page, px, py)) = self.typeset.view.page_at(x, y) else { return false };
        let Some(pdf) = self.typeset.view.pdf.clone() else { return false };
        let Some((mut file, mut line)) = crate::synctex::Sync::open(&pdf).and_then(|s| s.inverse(page, px, py)) else {
            self.message = Some("Cannot map that position to the PDF. Build again".into());
            return true;
        };
        let same = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let map = crate::markdown_sync::Map::open(&pdf).filter(|m| same(&m.generated) == same(&file));
        if let Some(map) = &map {
            file = map.source.clone();
            line = map.inverse(line).unwrap_or(1);
        }
        if self.doc.path.as_deref().map(same).as_deref() != Some(file.as_path()) {
            self.pick(file.clone(), false);
        }
        if map.as_ref().is_some_and(|m| m.text != self.doc.buf.text) {
            self.message = Some("Markdown changed since the PDF build. Build again (F5).".into());
            return true;
        }
        if self.doc.path.as_deref().map(same) == Some(file) {
            let li = line.saturating_sub(1).min(self.doc.buf.lines() - 1);
            self.go_to(self.doc.buf.starts[li]);
        }
        true
    }

    /// Another document is in the window: what was shown is not its, and
    /// it is built in its place if the preview is open.
    pub(super) fn typeset_loaded(&mut self) {
        self.typeset.view.clear();
        self.typeset.again = None;
        if self.side_open && self.tab == Tab::Preview && self.typeset.live && self.buildable() {
            self.build(Purpose::Preview);
        }
    }

    /// The document was saved: an open preview follows it.
    pub(super) fn typeset_saved(&mut self) {
        if self.side_open && self.tab == Tab::Preview && self.typeset.live && self.buildable() {
            self.build(Purpose::Preview);
        }
    }

    /// The name an export is offered: the document's own, as a PDF.
    pub(super) fn export_name(&self, ext: &str) -> String {
        let path = self.doc.path.clone().unwrap_or_else(|| self.root.join("Untitled"));
        path.with_extension(ext).to_string_lossy().into_owned()
    }

    /// Exports the document as a PDF, to the file named.
    pub(super) fn export_pdf(&mut self, input: &str) {
        let mut path = self.resolve(input);
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("pdf")) {
            path.as_mut_os_string().push(".pdf");
        }
        if path.is_dir() {
            self.message = Some(format!("{} is a folder", tools::short(&self.root, &path)));
            return;
        }
        self.prompt = None;
        self.message = Some("Exporting\u{2026}".into());
        self.build(Purpose::Export(path));
    }

    /// Writes a markdown document out as LaTeX, to a new file.
    pub(super) fn export_tex(&mut self, input: &str) {
        let path = self.resolve(input);
        if self.taken(&path) {
            return;
        }
        let name = path.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let tex = export::latex(&self.doc.buf.text, self.doc.path.as_deref(), &name);
        match super::files::make_dirs(&path).and_then(|()| std::fs::write(&path, tex)) {
            Ok(()) => {
                self.prompt = None;
                self.tree.refresh();
                self.message = Some(format!("Exported to {}", tools::short(&self.root, &path)));
            }
            Err(e) => self.message = Some(format!("Cannot export: {e}")),
        }
    }
}

/// Whether Tectonic has built anything on this machine yet, which is when
/// it fetches the packages.
fn first_built() -> bool {
    let cache = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache");
    cache.unwrap_or_else(home).join("Tectonic").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Key, Mods};
    use crate::gfx::Canvas;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightdown-typeset-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in for a TeX engine: a script, run in the source's folder,
    /// with the source's name last, as `$f`.
    fn engine(dir: &Path, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-engine");
        std::fs::write(&path, format!("#!/bin/sh\nfor f; do :; done\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn open(path: PathBuf, engine: String) -> App {
        let mut a = App::new(Some(path));
        a.cfg.latex = engine;
        a.resize(900, 600, 1.0);
        a
    }

    fn draw(a: &mut App) {
        let mut px = vec![0u32; 900 * 600];
        a.draw(&mut Canvas::new(&mut px, 900, 600));
    }

    #[test]
    fn latex_is_built_and_its_errors_are_gone_to() {
        let dir = folder("latex");
        let file = dir.join("paper.tex");
        std::fs::write(&file, "\\documentclass{article}\n\\begin{document}\n\\foo\n\\end{document}\n").unwrap();
        let failing = engine(&dir, "echo \"./$f:3: Undefined control sequence.\"; exit 1");
        let mut a = open(file.clone(), failing);

        // F5 saves what is typed, builds, and shows the preview without
        // taking the keyboard from the document.
        a.paste_text("% draft\n");
        a.key(Key::F(5), NONE, None);
        a.settle();
        assert!(!a.doc.buf.modified && a.side_open && a.tab == Tab::Preview && a.focus == Focus::Doc);
        assert_eq!(a.message.as_deref(), Some("The build failed: line 3: Undefined control sequence."));
        let p = &a.typeset.view.problems[0];
        assert_eq!((p.file.as_str(), p.line), ("", Some(3)));
        // A click on the error goes to its line.
        draw(&mut a);
        a.preview_hit(Hit::Problem(0));
        assert_eq!(a.doc.buf.line_of(a.doc.buf.cur), 2);

        // Once it builds, saving builds again while the preview is open.
        a.cfg.latex = engine(&dir, "echo built >> runs; echo pdf > \"${f%.tex}.pdf\"");
        a.key(Key::F(5), NONE, None);
        a.settle();
        assert!(a.typeset.view.problems.is_empty() && a.typeset.view.pdf == Some(dir.join("paper.pdf")));
        assert!(a.typeset.view.state.starts_with("Built") || a.typeset.view.state.contains("page"));
        a.paste_text("x");
        a.key(Key::Char('s'), CTRL, None);
        a.settle();
        assert_eq!(std::fs::read_to_string(dir.join("runs")).unwrap().lines().count(), 2);
        // Closed, the preview is left alone.
        a.toggle_preview();
        a.paste_text("y");
        a.key(Key::Char('s'), CTRL, None);
        a.settle();
        assert!(!a.side_open && std::fs::read_to_string(dir.join("runs")).unwrap().lines().count() == 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_is_exported_as_pdf_and_latex() {
        let dir = folder("markdown");
        let file = dir.join("notes.md");
        std::fs::write(&file, "# Notes\n\nSome *text*.\n").unwrap();
        // The engine is given LaTeX made from the markdown, somewhere else.
        let copying = engine(&dir, &format!("cp \"$f\" {0}/given.tex; echo pdf > \"${{f%.tex}}.pdf\"", dir.display()));
        let mut a = open(file.clone(), copying);
        a.file_action("export_pdf");
        assert_eq!(a.prompt.as_ref().unwrap().text, dir.join("notes.pdf").to_string_lossy());
        a.key(Key::Enter, NONE, None);
        a.settle();
        assert_eq!(a.message.as_deref(), Some("Exported to notes.pdf"));
        assert_eq!(std::fs::read_to_string(dir.join("notes.pdf")).unwrap(), "pdf\n");
        assert!(std::fs::read_to_string(dir.join("given.tex")).unwrap().contains("\\section*{Notes}"));
        assert!(!dir.join("notes.tex").exists() && !a.side_open);

        // As LaTeX it is written beside it, but never over a file.
        a.file_action("export_tex");
        a.key(Key::Enter, NONE, None);
        assert!(std::fs::read_to_string(dir.join("notes.tex")).unwrap().contains("Some \\emph{text}."));
        a.file_action("export_tex");
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.message.as_deref(), Some("notes.tex already exists"));

        // Without an engine, the preview offers to set one up.
        a.prompt = None;
        a.cfg.latex = dir.join("missing-engine").to_string_lossy().into_owned();
        a.key(Key::F(5), NONE, None);
        assert!(a.typeset.view.no_engine && a.side_open && a.tab == Tab::Preview);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
