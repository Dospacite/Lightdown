//! Optional, user-installed native grammars. Loading and parsing run on one
//! worker thread, started on the first request for an installed grammar.
//! Nothing here runs before the first frame, and the lexical highlighter,
//! folds and outline stay in use until a tree is ready.
use crate::{
    kernel::Waker,
    md::{self, Span},
};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator, Tree};

mod layers;
mod query_sets;
mod locals;
mod structure;
mod project;
mod objects;
mod highlights;
#[cfg(test)]
mod query_tests;
pub use project::symbols as project_symbols;

/// The largest text parsed; bigger files keep lexical tools.
const MAX: usize = 2 << 20;
/// How much text, in bytes, the result cache keeps, and how many results.
const CACHE_BYTES: usize = 6 << 20;
const CACHE_ENTRIES: usize = 128;
/// Requests waiting for the worker; older ones are dropped first.
const QUEUE: usize = 8;
const PARSE_MICROS: u64 = 500_000;
const QUERY_MICROS: u64 = 300_000;

pub struct Grammar {
    pub name: &'static str,
    /// File extensions and fence languages it highlights.
    pub aliases: &'static [&'static str],
    pub repo: &'static str,
    /// The pinned commit and the release it belongs to.
    pub revision: &'static str,
    pub version: &'static str,
    /// The folder in the repository that holds `src/`.
    dir: &'static str,
    /// Highlight queries, concatenated: a grammar name ("" for this one) and
    /// a path from the root of its repository.
    queries: &'static [(&'static str, &'static str)],
}

const fn g(
    name: &'static str,
    aliases: &'static [&'static str],
    repo: &'static str,
    version: &'static str,
    revision: &'static str,
) -> Grammar {
    Grammar { name, aliases, repo, revision, version, dir: "", queries: &[("", "queries/highlights.scm")] }
}

pub const GRAMMARS: &[Grammar] = &[
    g("bash", &["sh", "bash", "zsh", "shell"], "tree-sitter/tree-sitter-bash", "v0.25.1", "a06c2e4415e9bc0346c6b86d401879ffb44058f7"),
    g("c", &["c", "h"], "tree-sitter/tree-sitter-c", "v0.24.2", "b780e47fc780ddc8da13afa35a3f4ed5c157823d"),
    Grammar {
        queries: &[("c", "queries/highlights.scm"), ("", "queries/highlights.scm")],
        ..g("cpp", &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx"], "tree-sitter/tree-sitter-cpp", "v0.23.4", "f41e1a044c8a84ea9fa8577fdd2eab92ec96de02")
    },
    g("css", &["css"], "tree-sitter/tree-sitter-css", "v0.25.0", "dda5cfc5722c429eaba1c910ca32c2c0c5bb1a3f"),
    g("go", &["go", "golang"], "tree-sitter/tree-sitter-go", "v0.25.0", "1547678a9da59885853f5f5cc8a99cc203fa2e2c"),
    g("haskell", &["hs", "haskell"], "tree-sitter/tree-sitter-haskell", "v0.24.1", "97288e585b0bd44199720280d067fa35bacd81ff"),
    g("html", &["html", "htm"], "tree-sitter/tree-sitter-html", "v0.23.2", "5a5ca8551a179998360b4a4ca2c0f366a35acc03"),
    g("java", &["java"], "tree-sitter/tree-sitter-java", "v0.23.5", "94703d5a6bed02b98e438d7cad1136c01a60ba2c"),
    Grammar {
        queries: &[("", "queries/highlights.scm"), ("", "queries/highlights-jsx.scm")],
        ..g("javascript", &["js", "mjs", "cjs", "jsx", "javascript"], "tree-sitter/tree-sitter-javascript", "v0.25.0", "44c892e0be055ac465d5eeddae6d3e194424e7de")
    },
    g("json", &["json"], "tree-sitter/tree-sitter-json", "v0.24.8", "ee35a6ebefcef0c5c416c0d1ccec7370cfca5a24"),
    g("kotlin", &["kt", "kts", "kotlin"], "fwcd/tree-sitter-kotlin", "0.3.8", "e1a2d5ad1f61f5740677183cd4125bb071cd2f30"),
    g("lua", &["lua"], "tree-sitter-grammars/tree-sitter-lua", "v0.5.0", "10fe0054734eec83049514ea2e718b2a56acd0c9"),
    Grammar {
        dir: "tree-sitter-markdown",
        queries: &[("", "tree-sitter-markdown/queries/highlights.scm")],
        ..g("markdown", &["markdown", "md", "mdown", "mkd", "mkdn", "mdwn", "mdx"], "tree-sitter-grammars/tree-sitter-markdown", "v0.5.4", "41ff24a6c775492c543b09228aeb98d1cd2b3a0f")
    },
    Grammar {
        dir: "php",
        ..g("php", &["php"], "tree-sitter/tree-sitter-php", "v0.25.1", "58d5643086ae60e93a3746cca648363ee97c8761")
    },
    g("python", &["py", "pyi", "pyw", "python", "python3"], "tree-sitter/tree-sitter-python", "v0.25.0", "293fdc02038ee2bf0e2e206711b69c90ac0d413f"),
    g("ruby", &["rb", "rake", "gemspec", "ruby"], "tree-sitter/tree-sitter-ruby", "v0.23.1", "71bd32fb7607035768799732addba884a37a6210"),
    g("rust", &["rs", "rust"], "tree-sitter/tree-sitter-rust", "v0.24.2", "77a3747266f4d621d0757825e6b11edcbf991ca5"),
    // The generated parser lives on the gh-pages branch.
    g("sql", &["sql"], "DerekStride/tree-sitter-sql", "v0.3.11", "174cebb880f10bc7c5a24aa289431949d26d3c09"),
    // The generated parser lives on the with-generated-files branch.
    g("swift", &["swift"], "alex-pinkus/tree-sitter-swift", "0.7.4", "82bb3a533e0801fd2bbaa11dc49676e10bf41948"),
    g("toml", &["toml"], "tree-sitter-grammars/tree-sitter-toml", "v0.7.0", "64b56832c2cffe41758f28e05c756a3a98d16f41"),
    Grammar {
        dir: "tsx",
        queries: &[
            ("javascript", "queries/highlights.scm"),
            ("javascript", "queries/highlights-jsx.scm"),
            ("", "queries/highlights.scm"),
        ],
        ..g("tsx", &["tsx"], "tree-sitter/tree-sitter-typescript", "v0.23.2", "f975a621f4e7f532fe322e13c4f79495e0a7b2e7")
    },
    Grammar {
        dir: "typescript",
        queries: &[("javascript", "queries/highlights.scm"), ("", "queries/highlights.scm")],
        ..g("typescript", &["ts", "mts", "cts", "typescript"], "tree-sitter/tree-sitter-typescript", "v0.23.2", "f975a621f4e7f532fe322e13c4f79495e0a7b2e7")
    },
    g("yaml", &["yaml", "yml"], "tree-sitter-grammars/tree-sitter-yaml", "v0.7.2", "7708026449bed86239b1cd5bce6e3c34dbca6415"),
    g("zig", &["zig"], "tree-sitter-grammars/tree-sitter-zig", "v1.1.2", "b670c8df85a1568f498aa5c8cae42f51a90473c0"),
];

pub fn grammar(name: &str) -> Option<&'static Grammar> {
    GRAMMARS.iter().find(|g| g.name == name)
}
fn for_language(language: &str) -> Option<&'static Grammar> {
    GRAMMARS.iter().find(|g| g.aliases.contains(&language))
}

pub fn directory() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
        .join("lightdown/grammars")
}

/// What an installed grammar records about itself.
#[derive(Debug, PartialEq)]
pub struct Info {
    pub version: String,
    pub revision: String,
    pub abi: usize,
}
impl Info {
    fn parse(text: &str) -> Option<Info> {
        let field = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(str::trim);
        Some(Info { version: field("version")?.into(), revision: field("revision")?.into(), abi: field("abi")?.parse().ok()? })
    }
    fn render(&self) -> String {
        format!("version={}\nrevision={}\nabi={}\n", self.version, self.revision, self.abi)
    }
}
/// An installed grammar's record, or None when it is not installed.
pub fn info(g: &Grammar) -> Option<Info> {
    let dir = directory().join(g.name);
    if !dir.join("grammar.so").is_file() {
        return None;
    }
    Some(std::fs::read_to_string(dir.join("info")).ok().and_then(|t| Info::parse(&t)).unwrap_or(Info {
        version: "unknown".into(),
        revision: String::new(),
        abi: 0,
    }))
}
fn installed(g: &Grammar) -> bool {
    directory().join(g.name).join("grammar.so").is_file()
}
/// Whether this build's runtime reads grammars of an ABI version.
pub fn abi_supported(abi: usize) -> bool {
    (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION).contains(&abi)
}
fn abi_error(abi: usize) -> String {
    format!(
        "grammar ABI {abi} is not supported (this build reads {} to {}); reinstall the grammar",
        tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
        tree_sitter::LANGUAGE_VERSION
    )
}

/// A parse result and the tree it came from.
pub struct Parsed {
    tree: Tree,
    layers: Vec<layers::Layer>,
    spans: Vec<Span>,
    folds: Vec<(usize, usize)>,
    outline: Vec<crate::outline::Item>,
    objects: Vec<objects::Object>,
    // Dropped last: the tree points into the library.
    _library: Arc<libloading::Library>,
}

struct Entry {
    grammar: &'static str,
    text: Arc<str>,
    result: Option<Arc<Parsed>>,
}
struct Request {
    grammar: &'static Grammar,
    text: Arc<str>,
}
#[derive(Default)]
struct State {
    cache: VecDeque<Entry>,
    queue: Vec<Request>,
    running: Option<(&'static str, Arc<str>)>,
    worker: bool,
    /// Bumped when grammars are installed or removed, so the worker reloads them.
    generation: u64,
    installing: Vec<&'static str>,
    wake: Option<Waker>,
    message: Option<String>,
}
static STATE: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();
static READY: AtomicBool = AtomicBool::new(false);
fn shared() -> &'static (Mutex<State>, Condvar) {
    STATE.get_or_init(Default::default)
}
fn state() -> std::sync::MutexGuard<'static, State> {
    shared().0.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn set_waker(wake: Waker) {
    state().wake = Some(wake);
}
/// Whether a result has arrived since the last call.
pub fn completed() -> bool {
    READY.swap(false, Ordering::Relaxed)
}
pub fn message() -> Option<String> {
    state().message.take()
}
#[cfg(feature = "dump")]
pub fn settle() {
    let start = Instant::now();
    while {
        let s = state();
        (!s.queue.is_empty() || s.running.is_some()) && start.elapsed() < Duration::from_secs(5)
    } {
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn notify(s: &State) {
    READY.store(true, Ordering::Relaxed);
    if let Some(w) = &s.wake {
        w();
    }
}

/// The single edit that turns `old` into `new`: their shared prefix and suffix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Delta {
    start: usize,
    old_end: usize,
    new_end: usize,
    start_row: usize,
    old_end_row: usize,
    new_end_row: usize,
    /// The edit ends at the start of a line, which then only moves.
    ends_at_line_start: bool,
}
fn rows(text: &str, at: usize) -> (usize, usize) {
    let prefix = &text.as_bytes()[..at];
    let row = prefix.iter().filter(|b| **b == b'\n').count();
    (row, prefix.iter().rposition(|b| *b == b'\n').map_or(at, |n| at - n - 1))
}
impl Delta {
    fn between(old: &str, new: &str) -> Delta {
        let mut start = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
            start -= 1;
        }
        let limit = old.len().min(new.len()) - start;
        let mut suffix = old.bytes().rev().zip(new.bytes().rev()).take(limit).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
            suffix -= 1;
        }
        let row = |t: &str, at: usize| t.as_bytes()[..at].iter().filter(|b| **b == b'\n').count();
        let start_row = row(old, start);
        let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
        Delta {
            start,
            old_end,
            new_end,
            start_row,
            old_end_row: start_row + row(&old[start..], old_end - start),
            new_end_row: start_row + row(&new[start..], new_end - start),
            ends_at_line_start: old_end == 0 || old.as_bytes()[old_end - 1] == b'\n',
        }
    }
    /// Whether the texts share most of their bytes: one is an edit of the other.
    fn related(&self, old: &str, new: &str) -> bool {
        let shared = self.start + old.len() - self.old_end;
        old.len().max(new.len()) > 0 && shared * 2 >= old.len().max(new.len())
    }
    fn input(&self, old: &str, new: &str) -> tree_sitter::InputEdit {
        let point = |t: &str, at: usize| {
            let (row, column) = rows(t, at);
            tree_sitter::Point { row, column }
        };
        tree_sitter::InputEdit {
            start_byte: self.start,
            old_end_byte: self.old_end,
            new_end_byte: self.new_end,
            start_position: point(old, self.start),
            old_end_position: point(old, self.old_end),
            new_end_position: point(new, self.new_end),
        }
    }
    /// Where an old offset is now; None inside the replaced text.
    fn byte(&self, at: usize) -> Option<usize> {
        if at <= self.start {
            Some(at)
        } else if at >= self.old_end {
            Some(at - self.old_end + self.new_end)
        } else {
            None
        }
    }
    /// An old span, moved; None when it overlaps the replaced text.
    fn span(&self, s: Span) -> Option<Span> {
        let (a, b) = (s.start as usize, s.end as usize);
        if b <= self.start || a >= self.old_end && a > self.start {
            Some(Span { start: self.byte(a)? as u32, end: self.byte(b)? as u32, attr: s.attr })
        } else {
            None
        }
    }
    /// An old fold of lines, moved; None when an end lies on an edited line.
    fn fold(&self, (a, b): (usize, usize)) -> Option<(usize, usize)> {
        let row = |r: usize| {
            if r < self.start_row {
                Some(r)
            } else if r > self.old_end_row || r == self.old_end_row && self.ends_at_line_start {
                Some(r - self.old_end_row + self.new_end_row)
            } else {
                None
            }
        };
        let (a, b) = (row(a)?, row(b)?);
        (b > a).then_some((a, b))
    }
}

enum View {
    /// A result for this exact text.
    Exact(Arc<Parsed>),
    /// This exact text was tried and failed; lexical tools stay.
    Failed,
    /// A result for an earlier version of the text, and the edit since.
    Near(Arc<Parsed>, Delta),
    Missing,
}
fn view(text: &str, grammar: Option<&str>) -> View {
    let mut s = state();
    if let Some(i) = s.cache.iter().rposition(|e| grammar.is_none_or(|g| e.grammar == g) && *e.text == *text) {
        // Recently used results are evicted last.
        let Some(entry) = s.cache.remove(i) else { return View::Missing };
        let result = entry.result.clone();
        s.cache.push_back(entry);
        return result.map_or(View::Failed, View::Exact);
    }
    s.cache
        .iter()
        .rev()
        .filter(|e| grammar.is_none_or(|g| e.grammar == g))
        .find_map(|e| {
            let delta = Delta::between(&e.text, text);
            (delta.related(&e.text, text)).then(|| e.result.clone().map(|r| View::Near(r, delta))).flatten()
        })
        .unwrap_or(View::Missing)
}
fn near(text: &str) -> Option<(Arc<Parsed>, Option<Delta>)> {
    match view(text, None) {
        View::Exact(r) => Some((r, None)),
        View::Near(r, d) => Some((r, Some(d))),
        View::Failed | View::Missing => None,
    }
}

/// Highlight spans from the installed grammar for `language`. None means
/// use the lexical highlighter: no grammar, too large, or not parsed yet.
/// A parse is requested for text without a result.
pub fn spans(text: &str, language: &str) -> Option<Vec<Span>> {
    if text.len() > MAX {
        return None;
    }
    let g = for_language(language)?;
    let found = match view(text, Some(g.name)) {
        View::Exact(r) => return Some(r.spans.clone()),
        View::Failed => return None,
        View::Near(r, d) => Some((r, d)),
        View::Missing => None,
    };
    if !installed(g) {
        return None;
    }
    request(g, text);
    // Until the new tree is ready, the last one's spans move with the edit.
    let (r, delta) = found?;
    Some(r.spans.iter().filter_map(|s| delta.span(*s)).collect())
}
fn request(g: &'static Grammar, text: &str) {
    let mut s = state();
    if s.running.as_ref().is_some_and(|(n, t)| *n == g.name && **t == *text)
        || s.queue.iter().any(|r| r.grammar.name == g.name && *r.text == *text)
    {
        return;
    }
    if s.queue.len() >= QUEUE {
        s.queue.remove(0);
    }
    s.queue.push(Request { grammar: g, text: text.into() });
    if !s.worker {
        s.worker = true;
        std::thread::Builder::new().name("tree-sitter".into()).spawn(work).ok();
    }
    shared().1.notify_one();
}

/// Fold ranges from the tree, in source lines, one per starting line.
pub fn folds(text: &str) -> Option<Vec<(usize, usize)>> {
    let (r, delta) = near(text)?;
    Some(match delta {
        None => r.folds.clone(),
        Some(d) => r.folds.iter().filter_map(|f| d.fold(*f)).collect(),
    })
}
/// Outline entries from the tree, nested by declaration.
/// None until a usable tree is available.
pub fn outline(text: &str) -> Option<Vec<crate::outline::Item>> {
    let (r, delta) = near(text)?;
    Some(match delta {
        None => r.outline.clone(),
        Some(d) => r
            .outline
            .iter()
            .filter_map(|i| Some(crate::outline::Item { at: d.byte(i.at)?, ..i.clone() }))
            .collect(),
    })
}
/// The named nodes enclosing a selection, innermost first. Empty unless
/// the current text has a tree.
pub fn ranges(text: &str, a: usize, b: usize) -> Vec<(usize, usize)> {
    let View::Exact(r) = view(text, None) else { return Vec::new() };
    let mut out = Vec::new();
    objects::ranges(&r,a,b,&mut out);
    out
}

/// Query-defined Vim text objects; counts select successively enclosing objects.
pub fn text_object(text: &str, at: usize, around: bool, kind: char, count: usize) -> Option<(usize,usize)> {
    let View::Exact(r) = view(text,None) else { return None };
    objects::select(&r,at,around,kind,count)
}

/// A grammar loaded on the worker, kept for later parses.
struct Loaded {
    language: tree_sitter::Language,
    query: Query,
    injections: Query,
    locals: Query,
    outline: Query,
    folds: Query,
    objects: Query,
    roles: Vec<crate::md::Attr>,
    library: Arc<libloading::Library>,
}

fn work() {
    let mut loaded: HashMap<&'static str, Result<Arc<Loaded>, String>> = HashMap::new();
    // Grammars whose failure was already reported, once per generation.
    let mut reported: Vec<&'static str> = Vec::new();
    let mut generation = 0;
    loop {
        let (request, previous, current) = {
            let (lock, ready) = shared();
            let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
            while s.queue.is_empty() {
                s = ready.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            // Newest first; older versions of the same text are stale.
            let request = s.queue.pop().unwrap();
            s.queue.retain(|r| {
                r.grammar.name != request.grammar.name || !Delta::between(&r.text, &request.text).related(&r.text, &request.text)
            });
            s.running = Some((request.grammar.name, request.text.clone()));
            let previous: Vec<_> = s
                .cache
                .iter()
                .filter(|e| e.grammar == request.grammar.name)
                .filter_map(|e| Some((e.text.clone(), e.result.clone()?)))
                .collect();
            (request, previous, s.generation)
        };
        if current != generation {
            loaded.clear();
            reported.clear();
            generation = current;
        }
        let g = request.grammar;
        let grammar = loaded.entry(g.name).or_insert_with(|| load(g, &directory().join(g.name)).map(Arc::new)).clone();
        // The cached tree that shares the most text with this one.
        let previous = previous
            .iter()
            .map(|(t, r)| (Delta::between(t, &request.text), t, r))
            .filter(|(d, t, _)| d.related(t, &request.text))
            .max_by_key(|(d, t, _)| d.start + t.len() - d.old_end);
        let result = grammar.and_then(|l| {
            let old = previous.map(|(d, t, r)| (d.input(t, &request.text), &r.tree));
            let deadline = Instant::now() + Duration::from_micros(PARSE_MICROS + QUERY_MICROS);
            let mut result = parse(&l, &request.text, old)?;
            let prior = previous.map(|(d, t, r)| (d.input(t, &request.text), r.as_ref()));
            layers::inject(&l, &request.text, &mut result, prior, &mut loaded, deadline, 0)?;
            Ok(result)
        });
        let mut s = state();
        s.running = None;
        let error = result.as_ref().err().cloned();
        if let Some(e) = &error
            && s.generation == generation
            && !reported.contains(&g.name)
        {
            reported.push(g.name);
            s.message = Some(format!("{} grammar: {e}; using lexical tools", g.name));
        }
        if s.generation == generation {
            insert(&mut s, Entry { grammar: g.name, text: request.text, result: result.ok().map(Arc::new) });
        }
        notify(&s);
    }
}
fn insert(s: &mut State, entry: Entry) {
    // Older versions of this text go first, then the least recently used.
    let new = entry.text.clone();
    while s.cache.len() >= CACHE_ENTRIES || s.cache.iter().map(|e| e.text.len()).sum::<usize>() + new.len() > CACHE_BYTES
    {
        let older = s.cache.iter().position(|e| {
            e.grammar == entry.grammar && e.result.is_some() && Delta::between(&e.text, &new).related(&e.text, &new)
        });
        let removed = match older {
            Some(i) => s.cache.remove(i),
            None => s.cache.pop_front(),
        };
        if removed.is_none() {
            break;
        }
    }
    s.cache.push_back(entry);
}

fn role(name: &str) -> crate::md::Attr {
    let mut parts = name.split('.');
    let head = parts.next().unwrap_or("");
    let next = parts.next().unwrap_or("");
    match (head, next) {
        ("comment", _) => md::DIM,
        ("string" | "character" | "text", "special" | "escape" | "regex" | "regexp") => md::SPECIAL,
        ("string" | "character", _) => md::SOFT,
        ("text" | "markup", "literal" | "raw") => md::SOFT,
        ("text" | "markup", "title" | "heading" | "strong") => md::KEYWORD | md::BOLD,
        ("text" | "markup", "uri" | "link" | "reference") => md::FUNCTION,
        ("keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass" | "boolean", _) => {
            md::KEYWORD | md::BOLD
        }
        ("number" | "float", _) => md::NUMBER,
        ("constant", "numeric") => md::NUMBER,
        ("variable", "parameter") | ("parameter", _) => md::PARAMETER,
        ("property" | "field", _) | ("variable", "member") => md::PROPERTY,
        ("function" | "method", _) => md::FUNCTION,
        ("punctuation", "special") => md::SPECIAL,
        ("variable", "builtin") => md::SPECIAL,
        ("type" | "constant" | "constructor" | "attribute" | "label" | "tag" | "namespace" | "module", _) => {
            md::SPECIAL
        }
        _ => 0,
    }
}

/// Loads an installed grammar and its queries, refusing other ABI versions.
fn load(g: &Grammar, dir: &Path) -> Result<Loaded, String> {
    // The recorded ABI is checked before the library is opened at all.
    if let Some(info) = std::fs::read_to_string(dir.join("info")).ok().and_then(|t| Info::parse(&t))
        && !abi_supported(info.abi)
    {
        return Err(abi_error(info.abi));
    }
    // SAFETY: only grammars the user installed in their data directory are
    // loaded. Every language, query and tree keeps the library alive.
    let library = Arc::new(unsafe { libloading::Library::new(dir.join("grammar.so")) }.map_err(|e| e.to_string())?);
    let symbol = format!("tree_sitter_{}", g.name);
    let language = unsafe {
        let entry: libloading::Symbol<unsafe extern "C" fn() -> *const tree_sitter::ffi::TSLanguage> =
            library.get(symbol.as_bytes()).map_err(|e| e.to_string())?;
        let raw = entry();
        if raw.is_null() {
            return Err("the grammar returned no language".into());
        }
        tree_sitter::Language::from_raw(raw)
    };
    if !abi_supported(language.abi_version()) {
        return Err(abi_error(language.abi_version()));
    }
    let path = dir.join("highlights.scm");
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1 << 20 {
        return Err("highlight query too large".into());
    }
    let source = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let query = Query::new(&language, &source).map_err(|e| format!("highlight query: {e}"))?;
    let roles = query.capture_names().iter().map(|n| role(n)).collect();
    let injections = layers::load_query(g, dir, &language, "injections")?;
    let locals = layers::load_query(g, dir, &language, "locals")?;
    let outline = Query::new(&language, query_sets::bundled(g.name,"outline")).map_err(|e| format!("outline query: {e}"))?;
    let folds = Query::new(&language, query_sets::bundled(g.name,"folds")).map_err(|e| format!("fold query: {e}"))?;
    let objects = Query::new(&language, query_sets::bundled(g.name,"textobjects")).map_err(|e| format!("text object query: {e}"))?;
    Ok(Loaded { language, query, injections, locals, outline, folds, objects, roles, library })
}

#[allow(deprecated)]
fn parse(l: &Loaded, text: &str, old: Option<(tree_sitter::InputEdit, &Tree)>) -> Result<Parsed, String> {
    let mut parser = Parser::new();
    parser.set_language(&l.language).map_err(|e| e.to_string())?;
    parser.set_timeout_micros(PARSE_MICROS);
    let old = old.filter(|(_, t)| *t.language() == l.language).map(|(edit, tree)| {
        let mut tree = tree.clone();
        tree.edit(&edit);
        tree
    });
    let tree = parser.parse(text, old.as_ref()).ok_or("parse timed out")?;
    drop(old);
    analyse(l, tree, text, Instant::now() + Duration::from_micros(QUERY_MICROS))
}

#[allow(deprecated)]
fn analyse(l: &Loaded, tree: Tree, text: &str, deadline: Instant) -> Result<Parsed, String> {
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(4096);
    cursor.set_timeout_micros(deadline.saturating_duration_since(Instant::now()).as_micros().max(1) as u64);
    let started = Instant::now();
    let locals = locals::resolve(&l.locals, &tree, text, deadline)?;
    let mut captures = cursor.captures(&l.query, tree.root_node(), text.as_bytes());
    let mut spans = Vec::new();
    while let Some((m, i)) = captures.next() {
        let c = m.captures[*i];
        let local = locals.get(&(c.node.start_byte(), c.node.end_byte()));
        if l.query.property_predicates(m.pattern_index).iter().any(|(p, positive)| p.key.as_ref() == "local" && *positive != local.is_some()) { continue; }
        let attr = local.copied().filter(|a| *a != 0).unwrap_or(l.roles[c.index as usize]);
        spans.push((Span { start: c.node.start_byte() as u32, end: c.node.end_byte() as u32, attr }, m.pattern_index));
        if spans.len() >= 200_000 {
            return Err("too many highlights".into());
        }
    }
    drop(captures);
    if cursor.did_exceed_match_limit() || started.elapsed() > Duration::from_micros(QUERY_MICROS) {
        return Err("highlight query exceeded its budget".into());
    }
    for ((a,b),attr) in locals {
        if attr != 0 { spans.push((Span { start:a as u32, end:b as u32,attr }, usize::MAX)); }
    }
    // Specific captures and local bindings replace surrounding string colours.
    let spans = highlights::flatten(spans, deadline)?;
    let (folds, outline) = structure::extract(l, &tree, text, deadline)?;
    let objects = objects::extract(&l.objects,&tree,text,deadline)?;
    Ok(Parsed { tree, layers: Vec::new(), spans, folds, outline, objects, _library: l.library.clone() })
}

/// The grammars known to this build, with what is installed.
pub fn list() -> Vec<(&'static Grammar, Option<Info>)> {
    GRAMMARS.iter().map(|g| (g, info(g))).collect()
}
pub fn installing(name: &str) -> bool {
    state().installing.iter().any(|n| *n == name)
}
/// Downloads and compiles a grammar on a worker. Returns false when it is
/// unknown or already being installed.
pub fn install(name: &str) -> bool {
    let Some(g) = grammar(name) else { return false };
    {
        let mut s = state();
        if s.installing.contains(&g.name) {
            return false;
        }
        s.installing.push(g.name);
    }
    std::thread::spawn(move || {
        let result = install_sync(g);
        let mut s = state();
        s.installing.retain(|n| *n != g.name);
        s.message = Some(match result {
            Ok(info) => format!("Installed {} grammar {} (ABI {})", g.name, info.version, info.abi),
            Err(e) => format!("Grammar install failed for {}: {e}", g.name),
        });
        forget(&mut s);
        notify(&s);
    });
    true
}
/// Deletes an installed grammar. Trees already parsed keep their library.
pub fn remove(name: &str) -> Result<(), String> {
    let g = grammar(name).ok_or("unknown grammar")?;
    if installing(g.name) {
        return Err(format!("{} grammar is being installed", g.name));
    }
    let dir = directory().join(g.name);
    if !dir.exists() {
        return Err(format!("{} grammar is not installed", g.name));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut s = state();
    forget(&mut s);
    notify(&s);
    Ok(())
}
fn forget(s: &mut State) {
    s.cache.clear();
    s.queue.clear();
    s.generation += 1;
}

fn run(command: &mut std::process::Command, limit: Duration) -> Result<(), String> {
    use std::process::Stdio;
    let name = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() { Ok(()) } else { Err(format!("{name} exited {status}")) };
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{name} timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn fetch(url: &str, to: &Path, max: usize) -> Result<(), String> {
    run(
        std::process::Command::new("curl")
            .args(["--fail", "--silent", "--location", "--max-time", "120", "--max-filesize"])
            .arg(max.to_string())
            .arg("--output")
            .arg(to)
            .arg(url),
        Duration::from_secs(150),
    )
}
fn install_sync(g: &'static Grammar) -> Result<Info, String> {
    use std::{fs, process::Command};
    let root = directory();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let tmp = root.join(format!(".{}-{}", g.name, std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        fetch(&format!("https://codeload.github.com/{}/tar.gz/{}", g.repo, g.revision), &tmp.join("source.tar.gz"), 64 << 20)?;
        run(
            Command::new("tar").arg("xzf").arg(tmp.join("source.tar.gz")).args(["--strip-components=1", "-C"]).arg(&tmp),
            Duration::from_secs(60),
        )?;
        let src = tmp.join(g.dir).join("src");
        if !src.join("parser.c").is_file() {
            return Err("the pinned revision has no generated parser".into());
        }
        let mut cc = Command::new("cc");
        cc.current_dir(&src).args(["-shared", "-fPIC", "-O2", "-std=c11", "-I.", "parser.c"]);
        if src.join("scanner.c").exists() {
            cc.arg("scanner.c");
        }
        run(cc.arg("-o").arg(tmp.join("grammar.so")), Duration::from_secs(600))?;
        let mut query = String::new();
        for (i, (from, path)) in g.queries.iter().enumerate() {
            let owner = if from.is_empty() { g } else { grammar(from).ok_or("unknown query source")? };
            let file = tmp.join(format!("query{i}.scm"));
            if owner.name == g.name && tmp.join(path).is_file() {
                fs::copy(tmp.join(path), &file).map_err(|e| e.to_string())?;
            } else {
                fetch(&format!("https://raw.githubusercontent.com/{}/{}/{path}", owner.repo, owner.revision), &file, 1 << 20)?;
            }
            query.push_str(&fs::read_to_string(&file).map_err(|e| e.to_string())?);
            query.push('\n');
        }
        fs::write(tmp.join("highlights.scm"), &query).map_err(|e| e.to_string())?;
        layers::install_queries(g, &tmp)?;
        // Load it once here, so a broken grammar or query is never published.
        let loaded = load(g, &tmp)?;
        parse(&loaded, "", None)?;
        let info = Info { version: g.version.into(), revision: g.revision.into(), abi: loaded.language.abi_version() };
        drop(loaded);
        fs::write(tmp.join("info"), info.render()).map_err(|e| e.to_string())?;
        // Each file is renamed into place, the library last, so readers never see a partial one.
        let target = root.join(g.name);
        fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        for file in ["highlights.scm", "injections.scm", "locals.scm", "info", "grammar.so"] {
            fs::rename(tmp.join(file), target.join(file)).map_err(|e| e.to_string())?;
        }
        Ok(info)
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_use_existing_roles() {
        assert_eq!(role("function.method"), md::FUNCTION);
        assert_eq!(role("string.escape"), md::SPECIAL);
        assert_eq!(role("string"), md::SOFT);
        assert_eq!(role("keyword.control.repeat"), md::KEYWORD | md::BOLD);
        assert_eq!(role("conditional"), md::KEYWORD | md::BOLD);
        assert_eq!(role("constant.numeric"), md::NUMBER);
        assert_eq!(role("constant.builtin"), md::SPECIAL);
        assert_eq!(role("text.title"), md::KEYWORD | md::BOLD);
        assert_eq!(role("variable.parameter"), md::PARAMETER);
        assert_eq!(role("property"), md::PROPERTY);
        assert_eq!(role("punctuation.special"), md::SPECIAL);
        assert_eq!(role("punctuation.bracket"), 0);
    }

    #[test]
    fn the_table_is_pinned_and_unambiguous() {
        let mut aliases: Vec<&str> = Vec::new();
        for g in GRAMMARS {
            assert_eq!(g.revision.len(), 40, "{}", g.name);
            assert!(g.revision.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(!g.version.is_empty());
            for (from, _) in g.queries {
                assert!(from.is_empty() || grammar(from).is_some());
            }
            aliases.extend(g.aliases);
        }
        let count = aliases.len();
        aliases.sort();
        aliases.dedup();
        assert_eq!(aliases.len(), count, "an alias names two grammars");
        for name in ["rs", "py", "js", "tsx", "ts", "c", "cpp", "go", "java", "sh", "toml", "yaml", "css", "sql", "rb", "php", "kt", "swift", "zig", "hs"] {
            assert!(for_language(name).is_some(), "{name}");
        }
    }

    #[test]
    fn unknown_languages_fall_back() {
        assert!(spans("hello", "not-a-language").is_none());
    }

    #[test]
    fn info_round_trips_and_abi_is_checked() {
        let info = Info { version: "v1.2.3".into(), revision: "abc".into(), abi: 14 };
        assert_eq!(Info::parse(&info.render()), Some(info));
        assert!(Info::parse("version=x\n").is_none());
        assert!(abi_supported(tree_sitter::LANGUAGE_VERSION));
        assert!(!abi_supported(tree_sitter::LANGUAGE_VERSION + 1));
        assert!(!abi_supported(tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION - 1));
    }

    #[test]
    fn a_library_with_another_abi_is_refused_before_loading() {
        let dir = std::env::temp_dir().join(format!("lightdown-abi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The library is not even a library: loading it would fail differently.
        std::fs::write(dir.join("grammar.so"), b"not a library").unwrap();
        std::fs::write(dir.join("info"), Info { version: "v0".into(), revision: "x".into(), abi: 99 }.render()).unwrap();
        let error = load(grammar("rust").unwrap(), &dir).err().unwrap();
        assert!(error.contains("ABI 99"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deltas_find_the_edit_and_move_results_across_it() {
        let old = "fn a() {\n    one();\n}\nfn b() {}\n";
        let new = "fn a() {\n    one();\n    two();\n}\nfn b() {}\n";
        let d = Delta::between(old, new);
        assert!(d.related(old, new));
        assert_eq!(&new[d.start..d.new_end], "    two();\n");
        assert_eq!((d.start_row, d.old_end_row, d.new_end_row), (2, 2, 3));
        // The outer function's fold grows, the later one moves down.
        assert_eq!(d.fold((0, 2)), Some((0, 3)));
        let b = old.find("fn b").unwrap();
        assert_eq!(d.byte(b), Some(new.find("fn b").unwrap()));
        let span = Span { start: b as u32, end: b as u32 + 2, attr: md::KEYWORD };
        assert_eq!(d.span(span).map(|s| &new[s.start as usize..s.end as usize]), Some("fn"));
        // A span the edit runs through is dropped.
        let d = Delta::between("let abc = 1;", "let aXc = 1;");
        assert!(d.span(Span { start: 4, end: 7, attr: 0 }).is_none());
        assert!(!Delta::between("first text entirely", "something else").related("first text entirely", "something else"));
        // Multibyte characters are never split.
        let d = Delta::between("é", "è");
        assert_eq!((d.start, d.old_end, d.new_end), (0, 2, 2));
    }

    /// Installs grammars into a scratch data folder. Run with
    /// `cargo test -- --ignored grammars_build` and network access.
    pub(super) fn scratch() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| {
            let base = std::env::temp_dir().join("ld-ts-validation");
            // SAFETY: integration tests run alone with --test-threads=1, before workers.
            for (key, child) in [("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache")] {
                unsafe { std::env::set_var(key, base.join(child)) };
            }
        });
    }
    pub(super) fn ready(name: &str) -> &'static Grammar {
        scratch();
        let g = grammar(name).unwrap();
        if !installed(g) {
            install_sync(g).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        g
    }
    fn parse_now(name: &str, text: &str) -> Parsed {
        let g = ready(name);
        parse(&load(g, &directory().join(g.name)).unwrap(), text, None).unwrap()
    }

    #[test]
    #[ignore = "Downloads every pinned grammar and compiles it with cc"]
    fn grammars_build_and_load() {
        for g in GRAMMARS {
            let g = ready(g.name);
            let info = info(g).unwrap();
            assert_eq!(info.version, g.version);
            assert!(abi_supported(info.abi));
        }
    }

    #[test]
    #[ignore = "Downloads the Rust grammar and compiles it with cc"]
    fn grammars_build_rust_structure() {
        let text = "mod m {\n    struct S {\n        a: u8,\n    }\n    impl Show for S {\n        fn show(&self) {\n            call(1);\n        }\n    }\n}\n";
        let p = parse_now("rust", text);
        let titles: Vec<_> = p.outline.iter().map(|i| (i.depth, i.title.as_str())).collect();
        assert_eq!(titles, [(0, "m"), (1, "S"), (1, "impl Show for S"), (2, "show()")]);
        assert!(p.folds.contains(&(0, 9)) && p.folds.contains(&(4, 8)) && p.folds.contains(&(5, 7)));
        assert!(p.spans.iter().any(|s| s.attr & md::KEYWORD != 0 && &text[s.start as usize..s.end as usize] == "fn"));
        let at = text.find("1)").unwrap();
        let mut node = p.tree.root_node().named_descendant_for_byte_range(at, at);
        let mut kinds = Vec::new();
        while let Some(n) = node {
            kinds.push(n.kind());
            node = n.parent();
        }
        assert_eq!(&kinds[..3], ["integer_literal", "arguments", "call_expression"]);
    }

    #[test]
    #[ignore = "Downloads the Python grammar and compiles it with cc"]
    fn grammars_build_incremental_reuse() {
        let g = ready("python");
        let l = load(g, &directory().join(g.name)).unwrap();
        let old = "class A:\n    def f(self):\n        return 1\n";
        let new = "class A:\n    def f(self):\n        return 12\n\n    def g(self):\n        pass\n";
        let first = parse(&l, old, None).unwrap();
        let d = Delta::between(old, new);
        let second = parse(&l, new, Some((d.input(old, new), &first.tree))).unwrap();
        let fresh = parse(&l, new, None).unwrap();
        assert_eq!(second.tree.root_node().to_sexp(), fresh.tree.root_node().to_sexp());
        let titles: Vec<_> = second.outline.iter().map(|i| (i.depth, i.title.as_str())).collect();
        assert_eq!(titles, [(0, "A"), (1, "f()"), (1, "g()")]);
    }

    #[test]
    #[ignore = "Downloads C, Go, TypeScript and Markdown grammars and compiles them with cc"]
    fn grammars_build_other_outlines() {
        let titles = |name: &str, text: &str| {
            parse_now(name, text).outline.into_iter().map(|i| (i.depth, i.title)).collect::<Vec<_>>()
        };
        let c = titles("c", "struct point { int x; };\nstatic int *make(int n) {\n  return 0;\n}\nstruct point p;\n");
        assert_eq!(c, [(0, "point".into()), (0, "make()".into())]);
        let go = titles("go", "package p\ntype T struct{}\nfunc (t T) M() {}\nfunc F() {}\n");
        assert_eq!(go, [(0, "T".into()), (0, "M()".into()), (0, "F()".into())]);
        let ts = titles("typescript", "class A {\n  m() {}\n}\nconst f = () => 1;\ninterface I {}\n");
        assert_eq!(ts, [(0, "A".into()), (1, "m()".into()), (0, "f()".into()), (0, "I".into())]);
        let md = titles("markdown", "# One\ntext\n## Two\n");
        assert_eq!(md, [(0, "One".into()), (1, "Two".into())]);
    }
}
