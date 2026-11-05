//! Composition is an overlay. Only a committed string enters the buffer.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InputContext {
    pub key: (u64, u64, usize, usize, u64),
    pub text: String,
    pub cursor: i32,
    pub anchor: i32,
    pub rect: (i32, i32, i32, i32),
}
pub(super) struct Preedit {
    key: (u64, u64, usize, usize, u64),
    text: String,
    cursor: i32,
    end: i32,
}
impl App {
    fn input_pane(&mut self) -> Option<(u64, &mut Pane)> {
        if self.overlaid() || self.prompt_has_keys() { return None; }
        if self.focus == Focus::Doc && self.reader.is_none() && !self.doc.read_only && self.doc.vim_typing() {
            if self.doc.ghost().is_some() || self.inline_preview.is_some() || !self.doc.extra_cursors.is_empty() { return None; }
            Some((self.file_tabs[self.active_file].id, &mut self.doc))
        } else if self.focus == Focus::Chat && self.side_open && self.tab == Tab::Agent {
            Some((0, &mut self.ai.side.input))
        } else { None }
    }
    // A stable target number distinguishes fields even when their text is equal.
    fn input_field(&mut self) -> Option<(u64, &mut String, usize)> {
        let source = self.source.typing && self.source_shown() && self.focus == Focus::Files;
        if self.search_panel_has_keys() {
            let p = &mut self.search.panel;
            let f = if p.field == 1 { &mut p.replace } else if p.field == 0 { &mut p.query } else { return None };
            return Some((10 + p.field as u64, &mut f.text, f.cur));
        }
        if self.prompt_has_keys() {
            let p = self.prompt.as_mut()?;
            if matches!(p.kind, PromptKind::Trash | PromptKind::Reload) { return None; }
            let id = 20 + p.on_with as u64;
            let (t,c) = p.field();
            return Some((id,t,*c));
        }
        if let Some(c) = &mut self.chooser { let t = c.input_text(); let cur = t.len(); return Some((30,t,cur)); }
        if let Some((_,m)) = &mut self.menu { let t = m.input_text(); let cur = t.len(); return Some((40,t,cur)); }
        if self.popup.is_some() { return None; }
        if let Some(s) = &mut self.settings { return Some(s.input_text()); }
        if self.tests.typing && self.side_open && self.tab == Tab::Tests && !self.focus_mode {
            let cur = self.tests.filter.len(); return Some((60,&mut self.tests.filter,cur));
        }
        if source {
            return Some((70,&mut self.source.message,self.source.cur));
        }
        if self.focus == Focus::Doc && self.reader.is_none() {
            if let Some(t) = self.doc.vim_input() { let cur = t.len(); return Some((75,t,cur)); }
        }
        None
    }
    pub(super) fn input_press(&mut self, x: f32, y: f32) -> bool {
        let hit = ui::input_hit(&mut self.fonts.borrow_mut(),x,y);
        let Some((id,at)) = hit else { return false };
        let at = match self.input_field() {
            Some((same,text,_)) if same == id => editor::floor(text,at), _ => return false,
        };
        self.input_field_cursor(id,at);
        self.input_clear();
        true
    }
    fn input_field_cursor(&mut self, id: u64, cur: usize) {
        match id {
            10 => { self.search.panel.query.cur = cur; self.search_update(); }
            11 => self.search.panel.replace.cur = cur,
            20 | 21 => { if let Some(p) = &mut self.prompt { *p.field().1 = cur; } self.prompt_changed(); }
            30 => { if let Some(c) = &mut self.chooser { c.input_changed(); } }
            40 => {
                if let Some((_,m)) = &mut self.menu { m.input_changed(); }
                if let Some((kind,_)) = &self.menu {
                    if *kind == MenuKind::Palette { self.palette_update(); }
                    else if *kind == MenuKind::Search { self.search_update(); }
                }
            }
            50 | 51 => if let Some(s) = &mut self.settings { s.input_changed(cur); },
            70 => self.source.cur = cur,
            _ => {}
        }
    }
    pub(crate) fn input_context(&mut self) -> Option<InputContext> {
        if self.popup.is_some() || self.rebinding.is_some() { return None; }
        use std::hash::{Hash,Hasher};
        let mut identity = std::collections::hash_map::DefaultHasher::new();
        self.prompt.as_ref().map(|p| std::mem::discriminant(&p.kind)).hash(&mut identity);
        self.menu.as_ref().map(|(k,_)| std::mem::discriminant(k)).hash(&mut identity);
        let generation = self.ime_generation ^ identity.finish();
        let scale = self.scale;
        let field_rect = ui::input_caret().unwrap_or((20.0*scale, self.title_h()+30.0*scale, 1.0,20.0*scale));
        if let Some((id,text,cur)) = self.input_field() {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hash);
            let version = hash.finish();
            return Some(make_context((id,version,cur,cur,generation),text,cur,cur,field_rect,scale));
        }
        if !self.overlaid() && !self.prompt_has_keys() && self.focus == Focus::Term && self.term_open {
            let term = self.terms.get(self.term_cur)?;
            let rect = term.input_caret();
            return Some(make_context((80,term.agent_id,0,0,generation),"",0,0,rect,scale));
        }
        if self.search_panel_has_keys() { return None; }
        let (id, pane) = self.input_pane()?;
        let (x,y,h) = pane.caret()?;
        let buf = &pane.buf;
        Some(make_context((id+100,buf.version,buf.cur,buf.anchor,generation),&buf.text,buf.cur,buf.anchor,(x,y,1.0,h),scale))
    }
    pub(crate) fn preedit_clear(&mut self) {
        if self.preedit.take().is_some() { self.redraw = true; }
    }
    pub(crate) fn input_clear(&mut self) {
        self.preedit = None;
        self.ime_generation = self.ime_generation.wrapping_add(1);
        self.redraw = true;
    }
    pub(crate) fn input_apply(&mut self, expected: &InputContext, delete: (u32,u32), commit: Option<String>,
        preedit: Option<(String,i32,i32)>) {
        self.unsuggest();
        let Some(current) = self.input_context() else { self.input_clear(); return };
        if current.key != expected.key { self.input_clear(); return; }
        self.preedit = None;
        let changed = commit.is_some() || delete != (0,0);
        if changed {
            let id = current.key.0;
            if id >= 80 && id < 100 {
                if delete != (0,0) { return; }
                if let Some(text) = commit.as_deref() { self.terms[self.term_cur].input_commit(text); }
                self.ime_generation = self.ime_generation.wrapping_add(1);
            } else if id < 80 {
                let Some((_,text,cur)) = self.input_field() else { return };
                let Some((start,end)) = deletion_range(text,cur,cur,delete,expected) else { return };
                let ins = commit.as_deref().unwrap_or("").replace(['\n','\r'], " ");
                text.replace_range(start..end,&ins);
                self.input_field_cursor(id,start+ins.len());
            } else {
                let (_,pane) = self.input_pane().unwrap();
                let (a,b) = pane.buf.sel();
                let Some((start,end)) = deletion_range(&pane.buf.text,a,b,delete,expected) else { return };
                let replacement = commit.clone().unwrap_or_else(|| pane.buf.text[a..b].into());
                pane.buf.commit();
                pane.edit(start,end,&replacement,EditKind::Other);
                if commit.is_none() { pane.select(start,start+replacement.len()); }
                pane.buf.commit();
                pane.moved();
                if self.focus == Focus::Doc { self.edited(); }
            }
        }
        if let Some((text,cursor,end)) = preedit.filter(|(t,_,_)| !t.is_empty()) {
            if let Some(context) = self.input_context() {
                self.preedit = Some(Preedit { key: context.key, text, cursor, end });
            }
        }
        self.comp = None;
        self.collect();
        self.redraw = true;
    }
    pub(super) fn draw_preedit(&mut self, c: &mut Canvas) {
        let Some(context) = self.input_context() else { self.preedit = None; return };
        let Some(preedit) = self.preedit.as_ref().filter(|p| p.key == context.key) else { self.preedit = None; return };
        let text = preedit.text.replace(['\n','\r'], " ");
        let (x,y,_,h) = context.rect;
        let s = self.scale;
        let (x,y,h) = (x as f32*s,y as f32*s,h as f32*s);
        let size = self.doc.px().min(h*0.8).max(1.0);
        let mut fonts = self.fonts.borrow_mut();
        let width = fonts.width(font::UI,size,&text).min((self.w-x).max(1.0));
        c.clip(x,y,(self.w-x).max(0.0),h);
        c.rect(x,y,width+4.0*s,h,theme::raised());
        if preedit.cursor >= 0 && preedit.end >= 0 {
            let begin = editor::floor(&text,preedit.cursor as usize);
            let end = editor::floor(&text,preedit.end as usize);
            let left = crate::bidi::caret_x(&mut fonts,font::UI,size,&text,begin.min(end));
            let right = crate::bidi::caret_x(&mut fonts,font::UI,size,&text,begin.max(end));
            c.rect(x+left.min(right),y,(right-left).abs().max(s),h,theme::selection());
        }
        ui::text(c,&mut fonts,x,y+(h-size)/2.0+size*0.8,&text,font::UI,size,theme::fg());
        c.line(x,y+h-s,width,s,theme::caret());
        if preedit.cursor >= 0 {
            let at = editor::floor(&text,preedit.cursor as usize);
            let dx = crate::bidi::caret_x(&mut fonts,font::UI,size,&text,at);
            c.line(x+dx,y,s,h,theme::caret());
        }
        c.unclip();
    }
}
fn make_context(key: (u64,u64,usize,usize,u64), text: &str, cur: usize, anchor: usize, rect: ui::Rect, scale: f32) -> InputContext {
    let (a,b) = (cur.min(anchor),cur.max(anchor));
    let (start,end) = if b-a > 3500 { (cur,cur) } else {
        (editor::ceil(text,a.saturating_sub(200)),editor::floor(text,(b+200).min(text.len())))
    };
    InputContext { key, text: text[start..end].into(),cursor:(cur-start) as i32,
        anchor: if anchor >= start && anchor <= end { (anchor-start) as i32 } else { 0 },
        rect: ((rect.0/scale).round() as i32,(rect.1/scale).round() as i32,1,(rect.3/scale).ceil() as i32) }
}
fn deletion_range(text: &str, a: usize, b: usize, delete: (u32,u32), expected: &InputContext) -> Option<(usize,usize)> {
    let start = a.checked_sub(delete.0 as usize)?;
    let end = b.checked_add(delete.1 as usize)?;
    let before = expected.cursor.min(expected.anchor).max(0) as usize;
    let after = expected.text.len().saturating_sub(expected.cursor.max(expected.anchor).max(0) as usize);
    (delete.0 as usize <= before && delete.1 as usize <= after && end <= text.len()
        && text.is_char_boundary(start) && text.is_char_boundary(end)).then_some((start,end))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preedit_never_enters_text_and_commit_is_one_undo() {
        let mut a = App::new(None);
        a.resize(900,700,1.0);
        a.paste_text("café draft");
        a.doc.select(6,11);
        let context = a.input_context().unwrap();
        a.input_apply(&context,(0,0),None,Some(("中文".into(),0,3)));
        assert_eq!(a.doc.buf.text,"café draft");
        assert!(a.preedit.is_some());
        a.input_apply(&context,(0,0),Some("中文".into()),None);
        assert_eq!(a.doc.buf.text,"café 中文");
        a.doc.buf.undo();
        assert_eq!(a.doc.buf.text,"café draft");
        let context = a.input_context().unwrap();
        a.doc.select(0,0);
        a.input_apply(&context,(0,0),Some("stale".into()),None);
        assert_eq!(a.doc.buf.text,"café draft");
    }
    #[test]
    fn surrounding_text_is_bounded_and_unicode_deletion_is_checked() {
        let mut a = App::new(None);
        a.resize(900,700,1.0);
        a.paste_text("café");
        let context = a.input_context().unwrap();
        a.input_apply(&context,(1,0),Some("x".into()),None);
        assert_eq!(a.doc.buf.text,"café");
        a.input_apply(&context,(2,0),Some("e".into()),None);
        assert_eq!(a.doc.buf.text,"cafe");
        a.doc.set_text("中".repeat(5000), Doc::Text);
        a.doc.select(0,a.doc.buf.text.len());
        let context = a.input_context().unwrap();
        assert!(context.text.len() <= 4000);
        a.input_apply(&context,(0,0),Some("文".into()),None);
        assert_eq!(a.doc.buf.text,"文");
    }
    fn compose(a: &mut App, text: &str) {
        let context = a.input_context().expect("focused field accepts IME");
        let old = context.text.clone();
        a.input_apply(&context,(0,0),None,Some((text.into(),0,text.len() as i32)));
        assert_eq!(a.input_context().unwrap().text,old);
        a.input_apply(&context,(0,0),Some(text.into()),None);
        assert!(a.preedit.is_none());
    }
    #[test]
    fn composition_routes_to_dialog_palette_find_and_settings() {
        let mut a = App::new(None);
        a.resize(900,700,1.0);
        a.ask(PromptKind::New);
        let before = a.prompt.as_ref().unwrap().text.clone();
        compose(&mut a,"日本語");
        assert!(a.prompt.as_ref().unwrap().text.contains("日本語"));
        assert_ne!(a.prompt.as_ref().unwrap().text,before);
        a.prompt = None;
        a.menu = Some((MenuKind::Keys,Menu::new("Keys",vec![],"")));
        compose(&mut a,"עברית");
        assert_eq!(a.menu.as_ref().unwrap().1.typed(),"עברית");
        a.menu = Some((MenuKind::Palette,Menu::new("Commands",vec![],"")));
        compose(&mut a,"中文");
        assert_eq!(a.menu.as_ref().unwrap().1.typed(),"中文");
        a.menu = None;
        a.ask(PromptKind::Replace);
        compose(&mut a,"検索");
        a.prompt.as_mut().unwrap().on_with = true;
        compose(&mut a,"置換");
        assert_eq!(a.prompt.as_ref().unwrap().with,"置換");
        a.prompt = None;
        a.settings = Some(Settings::new());
        compose(&mut a,"خط");
        assert_eq!(a.input_context().unwrap().text,"خط");
        assert!(a.doc.buf.text.is_empty());
    }
    #[test]
    fn composition_routes_to_auxiliary_filters_and_rejects_old_target() {
        let mut a = App::new(None);
        a.resize(900,700,1.0);
        a.ask(PromptKind::New);
        let old = a.input_context().unwrap();
        a.ask(PromptKind::Rename);
        let before = a.prompt.as_ref().unwrap().text.clone();
        a.input_apply(&old,(0,0),Some("wrong".into()),None);
        assert_eq!(a.prompt.as_ref().unwrap().text,before);
        a.prompt = None;
        a.side_open = true;
        a.tab = Tab::Tests;
        a.tests.typing = true;
        compose(&mut a,"اختبار");
        assert_eq!(a.tests.filter,"اختبار");
        a.tests.typing = false;
        a.tab = Tab::Search;
        a.search.panel.rect = (500.0,40.0,350.0,600.0);
        a.search_panel_focus(0);
        compose(&mut a,"查找");
        assert_eq!(a.search.panel.query.text,"查找");
        a.search.panel.show_replace = true;
        a.search_panel_focus(1);
        compose(&mut a,"替换");
        assert_eq!(a.search.panel.replace.text,"替换");
        assert!(a.doc.buf.text.is_empty());
    }

    #[test]
    fn composition_terminal_commits_reach_the_pty_and_use_its_cursor() {
        let mut a = App::new(None);
        a.resize(900,700,1.0);
        let mut t = Term::new(&std::env::temp_dir(),a.fonts.clone(),Arc::new(|| {}));
        t.set_rect(0.0,350.0,900.0,300.0,1.0);
        t.settle();
        a.terms.push(t);
        a.term_cur = a.terms.len()-1;
        a.term_open = true;
        a.focus = Focus::Term;
        let context = a.input_context().unwrap();
        assert!(context.rect.1 >= 350);
        a.input_apply(&context,(0,0),None,Some(("中文".into(),0,6)));
        assert!(a.preedit.is_some());
        assert!(a.doc.buf.text.is_empty());
        a.key(Key::Escape,Mods::default(),None);
        assert!(a.preedit.is_none());
        compose(&mut a,"printf 'IME中文\\n'\r");
        a.terms[a.term_cur].settle();
        assert!(a.terms[a.term_cur].screen_text().contains("IME中文\n"));
        assert!(a.doc.buf.text.is_empty());
    }
    #[test]
    fn composition_candidate_rect_and_clicks_follow_a_dialog_caret() {
        let mut a = App::new(None);
        a.resize(900,700,1.0);
        a.ask(PromptKind::New);
        let p = a.prompt.as_mut().unwrap();
        p.text = "אבג".into(); p.cur = 0;
        let mut pixels = vec![0;900*700];
        a.draw(&mut Canvas::new(&mut pixels,900,700));
        let first = a.input_context().unwrap();
        a.prompt.as_mut().unwrap().cur = 4;
        a.draw(&mut Canvas::new(&mut pixels,900,700));
        let next = a.input_context().unwrap();
        assert!(next.rect.0 < first.rect.0);
        assert!(next.rect.1 > 0 && next.rect.3 > 0);
        a.press(first.rect.0 as f32,first.rect.1 as f32+3.0,Mods::default(),1);
        assert_eq!(a.prompt.as_ref().unwrap().cur,0);
    }

    #[test]
    fn composition_routes_to_chooser_and_vim_command_without_editing_document() {
        let mut a = App::new(None);
        a.resize(900,700,1.0);
        a.chooser = Some(Chooser::new(&std::env::temp_dir(),None));
        compose(&mut a,"文件");
        assert_eq!(a.input_context().unwrap().text,"文件");
        a.chooser = None;
        a.doc.vim.enabled = true;
        assert!(a.input_context().is_none());
        a.key(Key::Char(':'),Mods::default(),Some(":"));
        compose(&mut a,"edit שלום.md");
        assert_eq!(a.doc.vim_status().unwrap(),":edit שלום.md");
        assert!(a.doc.buf.text.is_empty());
    }

}
