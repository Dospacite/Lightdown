//! Prose masking for optional grammar checks.
use super::*;

impl Pane {
    /// Blanking excluded spans preserves UTF-16 positions in LanguageTool results.
    pub(crate) fn grammar_text(&self) -> String {
        let mut out = String::new(); let mut attrs = Vec::new();
        for li in 0..self.buf.lines() {
            let (a,b) = self.buf.line(li); let source = &self.buf.text[a..b];
            let prose = self.md.as_ref().and_then(|m| m.lines.get(li)).is_none_or(|l| !matches!(l.kind,Kind::Code|Kind::FenceOpen|Kind::FenceClose|Kind::Meta|Kind::Html));
            if let Some(md) = &self.md { md.attrs(a,b,usize::MAX,true,&mut attrs); } else { attrs.clear(); }
            let skip = if self.doc == Doc::Latex { md::KEYWORD|md::SOFT|md::DIM } else { md::CODE|md::LINK|md::HIDDEN|md::SOFT };
            for (i,c) in source.char_indices() {
                if !prose || attrs.get(i).is_some_and(|a| a&skip != 0) { for _ in 0..c.len_utf16() { out.push(' '); } }
                else { out.push(c); }
            }
            if b < self.buf.text.len() { out.push('\n'); }
        }
        out
    }
}
