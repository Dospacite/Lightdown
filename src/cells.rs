//! Documents made of cells: notebooks and chat transcripts. A cell starts at
//! a header line, which holds a control character, the cell's tag and an
//! optional label. Header lines are never shown as text; the layout draws a
//! divider in their place and the cursor steps over them.

pub const MARK: char = '\u{1e}';

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tag {
    Code,
    Markdown,
    Raw,
    User,
    Assistant,
    Think,
    Tool,
    Note,
}

const NAMES: [(Tag, &str); 8] = [
    (Tag::Code, "code"),
    (Tag::Markdown, "markdown"),
    (Tag::Raw, "raw"),
    (Tag::User, "user"),
    (Tag::Assistant, "assistant"),
    (Tag::Think, "think"),
    (Tag::Tool, "tool"),
    (Tag::Note, "note"),
];

impl Tag {
    pub fn name(self) -> &'static str {
        NAMES.iter().find(|n| n.0 == self).map_or("", |n| n.1)
    }

    /// Whether the cell's body is rendered as markdown.
    pub fn prose(self) -> bool {
        matches!(self, Tag::Markdown | Tag::User | Tag::Assistant)
    }
}

/// The tag and label of a header line, or `None` for any other line.
pub fn parse(line: &str) -> Option<(Tag, &str)> {
    let rest = line.strip_prefix(MARK)?;
    let (name, label) = rest.split_once(' ').unwrap_or((rest, ""));
    let tag = NAMES.iter().find(|n| n.1 == name)?.0;
    (!label.contains(MARK)).then_some((tag, label))
}

pub fn header(tag: Tag, label: &str) -> String {
    let space = if label.is_empty() { "" } else { " " };
    format!("{MARK}{}{space}{label}", tag.name())
}

pub struct Cell {
    /// The cell's first line: its header, if it has one.
    pub line: usize,
    pub tag: Tag,
    pub headed: bool,
    /// Stays with the cell across edits, so outputs can be kept by it.
    pub id: u64,
}

/// Whether every line of `text` is either free of marks or a whole header.
pub fn well_formed(text: &str) -> bool {
    text.split('\n').all(|l| !l.contains(MARK) || parse(l).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_round_trip() {
        assert_eq!(parse(&header(Tag::Code, "")), Some((Tag::Code, "")));
        assert_eq!(parse(&header(Tag::Assistant, "kimi k3")), Some((Tag::Assistant, "kimi k3")));
        assert_eq!(parse("\u{1e}codex"), None);
        assert_eq!(parse("code"), None);
        assert!(well_formed("a\n\u{1e}markdown\nb") && !well_formed("a\u{1e}code"));
    }
}
