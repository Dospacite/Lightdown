//! Project shell commands run off the input thread and apply only to unchanged text.
use super::*;
use crate::tools::Job;
use serde_json::Value;
#[derive(Clone)]
struct UserCommand { name:String, command:String, output:String, keys:Vec<Chord> }
struct Running { job:Job, command:UserCommand, key:(u64,u64,usize,usize), root:PathBuf }
#[derive(Default)]
pub(super) struct Commands { running:Option<Running> }
/// Whether the project's commands.json loads; for the schema tests.
#[cfg(test)]
pub(super) fn loads(root: &Path) -> Result<usize, String> {
    load(root).map(|c| c.len())
}
fn load(root:&Path) -> Result<Vec<UserCommand>,String> {
    let path = root.join(".lightdown/commands.json");
    if !path.exists() { return Ok(Vec::new()); }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 512*1024 { return Err("commands.json exceeds 512 KB".into()); }
    let v:Value = serde_json::from_slice(&bytes).map_err(|e| format!("commands.json: {e}"))?;
    let entries = v.as_array().or_else(|| v["commands"].as_array()).ok_or("commands.json must contain an array")?;
    let mut out:Vec<UserCommand> = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().filter(|s| !s.trim().is_empty() && !s.contains('\n')).ok_or("Command needs a name")?;
        if out.iter().any(|c| c.name == name) { return Err(format!("Duplicate command: {name}")); }
        let command = entry["command"].as_str().ok_or("Command needs shell text")?.to_string();
        let output = entry["output"].as_str().unwrap_or("show");
        if !matches!(output,"replace"|"insert"|"show") { return Err(format!("Unknown command output mode: {output}")); }
        let keys = if let Some(key) = entry["key"].as_str() { vec![key] } else {
            entry["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).collect()
        };
        let mut chords = Vec::new();
        for key in keys {
            let chord = Chord::parse(key).ok_or_else(|| format!("Invalid command key: {key}"))?;
            if !chord.mods.ctrl && !chord.mods.alt && !matches!(chord.key,Key::F(_)) { return Err("Command keys need Ctrl, Alt or a function key".into()); }
            if out.iter().any(|c| c.keys.iter().any(|k| k.code() == chord.code())) { return Err(format!("Duplicate command key: {key}")); }
            chords.push(chord);
        }
        out.push(UserCommand { name:name.into(),command,output:output.into(),keys:chords });
    }
    Ok(out)
}
impl App {
    pub(super) fn user_command_list(&mut self) {
        match load(&self.root) { Ok(_) => self.open_palette(">User:"), Err(e) => self.message = Some(e) }
    }
    pub(super) fn user_command_entries(&self) -> Vec<Entry> {
        load(&self.root).unwrap_or_default().into_iter().map(|c| Entry::new(&format!("c\nuser_command:{}",c.name),&format!("User: {}",c.name),&c.command).keys(c.keys.iter().map(Chord::code).collect::<Vec<_>>().join(" / "))).collect()
    }
    fn command_key(&self) -> (u64,u64,usize,usize) { (self.file_tabs[self.active_file].id,self.doc.buf.version,self.doc.buf.anchor,self.doc.buf.cur) }
    pub(super) fn user_command_key(&mut self,key:Key,mods:Mods) -> bool {
        if self.focus != Focus::Doc || self.overlaid() || (!mods.ctrl && !mods.alt && !matches!(key,Key::F(_))) { return false; }
        let code = Chord::new(key,mods).code();
        let Some(c) = load(&self.root).unwrap_or_default().into_iter().find(|c| c.keys.iter().any(|k| k.code() == code)) else { return false };
        self.user_command_run(&c.name); true
    }
    pub(super) fn user_command_run(&mut self,name:&str) {
        if self.user_commands.running.is_some() { self.message = Some("A user command is running. Use User: Stop command.".into()); return; }
        let c = match load(&self.root).and_then(|v| v.into_iter().find(|c| c.name == name).ok_or("Command not found".into())) {
            Ok(c) => c, Err(e) => { self.message = Some(e); return; }
        };
        if c.output != "show" && (self.doc.read_only || self.reader.is_some() || self.nb.is_some()) { self.message = Some("Insert and replace commands require an editable text file".into()); return; }
        self.unsuggest();
        let mut cmd = self.project_command(&c.command,120);
        cmd.env("FILE",self.doc.path.as_deref().unwrap_or(Path::new("")))
            .env("LINE",(self.doc.buf.line_of(self.doc.buf.cur)+1).to_string())
            .env("SELECTION",self.doc.selected().unwrap_or_default());
        let job = Job::start_text(cmd,self.waker.clone());
        self.message = Some(format!("Running {}",c.name));
        self.user_commands.running = Some(Running { job,command:c,key:self.command_key(),root:self.root.clone() });
    }
    fn user_command_finish(&mut self,out:String,ok:bool) {
        let Some(r) = self.user_commands.running.take() else { return };
        if !ok || r.command.output == "show" { self.open_output(out); self.message = Some(format!("{}: {}",r.command.name,if ok {"finished"} else {"failed"})); return; }
        if r.root != self.root || r.key != self.command_key() {
            self.open_output(out); self.message = Some("Command target changed; output opened for review".into()); return;
        }
        let (mut a,b) = self.doc.buf.sel(); if r.command.output == "insert" { a = b; }
        self.doc.edit(a,b,&out,EditKind::Other); self.doc.buf.commit(); self.doc.moved(); self.edited();
        self.message = Some(format!("{} finished. Review and save.",r.command.name));
    }
    pub(super) fn user_commands_poll(&mut self) {
        if let Some((out,ok)) = self.user_commands.running.as_ref().and_then(|r| r.job.poll()) { self.user_command_finish(out,ok); }
    }
    pub(super) fn user_command_stop(&mut self) {
        self.user_commands.running.take(); self.message = Some("User command stopped".into());
    }
    #[cfg(any(test,feature="dump"))]
    pub(super) fn user_commands_settle(&mut self) {
        if let Some((out,ok)) = self.user_commands.running.as_ref().and_then(|r| r.job.wait()) { self.user_command_finish(out,ok); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_receive_literal_environment_and_replace_undoably() {
        let root = std::env::temp_dir().join(format!("ld-ab9-commands-{}",std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap(); std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let config = r#"[{"name":"Echo","command":"printf '%s\\n' \"$SELECTION\"","output":"replace","key":"ctrl+alt+u"}]"#;
        std::fs::write(root.join(".lightdown/commands.json"),config).unwrap();
        let file = root.join("a.md"); std::fs::write(&file,"$(touch NEVER) 'é'").unwrap();
        let mut a = App::new(Some(file.clone())); let before = a.doc.buf.text.clone(); a.doc.select(0,before.len());
        assert!(a.user_command_entries()[0].label.contains("Echo"));
        a.user_command_run("Echo"); a.user_commands_settle(); assert_eq!(a.doc.buf.text,format!("{before}\n"));
        assert!(!root.join("NEVER").exists()); assert_eq!(std::fs::read_to_string(file).unwrap(),before);
        a.doc.key(Key::Char('z'),Mods {ctrl:true,..Default::default()},None); assert_eq!(a.doc.buf.text,before);
        a.user_command_run("Echo"); a.doc.insert("changed",EditKind::Other); a.user_commands_settle();
        assert!(a.doc.read_only); assert!(a.message.as_ref().unwrap().contains("changed"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
