//! Enclosing outline entries stay at the pane's top after scrolling away.
use super::*;
use crate::{font,ui};
impl Pane {
    pub(crate) fn sticky_draw(&mut self,c:&mut Canvas) {
        self.sticky_hits.clear();
        if !self.sticky || !self.numbers || self.chat || self.doc == Doc::Cells || self.w <= 0.0 || self.h < theme::ROW*self.scale*3.0 { return; }
        if self.sticky_cache.as_ref().is_none_or(|(v,_)| *v != self.buf.version) {
            let blocks = self.fold_ranges();
            let headings = crate::outline::items(&self.buf.text,self.doc);
            let entries = headings.into_iter().filter_map(|h| {
                let line = self.buf.line_of(h.at);
                let end = blocks.iter().filter(|(a,_)| *a == line).map(|(_,b)| *b).max()?;
                Some((line,end,h.title))
            }).collect();
            self.sticky_cache = Some((self.buf.version,entries));
        }
        let top = self.visible.first().map_or(self.top,|(i,_)| *i);
        let entries: Vec<_> = self.sticky_cache.as_ref().unwrap().1.iter().filter(|(a,b,_)| *a < top && *b >= top).cloned().collect();
        let rh = theme::ROW*self.scale;
        let count = entries.len().min(3).min((self.h/rh/3.0) as usize);
        let y = self.y;
        c.clip(self.x,self.y,self.w,self.h);
        c.rect(self.x,y,self.w,count as f32*rh,theme::raised());
        let mut fonts = self.fonts.borrow_mut();
        for (i,(line,_,title)) in entries.iter().rev().take(count).collect::<Vec<_>>().into_iter().rev().enumerate() {
            let row_y = y+i as f32*rh;
            let name = format!("{}  {}",line+1,title);
            let text = ui::fit(&mut fonts,font::UI_MEDIUM,theme::UI_TEXT*self.scale,&name,(self.w-24.0*self.scale).max(0.0));
            ui::text(c,&mut fonts,self.x+12.0*self.scale,ui::base(row_y,rh,theme::UI_TEXT*self.scale),&text,font::UI_MEDIUM,theme::UI_TEXT*self.scale,theme::dim());
            self.sticky_hits.push(((self.x,row_y,self.w,rh),self.buf.starts[*line]));
        }
        if count > 0 { c.rect(self.x,y+count as f32*rh-1.0,self.w,1.0,theme::line()); }
        c.unclip();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headers_follow_enclosing_sections_and_click_back_to_source() {
        let mut p = Pane::new(Rc::new(RefCell::new(Fonts::new())),Rc::new(RefCell::new(Pics::new())));
        p.numbers = true;
        p.set_text(format!("# First\n{}\n# Second\n{}", "body\n".repeat(30),"other\n".repeat(30)),Doc::Markdown);
        p.set_rect(0.0,0.0,600.0,300.0,1.0);
        p.scroll_lines(12.0);
        let mut pixels = vec![0;600*300]; p.draw(&mut Canvas::new(&mut pixels,600,300));
        assert_eq!(p.sticky_hits.len(),1);
        assert_eq!(p.sticky_hits[0].1,0);
        p.press(100.0,10.0,Mods::default(),1); assert_eq!(p.buf.cur,0);
        p.sticky = false; p.draw(&mut Canvas::new(&mut pixels,600,300)); assert!(p.sticky_hits.is_empty());
    }
}
