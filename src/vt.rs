//! A terminal's screen: the grid of character cells that the program
//! running in the terminal writes to, with text and escape sequences, and
//! the lines that have scrolled off its top. The sequences are xterm's,
//! which is what the program is told it is talking to; `vte` splits them
//! out of the bytes, and this module carries them out.

use std::collections::VecDeque;

/// Lines kept above the screen once they have scrolled off it.
pub const HISTORY: usize = 5000;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    /// One of the 256 colours of xterm's palette.
    Index(u8),
    Rgb(u8, u8, u8),
}

pub const BOLD: u8 = 1;
pub const DIM: u8 = 2;
pub const ITALIC: u8 = 4;
pub const UNDERLINE: u8 = 8;
pub const INVERSE: u8 = 16;
pub const HIDDEN: u8 = 32;
pub const STRIKE: u8 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: u8,
    /// 2 where a wide character starts, 0 in the cell it spills into, and
    /// 1 everywhere else.
    pub width: u8,
}

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ', fg: Color::Default, bg: Color::Default, attrs: 0, width: 1 };

    /// An erased cell, which keeps the background being written with.
    fn blank(bg: Color) -> Cell {
        Cell { bg, ..Cell::BLANK }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub cells: Vec<Cell>,
    /// The line runs on into the next: it was broken only because the
    /// screen was too narrow for it.
    pub wrapped: bool,
    /// What the shell said of its prompts and commands here.
    pub marks: Vec<Mark>,
}

/// A shell's mark (OSC 133) at a column of a line: `A` where a prompt
/// starts, `B` where the command typed at it starts, `C` where the
/// command's output starts and `D` where the command ended.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub col: usize,
    pub kind: char,
    /// On `D`, the exit status; on `A`, the status of the command run at
    /// that prompt, once it has ended.
    pub code: Option<i32>,
    /// On `C`, the command line, where the shell said what it was.
    pub text: Option<Box<str>>,
}

/// Marks kept on one line, and the longest command line kept.
const MARKS: usize = 16;
const COMMAND: usize = 4096;

impl Line {
    fn new(cols: usize, bg: Color) -> Line {
        Line { cells: vec![Cell::blank(bg); cols], wrapped: false, marks: Vec::new() }
    }

    /// The text of the cells from `from` up to `to`, without the halves
    /// that wide characters spill into.
    pub fn text(&self, from: usize, to: usize) -> String {
        let to = to.min(self.cells.len());
        self.cells.get(from..to).unwrap_or(&[]).iter().filter(|c| c.width > 0).map(|c| c.ch).collect()
    }
}

/// What the program wants to hear of the mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mouse {
    #[default]
    Off,
    /// Presses only (mode 9).
    Press,
    /// Presses and releases (1000).
    Click,
    /// Those, and motion while a button is down (1002).
    Drag,
    /// Those, and all motion (1003).
    Motion,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct Pen {
    fg: Color,
    bg: Color,
    attrs: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    x: usize,
    y: usize,
    pen: Pen,
    /// The last column has been written: the next character goes on the
    /// next line.
    wrap: bool,
    /// Rows count from the top of the scrolling region.
    origin: bool,
    /// Whether G0 and G1 hold the line-drawing set rather than ASCII.
    drawing: [bool; 2],
    /// G1 has been shifted in.
    shifted: bool,
}

pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    /// The screen, top to bottom, `rows` long.
    pub lines: Vec<Line>,
    /// What has scrolled off the top of the main screen, oldest first.
    /// Lines here lose their trailing blanks.
    pub history: VecDeque<Line>,
    /// How many lines have ever been let go from the front of the
    /// history, so that a line can be named by a number that stays its
    /// own as the history moves.
    pub dropped: u64,
    /// Lines that went into the history since this was last cleared.
    pub scrolled: usize,
    /// The main screen, put aside while the alternate one is shown.
    main: Option<Vec<Line>>,
    pub alt: bool,
    cur: Cursor,
    /// Saved cursors, for the main screen and the alternate one.
    saved: [Option<Cursor>; 2],
    /// The scrolling region: its first row, and the row after its last.
    top: usize,
    bottom: usize,
    tabs: Vec<bool>,
    pub app_cursor: bool,
    pub app_keypad: bool,
    autowrap: bool,
    insert: bool,
    pub cursor_visible: bool,
    /// The shape asked for: 0 or 1 or 2 a block, 3 or 4 an underline,
    /// 5 or 6 a bar.
    pub cursor_shape: u8,
    pub bracketed_paste: bool,
    pub mouse: Mouse,
    pub mouse_sgr: bool,
    pub focus_events: bool,
    /// The last character printed, for repeating it.
    last: char,
    pub title: String,
    /// The program's working directory, where it says so.
    pub cwd: Option<String>,
    /// Text the program has put on the clipboard.
    pub clipboard: Option<String>,
    /// The command line the shell said it is about to run, for the next
    /// `C` mark.
    command: Option<Box<str>>,
    pub bell: bool,
    /// What is to be written back to the program: answers to its queries.
    pub reply: Vec<u8>,
}

/// How many cells a character takes: 2 for the wide characters of East
/// Asian scripts and for emoji, 0 for marks that combine with the one
/// before, 1 for the rest.
pub fn width(c: char) -> usize {
    let u = c as u32;
    if u < 0x300 {
        return 1;
    }
    const ZERO: [(u32, u32); 14] = [
        (0x0300, 0x036f),
        (0x0483, 0x0489),
        (0x0591, 0x05bd),
        (0x0610, 0x061a),
        (0x064b, 0x065f),
        (0x0e31, 0x0e31),
        (0x0e34, 0x0e3a),
        (0x1ab0, 0x1aff),
        (0x1dc0, 0x1dff),
        (0x200b, 0x200f),
        (0x20d0, 0x20ff),
        (0xfe00, 0xfe0f),
        (0xfe20, 0xfe2f),
        (0xe0100, 0xe01ef),
    ];
    const WIDE: [(u32, u32); 36] = [
        (0x1100, 0x115f),
        (0x231a, 0x231b),
        (0x2329, 0x232a),
        (0x23e9, 0x23ec),
        (0x23f0, 0x23f0),
        (0x23f3, 0x23f3),
        (0x25fd, 0x25fe),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x267f, 0x267f),
        (0x2693, 0x2693),
        (0x26a1, 0x26a1),
        (0x26aa, 0x26ab),
        (0x26bd, 0x26be),
        (0x26c4, 0x26c5),
        (0x26ce, 0x26ce),
        (0x26d4, 0x26d4),
        (0x26ea, 0x26ea),
        (0x26f2, 0x26f5),
        (0x26fa, 0x26fd),
        (0x2705, 0x2705),
        (0x270a, 0x270b),
        (0x2728, 0x2728),
        (0x274c, 0x274c),
        (0x2753, 0x2757),
        (0x2795, 0x2797),
        (0x27b0, 0x27bf),
        (0x2b1b, 0x2b1c),
        (0x2b50, 0x2b55),
        (0x2e80, 0x303e),
        (0x3041, 0xa4cf),
        (0xac00, 0xd7a3),
        (0xf900, 0xfaff),
        (0xfe30, 0xfe6f),
        (0xff00, 0xff60),
        (0xffe0, 0xffe6),
    ];
    let within = |table: &[(u32, u32)]| table.iter().any(|&(a, b)| (a..=b).contains(&u));
    let emoji = (0x1f300..=0x1f64f).contains(&u)
        || (0x1f680..=0x1faff).contains(&u)
        || ((0x1f000..=0x1f2ff).contains(&u) && !(0x1f100..=0x1f18d).contains(&u));
    if within(&ZERO) {
        0
    } else if within(&WIDE) || emoji || (0x20000..=0x3fffd).contains(&u) {
        2
    } else {
        1
    }
}

/// The DEC line-drawing set, which programs switch to for borders.
fn drawing(c: char) -> char {
    const SET: &str = " ◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·";
    match c {
        '_'..='~' => SET.chars().nth(c as usize - '_' as usize).unwrap_or(c),
        _ => c,
    }
}

fn base64(s: &[u8]) -> Option<Vec<u8>> {
    let value = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for &b in s.iter().filter(|b| !b.is_ascii_whitespace() && **b != b'=') {
        acc = acc << 6 | value(b)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Screen {
        let (cols, rows) = (cols.max(2), rows.max(1));
        Screen {
            cols,
            rows,
            lines: vec![Line::new(cols, Color::Default); rows],
            history: VecDeque::new(),
            dropped: 0,
            scrolled: 0,
            main: None,
            alt: false,
            cur: Cursor::default(),
            saved: [None, None],
            top: 0,
            bottom: rows,
            tabs: (0..cols).map(|i| i % 8 == 0).collect(),
            app_cursor: false,
            app_keypad: false,
            autowrap: true,
            insert: false,
            cursor_visible: true,
            cursor_shape: 0,
            bracketed_paste: false,
            mouse: Mouse::Off,
            mouse_sgr: false,
            focus_events: false,
            last: ' ',
            title: String::new(),
            cwd: None,
            clipboard: None,
            command: None,
            bell: false,
            reply: Vec::new(),
        }
    }

    /// The cursor's column and row.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cur.x, self.cur.y)
    }

    /// How many lines there are, the history's and the screen's.
    pub fn total(&self) -> usize {
        self.history.len() + self.rows
    }

    /// A line of the history or of the screen, counting from the oldest
    /// line kept.
    pub fn row(&self, i: usize) -> Option<&Line> {
        match i.checked_sub(self.history.len()) {
            None => self.history.get(i),
            Some(on) => self.lines.get(on),
        }
    }

    /// Forgets the history and blanks the screen, leaving the cursor's
    /// line where it is at the top.
    pub fn clear(&mut self) {
        self.dropped += self.history.len() as u64;
        self.history.clear();
        let y = self.cur.y;
        if !self.alt && y > 0 {
            self.lines.drain(..y);
            self.lines.resize(self.rows, Line::new(self.cols, Color::Default));
            self.cur.y = 0;
        }
    }

    /// Changes the size of the screen. On the main screen a change of
    /// width wraps the lines anew, history and all. Otherwise lines are cut
    /// or filled out; rows taken off the top go into the history, and rows
    /// added at the top come back from it.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        if cols != self.cols && !self.alt {
            return self.reflow(cols, rows);
        }
        let fit = |line: &mut Line| {
            line.cells.resize(cols, Cell::BLANK);
            if let Some(last) = line.cells.last_mut().filter(|c| c.width == 2) {
                *last = Cell::blank(last.bg);
            }
        };
        self.lines.iter_mut().chain(self.main.iter_mut().flatten()).for_each(fit);
        if let Some(main) = &mut self.main {
            main.resize(rows, Line::new(cols, Color::Default));
        }
        if rows < self.rows {
            // Empty rows under the cursor go first, then rows off the top.
            let mut excess = self.rows - rows;
            while excess > 0 && self.lines.len() > self.cur.y + 1 {
                self.lines.pop();
                excess -= 1;
            }
            for line in self.lines.drain(..excess).collect::<Vec<_>>() {
                if !self.alt {
                    self.keep(line);
                }
            }
            self.cur.y -= excess;
        } else {
            let back = if self.alt { 0 } else { (rows - self.rows).min(self.history.len()) };
            for _ in 0..back {
                let mut line = self.history.pop_back().unwrap();
                line.cells.resize(cols, Cell::BLANK);
                self.lines.insert(0, line);
            }
            self.cur.y += back;
            self.lines.resize(rows, Line::new(cols, Color::Default));
        }
        let old = std::mem::replace(&mut self.cols, cols);
        (self.rows, self.top, self.bottom) = (rows, 0, rows);
        self.tabs.resize(cols, false);
        for (i, t) in self.tabs.iter_mut().enumerate().skip(old) {
            *t = i % 8 == 0;
        }
        let clamp = |c: &mut Cursor| {
            c.x = c.x.min(cols - 1);
            c.y = c.y.min(rows - 1);
            c.wrap = false;
        };
        clamp(&mut self.cur);
        self.saved.iter_mut().flatten().for_each(clamp);
    }

    /// Wraps the main screen and its history anew at another width. Lines
    /// the screen broke are joined and broken again; the cursor stays on
    /// the same character.
    fn reflow(&mut self, cols: usize, rows: usize) {
        let blank = |c: &Cell| c.ch == ' ' && c.bg == Color::Default && c.attrs == 0 && c.width == 1;
        // The screen's rows that hold anything, and the cursor's.
        let used = self.lines.iter().rposition(|l| !l.cells.iter().all(blank)).map_or(0, |i| i + 1);
        let used = used.max(self.cur.y + 1);
        let cursor_row = self.history.len() + self.cur.y;
        let cursor_col = self.cur.x + usize::from(self.cur.wrap);
        let mut old: Vec<Line> = std::mem::take(&mut self.history).into();
        old.extend(self.lines.drain(..used));

        // Whole lines, and where in which one the cursor is.
        let mut logical: Vec<Vec<Cell>> = Vec::new();
        let mut marks: Vec<Vec<Mark>> = Vec::new();
        let mut cursor = (0, 0);
        let mut joining = false;
        for (i, line) in old.iter().enumerate() {
            if !joining {
                logical.push(Vec::new());
                marks.push(Vec::new());
            }
            let index = logical.len() - 1;
            let whole = logical.last_mut().unwrap();
            let at = whole.len();
            marks[index].extend(line.marks.iter().map(|m| Mark { col: at + m.col, ..m.clone() }));
            let mut cells = line.cells.clone();
            // A wide character that did not fit left a blank behind.
            let next_wide = old.get(i + 1).and_then(|n| n.cells.first()).is_some_and(|c| c.width == 2);
            if line.wrapped && next_wide && cells.last().is_some_and(blank) {
                cells.pop();
            }
            if i == cursor_row {
                cursor = (index, whole.len() + cursor_col);
            }
            whole.extend(cells);
            joining = line.wrapped;
        }
        for (i, whole) in logical.iter_mut().enumerate() {
            let keep = if i == cursor.0 { cursor.1 } else { 0 };
            while whole.len() > keep && whole.last().is_some_and(blank) {
                whole.pop();
            }
            if i == cursor.0 && whole.len() < keep {
                whole.resize(keep, Cell::BLANK);
            }
        }

        // Broken again at the new width.
        let mut lines: Vec<Line> = Vec::new();
        let mut at = (0, 0, false);
        for (i, whole) in logical.iter().enumerate() {
            let mut line = Line { cells: Vec::with_capacity(cols), wrapped: false, marks: Vec::new() };
            let mut j = 0;
            let mut offset = 0;
            while j <= whole.len() {
                if i == cursor.0 && j == cursor.1 {
                    at = (lines.len(), line.cells.len(), false);
                }
                let Some(&cell) = whole.get(j) else { break };
                let span = if cell.width == 2 { 2 } else { 1 };
                if line.cells.len() + span > cols {
                    line.cells.resize(cols, Cell::BLANK);
                    line.wrapped = true;
                    lines.push(std::mem::replace(&mut line, Line { cells: Vec::with_capacity(cols), wrapped: false, marks: Vec::new() }));
                    offset = j;
                    if i == cursor.0 && j == cursor.1 {
                        at = (lines.len(), 0, false);
                    }
                }
                let here = |m: &&Mark| (j..j + span).contains(&m.col);
                line.marks.extend(marks[i].iter().filter(here).map(|m| Mark { col: j - offset, ..m.clone() }));
                line.cells.push(cell);
                if span == 2 {
                    line.cells
                        .push(whole.get(j + 1).copied().filter(|c| c.width == 0).unwrap_or(Cell { width: 0, ..cell }));
                    j += 1;
                }
                j += 1;
            }
            // Marks past the text stay at its end.
            let col = (whole.len() - offset).min(cols - 1);
            line.marks.extend(marks[i].iter().filter(|m| m.col >= whole.len()).map(|m| Mark { col, ..m.clone() }));
            // The cursor past the last column waits to wrap.
            if i == cursor.0 && at.1 >= cols {
                at = (lines.len(), cols - 1, true);
            }
            line.cells.resize(cols, Cell::BLANK);
            lines.push(line);
        }

        // The last rows are the screen; the rest is history.
        let first = lines.len().saturating_sub(rows).min(at.0);
        let screen: Vec<Line> = lines.split_off(first);
        for mut line in lines {
            while line.cells.last() == Some(&Cell::BLANK) {
                line.cells.pop();
            }
            self.history.push_back(line);
        }
        while self.history.len() > HISTORY {
            self.history.pop_front();
            self.dropped += 1;
        }
        self.lines = screen;
        self.lines.truncate(rows);
        self.lines.resize(rows, Line::new(cols, Color::Default));
        self.cur.x = at.1.min(cols - 1);
        self.cur.y = (at.0 - first).min(rows - 1);
        self.cur.wrap = at.2;
        let old = std::mem::replace(&mut self.cols, cols);
        (self.rows, self.top, self.bottom) = (rows, 0, rows);
        self.tabs.resize(cols, false);
        for (i, t) in self.tabs.iter_mut().enumerate().skip(old) {
            *t = i % 8 == 0;
        }
        if let Some(saved) = &mut self.saved[0] {
            (saved.x, saved.y, saved.wrap) = (saved.x.min(cols - 1), saved.y.min(rows - 1), false);
        }
    }

    // ---- writing --------------------------------------------------------

    /// Blanks a wide character that a write to this cell would cut in two.
    fn unwide(&mut self, y: usize, x: usize) {
        let cells = &mut self.lines[y].cells;
        match cells[x].width {
            0 if x > 0 => cells[x - 1] = Cell::blank(cells[x - 1].bg),
            2 if x + 1 < cells.len() => cells[x + 1] = Cell::blank(cells[x + 1].bg),
            _ => {}
        }
    }

    fn put(&mut self, c: char) {
        let set = if self.cur.shifted { 1 } else { 0 };
        let c = if self.cur.drawing[set] { drawing(c) } else { c };
        let w = width(c);
        if w == 0 {
            return;
        }
        self.last = c;
        if self.cur.wrap {
            self.cur.wrap = false;
            self.lines[self.cur.y].wrapped = true;
            self.cur.x = 0;
            self.index();
        }
        if w == 2 && self.cur.x + 1 >= self.cols {
            if self.autowrap {
                // It does not fit at the end of the line: it starts the next.
                let (x, y) = (self.cur.x, self.cur.y);
                self.erase(y, x, self.cols);
                self.lines[y].wrapped = true;
                self.cur.x = 0;
                self.index();
            } else {
                self.cur.x = self.cols - 2;
            }
        }
        let (x, y) = (self.cur.x, self.cur.y);
        if self.insert {
            let cells = &mut self.lines[y].cells;
            for _ in 0..w {
                cells.insert(x, Cell::blank(self.cur.pen.bg));
            }
            cells.truncate(self.cols);
            if let Some(last) = cells.last_mut().filter(|c| c.width == 2) {
                *last = Cell::blank(last.bg);
            }
        }
        self.unwide(y, x);
        if w == 2 {
            self.unwide(y, x + 1);
        }
        let Pen { fg, bg, attrs } = self.cur.pen;
        let cells = &mut self.lines[y].cells;
        cells[x] = Cell { ch: c, fg, bg, attrs, width: w as u8 };
        if w == 2 {
            cells[x + 1] = Cell { ch: ' ', fg, bg, attrs, width: 0 };
        }
        if x + w >= self.cols {
            self.cur.x = self.cols - 1;
            self.cur.wrap = self.autowrap;
        } else {
            self.cur.x = x + w;
        }
    }

    /// Blanks cells `from..to` of a row.
    fn erase(&mut self, y: usize, from: usize, to: usize) {
        let to = to.min(self.cols);
        if from >= to {
            return;
        }
        self.unwide(y, from);
        self.unwide(y, to - 1);
        let bg = self.cur.pen.bg;
        self.lines[y].cells[from..to].fill(Cell::blank(bg));
        if to == self.cols {
            self.lines[y].wrapped = false;
        }
    }

    fn blank_line(&self) -> Line {
        Line::new(self.cols, self.cur.pen.bg)
    }

    /// Puts a line that has scrolled off the main screen into the history.
    fn keep(&mut self, mut line: Line) {
        while line.cells.last() == Some(&Cell::BLANK) {
            line.cells.pop();
        }
        self.history.push_back(line);
        self.scrolled += 1;
        if self.history.len() > HISTORY {
            self.history.pop_front();
            self.dropped += 1;
        }
    }

    /// Moves the lines of the scrolling region up, blank lines coming in
    /// at its bottom.
    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top);
        for _ in 0..n {
            let line = self.lines.remove(self.top);
            self.lines.insert(self.bottom - 1, self.blank_line());
            if self.top == 0 && !self.alt {
                self.keep(line);
            }
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top);
        for _ in 0..n {
            self.lines.remove(self.bottom - 1);
            self.lines.insert(self.top, self.blank_line());
        }
    }

    /// A line feed: down a row, scrolling at the bottom of the region.
    fn index(&mut self) {
        if self.cur.y + 1 == self.bottom {
            self.scroll_up(1);
        } else if self.cur.y + 1 < self.rows {
            self.cur.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cur.y == self.top {
            self.scroll_down(1);
        } else if self.cur.y > 0 {
            self.cur.y -= 1;
        }
    }

    fn goto(&mut self, row: usize, col: usize) {
        let (top, last) = if self.cur.origin { (self.top, self.bottom - 1) } else { (0, self.rows - 1) };
        self.cur.y = (top + row).min(last);
        self.cur.x = col.min(self.cols - 1);
        self.cur.wrap = false;
    }

    fn tab(&mut self, forward: bool) {
        let x = self.cur.x;
        self.cur.x = if forward {
            (x + 1..self.cols).find(|&i| self.tabs[i]).unwrap_or(self.cols - 1)
        } else {
            (0..x).rev().find(|&i| self.tabs[i]).unwrap_or(0)
        };
        self.cur.wrap = false;
    }

    fn save_cursor(&mut self) {
        self.saved[self.alt as usize] = Some(self.cur);
    }

    fn restore_cursor(&mut self) {
        match self.saved[self.alt as usize] {
            Some(c) => self.cur = c,
            None => self.cur = Cursor { x: 0, y: 0, ..Cursor::default() },
        }
        self.cur.x = self.cur.x.min(self.cols - 1);
        self.cur.y = self.cur.y.min(self.rows - 1);
    }

    fn alternate(&mut self, on: bool) {
        if on == self.alt {
            return;
        }
        if on {
            let fresh = vec![Line::new(self.cols, Color::Default); self.rows];
            self.main = Some(std::mem::replace(&mut self.lines, fresh));
        } else if let Some(main) = self.main.take() {
            self.lines = main;
        }
        self.alt = on;
        (self.top, self.bottom) = (0, self.rows);
    }

    /// Back to how the terminal starts, as after a soft or a full reset.
    fn reset(&mut self, full: bool) {
        self.cur.pen = Pen::default();
        (self.cur.origin, self.cur.drawing, self.cur.shifted) = (false, [false; 2], false);
        (self.top, self.bottom) = (0, self.rows);
        (self.app_cursor, self.app_keypad, self.autowrap, self.insert) = (false, false, true, false);
        (self.cursor_visible, self.cursor_shape, self.saved) = (true, 0, [None, None]);
        if full {
            self.alternate(false);
            (self.bracketed_paste, self.mouse, self.mouse_sgr, self.focus_events) = (false, Mouse::Off, false, false);
            self.tabs = (0..self.cols).map(|i| i % 8 == 0).collect();
            self.lines = vec![Line::new(self.cols, Color::Default); self.rows];
            self.cur = Cursor::default();
            self.title.clear();
        }
    }

    fn private_mode(&mut self, mode: u16, on: bool) {
        match mode {
            1 => self.app_cursor = on,
            6 => {
                self.cur.origin = on;
                self.goto(0, 0);
            }
            7 => self.autowrap = on,
            25 => self.cursor_visible = on,
            9 => self.mouse = if on { Mouse::Press } else { Mouse::Off },
            1000 => self.mouse = if on { Mouse::Click } else { Mouse::Off },
            1002 => self.mouse = if on { Mouse::Drag } else { Mouse::Off },
            1003 => self.mouse = if on { Mouse::Motion } else { Mouse::Off },
            1004 => self.focus_events = on,
            1006 => self.mouse_sgr = on,
            47 | 1047 => {
                if !on && self.alt {
                    let rows = self.rows;
                    (0..rows).for_each(|y| self.erase(y, 0, self.cols));
                }
                self.alternate(on);
            }
            1048 if on => self.save_cursor(),
            1048 => self.restore_cursor(),
            1049 if on => {
                self.save_cursor();
                self.alternate(true);
                self.saved[1] = self.saved[0];
            }
            1049 => {
                self.alternate(false);
                self.restore_cursor();
            }
            2004 => self.bracketed_paste = on,
            _ => {}
        }
    }

    fn sgr(&mut self, params: &[&[u16]]) {
        if params.is_empty() {
            self.cur.pen = Pen::default();
            return;
        }
        let pen = &mut self.cur.pen;
        let mut i = 0;
        // An extended colour, from `38;5;n` or `38;2;r;g;b`, or the same
        // with colons, where they all come as one parameter.
        let extended = |i: &mut usize| -> Option<Color> {
            let p = params[*i];
            let rest: Vec<u16> =
                if p.len() > 1 { p[1..].to_vec() } else { params[*i + 1..].iter().map(|q| q[0]).collect() };
            let (color, used) = match rest.first() {
                Some(5) => (rest.get(1).map(|&n| Color::Index(n as u8)), 2),
                // With colons a colour space may stand before the values.
                Some(2) if p.len() > 5 => (Some(Color::Rgb(rest[2] as u8, rest[3] as u8, rest[4] as u8)), 5),
                Some(2) => match rest.get(1..4) {
                    Some(v) => (Some(Color::Rgb(v[0] as u8, v[1] as u8, v[2] as u8)), 4),
                    None => (None, rest.len()),
                },
                _ => (None, 1),
            };
            if p.len() == 1 {
                *i += used;
            }
            color
        };
        while i < params.len() {
            let p = params[i];
            match p[0] {
                0 => *pen = Pen::default(),
                1 => pen.attrs |= BOLD,
                2 => pen.attrs |= DIM,
                3 => pen.attrs |= ITALIC,
                4 if p.get(1) == Some(&0) => pen.attrs &= !UNDERLINE,
                4 | 21 => pen.attrs |= UNDERLINE,
                7 => pen.attrs |= INVERSE,
                8 => pen.attrs |= HIDDEN,
                9 => pen.attrs |= STRIKE,
                22 => pen.attrs &= !(BOLD | DIM),
                23 => pen.attrs &= !ITALIC,
                24 => pen.attrs &= !UNDERLINE,
                27 => pen.attrs &= !INVERSE,
                28 => pen.attrs &= !HIDDEN,
                29 => pen.attrs &= !STRIKE,
                n @ 30..=37 => pen.fg = Color::Index(n as u8 - 30),
                38 => pen.fg = extended(&mut i).unwrap_or(pen.fg),
                39 => pen.fg = Color::Default,
                n @ 40..=47 => pen.bg = Color::Index(n as u8 - 40),
                48 => pen.bg = extended(&mut i).unwrap_or(pen.bg),
                49 => pen.bg = Color::Default,
                // The colour of underlines is not shown.
                58 => {
                    extended(&mut i);
                }
                n @ 90..=97 => pen.fg = Color::Index(n as u8 - 90 + 8),
                n @ 100..=107 => pen.bg = Color::Index(n as u8 - 100 + 8),
                _ => {}
            }
            i += 1;
        }
    }

    /// Feeds the screen what the program wrote.
    #[cfg(test)]
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut parser = vte::Parser::new();
        parser.advance(self, bytes);
    }

    /// The screen's text, a line to a row, without trailing spaces.
    #[cfg(test)]
    pub fn text(&self) -> String {
        let lines: Vec<String> = self.lines.iter().map(|l| l.text(0, self.cols).trim_end().to_string()).collect();
        lines.join("\n").trim_end().to_string()
    }
}

impl vte::Perform for Screen {
    fn print(&mut self, c: char) {
        self.put(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.bell = true,
            0x08 => {
                self.cur.x = self.cur.x.saturating_sub(1);
                self.cur.wrap = false;
            }
            0x09 => self.tab(true),
            0x0a..=0x0c => self.index(),
            0x0d => {
                self.cur.x = 0;
                self.cur.wrap = false;
            }
            0x0e => self.cur.shifted = true,
            0x0f => self.cur.shifted = false,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, inter: &[u8], _ignore: bool, action: char) {
        let all: Vec<&[u16]> = params.iter().collect();
        // A parameter, with 0 or nothing meaning `default`.
        let arg = |i: usize, default: usize| match all.get(i).map(|p| p[0]) {
            None | Some(0) => default,
            Some(n) => n as usize,
        };
        let n = arg(0, 1);
        match (action, inter) {
            ('A', []) => {
                let top = if self.cur.y >= self.top { self.top } else { 0 };
                self.cur.y = self.cur.y.saturating_sub(n).max(top);
                self.cur.wrap = false;
            }
            ('B' | 'e', []) => {
                let last = if self.cur.y < self.bottom { self.bottom - 1 } else { self.rows - 1 };
                self.cur.y = (self.cur.y + n).min(last);
                self.cur.wrap = false;
            }
            ('C' | 'a', []) => {
                self.cur.x = (self.cur.x + n).min(self.cols - 1);
                self.cur.wrap = false;
            }
            ('D', []) => {
                self.cur.x = self.cur.x.saturating_sub(n);
                self.cur.wrap = false;
            }
            ('E', []) => {
                let last = if self.cur.y < self.bottom { self.bottom - 1 } else { self.rows - 1 };
                (self.cur.x, self.cur.y, self.cur.wrap) = (0, (self.cur.y + n).min(last), false);
            }
            ('F', []) => {
                let top = if self.cur.y >= self.top { self.top } else { 0 };
                (self.cur.x, self.cur.y, self.cur.wrap) = (0, self.cur.y.saturating_sub(n).max(top), false);
            }
            ('G' | '`', []) => {
                self.cur.x = (n - 1).min(self.cols - 1);
                self.cur.wrap = false;
            }
            ('H' | 'f', []) => self.goto(arg(0, 1) - 1, arg(1, 1) - 1),
            ('d', []) => {
                let x = self.cur.x;
                self.goto(n - 1, x);
            }
            ('I', []) => (0..n).for_each(|_| self.tab(true)),
            ('Z', []) => (0..n).for_each(|_| self.tab(false)),
            ('J', [] | [b'?']) => {
                let (x, y) = (self.cur.x, self.cur.y);
                match arg(0, 0) {
                    0 => {
                        self.erase(y, x, self.cols);
                        (y + 1..self.rows).for_each(|r| self.erase(r, 0, self.cols));
                        self.lines[y + 1..].iter_mut().for_each(|l| l.marks.clear());
                    }
                    1 => {
                        (0..y).for_each(|r| self.erase(r, 0, self.cols));
                        self.erase(y, 0, x + 1);
                        self.lines[..y].iter_mut().for_each(|l| l.marks.clear());
                    }
                    2 => {
                        (0..self.rows).for_each(|r| self.erase(r, 0, self.cols));
                        self.lines.iter_mut().for_each(|l| l.marks.clear());
                    }
                    3 => {
                        self.dropped += self.history.len() as u64;
                        self.history.clear();
                    }
                    _ => {}
                }
            }
            ('K', [] | [b'?']) => {
                let (x, y) = (self.cur.x, self.cur.y);
                match arg(0, 0) {
                    0 => self.erase(y, x, self.cols),
                    1 => self.erase(y, 0, x + 1),
                    2 => self.erase(y, 0, self.cols),
                    _ => {}
                }
            }
            ('L' | 'M', []) if (self.top..self.bottom).contains(&self.cur.y) => {
                let top = std::mem::replace(&mut self.top, self.cur.y);
                if action == 'L' {
                    self.scroll_down(n)
                } else {
                    self.scroll_up_quietly(n)
                }
                self.top = top;
                self.cur.x = 0;
                self.cur.wrap = false;
            }
            ('@', []) => {
                let (x, y) = (self.cur.x, self.cur.y);
                self.unwide(y, x);
                let bg = self.cur.pen.bg;
                let cells = &mut self.lines[y].cells;
                for _ in 0..n.min(self.cols - x) {
                    cells.insert(x, Cell::blank(bg));
                }
                cells.truncate(self.cols);
                if let Some(last) = cells.last_mut().filter(|c| c.width == 2) {
                    *last = Cell::blank(last.bg);
                }
            }
            ('P', []) => {
                let (x, y) = (self.cur.x, self.cur.y);
                let n = n.min(self.cols - x);
                self.unwide(y, x);
                self.unwide(y, (x + n).min(self.cols - 1));
                let bg = self.cur.pen.bg;
                let cells = &mut self.lines[y].cells;
                cells.drain(x..x + n);
                cells.resize(self.cols, Cell::blank(bg));
            }
            ('X', []) => {
                let (x, y) = (self.cur.x, self.cur.y);
                self.erase(y, x, x + n);
            }
            ('S', []) => self.scroll_up(n),
            ('T', []) => self.scroll_down(n),
            ('b', []) => (0..n.min(1 << 16)).for_each(|_| self.put(self.last)),
            ('g', []) => match arg(0, 0) {
                0 => self.tabs[self.cur.x] = false,
                3 => self.tabs.fill(false),
                _ => {}
            },
            ('h' | 'l', [b'?']) => all.iter().for_each(|p| self.private_mode(p[0], action == 'h')),
            ('h' | 'l', []) => {
                if all.iter().any(|p| p[0] == 4) {
                    self.insert = action == 'h';
                }
            }
            ('m', []) => self.sgr(&all),
            ('n', []) => match arg(0, 0) {
                5 => self.reply.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let y = if self.cur.origin { self.cur.y - self.top } else { self.cur.y };
                    self.reply.extend(format!("\x1b[{};{}R", y + 1, self.cur.x + 1).bytes());
                }
                _ => {}
            },
            ('c', []) if arg(0, 0) == 0 => self.reply.extend_from_slice(b"\x1b[?62;22c"),
            ('c', [b'>']) => self.reply.extend_from_slice(b"\x1b[>0;10;1c"),
            ('q', [b' ']) => self.cursor_shape = arg(0, 0).min(6) as u8,
            ('p', [b'!']) => self.reset(false),
            ('r', []) => {
                let (top, bottom) = (arg(0, 1) - 1, arg(1, self.rows).min(self.rows));
                if top + 1 < bottom {
                    (self.top, self.bottom) = (top, bottom);
                    self.goto(0, 0);
                }
            }
            ('s', []) => self.save_cursor(),
            ('u', []) => self.restore_cursor(),
            ('t', []) if arg(0, 0) == 18 => {
                self.reply.extend(format!("\x1b[8;{};{}t", self.rows, self.cols).bytes());
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, inter: &[u8], _ignore: bool, byte: u8) {
        match (inter, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.index(),
            ([], b'E') => {
                self.cur.x = 0;
                self.cur.wrap = false;
                self.index();
            }
            ([], b'H') => self.tabs[self.cur.x] = true,
            ([], b'M') => self.reverse_index(),
            ([], b'c') => self.reset(true),
            ([], b'=') => self.app_keypad = true,
            ([], b'>') => self.app_keypad = false,
            ([b'('], set) => self.cur.drawing[0] = set == b'0',
            ([b')'], set) => self.cur.drawing[1] = set == b'0',
            ([b'#'], b'8') => {
                for line in &mut self.lines {
                    line.cells.fill(Cell { ch: 'E', ..Cell::BLANK });
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell: bool) {
        let text = |i: usize| params.get(i).map(|p| String::from_utf8_lossy(p).into_owned()).unwrap_or_default();
        let end: &[u8] = if bell { b"\x07" } else { b"\x1b\\" };
        match params.first().copied().unwrap_or(b"") {
            b"0" | b"2" => {
                self.title = params[1..].iter().map(|p| String::from_utf8_lossy(p)).collect::<Vec<_>>().join(";")
            }
            // Prompt and command marks, and VS Code's form of them, which
            // also says what the command line was.
            b"133" | b"633" if !self.alt => match text(1).as_str() {
                "E" => self.command = params.get(2).map(|p| unescape(p)),
                kind => {
                    if let Some(kind @ ('A' | 'B' | 'C' | 'D')) = kind.chars().next().filter(|_| kind.len() == 1) {
                        let code = if kind == 'D' { text(2).trim().parse().ok() } else { None };
                        // fish names the command line as it starts.
                        for p in params.iter().skip(2).map(|p| String::from_utf8_lossy(p)) {
                            if let Some(url) = p.strip_prefix("cmdline_url=") {
                                self.command = Some(crate::pane::percent_decode(url).chars().take(COMMAND).collect());
                            } else if let Some(line) = p.strip_prefix("cmdline=") {
                                self.command = Some(line.chars().take(COMMAND).collect());
                            }
                        }
                        self.mark(kind, code);
                    }
                }
            },
            b"7" => {
                // file://host/path, with the path percent-encoded.
                let url = text(1);
                let path = url.strip_prefix("file://").and_then(|rest| rest.find('/').map(|i| &rest[i..]));
                self.cwd = path.map(crate::pane::percent_decode);
            }
            // What the colours of text and ground are, asked by programs
            // that pick their own colours to suit.
            code @ (b"10" | b"11") if params.get(1) == Some(&&b"?"[..]) => {
                let c = if code == b"10" { crate::theme::fg() } else { crate::theme::bg() };
                let (r, g, b) = (c >> 16 & 0xff, c >> 8 & 0xff, c & 0xff);
                let code = String::from_utf8_lossy(code);
                self.reply.extend(format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}").bytes());
                self.reply.extend_from_slice(end);
            }
            b"52" => {
                if let Some(bytes) = params.get(2).filter(|p| **p != b"?").and_then(|p| base64(p)) {
                    self.clipboard = Some(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
            _ => {}
        }
    }
}

/// A command line as VS Code's marks send it: `\\` for a backslash and
/// `\xNN` for a byte that could not be sent as it is.
fn unescape(raw: &[u8]) -> Box<str> {
    let mut out = Vec::with_capacity(raw.len().min(COMMAND));
    let mut i = 0;
    while i < raw.len() && out.len() < COMMAND {
        let hex = raw.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (raw[i], raw.get(i + 1)) {
            (b'\\', Some(b'\\')) => (out.push(b'\\'), i += 2),
            (b'\\', Some(b'x')) if hex.is_some() => (out.push(hex.unwrap()), i += 4),
            (c, _) => (out.push(c), i += 1),
        };
    }
    String::from_utf8_lossy(&out).into()
}

/// A command, as the shell's marks show it. Places are a row, counted
/// from the oldest line kept, and a column.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Command {
    pub prompt: (usize, usize),
    pub input: Option<(usize, usize)>,
    pub output: Option<(usize, usize)>,
    pub end: Option<(usize, usize)>,
    pub code: Option<i32>,
    pub text: Option<Box<str>>,
}

impl Screen {
    /// The commands the shell has marked, oldest first, each from its
    /// prompt on.
    pub fn commands(&self) -> Vec<Command> {
        let mut out: Vec<Command> = Vec::new();
        for row in 0..self.total() {
            let Some(line) = self.row(row) else { continue };
            for m in &line.marks {
                let at = (row, m.col);
                match (m.kind, out.last_mut()) {
                    ('A', _) => out.push(Command { prompt: at, ..Command::default() }),
                    ('B', Some(c)) if c.input.is_none() && c.end.is_none() => c.input = Some(at),
                    ('C', Some(c)) if c.output.is_none() && c.end.is_none() => {
                        (c.output, c.text) = (Some(at), m.text.clone());
                    }
                    ('D', Some(c)) if c.end.is_none() => (c.end, c.code) = (Some(at), m.code.filter(|_| c.output.is_some())),
                    _ => {}
                }
            }
        }
        out
    }

    /// Puts a shell's mark at the cursor. A mark where one of its kind
    /// already is takes its place; the end of a command tells the prompt
    /// it was typed at how it went.
    fn mark(&mut self, kind: char, code: Option<i32>) {
        let col = self.cur.x;
        let text = if kind == 'C' { self.command.take() } else { None };
        let marks = &mut self.lines[self.cur.y].marks;
        marks.retain(|m| !(m.kind == kind && m.col == col));
        if marks.len() >= MARKS {
            marks.remove(0);
        }
        marks.push(Mark { col, kind, code, text });
        if kind != 'D' {
            return;
        }
        // Back to the prompt: the command ran if its output started.
        let mut ran = false;
        let here = self.history.len() + self.cur.y;
        for row in (0..=here).rev() {
            let line = match row.checked_sub(self.history.len()) {
                None => &mut self.history[row],
                Some(y) => &mut self.lines[y],
            };
            let skip = if row == here { 1 } else { 0 };
            for m in line.marks.iter_mut().rev().skip(skip) {
                match m.kind {
                    'C' => ran = true,
                    'A' => {
                        m.code = code.filter(|_| ran);
                        return;
                    }
                    'D' => return,
                    _ => {}
                }
            }
        }
    }

    /// Deletes lines in the scrolling region without keeping them: they
    /// were taken out of the middle of the screen, not scrolled off it.
    fn scroll_up_quietly(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top);
        for _ in 0..n {
            self.lines.remove(self.top);
            self.lines.insert(self.bottom - 1, self.blank_line());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(cols: usize, rows: usize, bytes: &str) -> Screen {
        let mut s = Screen::new(cols, rows);
        s.feed(bytes.as_bytes());
        s
    }

    #[test]
    fn text_wraps_and_scrolls_into_the_history() {
        let s = screen(5, 2, "abcdefgh\r\nij\r\nkl");
        assert_eq!(s.text(), "ij\nkl");
        let history: Vec<String> = s.history.iter().map(|l| l.text(0, 5)).collect();
        assert_eq!(history, ["abcde", "fgh"]);
        assert!(s.history[0].wrapped && !s.history[1].wrapped);
        assert_eq!((s.total(), s.row(1).unwrap().text(0, 3)), (4, "fgh".to_string()));
        // The last column is written without moving to the next line yet.
        let s = screen(3, 2, "abc");
        assert_eq!((s.text().as_str(), s.cursor()), ("abc", (2, 0)));
    }

    #[test]
    fn lines_wrap_anew_when_the_width_changes() {
        // Narrower: a long line breaks again, and the cursor stays after it.
        let mut s = screen(10, 3, "abcdefghij12\r\n$ ");
        assert_eq!(s.history.len(), 0);
        s.resize(5, 3);
        let all: Vec<String> = (0..s.total()).map(|i| s.row(i).unwrap().text(0, 5)).collect();
        assert_eq!(all.join("|"), "abcde|fghij|12   |$    ");
        assert_eq!((s.history.len(), s.cursor()), (1, (2, 2)));
        // Wider again: it is one line once more.
        s.resize(20, 3);
        assert_eq!(s.text(), "abcdefghij12\n$");
        assert_eq!((s.history.len(), s.cursor()), (0, (2, 1)));
        // Lines that ended of themselves are not joined.
        let mut s = screen(6, 3, "one\r\ntwo\r\n");
        s.resize(3, 3);
        assert_eq!(s.text(), "one\ntwo");
        // Wide characters are not cut in two.
        let mut s = screen(6, 2, "ab\u{4e2d}\u{6587}");
        s.resize(3, 2);
        assert_eq!(s.text(), "ab\n\u{4e2d}\n\u{6587}".split('\n').skip(1).collect::<Vec<_>>().join("\n"));
    }

    #[test]
    fn the_cursor_moves_and_erases() {
        let s = screen(10, 3, "hello\x1b[2;3Hx\x1b[1;2H\x1b[K\x1b[3;1Hend\x1b[D\x1b[D\x1b[1P");
        assert_eq!(s.text(), "h\n  x\ned");
        let s = screen(10, 3, "one\r\ntwo\r\nthree\x1b[2;1H\x1b[J");
        assert_eq!(s.text(), "one");
        let s = screen(10, 2, "abcdef\x1b[1;3H\x1b[2@\x1b[1;9Hz\x1b[1;1H\x1b[3X");
        assert_eq!(s.text(), "    cdefz");
        // Tabs stop every eight columns.
        let s = screen(20, 1, "a\tb\tc");
        assert_eq!(s.text(), "a       b       c");
    }

    #[test]
    fn colours_and_attributes_go_with_the_text() {
        let s = screen(10, 1, "\x1b[1;31ma\x1b[38;5;200;48;2;1;2;3mb\x1b[38:2::9:8:7mc\x1b[0md");
        let cells = &s.lines[0].cells;
        assert_eq!((cells[0].fg, cells[0].attrs), (Color::Index(1), BOLD));
        assert_eq!((cells[1].fg, cells[1].bg), (Color::Index(200), Color::Rgb(1, 2, 3)));
        assert_eq!(cells[2].fg, Color::Rgb(9, 8, 7));
        assert_eq!(cells[3], Cell { ch: 'd', ..Cell::BLANK });
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let s = screen(5, 2, "a漢b");
        assert_eq!((s.lines[0].cells[1].width, s.lines[0].cells[2].width), (2, 0));
        assert_eq!((s.text().as_str(), s.cursor()), ("a漢b", (4, 0)));
        // One that does not fit at the end of a line starts the next.
        let s = screen(4, 2, "abc漢");
        assert_eq!(s.text(), "abc\n漢");
        // Writing over half of one blanks the other half.
        let s = screen(5, 1, "漢\x1b[1;2Hx");
        assert_eq!(s.text(), " x");
        assert_eq!((width('e'), width('\u{301}'), width('😀'), width('─')), (1, 0, 2, 1));
    }

    #[test]
    fn scrolling_regions_and_line_insertion() {
        let s = screen(5, 4, "1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[3;1H\r\nx");
        assert_eq!(s.text(), "1\n3\nx\n4");
        assert!(s.history.is_empty());
        let s = screen(5, 4, "1\r\n2\r\n3\r\n4\x1b[2;1H\x1b[L");
        assert_eq!(s.text(), "1\n\n2\n3");
        let s = screen(5, 3, "1\r\n2\r\n3\x1b[1;1H\x1b[M");
        assert_eq!(s.text(), "2\n3");
        let s = screen(5, 3, "1\r\n2\r\n3\x1b[1;1H\x1bMx");
        assert_eq!(s.text(), "x\n1\n2");
    }

    #[test]
    fn the_alternate_screen_leaves_the_main_one_as_it_was() {
        let mut s = screen(10, 3, "shell$ vim\x1b[?1049h\x1b[2J\x1b[Hediting");
        assert!(s.alt && s.text() == "editing");
        s.feed(b"\x1b[?1049l");
        assert_eq!((s.text().as_str(), s.cursor(), s.alt), ("shell$ vim", (10 - 1, 0), false));
        // Lines scrolled on the alternate screen are not kept.
        let s = screen(10, 2, "\x1b[?1049h1\r\n2\r\n3\r\n4");
        assert!(s.history.is_empty());
    }

    #[test]
    fn queries_are_answered() {
        let s = screen(10, 5, "ab\x1b[6n\x1b[c\x1b[5n");
        assert_eq!(s.reply, b"\x1b[1;3R\x1b[?62;22c\x1b[0n");
        let s = screen(10, 5, "\x1b]0;my title\x07\x1b]7;file://host/home/a%20b\x1b\\\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(
            (s.title.as_str(), s.cwd.as_deref(), s.clipboard.as_deref()),
            ("my title", Some("/home/a b"), Some("hello"))
        );
        let s = screen(10, 5, "\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?25l\x1b[5 q");
        assert!(s.app_cursor && s.bracketed_paste && s.mouse == Mouse::Drag && s.mouse_sgr && !s.cursor_visible);
        assert_eq!(s.cursor_shape, 5);
        // Line drawing comes in for the letters while it is chosen.
        let s = screen(10, 1, "\x1b(0lqk\x1b(Bq");
        assert_eq!(s.text(), "┌─┐q");
    }

    #[test]
    fn resizing_keeps_what_is_above_the_cursor() {
        let mut s = screen(10, 4, "1\r\n2\r\n3\r\n4");
        s.resize(10, 2);
        assert_eq!((s.text().as_str(), s.history.len(), s.cursor()), ("3\n4", 2, (1, 1)));
        s.resize(10, 4);
        assert_eq!((s.text().as_str(), s.history.len(), s.cursor()), ("1\n2\n3\n4", 0, (1, 3)));
        // With the cursor at the top, rows go from below it instead.
        let mut s = screen(10, 4, "1\x1b[H");
        s.resize(4, 2);
        assert_eq!((s.text().as_str(), s.history.len(), s.cols), ("1", 0, 4));
    }

    /// A prompt, a command typed at it, its output and its end.
    fn command(cmd: &str, out: &str, code: i32) -> String {
        format!("\x1b]133;A\x07$ \x1b]133;B\x07{cmd}\r\n\x1b]133;C\x07{out}\x1b]133;D;{code}\x07")
    }

    #[test]
    fn shell_marks_find_commands_and_their_statuses() {
        let text = command("false", "", 1) + &command("echo hi", "hi\r\n", 0) + "\x1b]133;A\x07$ \x1b]133;B\x07";
        let mut s = screen(20, 3, &text);
        let all = s.commands();
        assert_eq!(all.len(), 3);
        assert_eq!((all[0].code, all[1].code, all[2].code), (Some(1), Some(0), None));
        // The prompt knows how its command went, even once it has
        // scrolled into the history.
        assert_eq!(s.history.len(), 1);
        let a = |s: &Screen, row: usize| s.row(row).unwrap().marks.iter().find(|m| m.kind == 'A').and_then(|m| m.code);
        assert_eq!((a(&s, 0), a(&s, 1)), (Some(1), Some(0)));
        let (row, col) = all[1].output.unwrap();
        assert_eq!(s.row(row).unwrap().text(col, 20).trim(), "hi");
        // Narrower, the lines wrap anew and the marks go with their text.
        s.resize(4, 3);
        let all = s.commands();
        let (row, col) = all[1].input.unwrap();
        assert_eq!((col, s.row(row).unwrap().text(col, 4).as_str()), (2, "ec"));
        assert_eq!(s.row(row + 1).unwrap().text(0, 4), "ho h");
        assert_eq!(all[1].code, Some(0));
        // Clearing the screen takes its marks away.
        s.feed(b"\x1b[3J\x1b[H\x1b[2J");
        assert!(s.commands().is_empty());
    }

    #[test]
    fn an_empty_command_line_has_no_status_and_command_lines_are_named() {
        let s = screen(30, 5, "\x1b]133;A\x07$ \x1b]133;B\x07\r\n\x1b]133;D;0\x07");
        assert_eq!(s.commands()[0].code, None);
        let s = screen(30, 5, "\x1b]633;E;a\\x3bb \\\\ c\x07\x1b]133;C\x07");
        assert_eq!(s.lines[0].marks[0].text.as_deref(), Some("a;b \\ c"));
        let s = screen(30, 5, "\x1b]133;C;cmdline_url=echo%20hi\x1b\\");
        assert_eq!(s.lines[0].marks[0].text.as_deref(), Some("echo hi"));
        // On the alternate screen marks are not kept.
        let s = screen(30, 5, "\x1b[?1049h\x1b]133;A\x07");
        assert!(s.lines.iter().all(|l| l.marks.is_empty()));
    }
}
