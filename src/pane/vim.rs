//! Vim mode: modal keys in front of the editor's own key handling. Keys are
//! parsed into commands, and commands carried out with the pane's own
//! edits, indenting, commenting, undo and multiple selections. Insert mode
//! hands every key back to the editor, so typing there is unchanged.
use super::*;
use std::collections::HashMap;
mod extensions;
mod helix;

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) enum Mode {
    #[default]
    Normal,
    Insert,
    Replace,
    Visual,
    Line,
    Block,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    /// `w` and `W`, by whether it is the big kind.
    WordNext(bool),
    WordBack(bool),
    WordEnd(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    FirstLine,
    LastLine,
    Find { ch: char, forward: bool, till: bool },
    /// `;`, or `,` when reversed.
    RepeatFind(bool),
    Bracket,
    ParagraphNext,
    ParagraphBack,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    /// A mark, by its letter, and whether to its line only.
    Mark(char, bool),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Lower,
    Upper,
    Toggle,
    Comment,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Target {
    Motion(Motion),
    /// A text object: around or inside, and its character.
    Object(bool, char),
    /// The doubled operator, such as `dd`: whole lines.
    Lines,
    /// The visual selection, or with `true` its whole lines.
    Selection(bool),
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Action {
    Move(Motion),
    Operate(Op, Target),
    /// Visual mode selects a text object.
    Select(bool, char),
    /// Any other command, by its keys after the register and count.
    Other(String),
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Command {
    pub register: Option<char>,
    pub count: Option<usize>,
    pub action: Action,
}

#[derive(PartialEq, Debug)]
pub(crate) enum Parse {
    Pending,
    Invalid,
    Done(Command),
}

/// What the window does for Vim: things beyond the pane.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Request {
    /// Opens the find to search, backward with `true`.
    Search(bool),
    /// Searches for the word at the cursor, backward with `true`.
    Word(String, bool),
    /// `n`, or `N` with `true`.
    Next(bool),
    /// An Ex command the window carries out: `w`, `q`, `e file`...
    Ex(String),
    /// Pastes the system clipboard at the cursor.
    Paste,
    Command(&'static str),
}

/// A key as it arrived, kept to repeat a change with `.`.
type Stroke = (Key, Mods, Option<String>);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Default)]
pub(crate) struct Vim {
    pub enabled: bool,
    helix: helix::Helix,
    macro_register: Option<char>,
    macro_text: String,
    macro_depth: usize,
    macro_steps: usize,
    last_macro: Option<char>,
    last_visual: Option<(Mode, usize, usize)>,
    visual_change: Option<(Mode, usize, usize)>,
    changes: Vec<usize>,
    change_index: usize,
    mode: Mode,
    /// Keys of a command not yet complete, and the strokes that typed them.
    pending: String,
    strokes: Vec<Stroke>,
    /// The Ex command line being typed, after its colon, with the lines a
    /// visual selection gave it.
    command: Option<(String, Option<(usize, usize)>)>,
    registers: HashMap<char, (String, bool)>,
    /// The strokes of the last change, for `.`, and of the one being made.
    last_change: Vec<Stroke>,
    recording: Option<Vec<Stroke>>,
    /// What was typed since insert mode began, and how many times to insert it.
    inserted: Vec<Stroke>,
    insert_count: usize,
    opened_line: bool,
    replaying: bool,
    /// Characters overwritten in replace mode, so Backspace can restore them.
    replaced: Vec<Option<char>>,
    last_find: Option<(char, bool, bool)>,
    marks: HashMap<char, (usize, usize)>,
    /// Column to keep moving up and down; `usize::MAX` after `$`.
    want: Option<usize>,
    /// The visual selection's anchor and cursor, and what was shown for them.
    anchor: usize,
    cursor: usize,
    shown: (usize, usize),
    /// The undo history before an insert, so it ends as one step.
    group: Option<editor::Checkpoint>,
    pub search_back: bool,
    /// A search typed into the find, backward with `true`.
    pub searching: Option<bool>,
    pub request: Option<Request>,
}

// ---- parsing ----------------------------------------------------------

enum Part<T> {
    Pending,
    No,
    Yes(T),
}

fn counted(s: &str) -> (Option<usize>, &str) {
    let digits = if s.starts_with('0') { 0 } else { s.bytes().take_while(u8::is_ascii_digit).count() };
    (s[..digits].parse::<usize>().ok().map(|n| n.min(99_999)), &s[digits..])
}

fn motion_of(s: &str) -> Part<Motion> {
    use Motion::*;
    let mut it = s.chars();
    let Some(c) = it.next() else { return Part::Pending };
    let rest = it.as_str();
    let simple = match c {
        'h' => Some(Left),
        'l' => Some(Right),
        'j' => Some(Down),
        'k' => Some(Up),
        'w' | 'W' => Some(WordNext(c == 'W')),
        'b' | 'B' => Some(WordBack(c == 'B')),
        'e' | 'E' => Some(WordEnd(c == 'E')),
        '0' => Some(LineStart),
        '^' => Some(FirstNonBlank),
        '$' => Some(LineEnd),
        'G' => Some(LastLine),
        '%' => Some(Bracket),
        '}' => Some(ParagraphNext),
        '{' => Some(ParagraphBack),
        'H' => Some(ScreenTop),
        'M' => Some(ScreenMiddle),
        'L' => Some(ScreenBottom),
        ';' | ',' => Some(RepeatFind(c == ',')),
        _ => None,
    };
    if let Some(m) = simple {
        return if rest.is_empty() { Part::Yes(m) } else { Part::No };
    }
    let mut arg = rest.chars();
    match (c, arg.next(), arg.next()) {
        ('f' | 't' | 'F' | 'T' | '`' | '\'' | 'g', None, _) => Part::Pending,
        ('f' | 't' | 'F' | 'T', Some(ch), None) => {
            Part::Yes(Find { ch, forward: c.is_lowercase(), till: c.to_ascii_lowercase() == 't' })
        }
        ('`' | '\'', Some(m), None) => Part::Yes(Mark(m, c == '\'')),
        ('g', Some('g'), None) => Part::Yes(FirstLine),
        _ => Part::No,
    }
}

fn operator_of(s: &str) -> Part<(Op, usize)> {
    let mut it = s.chars();
    let op = match it.next() {
        Some('d') => Op::Delete,
        Some('c') => Op::Change,
        Some('y') => Op::Yank,
        Some('>') => Op::Indent,
        Some('<') => Op::Outdent,
        Some('g') => {
            return match it.next() {
                None => Part::Pending,
                Some('u') => Part::Yes((Op::Lower, 2)),
                Some('U') => Part::Yes((Op::Upper, 2)),
                Some('~') => Part::Yes((Op::Toggle, 2)),
                Some('c') => Part::Yes((Op::Comment, 2)),
                _ => Part::No,
            };
        }
        _ => return Part::No,
    };
    Part::Yes((op, 1))
}

const OBJECTS: &str = "fcaCwWp\"'`()b{}B[]<>";

/// Reads the keys typed in normal or visual mode as one command.
pub(crate) fn parse(keys: &str, visual: bool) -> Parse {
    let mut s = keys;
    let mut register = None;
    if let Some(rest) = s.strip_prefix('"') {
        let mut it = rest.chars();
        let Some(r) = it.next() else { return Parse::Pending };
        if !(r.is_ascii_alphanumeric() || "\"+*_-".contains(r)) {
            return Parse::Invalid;
        }
        register = Some(r);
        s = it.as_str();
    }
    let (count, s) = counted(s);
    if s.is_empty() {
        return Parse::Pending;
    }
    let done = |count: Option<usize>, action| Parse::Done(Command { register, count, action });
    let arg = |s: &str| s.chars().nth(1);
    if visual {
        let selection = |op, lines| done(count, Action::Operate(op, Target::Selection(lines)));
        if let Some(kind) = s.strip_prefix(['i', 'a']) {
            return match kind.chars().next() {
                None => Parse::Pending,
                Some(c) if kind.len() == c.len_utf8() && OBJECTS.contains(c) => {
                    done(count, Action::Select(s.starts_with('a'), c))
                }
                _ => Parse::Invalid,
            };
        }
        match motion_of(s) {
            Part::Yes(m) => return done(count, Action::Move(m)),
            Part::Pending if s != "g" => return Parse::Pending,
            _ => {}
        }
        return match s {
            "d" | "x" => selection(Op::Delete, false),
            "D" | "X" => selection(Op::Delete, true),
            "c" | "s" => selection(Op::Change, false),
            "C" | "S" | "R" => selection(Op::Change, true),
            "y" => selection(Op::Yank, false),
            "Y" => selection(Op::Yank, true),
            ">" => selection(Op::Indent, true),
            "<" => selection(Op::Outdent, true),
            "~" | "g~" => selection(Op::Toggle, false),
            "u" | "gu" => selection(Op::Lower, false),
            "U" | "gU" => selection(Op::Upper, false),
            "gc" => selection(Op::Comment, true),
            "g" | "r" | "m" | "q" | "@" | "z" => Parse::Pending,
            "J" | "gJ" | "p" | "P" | "o" | "O" | "I" | "A" | "v" | "V" | ":" | "*" | "#" | "n" | "N" => {
                done(count, Action::Other(s.into()))
            }
            _ if s.starts_with('r') && s.chars().count() == 2 => done(count, Action::Other(s.into())),
            _ => Parse::Invalid,
        };
    }
    match operator_of(s) {
        Part::Pending => return Parse::Pending,
        Part::Yes((op, len)) => {
            let target = &s[len..];
            let (inner, target) = counted(target);
            let count = match (count, inner) {
                (Some(a), Some(b)) => Some(a.saturating_mul(b).min(99_999)),
                (a, b) => a.or(b),
            };
            if target.is_empty() {
                return Parse::Pending;
            }
            // The doubled operator: dd, cc, >>, gUU, gcc, and also gUgU.
            if target == &s[len - 1..len] || target == &s[..len] {
                return done(count, Action::Operate(op, Target::Lines));
            }
            if s[..len] == *"g" && target == "g" {
                return Parse::Pending;
            }
            if let Some(kind) = target.strip_prefix(['i', 'a']) {
                return match kind.chars().next() {
                    None => Parse::Pending,
                    Some(c) if kind.len() == c.len_utf8() && OBJECTS.contains(c) => {
                        done(count, Action::Operate(op, Target::Object(target.starts_with('a'), c)))
                    }
                    _ => Parse::Invalid,
                };
            }
            return match motion_of(target) {
                Part::Pending => Parse::Pending,
                Part::No => Parse::Invalid,
                Part::Yes(m) => done(count, Action::Operate(op, Target::Motion(m))),
            };
        }
        Part::No => {}
    }
    match motion_of(s) {
        Part::Yes(m) => return done(count, Action::Move(m)),
        Part::Pending if s != "g" => return Parse::Pending,
        _ => {}
    }
    let operate = |op, target| done(count, Action::Operate(op, target));
    match s {
        "x" => operate(Op::Delete, Target::Motion(Motion::Right)),
        "X" => operate(Op::Delete, Target::Motion(Motion::Left)),
        "D" => operate(Op::Delete, Target::Motion(Motion::LineEnd)),
        "C" => operate(Op::Change, Target::Motion(Motion::LineEnd)),
        "s" => operate(Op::Change, Target::Motion(Motion::Right)),
        "S" => operate(Op::Change, Target::Lines),
        "Y" => operate(Op::Yank, Target::Lines),
        "g" | "r" | "m" | "q" | "@" | "z" => Parse::Pending,
        "gv" | "g;" | "g," | "zt" | "zz" | "zb" | "z\n" | "z." | "z-" => done(count, Action::Other(s.into())),
        "p" | "P" | "u" | "." | "J" | "gJ" | "~" | "i" | "a" | "I" | "A" | "o" | "O" | "v" | "V" | "R" | "n" | "N"
        | "*" | "#" | ":" | "/" | "?" => done(count, Action::Other(s.into())),
        _ if matches!(s.chars().next(), Some('r' | 'm' | 'q' | '@')) && arg(s).is_some() && s.chars().count() == 2 => {
            done(count, Action::Other(s.into()))
        }
        _ => Parse::Invalid,
    }
}

// ---- moving in text ---------------------------------------------------

fn line_start(t: &str, p: usize) -> usize {
    t[..p].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(t: &str, p: usize) -> usize {
    t[p..].find('\n').map_or(t.len(), |i| p + i)
}

fn first_non_blank(t: &str, p: usize) -> usize {
    let (s, e) = (line_start(t, p), line_end(t, p));
    s + t[s..e].len() - t[s..e].trim_start_matches([' ', '\t']).len()
}

fn char_at(t: &str, p: usize) -> Option<char> {
    t.get(p..).and_then(|s| s.chars().next())
}

fn column(t: &str, p: usize) -> usize {
    t[line_start(t, p)..p].chars().count()
}

/// The offset of a column on the line starting at `s`, or the line's end.
fn at_column(t: &str, s: usize, col: usize) -> usize {
    let e = line_end(t, s);
    t[s..e].char_indices().nth(col).map_or(e, |(i, _)| s + i)
}

/// The last character of a line, where normal mode's cursor stops.
fn last_char(t: &str, p: usize) -> usize {
    let (s, e) = (line_start(t, p), line_end(t, p));
    if e > s { editor::prev(t, e) } else { s }
}

fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn empty_line_at(t: &str, p: usize) -> bool {
    (p == 0 || t[..p].ends_with('\n')) && matches!(char_at(t, p), Some('\n') | None)
}

fn word_next(t: &str, mut p: usize, big: bool) -> usize {
    if let Some(c) = char_at(t, p).filter(|c| !c.is_whitespace()) {
        let k = class(c, big);
        while let Some(c) = char_at(t, p).filter(|&c| class(c, big) == k) {
            p += c.len_utf8();
        }
    }
    while let Some(c) = char_at(t, p).filter(|c| c.is_whitespace()) {
        p += c.len_utf8();
        if c == '\n' && empty_line_at(t, p) && p < t.len() {
            return p;
        }
    }
    p
}

fn word_end(t: &str, p: usize, big: bool) -> usize {
    let mut q = editor::next(t, p);
    while char_at(t, q).is_some_and(char::is_whitespace) {
        q += char_at(t,q).map_or(1,char::len_utf8);
    }
    let Some(c) = char_at(t, q) else { return last_char(t, t.len()).max(p) };
    let k = class(c, big);
    loop {
        let r = editor::next(t, q);
        match char_at(t, r) {
            Some(c) if class(c, big) == k => q = r,
            _ => return q,
        }
    }
}

fn word_back(t: &str, p: usize, big: bool) -> usize {
    let mut q = editor::prev(t, p);
    while q > 0 && char_at(t, q).is_some_and(char::is_whitespace) {
        if empty_line_at(t, q) && q < p {
            return q;
        }
        q = editor::prev(t, q);
    }
    let Some(c) = char_at(t, q).filter(|c| !c.is_whitespace()) else { return q };
    let k = class(c, big);
    while q > 0 {
        let r = editor::prev(t, q);
        if char_at(t, r).is_none_or(|c| class(c, big) != k) {
            break;
        }
        q = r;
    }
    q
}

/// `cw` changes to the end of the word, not to the start of the next.
fn change_word_end(t: &str, p: usize, n: usize, big: bool) -> usize {
    let mut q = p;
    for i in 0..n {
        let k = char_at(t, q).map(|c| class(c, big));
        let r = editor::next(t, q);
        let more = char_at(t, r).is_some_and(|c| Some(class(c, big)) == k);
        q = if i == 0 && !more { q } else { word_end(t, q, big) };
    }
    q
}

/// The `n`th `ch` on the line, from `p`; for `t`, the character before it.
fn find_char(t: &str, p: usize, ch: char, forward: bool, till: bool, n: usize, again: bool) -> Option<usize> {
    let (s, e) = (line_start(t, p), line_end(t, p));
    let mut q = p;
    // Repeating `t` from right before its character goes on to the next.
    if till && again {
        let skip = if forward { char_at(t, editor::next(t, q)) } else { t[s..q].chars().next_back() };
        if skip == Some(ch) {
            q = if forward { editor::next(t, q) } else { editor::prev(t, q) };
        }
    }
    for _ in 0..n {
        q = if forward {
            let from = editor::next(t, q).min(e);
            from + t[from..e].find(ch)?
        } else {
            s + t[s..q].rfind(ch)?
        };
    }
    Some(match (till, forward) {
        (false, _) => q,
        (true, true) => editor::prev(t, q),
        (true, false) => editor::next(t, q),
    })
}

/// The bracket matching the first one at or after `p` on its line.
fn bracket(t: &str, p: usize) -> Option<usize> {
    let e = line_end(t, p);
    let (i, c) = t[p..e].char_indices().find(|(_, c)| "()[]{}".contains(*c))?;
    let at = p + i;
    let (open, close, forward) = match c {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        _ => ('{', '}', false),
    };
    let mut depth = 0usize;
    if forward {
        for (j, c) in t[at..].char_indices() {
            depth = if c == open { depth + 1 } else if c == close { depth - 1 } else { depth };
            if depth == 0 {
                return Some(at + j);
            }
        }
    } else {
        for (j, c) in t[..=at].char_indices().rev() {
            depth = if c == close { depth + 1 } else if c == open { depth - 1 } else { depth };
            if depth == 0 {
                return Some(j);
            }
        }
    }
    None
}

fn line_list(t: &str) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut s = 0;
    for (i, _) in t.match_indices('\n') {
        lines.push((s, i));
        s = i + 1;
    }
    lines.push((s, t.len()));
    lines
}

/// `}` and `{`: the next empty line after a paragraph, or the text's end.
fn paragraph(t: &str, p: usize, forward: bool, n: usize) -> usize {
    let lines = line_list(t);
    let empty = |l: usize| lines[l].0 == lines[l].1;
    let mut l = lines.partition_point(|&(s, _)| s <= p) - 1;
    for _ in 0..n {
        if forward {
            while l + 1 < lines.len() && empty(l) {
                l += 1;
            }
            while l < lines.len() && !empty(l) {
                l += 1;
            }
            if l >= lines.len() {
                return t.len();
            }
        } else {
            if l > 0 && empty(l) {
                l -= 1;
            }
            while l > 0 && empty(l) {
                l -= 1;
            }
            while l > 0 && !empty(l) {
                l -= 1;
            }
        }
    }
    lines[l].0
}

/// A line up or down, at the column wanted.
fn vertical(t: &str, p: usize, n: usize, down: bool, col: usize) -> usize {
    let lines = line_list(t);
    let l = lines.partition_point(|&(s, _)| s <= p) - 1;
    let to = if down { (l + n).min(lines.len() - 1) } else { l.saturating_sub(n) };
    let s = lines[to].0;
    if col == usize::MAX { last_char(t, s) } else { at_column(t, s, col).min(last_char(t, s).max(s)) }
}

fn quote_object(t: &str, p: usize, around: bool, q: char) -> Option<(usize, usize)> {
    let (s, e) = (line_start(t, p), line_end(t, p));
    let marks: Vec<usize> = t[s..e]
        .char_indices()
        .filter(|&(i, c)| c == q && !t[s..s + i].ends_with('\\'))
        .map(|(i, _)| s + i)
        .collect();
    let pair = marks.chunks_exact(2).find(|w| p <= w[1]).map(|w| (w[0], w[1]))?;
    let (a, b) = pair;
    if !around {
        return Some((a + 1, b));
    }
    let after = t[b + 1..e].len() - t[b + 1..e].trim_start_matches([' ', '\t']).len();
    if after > 0 {
        return Some((a, b + 1 + after));
    }
    let before = t[s..a].len() - t[s..a].trim_end_matches([' ', '\t']).len();
    Some((a - before, b + 1))
}

fn bracket_object(t: &str, p: usize, around: bool, open: char, close: char, n: usize) -> Option<(usize, usize)> {
    let enclosing = |from: usize, include: bool| {
        let mut depth = 0usize;
        let end = if include { from + char_at(t, from).map_or(0, char::len_utf8) } else { from };
        for (i, c) in t[..end].char_indices().rev() {
            if c == close && i != from {
                depth += 1;
            } else if c == open {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
        }
        None
    };
    let mut a = if char_at(t, p) == Some(close) {
        let mut depth = 0usize;
        let mut found = None;
        for (i, c) in t[..p].char_indices().rev() {
            if c == close {
                depth += 1;
            } else if c == open {
                if depth == 0 {
                    found = Some(i);
                    break;
                }
                depth -= 1;
            }
        }
        found?
    } else {
        enclosing(p, true)?
    };
    for _ in 1..n {
        a = enclosing(a, false)?;
    }
    let mut depth = 0usize;
    let b = t[a..].char_indices().find_map(|(i, c)| {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(a + i);
            }
        }
        None
    })?;
    if around {
        return Some((a, b + close.len_utf8()));
    }
    // A block on lines of its own leaves its brackets' lines alone.
    let mut inner = (a + open.len_utf8(), b);
    if t[inner.0..b].starts_with('\n') {
        inner.0 += 1;
        let ls = line_start(t, b);
        if t[ls..b].trim().is_empty() && ls > inner.0 {
            inner.1 = ls - 1;
        }
    }
    Some(inner)
}

fn word_object(t: &str, p: usize, around: bool, big: bool) -> Option<(usize, usize)> {
    let c = char_at(t, p).filter(|&c| c != '\n')?;
    let blank = |c: char| c == ' ' || c == '\t';
    let k = |c: char| if blank(c) { 0 } else { class(c, big) };
    let run = |p: usize| {
        let kind = k(char_at(t, p).unwrap());
        let mut a = p;
        while let Some(c) = t[..a].chars().next_back().filter(|&c| c != '\n' && k(c) == kind) {
            a -= c.len_utf8();
        }
        let mut b = p;
        while let Some(c) = char_at(t, b).filter(|&c| c != '\n' && k(c) == kind) {
            b += c.len_utf8();
        }
        (a, b)
    };
    let (a, b) = run(p);
    if !around {
        return Some((a, b));
    }
    if blank(c) {
        let b = if char_at(t, b).is_some_and(|c| c != '\n') { run(b).1 } else { b };
        return Some((a, b));
    }
    if char_at(t, b).is_some_and(blank) {
        return Some((a, run(b).1));
    }
    let a = match t[..a].chars().next_back() {
        Some(c) if blank(c) => run(a - c.len_utf8()).0,
        _ => a,
    };
    Some((a, b))
}

/// `ip` and `ap`: the lines of the paragraph, or of the blank run, at `p`.
fn paragraph_object(t: &str, p: usize, around: bool) -> (usize, usize) {
    let lines = line_list(t);
    let blank = |l: usize| t[lines[l].0..lines[l].1].trim().is_empty();
    let l = lines.partition_point(|&(s, _)| s <= p) - 1;
    let kind = blank(l);
    let (mut a, mut b) = (l, l);
    while a > 0 && blank(a - 1) == kind {
        a -= 1;
    }
    while b + 1 < lines.len() && blank(b + 1) == kind {
        b += 1;
    }
    if around {
        if b + 1 < lines.len() {
            b += 1;
            while b + 1 < lines.len() && blank(b + 1) != kind {
                if !kind {
                    b += 1;
                } else {
                    break;
                }
            }
            if kind {
                while b + 1 < lines.len() && !blank(b + 1) {
                    b += 1;
                }
            }
        } else {
            while a > 0 && blank(a - 1) != kind {
                a -= 1;
            }
        }
    }
    (a, b)
}

/// A text object at `p`: its range, and whether it is whole lines.
pub(crate) fn object(t: &str, p: usize, around: bool, c: char, n: usize) -> Option<(usize, usize, bool)> {
    let pair = |open, close| bracket_object(t, p, around, open, close, n);
    let range = match c {
        'f' | 'c' | 'a' | 'C' => crate::treesitter::text_object(t, p, around, c, n),
        'w' | 'W' => word_object(t, p, around, c == 'W'),
        'p' => {
            let (a, b) = paragraph_object(t, p, around);
            let lines = line_list(t);
            return Some((lines[a].0, lines[b].1, true));
        }
        '"' | '\'' | '`' => quote_object(t, p, around, c),
        '(' | ')' | 'b' => pair('(', ')'),
        '{' | '}' | 'B' => pair('{', '}'),
        '[' | ']' => pair('[', ']'),
        '<' | '>' => pair('<', '>'),
        _ => None,
    }?;
    Some((range.0, range.1, false))
}

/// Text in another case: lower, upper, or each letter swapped.
fn recase(s: &str, op: Op) -> String {
    match op {
        Op::Lower => s.to_lowercase(),
        Op::Upper => s.to_uppercase(),
        _ => s
            .chars()
            .flat_map(|c| -> Vec<char> {
                if c.is_uppercase() { c.to_lowercase().collect() } else { c.to_uppercase().collect() }
            })
            .collect(),
    }
}

/// Joins `n` lines after the one at `p`, the way `J` does, or with
/// `plain` without changing spaces. Returns the new text of the joined
/// stretch, its start and end, and where the last join is.
fn joined(t: &str, p: usize, n: usize, plain: bool) -> Option<(usize, usize, String, usize)> {
    let lines = line_list(t);
    let l = lines.partition_point(|&(s, _)| s <= p) - 1;
    if l + 1 >= lines.len() {
        return None;
    }
    let last = (l + n.max(1)).min(lines.len() - 1);
    let mut out = t[lines[l].0..lines[l].1].to_string();
    let mut at = out.len();
    for &(s, e) in &lines[l + 1..=last] {
        let next = &t[s..e];
        if plain {
            at = out.len();
            out.push_str(next);
            continue;
        }
        let next = next.trim_start_matches([' ', '\t']);
        let trimmed = out.trim_end_matches([' ', '\t']).len();
        out.truncate(trimmed);
        at = out.len();
        if !next.is_empty() && !out.is_empty() && !next.starts_with(')') {
            out.push(' ');
        }
        out.push_str(next);
    }
    Some((lines[l].0, lines[last].1, out, at))
}

/// A substitution: `s/pattern/replacement/flags`, with any delimiter.
fn substitution(s: &str) -> Option<(String, String, String)> {
    let mut it = s.strip_prefix('s')?.chars();
    let delim = it.next().filter(|c| !c.is_alphanumeric() && *c != '\\' && *c != ' ')?;
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for c in it {
        if escaped {
            if c != delim {
                parts.last_mut()?.push('\\');
            }
            parts.last_mut()?.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == delim && parts.len() < 3 {
            parts.push(String::new());
        } else {
            parts.last_mut()?.push(c);
        }
    }
    if escaped {
        parts.last_mut()?.push('\\');
    }
    while parts.len() < 3 {
        parts.push(String::new());
    }
    let flags = parts.pop()?;
    let rep = parts.pop()?;
    let pat = parts.pop()?;
    (!pat.is_empty()).then_some((pat, rep, flags))
}

/// Vim's replacement text in the regex crate's: `&` and `\0` are the match,
/// `\1` a group, and `$` is itself.
fn replacement(rep: &str) -> String {
    let mut out = String::new();
    let mut it = rep.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some(d) if d.is_ascii_digit() => out.push_str(&format!("${{{d}}}")),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('&') => out.push('&'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            },
            '&' => out.push_str("${0}"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

/// Lines an Ex range names: `%`, `.`, `$`, a number, or two with a comma.
fn ex_range(spec: &str, here: usize, lines: usize) -> Option<(usize, usize)> {
    let one = |s: &str| {
        let offset = s.char_indices().find(|&(_,c)| c == '+' || c == '-').map(|(i,_)|i).unwrap_or(s.len());
        let (base,tail)=s.split_at(offset);
        let mut line = match base { "" | "."=>here, "$"=>lines-1, _=>base.parse::<usize>().ok()?.saturating_sub(1) };
        let mut rest=tail;
        while !rest.is_empty() {
            let add=rest.starts_with('+'); rest=&rest[1..];
            let len=rest.bytes().take_while(u8::is_ascii_digit).count();
            let n=if len==0 {1} else {rest[..len].parse::<usize>().ok()?}; rest=&rest[len..];
            line=if add {line.saturating_add(n)} else {line.saturating_sub(n)};
        }
        Some(line.min(lines-1))
    };
    match spec {
        "%" => Some((0, lines - 1)),
        _ => match spec.split_once(',') {
            Some((a, b)) => {
                let (a, b) = (one(a)?, one(b)?);
                Some((a.min(b), a.max(b)))
            }
            None => one(spec).map(|l| (l, l)),
        },
    }
}

// ---- in the pane ------------------------------------------------------

impl Pane {
    fn vim_visual(&self) -> bool {
        matches!(self.vim.mode, Mode::Visual | Mode::Line | Mode::Block)
    }

    /// The mode's name for the status bar.
    pub(crate) fn vim_label(&self) -> Option<&'static str> {
        if self.vim.enabled && self.vim.helix.enabled {
            return Some(if self.vim.mode == Mode::Insert { "INS" } else if self.vim.helix.select { "SEL" } else { "NOR" });
        }
        self.vim.enabled.then_some(match self.vim.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Replace => "REPLACE",
            Mode::Visual => "VISUAL",
            Mode::Line => "V-LINE",
            Mode::Block => "V-BLOCK",
        })
    }

    pub(crate) fn vim_input(&mut self) -> Option<&mut String> {
        if !self.vim.enabled { return None; }
        self.vim.command.as_mut().map(|c| &mut c.0)
    }

    /// What the status bar shows: the command line while one is typed,
    /// otherwise the mode and any keys of a command not yet complete.
    pub(crate) fn vim_status(&self) -> Option<String> {
        let label = self.vim_label()?;
        if self.vim.helix.enabled && self.vim.command.is_none() { return Some(label.into()); }
        if let Some((line, _)) = &self.vim.command {
            return Some(format!(":{line}"));
        }
        if let Some(reg)=self.vim.macro_register { return Some(format!("{label} recording @{reg}")); }
        Some(if self.vim.pending.is_empty() { label.into() } else { format!("{label}  {}", self.vim.pending) })
    }

    /// Whether keys type text, as without Vim.
    pub(crate) fn vim_typing(&self) -> bool {
        !self.vim.enabled || matches!(self.vim.mode, Mode::Insert | Mode::Replace)
    }

    /// Keys Vim takes before the window's own shortcuts see them: Esc
    /// while there is a mode or command to leave, and Ctrl+R, Ctrl+U and
    /// Ctrl+V outside insert mode.
    pub(crate) fn vim_takes(&self, key: Key, mods: Mods) -> bool {
        if !self.vim.enabled {
            return false;
        }
        if self.vim.helix.enabled && !self.vim_typing() && (!mods.ctrl && !mods.alt || mods.alt && matches!(key,Key::Char(';'|','|'s'))) { return true; }
        let busy = !self.vim.pending.is_empty() || self.vim.command.is_some();
        match key {
            Key::Escape => self.vim.mode != Mode::Normal || busy || self.buf.has_sel(),
            Key::Char('r' | 'u' | 'v' | 'd' | 'e' | 'y' | 'f' | 'b') => mods.ctrl && !mods.alt && !mods.shift && !self.vim_typing(),
            _ => false,
        }
    }

    /// Where normal and visual modes draw a block caret.
    pub(crate) fn vim_block(&self) -> Option<usize> {
        if !self.vim.enabled {
            return None;
        }
        match self.vim.mode {
            Mode::Insert | Mode::Replace => None,
            Mode::Normal if self.vim.helix.enabled => Some(self.hx_cursor()),
            Mode::Normal => Some(self.buf.cur),
            _ => Some(self.vim.cursor.min(self.buf.text.len())),
        }
    }

    /// The width of the block caret at `at`, on its row of a layout.
    pub(crate) fn vim_block_width(&self, lay: &Layout, ls: usize, at: usize) -> f32 {
        let next = editor::next(&self.buf.text, at);
        let (row, x) = lay.locate((at - ls) as u32);
        let (row2, x2) = lay.locate((next - ls) as u32);
        let le = self.buf.line(self.buf.line_of(at)).1;
        if next <= le && row2 == row && x2 > x { x2 - x } else { (self.px() * 0.6).round() }
    }

    fn vim_char(key: Key, mods: Mods, text: Option<&str>, arg: bool) -> Option<char> {
        if let Some(c) = text.and_then(|s| s.chars().next()).filter(|c| !c.is_control()) {
            return Some(c);
        }
        Some(match key {
            Key::Char(c) => if mods.shift { c.to_ascii_uppercase() } else { c },
            Key::Enter if arg => '\n',
            Key::Tab if arg => '\t',
            Key::Left | Key::Backspace => 'h',
            Key::Right => 'l',
            Key::Up => 'k',
            Key::Down | Key::Enter => 'j',
            Key::Home => '0',
            Key::End => '$',
            Key::Delete => 'x',
            _ => return None,
        })
    }

    /// Handles a key in Vim mode. Returns whether Vim took it; keys it
    /// leaves go on to the editor as usual.
    pub(crate) fn vim_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if self.vim.enabled && self.vim.helix.enabled { return self.helix_key(key, mods, text); }
        self.vim_macro_key(key, mods, text)
    }

    fn vim_key_inner(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if !self.vim.enabled {
            return false;
        }
        if key == Key::Other && mods.ctrl && mods.alt && self.vim.replaying {
            if let Some(value) = text.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) {
                let mut a = self.buf.cur;
                let mut b = a;
                for _ in 0..value[0].as_u64().unwrap_or(0) { a = editor::prev(&self.buf.text, a); }
                for _ in 0..value[1].as_u64().unwrap_or(0) { b = editor::next(&self.buf.text, b); }
                self.edit(a, b, value[2].as_str().unwrap_or(""), EditKind::Other);
                return true;
            }
        }
        let stroke = (key, mods, text.map(str::to_string));
        if matches!(self.vim.mode, Mode::Insert | Mode::Replace) {
            if !self.vim.replaying
                && let Some(r) = &mut self.vim.recording
            {
                r.push(stroke.clone());
            }
            if key == Key::Escape && !mods.ctrl && !mods.alt {
                self.vim_leave_insert();
                return true;
            }
            self.vim.inserted.push(stroke);
            return self.vim.mode == Mode::Replace && self.vim_replace_key(key, mods, text);
        }
        self.redraw = true;
        if let Some((line, range)) = &mut self.vim.command {
            match key {
                Key::Enter => {
                    let (line, range) = (line.clone(), *range);
                    self.vim.command = None;
                    self.vim_ex(&line, range);
                }
                Key::Escape => self.vim.command = None,
                Key::Backspace if line.is_empty() => self.vim.command = None,
                Key::Backspace => {
                    line.pop();
                }
                _ => {
                    if let Some(t) = text.filter(|t| !mods.ctrl && !mods.alt && !t.chars().any(char::is_control)) {
                        line.push_str(t);
                    }
                }
            }
            return true;
        }
        if mods.ctrl || mods.alt {
            if !self.vim_takes(key, mods) {
                return false;
            }
            self.vim.strokes.clear();
            let count = counted(&self.vim.pending).0;
            self.vim.pending.clear();
            match key {
                Key::Char('e' | 'y' | 'd' | 'u' | 'f' | 'b') => {
                    self.vim_scroll(key, count);
                    return true;
                }
                Key::Char('r') => {
                    if !self.multi_undo(true) && let Some(c) = self.buf.redo() {
                        self.changed(c);
                    }
                    self.vim_normal();
                }
                _ => {
                    if self.vim.mode == Mode::Block {
                        self.vim_normal();
                    } else {
                        self.vim_enter_visual(Mode::Block);
                    }
                }
            }
            self.vim_settle();
            return true;
        }
        if matches!(key, Key::PageUp | Key::PageDown) || mods.shift && !matches!(key, Key::Char(_)) && text.is_none() {
            return false;
        }
        if key == Key::Escape {
            self.vim.pending.clear();
            self.vim.strokes.clear();
            if self.vim_visual() { self.vim.last_visual = Some((self.vim.mode, self.vim.anchor, self.vim.cursor)); }
            if self.vim_visual() || self.buf.has_sel() || !self.extra_cursors.is_empty() {
                let at = if self.vim_visual() { self.vim.cursor } else { self.buf.cur };
                self.vim.mode = Mode::Normal;
                self.extra_cursors.clear();
                self.select(at, at);
            }
            self.vim_settle();
            return true;
        }
        // A selection made some other way, by the mouse or Ctrl+D, is
        // taken up as a visual one.
        if self.vim.mode == Mode::Normal && (self.buf.has_sel() || !self.extra_cursors.is_empty()) {
            self.vim.mode = Mode::Visual;
            self.vim_take_selection();
        } else if matches!(self.vim.mode, Mode::Visual | Mode::Line)
            && (self.buf.anchor, self.buf.cur) != self.vim.shown
        {
            self.vim_take_selection();
        }
        let arg = self.vim.pending.ends_with(['r', 'f', 't', 'F', 'T', 'm', '`', '\'', '"', 'q', '@', 'z']);
        let Some(c) = Self::vim_char(key, mods, text, arg) else { return true };
        self.vim.pending.push(c);
        self.vim.strokes.push(stroke);
        match parse(&self.vim.pending, self.vim_visual()) {
            Parse::Pending => {}
            Parse::Invalid => {
                self.vim.pending.clear();
                self.vim.strokes.clear();
            }
            Parse::Done(command) => {
                self.vim.pending.clear();
                let strokes = std::mem::take(&mut self.vim.strokes);
                self.vim_run(command, strokes);
            }
        }
        true
    }

    fn vim_take_selection(&mut self) {
        let (a, c) = (self.buf.anchor, self.buf.cur);
        self.vim.anchor = a;
        self.vim.cursor = if c > a { editor::prev(&self.buf.text, c) } else { c };
        self.vim.shown = (a, c);
    }

    fn vim_is_change(&self, command: &Command) -> bool {
        match &command.action {
            Action::Operate(op, _) => *op != Op::Yank,
            Action::Other(s) => {
                let first = s.chars().next().unwrap_or(' ');
                if self.vim_visual() {
                    matches!(s.as_str(), "J" | "gJ" | "p" | "P" | "I" | "A") || first == 'r'
                } else {
                    "xXsSCDpPJ~riaIAoOR".contains(first) || s == "gJ"
                }
            }
            _ => false,
        }
    }

    /// Carries out a command, as one undo step.
    fn vim_run(&mut self, command: Command, strokes: Vec<Stroke>) {
        let change = self.vim_is_change(&command);
        let visual = self.vim_visual();
        let checkpoint = change.then(|| self.buf.checkpoint());
        if let Action::Other(s) = &command.action
            && s == "."
        {
            return self.vim_repeat(command.count);
        }
        if visual { self.vim.last_visual = Some((self.vim.mode, self.vim.anchor, self.vim.cursor)); }
        if change && !self.vim.replaying {
            self.vim.visual_change = visual.then(|| {
                let (a, b) = self.buf.sel();
                let rows = self.buf.line_of(self.vim.anchor).abs_diff(self.buf.line_of(self.vim.cursor));
                (self.vim.mode, rows, if self.vim.mode == Mode::Block { column(&self.buf.text,self.vim.anchor).abs_diff(column(&self.buf.text,self.vim.cursor))+1 } else if rows>0 { column(&self.buf.text,editor::prev(&self.buf.text,b))+1 } else { self.buf.text[a..b].chars().count() })
            });
            self.vim_note_change();
        }
        self.vim_do(command);
        let inserting = matches!(self.vim.mode, Mode::Insert | Mode::Replace);
        if change && !self.vim.replaying {
            if inserting {
                self.vim.recording = Some(strokes);
            } else {
                self.vim.last_change = strokes;
            }
        }
        if let Some(checkpoint) = checkpoint {
            if inserting && self.vim.group.is_none() {
                self.vim.group = Some(checkpoint);
            } else {
                self.buf.finish(checkpoint);
            }
        }
        self.vim_settle();
    }

    fn vim_repeat(&mut self, count: Option<usize>) {
        let mut strokes = self.vim.last_change.clone();
        if strokes.is_empty() {
            return;
        }
        if let Some(n) = count.filter(|_| self.vim.visual_change.is_none()) {
            // The count given replaces the count the change was made with.
            let reg = if strokes.first().and_then(|s| s.2.as_deref()) == Some("\"") { 2 } else { 0 };
            let digits = strokes[reg.min(strokes.len())..]
                .iter()
                .take_while(|s| s.2.as_deref().is_some_and(|t| t.len() == 1 && t.as_bytes()[0].is_ascii_digit()))
                .count();
            let typed: Vec<Stroke> =
                n.to_string().chars().map(|c| (Key::Char(c), Mods::default(), Some(c.to_string()))).collect();
            strokes.splice(reg..reg + digits, typed);
        }
        let checkpoint = self.buf.checkpoint();
        let was = std::mem::replace(&mut self.vim.replaying, true);
        if let Some((mode, rows, chars)) = self.vim.visual_change {
            let at = self.buf.cur;
            self.vim_enter_visual(mode);
            self.vim.cursor = if mode == Mode::Line {
                vertical(&self.buf.text, at, rows, true, 0)
            } else if mode == Mode::Block {
                let line=(self.buf.line_of(at)+rows).min(self.buf.lines()-1);
                at_column(&self.buf.text,self.buf.starts[line],column(&self.buf.text,at)+chars-1)
            } else if rows>0 {
                let line=(self.buf.line_of(at)+rows).min(self.buf.lines()-1);
                at_column(&self.buf.text,self.buf.starts[line],chars.saturating_sub(1))
            } else {
                self.buf.text[at..].char_indices().nth(chars.saturating_sub(1)).map_or(at, |(i, _)| at + i)
            };
            self.vim_show();
        }
        for (key, mods, text) in strokes {
            self.key(key, mods, text.as_deref());
        }
        if matches!(self.vim.mode, Mode::Insert | Mode::Replace) {
            self.vim_leave_insert();
        }
        self.vim.replaying = was;
        self.buf.finish(checkpoint);
        self.vim_settle();
    }

    /// After a command: normal mode's cursor rests on a character, not past
    /// the end of its line.
    fn vim_settle(&mut self) {
        if self.vim.mode == Mode::Normal {
            self.extra_cursors.clear();
            let t = &self.buf.text;
            let c = self.buf.cur.min(t.len());
            let c = if line_end(t, c) == c { last_char(t, c) } else { c };
            (self.buf.anchor, self.buf.cur) = (c, c);
        }
        self.buf.commit();
        self.moved();
        // Moving may step the cursor off a notebook's cell header.
        if self.vim.mode == Mode::Normal {
            let t = &self.buf.text;
            let c = self.buf.cur;
            let c = if line_end(t, c) == c { last_char(t, c) } else { c };
            (self.buf.anchor, self.buf.cur) = (c, c);
        }
    }

    fn vim_normal(&mut self) {
        self.vim.mode = Mode::Normal;
        let c = self.buf.cur;
        self.extra_cursors.clear();
        (self.buf.anchor, self.buf.cur) = (c, c);
    }

    fn vim_go(&mut self, to: usize) {
        if self.vim_visual() {
            self.vim.cursor = to;
            self.vim_show();
        } else {
            (self.buf.anchor, self.buf.cur) = (to, to);
        }
    }

    fn vim_enter_visual(&mut self, mode: Mode) {
        if !self.vim_visual() {
            self.vim.anchor = self.buf.cur;
            self.vim.cursor = self.buf.cur;
        }
        self.vim.mode = mode;
        self.vim_show();
    }

    /// Shows the visual selection, which takes in the character under the
    /// cursor, as the editor's selection, which does not.
    fn vim_show(&mut self) {
        let t = &self.buf.text;
        let (a, c) = (self.vim.anchor.min(t.len()), self.vim.cursor.min(t.len()));
        self.extra_cursors.clear();
        match self.vim.mode {
            Mode::Line => {
                let (la, lc) = (line_start(t, a), line_start(t, c));
                let past = |p: usize| (line_end(t, p) + 1).min(t.len());
                (self.buf.anchor, self.buf.cur) = if c >= a { (la, past(c)) } else { (past(a), lc) };
            }
            Mode::Block => {
                let (ca, cc) = (column(t, a), column(t, c));
                let (from, to) = if cc >= ca { (a, editor::next(t, c)) } else { (editor::next(t, a), c) };
                let to = to.min(line_end(t, c).max(c));
                self.column_select(from, to);
            }
            _ => {
                (self.buf.anchor, self.buf.cur) = if c >= a { (a, editor::next(t, c)) } else { (editor::next(t, a), c) };
            }
        }
        self.vim.shown = (self.buf.anchor, self.buf.cur);
    }

    /// Where a motion goes from `from`, and how an operator takes it in.
    fn vim_motion(&mut self, m: Motion, count: Option<usize>, from: usize) -> Option<(usize, Kind)> {
        use Motion::*;
        let n = count.unwrap_or(1).max(1);
        let t = &self.buf.text;
        let col = || self.vim.want.unwrap_or_else(|| column(t, from));
        let to = match m {
            Left => (from - t[line_start(t, from)..from].chars().rev().take(n).map(char::len_utf8).sum::<usize>(), Kind::Exclusive),
            Right => {
                let e = line_end(t, from);
                (from + t[from..e].chars().take(n).map(char::len_utf8).sum::<usize>(), Kind::Exclusive)
            }
            Up | Down => {
                let col = col();
                let mut to = vertical(t, from, n, m == Down, col);
                // Folded lines are passed over, as one line.
                let line = self.buf.line_of(to);
                if let Some(&(a, b)) = self.folds.iter().find(|&&(a, b)| line > a && line <= b) {
                    let l = if m == Down { (b + 1).min(self.buf.lines() - 1) } else { a };
                    to = vertical(t, self.buf.starts[l], 0, true, col);
                }
                self.vim.want = Some(col);
                return Some((to, Kind::Linewise));
            }
            WordNext(big) => (
                (0..n).fold(from, |p, _| word_next(t, p, big)),
                Kind::Exclusive,
            ),
            WordBack(big) => ((0..n).fold(from, |p, _| word_back(t, p, big)), Kind::Exclusive),
            WordEnd(big) => ((0..n).fold(from, |p, _| word_end(t, p, big)), Kind::Inclusive),
            LineStart => (line_start(t, from), Kind::Exclusive),
            FirstNonBlank => (first_non_blank(t, from), Kind::Exclusive),
            LineEnd => {
                let down = vertical(t, from, n - 1, true, 0);
                self.vim.want = Some(usize::MAX);
                return Some((line_end(t, down), Kind::Exclusive));
            }
            FirstLine | LastLine => {
                let line = match (count, m) {
                    (Some(n), _) => n - 1,
                    (None, FirstLine) => 0,
                    _ => self.buf.lines() - 1,
                };
                let s = self.buf.starts[line.min(self.buf.lines() - 1)];
                (first_non_blank(t, s), Kind::Linewise)
            }
            Find { ch, forward, till } => {
                self.vim.last_find = Some((ch, forward, till));
                (find_char(t, from, ch, forward, till, n, false)?, Kind::Inclusive)
            }
            RepeatFind(reverse) => {
                let (ch, forward, till) = self.vim.last_find?;
                let forward = forward != reverse;
                (find_char(t, from, ch, forward, till, n, true)?, if forward { Kind::Inclusive } else { Kind::Exclusive })
            }
            Bracket => (bracket(t, from)?, Kind::Inclusive),
            ParagraphNext | ParagraphBack => (paragraph(t, from, m == ParagraphNext, n), Kind::Exclusive),
            ScreenTop | ScreenMiddle | ScreenBottom => {
                let line = self.vim_screen_line(m, n);
                (first_non_blank(&self.buf.text, self.buf.starts[line]), Kind::Linewise)
            }
            Mark(name, lines) => {
                let &(line, col) = self.vim.marks.get(&name)?;
                let s = self.buf.starts[line.min(self.buf.lines() - 1)];
                if lines {
                    (first_non_blank(t, s), Kind::Linewise)
                } else {
                    (at_column(t, s, col), Kind::Exclusive)
                }
            }
        };
        if !matches!(m, Up | Down) {
            self.vim.want = None;
        }
        Some(to)
    }

    /// The first, middle or last line on screen, `n` lines in.
    fn vim_screen_line(&mut self, m: Motion, n: usize) -> usize {
        let lines = self.buf.lines();
        let first = (self.top + usize::from(self.top_off > 1.0)).min(lines - 1);
        let mut last = first;
        let mut y = -self.top_off;
        for l in self.top..lines {
            y += self.height(l);
            if y > self.h + 1.0 {
                break;
            }
            last = l.max(first);
        }
        match m {
            Motion::ScreenTop => (first + n - 1).min(last),
            Motion::ScreenBottom => last.saturating_sub(n - 1).max(first),
            _ => (first + last) / 2,
        }
    }

    /// The range an operator works on, and whether it is whole lines.
    fn vim_range(&mut self, op: Op, target: Target, count: Option<usize>) -> Option<(usize, usize, bool)> {
        let n = count.unwrap_or(1).max(1);
        let cur = self.buf.cur;
        let t = &self.buf.text;
        match target {
            Target::Lines => {
                let l = self.buf.line_of(cur);
                let last = (l + n - 1).min(self.buf.lines() - 1);
                Some((self.buf.starts[l], self.buf.line(last).1, true))
            }
            Target::Object(around, c) => object(t, cur, around, c, n),
            Target::Motion(Motion::WordNext(big))
                if op == Op::Change && char_at(t, cur).is_some_and(|c| !c.is_whitespace()) =>
            {
                let end = change_word_end(t, cur, n, big);
                Some((cur, editor::next(t, end), false))
            }
            Target::Motion(m) => {
                let (to, kind) = self.vim_motion(m, count, cur)?;
                let t = &self.buf.text;
                let (a, b) = (cur.min(to), cur.max(to));
                match kind {
                    Kind::Linewise => Some((line_start(t, a), line_end(t, b), true)),
                    Kind::Inclusive => Some((a, editor::next(t, b).min(t.len()), false)),
                    Kind::Exclusive => {
                        // An exclusive motion ending at the start of a later
                        // line stops at the end of the line before; from the
                        // start of a line, it takes whole lines.
                        let (s, fb) = (line_start(t, b), first_non_blank(t, b));
                        if b > a && s > line_start(t, a) && b <= fb {
                            if a <= first_non_blank(t, a) && !matches!(m, Motion::WordNext(_)) {
                                return Some((line_start(t, a), s - 1, true));
                            }
                            return Some((a, s - 1, false));
                        }
                        (a != b).then_some((a, b, false))
                    }
                }
            }
            Target::Selection(_) => None,
        }
    }

    /// Keeps text in a register: the unnamed one always, a named one when
    /// given, and the system clipboard for `"+`.
    fn vim_store(&mut self, register: Option<char>, text: String, lines: bool, yank: bool) {
        let r = register.unwrap_or('"');
        if r == '_' {
            return;
        }
        if r.is_ascii_uppercase() {
            let entry = self.vim.registers.entry(r.to_ascii_lowercase()).or_default();
            entry.0.push_str(&text);
            entry.1 |= lines;
        } else if r.is_ascii_lowercase() || r.is_ascii_digit() || r == '-' {
            self.vim.registers.insert(r, (text.clone(), lines));
        } else if r == '+' || r == '*' {
            self.copy = Some(text.clone());
        }
        if yank && register.is_none() {
            self.vim.registers.insert('0', (text.clone(), lines));
        } else if !yank {
            if lines || text.contains('\n') {
                for digit in ('2'..='9').rev() {
                    if let Some(value) = self.vim.registers.get(&char::from_u32(digit as u32 - 1).unwrap()).cloned() {
                        self.vim.registers.insert(digit, value);
                    } else { self.vim.registers.remove(&digit); }
                }
                self.vim.registers.insert('1', (text.clone(), lines));
            } else { self.vim.registers.insert('-', (text.clone(), false)); }
        }
        self.vim.registers.insert('"', (text, lines));
    }

    /// Applies an operator to a range.
    fn vim_apply(&mut self, op: Op, range: (usize, usize, bool), register: Option<char>, times: usize) {
        let (a, b, lines) = range;
        let t = &self.buf.text;
        let (l0, l1) = (self.buf.line_of(a), self.buf.line_of(b));
        // Whole lines take in their line break.
        let (start, end) = if lines {
            let s = self.buf.starts[l0];
            let e = self.buf.line(l1).1;
            if e < t.len() { (s, e + 1) } else { (s.saturating_sub(usize::from(s > 0)), e) }
        } else {
            (a, b)
        };
        let text = if lines {
            format!("{}\n", &t[self.buf.starts[l0]..self.buf.line(l1).1])
        } else {
            t[a..b].to_string()
        };
        match op {
            Op::Yank => {
                self.vim_store(register, text, lines, true);
                let cur = self.buf.cur;
                let at = match lines {
                    true if l0 < self.buf.line_of(cur) => {
                        let t = &self.buf.text;
                        at_column(t, self.buf.starts[l0], column(t, cur))
                    }
                    true => cur,
                    false => a,
                };
                self.vim_go(at);
            }
            Op::Delete => {
                self.vim_store(register, text, lines, false);
                if self.edit(start, end, "", EditKind::Other) {
                    let at = start.min(self.buf.text.len());
                    let at = if lines { first_non_blank(&self.buf.text, at) } else { at };
                    self.vim_go(at);
                }
            }
            Op::Change => {
                self.vim_store(register, text, lines, false);
                if lines {
                    let s = self.buf.starts[l0];
                    let e = self.buf.line(l1).1;
                    let indent = self.buf.text[s..first_non_blank(&self.buf.text, s)].to_string();
                    if self.edit(s, e, &indent, EditKind::Other) || s + indent.len() == e {
                        self.vim_insert(s + indent.len());
                    }
                } else if a == b || self.edit(a, b, "", EditKind::Other) {
                    self.vim_insert(a);
                }
            }
            Op::Indent | Op::Outdent => {
                self.vim_shift(l0, l1, op == Op::Outdent, times);
            }
            Op::Lower | Op::Upper | Op::Toggle => {
                let new = recase(&t[a..b], op);
                if new != t[a..b] {
                    self.edit(a, b, &new, EditKind::Other);
                }
                self.vim_go(a);
            }
            Op::Comment => {
                (self.buf.anchor, self.buf.cur) = (self.buf.starts[l0], self.buf.line(l1).1);
                self.comment();
                let at = first_non_blank(&self.buf.text, self.buf.starts[l0.min(self.buf.lines() - 1)]);
                self.vim_go(at);
            }
        }
    }

    /// Shifts lines in or out by the file's indentation, leaving blank
    /// lines alone.
    fn vim_shift(&mut self, l0: usize, l1: usize, out: bool, times: usize) {
        let spaces = self.prose_at(self.buf.starts[l0]) || self.python_at(self.buf.starts[l0]) || self.doc == Doc::Latex;
        let configured = self.indentation();
        let unit = configured.unwrap_or_else(|| if spaces { "    ".into() } else { "\t".into() });
        for _ in 0..times.max(1) {
            (self.buf.anchor, self.buf.cur) = (self.buf.starts[l0], self.buf.line(l1).1);
            self.rewrite_lines(|line| {
                if line.trim().is_empty() {
                    return line.to_string();
                }
                if !out {
                    return format!("{unit}{line}");
                }
                let cut = if line.starts_with('\t') {
                    1
                } else {
                    line.bytes().take(if unit == "\t" { 4 } else { unit.len() }).take_while(|&c| c == b' ').count()
                };
                line[cut..].to_string()
            });
        }
        let at = first_non_blank(&self.buf.text, self.buf.starts[l0]);
        self.vim_go(at);
    }

    fn vim_insert(&mut self, at: usize) {
        (self.buf.anchor, self.buf.cur) = (at, at);
        self.vim_begin_insert(Mode::Insert, 1);
    }

    fn vim_begin_insert(&mut self, mode: Mode, count: usize) {
        self.vim.mode = mode;
        self.vim.inserted.clear();
        self.vim.replaced.clear();
        self.vim.insert_count = count.max(1);
        self.vim.opened_line = false;
        self.vim.want = None;
        self.buf.commit();
    }

    /// Esc from insert or replace mode: what was typed goes in again for a
    /// count, and it all becomes one undo step.
    fn vim_leave_insert(&mut self) {
        let typed = std::mem::take(&mut self.vim.inserted);
        let was = std::mem::replace(&mut self.vim.replaying, true);
        for _ in 1..self.vim.insert_count {
            if self.vim.opened_line {
                self.newline();
            }
            for (key, mods, text) in &typed {
                self.key(*key, *mods, text.as_deref());
            }
        }
        self.vim.replaying = was;
        self.vim.insert_count = 1;
        self.vim.mode = Mode::Normal;
        self.extra_cursors.clear();
        self.snippet = None;
        let t = &self.buf.text;
        let c = self.buf.cur;
        let c = if c > line_start(t, c) { editor::prev(t, c) } else { c };
        (self.buf.anchor, self.buf.cur) = (c, c);
        if let Some(checkpoint) = self.vim.group.take() {
            self.buf.finish(checkpoint);
        }
        if !self.vim.replaying
            && let Some(r) = self.vim.recording.take()
        {
            self.vim.last_change = r;
        }
        self.vim_settle();
    }

    /// A key in replace mode: typing overwrites, and Backspace puts back
    /// what was overwritten.
    fn vim_replace_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if mods.ctrl || mods.alt {
            return false;
        }
        let cur = self.buf.cur;
        let t = &self.buf.text;
        match key {
            Key::Backspace => {
                let Some(old) = self.vim.replaced.pop() else {
                    let to = editor::prev(t, cur).max(line_start(t, cur));
                    self.select(to, to);
                    return true;
                };
                let p = editor::prev(t, cur);
                let s = old.map(String::from).unwrap_or_default();
                self.edit(p, cur, &s, EditKind::Other);
                self.select(p, p);
                true
            }
            _ => {
                let Some(s) = text.filter(|s| !s.is_empty() && !s.chars().any(char::is_control)) else { return false };
                let end = line_end(t, cur);
                let old = char_at(t, cur).filter(|_| cur < end);
                let to = if cur < end { editor::next(t, cur) } else { cur };
                self.vim.replaced.push(old);
                self.edit(cur, to, s, EditKind::Type);
                self.moved();
                true
            }
        }
    }

    fn vim_do(&mut self, command: Command) {
        let Command { register, count, action } = command;
        let n = count.unwrap_or(1).max(1);
        match action {
            Action::Move(m) => {
                let from = if self.vim_visual() { self.vim.cursor } else { self.buf.cur };
                if let Some((to, _)) = self.vim_motion(m, count, from) {
                    let to = if self.vim_visual() || matches!(m, Motion::LineEnd) { to } else { to.min(last_char(&self.buf.text, to).max(line_start(&self.buf.text, to))) };
                    let to = if self.vim_visual() && to == line_end(&self.buf.text, to) && to > line_start(&self.buf.text, to) {
                        last_char(&self.buf.text, to)
                    } else {
                        to
                    };
                    self.vim_go(to);
                }
            }
            Action::Select(around, c) => {
                let (a, b, lines) = object(&self.buf.text, self.vim.cursor, around, c, n).unwrap_or((self.vim.cursor, self.vim.cursor, false));
                if b > a {
                    self.vim.anchor = a;
                    self.vim.cursor = editor::prev(&self.buf.text, b);
                    if lines && self.vim.mode == Mode::Visual {
                        self.vim.mode = Mode::Line;
                    }
                    self.vim_show();
                }
            }
            Action::Operate(op, Target::Selection(lines)) => self.vim_visual_operate(op, lines, register, n),
            Action::Operate(op, target) => {
                if let Some(range) = self.vim_range(op, target, count) {
                    self.vim_apply(op, range, register, 1);
                }
            }
            Action::Other(s) if self.vim_visual() => self.vim_visual_other(&s, register),
            Action::Other(s) => self.vim_other(&s, register, count),
        }
    }

    /// The ranges a visual selection covers: one, or one a line in a block
    /// or across several selections.
    fn vim_selections(&self) -> Vec<(usize, usize)> {
        let mut all: Vec<(usize, usize)> = self.extra_cursors.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
        all.push(self.buf.sel());
        all.sort_unstable();
        all
    }

    fn vim_visual_operate(&mut self, op: Op, force_lines: bool, register: Option<char>, n: usize) {
        let mode = self.vim.mode;
        let ranges = self.vim_selections();
        let (first, last) = (ranges[0].0, ranges[ranges.len() - 1].1);
        let lines = force_lines || mode == Mode::Line || matches!(op, Op::Indent | Op::Outdent | Op::Comment);
        self.vim.mode = Mode::Normal;
        if ranges.len() > 1 && !lines {
            let text = ranges.iter().map(|&(a, b)| &self.buf.text[a..b]).collect::<Vec<_>>().join("\n");
            match op {
                Op::Yank | Op::Delete | Op::Change => {
                    self.vim_store(register, text, false, op == Op::Yank);
                    if op == Op::Yank {
                        self.extra_cursors.clear();
                        self.vim_go(first);
                        return;
                    }
                    self.multiple_insert("");
                    if op == Op::Change {
                        self.vim_begin_insert(Mode::Insert, 1);
                    } else {
                        let at = self.extra_cursors.iter().map(|r| r.0).chain([self.buf.cur]).min().unwrap_or(first);
                        self.extra_cursors.clear();
                        self.vim_go(at);
                    }
                }
                _ => {
                    for &(a, b) in ranges.iter().rev() {
                        let new = recase(&self.buf.text[a..b], op);
                        self.edit(a, b, &new, EditKind::Other);
                    }
                    self.vim_go(first);
                }
            }
            return;
        }
        let end = if lines { last.saturating_sub(usize::from(last > first && self.buf.text[..last].ends_with('\n'))) } else { last };
        self.extra_cursors.clear();
        (self.buf.anchor, self.buf.cur) = (first, first);
        self.vim_apply(op, (first, end.max(first), lines), register, n);
    }

    fn vim_visual_other(&mut self, s: &str, register: Option<char>) {
        let ranges = self.vim_selections();
        let (first, last) = (ranges[0].0, ranges[ranges.len() - 1].1);
        let lines = || {
            let t = &self.buf.text;
            let l1 = self.buf.line_of(if last > first && t[..last].ends_with('\n') { last - 1 } else { last });
            (self.buf.line_of(first), l1)
        };
        match s {
            "v" | "V" => {
                let mode = if s == "v" { Mode::Visual } else { Mode::Line };
                if self.vim.mode == mode {
                    let at = self.vim.cursor;
                    self.vim_normal();
                    self.vim_go(at);
                } else {
                    self.vim.mode = mode;
                    self.vim_show();
                }
            }
            "o" | "O" => {
                std::mem::swap(&mut self.vim.anchor, &mut self.vim.cursor);
                self.vim_show();
            }
            "J" | "gJ" => {
                let (l0, l1) = lines();
                self.vim.mode = Mode::Normal;
                let at = self.buf.starts[l0];
                self.vim_join(at, (l1 - l0).max(1), s == "gJ");
            }
            ":" => {
                let range = lines();
                self.vim_normal();
                self.vim_go(self.vim.cursor);
                self.vim.command = Some((String::new(), Some(range)));
            }
            "I" | "A" if self.vim.mode == Mode::Block => {
                let t = &self.buf.text;
                let (ca, cc) = (column(t, self.vim.anchor), column(t, self.vim.cursor));
                let col = if s == "I" { ca.min(cc) } else { ca.max(cc) + 1 };
                let (l0, l1) = (self.buf.line_of(first), self.buf.line_of(last));
                let mut at: Vec<(usize, usize)> = (l0..=l1)
                    .map(|l| {
                        let p = at_column(t, self.buf.starts[l], col);
                        (p, p)
                    })
                    .collect();
                let (p, _) = at.pop().unwrap();
                self.vim_insert(p);
                self.extra_cursors = at;
            }
            "I" | "A" => {
                let at = if s == "I" { first } else { last };
                self.vim_insert(at);
            }
            "p" | "P" => {
                let Some((text, reg_lines)) = self.vim.registers.get(&register.unwrap_or('"')).cloned() else {
                    return self.vim_normal();
                };
                let old = self.buf.text[first..last].to_string();
                let mode = self.vim.mode;
                self.vim_normal();
                let text = if reg_lines && mode != Mode::Line { format!("\n{}", text) } else { text };
                if self.edit(first, last, &text, EditKind::Other) {
                    self.vim_store(None, old, mode == Mode::Line, false);
                    self.vim_go(first);
                }
            }
            "*" | "#" | "n" | "N" => {
                let t = self.buf.text[first..last].to_string();
                self.vim_normal();
                self.vim_go(first);
                self.vim.request = Some(match s {
                    "n" => Request::Next(false),
                    "N" => Request::Next(true),
                    _ => Request::Word(t, s == "#"),
                });
            }
            _ if s.starts_with('r') => {
                let ch = s[1..].chars().next().unwrap_or(' ');
                self.vim.mode = Mode::Normal;
                for &(a, b) in ranges.iter().rev() {
                    let new: String = self.buf.text[a..b].chars().map(|c| if c == '\n' { c } else { ch }).collect();
                    self.edit(a, b, &new, EditKind::Other);
                }
                self.vim_go(first);
            }
            _ => {}
        }
    }

    fn vim_join(&mut self, at: usize, n: usize, plain: bool) {
        if let Some((a, b, text, join)) = joined(&self.buf.text, at, n, plain)
            && self.edit(a, b, &text, EditKind::Other)
        {
            self.vim_go(a + join);
        }
    }

    fn vim_paste(&mut self, register: Option<char>, before: bool, n: usize) {
        let r = register.unwrap_or('"');
        let cur = self.buf.cur;
        if r == '+' || r == '*' {
            let at = if before || cur >= line_end(&self.buf.text, cur) { cur } else { editor::next(&self.buf.text, cur) };
            self.select(at, at);
            self.vim.request = Some(Request::Paste);
            return;
        }
        let Some((text, lines)) = self.vim.registers.get(&r).cloned() else {
            self.note = Some(format!("Register {r} is empty"));
            return;
        };
        let text = text.repeat(n);
        let t = &self.buf.text;
        if lines {
            let body = if text.ends_with('\n') { text.clone() } else { format!("{text}\n") };
            let (at, ins) = if before {
                (line_start(t, cur), body)
            } else {
                let e = line_end(t, cur);
                if e < t.len() { (e + 1, body) } else { (e, format!("\n{}", body.trim_end_matches('\n'))) }
            };
            let start = if ins.starts_with('\n') && !before { at + 1 } else { at };
            if self.edit(at, at, &ins, EditKind::Other) {
                self.vim_go(first_non_blank(&self.buf.text, start));
            }
        } else {
            let at = if before || cur >= line_end(t, cur) { cur } else { editor::next(t, cur) };
            if self.edit(at, at, &text, EditKind::Other) {
                let end = at + text.len();
                self.vim_go(if text.contains('\n') { at } else { editor::prev(&self.buf.text, end).max(at) });
            }
        }
    }

    fn vim_other(&mut self, s: &str, register: Option<char>, count: Option<usize>) {
        if self.vim_extended(s, count) { return; }
        let n = count.unwrap_or(1).max(1);
        let cur = self.buf.cur;
        let t = &self.buf.text;
        match s {
            "u" => {
                for _ in 0..n {
                    if self.multi_undo(false) { break; }
                    match self.buf.undo() {
                        Some(c) => self.changed(c),
                        None => break,
                    }
                }
                let c = self.buf.cur.min(self.buf.text.len());
                self.vim_go(c);
            }
            "p" | "P" => self.vim_paste(register, s == "P", n),
            "J" | "gJ" => self.vim_join(cur, (n - 1).max(1), s == "gJ"),
            "~" => {
                let e = line_end(t, cur);
                let b = cur + t[cur..e].chars().take(n).map(char::len_utf8).sum::<usize>();
                let new = recase(&t[cur..b], Op::Toggle);
                if b > cur && self.edit(cur, b, &new, EditKind::Other) {
                    self.vim_go(b.min(last_char(&self.buf.text, b)));
                }
            }
            "i" | "a" | "I" | "A" => {
                let e = line_end(t, cur);
                let at = match s {
                    "a" if cur < e => editor::next(t, cur),
                    "A" => e,
                    "I" => first_non_blank(t, cur),
                    _ => cur,
                };
                self.vim_insert(at);
                self.vim.insert_count = n;
            }
            "o" | "O" => {
                // The new line takes the indentation, list marker or
                // environment the editor's Enter would give it.
                let ls = line_start(t, cur);
                if s == "o" {
                    let e = line_end(t, cur);
                    self.select(e, e);
                    self.newline();
                } else if ls == 0 {
                    let indent = t[..first_non_blank(t, 0)].to_string();
                    if self.edit(0, 0, &format!("{indent}\n"), EditKind::Other) {
                        self.select(indent.len(), indent.len());
                    }
                } else {
                    let e = ls - 1;
                    self.select(e, e);
                    self.newline();
                }
                let at = self.buf.cur;
                self.vim_insert(at);
                self.vim.insert_count = n;
                self.vim.opened_line = true;
            }
            "R" => {
                self.vim_begin_insert(Mode::Replace, 1);
            }
            "v" => self.vim_enter_visual(Mode::Visual),
            "V" => self.vim_enter_visual(Mode::Line),
            "n" | "N" => self.vim.request = Some(Request::Next(s == "N")),
            "*" | "#" => {
                let (a, b) = editor::word_at(t, cur);
                if b > a {
                    let word = t[a..b].to_string();
                    self.vim_go(a);
                    self.vim.request = Some(Request::Word(word, s == "#"));
                }
            }
            "/" | "?" => self.vim.request = Some(Request::Search(s == "?")),
            ":" => {
                let range = count.map(|n| {
                    let l = self.buf.line_of(cur);
                    (l, (l + n - 1).min(self.buf.lines() - 1))
                });
                self.vim.command = Some((String::new(), range));
            }
            _ if s.starts_with('m') => {
                if let Some(c) = s[1..].chars().next() {
                    let line = self.buf.line_of(cur);
                    self.vim.marks.insert(c, (line, column(t, cur)));
                }
            }
            _ if s.starts_with('r') => {
                let ch = s[1..].chars().next().unwrap_or(' ');
                let e = line_end(t, cur);
                let chars: Vec<usize> = t[cur..e].char_indices().map(|(i, _)| cur + i).take(n).collect();
                if chars.len() == n {
                    let b = chars.last().map_or(cur, |&p| editor::next(t, p));
                    let new = if ch == '\n' { "\n".to_string() } else { ch.to_string().repeat(n) };
                    if self.edit(cur, b, &new, EditKind::Other) {
                        let at = if ch == '\n' { cur + 1 } else { cur + new.len() - ch.len_utf8() };
                        self.vim_go(at);
                    }
                }
            }
            _ => {}
        }
    }

    /// An Ex command typed after `:`. Lines, substitutions and clearing
    /// the search are done here; writing, closing and opening go to the
    /// window.
    fn vim_ex(&mut self, line: &str, range: Option<(usize, usize)>) {
        let line = line.trim();
        let here = self.buf.line_of(self.buf.cur);
        let lines = self.buf.lines();
        // A range first: %, a line number, or two of them with a comma.
        let spec_len = line.find(|c: char| !(c.is_ascii_digit() || ".$%,+-".contains(c))).unwrap_or(line.len());
        let (spec, rest) = line.split_at(spec_len);
        let rest = rest.trim_start();
        if rest.is_empty() && !spec.is_empty() {
            if let Some((_, l)) = ex_range(spec, here, lines) {
                let at = first_non_blank(&self.buf.text, self.buf.starts[l]);
                self.vim_go(at);
                self.vim_settle();
            }
            return;
        }
        if rest.starts_with(['g','v']) && rest.len()>1 {
            let span = if spec.is_empty() { range.unwrap_or((0,lines-1)) } else {
                match ex_range(spec,here,lines) { Some(r)=>r, None=>return self.note=Some("Invalid range".into()) }
            };
            return self.vim_global(rest,span);
        }
        if rest.starts_with('s') && rest.len() > 1 && !rest[1..].starts_with(|c: char| c.is_alphabetic()) {
            let span = if spec.is_empty() { range.unwrap_or((here, here)) } else {
                match ex_range(spec, here, lines) {
                    Some(r) => r,
                    None => return self.note = Some(format!("Not a range: {spec}")),
                }
            };
            return self.vim_substitute(rest, span);
        }
        if spec.is_empty() && !rest.is_empty() {
            self.vim.request = Some(Request::Ex(rest.to_string()));
        }
    }

    fn vim_substitute(&mut self, command: &str, (l0, l1): (usize, usize)) {
        let Some((pat, rep, flags)) = substitution(command) else {
            self.note = Some("Write it as :s/pattern/replacement/g".into());
            return;
        };
        let re = match regex::RegexBuilder::new(&pat).case_insensitive(flags.contains('i') && !flags.contains('I')).size_limit(4 << 20).build() {
            Ok(re) => re,
            Err(e) => return self.note = Some(format!("Not a pattern: {e}")),
        };
        if flags.chars().any(|c| !"giIenp#l".contains(c)) {
            self.note = Some("Supported substitution flags: g i I e n p # l".into()); return;
        }
        if flags.contains('n') {
            let count: usize = (l0..=l1).map(|l| {
                let (a,b)=self.buf.line(l);
                let hits=re.find_iter(&self.buf.text[a..b]).count();
                if flags.contains('g') { hits } else { hits.min(1) }
            }).sum();
            self.note=Some(format!("{count} matches")); return;
        }
        let rep = replacement(&rep);
        let (start, end) = (self.buf.starts[l0], self.buf.line(l1).1);
        let mut changed = 0;
        let mut last = l0;
        let block: Vec<String> = self.buf.text[start..end]
            .split('\n')
            .enumerate()
            .map(|(i, line)| {
                let new = if flags.contains('g') { re.replace_all(line, rep.as_str()) } else { re.replace(line, rep.as_str()) };
                if re.is_match(line) {
                    changed += 1;
                    last = l0 + i;
                }
                new.into_owned()
            })
            .collect();
        if changed == 0 {
            if !flags.contains('e') { self.note = Some(format!("Pattern not found: {pat}")); }
            return;
        }
        let block = block.join("\n");
        let checkpoint = self.buf.checkpoint();
        if self.edit(start, end, &block, EditKind::Other) {
            let last = last.min(self.buf.lines() - 1);
            let at = first_non_blank(&self.buf.text, self.buf.starts[last]);
            self.vim_go(at);
            if flags.contains('p') || flags.contains('#') || flags.contains('l') {
                let (a,b)=self.buf.line(last);
                self.note=Some(format!("{}{}{}",if flags.contains('#') {format!("{} ",last+1)} else {String::new()}, &self.buf.text[a..b],if flags.contains('l') {"$"} else {""}));
            } else if changed > 1 {
                self.note = Some(format!("Substituted on {changed} lines"));
            }
        }
        self.buf.finish(checkpoint);
        self.vim_settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(text: &str) -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text(text.into(), Doc::Code);
        p.vim.enabled = true;
        p
    }

    /// Types keys, with `<esc>`, `<cr>`, `<bs>` and `<c-r>` for those keys.
    fn keys(p: &mut Pane, s: &str) {
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            let named = [
                ("<esc>", Key::Escape, Mods::default()),
                ("<cr>", Key::Enter, Mods::default()),
                ("<bs>", Key::Backspace, Mods::default()),
                ("<c-r>", Key::Char('r'), Mods { ctrl: true, ..Mods::default() }),
                ("<c-v>", Key::Char('v'), Mods { ctrl: true, ..Mods::default() }),
                ("<c-d>", Key::Char('d'), Mods { ctrl: true, ..Mods::default() }),
            ];
            if let Some((name, key, mods)) = named.iter().find(|n| rest.starts_with(n.0)) {
                p.key(*key, *mods, None);
                rest = &rest[name.len()..];
                continue;
            }
            p.key(Key::Char(c.to_ascii_lowercase()), Mods::default(), Some(&c.to_string()));
            rest = &rest[c.len_utf8()..];
        }
    }

    fn run(text: &str, s: &str) -> Pane {
        let mut p = pane(text);
        keys(&mut p, s);
        p
    }

    fn at(text: &str, s: &str) -> usize {
        run(text, s).buf.cur
    }

    #[test]
    fn macros_record_append_replay_and_share_registers() {
        assert_eq!(run("abcdef", "qaxq2@a@@").buf.text, "ef");
        assert_eq!(run("abcdef", "qaxqqAxq@a").buf.text, "ef");
        assert_eq!(run("abc", "qaxq\"ap").buf.text, "bxc");
        assert_eq!(run("x\nabc", "\"ayyj@a").buf.text, "x\nbc");
        let p = run("abc", "qa@aq@a");
        assert!(p.note.as_deref().is_some_and(|s| s.contains("limit")));
        assert_eq!(run("abc", "qaiz<esc>q@a").buf.text, "zzabc");
    }

    #[test]
    fn delete_and_yank_registers_rotate_independently() {
        let p = run("a\nb\nc\nd", "yyddxdd");
        assert_eq!(p.vim.registers[&'0'].0, "a\n");
        assert_eq!(p.vim.registers[&'1'].0, "\n");
        assert_eq!(p.vim.registers[&'2'].0, "a\n");
        assert_eq!(p.vim.registers[&'-'].0, "b");
        assert_eq!(run("abc", "x\"-p").buf.text, "bac");
        assert_eq!(run("a\nb\nc", "dddd\"2P").buf.text, "a\nc");
    }

    #[test]
    fn visual_changes_repeat_and_reselect() {
        assert_eq!(run("abcdef", "vld.").buf.text, "ef");
        assert_eq!(run("a\nb\nc\nd\ne", "Vjd.").buf.text, "e");
        assert_eq!(run("abcdef", "vlcX<esc>l.").buf.text, "XXef");
        assert_eq!(run("abcdef", "vl<esc>gvU").buf.text, "ABcdef");
        assert_eq!(at("abc def", "xwxg;g;"), 0);
        assert_eq!(at("abc def", "xwxg;g;g,"), 3);
        assert_eq!(run("abc\nabc\nabc\nabc", "<c-v>jldjj.").buf.text,"c\nc\nc\nc");
        assert_eq!(run("abc\ndef\nghi\njkl", "vjld.").buf.text,"i\njkl");
    }

    #[test]
    fn accepted_completion_repeats_its_edit() {
        let mut p = run("foo foo", "ic");
        let before = p.buf.text.clone();
        let cur = p.buf.cur;
        p.edit(0, cur, "completion", EditKind::Other);
        p.vim_completion(&before, cur);
        keys(&mut p, "<esc>w.");
        assert_eq!(p.buf.text, "completionfoo completionfoo");
    }

    #[test]
    fn ex_global_flags_and_relative_ranges() {
        assert_eq!(run("aa\naa\naa",":2,$s/a/b/g<cr>").buf.text,"aa\nbb\nbb");
        assert_eq!(run("aa\naa\naa",":.+1,$-1s/a/b/g<cr>").buf.text,"aa\nbb\naa");
        assert_eq!(run("A a",":s/a/b/giI<cr>").buf.text,"A b");
        let p=run("aa\naa",":%s/a/b/gn<cr>"); assert_eq!(p.buf.text,"aa\naa"); assert_eq!(p.note.as_deref(),Some("4 matches"));
        assert!(run("aa",":s/z/b/e<cr>").note.is_none());
        assert_eq!(run("aa",":s/a/b/p<cr>").note.as_deref(),Some("ba"));
        assert_eq!(run("aa",":s/a/b/#<cr>").note.as_deref(),Some("1 ba"));
        assert_eq!(run("aa",":s/a/b/l<cr>").note.as_deref(),Some("ba$"));
        assert_eq!(run("a\nb\na",":g/a/d<cr>").buf.text,"b");
        assert_eq!(run("a\nb\na",":v/a/d<cr>").buf.text,"a\na");
        assert_eq!(run("a\nb\na",":g!/a/d<cr>").buf.text,"a\na");
        assert_eq!(run("a\nb\na",":2,3g/a/d<cr>").buf.text,"a\nb");
        assert_eq!(run("a\nb\na",":g/a/s/a/x/<cr>").buf.text,"x\nb\nx");
        assert_eq!(run("a\nb\na",":g/a/d<cr>u").buf.text,"a\nb\na");
        assert_eq!(run("a\nb\na",":g/a/y<cr>").vim.registers[&'0'].0,"a\na\n");
        assert_eq!(run("a\nb\na",":g/a/p<cr>").note.as_deref(),Some("a\na"));
    }

    #[test]
    fn scrolling_commands_counts_and_screen_motions() {
        let text=(0..100).map(|n|format!("line {n}")).collect::<Vec<_>>().join("\n");
        for (key,down) in [('e',true),('y',false),('d',true),('u',false),('f',true),('b',false)] {
            let mut p=pane(&text); p.set_rect(0.0,0.0,800.0,300.0,1.0);
            keys(&mut p,"50Gzz");let top=p.top;
            keys(&mut p,"2");p.key(Key::Char(key),Mods{ctrl:true,..Mods::default()},None);
            assert!(if down {p.top>top} else {p.top<top},"Ctrl+{key}: {} vs {top}",p.top);
        }
        for command in ["zt","zz","zb","z<cr>","z.","z-"] {
            let mut p=pane(&text); p.set_rect(0.0,0.0,800.0,300.0,1.0); keys(&mut p,"50G");keys(&mut p,command);
            let at=p.buf.cur; assert_eq!(p.buf.line_of(at),49, "{command}");
            assert!(p.top<=49,"{command}");
        }
        let mut p=pane(&text);p.set_rect(0.0,0.0,800.0,300.0,1.0);keys(&mut p,"50GzzH");let top=p.buf.line_of(p.buf.cur);
        keys(&mut p,"L");let bottom=p.buf.line_of(p.buf.cur);assert!(bottom>top);
        keys(&mut p,"M");let middle=p.buf.line_of(p.buf.cur);assert!(middle>=top && middle<=bottom);
    }

    #[test]
    fn parser_reads_counts_registers_operators_and_objects() {
        let done = |keys| match parse(keys, false) {
            Parse::Done(c) => c,
            other => panic!("{keys}: {other:?}"),
        };
        assert_eq!(done("2d3w").count, Some(6));
        assert_eq!(done("2d3w").action, Action::Operate(Op::Delete, Target::Motion(Motion::WordNext(false))));
        assert_eq!(done("\"a5yy").register, Some('a'));
        assert_eq!(done("\"a5yy").action, Action::Operate(Op::Yank, Target::Lines));
        assert_eq!(done("ci(").action, Action::Operate(Op::Change, Target::Object(false, '(')));
        assert_eq!(done("gUU").action, Action::Operate(Op::Upper, Target::Lines));
        assert_eq!(done("gcc").action, Action::Operate(Op::Comment, Target::Lines));
        assert_eq!(done("gg").action, Action::Move(Motion::FirstLine));
        assert_eq!(done("dfx").action, Action::Operate(Op::Delete, Target::Motion(Motion::Find { ch: 'x', forward: true, till: false })));
        assert_eq!(done("0").action, Action::Move(Motion::LineStart));
        assert_eq!(done("10j").count, Some(10));
        assert_eq!(done("ra").action, Action::Other("ra".into()));
        assert_eq!(done("x").action, Action::Operate(Op::Delete, Target::Motion(Motion::Right)));
        for pending in ["d", "2", "\"", "\"a", "g", "di", "f", "gu", "r", "'"] {
            assert_eq!(parse(pending, false), Parse::Pending, "{pending}");
        }
        for invalid in ["Q", "dQ", "diQ", "gq"] {
            assert_eq!(parse(invalid, false), Parse::Invalid, "{invalid}");
        }
        assert!(matches!(parse("iw", true), Parse::Done(Command { action: Action::Select(false, 'w'), .. })));
        assert!(matches!(parse("d", true), Parse::Done(Command { action: Action::Operate(Op::Delete, Target::Selection(false)), .. })));
    }

    #[test]
    fn word_motions_follow_vim() {
        let t = "foo.bar  baz\n\nqux";
        assert_eq!(word_next(t, 0, false), 3);
        assert_eq!(word_next(t, 3, false), 4);
        assert_eq!(word_next(t, 0, true), 9);
        assert_eq!(word_next(t, 9, false), 13, "an empty line is a word");
        assert_eq!(word_end(t, 0, false), 2);
        assert_eq!(word_end(t, 2, false), 3);
        assert_eq!(word_end(t, 0, true), 6);
        assert_eq!(word_back(t, 9, false), 4);
        assert_eq!(word_back(t, 14, false), 13);
        assert_eq!(word_back(t, 4, true), 0);
        assert_eq!(word_next("é abc", 0, false), 3);
    }

    #[test]
    fn line_motions_and_finds() {
        let t = "  one(two, three)\nx";
        assert_eq!(first_non_blank(t, 9), 2);
        assert_eq!(find_char(t, 2, ',', true, false, 1, false), Some(9));
        assert_eq!(find_char(t, 2, ',', true, true, 1, false), Some(8));
        assert_eq!(find_char(t, 9, 'o', false, false, 1, false), Some(8));
        assert_eq!(find_char(t, 2, 'x', true, false, 1, false), None, "f stays on its line");
        assert_eq!(bracket(t, 0), Some(16));
        assert_eq!(bracket(t, 16), Some(5));
        assert_eq!(vertical("abcd\nx\nabcd", 3, 2, true, 3), 10);
        assert_eq!(vertical("abcd\nx\nabcd", 3, 1, true, 3), 5);
        assert_eq!(paragraph("a\nb\n\nc\n\nd", 0, true, 1), 4);
        assert_eq!(paragraph("a\nb\n\nc\n\nd", 0, true, 2), 7);
        assert_eq!(paragraph("a\nb\n\nc\n\nd", 9, false, 1), 7);
        assert_eq!(at("one two three", "w"), 4);
        assert_eq!(at("one two three", "2w"), 8);
        assert_eq!(at("one two three", "$"), 12);
        assert_eq!(at("one two three", "$0"), 0);
        assert_eq!(at("a b a b", "fb;"), 6);
        assert_eq!(at("a b a b", "$Fa,"), 4);
        assert_eq!(at("abc\nx\nabc", "lljj"), 8, "the column is kept past a short line");
        assert_eq!(at("abc\nx\nabc", "G"), 6);
        assert_eq!(at("abc\nx\nabc", "G2G"), 4);
        assert_eq!(at("abc\nx\nabc", "Ggg"), 0);
        assert_eq!(at("a (b) c", "%"), 4);
        assert_eq!(at("a (b) c", "fb%"), 2);
    }

    #[test]
    fn text_objects() {
        assert!(matches!(parse("dif", false), Parse::Done(Command { action: Action::Operate(Op::Delete, Target::Object(false, 'f')), .. })));
        assert!(matches!(parse("dac", false), Parse::Done(Command { action: Action::Operate(Op::Delete, Target::Object(true, 'c')), .. })));
        assert!(matches!(parse("daa", false), Parse::Done(Command { action: Action::Operate(Op::Delete, Target::Object(true, 'a')), .. })));
        let t = "say(\"hi there\", [1, (2)])";
        assert_eq!(object(t, 7, false, '"', 1), Some((5, 13, false)));
        assert_eq!(object(t, 7, true, '"', 1), Some((4, 14, false)));
        assert_eq!(object(t, 7, false, '(', 1), Some((4, 24, false)));
        assert_eq!(object(t, 21, false, '(', 1), Some((21, 22, false)));
        assert_eq!(object(t, 21, false, '(', 2), Some((4, 24, false)));
        assert_eq!(object(t, 18, true, '[', 1), Some((16, 24, false)));
        assert_eq!(object("one two  three", 4, true, 'w', 1), Some((4, 9, false)));
        assert_eq!(object("one two", 4, true, 'w', 1), Some((3, 7, false)));
        assert_eq!(object("a.b c", 0, false, 'W', 1), Some((0, 3, false)));
        assert_eq!(object("<a href>", 3, false, '<', 1), Some((1, 7, false)));
        assert_eq!(object("f {\n    x\n}", 9, false, '{', 1), Some((4, 9, false)));
        assert_eq!(object("a\nb\n\nc", 0, false, 'p', 1), Some((0, 3, true)));
        assert_eq!(object("a\nb\n\nc", 0, true, 'p', 1), Some((0, 4, true)));
    }

    #[test]
    fn operators_with_motions_objects_and_counts() {
        assert_eq!(run("one two three four", "2dw").buf.text, "three four");
        assert_eq!(run("one two three four", "d2w").buf.text, "three four");
        assert_eq!(run("one two", "dw").buf.text, "two");
        assert_eq!(run("one\ntwo", "wdw").buf.text, "one\n", "dw on the last word keeps the line break");
        assert_eq!(run("one two", "cwxx<esc>").buf.text, "xx two");
        assert_eq!(run("a\nb\nc\nd", "2dd").buf.text, "c\nd");
        assert_eq!(run("a\nb\nc", "Gdd").buf.text, "a\nb");
        assert_eq!(run("a\nb\nc", "dj").buf.text, "c");
        assert_eq!(run("one two", "d$").buf.text, "");
        assert_eq!(run("one two", "wD").buf.text, "one ");
        assert_eq!(run("f(a, b)", "fadi(").buf.text, "f()");
        assert_eq!(run("f(a, b)", "fada(").buf.text, "f");
        assert_eq!(run("x = \"hi\"", "ci\"yo<esc>").buf.text, "x = \"yo\"");
        assert_eq!(run("one two", "gUiw").buf.text, "ONE two");
        assert_eq!(run("One Two", "g~~").buf.text, "oNE tWO");
        assert_eq!(run("ab\ncd", "guuU").buf.text, "ab\ncd");
        assert_eq!(run("a\nb", ">>").buf.text, "\ta\nb");
        assert_eq!(run("a\nb", "2>>").buf.text, "\ta\n\tb");
        assert_eq!(run("\t\ta", "<<").buf.text, "\ta");
        assert_eq!(run("abc", "x").buf.text, "bc");
        assert_eq!(run("abc", "$X").buf.text, "ac");
        assert_eq!(run("abc", "3x").buf.text, "");
        assert_eq!(run("abc", "rz").buf.text, "zbc");
        assert_eq!(run("abc", "2rz").buf.text, "zzc");
        assert_eq!(run("abc", "~~").buf.text, "ABc");
        assert_eq!(run("abc def", "sX<esc>").buf.text, "Xbc def");
        assert_eq!(run("  abc\nx", "Sy<esc>").buf.text, "  y\nx");
        assert_eq!(run("one two", "wCz<esc>").buf.text, "one z");
        assert_eq!(run("a\n  b\nc", "J").buf.text, "a b\nc");
        assert_eq!(run("a\nb\nc", "3J").buf.text, "a b c");
        assert_eq!(run("a\n b", "gJ").buf.text, "a b");
        assert_eq!(run("a\nb\n\nc", "dap").buf.text, "c");
        assert_eq!(run("a\nb\n\nc", "dip").buf.text, "\nc");
    }

    #[test]
    fn insert_commands_and_counts() {
        assert_eq!(run("abc", "ix<esc>").buf.text, "xabc");
        assert_eq!(run("abc", "ax<esc>").buf.text, "axbc");
        assert_eq!(run("  abc", "$Ix<esc>").buf.text, "  xabc");
        assert_eq!(run("abc", "Ax<esc>").buf.text, "abcx");
        assert_eq!(run("abc", "3ix<esc>").buf.text, "xxxabc");
        assert_eq!(run("a\nb", "ox<esc>").buf.text, "a\nx\nb");
        assert_eq!(run("a\nb", "jOx<esc>").buf.text, "a\nx\nb");
        assert_eq!(run("a", "Ox<esc>").buf.text, "x\na");
        assert_eq!(run("  a", "ox<esc>").buf.text, "  a\n  x");
        assert_eq!(run("a", "2ox<esc>").buf.text, "a\nx\nx");
        let p = run("abc", "ix<esc>");
        assert_eq!(p.buf.cur, 0, "Esc steps back onto what was typed");
        assert_eq!(p.vim_label(), Some("NORMAL"));
    }

    #[test]
    fn replace_mode_overwrites_and_restores() {
        assert_eq!(run("abc", "Rxy<esc>").buf.text, "xyc");
        assert_eq!(run("ab", "Rxyz<esc>").buf.text, "xyz");
        assert_eq!(run("abc", "Rxy<bs><esc>").buf.text, "xbc");
        assert_eq!(run("abc", "Rxy<esc>u").buf.text, "abc");
    }

    #[test]
    fn registers_and_paste() {
        assert_eq!(run("a\nb\nc", "ddp").buf.text, "b\na\nc");
        assert_eq!(run("a\nb", "yyP").buf.text, "a\na\nb");
        assert_eq!(run("a\nb", "yy2p").buf.text, "a\na\na\nb");
        assert_eq!(run("a\nb", "jyyp").buf.text, "a\nb\nb");
        assert_eq!(run("one two", "dwP").buf.text, "one two");
        assert_eq!(run("one two", "ywP").buf.text, "one one two");
        assert_eq!(run("ab", "xp").buf.text, "ba");
        // Named registers keep what the unnamed one moves on from.
        assert_eq!(run("a\nb\nc", "\"ayyjdd\"ap").buf.text, "a\nc\na");
        assert_eq!(run("a\nb", "\"ayyj\"Ayy\"aP").buf.text, "a\na\nb\nb");
        assert_eq!(run("a b", "\"_dwP").buf.text, "b", "the black hole keeps nothing");
        let p = run("one", "\"+yiw");
        assert_eq!(p.copy.as_deref(), Some("one"));
        let mut p = run("one", "\"+p");
        assert_eq!(p.vim.request.take(), Some(Request::Paste));
    }

    #[test]
    fn each_command_is_one_undo_step() {
        let p = run("one two three", "dwu");
        assert_eq!(p.buf.text, "one two three");
        let p = run("one two three", "ciwnew<esc>");
        assert_eq!(p.buf.text, "new two three");
        let p = run("one two three", "ciwnew word<esc>u");
        assert_eq!(p.buf.text, "one two three");
        let p = run("a\nb", "oone two<esc>u");
        assert_eq!(p.buf.text, "a\nb");
        let p = run("one two three", "dwdwuu<c-r>");
        assert_eq!(p.buf.text, "two three");
        let p = run("a(", "A)b<esc>u");
        assert_eq!(p.buf.text, "a(");
        let p = run("abc", "3ix<esc>u");
        assert_eq!(p.buf.text, "abc");
    }

    #[test]
    fn dot_repeats_changes_with_their_text() {
        assert_eq!(run("one two three four", "dw.").buf.text, "three four");
        assert_eq!(run("one two three four", "dw2.").buf.text, "four");
        assert_eq!(run("a b c", "ciwx<esc>w.w.").buf.text, "x x x");
        assert_eq!(run("a\nb\nc", "A;<esc>j.j.").buf.text, "a;\nb;\nc;");
        assert_eq!(run("a\nb\nc", "ddj.").buf.text, "b");
        let p = run("a b c", "ciwx<esc>w.u");
        assert_eq!(p.buf.text, "x b c", "a repeat is one undo step");
        assert_eq!(run("abc", "x.").buf.text, "c");
    }

    #[test]
    fn visual_modes_select_and_operate() {
        assert_eq!(run("one two", "vld").buf.text, "e two");
        assert_eq!(run("one two", "vey$p").buf.text, "one twoone");
        assert_eq!(run("one two", "wvbd").buf.text, "wo");
        assert_eq!(run("a\nb\nc", "Vjd").buf.text, "c");
        assert_eq!(run("a\nb\nc", "jVd").buf.text, "a\nc");
        assert_eq!(run("a\nb", "Vj>").buf.text, "\ta\n\tb");
        assert_eq!(run("one two", "viwU").buf.text, "ONE two");
        assert_eq!(run("one two", "viwcX<esc>").buf.text, "X two");
        assert_eq!(run("abc", "vlrx").buf.text, "xxc");
        assert_eq!(run("a\nb\nc", "VjJ").buf.text, "a b\nc");
        assert_eq!(run("one two", "vlo").vim.cursor, 0);
        assert_eq!(run("one two", "vl<esc>").vim_label(), Some("NORMAL"));
        let p = run("one two", "vl");
        assert_eq!((p.buf.anchor, p.buf.cur), (0, 2), "the character under the cursor is selected");
    }

    #[test]
    fn visual_block_uses_the_rectangular_selection() {
        assert_eq!(run("abc\nabc\nabc", "<c-v>jjld").buf.text, "c\nc\nc");
        assert_eq!(run("abc\nabc", "<c-v>jIx<esc>").buf.text, "xabc\nxabc");
        assert_eq!(run("abc\nabc", "<c-v>jAx<esc>").buf.text, "axbc\naxbc");
        assert_eq!(run("abc\nabc", "l<c-v>jcZ<esc>").buf.text, "aZc\naZc");
        let p = run("abc\nabc", "<c-v>jly");
        assert_eq!(p.vim.registers[&'"'].0, "ab\nab");
        assert_eq!(p.vim_label(), Some("NORMAL"));
    }

    #[test]
    fn selections_from_ctrl_d_take_vim_operators() {
        let mut p = pane("ab ab ab"); p.add_next_cursor(); p.add_next_cursor(); keys(&mut p, "cX<esc>");
        assert_eq!(p.buf.text, "X X ab");
        assert_eq!(p.vim_label(), Some("NORMAL"));
        let mut p = pane("ab ab ab"); p.add_next_cursor(); p.add_next_cursor(); p.add_next_cursor(); keys(&mut p, "d"); assert_eq!(p.buf.text, "  ");
        let mut p = pane("ab ab ab"); p.add_next_cursor(); p.add_next_cursor(); keys(&mut p, "cX<esc>u"); assert_eq!(p.buf.text, "ab ab ab");
    }

    #[test]
    fn marks_and_screen_free_jumps() {
        assert_eq!(at("abc\ndef\nghi", "lmajj`a"), 1);
        assert_eq!(at("abc\n  def\nghi", "jllmbgg'b"), 6);
        assert_eq!(run("a\nb\nc\nd", "jmajjd'a").buf.text, "a");
    }

    #[test]
    fn search_requests_go_to_the_window() {
        let mut p = run("foo bar foo", "*");
        assert_eq!(p.vim.request.take(), Some(Request::Word("foo".into(), false)));
        keys(&mut p, "N");
        assert_eq!(p.vim.request.take(), Some(Request::Next(true)));
        keys(&mut p, "?");
        assert_eq!(p.vim.request.take(), Some(Request::Search(true)));
    }

    #[test]
    fn ex_commands() {
        let mut p = run("a\nb\nc", ":wq<cr>");
        assert_eq!(p.vim.request.take(), Some(Request::Ex("wq".into())));
        assert_eq!(at("a\nb\nc", ":3<cr>"), 4);
        assert_eq!(at("a\nb\nc", "G:1<cr>"), 0);
        assert_eq!(run("aa\naa", ":s/a/b/<cr>").buf.text, "ba\naa");
        assert_eq!(run("aa\naa", ":s/a/b/g<cr>").buf.text, "bb\naa");
        assert_eq!(run("aa\naa", ":%s/a/b/g<cr>").buf.text, "bb\nbb");
        assert_eq!(run("aa\naa", ":%s/a/b/g<cr>u").buf.text, "aa\naa", "a substitution is one undo step");
        assert_eq!(run("ab\nab\nab", "jVj:s/b/c/<cr>").buf.text, "ab\nac\nac");
        assert_eq!(run("one two", ":s/(\\w+) (\\w+)/\\2 \\1/<cr>").buf.text, "two one");
        assert_eq!(run("a/b", ":s#/#-#<cr>").buf.text, "a-b");
        assert_eq!(run("ab", ":s/b/[&]/<cr>").buf.text, "a[b]");
        assert_eq!(run("ab", ":s/b<bs>a/x/<cr>").buf.text, "xb");
        let p = run("ab", ":<esc>");
        assert!(p.vim.command.is_none());
        assert_eq!(substitution("s/a\\/b/c/g"), Some(("a/b".into(), "c".into(), "g".into())));
        assert_eq!(replacement("$&\\1"), "$$${0}${1}");
        assert_eq!(ex_range("2,$", 0, 5), Some((1, 4)));
    }

    #[test]
    fn insert_mode_is_the_editors_own() {
        // Automatic pairs, Enter's indentation and Backspace behave as without Vim.
        assert_eq!(run("", "i(<esc>").buf.text, "()");
        assert_eq!(run("  a", "A<cr>b<esc>").buf.text, "  a\n  b");
        assert_eq!(run("ab", "A<bs><esc>").buf.text, "a");
        // Keys in normal mode never type.
        assert_eq!(run("ab", "QZ").buf.text, "ab");
    }

    #[test]
    fn read_only_files_and_notebook_cells_are_protected() {
        let mut p = pane("a\nb\nc");
        p.read_only = true;
        keys(&mut p, "ddxp");
        assert_eq!(p.buf.text, "a\nb\nc");
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        let header = cells::header(Tag::Code, "");
        p.set_text(format!("{header}\nx = 1\n{header}\ny = 2"), Doc::Cells);
        p.vim.enabled = true;
        keys(&mut p, "ggx");
        assert_eq!(p.buf.text, format!("{header}\n = 1\n{header}\ny = 2"), "the cursor never rests on a header");
        keys(&mut p, "jx");
        assert!(p.buf.text.contains(&header) && p.buf.text.contains("\n = 2"), "{:?}", p.buf.text);
    }

    /// Keys typed into the whole window, as the app receives them.
    fn window(text: &str, s: &str) -> crate::app::App {
        use crate::app::App;
        let mut a = App::new(None);
        a.paste_text(text);
        a.cfg.modal_editing = "vim".into();
        a.doc.vim.enabled = true;
        a.key(Key::Escape, Mods::default(), None);
        a.key(Key::Char('g'), Mods::default(), Some("g"));
        a.key(Key::Char('g'), Mods::default(), Some("g"));
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            let ctrl = Mods { ctrl: true, ..Mods::default() };
            let named = [("<esc>", Key::Escape, Mods::default()), ("<cr>", Key::Enter, Mods::default()), ("<c-r>", Key::Char('r'), ctrl)];
            if let Some((name, key, mods)) = named.iter().find(|n| rest.starts_with(n.0)) {
                a.key(*key, *mods, None);
                rest = &rest[name.len()..];
                continue;
            }
            a.key(Key::Char(c.to_ascii_lowercase()), Mods::default(), Some(&c.to_string()));
            rest = &rest[c.len_utf8()..];
        }
        a
    }

    #[test]
    fn the_window_routes_search_redo_and_escape_to_vim() {
        let a = window("one foo two foo", "/foo<cr>");
        assert_eq!((a.doc.buf.cur, a.doc.buf.anchor), (4, 4), "a search leaves the cursor on the match");
        assert_eq!(a.doc.vim_label(), Some("NORMAL"));
        let a = window("one foo two foo", "/foo<cr>n");
        assert_eq!(a.doc.buf.cur, 12);
        let a = window("one foo two foo", "/foo<cr>nN");
        assert_eq!(a.doc.buf.cur, 4);
        let a = window("one foo two foo", "$?foo<cr>");
        assert_eq!(a.doc.buf.cur, 12);
        let a = window("foo bar foo", "*");
        assert_eq!(a.doc.buf.cur, 8);
        let a = window("foo bar foo", "*dw");
        assert_eq!(a.doc.buf.text, "foo bar ", "keys go back to the document after a search");
        let a = window("one two", "dwu<c-r>");
        assert_eq!(a.doc.buf.text, "two", "Ctrl+R redoes rather than going to a symbol");
        let a = window("one", "ix<esc>");
        assert_eq!((a.doc.buf.text.as_str(), a.doc.vim_label()), ("xone", Some("NORMAL")));
        let a = window("one", "ccx<esc>");
        assert_eq!(a.doc.buf.text, "x", "normal-mode keys do not open completion");
    }
}
