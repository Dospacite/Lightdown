use super::*;
impl App {
    pub(super) fn snippet_items(&self, prefix: &str) -> Vec<Candidate> {
        let ext = self.doc.path.as_deref().and_then(Path::extension).and_then(|s| s.to_str()).unwrap_or("txt");
        crate::snippets::load(&self.root, ext)
            .into_iter()
            .filter(|s| !prefix.is_empty() && s.prefix.starts_with(prefix))
            .map(|s| Candidate { name: s.prefix, kind: "snippet".into() })
            .collect()
    }
    pub(super) fn snippet_take(&mut self, start: usize, name: &str) -> bool {
        self.snippet_insert(start, name, None)
    }

    /// Expands a snippet over `start..cursor`. A body that uses
    /// `$CLIPBOARD` first asks for the clipboard, which arrives later.
    fn snippet_insert(&mut self, start: usize, name: &str, clipboard: Option<String>) -> bool {
        let ext = self.doc.path.as_deref().and_then(Path::extension).and_then(|s| s.to_str()).unwrap_or("txt");
        let Some(s) = crate::snippets::load(&self.root, ext).into_iter().find(|s| s.prefix == name) else {
            return false;
        };
        if clipboard.is_none() && s.body.contains("CLIPBOARD") {
            self.snippet_wait = Some(Waiting {
                start,
                name: name.into(),
                path: self.doc.path.clone(),
                version: self.doc.buf.version,
                cur: self.doc.buf.cur,
            });
            self.clipboard_read = true;
            return true;
        }
        let path = self.doc.path.as_deref();
        let (a, b) = self.doc.buf.sel();
        let mut values = crate::snippets::context(&self.root);
        values.extend([
            ("TM_FILENAME", path.and_then(Path::file_name).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            ("TM_FILENAME_BASE", path.and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            ("TM_DIRECTORY", path.and_then(Path::parent).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            ("TM_LINE_NUMBER", (self.doc.buf.line_of(self.doc.buf.cur) + 1).to_string()),
            ("TM_SELECTED_TEXT", self.doc.buf.text[a..b].to_string()),
            ("CLIPBOARD", clipboard.unwrap_or_default()),
        ]);
        let body = crate::snippets::variables(&s.body, &values);
        let expansion = match crate::snippets::expand_full(&body) {
            Ok(v) => v,
            Err(e) => {
                self.message = Some(e);
                return true;
            }
        };
        if self.doc.edit(start, self.doc.buf.cur, &expansion.text, EditKind::Other) {
            self.doc.buf.commit();
            self.doc.start_snippet(start, expansion);
            self.edited();
            self.snippet_offer_choices();
        }
        true
    }

    /// The clipboard's text, asked for by a snippet using `$CLIPBOARD`. The
    /// snippet goes in if nothing moved meanwhile.
    pub fn clipboard_arrived(&mut self, text: String) {
        let Some(w) = self.snippet_wait.take() else { return };
        if w.path == self.doc.path && w.version == self.doc.buf.version && w.cur == self.doc.buf.cur {
            self.snippet_insert(w.start, &w.name, Some(text));
            self.redraw = true;
        }
    }

    /// Lists the choices of a snippet stop when the caret reaches it.
    pub(super) fn snippet_offer_choices(&mut self) {
        let Some((start, choices)) = self.doc.snippet_choices() else { return };
        let items = choices.iter().map(|c| Candidate { name: c.clone(), kind: "choice".into() }).collect();
        self.comp = Some(Completion { query: py::Query { start, prefix: String::new(), base: None }, items, sel: 0, top: 0, seq: 0 });
    }
}

/// A snippet waiting for the clipboard: where it goes, and the document as
/// it was, so a late answer cannot land somewhere else.
pub(super) struct Waiting {
    start: usize,
    name: String,
    path: Option<PathBuf>,
    version: u64,
    cur: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_expands_a_language_template_and_tracks_typed_stops() {
        let root = std::env::temp_dir().join(format!("ld-snippet-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(
            root.join(".lightdown/snippets.json"),
            r#"{"rs":[{"prefix":"fun","body":"fn ${1:name}() {\n    $0\n}"}]}"#,
        )
        .unwrap();
        std::fs::write(root.join("main.rs"), "").unwrap();
        let mut a = App::new(Some(root.join("main.rs")));
        a.resize(900, 700, 1.0);
        a.paste_text("fun");
        a.complete(true);
        assert_eq!(a.comp.as_ref().unwrap().items[0].kind, "snippet");
        a.key(Key::Tab, Mods::default(), None);
        assert_eq!(&a.doc.buf.text[a.doc.buf.sel().0..a.doc.buf.sel().1], "name");
        a.key(Key::Char('x'), Mods::default(), Some("é"));
        a.key(Key::Tab, Mods::default(), None);
        assert_eq!(&a.doc.buf.text[..a.doc.buf.cur], "fn é() {\n    ");
        a.key(Key::Tab, Mods::default(), None);
        assert!(a.doc.snippet.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mirrors_follow_typing_choices_list_and_each_stop_undoes_at_once() {
        let root = std::env::temp_dir().join(format!("ld-snippet-mirror-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(
            root.join(".lightdown/snippets.json"),
            r#"{"rs":[{"prefix":"let","body":"let ${1:x} = ${2|one,two|}; $1 + ${3:$CLIPBOARD}$0"}]}"#,
        )
        .unwrap();
        std::fs::write(root.join("main.rs"), "").unwrap();
        let mut a = App::new(Some(root.join("main.rs")));
        a.resize(900, 700, 1.0);
        a.paste_text("let");
        a.complete(true);
        a.key(Key::Tab, Mods::default(), None);
        // The clipboard is asked for first.
        assert!(std::mem::take(&mut a.clipboard_read));
        assert_eq!(a.doc.buf.text, "let");
        a.clipboard_arrived("clip}".into());
        assert_eq!(a.doc.buf.text, "let x = one; x + clip}");
        for ch in ["a", "b"] {
            a.key(Key::Char(ch.chars().next().unwrap()), Mods::default(), Some(ch));
        }
        assert_eq!(a.doc.buf.text, "let ab = one; ab + clip}");
        assert_eq!(a.doc.buf.sel(), (6, 6));
        // Leaving the stop lists the next one's choices.
        a.key(Key::Tab, Mods::default(), None);
        let names: Vec<_> = a.comp.as_ref().unwrap().items.iter().map(|i| i.name.clone()).collect();
        assert_eq!(names, ["one", "two"]);
        a.key(Key::Down, Mods::default(), None);
        a.key(Key::Enter, Mods::default(), None);
        assert_eq!(a.doc.buf.text, "let ab = two; ab + clip}");
        a.key(Key::Tab, Mods::default(), None);
        assert_eq!(&a.doc.buf.text[a.doc.buf.sel().0..a.doc.buf.sel().1], "clip}");
        // Undo takes back the choice, then all the typing in the first
        // stop with its mirror, then the snippet.
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        a.key(Key::Escape, Mods::default(), None);
        a.key(Key::Char('z'), ctrl, None);
        assert_eq!(a.doc.buf.text, "let ab = one; ab + clip}");
        a.key(Key::Char('z'), ctrl, None);
        assert_eq!(a.doc.buf.text, "let x = one; x + clip}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
