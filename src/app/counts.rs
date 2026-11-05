//! Counts use Unicode characters, including spaces, and 200 words per minute.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Counts {
    words: usize,
    characters: usize,
}
impl Counts {
    fn of(text: &str) -> Self {
        Self { words: text.split_whitespace().count(), characters: text.chars().count() }
    }
    fn minutes(self) -> usize { self.words.div_ceil(200) }
    fn label(self) -> String {
        format!("{} words · {} chars · {} min read", self.words, self.characters, self.minutes())
    }
}
impl App {
    pub(super) fn count_status(&mut self) -> String {
        let all = *self.words.get_or_insert_with(|| Counts::of(&self.doc.buf.text));
        let (a, b) = self.doc.buf.sel();
        if a == b { return all.label(); }
        let selected = Counts::of(&self.doc.buf.text[a..b]);
        format!("{}/{} words · {} chars · {} min read", selected.words, all.words, selected.characters, selected.minutes())
    }
    pub(super) fn show_counts(&mut self) {
        let count = |text: &str| {
            // Notebook cell dividers are UI, not document text.
            let source = text.lines().filter(|line| !line.starts_with(cells::MARK)).collect::<Vec<_>>().join("\n");
            Counts::of(&source)
        };
        let all = if self.nb.is_some() { count(&self.doc.buf.text) } else { Counts::of(&self.doc.buf.text) };
        let (a, b) = self.doc.buf.sel();
        self.message = Some(if a != b {
            let selected = if self.nb.is_some() { count(&self.doc.buf.text[a..b]) } else { Counts::of(&self.doc.buf.text[a..b]) };
            format!("Selection: {}. Document: {}", selected.label(), all.label())
        } else { format!("Document: {}", all.label()) });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_counts_selection_and_reading_round_up() {
        assert_eq!(Counts::of("café 中文\n"), Counts { words: 2, characters: 8 });
        assert_eq!(Counts::of("").minutes(), 0);
        assert_eq!(Counts::of(&"a ".repeat(201)).minutes(), 2);
        let mut a = App::new(None);
        a.paste_text("one café three");
        assert_eq!(a.count_status(), "3 words · 14 chars · 1 min read");
        a.doc.select(4, 9);
        assert_eq!(a.count_status(), "1/3 words · 4 chars · 1 min read");
        a.show_counts();
        assert!(a.message.unwrap().contains("Document: 3 words"));
    }
}
