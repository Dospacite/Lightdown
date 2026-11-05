//! Replacing what Find finds: the match selected, or every match at once.

use super::{App, Mods};
use crate::editor::{self, EditKind};

/// Where `needle` occurs in `text`, without overlaps. Case matters only if
/// the needle has capitals, as in Find.
#[cfg(test)]
pub(super) fn matches(text: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let fold = !needle.chars().any(char::is_uppercase);
    let (hay, needle) = if fold {
        (text.to_ascii_lowercase(), needle.to_ascii_lowercase())
    } else {
        (text.to_string(), needle.to_string())
    };
    hay.match_indices(&needle).map(|(i, m)| (i, i + m.len())).collect()
}

#[derive(Clone, Default, PartialEq, Debug)]
pub(super) struct Options {
    pub regex: bool,
    pub word: bool,
    pub case: bool,
    pub preserve_case: bool,
    pub include: String,
    pub exclude: String,
}
impl Options {
    pub fn pattern(&self, needle: &str) -> Result<regex::Regex, String> {
        let pattern = if self.regex { needle.to_string() } else { regex::escape(needle) };
        let pattern = if self.word { format!(r"\b(?:{pattern})\b") } else { pattern };
        regex::RegexBuilder::new(&pattern)
            .case_insensitive(!self.case && !needle.chars().any(char::is_uppercase))
            .multi_line(true)
            .size_limit(4 << 20)
            .build()
            .map_err(|e| e.to_string())
    }
    pub fn matches(&self, text: &str, needle: &str) -> Result<Vec<(usize, usize)>, String> {
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.pattern(needle)?.find_iter(text).map(|m| (m.start(), m.end())).collect())
    }
    pub fn replacement(&self, text: &str, needle: &str, with: &str) -> Result<String, String> {
        let re = self.pattern(needle)?;
        Ok(re.replace_all(text, |caps: &regex::Captures<'_>| self.expand(caps, with)).into_owned())
    }

    /// What one match becomes: captures expanded for a regex, and the case
    /// of the match kept when asked.
    pub fn expand(&self, caps: &regex::Captures<'_>, with: &str) -> String {
        let mut replacement = String::new();
        if self.regex { caps.expand(with, &mut replacement); }
        else { replacement.push_str(with); }
        if self.preserve_case { preserve_case(&caps[0], &replacement) } else { replacement }
    }

    pub fn accepts(&self, path: &str) -> bool {
        let matches = |patterns: &str| {
            patterns.split(',').map(str::trim).filter(|s| !s.is_empty()).any(|p| super::saving::glob(p, path))
        };
        (self.include.trim().is_empty() || matches(&self.include)) && !matches(&self.exclude)
    }
}

fn preserve_case(source: &str, replacement: &str) -> String {
    let letters: Vec<_> = source.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() { return replacement.into(); }
    if letters.iter().all(|c| c.is_uppercase()) { return replacement.to_uppercase(); }
    if letters.iter().all(|c| c.is_lowercase()) { return replacement.to_lowercase(); }
    if letters[0].is_uppercase() && letters[1..].iter().all(|c| c.is_lowercase()) {
        let lower = replacement.to_lowercase();
        let mut chars = lower.chars();
        if let Some(first) = chars.next() { return first.to_uppercase().collect::<String>() + chars.as_str(); }
    }
    replacement.into()
}

impl App {
    /// Enter in the Replace prompt: with Ctrl or Alt every match is
    /// replaced; else the match selected is, and the next one selected.
    /// Shift goes back to the previous match without replacing.
    pub(super) fn replace_key(&mut self, mods: Mods) {
        let Some(p) = &self.prompt else { return };
        let (needle, with) = (p.text.clone(), p.with.clone());
        if needle.is_empty() {
            return;
        }
        if mods.shift {
            let a = self.doc.buf.sel().0;
            return self.find(a, false);
        }
        if mods.ctrl || mods.alt {
            return self.replace_all(&needle, &with);
        }
        let sel = self.doc.buf.sel();
        if self.scoped_matches(&needle).unwrap_or_default().contains(&sel) {
            let with = if self.search_options.regex {
                let re = self.search_options.pattern(&needle).unwrap();
                let Some(cap) = re
                    .captures_iter(&self.doc.buf.text)
                    .find(|c| c.get(0).is_some_and(|m| (m.start(), m.end()) == sel))
                else {
                    return;
                };
                let mut out = String::new();
                cap.expand(&with, &mut out);
                out
            } else {
                with
            };
            let with = if self.search_options.preserve_case {
                preserve_case(&self.doc.buf.text[sel.0..sel.1], &with)
            } else { with };
            if !self.doc.edit(sel.0, sel.1, &with, EditKind::Other) {
                return;
            }
            self.doc.buf.commit();
            self.edited();
            let at = sel.0 + with.len();
            self.doc.select(at, at);
            self.find(at, true);
        } else {
            self.find(editor::next(&self.doc.buf.text, sel.0).min(self.doc.buf.text.len()), true);
        }
    }

    /// Replaces every match, as one step to undo.
    fn replace_all(&mut self, needle: &str, with: &str) {
        let text = &self.doc.buf.text;
        let found = match self.scoped_matches(needle) {
            Ok(v) => v,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        if found.is_empty() {
            self.message = Some("No match".into());
            return;
        }
        let (from, to) = self.doc.find_scope.unwrap_or((0, text.len()));
        let to = to.min(text.len());
        if from > to || !text.is_char_boundary(from) || !text.is_char_boundary(to) {
            self.message = Some("Selection changed; toggle find in selection again".into()); return;
        }
        let out = match self.search_options.replacement(&text[from..to], needle, with) {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        if self.doc.find_scope.is_some() { self.doc.find_scope = Some((from, from + out.len())); }
        if !self.doc.edit(from, to, &out, EditKind::Other) {
            return;
        }
        self.doc.buf.commit();
        self.edited();
        self.doc.moved();
        let n = found.len();
        self.message = Some(format!("Replaced {n} match{}", if n == 1 { "" } else { "es" }));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Key, PromptKind};
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    fn typed(a: &mut App, s: &str) {
        for ch in s.chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
    }

    #[test]
    fn replacement_preserves_case_and_expands_captures() {
        let mut options = Options { preserve_case: true, ..Options::default() };
        assert_eq!(options.replacement("foo Foo FOO fOo", "foo", "bar").unwrap(), "bar Bar BAR bar");
        options.regex = true;
        assert_eq!(options.replacement("FOO", "(foo)", "$1-bar").unwrap(), "FOO-BAR");
        assert_eq!(preserve_case("Écho", "été"), "Été");
    }
    #[test]
    fn matches_fold_case_unless_asked() {
        assert_eq!(matches("Cat cat CAT", "cat"), [(0, 3), (4, 7), (8, 11)]);
        assert_eq!(matches("Cat cat CAT", "Cat"), [(0, 3)]);
        assert_eq!(matches("aaaa", "aa"), [(0, 2), (2, 4)]);
        assert!(matches("abc", "").is_empty());
    }

    #[test]
    fn one_match_then_all() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("one fish two fish red fish");
        a.doc.select(0, 0);
        a.key(Key::Char('h'), CTRL, None);
        assert!(a.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::Replace));
        typed(&mut a, "fish");
        a.key(Key::Tab, NONE, None);
        typed(&mut a, "cat");
        // The first Enter replaces the match Find selected, and goes on.
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.doc.buf.text, "one cat two fish red fish");
        assert_eq!(a.doc.buf.sel(), (12, 16));
        // Ctrl+Enter replaces the rest, and one undo takes them back.
        a.key(Key::Enter, CTRL, None);
        assert_eq!(a.doc.buf.text, "one cat two cat red cat");
        assert_eq!(a.message.as_deref(), Some("Replaced 2 matches"));
        a.key(Key::Escape, NONE, None);
        a.key(Key::Char('z'), CTRL, None);
        assert_eq!(a.doc.buf.text, "one cat two fish red fish");
    }
}
