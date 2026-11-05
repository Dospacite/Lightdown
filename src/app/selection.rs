//! Selection growth is reversible until the buffer or selection changes.
use super::*;
#[derive(Default)]
pub(super) struct Selection {
    key: Option<(u64,u64,usize,usize)>,
    history: Vec<(usize,usize)>,
    ranges: Vec<(usize,usize)>,
}
impl App {
    fn selection_key(&self) -> (u64,u64,usize,usize) { (self.file_tabs[self.active_file].id,self.doc.buf.version,self.doc.buf.anchor,self.doc.buf.cur) }
    pub(super) fn selection_change(&mut self,shrink:bool) {
        if self.reader.is_some() || self.doc.ghost().is_some() { return; }
        if self.selection.key != Some(self.selection_key()) { self.selection = Default::default(); }
        if shrink {
            if let Some((a,b)) = self.selection.history.pop() { self.doc.select(a,b); self.doc.moved(); }
            self.selection.key = Some(self.selection_key()); return;
        }
        let (a,b) = self.doc.buf.sel();
        let cur = self.doc.buf.cur;
        let mut ranges = lexical(&self.doc.buf.text,cur);
        ranges.extend(crate::treesitter::ranges(&self.doc.buf.text,a,b));
        for (start,end) in self.doc.fold_ranges() { ranges.push((self.doc.buf.starts[start],self.doc.buf.line(end).1)); }
        if self.doc.doc == Doc::Cells {
            let (s,e) = self.doc.cell_body(self.doc.cell_at(cur)); ranges.retain(|&(a,b)| a >= s && b <= e); ranges.push((s,e));
        }
        ranges.extend(&self.selection.ranges);
        ranges.sort_by_key(|&(a,b)| b-a); ranges.dedup();
        if let Some((s,e)) = ranges.into_iter().find(|&(s,e)| s <= a && e >= b && (s < a || e > b)) {
            self.selection.history.push((self.doc.buf.anchor,cur)); self.doc.select(s,e); self.doc.moved();
            self.selection.key = Some(self.selection_key());
            if self.selection.ranges.is_empty() && self.nb.is_none() { self.language_selection_request(cur); }
        }
    }
    pub(super) fn selection_ranges(&mut self,v:&serde_json::Value) {
        if self.selection.key != Some(self.selection_key()) { return; }
        let mut node = &v[0];
        for _ in 0..128 {
            if let (Some(a),Some(b)) = (crate::lsp::offset(&self.doc.buf.text,&node["range"]["start"]),crate::lsp::offset(&self.doc.buf.text,&node["range"]["end"])) {
                if a <= b { self.selection.ranges.push((a,b)); }
            }
            if node["parent"].is_null() { break; } node = &node["parent"];
        }
    }
}
fn lexical(text:&str,cur:usize) -> Vec<(usize,usize)> {
    let mut out = vec![editor::word_at(text,cur),(0,text.len())];
    let a = text[..cur].rfind('\n').map_or(0,|i| i+1);
    let b = text[cur..].find('\n').map_or(text.len(),|i| cur+i);
    out.push((a,b));
    let mut brackets = Vec::new(); let mut quote = None; let mut chars = text.char_indices().peekable();
    let mut comment = false;
    while let Some((i,c)) = chars.next() {
        if comment { if c == '\n' { comment = false; } continue; }
        if let Some((start,q)) = quote {
            if c == '\\' { chars.next(); } else if c == q { out.push((start,i+c.len_utf8())); quote = None; }
            continue;
        }
        if c == '/' && chars.peek().is_some_and(|(_,c)| *c == '/') { comment = true; continue; }
        if matches!(c,'\''|'"'|'`') { quote = Some((i,c)); continue; }
        if "([{".contains(c) { brackets.push((i,c)); }
        else if let Some(expected) = match c { ')' => Some('('),']' => Some('['),'}' => Some('{'),_ => None } {
            if brackets.last().is_some_and(|(_,c)| *c == expected) {
                let (start,_) = brackets.pop().unwrap(); out.push((start+1,i)); out.push((start,i+1));
            }
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grow_and_shrink_nested_unicode_and_reset_after_edits() {
        let mut a = App::new(None); a.doc.set_text("call([\"café\"])\n".into(),Doc::Code); a.doc.select(9,9);
        a.selection_change(false); assert_eq!(&a.doc.buf.text[a.doc.buf.sel().0..a.doc.buf.sel().1],"café");
        let word = a.doc.buf.sel(); a.selection_change(false); assert_eq!(&a.doc.buf.text[a.doc.buf.sel().0..a.doc.buf.sel().1],"\"café\"");
        a.selection_change(true); assert_eq!(a.doc.buf.sel(),word);
        a.selection_change(true); assert_eq!(a.doc.buf.sel(),(9,9));
        a.selection_change(false); a.doc.insert("x",EditKind::Other); let before = a.doc.buf.sel(); a.selection_change(true); assert_eq!(a.doc.buf.sel(),before);
    }
}
