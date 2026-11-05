use super::*;
impl App {
    pub(super) fn inline_request(&mut self, instruction: &str) {
        self.prompt = None;
        self.unsuggest();
        self.comp = None;
        if self.reader.is_some() || self.doc.read_only || self.nb.is_some() || !self.doc.buf.has_sel() {
            self.message = Some("Select editable source text first".into());
            return;
        }
        let Some(key) = self.cfg.key_for(&self.cfg.suggest_model) else {
            self.message = Some("Configure the suggestion model's API key first".into());
            return;
        };
        if let Some(why) = self.over_limit() {
            self.message = Some(why);
            return;
        }
        let (a, b) = self.doc.buf.sel();
        let original = self.doc.buf.text[a..b].to_string();
        let model = self.cfg.agent_model(&self.ai.catalog, &self.cfg.suggest_model);
        let prompt = "Rewrite the selected text following the instruction. Return only the replacement text, without fences or explanation. Preserve indentation and surrounding code conventions.";
        let user = format!(
            "Instruction: {instruction}\nFile: {}\nBefore:\n{}\nSelected:\n{original}\nAfter:\n{}",
            self.name(),
            &self.doc.buf.text[editor::ceil(&self.doc.buf.text, a.saturating_sub(2000))..a],
            &self.doc.buf.text[b..editor::floor(&self.doc.buf.text, (b + 2000).min(self.doc.buf.text.len()))]
        );
        let req = agent::completion(&model, &key, &self.ai.thread.id, prompt, &user, &self.cfg.suggest_reasoning);
        self.suggest = Some(suggest::Suggesting::inline(
            agent::Stream::start(req, self.waker.clone()),
            model.key,
            (self.file_tabs[self.active_file].id, self.doc.buf.version, a, b, original),
        ));
        self.message = Some("Asking for an inline edit…".into());
    }
    pub(super) fn inline_preview(&mut self, a: usize, b: usize, replacement: &str) {
        let original = self.doc.buf.text[a..b].to_string();
        let selection = (self.doc.buf.anchor, self.doc.buf.cur);
        self.doc.preview_raw(a, b, "");
        self.doc.select(a, a);
        self.doc.show_ghost(replacement);
        self.inline_preview = Some((a, original, selection.0, selection.1));
        self.message = Some("Inline edit: Tab to keep, Esc to discard".into());
    }
    pub(super) fn inline_key(&mut self, key: Key, mods: Mods) -> Option<bool> {
        let (a, old, _, _) = self.inline_preview.as_ref()?.clone();
        let replacement = self.doc.ghost().unwrap_or("").to_string();
        self.unsuggest();
        if key == Key::Tab && !mods.ctrl && !mods.alt && !mods.shift {
            self.doc.edit(a, a + old.len(), &replacement, EditKind::Other);
            self.doc.buf.commit();
            self.doc.moved();
            self.edited();
            self.message = Some("Inline edit kept".into());
            Some(true)
        } else {
            Some(key == Key::Escape)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inline_preview_is_temporary_and_acceptance_is_one_undo() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("before café after");
        a.doc.buf.modified = false;
        a.doc.select(7, 12);
        a.inline_preview(7, 12, "tea");
        assert_eq!(a.doc.buf.text, "before tea after");
        assert!(!a.doc.buf.modified);
        a.key(Key::Escape, Mods::default(), None);
        assert_eq!(a.doc.buf.text, "before café after");
        assert_eq!(a.doc.buf.sel(), (7, 12));
        assert!(!a.doc.buf.modified);
        a.inline_preview(7, 12, "tea");
        a.key(Key::Tab, Mods::default(), None);
        assert_eq!(a.doc.buf.text, "before tea after");
        assert!(a.doc.buf.modified);
        a.key(Key::Char('z'), Mods { ctrl: true, ..Default::default() }, None);
        assert_eq!(a.doc.buf.text, "before café after");
        a.inline_preview(7, 12, "");
        a.key(Key::Tab, Mods::default(), None);
        assert_eq!(a.doc.buf.text, "before  after");
    }
}
