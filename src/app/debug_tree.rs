//! The Debug tab's variables as a tree: rows open and close by click or by
//! Right and Left, and long containers load 200 children at a time. Both
//! debuggers fetch children lazily from the paused process.
use super::*;
use serde_json::{Value, json};

/// Children are fetched a page at a time, up to this many per variable.
pub(super) const PAGE: usize = 200;
pub(super) const MAX_CHILDREN: usize = 20_000;
/// Rows deeper than this are not opened.
pub(super) const MAX_DEPTH: usize = 8;

/// One line of the variables section.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Row {
    /// The line of the Debug tab's text it is drawn on.
    pub line: usize,
    pub depth: usize,
    /// The reference its children are fetched by; 0 for a leaf.
    pub reference: i64,
    pub open: bool,
    /// "Show 200 more" under `reference`'s loaded children.
    pub more: bool,
    pub parent: Option<usize>,
}

#[derive(Default)]
pub(super) struct Tree {
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Whether the arrow keys move in the tree.
    pub focus: bool,
}

/// Writes the variables under `text`, starting on `line`, and returns the
/// rows written. `expanded` holds the loaded children by reference.
pub(super) fn render(text: &mut String, line: usize, variables: &Value, expanded: &Value) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut line = line;
    walk(text, &mut line, &mut rows, variables, expanded, 0, None);
    rows
}

fn walk(text: &mut String, line: &mut usize, rows: &mut Vec<Row>, variables: &Value, expanded: &Value, depth: usize, parent: Option<usize>) {
    for v in variables.as_array().into_iter().flatten().take(MAX_CHILDREN) {
        if text.len() > 60_000 { return; }
        let reference = if depth < MAX_DEPTH { v["variablesReference"].as_i64().unwrap_or(0).max(0) } else { 0 };
        let children = &expanded[reference.to_string()];
        let open = reference > 0 && children.is_array();
        let marker = if reference == 0 { "  " } else if open { "▾ " } else { "▸ " };
        let value: String = v["value"].as_str().unwrap_or("").chars().take(500).collect();
        let kind = v["type"].as_str().filter(|t| !t.is_empty()).map_or(String::new(), |t| format!(": {t}"));
        text.push_str(&format!("{}{marker}{}{kind} = {}\n", "  ".repeat(depth), v["name"].as_str().unwrap_or(""), value.replace('\n', " ")));
        let me = rows.len();
        rows.push(Row { line: *line, depth, reference, open, more: false, parent });
        *line += 1;
        if open {
            walk(text, line, rows, children, expanded, depth + 1, Some(me));
            let count = children.as_array().map_or(0, Vec::len);
            if count > 0 && count % PAGE == 0 && count < MAX_CHILDREN && text.len() <= 60_000 {
                text.push_str(&format!("{}  Show {PAGE} more\n", "  ".repeat(depth + 1)));
                rows.push(Row { line: *line, depth: depth + 1, reference, open: false, more: true, parent: Some(me) });
                *line += 1;
            }
        }
    }
}

/// The references loaded under `reference`, at any depth, itself included.
fn descendants(expanded: &Value, reference: i64, out: &mut Vec<String>) {
    let key = reference.to_string();
    if out.contains(&key) || out.len() > 10_000 { return; }
    for v in expanded[&key].as_array().into_iter().flatten() {
        if let Some(r) = v["variablesReference"].as_i64().filter(|r| *r > 0) { descendants(expanded, r, out); }
    }
    out.push(key);
}

impl App {
    /// Opens a variable, or loads its next page of children.
    pub(super) fn debug_expand(&mut self, reference: i64) -> Result<(), String> {
        if !self.debugger.paused { return Err("Pause the debugger first".into()); }
        if self.debugger.dap.is_some() { return self.native_expand(reference); }
        let Some(p) = &self.debugger.process else { return Ok(()) };
        let start = self.debugger.snapshot["expanded"][reference.to_string()].as_array().map_or(0, Vec::len);
        if start >= MAX_CHILDREN { return Err("Variable page limit reached".into()); }
        p.send(json!({"action":"expand","reference":reference,"start":start}));
        Ok(())
    }

    /// Closes a variable and forgets what was loaded beneath it.
    pub(super) fn debug_collapse(&mut self, reference: i64) {
        let mut keys = Vec::new();
        descendants(&self.debugger.snapshot["expanded"], reference, &mut keys);
        if let Some(o) = self.debugger.snapshot["expanded"].as_object_mut() {
            for k in keys { o.remove(&k); }
        }
    }

    /// Adds a page of children to what is loaded, unless it is a page
    /// already had or asked for twice.
    pub(super) fn debug_children(&mut self, key: &str, start: usize, page: &Value) {
        if !self.debugger.snapshot["expanded"].is_object() { self.debugger.snapshot["expanded"] = json!({}); }
        let mut items = self.debugger.snapshot["expanded"][key].as_array().cloned().unwrap_or_default();
        if items.len() != start || items.len() >= MAX_CHILDREN { return; }
        items.extend(page.as_array().into_iter().flatten().take(PAGE).cloned());
        self.debugger.snapshot["expanded"][key] = json!(items);
    }

    /// Does what a row does when chosen: opens or closes it, or loads more.
    fn debug_activate(&mut self, row: usize, open: Option<bool>) {
        let Some(r) = self.debugger.tree.rows.get(row).cloned() else { return };
        if r.reference == 0 { return; }
        let result = if r.more {
            self.debug_expand(r.reference)
        } else if open.unwrap_or(!r.open) {
            if r.open { Ok(()) } else { self.debug_expand(r.reference) }
        } else {
            self.debug_collapse(r.reference);
            Ok(())
        };
        if let Err(e) = result { self.message = Some(e); }
        self.debugger.update();
    }

    /// A press in the Debug tab: a variable's row opens or closes, and the
    /// arrow keys then move among the rows. Returns whether it was there.
    pub(super) fn debug_tree_press(&mut self, x: f32, y: f32) -> bool {
        if !self.debugger.view.contains(x, y) { self.debugger.tree.focus = false; return false; }
        self.debugger.view.place_at(x, y);
        let line = self.debugger.view.buf.line_of(self.debugger.view.buf.cur);
        let Some(i) = self.debugger.tree.rows.iter().position(|r| r.line == line) else {
            self.debugger.tree.focus = false;
            return true;
        };
        self.focus = Focus::Doc;
        self.debugger.tree.focus = true;
        self.debugger.tree.selected = i;
        self.debug_activate(i, None);
        self.debug_tree_mark();
        true
    }

    /// Puts the keyboard in the variables, from the palette.
    pub(super) fn debug_tree_focus(&mut self) {
        self.show_side(Some(Tab::Debug));
        if self.debugger.tree.rows.is_empty() { self.message = Some("Pause the debugger to browse variables".into()); return; }
        self.debugger.tree.focus = true;
        self.debugger.tree.selected = self.debugger.tree.selected.min(self.debugger.tree.rows.len() - 1);
        self.debug_tree_mark();
    }

    /// The arrow keys, Enter and Esc while the variables have the keyboard.
    pub(super) fn debug_tree_key(&mut self, key: Key, mods: Mods) -> bool {
        let tree = &mut self.debugger.tree;
        if !tree.focus || mods.ctrl || mods.alt || mods.shift { return false; }
        if !(self.side_open && self.tab == Tab::Debug) || tree.rows.is_empty() { tree.focus = false; return false; }
        let i = tree.selected.min(tree.rows.len() - 1);
        let row = tree.rows[i].clone();
        match key {
            Key::Up => tree.selected = i.saturating_sub(1),
            Key::Down => tree.selected = (i + 1).min(tree.rows.len() - 1),
            Key::Home => tree.selected = 0,
            Key::End => tree.selected = tree.rows.len() - 1,
            Key::Right if row.open => tree.selected = (i + 1).min(tree.rows.len() - 1),
            Key::Right => self.debug_activate(i, Some(true)),
            Key::Left if row.open => self.debug_activate(i, Some(false)),
            Key::Left => tree.selected = row.parent.unwrap_or(i),
            Key::Enter | Key::Char(' ') => self.debug_activate(i, None),
            Key::Escape => { tree.focus = false; self.debugger.view.select(0, 0); return true; }
            _ => { tree.focus = false; return false; }
        }
        self.debug_tree_mark();
        true
    }

    /// Picks out the selected row while the variables have the keyboard.
    pub(super) fn debug_tree_mark(&mut self) {
        let d = &mut self.debugger;
        if !d.tree.focus { return; }
        d.tree.selected = d.tree.selected.min(d.tree.rows.len().saturating_sub(1));
        let Some(row) = d.tree.rows.get(d.tree.selected) else { return };
        if row.line >= d.view.buf.lines() { return; }
        let (start, end) = d.view.buf.line(row.line);
        if d.view.buf.sel() != (start, end) { d.view.select(start, end); }
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_loaded_children_and_offer_more_pages() {
        let page: Vec<Value> = (0..PAGE).map(|i| json!({"name":i.to_string(),"type":"int","value":"1","variablesReference":0})).collect();
        let vars = json!([{"name":"a","type":"int","value":"1","variablesReference":0},
            {"name":"items","type":"list","value":"[…]","variablesReference":4},
            {"name":"obj","type":"Foo","value":"<Foo object>","variablesReference":5}]);
        let expanded = json!({"4": page});
        let mut text = String::from("header\n");
        let rows = render(&mut text, 1, &vars, &expanded);
        assert_eq!(rows.len(), 3 + PAGE + 1);
        assert!(text.contains("\n  a: int = 1\n▾ items: list = […]\n    0: int = 1\n"));
        assert!(text.contains(&format!("  Show {PAGE} more\n▸ obj: Foo = <Foo object>\n")));
        let more = &rows[2 + PAGE];
        assert!(more.more && more.reference == 4 && more.parent == Some(1) && more.line == 3 + PAGE);
        assert_eq!(text.lines().nth(rows[2].line), Some("    0: int = 1"));
        assert!(rows[1].open && !rows[2 + PAGE + 1].open);
    }

    #[test]
    fn depth_is_bounded_and_collapse_forgets_descendants() {
        let mut expanded = json!({});
        for d in 1..12 { expanded[d.to_string()] = json!([{"name":"x","value":"","variablesReference":d + 1}]); }
        let mut text = String::new();
        let rows = render(&mut text, 0, &json!([{"name":"x","value":"","variablesReference":1}]), &expanded);
        assert_eq!(rows.len(), MAX_DEPTH + 1);
        assert_eq!(rows.last().unwrap().reference, 0);
        let mut keys = Vec::new();
        descendants(&expanded, 3, &mut keys);
        assert_eq!(keys.len(), 10);
        assert!(keys.contains(&"3".to_string()) && !keys.contains(&"2".to_string()));
    }
}

#[cfg(test)]
mod python {
    use super::*;
    fn wait(a: &mut App, what: &str, predicate: impl Fn(&App) -> bool) {
        let start = std::time::Instant::now();
        while !predicate(a) && start.elapsed().as_secs() < 5 {
            a.debug_poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(predicate(a), "{what}: {}\n{}", a.debugger.output, a.debugger.view.buf.text);
    }
    fn row(a: &App, text: &str) -> usize {
        let lines: Vec<&str> = a.debugger.view.buf.text.lines().collect();
        a.debugger.tree.rows.iter().position(|r| lines.get(r.line).is_some_and(|l| l.trim_start().trim_start_matches(['▸', '▾']).trim_start().starts_with(text)))
            .unwrap_or_else(|| panic!("no row {text}: {}", a.debugger.view.buf.text))
    }
    fn press(a: &mut App, i: usize, key: Key) {
        a.debugger.tree.focus = true;
        a.debugger.tree.selected = i;
        assert!(a.debug_tree_key(key, Mods::default()));
    }

    #[test]
    fn python_objects_open_lazily_page_by_page_without_running_properties() {
        let dir = std::env::temp_dir().join(format!("ld-debug-tree-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let path = dir.join("main.py");
        std::fs::write(&path, "class Box:\n    def __init__(self):\n        self.items = list(range(450))\n        self.name = 'b'\n    @property\n    def boom(self):\n        raise SystemExit('property ran')\n    def __repr__(self):\n        raise SystemExit('repr ran')\nb = Box()\nprint('done')\n").unwrap();
        let mut a = App::new(Some(path.clone()));
        a.resize(1200, 800, 1.0);
        a.debug_settings();
        a.debugger.points = vec![(path.canonicalize().unwrap(), 11)];
        a.debug_command("start");
        wait(&mut a, "started", |a| a.debugger.paused);
        a.debug_command("continue");
        wait(&mut a, "paused", |a| a.debugger.paused);
        assert!(a.debugger.view.buf.text.contains("▸ b: Box = <Box object>"), "{}", a.debugger.view.buf.text);
        let b = row(&a, "b: Box");
        press(&mut a, b, Key::Right);
        wait(&mut a, "opened b", |a| a.debugger.view.buf.text.contains("  ▸ items: list = [0, 1, 2"));
        assert!(a.debugger.view.buf.text.contains("    name: str = 'b'"));
        let items = row(&a, "items: list");
        press(&mut a, items, Key::Right);
        wait(&mut a, "first page", |a| a.debugger.view.buf.text.contains("      199: int = 199"));
        assert!(!a.debugger.view.buf.text.contains("200: int"));
        let more = row(&a, "Show 200 more");
        press(&mut a, more, Key::Enter);
        wait(&mut a, "second page", |a| a.debugger.view.buf.text.contains("      399: int = 399"));
        let more = row(&a, "Show 200 more");
        press(&mut a, more, Key::Enter);
        wait(&mut a, "last page", |a| a.debugger.view.buf.text.contains("      449: int = 449"));
        assert!(!a.debugger.view.buf.text.contains("Show 200 more"));
        // Right on an open row goes to its first child, Left back to it.
        press(&mut a, items, Key::Right);
        assert_eq!(a.debugger.tree.selected, items + 1);
        assert!(a.debug_tree_key(Key::Left, Mods::default()));
        assert_eq!(a.debugger.tree.selected, items);
        assert!(a.debug_tree_key(Key::Left, Mods::default()));
        assert!(!a.debugger.view.buf.text.contains("449: int"));
        assert!(a.debugger.view.buf.text.contains("▸ items: list"));
        a.debug_command("continue");
        wait(&mut a, "finished", |a| a.debugger.process.is_none());
        assert!(!a.debugger.output.contains("ran"), "{}", a.debugger.output);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_inspection_unit_tests_pass() {
        let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
        let out = std::process::Command::new("python3").args(["-B", "-m", "unittest", "discover", "-s", assets, "-p", "test_debug_inspection.py"])
            .env("PYTHONDONTWRITEBYTECODE", "1").output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
}
