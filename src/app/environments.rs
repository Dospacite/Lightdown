use super::*;
impl App {
    pub(super) fn open_environment(&mut self) {
        let current = self.cfg.project(&self.root).python;
        let mut entries: Vec<_> = crate::environment::interpreters(&self.root)
            .into_iter()
            .map(|(path, label)| Entry::new(&path, &label, &path))
            .collect();
        if !current.is_empty() && !entries.iter().any(|e| e.key == current) {
            entries.push(Entry::new(&current, "Configured interpreter", &current));
        }
        self.menu = Some((MenuKind::Environment, Menu::new("Python interpreter for this project", entries, &current)));
    }
    pub(super) fn pick_environment(&mut self, key: &str) {
        if self.scripts_busy()
            || self.nb.as_ref().is_some_and(Notebook::busy)
            || self
                .file_tabs
                .iter()
                .filter_map(|t| t.state.as_ref())
                .any(|s| s.root == self.root && s.notebook.as_ref().is_some_and(Notebook::busy))
        {
            self.message = Some("Stop running cells before switching interpreter".into());
            return;
        }
        self.cfg.project_mut(&self.root).python = key.into();
        self.scripts_reset();
        self.helper = None;
        self.comp = None;
        self.docs = Default::default();
        for state in self.file_tabs.iter_mut().filter_map(|t| t.state.as_mut()).filter(|s| s.root == self.root) {
            state.helper = None;
            if let Some(nb) = &mut state.notebook {
                nb.restart(&mut state.pane);
                nb.reset_choice();
            }
        }
        if let Some(nb) = &mut self.nb {
            nb.restart(&mut self.doc);
            nb.reset_choice();
        }
        let _ = self.cfg.save();
        self.message = Some(format!("Python: {}", self.cfg.python_for(&self.root)));
    }
    pub(super) fn project_command(&self, command: &str, timeout: u64) -> std::process::Command {
        if let Some(remote) = crate::remote::active(&self.root) {
            return remote.command(&format!("timeout -k 2 {} sh -c {}", timeout.clamp(1, 86400), crate::remote::quote(command)), false);
        }
        let mut cmd = crate::tools::shell(&self.root, command, timeout);
        crate::environment::apply(&mut cmd, &self.root, Some(&self.cfg.python_for(&self.root)));
        cmd
    }
}
