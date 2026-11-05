//! Fold ranges use source line numbers. Editing unfolds the document.
use super::*;
impl Pane {
    pub(crate) fn fold_ranges(&self) -> Vec<(usize, usize)> {
        if matches!(self.doc, Doc::Code | Doc::Python | Doc::Markdown) && let Some(ranges) = crate::treesitter::folds(&self.buf.text) { return ranges; }
        let lines: Vec<_> = self.buf.text.split('\n').collect();
        let mut out = Vec::new();
        if self.doc == Doc::Markdown {
            let headings = crate::outline::items(&self.buf.text, self.doc);
            for (i, h) in headings.iter().enumerate() {
                let a = self.buf.line_of(h.at);
                let end = headings[i + 1..]
                    .iter()
                    .find(|n| n.depth <= h.depth)
                    .map_or(lines.len(), |n| self.buf.line_of(n.at));
                if end > a + 1 {
                    out.push((a, end - 1));
                }
            }
        }
        let mut braces = Vec::new();
        let mut envs: Vec<(String, usize)> = Vec::new();
        let mut quote = None;
        let mut block_comment = false;
        for (i, line) in lines.iter().enumerate() {
            if self.doc == Doc::Latex {
                if let Some(rest) = line.trim_start().strip_prefix("\\begin{").and_then(|r| r.split_once('}')) {
                    envs.push((rest.0.into(), i));
                }
                if let Some(rest) = line.trim_start().strip_prefix("\\end{").and_then(|r| r.split_once('}'))
                    && let Some(pos) = envs.iter().rposition(|(name, _)| name == rest.0)
                {
                    let (_, a) = envs.remove(pos);
                    if i > a {
                        out.push((a, i));
                    }
                }
            }
            let indent = line.len() - line.trim_start().len();
            if line.trim_end().ends_with(':') && !line.trim_start().starts_with('#') {
                let end = (i + 1..lines.len())
                    .find(|&n| !lines[n].trim().is_empty() && lines[n].len() - lines[n].trim_start().len() <= indent)
                    .unwrap_or(lines.len());
                let b = (i + 1..end).rev().find(|&n| !lines[n].trim().is_empty());
                if let Some(b) = b {
                    out.push((i, b));
                }
            }
            let mut chars = line.char_indices().peekable();
            while let Some((_, c)) = chars.next() {
                if block_comment {
                    if c == '*' && chars.peek().is_some_and(|(_, c)| *c == '/') {
                        chars.next();
                        block_comment = false;
                    }
                    continue;
                }
                if let Some(q) = quote {
                    if c == '\\' {
                        chars.next();
                    } else if c == q {
                        quote = None;
                    }
                    continue;
                }
                if c == '/' && chars.peek().is_some_and(|(_, c)| *c == '/') {
                    break;
                }
                if c == '/' && chars.peek().is_some_and(|(_, c)| *c == '*') {
                    chars.next();
                    block_comment = true;
                    continue;
                }
                if matches!(c, '"' | '\'') {
                    quote = Some(c);
                    continue;
                }
                if c == '{' {
                    braces.push(i);
                } else if c == '}'
                    && let Some(a) = braces.pop()
                    && i > a
                {
                    out.push((a, i));
                }
            }
            // Ordinary quoted strings do not continue over an unescaped newline.
            if !line.ends_with('\\') {
                quote = None;
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
    pub(crate) fn fold_hidden(&self, line: usize) -> bool {
        self.folds.iter().any(|&(a, b)| line > a && line <= b)
    }
    pub(crate) fn fold_toggle(&mut self) {
        let line = self.buf.line_of(self.buf.cur);
        if let Some(index) = self.folds.iter().position(|&(a, b)| line >= a && line <= b) {
            self.folds.remove(index);
        } else if let Some(range) =
            self.fold_ranges().into_iter().filter(|&(a, b)| a <= line && b >= line).max_by_key(|&(a, _)| a)
        {
            self.buf.cur = self.buf.starts[range.0];
            self.buf.anchor = self.buf.cur;
            self.folds.push(range);
        }
        self.top = self.top.min(self.buf.line_of(self.buf.cur));
        self.redraw = true;
    }
    pub(crate) fn unfold_all(&mut self) {
        self.folds.clear();
        self.redraw = true;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    #[test]
    fn folded_lines_disappear_and_jumps_reveal_them() {
        let mut a = App::new(None);
        a.resize(900, 700, 1.0);
        a.doc.set_text("fn example() {\n    let s = \"}\";\n    work();\n}\nafter();\n".into(), Doc::Code);
        a.doc.select(0, 0);
        assert!(a.doc.fold_ranges().contains(&(0, 3)));
        a.doc.fold_toggle();
        assert!(a.doc.fold_hidden(2));
        assert_eq!(a.doc.layout(2).h, 0.0);
        a.doc.select(32, 32);
        assert!(!a.doc.fold_hidden(2));
        a.doc.set_text("# A\ntext\n## B\ntext\n# C\nend".into(), Doc::Markdown);
        assert!(a.doc.fold_ranges().contains(&(0, 3)));
        a.doc.set_text("def f():\n    if True:\n        pass\nnext()\n".into(), Doc::Python);
        assert!(a.doc.fold_ranges().contains(&(0, 2)));
        a.doc.set_text("\\begin{itemize}\nitem\n\\end{itemize}\n".into(), Doc::Latex);
        assert!(a.doc.fold_ranges().contains(&(0, 2)));
    }
}
