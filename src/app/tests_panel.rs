//! The right panel's Tests tab: a filter field, a summary, and a tree of
//! runners, files or modules, and tests, each with its state, its time and
//! controls to run or debug it.
use super::testing::{Status, Tests};
use crate::icons::{self, Icon};
use crate::font::{self, Fonts};
use super::*;

/// What a click in the panel does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Press {
    Filter,
    ClearFilter,
    Output,
    Toggle(String),
    Run(Vec<usize>),
    Debug(usize),
    Select(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Runner(usize),
    /// A file, module or package: its key for folding, and its name.
    Group(String, String),
    Test(usize),
}

/// One row of the tree.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Line {
    pub depth: usize,
    pub node: Node,
    /// The tests it stands for.
    pub members: Vec<usize>,
    pub open: bool,
}

impl Tests {
    pub(super) fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.2 > 0.0 && ui::inside(self.rect, Some((x, y)))
    }
    pub(super) fn scroll_by(&mut self, dy: f32) {
        self.scroll = (self.scroll + dy).clamp(0.0, (self.height - self.rect.3).max(0.0));
    }
    /// Whether a test matches the filter: every word is in its runner,
    /// group or ID, and `is:failed`, `is:passed`, `is:skipped` or
    /// `is:running` names its state.
    fn matches(&self, i: usize) -> bool {
        let t = &self.items[i];
        let hay = format!("{} {} {}", self.runners[t.runner].kind.name(), t.group, t.id).to_lowercase();
        self.filter.to_lowercase().split_whitespace().all(|word| match word.strip_prefix("is:") {
            Some(state) => format!("{:?}", t.status).to_lowercase().starts_with(state),
            None => hay.contains(word),
        })
    }
    /// The tree's rows, with the filter applied. While filtering, every
    /// match is shown even in folded groups.
    pub(super) fn lines(&self) -> Vec<Line> {
        let filtering = !self.filter.trim().is_empty();
        let shown: Vec<usize> = (0..self.items.len()).filter(|&i| self.matches(i)).collect();
        let used: Vec<usize> = (0..self.runners.len()).filter(|r| shown.iter().any(|&i| self.items[i].runner == *r)).collect();
        let many = self.runners.len() > 1;
        let mut out = Vec::new();
        for r in used {
            let members: Vec<usize> = shown.iter().copied().filter(|&i| self.items[i].runner == r).collect();
            let key = format!("r:{r}");
            let open = filtering || !self.collapsed.contains(&key);
            if many {
                out.push(Line { depth: 0, node: Node::Runner(r), members: members.clone(), open });
                if !open {
                    continue;
                }
            }
            let mut groups: Vec<&str> = Vec::new();
            for &i in &members {
                if !groups.contains(&self.items[i].group.as_str()) {
                    groups.push(&self.items[i].group);
                }
            }
            for group in groups {
                let inside: Vec<usize> = members.iter().copied().filter(|&i| self.items[i].group == group).collect();
                let key = format!("g:{r}:{group}");
                let open = filtering || !self.collapsed.contains(&key);
                let depth = usize::from(many);
                out.push(Line { depth, node: Node::Group(key, group.to_string()), members: inside.clone(), open });
                if open {
                    out.extend(inside.into_iter().map(|i| Line { depth: depth + 1, node: Node::Test(i), members: vec![i], open: false }));
                }
            }
        }
        out
    }
}

/// The state a group of tests shows.
fn summary(items: &[testing::Test], members: &[usize]) -> Status {
    let has = |s: Status| members.iter().any(|&i| items[i].status == s);
    if has(Status::Running) {
        Status::Running
    } else if has(Status::Failed) {
        Status::Failed
    } else if has(Status::Passed) && members.iter().all(|&i| matches!(items[i].status, Status::Passed | Status::Skipped)) {
        Status::Passed
    } else if !members.is_empty() && members.iter().all(|&i| items[i].status == Status::Skipped) {
        Status::Skipped
    } else {
        Status::Idle
    }
}

pub(super) fn duration(secs: f64) -> String {
    if secs < 1.0 {
        format!("{} ms", (secs * 1000.0).round() as u64)
    } else if secs < 60.0 {
        format!("{secs:.1} s")
    } else {
        format!("{}m {}s", (secs / 60.0) as u64, (secs % 60.0) as u64)
    }
}

fn status_icon(status: Status) -> (Icon, u32) {
    match status {
        Status::Idle => (Icon::Dot, theme::faint()),
        Status::Running => (Icon::Dot, theme::warning()),
        Status::Passed => (Icon::Check, theme::success()),
        Status::Failed => (Icon::X, theme::danger()),
        Status::Skipped => (Icon::Minus, theme::faint()),
    }
}

impl App {
    pub(super) fn open_panel_tabs(&mut self) {
        self.panel_tabs_menu(false);
    }

    pub(super) fn tests_press(&mut self, x: f32, y: f32) {
        let press = self.tests.rows.iter().find(|(r, _)| ui::inside(*r, Some((x, y)))).map(|(_, p)| p.clone());
        self.tests.typing = press == Some(Press::Filter);
        match press {
            Some(Press::Filter) | None => {}
            Some(Press::ClearFilter) => {
                self.tests.filter.clear();
                self.tests.typing = true;
            }
            Some(Press::Output) => self.open_output(self.tests.output.clone()),
            Some(Press::Toggle(key)) => {
                if !self.tests.collapsed.remove(&key) {
                    self.tests.collapsed.insert(key);
                }
            }
            Some(Press::Run(list)) => self.tests_run(list),
            Some(Press::Debug(i)) => self.tests_debug(i),
            Some(Press::Select(i)) => {
                self.tests.selected = Some(i);
                let failed = self.tests.items.get(i).is_some_and(|t| t.status == Status::Failed);
                self.tests_reveal(i, failed);
            }
        }
        self.redraw = true;
    }

    /// Keys typed into the filter. Returns whether it took the key.
    pub(super) fn tests_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if !self.tests.typing || !self.side_open || self.tab != Tab::Tests || self.focus_mode {
            self.tests.typing = false;
            return false;
        }
        match key {
            Key::Escape if self.tests.filter.is_empty() => self.tests.typing = false,
            Key::Escape => self.tests.filter.clear(),
            Key::Enter | Key::Tab => self.tests.typing = false,
            Key::Backspace if mods.ctrl => self.tests.filter.clear(),
            Key::Backspace => {
                self.tests.filter.pop();
            }
            _ if mods.ctrl || mods.alt => return false,
            _ => match text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                Some(t) => self.tests.filter.push_str(t),
                None => return false,
            },
        }
        self.tests.scroll = 0.0;
        self.redraw = true;
        true
    }

    pub(super) fn draw_tests(&mut self, c: &mut Canvas) {
        let (x, y, w, h) = self.tests.rect;
        let s = self.scale;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.tests.rows.clear();
        c.clip(x, y, w, h);
        c.rect(x, y, w, h, theme::bg());
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        let text = px(theme::UI_TEXT, s);
        let small = px(theme::SMALL, s);
        let pointer = if self.overlaid() { None } else { Some(self.pointer) };
        let pad = px(8.0, s);

        // The filter.
        let field = (x + pad, y + pad, (w - pad * 2.0).max(0.0), px(theme::CONTROL, s));
        ui::input(c, field, self.tests.typing, s);
        icons::draw(c, Icon::Search, field.0 + px(8.0, s), field.1 + (field.3 - px(14.0, s)) / 2.0, px(14.0, s), theme::icon());
        let tx = field.0 + px(28.0, s);
        let room = (field.2 - px(56.0, s)).max(0.0);
        if self.tests.filter.is_empty() {
            let hint = ui::fit(fonts, font::UI, text, "Filter tests, or is:failed", room);
            ui::text(c, fonts, tx, ui::base(field.1, field.3, text), &hint, font::UI, text, theme::faint());
        } else {
            let shown = ui::fit_start(fonts, font::UI, text, &self.tests.filter, room);
            ui::text(c, fonts, tx, ui::base(field.1, field.3, text), &shown, font::UI, text, theme::fg());
        }
        if self.tests.typing {
            let shown = ui::fit_start(fonts, font::UI, text, &self.tests.filter, room);
            let end = tx + crate::bidi::caret_x(fonts,font::UI,text,&shown,shown.len());
            ui::caret(c, end + 1.0, field.1 + field.3 / 2.0, text * 1.3, s);
        }
        if !self.tests.filter.is_empty() {
            let b = px(theme::SMALL_CONTROL, s) - px(4.0, s);
            let r = (field.0 + field.2 - b - px(3.0, s), field.1 + (field.3 - b) / 2.0, b, b);
            ui::icon_button(c, r, Icon::X, ui::inside(r, pointer), false, s);
            ui::tip(r, "Clear the filter", "");
            self.tests.rows.push((r, Press::ClearFilter));
        }
        self.tests.rows.push((field, Press::Filter));

        // The summary, and the log at its right.
        let sy = field.1 + field.3 + px(6.0, s);
        let sh = px(22.0, s);
        let [passed, failed, skipped] = self.tests.counts();
        let total = self.tests.items.iter().filter(|t| !t.whole_file).count();
        let line = if self.tests.discovering() {
            "Discovering tests…".to_string()
        } else if self.tests.busy() {
            let running = self.tests.items.iter().filter(|t| t.status == Status::Running).count();
            format!("Running {running} tests…")
        } else if self.tests.root != self.root || self.tests.runners.is_empty() {
            String::new()
        } else {
            let mut parts = vec![format!("{total} tests")];
            for (n, word) in [(passed, "passed"), (failed, "failed"), (skipped, "skipped")] {
                if n > 0 {
                    parts.push(format!("{n} {word}"));
                }
            }
            if let Some(t) = self.tests.elapsed {
                parts.push(duration(t));
            }
            parts.join(" · ")
        };
        // A window of several folders shows whose tests these are.
        let line = if self.multi_root() && !line.is_empty() { format!("{} \u{b7} {line}", self.root_name(&self.root)) } else { line };
        let label = "Output";
        let lw = fonts.width(font::UI_MEDIUM, small, label) + px(12.0, s);
        let out_r = (x + w - pad - lw, sy, lw, sh);
        if !self.tests.output.is_empty() {
            let hot = ui::inside(out_r, pointer);
            if hot {
                c.round_rect(out_r.0, out_r.1, out_r.2, out_r.3, px(theme::RADIUS_MD, s), theme::hover());
            }
            ui::text(c, fonts, out_r.0 + px(6.0, s), ui::base(sy, sh, small), label, font::UI_MEDIUM, small, if hot { theme::fg() } else { theme::dim() });
            ui::tip(out_r, "Tests: Show output", &self.keys_of("tests_output"));
            self.tests.rows.push((out_r, Press::Output));
        }
        let fitted = ui::fit(fonts, font::UI, small, &line, (out_r.0 - x - pad * 2.0).max(0.0));
        let ink = if failed > 0 && !self.tests.busy() { theme::danger() } else { theme::dim() };
        ui::text(c, fonts, x + pad + px(4.0, s), ui::base(sy, sh, small), &fitted, font::UI, small, ink);

        // The tree.
        let top = sy + sh + px(4.0, s);
        c.clip(x, top, w, (y + h - top).max(0.0));
        let row_h = px(theme::ROW, s);
        let note_h = px(18.0, s);
        let mut cy = top - self.tests.scroll;
        let lines = if self.tests.root == self.root { self.tests.lines() } else { Vec::new() };
        if lines.is_empty() && !self.tests.busy() {
            let empty = if self.tests.root == self.root && !self.tests.items.is_empty() {
                "No tests match the filter"
            } else if self.tests.root == self.root && !self.tests.runners.is_empty() {
                "No tests were found; Tests: Show output has the log"
            } else {
                "No tests yet. Tests: Refresh tests looks for cargo, pytest, Jest, Vitest, go test and CTest."
            };
            let mut ty = cy + px(18.0, s);
            for part in wrap_words(fonts, empty, small, (w - pad * 3.0).max(40.0)) {
                ui::text(c, fonts, x + pad + px(4.0, s), ty, &part, font::UI, small, theme::dim());
                ty += px(18.0, s);
            }
        }
        let indent = px(14.0, s);
        let btn = px(theme::SMALL_CONTROL, s) - px(2.0, s);
        for line in lines {
            let failed_note = match line.node {
                Node::Test(i) => self.tests.items[i].message.clone().filter(|_| self.tests.items[i].status == Status::Failed),
                _ => None,
            };
            let height = row_h + if failed_note.is_some() { note_h } else { 0.0 };
            if cy > y + h {
                cy += height;
                continue;
            }
            let r = (x + px(4.0, s), cy, (w - px(8.0, s)).max(0.0), height);
            if cy + height < top {
                cy += height;
                continue;
            }
            let hot = ui::inside(r, pointer);
            let chosen = matches!(line.node, Node::Test(i) if self.tests.selected == Some(i));
            if chosen || hot {
                c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), if chosen { theme::active() } else { theme::hover() });
            }
            let lx = r.0 + px(6.0, s) + indent * line.depth as f32;
            let mid = cy + row_h / 2.0;
            let isz = px(12.0, s);
            let busy = self.tests.busy();
            // Controls at the right, shown under the pointer.
            let mut right = r.0 + r.2 - px(2.0, s);
            let mut controls: Vec<(Icon, Press, String)> = Vec::new();
            let (label, face, status, after, press) = match &line.node {
                Node::Runner(n) => {
                    let name = self.tests.runners[*n].kind.name().to_string();
                    controls.push((Icon::Play, Press::Run(line.members.clone()), format!("Run {name} tests")));
                    let st = summary(&self.tests.items, &line.members);
                    (name, font::UI_BOLD, st, format!("{}", line.members.len()), Press::Toggle(format!("r:{n}")))
                }
                Node::Group(key, name) => {
                    controls.push((Icon::Play, Press::Run(line.members.clone()), "Run these tests".into()));
                    let st = summary(&self.tests.items, &line.members);
                    (name.clone(), font::UI_MEDIUM, st, format!("{}", line.members.len()), Press::Toggle(key.clone()))
                }
                Node::Test(i) => {
                    let t = &self.tests.items[*i];
                    if super::test_runners::debuggable(self.tests.runners[t.runner].kind) {
                        controls.push((Icon::Bug, Press::Debug(*i), "Debug test".into()));
                    }
                    controls.push((Icon::Play, Press::Run(vec![*i]), if t.whole_file { "Run the file".into() } else { "Run test".into() }));
                    let after = t.duration.map(duration).unwrap_or_default();
                    (t.name().to_string(), font::UI, t.status, after, Press::Select(*i))
                }
            };
            if hot && !busy {
                for (icon, press, tip) in controls.into_iter().rev() {
                    right -= btn;
                    let b = (right, mid - btn / 2.0, btn, btn);
                    ui::icon_button(c, b, icon, ui::inside(b, pointer), false, s);
                    ui::tip(b, &tip, "");
                    self.tests.rows.push((b, press));
                    right -= px(2.0, s);
                }
            } else if !after.is_empty() {
                let aw = fonts.width(font::UI, small, &after);
                right -= aw + px(6.0, s);
                ui::text(c, fonts, right, ui::base(cy, row_h, small), &after, font::UI, small, theme::faint());
            }
            let mut tx = lx;
            if !matches!(line.node, Node::Test(_)) {
                let chevron = if line.open { Icon::ChevronDown } else { Icon::ChevronRight };
                icons::draw(c, chevron, tx, mid - isz / 2.0, isz, theme::icon());
                tx += isz + px(4.0, s);
            }
            let (icon, color) = status_icon(status);
            icons::draw(c, icon, tx, mid - isz / 2.0, isz, color);
            tx += isz + px(6.0, s);
            let label = ui::fit(fonts, face, text, &label, (right - tx - px(4.0, s)).max(0.0));
            let ink = if matches!(line.node, Node::Test(_)) { theme::fg() } else { theme::bright() };
            ui::text(c, fonts, tx, ui::base(cy, row_h, text), &label, face, text, ink);
            if let Some(note) = failed_note {
                let note = note.lines().next().unwrap_or("").to_string();
                let note = ui::fit(fonts, font::UI, small, &note, (r.0 + r.2 - tx - px(8.0, s)).max(0.0));
                ui::text(c, fonts, tx, ui::base(cy + row_h - px(3.0, s), note_h, small), &note, font::UI, small, theme::mix(theme::bg(), theme::danger(), 210));
            }
            self.tests.rows.push((r, press));
            cy += height;
        }
        self.tests.height = cy + self.tests.scroll - y + px(12.0, s);
        c.unclip();
        c.unclip();
    }
}

/// Splits text into lines no wider than `max`.
fn wrap_words(fonts: &mut Fonts, text: &str, size: f32, max: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let next = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if fonts.width(font::UI, size, &next) > max && !line.is_empty() {
            out.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_runners::{Found, Kind, Runner};
    use super::*;

    fn sample() -> Tests {
        let mut t = Tests::default();
        t.runners = vec![Runner { kind: Kind::Cargo, launch: String::new() }, Runner { kind: Kind::Pytest, launch: "python3".into() }];
        let mut add = |runner: usize, id: &str, group: &str, status: Status| {
            let mut test = testing::Test::new(runner, Found { id: id.into(), group: group.into(), path: None, line: None, whole_file: false });
            test.status = status;
            t.items.push(test);
        };
        add(0, "a::one", "a", Status::Passed);
        add(0, "a::two", "a", Status::Failed);
        add(0, "b::three", "b", Status::Idle);
        add(1, "test_x.py::test_y", "test_x.py", Status::Skipped);
        t
    }

    #[test]
    fn the_tree_groups_by_runner_then_file_or_module() {
        let mut t = sample();
        let nodes: Vec<(usize, Node)> = t.lines().into_iter().map(|l| (l.depth, l.node)).collect();
        assert_eq!(nodes.len(), 9);
        assert_eq!(nodes[0], (0, Node::Runner(0)));
        assert_eq!(nodes[1], (1, Node::Group("g:0:a".into(), "a".into())));
        assert_eq!(nodes[2], (2, Node::Test(0)));
        assert_eq!(nodes[6], (0, Node::Runner(1)));
        assert_eq!(summary(&t.items, &[0, 1]), Status::Failed);
        assert_eq!(summary(&t.items, &[0]), Status::Passed);
        // Folding hides a group's tests, and filtering shows them again.
        t.collapsed.insert("g:0:a".into());
        assert_eq!(t.lines().len(), 7);
        t.filter = "TWO".into();
        let lines = t.lines();
        assert_eq!(lines.iter().filter(|l| matches!(l.node, Node::Test(_))).count(), 1);
        t.filter = "is:failed".into();
        assert_eq!(t.lines().last().unwrap().node, Node::Test(1));
        t.filter = "nothing".into();
        assert!(t.lines().is_empty());
        // One runner needs no runner row.
        let mut one = sample();
        one.runners.truncate(1);
        one.items.truncate(3);
        assert!(matches!(one.lines()[0].node, Node::Group(..)));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(0.0042), "4 ms");
        assert_eq!(duration(2.345), "2.3 s");
        assert_eq!(duration(125.0), "2m 5s");
    }
}
