//! Keeps markdown links pointing at what they named when a file or folder
//! moves, or a heading is renamed. Changes are offered in the project
//! replacement review: each file's diff can be read, then applied as
//! unsaved, undoable edits.
use super::*;
use std::path::Component;

/// The most markdown read when looking for links, as for project search.
const MOST: usize = 64 << 20;

/// `a/./b/../c` as `a/c`, without asking the disk.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    out
}

/// `to` as a path from the folder `from`, with `/` between names.
pub(super) fn relative(from: &Path, to: &Path) -> String {
    let (from, to) = (tidy(from), tidy(to));
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let same = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - same];
    parts.extend(b[same..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

/// A path written into a link destination.
fn encode(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            c => out.push(c),
        }
    }
    out
}

fn is_note(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Where an inline link's destination is in its source, as byte offsets.
/// Reference links, whose destination is elsewhere, have none.
fn destination(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let span = text.get(start..end)?;
    if !span.ends_with(')') {
        return None;
    }
    let open = start + span.rfind("](")? + 2;
    let inner = &text[open..end - 1];
    let lead = inner.len() - inner.trim_start().len();
    let rest = &inner[lead..];
    let len = if rest.starts_with('<') { rest.find('>')? + 1 } else { rest.find(char::is_whitespace).unwrap_or(rest.len()) };
    (len > 0).then_some((open + lead, open + lead + len))
}

/// A link's destination split into its path, and what follows it (`?`
/// or `#` and on). `None` for addresses and links within the document.
fn split_destination(raw: &str) -> Option<(&str, &str)> {
    let dest = raw.trim_start_matches('<').trim_end_matches('>');
    if dest.is_empty() || dest.starts_with('#') || dest.starts_with("//") || dest.contains(':') || dest.starts_with("~/") {
        return None;
    }
    let cut = dest.find(['?', '#']).unwrap_or(dest.len());
    Some((&dest[..cut], &dest[cut..]))
}

/// A move: a file or a folder, from `old` to `new`.
pub(super) struct Move {
    pub old: PathBuf,
    pub new: PathBuf,
}

impl Move {
    /// Where something that was at `p` is now.
    fn map(&self, p: &Path) -> PathBuf {
        match p.strip_prefix(&self.old) {
            Ok(rest) if rest.as_os_str().is_empty() => self.new.clone(),
            Ok(rest) => self.new.join(rest),
            Err(_) => p.to_path_buf(),
        }
    }
    /// Where something now at `p` was.
    fn unmap(&self, p: &Path) -> PathBuf {
        match p.strip_prefix(&self.new) {
            Ok(rest) if rest.as_os_str().is_empty() => self.old.clone(),
            Ok(rest) => self.old.join(rest),
            Err(_) => p.to_path_buf(),
        }
    }
    fn moved(&self, p: &Path) -> bool {
        p.starts_with(&self.old)
    }
}

/// Splices edits, given as ranges and their new text, into `text`.
fn splice(text: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (a, b, with) in edits {
        out.replace_range(a..b, &with);
    }
    out
}

/// A wikilink's parts: the name, then `#…` and `|…` as written.
fn wiki_parts(inner: &str) -> (&str, &str, &str) {
    let (target, label) = inner.split_once('|').map_or((inner, ""), |(t, _)| (t, &inner[t.len()..]));
    let (name, fragment) = target.split_once('#').map_or((target, ""), |(n, _)| (n, &target[n.len()..]));
    (name, fragment, label)
}

/// The note a wikilink named before a move, if it was in what moved: the
/// note beside the linking one, or from the project's folder, or by its
/// name alone.
fn wiki_target_before(root: &Path, file_before: &Path, name: &str, mv: &Move) -> Option<PathBuf> {
    let with_md = |p: PathBuf| if p.extension().is_none() { p.with_extension("md") } else { p };
    let name_path = Path::new(name);
    let beside = tidy(&with_md(file_before.parent().unwrap_or(root).join(name_path)));
    let from_root = tidy(&with_md(root.join(name_path)));
    for candidate in [beside, from_root] {
        if mv.moved(&candidate) {
            return Some(candidate);
        }
    }
    let stem = name_path.file_stem()?;
    (!name.contains('/') && is_note(&mv.old) && mv.old.file_stem() == Some(stem)).then(|| mv.old.clone())
}

/// A markdown file's links, rewritten for a move. `file` is where the file
/// is now; links that still lead where they did are left as written.
pub(super) fn after_move(root: &Path, file: &Path, text: &str, mv: &Move, notes: &[PathBuf]) -> String {
    let buf = editor::Buffer::new(text.into());
    let md = crate::md::parse(text, &buf.starts);
    let before = mv.unmap(file);
    let dir = file.parent().unwrap_or(root);
    let mut edits = Vec::new();
    for link in &md.links {
        let (start, end) = (link.start as usize, link.end as usize);
        if text[start..].starts_with("[[") && end >= start + 4 {
            let inner = &text[start + 2..end - 2];
            let (name, fragment, label) = wiki_parts(inner);
            if name.is_empty() {
                continue;
            }
            let Some(target) = wiki_target_before(root, &before, &pane::percent_decode(name), mv) else { continue };
            let target = mv.map(&target);
            let keep_md = name.ends_with(".md");
            let stem = target.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let unique = notes.iter().filter(|p| p.file_stem() == target.file_stem()).count() <= 1;
            let mut new_name = if !name.contains('/') && unique {
                stem
            } else {
                relative(root, &target).trim_end_matches(".md").to_string()
            };
            if keep_md {
                new_name += ".md";
            }
            // Only a name that leads to the note is written.
            if super::notes::resolve_note(root, Some(file), &new_name, notes).ok().as_deref() != Some(target.as_path()) {
                continue;
            }
            if new_name != name {
                edits.push((start + 2, end - 2, format!("{new_name}{fragment}{label}")));
            }
            continue;
        }
        let Some((a, b)) = destination(text, start, end) else { continue };
        let raw = &text[a..b];
        let Some((path, suffix)) = split_destination(raw) else { continue };
        if path.is_empty() {
            continue;
        }
        let decoded = pane::percent_decode(path);
        let was = tidy(&pane::resolve(Some(&before), &decoded));
        let now = mv.map(&was);
        if tidy(&pane::resolve(Some(file), &decoded)) == now {
            continue;
        }
        let mut written = if Path::new(&decoded).is_absolute() { now.to_string_lossy().into_owned() } else { relative(dir, &now) };
        if path.ends_with('/') {
            written.push('/');
        }
        let with = if raw.starts_with('<') { format!("<{written}{suffix}>") } else { format!("{}{suffix}", encode(&written)) };
        edits.push((a, b, with));
    }
    splice(text, edits)
}

/// A markdown file's headings: level, title and anchor, the anchor numbered
/// as the link checker numbers repeats.
pub(super) fn headings(text: &str) -> Vec<(u8, String, String)> {
    use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
    let mut out = Vec::new();
    let mut counts = std::collections::HashMap::new();
    let mut current: Option<(u8, String)> = None;
    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                current = Some((n, String::new()));
            }
            Event::Text(s) | Event::Code(s) => {
                if let Some((_, title)) = &mut current {
                    title.push_str(&s);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, title)) = current.take() {
                    let base = super::links::slug(&title);
                    let n = counts.entry(base.clone()).or_insert(0);
                    let anchor = if *n == 0 { base } else { format!("{base}-{n}") };
                    *n += 1;
                    out.push((level, title, anchor));
                }
            }
            _ => {}
        }
    }
    out
}

/// Headings renamed between two versions of a file: their old and new
/// titles and anchors. Only edits that keep the same headings in the same
/// order and levels are read as renames.
pub(super) fn renamed_headings(before: &str, after: &str) -> Vec<(String, String, String, String)> {
    let (a, b) = (headings(before), headings(after));
    if a.len() != b.len() || a.iter().zip(&b).any(|(x, y)| x.0 != y.0) {
        return Vec::new();
    }
    a.into_iter()
        .zip(b)
        .filter(|(x, y)| x.2 != y.2)
        .map(|(x, y)| (x.1, x.2, y.1, y.2))
        .collect()
}

/// A markdown file's links to the renamed headings of `target`, rewritten.
pub(super) fn after_heading_renames(
    root: &Path,
    file: &Path,
    text: &str,
    target: &Path,
    renames: &[(String, String, String, String)],
    notes: &[PathBuf],
) -> String {
    let buf = editor::Buffer::new(text.into());
    let md = crate::md::parse(text, &buf.starts);
    let mut edits = Vec::new();
    for link in &md.links {
        let (start, end) = (link.start as usize, link.end as usize);
        if text[start..].starts_with("[[") && end >= start + 4 {
            let inner = &text[start + 2..end - 2];
            let (name, fragment, label) = wiki_parts(inner);
            let Some(anchor) = fragment.strip_prefix('#') else { continue };
            let leads = if name.is_empty() {
                file == target
            } else {
                super::notes::resolve_note(root, Some(file), name, notes).ok().as_deref() == Some(target)
            };
            let new = renames.iter().find_map(|(old_title, old_anchor, new_title, new_anchor)| {
                if anchor == old_anchor { Some(new_anchor) } else if anchor == old_title { Some(new_title) } else { None }
            });
            if let (true, Some(new)) = (leads, new) {
                edits.push((start + 2, end - 2, format!("{name}#{new}{label}")));
            }
            continue;
        }
        let Some((a, b)) = destination(text, start, end) else { continue };
        let raw = &text[a..b];
        let dest = raw.trim_start_matches('<').trim_end_matches('>');
        let Some(hash) = dest.find('#') else { continue };
        let (path, anchor) = (&dest[..hash], pane::percent_decode(&dest[hash + 1..]));
        let path_only = path.split('?').next().unwrap_or(path);
        let leads = if path_only.is_empty() {
            file == target
        } else if path_only.contains(':') || path_only.starts_with("//") {
            false
        } else {
            tidy(&pane::resolve(Some(file), &pane::percent_decode(path_only))) == target
        };
        let Some((_, _, _, new_anchor)) = renames.iter().find(|r| r.1 == anchor) else { continue };
        if leads {
            let with = format!("{path}#{new_anchor}");
            edits.push((a, b, if raw.starts_with('<') { format!("<{with}>") } else { with }));
        }
    }
    splice(text, edits)
}

impl App {
    /// The project's markdown files and their text, within the usual limits.
    fn markdown_files(&self) -> Vec<(PathBuf, String)> {
        let mut left = MOST;
        let mut out = Vec::new();
        for path in super::search::files(&self.root).into_iter().filter(|p| is_note(p)) {
            let Some(size) = std::fs::metadata(&path).ok().map(|m| m.len() as usize).filter(|n| *n <= 2 << 20 && *n <= left) else { continue };
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            left -= size;
            out.push((path, text));
        }
        out
    }

    /// After a file or folder moves, offers to update the links to it and
    /// those of moved notes, in the replacement review.
    pub(super) fn offer_link_updates(&mut self, old: &Path, new: &Path) {
        let mv = Move { old: tidy(old), new: tidy(new) };
        let files = self.markdown_files();
        let notes: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        let plans: Vec<_> = files
            .into_iter()
            .filter_map(|(path, text)| {
                let after = after_move(&self.root, &path, &text, &mv, &notes);
                (after != text).then_some((path, text, after))
            })
            .collect();
        if plans.is_empty() {
            return;
        }
        let said = self.message.take().unwrap_or_default();
        let n = plans.len();
        self.search.replacements = plans;
        self.review_replacements("Update links to the moved file");
        self.message = Some(format!("{said}. Links in {n} file{} still point to the old location", if n == 1 { "" } else { "s" }));
    }

    /// After saving a markdown file whose headings were renamed, offers to
    /// update the links to them.
    pub(super) fn offer_heading_link_updates(&mut self, path: &Path, before: &str, after: &str) {
        let renames = renamed_headings(before, after);
        if renames.is_empty() {
            return;
        }
        let target = tidy(path);
        let files = self.markdown_files();
        let notes: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        self.heading_link_updates = files
            .into_iter()
            .filter_map(|(file, text)| {
                let updated = after_heading_renames(&self.root, &tidy(&file), &text, &target, &renames, &notes);
                (updated != text).then_some((file, text, updated))
            })
            .collect();
        if !self.heading_link_updates.is_empty() {
            let n = self.heading_link_updates.len();
            self.message = Some(format!(
                "Saved. Links in {n} file{} use a renamed heading: Markdown: Update links to renamed headings",
                if n == 1 { "" } else { "s" }
            ));
        }
    }

    pub(super) fn review_heading_link_updates(&mut self) {
        if self.heading_link_updates.is_empty() {
            self.message = Some("No links to update. Rename a heading and save first".into());
            return;
        }
        self.search.replacements = std::mem::take(&mut self.heading_link_updates);
        self.review_replacements("Update links to renamed headings");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_a_note_and_then_a_heading_offers_reviewed_link_updates() {
        let root = std::env::temp_dir().join(format!("ld-link-updates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("plan.md"), "# Plan\n\n## Goals\n\nSee [goals](#goals).\n").unwrap();
        std::fs::write(root.join("index.md"), "[p](plan.md#goals) [[plan]]\n").unwrap();
        let mut a = App::new(Some(root.join("plan.md")));
        a.resize(1000, 700, 1.0);
        a.rename("archive/plan.md");
        assert!(root.join("archive/plan.md").is_file());
        assert_eq!(a.search.replacements.len(), 1, "{:?}", a.message);
        assert!(a.menu.is_some() && a.message.as_deref().unwrap().contains("Links in 1 file"));
        a.pick_project_replace("apply");
        assert_eq!(a.doc.buf.text, "[p](archive/plan.md#goals) [[plan]]\n");
        assert!(a.doc.buf.modified, "updates wait to be saved");
        a.save();
        // Renaming a heading and saving offers its links too.
        a.open_tab(root.join("archive/plan.md"));
        let at = a.doc.buf.text.find("Goals").unwrap();
        a.doc.edit(at, at + 5, "Aims", crate::editor::EditKind::Other);
        a.edited();
        a.save();
        assert!(a.message.as_deref().unwrap().contains("renamed heading"), "{:?}", a.message);
        a.review_heading_link_updates();
        assert_eq!(a.search.replacements.len(), 2);
        a.pick_project_replace("apply");
        let texts: Vec<String> = a.file_tabs.iter().enumerate().map(|(i, t)| {
            if i == a.active_file { a.doc.buf.text.clone() } else { t.state.as_ref().map_or(String::new(), |s| s.pane.buf.text.clone()) }
        }).collect();
        assert!(texts.iter().any(|t| t == "[p](archive/plan.md#aims) [[plan]]\n"), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("See [goals](#aims).")), "{texts:?}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paths_are_written_relative_and_encoded() {
        assert_eq!(relative(Path::new("/p/docs"), Path::new("/p/img/a b.png")), "../img/a b.png");
        assert_eq!(relative(Path::new("/p"), Path::new("/p/x.md")), "x.md");
        assert_eq!(encode("../img/a b(1).png"), "../img/a%20b%281%29.png");
        let text = "see [x](  <a b.md#t> \"title\") and ![i](img/p.png)";
        let (a, b) = destination(text, 4, text.find(')').unwrap() + 1).unwrap();
        assert_eq!(&text[a..b], "<a b.md#t>");
        assert_eq!(split_destination("img/p.png?v=1#x"), Some(("img/p.png", "?v=1#x")));
        assert_eq!(split_destination("https://a.example/x"), None);
    }

    #[test]
    fn moving_a_file_rewrites_links_to_it_and_from_it() {
        let root = Path::new("/p");
        let mv = Move { old: "/p/notes/plan.md".into(), new: "/p/archive/2026/plan.md".into() };
        let notes: Vec<PathBuf> = vec!["/p/index.md".into(), "/p/archive/2026/plan.md".into()];
        let index = "[Plan](notes/plan.md#goals), [[plan]], [[notes/plan|the plan]], [web](https://x.example/notes/plan.md), [other](notes/other.md)\n";
        let out = after_move(root, Path::new("/p/index.md"), index, &mv, &notes);
        // The bare name still finds the note; the path-like one is written anew.
        assert_eq!(out, "[Plan](archive/2026/plan.md#goals), [[plan]], [[archive/2026/plan|the plan]], [web](https://x.example/notes/plan.md), [other](notes/other.md)\n");
        // The moved note's own links still lead where they did.
        let plan = "![chart](../img/chart%20one.png) [up](../index.md) [self](#goals)\n";
        let out = after_move(root, Path::new("/p/archive/2026/plan.md"), plan, &mv, &notes);
        assert_eq!(out, "![chart](../../img/chart%20one.png) [up](../../index.md) [self](#goals)\n");
    }

    #[test]
    fn moving_a_folder_maps_everything_inside_it() {
        let root = Path::new("/p");
        let mv = Move { old: "/p/img".into(), new: "/p/assets/img".into() };
        let out = after_move(root, Path::new("/p/doc.md"), "![a](img/a.png) ![b](./img/sub/b.png)\n", &mv, &[]);
        assert_eq!(out, "![a](assets/img/a.png) ![b](assets/img/sub/b.png)\n");
    }

    #[test]
    fn renamed_headings_are_found_and_their_links_follow() {
        let before = "# Plan\n\n## Goals\n\nSee [goals](#goals).\n\n## Risks\n";
        let after = "# Plan\n\n## Aims for 2026\n\nSee [goals](#goals).\n\n## Risks\n";
        let renames = renamed_headings(before, after);
        assert_eq!(renames, vec![("Goals".into(), "goals".into(), "Aims for 2026".into(), "aims-for-2026".into())]);
        assert!(renamed_headings(before, "# Plan\n\n## Risks\n").is_empty(), "a removed heading is not a rename");
        let root = Path::new("/p");
        let target = Path::new("/p/plan.md");
        let notes: Vec<PathBuf> = Vec::new();
        assert_eq!(after_heading_renames(root, target, after, target, &renames, &notes), after.replace("(#goals)", "(#aims-for-2026)"));
        let other = "[g](plan.md#goals) [h](other.md#goals) [[plan#Goals|g]] [[#goals]]\n";
        let out = after_heading_renames(root, Path::new("/p/notes.md"), other, target, &renames, &notes);
        assert!(out.starts_with("[g](plan.md#aims-for-2026) [h](other.md#goals) "), "{out}");
        // A wikilink to an unresolvable note is left alone; one in the same file follows.
        assert!(out.ends_with("[[#goals]]\n"));
    }
}
