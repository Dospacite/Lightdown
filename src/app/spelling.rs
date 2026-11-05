//! Misspelt words marked in the document, and put right from its menu.

use super::App;
use crate::editor::EditKind;
use crate::spell::{self, Speller};
use crate::ui::Item;

impl App {
    /// Marks the misspelt words in view, asking about those not yet known.
    pub(super) fn spell_marks(&mut self) {
        if !self.cfg.spell || self.reader.is_some() {
            self.doc.misspelled.clear();
            return;
        }
        let dict = spell::dictionary(self.cfg.dictionary_for(&self.root));
        if self.speller.as_ref().is_none_or(|s| s.dict != dict) {
            self.speller = Some(Speller::new(&dict, self.waker.clone()));
        }
        let speller = self.speller.as_mut().unwrap();
        if speller.failed() {
            self.doc.misspelled.clear();
            return;
        }
        let words = self.doc.spell_words();
        let text = &self.doc.buf.text;
        let mut marks: Vec<(usize, usize)> =
            words.into_iter().filter(|&(a, b)| matches!(speller.check(&text[a..b]), Some(Some(_)))).collect();
        marks.sort_unstable();
        self.doc.misspelled = marks;
    }

    /// Takes in what hunspell has said.
    pub(super) fn spell_poll(&mut self) {
        if self.speller.as_mut().is_some_and(Speller::poll) {
            self.redraw = true;
        }
    }

    /// The misspelt word the cursor is in, if any.
    fn misspelt_here(&self) -> Option<(usize, usize)> {
        let cur = self.doc.buf.cur;
        self.doc.misspelled.iter().copied().find(|&(a, b)| a <= cur && cur <= b)
    }

    /// Rows for the menu of a misspelt word: what it may have been meant
    /// to be, and taking it as right.
    pub(super) fn spell_items(&mut self) -> Vec<Item> {
        let Some((a, b)) = self.misspelt_here().filter(|_| !self.doc.read_only) else { return Vec::new() };
        let Some(word) = self.doc.buf.text.get(a..b).map(str::to_string) else { return Vec::new() };
        let mut items: Vec<Item> = Vec::new();
        let suggestions = self.speller.as_ref().map(|s| s.suggestions(&word)).unwrap_or_default();
        for (i, s) in suggestions.iter().take(6).enumerate() {
            items.push(Item::new(&format!("spell_{i}"), s, ""));
        }
        if suggestions.is_empty() {
            items.push(Item::new("spell_none", "No suggestions", "").when(false));
        }
        items.push(Item::new("spell_add", &format!("Add \"{word}\" to the dictionary"), ""));
        items.push(Item::Rule);
        self.spell_fix = Some((a, b, suggestions));
        items
    }

    /// Does what a spelling row of the menu says.
    pub(super) fn spell_pick(&mut self, key: &str) {
        let Some((a, b, suggestions)) = self.spell_fix.take() else { return };
        let Some(word) = self.doc.buf.text.get(a..b).map(str::to_string) else { return };
        if key == "spell_add" {
            if let Some(s) = &mut self.speller {
                s.add(&word);
            }
            self.doc.misspelled.retain(|m| self.doc.buf.text.get(m.0..m.1) != Some(word.as_str()));
        } else if let Some(fix) =
            key.strip_prefix("spell_").and_then(|i| i.parse::<usize>().ok()).and_then(|i| suggestions.get(i))
            && self.doc.edit(a, b, fix, EditKind::Other)
        {
            self.doc.buf.commit();
            self.edited();
            self.doc.moved();
        }
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::context::Target;
    use super::*;
    use crate::gfx::Canvas;

    #[test]
    fn misspelt_words_are_marked_and_put_right() {
        let dir = std::env::temp_dir().join(format!("ld-spelling-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tiny.aff"), "SET UTF-8\nTRY esianrtolcdugmphbyfvkwz\n").unwrap();
        std::fs::write(dir.join("tiny.dic"), "4\nhello\nworld\nhelp\nsay\n").unwrap();
        let mut a = App::new(None);
        a.cfg.spell_dict = dir.join("tiny").to_string_lossy().into_owned();
        a.resize(900, 700, 1.0);
        a.paste_text("say helo world `helo`\n\n```\nhelo\n```\n");
        a.doc.select(0, 0);
        let mut px = vec![0u32; 900 * 700];
        a.draw(&mut Canvas::new(&mut px, 900, 700));
        a.speller.as_mut().unwrap().settle();
        if a.speller.as_ref().unwrap().failed() {
            return;
        }
        a.draw(&mut Canvas::new(&mut px, 900, 700));
        // Only the word in prose is marked, not those in code.
        assert_eq!(a.doc.misspelled, [(4, 8)]);
        a.doc.select(5, 5);
        let items = a.spell_items();
        assert!(!items.is_empty());
        let (_, _, suggestions) = a.spell_fix.clone().unwrap();
        let i = suggestions.iter().position(|s| s == "hello").unwrap();
        a.context_pick(Target::Doc { link: None }, &format!("spell_{i}"));
        assert!(a.doc.buf.text.starts_with("say hello world"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
