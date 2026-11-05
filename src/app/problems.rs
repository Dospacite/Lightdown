//! One project-wide list for diagnostics, failed jobs and test failures.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Problem {
    pub path: Option<PathBuf>,
    pub line: usize,
    pub col: usize,
    pub message: String,
    pub source: &'static str,
}
#[derive(Default)]
pub(super) struct Problems {
    pub(super) ci: Vec<Problem>,
    pub rect: (f32,f32,f32,f32),
    rows: Vec<((f32,f32,f32,f32),Problem)>,
    selected: Option<Problem>,
    scroll: f32,
    height: f32,
}
impl Problems {
    pub fn contains(&self,x:f32,y:f32) -> bool { self.rect.2 > 0.0 && ui::inside(self.rect,Some((x,y))) }
    pub fn scroll(&mut self,dy:f32) { self.scroll = (self.scroll+dy).max(0.0).min((self.height-self.rect.3).max(0.0)); }
}
impl App {
    pub(super) fn problem_list(&self) -> Vec<Problem> {
        let mut list = self.language_problems();
        list.extend(self.config_problems());
        list.extend(self.problems.ci.iter().filter(|p| p.path.as_ref().is_some_and(|p| p.starts_with(&self.root))).cloned());
        list.extend(self.task_problems()); list.extend(self.test_problems()); list.extend(self.pdf_problems());
        list.retain(|p| p.path.as_ref().is_none_or(|p| p.starts_with(&self.root) || self.in_roots(p)));
        list.sort_by(|a,b| (&a.path,a.line,a.col,a.source,&a.message).cmp(&(&b.path,b.line,b.col,b.source,&b.message)));
        list.dedup(); list
    }
    pub(super) fn next_problem(&mut self,back:bool) {
        let list = self.problem_list();
        if list.is_empty() { self.message = Some("No problems reported".into()); return; }
        let n = list.len();
        let i = self.problems.selected.as_ref().and_then(|p| list.iter().position(|q| q == p))
            .map_or(if back {n-1} else {0},|i| if back {(i+n-1)%n} else {(i+1)%n});
        self.pick_problem(list[i].clone());
        self.message = Some(format!("{} of {}: {}",i+1,n,list[i].message));
    }
    fn pick_problem(&mut self,p:Problem) {
        self.problems.selected = Some(p.clone());
        if let Some(path) = p.path {
            self.remember_jump(); self.jump_to(path.clone(),0,0);
            if self.doc.path.as_ref() == Some(&path) && self.reader.is_none() {
                let (a,b) = self.doc.buf.line(p.line.saturating_sub(1).min(self.doc.buf.lines()-1));
                let at = if p.source == "LSP" {
                    crate::lsp::offset(&self.doc.buf.text,&serde_json::json!({"line":p.line.saturating_sub(1),"character":p.col.saturating_sub(1)})).unwrap_or(a)
                } else { a+self.doc.buf.text[a..b].char_indices().nth(p.col.saturating_sub(1)).map_or(b-a,|(n,_)| n) };
                self.doc.select(at,at); self.doc.moved();
            }
        }
        self.message = Some(format!("{}: {}",p.source,p.message));
    }
    pub(super) fn problem_at(&self,x:f32,y:f32) -> Option<Problem> {
        self.problems.rows.iter().find(|(r,_)| ui::inside(*r,Some((x,y)))).map(|(_,p)| p.clone())
    }
    pub(super) fn ask_problem(&mut self,p:&Problem) {
        let mut text = format!("{} diagnostic\nFile: {}\nLocation: {}:{}\n{}\n",p.source,p.path.as_deref().map(|p| self.root_short(p)).unwrap_or_else(|| "Project".into()),p.line,p.col,p.message);
        if let Some(path) = &p.path {
            if let Some((_,items)) = self.languages.diagnostics.get(path) {
                for d in items.iter().filter(|d| d["range"]["start"]["line"].as_u64() == Some(p.line.saturating_sub(1) as u64) && d["message"].as_str() == Some(p.message.as_str())) {
                    text.push_str(&format!("Range, zero-based UTF-16: {}\n",d["range"]));
                }
            }
            let source = if self.doc.path.as_ref() == Some(path) { Some(self.doc.buf.text.clone()) }
                else { self.file_tabs.iter().filter_map(|t| t.state.as_ref()).find(|s| s.pane.path.as_ref() == Some(path)).map(|s| s.pane.buf.text.clone())
                    .or_else(|| std::fs::metadata(path).ok().filter(|m| m.len() <= 2 << 20).and_then(|_| std::fs::read_to_string(path).ok())) };
            if let Some(source) = source {
                for (i,line) in source.lines().enumerate().skip(p.line.saturating_sub(4)).take(7) {
                    let end = editor::floor(line,line.len().min(1000));
                    text.push_str(&format!("{}: {}\n",i+1,&line[..end]));
                }
            }
        }
        self.draft_agent_question("diagnostic",&text);
    }
    pub(super) fn problems_press(&mut self,x:f32,y:f32) {
        if let Some((_,p)) = self.problems.rows.iter().find(|(r,_)| ui::inside(*r,Some((x,y)))).cloned() { self.pick_problem(p); }
    }
    pub(super) fn draw_problems(&mut self,c:&mut Canvas) {
        let list = self.problem_list();
        let (x,y,w,h) = self.problems.rect; let s = self.scale;
        if w <= 0.0 || h <= 0.0 { return; }
        self.problems.rows.clear();
        c.clip(x,y,w,h); c.rect(x,y,w,h,theme::bg());
        let mut fonts = self.fonts.borrow_mut();
        let mut cy = y+12.0*s-self.problems.scroll;
        let mut file = None;
        if list.is_empty() { ui::text(c,&mut fonts,x+12.0*s,cy+16.0*s,"No problems reported",font::UI,theme::UI_TEXT*s,theme::dim()); }
        for p in list {
            if file.as_ref() != Some(&p.path) {
                file = Some(p.path.clone());
                let name = p.path.as_deref().map(|p| self.root_short(p)).unwrap_or_else(|| "Project".into());
                let label = ui::fit(&mut fonts,font::UI_BOLD,theme::UI_TEXT*s,&name,(w-24.0*s).max(0.0));
                ui::text(c,&mut fonts,x+12.0*s,cy+16.0*s,&label,font::UI_BOLD,theme::UI_TEXT*s,theme::fg()); cy += 28.0*s;
            }
            let r = (x+6.0*s,cy,(w-12.0*s).max(0.0),48.0*s);
            if self.problems.selected.as_ref() == Some(&p) || ui::inside(r,Some(self.pointer)) { c.round_rect(r.0,r.1,r.2,r.3,theme::RADIUS_MD*s,theme::active()); }
            let label = ui::fit(&mut fonts,font::UI,theme::UI_TEXT*s,&p.message.replace('\n'," "),(w-32.0*s).max(0.0));
            ui::text(c,&mut fonts,x+16.0*s,cy+17.0*s,&label,font::UI,theme::UI_TEXT*s,theme::fg());
            ui::text(c,&mut fonts,x+16.0*s,cy+36.0*s,&format!("{} · {}:{}",p.source,p.line,p.col),font::UI,theme::SMALL*s,theme::dim());
            if cy+48.0*s >= y && cy < y+h { self.problems.rows.push((r,p)); }
            cy += 50.0*s;
        }
        self.problems.height = cy-y+self.problems.scroll+12.0*s;
        c.unclip();
    }
}
