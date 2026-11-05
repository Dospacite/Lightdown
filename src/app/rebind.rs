//! The shortcut list and a chord recorder backed by keymap.json.
use super::assist::MenuKind;
use super::{App, Focus, Key, Mods};
use crate::keys::{ACTIONS, Chord, FIXED, Scope};
use crate::keys::contextual::{self, Sequence};
use crate::ui::{Entry, Menu};

const CONTEXTS: &[&str] = &["", "Main", "Document", "Editor", "Editor && mode == source", "Editor && mode == markdown",
    "Editor && vim_mode == normal", "Editor && vim_mode == insert", "Editor && vim_mode == visual",
    "Editor && helix_mode == normal", "Editor && helix_mode == insert", "Editor && helix_mode == visual",
    "Terminal", "Palette", "Explorer", "Notebook", "AgentComposer", "Dialog", "SourceControl", "SearchPanel"];

impl App {
    pub(super) fn in_scope(&self, scope: Scope) -> bool {
        let main = self.prompt.is_none() && !self.overlaid();
        let chat = self.focus == Focus::Chat && self.side_is_open(super::Tab::Agent);
        match scope {
            Scope::Always => true,
            Scope::Main => main,
            Scope::Doc => main && !self.in_side(),
            Scope::Notebook => main && !self.in_side() && self.nb.is_some(),
            Scope::Chat => main && chat,
        }
    }

    pub(super) fn keys_menu(&self, current: &str) -> Menu {
        let mut entries = Vec::new();
        for (id,name,scope) in super::keybindings::commands() {
            let default=contextual::scope_context(scope);
            let mut contexts=vec![default.to_string()];
            for b in &self.key_bindings.map.bindings {
                if b.command.as_deref()==Some(id) && !contexts.contains(&b.context.code) { contexts.push(b.context.code.clone()); }
            }
            for context in contexts {
                let keys=self.binding_label(id,&context);
                let conflicts=self.binding_sequences(id,&context).iter().flat_map(|s|self.key_conflicts(s,&context,id)).collect::<Vec<_>>();
                let detail=format!("{id} · {}{}",if context.is_empty() { "Global" } else { &context },if conflicts.is_empty() { String::new() } else { format!(" · conflicts: {}",conflicts.join(", ")) });
                entries.push(Entry::new(&format!("{id}\n{context}"),name,&detail).keys(keys));
            }
        }
        for &(name,keys) in FIXED { entries.push(Entry::new("",name,"Built-in input handling; contextual bindings can override it").keys(keys)); }
        let mut menu=Menu::new("Keybinds",entries,current);
        menu.tick=false; menu.width=780.0; menu.icon=crate::icons::Icon::Keyboard;
        menu.hint="Filter; Enter records a chord, Delete restores the base; Tab changes context while recording".into();
        menu
    }
    pub(super) fn rebind(&mut self, key: &str) {
        let (id,context)=key.split_once('\n').unwrap_or((key,""));
        let commands=super::keybindings::commands();
        let Some(index)=commands.iter().position(|(known,_,_)| *known==id) else { return self.open_menu(MenuKind::Keys) };
        self.rebinding=Some(index);
        self.key_bindings.context=if key.contains('\n') { context.into() } else { contextual::scope_context(commands[index].2).into() };
        self.key_bindings.recording.clear(); self.key_bindings.map.cancel();
        self.record_menu();
    }
    fn record_menu(&mut self) {
        let Some(index)=self.rebinding else {return};
        let commands=super::keybindings::commands();
        let (id,name,_)=commands[index];
        let context=&self.key_bindings.context;
        let sequence=Sequence(self.key_bindings.recording.clone());
        let conflicts=if sequence.0.is_empty() { Vec::new() } else { self.key_conflicts(&sequence,context,id) };
        let entries=vec![
            Entry::new("","Context", "Tab cycles contexts").keys(if context.is_empty(){"Global"}else{context}),
            Entry::new("","Keys now", "").keys(self.binding_label(id,context)),
            Entry::new("","Recording", "Enter saves").keys(sequence.name()),
            Entry::new("","Potential conflicts", &conflicts.join("; ")),
            Entry::new("","Esc cancels; Backspace removes a stroke or unbinds; Delete restores the base", ""),
        ];
        let mut menu=Menu::new(&format!("Record keys for {name}"),entries,"");
        menu.width=780.0; menu.icon=crate::icons::Icon::Keyboard;
        menu.hint="Press each stroke, then Enter to save. Tab chooses its context.".into();
        self.menu=Some((MenuKind::Keys,menu));
    }
    fn key_conflicts(&self,sequence: &Sequence,context: &str,id: &str) -> Vec<String> {
        let mut out=Vec::new();
        // Different contexts can intentionally share keys; label all potential overlaps.
        for (other,name,scope) in super::keybindings::commands() {
            if other==id {continue;}
            let mut contexts=vec![contextual::scope_context(scope).to_string()];
            contexts.extend(self.key_bindings.map.bindings.iter().filter(|b|b.command.as_deref()==Some(other)).map(|b|b.context.code.clone()));
            contexts.sort(); contexts.dedup();
            for other_context in contexts {
                if self.binding_sequences(other,&other_context).iter().any(|s|s.0.starts_with(&sequence.0)||sequence.0.starts_with(&s.0)) && contextual::contexts_overlap(context,&other_context) {
                    out.push(format!("{name} [{}]",if other_context.is_empty(){"Global"}else{&other_context}));
                }
            }
        }
        out
    }
    pub(super) fn rebind_key(&mut self,key: Key,mods: Mods) {
        if self.rebinding.is_none() {return;}
        let plain=!mods.ctrl&&!mods.alt&&!mods.shift;
        if plain {
            match key {
                Key::Escape=>{self.rebinding=None;self.key_bindings.recording.clear();self.menu=Some((MenuKind::Keys,self.keys_menu("")));return;}
                Key::Tab=>{
                    let at=CONTEXTS.iter().position(|c|*c==self.key_bindings.context).unwrap_or(0);
                    self.key_bindings.context=CONTEXTS[(at+1)%CONTEXTS.len()].into(); self.record_menu(); return;
                }
                Key::Backspace if !self.key_bindings.recording.is_empty()=>{self.key_bindings.recording.pop();self.record_menu();return;}
                Key::Backspace=>{self.save_recording(None,false);return;}
                Key::Delete if self.key_bindings.recording.is_empty()=>{self.save_recording(None,true);return;}
                Key::Enter if !self.key_bindings.recording.is_empty()=>{
                    self.save_recording(Some(Sequence(self.key_bindings.recording.clone())),false);return;
                }
                _=>{}
            }
        }
        let chord=Chord::new(key,mods);
        let context=&self.key_bindings.context;
        let nontyping=contextual::plain_context(context);
        if self.key_bindings.recording.is_empty() && !chord.assignable() && !mods.ctrl&&!mods.alt&&!nontyping {
            self.message=Some("A first stroke that types needs a non-typing context, such as Explorer or modal normal mode".into());return;
        }
        self.key_bindings.recording.push(chord); self.record_menu();
    }
    fn save_recording(&mut self,replacement: Option<Sequence>,reset: bool) {
        let Some(index)=self.rebinding else {return};
        let commands=super::keybindings::commands();
        let (id,_,_)=commands[index];
        let context=self.key_bindings.context.clone();
        let mut old=self.binding_sequences(id,&context);
        if let Some(a)=ACTIONS.iter().find(|a|a.id==id) {old.extend(a.keys.iter().filter_map(|s|Sequence::parse(s)));}
        let result=(|| -> Result<(),String> {
            let text=match std::fs::read_to_string(super::keybindings::path()) { Ok(s)=>s, Err(e) if e.kind()==std::io::ErrorKind::NotFound=>"[]".into(),Err(e)=>return Err(e.to_string()) };
            contextual::parse(&text)?;
            let text=contextual::edit_document(&text,&context,id,&old,replacement.as_ref(),reset)?;
            crate::config::write_private(&super::keybindings::path(),text.as_bytes()).map_err(|e|e.to_string())?;
            self.key_bindings.reload(&self.cfg)?; Ok(())
        })();
        if let Err(e)=result {self.message=Some(format!("Cannot save keymap: {e}"));return;}
        self.rebinding=None;self.key_bindings.recording.clear();
        self.menu=Some((MenuKind::Keys,self.keys_menu(&format!("{id}\n{context}"))));
    }
    pub(super) fn reset_key(&mut self,key: &str) {
        let (id,context)=key.split_once('\n').unwrap_or((key,""));
        if let Some(index)=super::keybindings::commands().iter().position(|(known,_,_)|*known==id) {
            self.rebinding=Some(index);self.key_bindings.context=context.into();self.save_recording(None,true);
        }
    }
    pub(super) fn keys_of(&self,id: &str) -> String {
        let scope=ACTIONS.iter().find(|a|a.id==id).map_or(Scope::Main,|a|a.scope);
        let mut sequences=self.binding_sequences(id,contextual::scope_context(scope));
        for binding in self.key_bindings.map.bindings.iter().filter(|b| b.command.as_deref()==Some(id)) {
            for sequence in self.binding_sequences(id,&binding.context.code) {
                if !sequences.contains(&sequence) { sequences.push(sequence); }
            }
        }
        sequences.iter().map(Sequence::name).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    #[test]
    fn shortcuts_are_listed_and_moved() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.paste_text("one two");
        // F1 lists them; typing narrows the list, Enter asks for new keys.
        a.key(Key::F(1), NONE, None);
        for ch in "select".chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
        a.key(Key::Enter, NONE, None);
        assert_eq!(a.rebinding.map(|i| ACTIONS[i].id), Some("select_all"));
        // A key that types is no shortcut; Ctrl+U is.
        a.key(Key::Char('u'), NONE, Some("u"));
        assert!(a.rebinding.is_some() && a.doc.buf.text == "one two");
        a.key(Key::Char('u'), CTRL, None);
        assert!(a.rebinding.is_some());
        a.key(Key::Enter, NONE, None);
        assert!(a.rebinding.is_none() && a.menu.is_some());
        assert_eq!(
            (a.keys_of("select_all"), a.key_bindings.user.iter().any(|b| b.command.as_deref()==Some("select_all"))),
            ("Ctrl+U".into(), true)
        );
        a.key(Key::Escape, NONE, None);

        // The new keys select all; the old ones no longer do.
        a.key(Key::Char('a'), CTRL, None);
        assert!(!a.doc.buf.has_sel());
        a.key(Key::Char('u'), CTRL, None);
        assert_eq!(a.doc.buf.sel(), (0, 7));
        // Delete in the list gives the default back.
        a.key(Key::F(1), NONE, None);
        for ch in "select".chars() {
            a.key(Key::Char(ch), NONE, Some(&ch.to_string()));
        }
        a.key(Key::Delete, NONE, None);
        assert!(a.cfg.keys.is_empty() && a.keys_of("select_all") == "Ctrl+A");
        // Contextual chords are recorded stroke by stroke and committed together.
        a.rebind("select_all\nEditor");
        a.key(Key::Char('k'), CTRL, None);
        a.key(Key::Char('a'), CTRL, None);
        assert!(a.rebinding.is_some());
        a.key(Key::Enter, NONE, None);
        assert!(a.key_bindings.user.iter().any(|b| b.context.code == "Editor" && b.sequence.name() == "Ctrl+K Ctrl+A"));
        a.key(Key::Escape, NONE, None);
        a.doc.select(7, 7);
        a.key(Key::Char('k'), CTRL, None);
        a.key(Key::Char('a'), CTRL, None);
        assert_eq!(a.doc.buf.sel(), (0, 7));
        let _ = std::fs::remove_file(super::super::keybindings::path());

    }
}
