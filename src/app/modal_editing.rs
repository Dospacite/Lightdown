//! Window integration for modal editing, without editor key parsing.
use super::*;

impl App {
    pub(super) fn modal_buffer_entries(&self) -> Vec<Entry> {
        self.file_tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let pane = if index == self.active_file { &self.doc } else { &tab.state.as_ref().unwrap().pane };
                let name = pane.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Untitled".into());
                Entry::new(&format!("t\n{index}"), &name, "Open tab")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keys(app: &mut App, text: &str) {
        for c in text.chars() {
            let key = match c {
                '\n' => Key::Enter,
                '\u{1b}' => Key::Escape,
                _ => Key::Char(c.to_ascii_lowercase()),
            };
            let text = c.to_string();
            app.key(key, Mods::default(), (!c.is_control()).then_some(text.as_str()));
        }
    }
    #[test]
    fn helix_window_routes_modes_selections_space_and_ex() {
        let mut app = App::new(None);
        app.cfg.modal_editing = "helix".into();
        app.doc.set_modal_editing("helix");
        app.doc.set_text("one two".into(), Doc::Code);
        keys(&mut app, "wcnew\u{1b}");
        assert_eq!(app.doc.buf.text, "newtwo");
        assert_eq!(app.doc.vim_label(), Some("NOR"));
        keys(&mut app, "u");
        assert_eq!(app.doc.buf.text, "one two");
        keys(&mut app, " b");
        assert!(matches!(&app.menu,Some((MenuKind::Palette,menu)) if menu.typed()=="%"));
        keys(&mut app, "Untitled");
        assert_eq!(app.palette_mode, Some(palette::Mode::Buffers));
        app.key(Key::Enter, Mods::default(), None);
        assert!(app.menu.is_none());
        keys(&mut app, ":noh\n");
        assert!(app.doc.vim.request.is_none());
        app.cfg.modal_editing = "none".into();
        app.doc.set_modal_editing("none");
        assert_eq!(app.doc.vim_label(), None);
    }
    #[test]
    fn vim_completion_acceptance_records_the_result_for_dot() {
        let mut app = App::new(None);
        app.cfg.modal_editing = "vim".into();
        app.doc.set_modal_editing("vim");
        app.doc.set_text("foo foo".into(), Doc::Code);
        keys(&mut app, "ic");
        app.comp = Some(Completion {
            query: py::Query { start: 0, prefix: "c".into(), base: None },
            items: vec![Candidate { name: "completion".into(), kind: "name".into() }],
            sel: 0,
            top: 0,
            seq: 0,
        });
        app.key(Key::Tab, Mods::default(), None);
        keys(&mut app, "\u{1b}w.");
        assert_eq!(app.doc.buf.text, "completionfoo completionfoo");
    }
}
