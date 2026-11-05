//! Source-table editing uses pulldown's cell ranges, including escaped pipes.
use super::*;

struct TableEdit {
    start: usize,
    end: usize,
    rows: Vec<Vec<String>>,
    ranges: Vec<Vec<(usize, usize)>>,
    aligns: Vec<u8>,
    row: usize,
    column: usize,
    offset: usize,
}
impl TableEdit {
    fn source(&self) -> (String, Vec<Vec<(usize, usize)>>) {
        let columns = self.aligns.len();
        let widths: Vec<usize> = (0..columns).map(|c| self.rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0).max(3)).collect();
        let mut out = String::new();
        let mut ranges = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            if r > 0 { out.push('\n'); }
            out.push('|');
            let mut cells = Vec::new();
            for (c, text) in row.iter().enumerate() {
                out.push(' ');
                let start = out.len();
                out += text;
                cells.push((start, out.len()));
                out += &" ".repeat(widths[c].saturating_sub(text.chars().count()) + 1);
                out.push('|');
            }
            ranges.push(cells);
            if r == 0 {
                out += "\n|";
                for (c, w) in widths.iter().enumerate() {
                    let value = match self.aligns[c] {
                        1 => format!(":{}:", "-".repeat(w.saturating_sub(2).max(1))),
                        2 => format!("{}:", "-".repeat(w.saturating_sub(1).max(2))),
                        _ => "-".repeat(*w),
                    };
                    out += &format!(" {value} |");
                }
            }
        }
        (out, ranges)
    }
}
impl Pane {
    fn current_table(&self) -> Option<TableEdit> {
        if self.read_only || self.doc != Doc::Markdown || !self.extra_cursors.is_empty() { return None; }
        let md = self.md.as_ref()?;
        let cur = self.buf.cur;
        let block = md.blocks.iter().find(|b| b.table.is_some() && b.start as usize <= cur && cur <= b.end as usize)?;
        let first = self.buf.line_of(block.start as usize);
        // Rewriting container prefixes needs its own source mapping.
        if !md.lines[first].prefix.is_empty() { return None; }
        let table = &md.tables[block.table? as usize];
        let base = block.start as usize;
        let ranges: Vec<Vec<_>> = table.rows.iter().map(|row| row.iter().map(|&(a, b)| (base + a as usize, base + b as usize)).collect()).collect();
        if ranges.is_empty() || table.aligns.is_empty() { return None; }
        let row = ranges.iter().position(|r| r.first().is_some_and(|&(a, _)| self.buf.line_of(a) == self.buf.line_of(cur))).unwrap_or(0);
        let column = ranges[row].iter().position(|&(_, b)| {
            let end = self.buf.line(self.buf.line_of(b)).1;
            cur <= b + self.buf.text[b..end].find('|').unwrap_or(end - b)
        }).unwrap_or(ranges[row].len() - 1);
        let offset = cur.saturating_sub(ranges[row][column].0);
        let rows = ranges.iter().map(|r| {
            let mut cells: Vec<_> = r.iter().map(|&(a, b)| self.buf.text[a..b].to_string()).collect();
            cells.resize(table.aligns.len(), String::new());
            cells
        }).collect();
        Some(TableEdit { start: base, end: block.end as usize, rows, ranges, aligns: table.aligns.clone(), row, column, offset })
    }
    fn write_table(&mut self, table: TableEdit, select: bool) {
        let (source, ranges) = table.source();
        if source != self.buf.text[table.start..table.end] { self.edit(table.start, table.end, &source, EditKind::Other); }
        let (a, b) = ranges[table.row][table.column];
        if select { self.select(table.start + a, table.start + b); }
        else {
            let pos = table.start + a + editor::floor(&source[a..b], table.offset.min(b - a));
            self.select(pos, pos);
        }
        self.moved();
    }
    pub fn table_tool(&mut self, action: &str) -> bool {
        let Some(mut t) = self.current_table() else { self.note = Some("Place the cursor inside a markdown table".into()); return false };
        let columns = t.aligns.len();
        match action {
            "align" => {}
            "next" => {
                t.column += 1;
                if t.column == columns { t.column = 0; t.row += 1; }
                if t.row == t.rows.len() { t.rows.push(vec![String::new(); columns]); }
            }
            "previous" => {
                if t.column > 0 { t.column -= 1; }
                else if t.row > 0 { t.row -= 1; t.column = columns - 1; }
            }
            "row_insert" => { t.row += 1; t.rows.insert(t.row, vec![String::new(); columns]); }
            "row_delete" if t.row > 0 => { t.rows.remove(t.row); t.row = t.row.min(t.rows.len() - 1); }
            "column_insert" => {
                t.column += 1;
                for row in &mut t.rows { row.insert(t.column, String::new()); }
                t.aligns.insert(t.column, 0);
            }
            "column_delete" if columns > 1 => {
                for row in &mut t.rows { row.remove(t.column); }
                t.aligns.remove(t.column);
                t.column = t.column.min(columns - 2);
            }
            "sort" => {
                let column = t.column;
                t.rows[1..].sort_by(|a, b| match (a[column].parse::<f64>(), b[column].parse::<f64>()) {
                    (Ok(a), Ok(b)) => a.total_cmp(&b),
                    _ => a[column].cmp(&b[column]),
                });
            }
            _ => { self.note = Some("Keep the table header and at least one column".into()); return false; }
        }
        self.write_table(t, matches!(action, "next" | "previous" | "row_insert" | "column_insert"));
        true
    }
    /// Rebuild the table in one edit when typing inside one cell.
    pub(super) fn table_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if mods.ctrl || mods.alt { return false; }
        if key == Key::Tab { return self.current_table().is_some() && self.table_tool(if mods.shift { "previous" } else { "next" }); }
        let typed = text.filter(|s| !s.is_empty() && !s.contains(['|', '\n', '\r']) && !s.chars().any(char::is_control));
        if typed.is_none() && !matches!(key, Key::Backspace | Key::Delete) { return false; }
        let Some(mut t) = self.current_table() else { return false };
        let (a, b) = t.ranges[t.row][t.column];
        let (sa, sb) = self.buf.sel();
        let cell_end = t.ranges[t.row].get(t.column + 1).map_or(self.buf.line(self.buf.line_of(a)).1.saturating_sub(1), |&(next, _)| next.saturating_sub(2));
        if sa < a || sb > cell_end || self.buf.line_of(sa) != self.buf.line_of(a) { return false; }
        let cell = &mut t.rows[t.row][t.column];
        if sb > b { cell.push_str(&" ".repeat(sb - b)); }
        let (mut a, mut b) = (sa - a, sb - a);
        let ins = typed.unwrap_or("");
        if typed.is_none() && a == b {
            if key == Key::Backspace { a = editor::prev(cell, a); }
            else { b = editor::next(cell, b); }
            if a == b { return false; }
        }
        cell.replace_range(a..b, ins);
        t.offset = a + ins.len();
        self.write_table(t, false);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pane(text: &str) -> Pane {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())), Rc::new(RefCell::new(Pics::new())));
        p.set_text(text.into(), Doc::Markdown);
        p
    }
    #[test]
    fn navigation_appends_rows_and_skips_separator() {
        let mut p = pane("| A | B |\n| --- | ---: |\n| 2 | x\\|y |");
        p.select(2, 2);
        p.table_tool("next");
        assert_eq!(p.selected().as_deref(), Some("B"));
        p.table_tool("next");
        assert_eq!(p.selected().as_deref(), Some("2"));
        p.table_tool("next"); p.table_tool("next");
        assert_eq!(p.current_table().unwrap().rows.len(), 3);
        p.table_tool("previous");
        assert_eq!(p.selected().as_deref(), Some("x\\|y"));
    }
    #[test]
    fn typing_words_keeps_spaces_at_cell_end() {
        let mut p = pane("| A | B |\n| --- | --- |\n| x | y |");
        p.select(3, 3);
        for c in " long words".chars() {
            assert!(p.table_key(Key::Char(c), Mods::default(), Some(&c.to_string())));
        }
        assert_eq!(p.current_table().unwrap().rows[0][0], "A long words");
    }
    #[test]
    fn commands_and_typing_preserve_alignment_and_undo() {
        let mut p = pane("| A | B |\n| :---: | ---: |\n| 10 | é |\n| 2 | x |");
        p.select(2, 2); p.table_tool("sort");
        assert_eq!(p.current_table().unwrap().rows[1][0], "2");
        p.table_tool("column_insert");
        assert_eq!(p.current_table().unwrap().aligns, [1, 0, 2]);
        p.table_tool("column_delete");
        p.table_tool("row_insert");
        assert_eq!(p.current_table().unwrap().row, 1);
        p.table_tool("row_delete");
        p.select(2, 3);
        let before = p.buf.text.clone();
        assert!(p.table_key(Key::Char('z'), Mods::default(), Some("long")), "{}", p.buf.text);
        assert!(p.buf.text.contains("long"));
        p.key(Key::Char('z'), Mods { ctrl: true, ..Mods::default() }, None);
        assert_eq!(p.buf.text, before);
    }
}
