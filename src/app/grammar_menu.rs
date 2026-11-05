//! Installing, listing and removing tree-sitter grammars from the palette.
use super::*;
use crate::{icons::Icon, treesitter};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Install,
    List,
    Remove,
}

impl App {
    pub(super) fn grammars_menu(&mut self, mode: Mode) {
        let mut entries = Vec::new();
        for (g, info) in treesitter::list() {
            let languages = g.aliases.join(" ");
            let entry = match (&info, mode) {
                (None, Mode::Install) => {
                    let state = if treesitter::installing(g.name) { "installing" } else { "not installed" };
                    Entry::new(&format!("install:{}", g.name), g.name, &format!("{state} · {} {} · {languages}", g.repo, g.version))
                }
                (Some(i), Mode::Install) => Entry::new(
                    &format!("install:{}", g.name),
                    g.name,
                    &format!("installed {}{} · reinstall", i.version, if i.version == g.version { "" } else { ", update available" }),
                ),
                (Some(i), Mode::List) => {
                    let abi = if treesitter::abi_supported(i.abi) { format!("ABI {}", i.abi) } else { format!("ABI {} unsupported", i.abi) };
                    Entry::new(&format!("show:{}", g.name), g.name, &format!("{} · {abi} · {languages}", i.version))
                }
                (Some(i), Mode::Remove) => Entry::new(&format!("remove:{}", g.name), g.name, &format!("{} · {}", i.version, languages)),
                (None, _) => continue,
            };
            entries.push(entry.icon(Icon::Braces));
        }
        if entries.is_empty() {
            self.message = Some("No grammars installed; use Code: Install language grammar".into());
            return;
        }
        let title = match mode {
            Mode::Install => "Install language grammar",
            Mode::List => "Installed grammars",
            Mode::Remove => "Remove grammar",
        };
        self.menu = Some((MenuKind::Grammars, Menu::new(title, entries, "")));
        self.redraw = true;
    }

    pub(super) fn parser_grammar_pick(&mut self, key: &str) {
        let Some((action, name)) = key.split_once(':') else { return };
        self.message = Some(match action {
            "install" if treesitter::install(name) => {
                format!("Downloading and compiling the {name} grammar in the background")
            }
            "install" => format!("The {name} grammar is already being installed"),
            "remove" => match treesitter::remove(name) {
                Ok(()) => {
                    self.doc.reparse();
                    format!("Removed the {name} grammar; lexical tools are back")
                }
                Err(e) => format!("Could not remove grammar: {e}"),
            },
            _ => return,
        });
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar_menus_offer_every_known_grammar_and_refuse_unknown_ones() {
        let mut a = App::new(None);
        a.grammars_menu(Mode::Install);
        let Some((MenuKind::Grammars, menu)) = &a.menu else { panic!("no menu") };
        assert_eq!(menu.entries.len(), treesitter::GRAMMARS.len());
        assert!(menu.entries.iter().all(|e| e.key.starts_with("install:")));
        a.parser_grammar_pick("remove:not-a-grammar");
        assert!(a.message.as_deref().unwrap().starts_with("Could not remove grammar"));
    }
}
