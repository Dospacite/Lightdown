//! Explicit exports run off the input thread and preserve the previous output on failure.
use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::process::{Command, Stdio};

static SERIAL: AtomicU64 = AtomicU64::new(1);
type Finished = Result<String, String>;
#[derive(Default)]
pub(super) struct DocumentExports {
    pub format: String,
    pub(super) pending: Option<Arc<Mutex<Option<Finished>>>>,
    pub(super) rich: Option<Arc<Mutex<Option<Result<(String, String), String>>>>>,
}
fn export(source: &str, document: Option<&Path>, target: &Path, format: &str, engine: &str, pandoc: &Path) -> Finished {
    if document.is_some_and(|doc| doc == target || doc.canonicalize().ok().zip(target.canonicalize().ok()).is_some_and(|(a, b)| a == b)) {
        return Err("Export must not overwrite the source document".into());
    }
    if format == "html" {
        let html = crate::html_export::render(source, document, engine)?;
        write_file(target, html.as_bytes()).map_err(|e| format!("Cannot write HTML: {e}"))?;
    } else {
        let parent = target.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let scratch = parent.join(format!(".lightdown-export-{}-{serial}.{format}", std::process::id()));
        let input = parent.join(format!(".lightdown-export-{}-{serial}.md", std::process::id()));
        let result = (|| {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&input).map_err(|e| e.to_string())?;
            file.write_all(source.as_bytes()).map_err(|e| e.to_string())?;
            // Reserve the output too; pandoc overwrites only this newly created temporary file.
            std::fs::OpenOptions::new().write(true).create_new(true).open(&scratch).map_err(|e| e.to_string())?;
            let resource = document.and_then(Path::parent).unwrap_or(Path::new("."));
            let output = Command::new("timeout").arg("60").arg(pandoc)
                .args(["--from=markdown+tex_math_dollars", "--standalone", "--to"]).arg(format)
                .arg("--resource-path").arg(resource).arg("--output").arg(&scratch).arg(&input)
                .stdin(Stdio::null()).output().map_err(|e| format!("Cannot run pandoc or timeout: {e}"))?;
            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr).chars().take(1000).collect::<String>();
                return Err(format!("pandoc export failed. Install pandoc for DOCX/EPUB. {error}"));
            }
            if std::fs::metadata(&scratch).map_err(|e| e.to_string())?.len() == 0 { return Err("pandoc wrote an empty export".into()); }
            std::fs::rename(&scratch, target).map_err(|e| e.to_string())?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&input); let _ = std::fs::remove_file(&scratch);
        result?;
    }
    Ok(format!("Exported to {}", target.display()))
}
impl App {
    pub(super) fn copy_rich(&mut self) {
        self.unsuggest();
        if self.doc.doc != Doc::Markdown || self.reader.is_some() { self.message = Some("Copy as rich text needs markdown".into()); return; }
        if self.document_exports.rich.is_some() { self.message = Some("Rich text is being prepared".into()); return; }
        let source = self.doc.selected().unwrap_or_else(|| self.doc.buf.text.clone());
        let (document, engine, wake) = (self.doc.path.clone(), self.cfg.latex.clone(), self.waker.clone());
        let done = Arc::new(Mutex::new(None)); self.document_exports.rich = Some(done.clone());
        self.message = Some("Preparing rich text…".into());
        std::thread::spawn(move || {
            let result = crate::html_export::render(&source, document.as_deref(), &engine).map(|html| (source, html));
            *done.lock().unwrap() = Some(result); wake();
        });
    }

    pub(super) fn ask_document_export(&mut self, format: &str) {
        if !self.latex_exportable() { self.message = Some("This export needs editable markdown".into()); return; }
        self.document_exports.format = format.into();
        self.ask(PromptKind::ExportDocument);
    }
    pub(super) fn export_document(&mut self, input: &str) {
        if self.document_exports.pending.is_some() { self.message = Some("A document export is running".into()); return; }
        if !self.latex_exportable() { return; }
        let target = self.target(input);
        let (source, document, engine, format, wake) = (self.doc.buf.text.clone(), self.doc.path.clone(), self.cfg.latex.clone(), self.document_exports.format.clone(), self.waker.clone());
        if !matches!(format.as_str(), "html" | "docx" | "epub") { return; }
        self.prompt = None;
        self.message = Some(format!("Exporting {format}…"));
        let done = Arc::new(Mutex::new(None)); self.document_exports.pending = Some(done.clone());
        std::thread::spawn(move || {
            *done.lock().unwrap() = Some(export(&source, document.as_deref(), &target, &format, &engine, Path::new("pandoc")));
            wake();
        });
    }
    pub(super) fn document_export_poll(&mut self) {
        let rich = self.document_exports.rich.as_ref().and_then(|p| p.lock().unwrap().take());
        if let Some(result) = rich {
            self.document_exports.rich = None;
            match result {
                Ok((plain, html)) => { self.copy = Some(plain); self.copy_html = Some(html); self.message = Some("Copied as rich text".into()); }
                Err(e) => self.message = Some(e),
            }
            self.redraw = true;
        }

        let Some(pending) = &self.document_exports.pending else { return };
        let finished = pending.lock().unwrap().take();
        let Some(result) = finished else { return };
        self.document_exports.pending = None;
        self.message = Some(match result { Ok(message) => message, Err(e) => e });
        self.tree.refresh(); self.redraw = true;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn pandoc_is_optional_atomic_and_accepts_unsaved_source() {
        let dir = std::env::temp_dir().join(format!("lightdown-pandoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("pandoc");
        std::fs::write(&fake, "#!/bin/sh\nwhile [ $# -gt 0 ]; do\nif [ \"$1\" = --output ]; then shift; out=$1; fi\nin=$1; shift\ndone\ncat \"$in\" > \"$out\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = dir.join("out.docx");
        export("unsaved", None, &target, "docx", "", &fake).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unsaved");
        assert!(export("new", None, &target, "docx", "", &dir.join("missing")).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unsaved");
        assert!(export("new", Some(&target), &target, "html", "", &fake).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod rich_tests {
    use super::*;
    #[test]
    fn rich_copy_prepares_both_mimes_from_selection_without_live_clipboard() {
        let mut a = App::new(None);
        a.paste_text("before **bold** after"); a.doc.select(7, 15); a.copy_rich();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while a.document_exports.rich.is_some() && std::time::Instant::now() < until { a.document_export_poll(); std::thread::yield_now(); }
        assert_eq!(a.copy.as_deref(), Some("**bold**"));
        assert!(a.copy_html.as_ref().unwrap().contains("<strong>bold</strong>"));
        assert_eq!(a.doc.buf.text, "before **bold** after");
    }
}
