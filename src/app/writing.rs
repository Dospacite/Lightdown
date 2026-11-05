//! Document clipboard operations. Asset files are never overwritten.
use super::*;
use std::io::{Read, Write};

const IMAGE_BYTES: usize = 32 * 1024 * 1024;

fn save_asset(document: &Path, bytes: &[u8], extension: &str) -> Result<String, String> {
    let folder = document.parent().ok_or("Save the markdown document before inserting an image")?.join("assets");
    std::fs::create_dir_all(&folder).map_err(|e| format!("Cannot create assets folder: {e}"))?;
    for n in 1..=100_000 {
        let name = format!("image-{n}.{extension}");
        let path = folder.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes) {
                    let _ = std::fs::remove_file(&path);
                    return Err(format!("Cannot save image: {e}"));
                }
                return Ok(format!("![](assets/{name})"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Cannot save image: {e}")),
        }
    }
    Err("Too many images in assets folder".into())
}

fn png_valid(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > IMAGE_BYTES {
        return Err("Image exceeds 32 MB".into());
    }
    let mut decoder = zune_png::PngDecoder::new(std::io::Cursor::new(bytes));
    decoder.decode_headers().map_err(|_| "Clipboard does not contain a valid PNG".to_string())?;
    let (w, h) = decoder.dimensions().ok_or("PNG has no dimensions")?;
    if w == 0 || h == 0 || w > crate::pics::MAX_SIDE as usize || h > crate::pics::MAX_SIDE as usize || w * h > 32_000_000 {
        return Err("Image exceeds 32 million pixels or 20,000 pixels per side".into());
    }
    decoder.decode().map_err(|_| "Clipboard does not contain a valid PNG".to_string())?;
    Ok(())
}

/// Only a single explicit web/mail address changes a selection into a link.
pub(super) fn selection_link(doc: Doc, text: &str, url: &str) -> Option<String> {
    if text.is_empty() || url.chars().any(|c| c.is_whitespace() || c.is_control()) { return None; }
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ftp://")).or_else(|| url.strip_prefix("mailto:"))?;
    if rest.is_empty() || rest.starts_with('/') { return None; }
    match doc {
        Doc::Markdown => {
            let label = text.replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]");
            let url = url.replace('<', "%3C").replace('>', "%3E").replace('(', "%28").replace(')', "%29").replace('\\', "%5C");
            Some(format!("[{label}]({url})"))
        }
        Doc::Latex => {
            let url = url.replace('\\', "%5C").replace('{', "%7B").replace('}', "%7D")
                .replace('%', r"\%").replace('#', r"\#").replace('&', r"\&").replace('_', r"\_");
            Some(format!(r"\href{{{url}}}{{{text}}}"))
        }
        _ => None,
    }
}

impl App {
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn writing_settle(&mut self) {
        self.diagrams_tick(false);
        self.links_tick(false);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < until && (self.diagrams.pending.is_some() || self.links.pending.is_some()
            || self.notes.pending.is_some() || self.document_exports.pending.is_some() || self.document_exports.rich.is_some()) {
            self.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    pub fn paste_csv_text(&mut self, s: &str) {
        if !self.accepts_markdown_paste() { self.message = Some("CSV paste needs editable markdown".into()); return; }
        match crate::paste::csv(s) {
            Ok(markdown) => self.paste_plain_text(&markdown),
            Err(e) => { self.message = Some(e); self.redraw = true; }
        }
    }
    pub fn paste_plain_text(&mut self, s: &str) { self.paste_document_text(s, true); }
    pub fn paste_html(&mut self, html: &str) {
        if !self.accepts_markdown_paste() { return; }
        match crate::paste::html(html) {
            Ok(markdown) => self.paste_plain_text(&markdown),
            Err(e) => { self.message = Some(e); self.redraw = true; }
        }
    }

    pub(super) fn accepts_markdown_drop(&self) -> bool {
        self.doc.doc == Doc::Markdown && !self.doc.read_only && self.reader.is_none()
            && !self.overlaid() && !self.prompt_has_keys()
    }
    pub fn accepts_markdown_paste(&self) -> bool {
        self.focus == Focus::Doc && self.accepts_markdown_drop()
    }
    pub fn paste_image(&mut self, bytes: &[u8]) {
        if self.accepts_screenshot() {
            self.paste_png(bytes);
        } else if self.accepts_markdown_paste() {
            let result = self.doc.path.as_deref().ok_or_else(|| "Save the markdown document before inserting an image".into())
                .and_then(|path| { png_valid(bytes)?; save_asset(path, bytes, "png") });
            match result {
                Ok(markdown) => self.paste_text(&markdown),
                Err(e) => self.message = Some(e),
            }
            self.redraw = true;
        }
    }
    pub(super) fn drop_markdown_images(&mut self, paths: &[PathBuf]) {
        let Some(document) = self.doc.path.clone() else {
            self.message = Some("Save the markdown document before inserting an image".into());
            return;
        };
        let mut inserted = Vec::new();
        for path in paths {
            let result = (|| {
                let file = std::fs::File::open(path).map_err(|e| format!("Cannot read image: {e}"))?;
                let mut bytes = Vec::new();
                file.take(IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > IMAGE_BYTES { return Err("Image exceeds 32 MB".into()); }
                if crate::pics::probe(path).is_none() { return Err("Cannot read this picture".into()); }
                save_asset(&document, &bytes, &ext(path))
            })();
            match result {
                Ok(markdown) => inserted.push(markdown),
                Err(e) => { self.message = Some(e); break; }
            }
        }
        if !inserted.is_empty() { self.paste_text(&inserted.join("\n")); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_assets_preserve_bytes_and_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("lightdown-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let document = dir.join("notes.md");
        let png = crate::pics::tests::png(1, 1, &[255, 0, 0, 255]);
        png_valid(&png).unwrap();
        assert!(png_valid(b"not png").is_err());
        assert_eq!(save_asset(&document, &png, "png").unwrap(), "![](assets/image-1.png)");
        assert_eq!(save_asset(&document, &png, "png").unwrap(), "![](assets/image-2.png)");
        assert_eq!(std::fs::read(dir.join("assets/image-1.png")).unwrap(), png);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn image_paste_requires_saved_editable_markdown_and_is_undoable() {
        let dir = std::env::temp_dir().join(format!("lightdown-paste-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let png = crate::pics::tests::png(1, 1, &[0, 0, 0, 255]);
        let mut app = App::new(None);
        app.paste_image(&png);
        assert!(app.doc.buf.text.is_empty());
        assert!(app.message.as_ref().unwrap().contains("Save"));
        app.doc.path = Some(dir.join("note.md"));
        app.paste_image(&png);
        assert_eq!(app.doc.buf.text, "![](assets/image-1.png)");
        app.doc.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert!(app.doc.buf.text.is_empty());
        app.doc.read_only = true;
        assert!(!app.accepts_markdown_paste());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;
    #[test]
    fn selected_urls_escape_delimiters_and_preserve_unicode() {
        assert_eq!(selection_link(Doc::Markdown, "café [x]", "https://example.org/a(b)"), Some(r"[café \[x\]](https://example.org/a%28b%29)".into()));
        assert_eq!(selection_link(Doc::Latex, "café", "https://e.org/a_b?q=1&b=2#top"), Some(r"\href{https://e.org/a\_b?q=1\&b=2\#top}{café}".into()));
        for url in ["https://", "words", "https://e.org two", "javascript:alert(1)", "https:///no-host"] {
            assert!(selection_link(Doc::Markdown, "text", url).is_none());
        }
        assert!(selection_link(Doc::Code, "text", "https://e.org").is_none());
    }
    #[test]
    fn url_paste_is_one_undo_step() {
        let mut app = App::new(None);
        app.paste_text("hello");
        app.doc.select(0, 5);
        app.paste_text("https://example.org");
        assert_eq!(app.doc.buf.text, "[hello](https://example.org)");
        app.doc.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(app.doc.buf.text, "hello");
    }
}

#[cfg(test)]
mod html_tests {
    use super::*;
    #[test]
    fn plain_paste_bypasses_link_conversion_and_html_is_one_edit() {
        let mut a = App::new(None);
        a.paste_text("selected");
        a.doc.select(0, 8);
        a.paste_plain_text("https://example.org");
        assert_eq!(a.doc.buf.text, "https://example.org");
        a.doc.select(0, a.doc.buf.text.len());
        a.paste_html("<h1>Hello</h1><p><em>world</em></p>");
        assert_eq!(a.doc.buf.text, "# Hello\n\n*world*");
        a.doc.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(a.doc.buf.text, "https://example.org");
    }
}
