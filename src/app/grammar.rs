//! Optional grammar checks use a local LanguageTool HTTP server, only on request.
use super::*;
use crate::tools::Job;
use serde_json::Value;
#[derive(Clone)]
struct Issue { a:usize,b:usize,message:String,fixes:Vec<String> }
struct Running { job:Job,key:(u64,u64),text:String,temp:PathBuf }
impl Drop for Running { fn drop(&mut self) { let _ = std::fs::remove_file(&self.temp); } }
#[derive(Default)]
pub(super) struct Grammar { running:Option<Running>,key:Option<(u64,u64)>,issues:Vec<Issue>,fix:Option<Issue> }
fn byte_at_utf16(text:&str,n:usize) -> Option<usize> {
    let mut units = 0;
    for (i,c) in text.char_indices() { if units == n { return Some(i); } units += c.len_utf16(); }
    (units == n).then_some(text.len())
}
fn issues(text:&str,v:&Value) -> Vec<Issue> {
    v["matches"].as_array().into_iter().flatten().filter_map(|v| {
        let n = usize::try_from(v["offset"].as_u64()?).ok()?;
        let len = usize::try_from(v["length"].as_u64()?).ok()?;
        let (a,b) = (byte_at_utf16(text,n)?,byte_at_utf16(text,n.checked_add(len)?)?);
        if a == b { return None; }
        Some(Issue { a,b,message:v["message"].as_str().unwrap_or("Grammar or style").into(),fixes:v["replacements"].as_array().into_iter().flatten().filter_map(|r| r["value"].as_str().map(str::to_string)).take(6).collect() })
    }).take(1000).collect()
}
pub(super) fn endpoint(url:&str) -> bool {
    let Some(host) = url.strip_prefix("http://") else { return false };
    ["localhost:","127.0.0.1:","[::1]:"].iter().any(|prefix| host.strip_prefix(prefix).is_some_and(|port| port.parse::<u16>().is_ok_and(|n| n > 0)))
}
impl App {
    fn grammar_key(&self) -> (u64,u64) { (self.file_tabs[self.active_file].id,self.doc.buf.version) }
    pub(super) fn grammar_check(&mut self) {
        if self.grammar.running.is_some() { self.message = Some("A grammar check is running".into()); return; }
        if self.reader.is_some() || self.doc.read_only || self.nb.is_some() || !matches!(self.doc.doc,Doc::Text|Doc::Markdown|Doc::Latex) {
            self.message = Some("Grammar checks require a prose text document".into()); return;
        }
        let run = || -> Result<(std::process::Command,PathBuf),String> {
            let bytes = std::fs::read(self.root.join(".lightdown/grammar.json")).map_err(|_| "Configure a local LanguageTool server in .lightdown/grammar.json")?;
            if bytes.len() > 512*1024 { return Err("grammar.json exceeds 512 KB".into()); }
            let v:Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let url = v["url"].as_str().unwrap_or("http://127.0.0.1:8081").trim_end_matches('/');
            if !endpoint(url) { return Err("Grammar URL must be a local HTTP host and port".into()); }
            if self.doc.buf.text.len() > 256*1024 { return Err("Grammar checks are limited to 256 KB".into()); }
            let dir = config::cache_dir().join("grammar"); std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let temp = dir.join(format!("{}-{}.txt",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()));
            config::write_private(&temp,self.doc.grammar_text().as_bytes()).map_err(|e| e.to_string())?;
            let mut cmd = std::process::Command::new("curl");
            cmd.args(["--fail","--silent","--show-error","--noproxy","*","--max-time","15","--data-urlencode"])
                .arg(format!("language={}",v["language"].as_str().unwrap_or("auto")))
                .arg("--data-urlencode").arg(format!("text@{}",temp.display())).arg(format!("{url}/v2/check"));
            Ok((cmd,temp))
        };
        match run() {
            Ok((cmd,temp)) => {
                let job = Job::start_text(cmd,self.waker.clone());
                self.grammar.running = Some(Running {job,temp,key:self.grammar_key(),text:self.doc.buf.text.clone()});
                self.message = Some("Checking grammar and style…".into());
            }
            Err(e) => self.message = Some(e),
        }
    }
    fn grammar_finish(&mut self,out:String,ok:bool) {
        let Some(r) = self.grammar.running.take() else { return };
        if r.key != self.grammar_key() { self.message = Some("Discarded stale grammar result".into()); return; }
        if !ok { self.message = Some(format!("Grammar: {}",crate::tools::clip_line(&out,200))); return; }
        match serde_json::from_str::<Value>(&out) {
            Ok(v) if v["matches"].is_array() => {
                self.grammar.issues = issues(&r.text,&v); self.grammar.key = Some(r.key);
                self.message = Some(format!("{} grammar and style suggestions",self.grammar.issues.len()));
            }
            _ => self.message = Some("LanguageTool returned an invalid response".into()),
        }
    }
    pub(super) fn grammar_poll(&mut self) {
        if let Some((out,ok)) = self.grammar.running.as_ref().and_then(|r| r.job.poll()) { self.grammar_finish(out,ok); }
    }
    pub(super) fn grammar_clear(&mut self) { self.grammar = Default::default(); self.doc.misspelled.clear(); }
    pub(super) fn grammar_marks(&mut self) {
        if self.grammar.key == Some(self.grammar_key()) && self.reader.is_none() {
            self.doc.misspelled.extend(self.grammar.issues.iter().map(|i| (i.a,i.b))); self.doc.misspelled.sort_unstable(); self.doc.misspelled.dedup();
        }
    }
    pub(super) fn grammar_items(&mut self) -> Vec<ui::Item> {
        if self.grammar.key != Some(self.grammar_key()) { return Vec::new(); }
        let Some(issue) = self.grammar.issues.iter().find(|i| i.a <= self.doc.buf.cur && self.doc.buf.cur <= i.b).cloned() else { return Vec::new() };
        let mut out = vec![ui::Item::new("grammar_message",&issue.message,"").when(false)];
        for (n,fix) in issue.fixes.iter().enumerate() { out.push(ui::Item::new(&format!("grammar_fix_{n}"),fix,"")); }
        out.push(ui::Item::Rule); self.grammar.fix = Some(issue); out
    }
    pub(super) fn grammar_pick(&mut self,key:&str) {
        if self.grammar.key != Some(self.grammar_key()) { return; }
        let Some(i) = self.grammar.fix.take() else { return };
        let Some(fix) = key.strip_prefix("grammar_fix_").and_then(|s| s.parse::<usize>().ok()).and_then(|n| i.fixes.get(n)) else { return };
        self.doc.edit(i.a,i.b,fix,EditKind::Other); self.doc.buf.commit(); self.doc.moved(); self.edited();
    }
    #[cfg(any(test,feature="dump"))]
    pub(super) fn grammar_settle(&mut self) {
        if let Some((out,ok)) = self.grammar.running.as_ref().and_then(|r| r.job.wait()) { self.grammar_finish(out,ok); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_http_checks_utf16_and_offers_undoable_fixes() {
        use std::io::{BufRead,Read,Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); let port = server.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (stream,_) = server.accept().unwrap(); let mut reader = std::io::BufReader::new(stream);
            let mut length = 0;
            loop { let mut line = String::new(); reader.read_line(&mut line).unwrap(); if line=="\r\n" { break; } if let Some((k,v)) = line.split_once(':') { if k.eq_ignore_ascii_case("content-length") { length = v.trim().parse().unwrap(); } } }
            let mut body = vec![0;length]; reader.read_exact(&mut body).unwrap();
            assert!(String::from_utf8_lossy(&body).contains("helo"));
            let body = r#"{"matches":[{"offset":3,"length":4,"message":"Use hello","replacements":[{"value":"hello"}]}]}"#;
            write!(reader.get_mut(),"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let root = std::env::temp_dir().join(format!("ld-ab9-grammar-{}",std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap(); std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join(".lightdown/grammar.json"),format!(r#"{{"url":"http://127.0.0.1:{port}","language":"en-US"}}"#)).unwrap();
        std::fs::write(root.join("a.md"),"😀 helo").unwrap(); let mut a = App::new(Some(root.join("a.md")));
        a.grammar_check(); a.grammar_settle(); worker.join().unwrap(); a.grammar_marks(); assert_eq!(a.doc.misspelled,vec![(5,9)]);
        a.doc.select(6,6); assert_eq!(a.grammar_items().len(),3); a.grammar_pick("grammar_fix_0"); assert_eq!(a.doc.buf.text,"😀 hello");
        a.doc.key(Key::Char('z'),Mods {ctrl:true,..Default::default()},None); assert_eq!(a.doc.buf.text,"😀 helo");
        assert!(!endpoint("https://example.org")); assert!(!endpoint("http://localhost:8081.evil"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
