//! LaTeX support that needs no TeX installation: token classes for
//! highlighting, the environment Enter closes, and the names offered for
//! completion.

use std::collections::HashSet;

use crate::md::{self, Span};
use crate::py::{Candidate, Query};

/// Commands offered for completion besides those the document uses.
const COMMANDS: [&str; 112] = [
    "documentclass",
    "usepackage",
    "begin",
    "end",
    "title",
    "author",
    "date",
    "maketitle",
    "tableofcontents",
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
    "appendix",
    "label",
    "ref",
    "eqref",
    "pageref",
    "cite",
    "footnote",
    "caption",
    "item",
    "includegraphics",
    "input",
    "include",
    "bibliography",
    "bibliographystyle",
    "printbibliography",
    "addbibresource",
    "newcommand",
    "renewcommand",
    "newenvironment",
    "textbf",
    "textit",
    "emph",
    "underline",
    "texttt",
    "textsc",
    "textrm",
    "textsf",
    "url",
    "href",
    "hline",
    "centering",
    "newpage",
    "clearpage",
    "newline",
    "linebreak",
    "noindent",
    "vspace",
    "hspace",
    "quad",
    "qquad",
    "frac",
    "dfrac",
    "sqrt",
    "sum",
    "prod",
    "int",
    "iint",
    "oint",
    "lim",
    "infty",
    "partial",
    "nabla",
    "cdot",
    "cdots",
    "ldots",
    "times",
    "leq",
    "geq",
    "neq",
    "approx",
    "equiv",
    "in",
    "notin",
    "subset",
    "subseteq",
    "cup",
    "cap",
    "forall",
    "exists",
    "rightarrow",
    "leftarrow",
    "Rightarrow",
    "Leftrightarrow",
    "mapsto",
    "alpha",
    "beta",
    "gamma",
    "delta",
    "epsilon",
    "theta",
    "lambda",
    "mu",
    "pi",
    "sigma",
    "phi",
    "omega",
    "Gamma",
    "Delta",
    "Omega",
    "mathbb",
    "mathcal",
    "mathrm",
    "mathbf",
    "left",
    "right",
    "text",
];

/// Environments offered after `\begin{`.
const ENVIRONMENTS: [&str; 32] = [
    "document",
    "itemize",
    "enumerate",
    "description",
    "figure",
    "table",
    "tabular",
    "center",
    "flushleft",
    "flushright",
    "quote",
    "quotation",
    "verbatim",
    "abstract",
    "equation",
    "equation*",
    "align",
    "align*",
    "gather",
    "gather*",
    "multline",
    "split",
    "cases",
    "matrix",
    "pmatrix",
    "bmatrix",
    "array",
    "minipage",
    "theorem",
    "proof",
    "lstlisting",
    "thebibliography",
];

/// Environments whose body is mathematics.
const MATH: [&str; 13] = [
    "equation",
    "equation*",
    "align",
    "align*",
    "gather",
    "gather*",
    "multline",
    "multline*",
    "displaymath",
    "math",
    "eqnarray",
    "eqnarray*",
    "flalign",
];

/// Environments whose body is taken as it is written.
const VERBATIM: [&str; 5] = ["verbatim", "verbatim*", "lstlisting", "minted", "comment"];

/// Commands whose argument names a label.
const REFS: [&str; 6] = ["ref", "eqref", "pageref", "autoref", "cref", "Cref"];

fn letter(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'@'
}

/// The name in braces at `at`, if one is there, and the offset after it.
fn braced(text: &str, at: usize) -> Option<(&str, usize)> {
    let rest = text.get(at..)?.strip_prefix('{')?;
    let close = rest.find(['}', '\n'])?;
    rest[close..].starts_with('}').then(|| (&rest[..close], at + close + 2))
}

/// Appends the styled stretches of `text`: commands, comments and
/// mathematics. Offsets are shifted by `base`.
pub fn spans(text: &str, base: usize, out: &mut Vec<Span>) {
    let b = text.as_bytes();
    let mut attr = vec![0 as crate::md::Attr; b.len()];
    let paint = |attr: &mut [crate::md::Attr], s: usize, e: usize, a: crate::md::Attr| attr[s..e.min(b.len())].iter_mut().for_each(|x| *x |= a);
    // Where the mathematics being read began, and what ends it.
    let mut math: Option<(usize, String)> = None;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let e = b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n);
                paint(&mut attr, i, e, md::DIM);
                i = e;
            }
            b'\\' if b.get(i + 1).copied().is_some_and(letter) => {
                let e = i + 1 + b[i + 1..].iter().take_while(|&&c| letter(c)).count();
                let name = &text[i + 1..e];
                paint(&mut attr, i, e, md::BOLD | md::KEYWORD);
                i = e;
                let Some((env, after)) = braced(text, e).filter(|_| matches!(name, "begin" | "end")) else { continue };
                paint(&mut attr, e, after, md::BOLD | md::KEYWORD);
                i = after;
                if name == "begin" && VERBATIM.contains(&env) {
                    let close = format!("\\end{{{env}}}");
                    i = text[i..].find(&close).map_or(b.len(), |n| i + n);
                } else if name == "begin" && MATH.contains(&env) && math.is_none() {
                    math = Some((e - 6, format!("\\end{{{env}}}")));
                } else if name == "end" && math.as_ref().is_some_and(|(_, m)| *m == text[e - 4..after]) {
                    let (from, _) = math.take().unwrap();
                    paint(&mut attr, from, after, md::SOFT);
                }
            }
            b'\\' => {
                let next = b.get(i + 1).copied().unwrap_or(0);
                let (open, close) = (matches!(next, b'(' | b'['), matches!(next, b')' | b']'));
                if open && math.is_none() {
                    math = Some((i, if next == b'(' { "\\)" } else { "\\]" }.to_string()));
                } else if close && math.as_ref().is_some_and(|(_, m)| m.as_bytes()[1] == next) {
                    let (from, _) = math.take().unwrap();
                    paint(&mut attr, from, i + 2, md::SOFT);
                }
                i += 2;
            }
            b'$' => {
                let double = b.get(i + 1) == Some(&b'$');
                let mark = if double { "$$" } else { "$" };
                match &math {
                    Some((from, m)) if m.as_str() == mark => {
                        paint(&mut attr, *from, i + mark.len(), md::SOFT);
                        math = None;
                    }
                    None => math = Some((i, mark.to_string())),
                    Some(_) => {}
                }
                i += mark.len();
            }
            // Inline mathematics cannot run on past a paragraph.
            b'\n' if b.get(i + 1) == Some(&b'\n') && math.as_ref().is_some_and(|(_, m)| !m.starts_with("\\end")) => {
                let (from, _) = math.take().unwrap();
                paint(&mut attr, from, i, md::SOFT);
                i += 1;
            }
            _ => i += 1,
        }
    }
    if let Some((from, _)) = math {
        paint(&mut attr, from, b.len(), md::SOFT);
    }
    let mut s = 0;
    while s < attr.len() {
        let e = s + attr[s..].iter().take_while(|&&a| a == attr[s]).count();
        if attr[s] != 0 {
            out.push(Span { start: (base + s) as u32, end: (base + e) as u32, attr: attr[s] });
        }
        s = e;
    }
}

/// The environment a line begins and leaves open, as in `\begin{itemize}`.
pub fn opens(line: &str) -> Option<&str> {
    let at = line.rfind("\\begin{")?;
    let (env, after) = braced(line, at + 6)?;
    // Arguments may follow, but nothing else.
    let rest = line[after..].trim();
    let args = rest.is_empty() || rest.starts_with(['[', '{']) && rest.ends_with([']', '}']);
    (args && !env.is_empty() && !line[after..].contains("\\end{")).then_some(env)
}

/// Whether an environment is begun more often than it is ended.
pub fn unclosed(text: &str, env: &str) -> bool {
    text.matches(&format!("\\begin{{{env}}}")).count() > text.matches(&format!("\\end{{{env}}}")).count()
}

/// The environments open at an offset, innermost last.
fn open_at(text: &str, off: usize) -> Vec<&str> {
    let mut open = Vec::new();
    let mut rest = &text[..off];
    let mut at = 0;
    while let Some(n) = rest.find('\\') {
        let from = at + n + 1;
        let word = &text[from..off];
        let (name, arg) = match word.strip_prefix("begin") {
            Some(_) => (true, from + 5),
            None if word.starts_with("end") => (false, from + 3),
            None => {
                at = from;
                rest = &text[at..off];
                continue;
            }
        };
        if let Some((env, after)) = braced(&text[..off], arg) {
            if name {
                open.push(env);
            } else if let Some(i) = open.iter().rposition(|e| *e == env) {
                open.truncate(i);
            }
            at = after;
        } else {
            at = from;
        }
        rest = &text[at..off];
    }
    open
}

/// What is being typed at the cursor, as far as completion cares: a
/// command after its backslash, or the name in the braces of `\begin`,
/// `\end` or a reference. `force` offers commands on the bare backslash.
pub fn query(text: &str, cur: usize, force: bool) -> Option<Query> {
    let b = text.as_bytes();
    let start = cur - b[..cur].iter().rev().take_while(|&&c| letter(c) || c == b'*' || c == b':').count();
    let before = &text[..start];
    if before.ends_with('\\') && !before.ends_with("\\\\") && !text[start..cur].contains([':', '*']) {
        let prefix = &text[start - 1..cur];
        return (force || prefix.len() > 1).then(|| Query { start: start - 1, prefix: prefix.into(), base: None });
    }
    let head = before.strip_suffix('{')?;
    let command = head.rsplit('\\').next().filter(|c| c.bytes().all(letter))?;
    let kind = match command {
        "begin" | "end" => command,
        c if REFS.contains(&c) => "ref",
        _ => return None,
    };
    Some(Query { start, prefix: text[start..cur].into(), base: Some(kind.into()) })
}

/// The names that fit a query: environments, labels, or commands from the
/// document and from the common ones.
pub fn candidates(text: &str, q: &Query, limit: usize) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut offer = |name: &str, kind: &str| {
        if name.len() > q.prefix.len() && name.starts_with(&q.prefix) && seen.insert(name.to_string()) {
            out.push(Candidate { name: name.to_string(), kind: kind.to_string() });
        }
    };
    let used = |command: &str| {
        let key = format!("\\{command}{{");
        let found = text.match_indices(&key).filter(|(i, _)| *i + key.len() != q.start);
        found.filter_map(|(i, _)| braced(text, i + key.len() - 1).map(|(n, _)| n.to_string())).collect::<Vec<_>>()
    };
    match q.base.as_deref() {
        Some("end") => {
            for env in open_at(text, q.start).into_iter().rev() {
                offer(env, "open");
            }
            for env in used("begin") {
                offer(&env, "");
            }
        }
        Some("begin") => {
            for env in used("begin") {
                offer(&env, "");
            }
            for env in ENVIRONMENTS {
                offer(env, "environment");
            }
        }
        Some(_) => {
            for label in used("label") {
                offer(&label, "label");
            }
        }
        None => {
            let b = text.as_bytes();
            let mut names: Vec<&str> = Vec::new();
            for (i, _) in text.match_indices('\\') {
                let e = i + 1 + b[i + 1..].iter().take_while(|&&c| letter(c)).count();
                if e > i + 1 && i != q.start {
                    names.push(&text[i..e]);
                }
            }
            names.sort_unstable();
            for name in names {
                offer(name, "");
            }
            for name in COMMANDS {
                offer(&format!("\\{name}"), "command");
            }
        }
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(text: &str) -> Vec<(&str, crate::md::Attr)> {
        let mut out = Vec::new();
        spans(text, 0, &mut out);
        out.iter().map(|s| (&text[s.start as usize..s.end as usize], s.attr)).collect()
    }

    #[test]
    fn tokens() {
        let soft_bold = md::SOFT | md::BOLD | md::KEYWORD;
        assert_eq!(
            styled("\\section{A} 50\\% done % note\n$x^\\alpha$ and \\[y\\]"),
            [
                ("\\section", md::BOLD | md::KEYWORD),
                ("% note", md::DIM),
                ("$x^", md::SOFT),
                ("\\alpha", soft_bold),
                ("$", md::SOFT),
                ("\\[y\\]", md::SOFT),
            ]
        );
        // Environments name themselves; the body of a verbatim one is
        // left as it is, and an unfinished `$` stops at the paragraph.
        assert_eq!(
            styled("\\begin{verbatim}\n% \\x $\n\\end{verbatim}\n$a\n\nb"),
            [
                ("\\begin{verbatim}", md::BOLD | md::KEYWORD),
                ("\\end{verbatim}", md::BOLD | md::KEYWORD),
                ("$a", md::SOFT)
            ]
        );
        let eq = styled("\\begin{align}\nx &= 1\n\\end{align} y");
        assert_eq!(eq, [("\\begin{align}", soft_bold), ("\nx &= 1\n", md::SOFT), ("\\end{align}", soft_bold)]);
    }

    #[test]
    fn environments_are_closed() {
        assert_eq!(opens("  \\begin{itemize}"), Some("itemize"));
        assert_eq!(opens("\\begin{figure}[ht]"), Some("figure"));
        assert_eq!(opens("\\begin{tabular}{ll}"), Some("tabular"));
        assert_eq!(opens("\\begin{a} text"), None);
        assert_eq!(opens("\\begin{a}\\end{a}"), None);
        assert!(unclosed("\\begin{a}\n\\begin{a}\n\\end{a}", "a") && !unclosed("\\begin{a}\\end{a}", "a"));
    }

    #[test]
    fn completion() {
        let names = |text: &str, force: bool| {
            let q = query(text, text.len(), force)?;
            Some(candidates(text, &q, 60).into_iter().map(|c| c.name).collect::<Vec<_>>())
        };
        let found = names("\\newcommand{\\vect}{}\n\\v", false).unwrap();
        assert_eq!(found[..2], ["\\vect", "\\vspace"]);
        assert!(names("\\", false).is_none() && names("\\", true).unwrap().len() == 60);
        assert!(names("a\\\\b", false).is_none() && names("word", false).is_none());
        // `\end{` offers what is open, innermost first.
        let open = "\\begin{document}\n\\begin{itemize}\n\\begin{enumerate}\\end{enumerate}\n\\end{";
        assert_eq!(names(open, false).unwrap()[..2], ["itemize", "document"]);
        assert_eq!(names("\\begin{ali", false).unwrap(), ["align", "align*"]);
        assert_eq!(names("\\label{fig:a} \\label{eq:b} \\ref{fig", false).unwrap(), ["fig:a"]);
    }
}
