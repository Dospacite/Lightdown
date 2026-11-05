//! Pointer controls attached to visible notebook cells.
use super::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum CellAction {
    Run,
    Add,
    Kind,
    Menu,
}

/// A cell's control: where it is, the cell's index, what it does and its label.
type Control = ((f32, f32, f32, f32), usize, CellAction, &'static str);

/// How a cell's kind button names a kernel's language.
fn language_name(language: &str) -> &'static str {
    match language {
        "r" => "R",
        "julia" => "Julia",
        "typescript" => "TypeScript",
        "javascript" => "JavaScript",
        "rust" => "Rust",
        "python" | "" => "Python",
        _ => "Code",
    }
}

impl App {
    fn notebook_controls(&self) -> Vec<Control> {
        if self.nb.is_none() || self.doc.read_only {
            return Vec::new();
        }
        let mut controls = Vec::new();
        let (row_h, under) = crate::layout::header_row(self.doc.px());
        let btn = px(theme::SMALL_CONTROL, self.scale);
        for (index, (x, y, width, h)) in self.doc.cell_headers() {
            // Partly scrolled headers must never expose a button outside its row.
            if h < row_h + under {
                continue;
            }
            let code = self.doc.cells[index].tag == Tag::Code;
            let full = width >= 420.0 * self.scale;
            let language = self.nb.as_ref().map_or_else(|| "python".to_string(), |nb| nb.language());
            let kind = match self.doc.cells[index].tag {
                Tag::Code => language_name(&language),
                Tag::Raw => "Raw",
                _ => "Markdown",
            };
            let actions = [
                (CellAction::Menu, "", btn, true),
                (CellAction::Add, "", btn, full),
                (CellAction::Kind, kind, px(84.0, self.scale), full),
                (CellAction::Run, "", btn, code),
            ];
            let mid = y + h - under - row_h / 2.0;
            let mut right = x + width;
            for (action, label, w, visible) in actions {
                if !visible {
                    continue;
                }
                right -= w;
                // Keep the cell number and execution state readable.
                if right < x + 115.0 * self.scale {
                    continue;
                }
                controls.push(((right, (mid - btn / 2.0).round(), w, btn), index, action, label));
                right -= px(2.0, self.scale);
            }
        }
        controls
    }

    pub(super) fn notebook_hit(&self, x: f32, y: f32) -> Option<(usize, CellAction)> {
        self.notebook_controls()
            .into_iter()
            .find(|(r, _, _, _)| ui::inside(*r, Some((x, y))))
            .map(|(_, i, a, _)| (i, a))
    }

    pub(super) fn notebook_press(&mut self, x: f32, y: f32) -> bool {
        let Some((index, action)) = self.notebook_hit(x, y) else {
            return false;
        };
        self.focus = Focus::Doc;
        let start = self.doc.cell_body(index).0;
        self.doc.select(start, start);
        match action {
            CellAction::Run => {
                self.run_cell(index);
            }
            CellAction::Add => {
                self.insert_cell(index, true, Tag::Code, "");
            }
            CellAction::Kind => {
                let tag = if self.doc.cells[index].tag == Tag::Code { Tag::Markdown } else { Tag::Code };
                self.retag_cell(index, tag);
            }
            CellAction::Menu => {
                self.open_cell_menu(x, y);
            }
        }
        self.edited();
        true
    }

    pub(super) fn draw_notebook_controls(&mut self, c: &mut Canvas) {
        use crate::icons::Icon;
        let s = self.scale;
        let size = px(theme::SMALL, s);
        let fonts = self.fonts.clone();
        let fonts = &mut fonts.borrow_mut();
        for (r, _, action, label) in self.notebook_controls() {
            let hot = !self.overlaid() && ui::inside(r, Some(self.pointer));
            // The header's rule stops short of the controls.
            c.rect(r.0 - px(4.0, s), r.1, r.2 + px(4.0, s), r.3, theme::bg());
            match action {
                CellAction::Run => {
                    let fill = if hot { theme::primary_hover() } else { theme::primary() };
                    c.round_rect(r.0, r.1, r.2, r.3, px(theme::RADIUS_MD, s), fill);
                    crate::icons::centred(c, Icon::Play, r, px(12.0, s), theme::on_primary());
                    ui::tip(r, "Run the cell", &self.keys_of("run"));
                }
                CellAction::Kind => {
                    let radius = px(theme::RADIUS_MD, s);
                    c.round_rect(r.0, r.1, r.2, r.3, radius, if hot { theme::hover() } else { theme::bg() });
                    c.round_ring(r.0, r.1, r.2, r.3, radius, 1.0, theme::edge());
                    let tw = fonts.width(font::UI_MEDIUM, size, label);
                    let isz = px(12.0, s);
                    let x = (r.0 + (r.2 - tw - isz - px(4.0, s)) / 2.0).round();
                    ui::text(c, fonts, x, ui::base(r.1, r.3, size), label, font::UI_MEDIUM, size, theme::dim());
                    crate::icons::draw(
                        c,
                        Icon::ChevronDown,
                        x + tw + px(4.0, s),
                        (r.1 + (r.3 - isz) / 2.0).round(),
                        isz,
                        theme::icon(),
                    );
                    ui::tip(r, "Switch between code and markdown", &self.keys_of("cell_kind"));
                }
                CellAction::Add => {
                    ui::icon_button(c, r, Icon::Plus, hot, false, s);
                    ui::tip(r, "Add a cell below", &self.keys_of("cell_below"));
                }
                CellAction::Menu => {
                    ui::icon_button(c, r, Icon::Ellipsis, hot, false, s);
                    ui::tip(r, "Cell actions", "");
                }
            }
        }
    }
}

impl App {
    pub(super) fn open_variables(&mut self) {
        if self.script_cells_here() {
            self.script_variables();
            return;
        }
        let Some(nb) = &self.nb else {
            self.message = Some("Open a notebook to inspect its variables".into());
            return;
        };
        let entries = nb
            .variables
            .iter()
            .map(|(name, kind, shape)| Entry::new(name, name, &format!("{kind}  {shape}")))
            .collect();
        let mut menu = Menu::new("Notebook variables", entries, "");
        menu.hint = "Names, types and shapes from the last completed cell".into();
        self.menu = Some((MenuKind::Variables, menu));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };

    fn draw(a: &mut App) {
        let mut pixels = vec![0; (a.w * a.h) as usize];
        a.draw(&mut Canvas::new(&mut pixels, a.w as i32, a.h as i32));
    }

    fn click(a: &mut App, index: usize, action: CellAction) {
        draw(a);
        let (r, _, _, _) =
            a.notebook_controls().into_iter().find(|(_, i, ac, _)| *i == index && *ac == action).unwrap();
        let (x, y) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
        assert_eq!(a.zone(x, y, false), Zone::Button);
        a.press(x, y, NONE, 1);
        a.release();
    }

    #[test]
    fn mouse_controls_act_on_the_clicked_cell_and_allow_undo() {
        let file = std::env::temp_dir().join(format!("lightdown-mouse-{}.ipynb", std::process::id()));
        let json = serde_json::json!({ "nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": [
            {"cell_type": "code", "source": ["print('first')"], "outputs": [], "metadata": {}, "execution_count": null},
            {"cell_type": "code", "source": ["print(42)"], "outputs": [], "metadata": {}, "execution_count": null}
        ]});
        std::fs::write(&file, json.to_string()).unwrap();
        let mut a = App::new(Some(file.clone()));
        a.show_side(None);
        a.resize(1200, 800, 1.0);
        a.cfg.python = "python3".into();
        click(&mut a, 1, CellAction::Run);
        a.settle();
        assert!(a.nb.as_ref().unwrap().output_text(a.doc.cells[1].id, 100).contains("42"));
        assert!(a.nb.as_ref().unwrap().output_text(a.doc.cells[0].id, 100).is_empty());
        click(&mut a, 1, CellAction::Kind);
        assert_eq!(a.doc.cells[1].tag, Tag::Markdown);
        click(&mut a, 1, CellAction::Add);
        assert_eq!(a.doc.cells.len(), 3);
        click(&mut a, 1, CellAction::Menu);
        draw(&mut a);
        let (_, popup) = a.popup.as_ref().unwrap();
        let (x, y) = popup.point("Delete the cell");
        a.press(x, y, NONE, 1);
        assert_eq!(a.doc.cells.len(), 2);
        assert_eq!(a.doc.cell_body(1).0, a.doc.cell_body(1).1);
        a.act("undo");
        assert_eq!(a.doc.cells.len(), 3);
        assert_eq!(a.doc.cells[1].tag, Tag::Markdown);
        // Small and scaled windows retain a cell menu and fit its rows.
        for scale in [1.0, 2.0] {
            a.resize((320.0 * scale) as u32, (400.0 * scale) as u32, scale);
            a.doc.select(a.doc.cell_body(0).0, a.doc.cell_body(0).0);
            click(&mut a, 0, CellAction::Menu);
            draw(&mut a);
            assert!(a.popup.is_some());
            a.key(Key::Escape, NONE, None);
        }
        std::fs::remove_file(file).unwrap();
    }
}
