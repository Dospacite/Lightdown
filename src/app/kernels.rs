//! Choosing the kernel a notebook, or a script's percent cells, run on, and
//! answering a cell that asks for input.
use super::*;
use crate::kernelspec::{self, Choice};

impl App {
    /// The kernels to choose from, the current one marked.
    pub(super) fn open_kernels(&mut self) {
        let in_script = self.nb.is_none() && self.script_cells_here();
        if self.nb.is_none() && !in_script {
            self.message = Some("Open a notebook, or a script with percent cells, to choose its kernel".into());
            return;
        }
        let python = self.cfg.python_for(&self.root);
        let specs = kernelspec::discover(&python, &self.root);
        let current = match (&self.nb, in_script) {
            (Some(nb), _) => nb.choice.clone(),
            (None, _) => {
                let language = self.script_language_here();
                self.script_choice(&language).ok()
            }
        };
        let key_of = |c: &Choice| match c {
            Choice::Builtin => "builtin".to_string(),
            Choice::Jupyter(spec) => format!("spec:{}", spec.name),
        };
        let mut entries = vec![Entry::new("builtin", "Built-in Python", "Lightdown's own helper; needs only Python")];
        for spec in &specs {
            let place = spec.dir.as_ref().map_or_else(|| format!("{python} -m ipykernel"), |d| d.display().to_string());
            entries.push(Entry::new(
                &format!("spec:{}", spec.name),
                &spec.label(),
                &format!("{}  {place}", spec.language),
            ));
        }
        let selected = current.as_ref().map(key_of).unwrap_or_default();
        let mut menu = Menu::new("Kernel", entries, &selected);
        menu.hint = if kernelspec::has_package(&python, "jupyter_client") {
            "Jupyter kernels found on this machine and in the project".into()
        } else {
            format!("Jupyter kernels need jupyter_client in {python}")
        };
        self.menu = Some((MenuKind::Kernel, menu));
    }

    fn script_language_here(&self) -> String {
        if self.doc.doc == Doc::Python {
            return "python".into();
        }
        self.doc.path.as_deref().and_then(kernelspec::script_language).map_or("python", |l| l.0).to_string()
    }

    pub(super) fn pick_kernel(&mut self, key: &str) {
        let python = self.cfg.python_for(&self.root);
        let choice = if key == "builtin" {
            Choice::Builtin
        } else {
            let name = key.strip_prefix("spec:").unwrap_or(key);
            match kernelspec::discover(&python, &self.root).into_iter().find(|s| s.name == name) {
                Some(spec) => Choice::Jupyter(spec),
                None => {
                    self.message = Some(format!("No kernel named {name}"));
                    return;
                }
            }
        };
        let label = choice.label();
        if let Some(nb) = &mut self.nb {
            if nb.busy() {
                self.message = Some("Stop running cells before switching kernel".into());
                return;
            }
            nb.set_choice(&mut self.doc, choice);
            self.doc.buf.modified = true;
        } else {
            let language = self.script_language_here();
            let key = (self.root.clone(), language);
            self.scripts.chosen.insert(key.clone(), choice);
            self.scripts_forget(&key);
        }
        self.message = Some(format!("Kernel: {label}"));
    }

    /// Keys typed while a cell waits for input go to its prompt. Returns
    /// whether the key was taken.
    pub(super) fn notebook_input_key(&mut self, key: Key, mods: Mods, text: Option<&str>) -> bool {
        if self.focus != Focus::Doc {
            return false;
        }
        let Some(nb) = self.nb.as_mut().filter(|nb| nb.input.is_some()) else { return false };
        match key {
            Key::Enter if !mods.ctrl && !mods.alt => nb.submit_input(&mut self.doc),
            Key::Escape => {
                nb.interrupt(&mut self.doc);
                self.message = Some("Interrupted".into());
            }
            Key::Backspace => nb.edit_input(&mut self.doc, |t| {
                t.pop();
            }),
            _ if mods.ctrl || mods.alt => return false,
            _ => match text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                Some(t) => {
                    let t = t.to_string();
                    nb.edit_input(&mut self.doc, |typed| typed.push_str(&t));
                }
                None => return matches!(key, Key::Char(_)),
            },
        }
        self.redraw = true;
        true
    }

    /// Pasted text goes to a waiting prompt: its first line. Returns whether it did.
    pub(super) fn paste_into_input(&mut self, s: &str) -> bool {
        let Some(nb) = self.nb.as_mut().filter(|nb| nb.input.is_some() && self.focus == Focus::Doc) else {
            return false;
        };
        let line = s.lines().next().unwrap_or("").to_string();
        nb.edit_input(&mut self.doc, |t| t.push_str(&line));
        true
    }
}
