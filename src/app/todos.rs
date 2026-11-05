//! Scan project comments on a worker, reusing the editor's lexical highlighters.
use super::*;
#[derive(Clone)]
struct Todo { path:PathBuf,at:usize,end:usize,line:usize,text:String }
type Results = Arc<std::sync::Mutex<Option<Vec<Todo>>>>;
#[derive(Default)]
pub(super) struct Todos { root:PathBuf,items:Vec<Todo>,pending:Option<Results> }
fn scan(path:&Path,text:&str,out:&mut Vec<Todo>) {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let mut spans = Vec::new(); crate::syntax::spans(text,0,ext,&mut spans);
    if matches!(ext,"md"|"markdown"|"html"|"htm") {
        let mut from = 0;
        while let Some(i) = text[from..].find("<!--") {
            let start = from+i; let end = text[start+4..].find("-->").map_or(text.len(),|i| start+4+i+3);
            spans.push(crate::md::Span { start:start as u32,end:end as u32,attr:crate::md::DIM }); from = end;
        }
    }
    for span in spans.iter().filter(|s| s.attr & crate::md::DIM != 0) {
        let (a,b) = (span.start as usize,span.end as usize);
        let Some(comment) = text.get(a..b) else { continue };
        for tag in ["TODO","FIXME","XXX"] {
            for (i,_) in comment.match_indices(tag) {
                let ident = |c:char| c.is_alphanumeric() || c == '_';
                if comment[..i].chars().next_back().is_some_and(ident) || comment[i+tag.len()..].chars().next().is_some_and(ident) { continue; }
                let at = a+i;
                let start = text[..at].rfind('\n').map_or(0,|i| i+1);
                let end = text[at..].find('\n').map_or(text.len(),|i| at+i);
                out.push(Todo { path:path.into(),at,end:at+tag.len(),line:text[..start].bytes().filter(|&b| b==b'\n').count()+1,text:text[start..end].trim().into() });
                if out.len() >= 1000 { return; }
            }
        }
    }
}
impl App {
    pub(super) fn open_todos(&mut self) {
        let mut snapshots = std::collections::HashMap::new();
        for (i,tab) in self.file_tabs.iter().enumerate() {
            let pane = if i == self.active_file { Some(&self.doc) } else {tab.state.as_ref().filter(|s| s.root == self.root || self.multi_root() && self.folders.contains(&s.root)).map(|s| &s.pane)};
            if let Some(pane) = pane.filter(|p| p.doc != Doc::Cells && !p.read_only) {
                if let Some(path) = &pane.path { if self.in_roots(path) && pane.buf.text.len() <= 2 << 20 { snapshots.insert(path.clone(),pane.buf.text.clone()); } }
            }
        }
        let result:Results = Arc::default(); let slot = result.clone(); let roots = self.roots(); let wake = self.waker.clone();
        std::thread::spawn(move || {
            let mut paths = super::roots::files_in(&roots); paths.extend(snapshots.keys().filter(|p| !paths.contains(p)).cloned().collect::<Vec<_>>()); paths.sort(); paths.dedup();
            let mut out = Vec::new(); let mut total = 0;
            for path in paths {
                let text = if let Some(text) = snapshots.remove(&path) {text} else {
                    if std::fs::metadata(&path).map_or(true,|m| m.len()>2 << 20) { continue; }
                    let Ok(text) = std::fs::read_to_string(&path) else { continue }; text
                };
                if text.contains('\0') { continue; } total += text.len(); if total > 64 << 20 { break; }
                scan(&path,&text,&mut out); if out.len() >= 1000 { break; }
            }
            out.sort_by(|a,b| (&a.path,a.at).cmp(&(&b.path,b.at))); *slot.lock().unwrap() = Some(out); wake();
        });
        self.todos = Todos { root:self.root.clone(),pending:Some(result),items:Vec::new() };
        self.menu = Some((MenuKind::Todos,Menu::new("Project TODO comments",Vec::new(),"Scanning comments…")));
    }
    pub(super) fn todos_poll(&mut self) {
        let Some(items) = self.todos.pending.as_ref().and_then(|slot| slot.lock().unwrap().take()) else { return };
        self.todos.pending = None;
        if self.todos.root != self.root { return; }
        self.todos.items = items;
        if matches!(self.menu,Some((MenuKind::Todos,_))) {
            let entries = self.todos.items.iter().enumerate().map(|(n,t)| Entry::new(&n.to_string(),&t.text,&format!("{}:{}",self.root_short(&t.path),t.line))).collect();
            self.menu = Some((MenuKind::Todos,Menu::new("Project TODO comments",entries,"")));
        }
    }
    pub(super) fn todo_pick(&mut self,key:&str) {
        if self.todos.root != self.root { return; }
        let Some(t) = key.parse::<usize>().ok().and_then(|n| self.todos.items.get(n)).cloned() else { return };
        self.jump_to(t.path.clone(),t.at,t.end);
        if self.doc.path.as_ref() == Some(&t.path) {
            let line = self.doc.buf.line_of(self.doc.buf.cur); let (a,b) = self.doc.buf.line(line);
            if self.doc.buf.text[a..b].trim() != t.text { self.message = Some("TODO source changed; refresh Go to: Project TODO comments".into()); }
        }
    }
    #[cfg(any(test,feature="dump"))]
    pub(super) fn todos_settle(&mut self) {
        while self.todos.pending.is_some() { self.todos_poll(); std::thread::sleep(std::time::Duration::from_millis(5)); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn todos_are_comment_tokens_not_strings_and_include_unsaved_tabs() {
        let path = PathBuf::from("a.rs"); let mut out = Vec::new();
        scan(&path,"let s = \"TODO ignored\"; // TODO: real\n/* FIXME: block\n XXX: last */\n// TODOS ignored\n",&mut out);
        assert_eq!(out.len(),3); assert_eq!(out[0].line,1);
        let root = std::env::temp_dir().join(format!("ld-ab9-todos-{}",std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap(); let path = root.join("a.py"); std::fs::write(&path,"# TODO: disk\n").unwrap();
        let mut a = App::new(Some(path.clone())); a.doc.select(0,a.doc.buf.text.len()); a.doc.insert("# FIXME: unsaved\n",EditKind::Other);
        a.open_todos(); a.todos_settle(); assert_eq!(a.todos.items.len(),1); assert!(a.todos.items[0].text.contains("unsaved"));
        a.menu = None; a.todo_pick("0"); assert_eq!(a.doc.selected().as_deref(),Some("FIXME"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
