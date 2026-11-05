//! Jupyter notebooks. The cells' sources live in the pane as one text, each
//! under a header line; this module keeps what the text does not: outputs,
//! execution counts and metadata. It also reads and writes the `.ipynb` file
//! and runs cells through the Python helper.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::cells::{self, Tag};
use crate::kernel::{Event, Kernel, Waker};
use crate::kernelspec::{self, Choice};
use crate::layout::{Image, OutItem};
use crate::pane::{Decor, Pane};
use crate::pics::Pics;

/// Lines of output shown under a cell before the rest is summarised.
const SHOWN_LINES: usize = 40;
const SHOWN_COLUMNS: usize = 2000;

#[derive(Clone, Copy, PartialEq, Default)]
enum State {
    #[default]
    Idle,
    Queued,
    Running,
}

#[derive(Default)]
struct CellData {
    /// The cell as it was read, for the fields this program does not touch.
    raw: Value,
    outputs: Vec<Value>,
    count: Option<u64>,
    state: State,
    timing: Option<(f64, f64)>,
}

pub struct Notebook {
    /// The notebook's top-level object, without its cells.
    root: Value,
    data: HashMap<u64, CellData>,
    kernel: Option<Kernel>,
    queue: VecDeque<(u64, String)>,
    running: Option<u64>,
    /// The running cell's code, to run again on the built-in kernel when
    /// a Jupyter kernel cannot be started.
    running_code: String,
    /// What runs the cells; decided when the kernel first starts, or chosen.
    pub choice: Option<Choice>,
    /// The kernel was picked from the list, not worked out.
    picked: bool,
    /// Why the kernel is not the one asked for.
    pub kernel_note: Option<String>,
    /// Something to tell the user once, as the status bar's message.
    pub notice: Option<String>,
    /// The kernel's `language_info`, written back on save.
    language_info: Option<Value>,
    /// Cells whose output clears when the next output comes.
    clear_pending: HashSet<u64>,
    /// A cell waiting for a line of input: its prompt and what is typed so far.
    pub input: Option<Prompt>,
    /// The interpreter's version, once it has started.
    pub version: Option<String>,
    pub variables: Vec<(String, String, String)>,
    pub env_root: Option<PathBuf>,
    /// Why the interpreter is not running, if it failed.
    pub trouble: Option<String>,
    /// Cells that finished since `take_finished`, and whether they succeeded.
    finished: Vec<(u64, bool)>,
    /// The latest answer to a completion request, until taken.
    pub matches: Option<(u64, Vec<(String, String)>)>,
    /// The latest answers to documentation requests, until taken.
    pub infos: Vec<(u64, Option<crate::kernel::Info>)>,
    pics: Rc<RefCell<Pics>>,
    stamp: u64,
}

/// A cell's request for input, as the pane shows it under the cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub cell: u64,
    pub prompt: String,
    pub password: bool,
    pub typed: String,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// A text as the notebook format stores it: a list of lines, each keeping
/// its newline.
fn lines_of(s: &str) -> Value {
    Value::Array(s.split_inclusive('\n').map(|l| Value::String(l.to_string())).collect())
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
        } else if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

fn base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// The file an output image is written to, so that it loads like any
/// other picture. Named by its content; the format is told by the bytes.
fn image_file(bytes: &[u8]) -> Option<PathBuf> {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    let path = crate::config::cache_dir().join("outputs").join(format!("{:016x}.img", h.finish()));
    if !path.exists() {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

/// The text of a terminal's colours and weights as the layout's attributes,
/// one per byte of the text left once the escapes are taken out.
pub fn ansi(s: &str) -> (String, Vec<crate::md::Attr>) {
    use crate::md;
    let (mut out, mut attrs) = (String::with_capacity(s.len()), Vec::with_capacity(s.len()));
    let (mut color, mut bold, mut dim) = (0 as crate::md::Attr, false, false);
    let mut rest = s;
    while let Some(at) = rest.find('\u{1b}') {
        let (text, tail) = rest.split_at(at);
        let attr = color | if bold { md::BOLD } else { 0 } | if dim { md::DIM } else { 0 };
        out.push_str(text);
        attrs.resize(out.len(), attr);
        let tail = &tail[1..];
        let Some(body) = tail.strip_prefix('[') else {
            rest = tail;
            continue;
        };
        let end = body.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(body.len());
        let (params, cmd) = (&body[..end], body[end..].chars().next());
        rest = body.get(end + 1..).unwrap_or("");
        if cmd != Some('m') {
            continue;
        }
        let codes: Vec<u32> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
        let mut i = 0;
        while i < codes.len() {
            match codes[i] {
                0 => (color, bold, dim) = (0, false, false),
                1 => bold = true,
                2 => dim = true,
                22 => (bold, dim) = (false, false),
                31 | 91 | 41 | 101 => color = md::SPECIAL,
                32 | 92 => color = md::SOFT,
                33 | 93 => color = md::NUMBER,
                34 | 94 | 36 | 96 => color = md::FUNCTION,
                35 | 95 => color = md::KEYWORD,
                90 => color = md::DIM,
                30 | 37 | 39 | 97 => color = 0,
                38 | 48 => {
                    // 256 colours and true colour have no counterpart in the palette.
                    i += if codes.get(i + 1) == Some(&5) { 2 } else { 4 };
                    color = 0;
                }
                _ => {}
            }
            i += 1;
        }
    }
    let attr = color | if bold { md::BOLD } else { 0 } | if dim { md::DIM } else { 0 };
    out.push_str(rest);
    attrs.resize(out.len(), attr);
    if attrs.iter().all(|&a| a == 0) {
        attrs.clear();
    }
    (out, attrs)
}

/// The richest form of an output's data that can be drawn.
#[derive(Debug, PartialEq)]
enum Form {
    Table(crate::table::Table),
    Image(Vec<u8>),
    Markdown(String),
    Latex(String),
    Text(String),
}

fn richest(data: &Value) -> Form {
    if let Some(table) = crate::table::parse(&text_of(&data["text/html"])) {
        return Form::Table(table);
    }
    for mime in ["image/png", "image/jpeg"] {
        let b64 = text_of(&data[mime]);
        if !b64.is_empty() {
            return Form::Image(base64_decode(&b64));
        }
    }
    let svg = text_of(&data["image/svg+xml"]);
    if !svg.is_empty() {
        return Form::Image(svg.into_bytes());
    }
    let markdown = text_of(&data["text/markdown"]);
    if !markdown.trim().is_empty() {
        return Form::Markdown(markdown);
    }
    let latex = text_of(&data["text/latex"]);
    if !latex.trim().is_empty() {
        return Form::Latex(latex);
    }
    if let Some(json) = data.get("application/json") {
        return Form::Text(serde_json::to_string_pretty(json).unwrap_or_default());
    }
    Form::Text(text_of(&data["text/plain"]))
}

/// The formula in a LaTeX output, without the delimiters around it.
fn formula(latex: &str) -> String {
    let t = latex.trim();
    for (open, close) in [("$$", "$$"), ("\\[", "\\]"), ("$", "$"), ("\\(", "\\)")] {
        if let Some(inner) = t.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            return inner.trim().to_string();
        }
    }
    for env in ["equation*", "equation", "displaymath"] {
        let (open, close) = (format!("\\begin{{{env}}}"), format!("\\end{{{env}}}"));
        if let Some(inner) = t.strip_prefix(&open).and_then(|r| r.strip_suffix(&close)) {
            return inner.trim().to_string();
        }
    }
    t.to_string()
}

/// One output as plain text, and whether it is a picture.
fn output_parts(out: &Value) -> (String, bool, bool) {
    match out["output_type"].as_str().unwrap_or("") {
        "stream" => (text_of(&out["text"]), out["name"] == "stderr", false),
        "error" => {
            let trace: Vec<String> = out["traceback"].as_array().into_iter().flatten().map(text_of).collect();
            let text = if trace.is_empty() {
                format!("{}: {}", text_of(&out["ename"]), text_of(&out["evalue"]))
            } else {
                trace.join("\n")
            };
            (strip_ansi(&text), false, false)
        }
        _ => match richest(&out["data"]) {
            Form::Image(_) => (String::new(), false, true),
            Form::Markdown(text) | Form::Latex(text) | Form::Text(text) => (text, false, false),
            Form::Table(_) => (text_of(&out["data"]["text/plain"]), false, false),
        },
    }
}

impl Notebook {
    /// Reads a notebook file. Returns the notebook and the text to show in
    /// the pane; `attach` must follow once the pane holds that text.
    pub fn open(source: &str, pics: Rc<RefCell<Pics>>) -> Result<(Notebook, String, Vec<Value>), String> {
        let mut root: Value = if source.trim().is_empty() {
            json!({"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5})
        } else {
            serde_json::from_str(source).map_err(|e| format!("Not a notebook: {e}"))?
        };
        let mut raw = match root.get_mut("cells").map(Value::take) {
            Some(Value::Array(cells)) => cells,
            _ if root.is_object() => Vec::new(),
            _ => return Err("Not a notebook: no cells".into()),
        };
        if raw.is_empty() {
            raw.push(json!({"cell_type": "code", "source": ""}));
        }
        let mut text = String::new();
        for (i, cell) in raw.iter().enumerate() {
            let tag = match cell["cell_type"].as_str() {
                Some("markdown") => Tag::Markdown,
                Some("raw") => Tag::Raw,
                _ => Tag::Code,
            };
            if i > 0 {
                text.push('\n');
            }
            text += &cells::header(tag, "");
            text.push('\n');
            text += &text_of(&cell["source"]).replace("\r\n", "\n").replace(cells::MARK, "");
        }
        let nb = Notebook {
            root,
            data: HashMap::new(),
            kernel: None,
            queue: VecDeque::new(),
            running: None,
            running_code: String::new(),
            choice: None,
            picked: false,
            kernel_note: None,
            notice: None,
            language_info: None,
            clear_pending: HashSet::new(),
            input: None,
            version: None,
            variables: Vec::new(),
            env_root: None,
            trouble: None,
            finished: Vec::new(),
            matches: None,
            infos: Vec::new(),
            pics,
            stamp: 0,
        };
        Ok((nb, text, raw))
    }

    /// Ties the cells read from the file to the pane's cells, in order.
    pub fn attach(&mut self, pane: &mut Pane, raw: Vec<Value>) {
        for (cell, mut raw) in pane.cells.iter().zip(raw) {
            let outputs = match raw.get_mut("outputs").map(Value::take) {
                Some(Value::Array(outputs)) => outputs,
                _ => Vec::new(),
            };
            let count = raw["execution_count"].as_u64();
            self.data.insert(
                cell.id,
                CellData {
                    timing: raw["metadata"]["lightdown"]["elapsed_ms"]
                        .as_f64()
                        .zip(raw["metadata"]["lightdown"]["ran_at"].as_f64()),
                    raw,
                    outputs,
                    count,
                    state: State::Idle,
                },
            );
        }
        let language = self.language();
        if pane.cell_language != language {
            pane.cell_language = language;
            pane.reparse();
        }
        self.sync(pane);
    }

    /// The notebook as a file, in the layout Jupyter itself writes.
    pub fn to_json(&self, pane: &Pane) -> String {
        let minor = self.root["nbformat_minor"].as_u64().unwrap_or(5);
        let mut list = Vec::new();
        for (i, cell) in pane.cells.iter().enumerate() {
            let data = self.data.get(&cell.id);
            let mut obj = match data.map(|d| &d.raw) {
                Some(Value::Object(raw)) => raw.clone(),
                _ => Map::new(),
            };
            let (s, e) = pane.cell_body(i);
            obj.insert("cell_type".into(), cell.tag.name().into());
            obj.insert("source".into(), lines_of(&pane.buf.text[s..e]));
            obj.entry("metadata").or_insert_with(|| json!({}));
            if let Some((elapsed, ran_at)) = data.and_then(|d| d.timing) {
                if !obj["metadata"].is_object() {
                    obj.insert("metadata".into(), json!({}));
                }
                let metadata = obj.get_mut("metadata").unwrap();
                if !metadata["lightdown"].is_object() {
                    metadata["lightdown"] = json!({});
                }
                metadata["lightdown"]["elapsed_ms"] = elapsed.into();
                metadata["lightdown"]["ran_at"] = ran_at.into();
            }
            if minor >= 5 && !obj.contains_key("id") {
                let mut h = DefaultHasher::new();
                (cell.id, std::process::id()).hash(&mut h);
                obj.insert("id".into(), format!("{:08x}", h.finish() as u32).into());
            }
            if cell.tag == Tag::Code {
                let mut outputs = data.map_or(Vec::new(), |d| d.outputs.clone());
                for out in &mut outputs {
                    if out["output_type"] == "stream" {
                        out["text"] = lines_of(&text_of(&out["text"]));
                    }
                    if let Some(out) = out.as_object_mut() {
                        out.remove("transient");
                    }
                }
                obj.insert("outputs".into(), outputs.into());
                obj.insert("execution_count".into(), data.and_then(|d| d.count).map_or(Value::Null, Value::from));
            } else {
                obj.remove("outputs");
                obj.remove("execution_count");
            }
            list.push(Value::Object(obj));
        }
        let mut root = match &self.root {
            Value::Object(root) => root.clone(),
            _ => Map::new(),
        };
        root.insert("cells".into(), list.into());
        root.entry("metadata").or_insert_with(|| {
            json!({
                "kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"},
                "language_info": {"name": "python"},
            })
        });
        // The kernel the cells ran on, so Jupyter and Lightdown pick it again.
        if let Some(Value::Object(meta)) = root.get_mut("metadata") {
            if let Some(Choice::Jupyter(spec)) = &self.choice {
                meta.insert("kernelspec".into(), spec.metadata());
            }
            if let Some(info) = &self.language_info {
                meta.insert("language_info".into(), info.clone());
            }
        }
        root.entry("nbformat").or_insert(4.into());
        root.entry("nbformat_minor").or_insert(5.into());
        let mut out = Vec::new();
        let format = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut ser = serde_json::Serializer::with_formatter(&mut out, format);
        let _ = Value::Object(root).serialize(&mut ser);
        out.push(b'\n');
        String::from_utf8(out).unwrap_or_default()
    }

    // ---- what the pane shows ---------------------------------------------

    fn label(&self, id: u64) -> String {
        let data = self.data.get(&id);
        let state: String = match data.map_or(State::Idle, |d| d.state) {
            State::Running => "[*]".into(),
            State::Queued => "[\u{b7}]".into(),
            State::Idle => data.and_then(|d| d.count).map_or("[ ]".into(), |n| format!("[{n}]")),
        };
        match data.and_then(|d| d.timing) {
            Some((ms, _)) if data.is_some_and(|d| d.state == State::Idle) => format!("{state}  {:.2}s", ms / 1000.0),
            _ => state,
        }
    }
    pub fn timing(&self, id: u64) -> Option<(f64, f64)> {
        self.data.get(&id)?.timing
    }

    fn items(&self, outputs: &[Value], maths: Option<&RefCell<crate::math::Maths>>) -> Vec<OutItem> {
        let mut items = Vec::new();
        let (mut shown, mut hidden) = (0, 0);
        for out in outputs {
            let kind = out["output_type"].as_str().unwrap_or("");
            let stderr = out["name"] == "stderr";
            let (text, attrs, error) = match kind {
                "stream" => {
                    let (text, attrs) = ansi(&text_of(&out["text"]));
                    (text, attrs, stderr)
                }
                "error" => {
                    let trace: Vec<String> = out["traceback"].as_array().into_iter().flatten().map(text_of).collect();
                    let raw = if trace.is_empty() {
                        format!("{}: {}", text_of(&out["ename"]), text_of(&out["evalue"]))
                    } else {
                        trace.join("\n")
                    };
                    let (text, attrs) = ansi(&raw);
                    // Coloured tracebacks keep their colours; plain ones are red.
                    let plain = attrs.is_empty();
                    (text, attrs, plain)
                }
                _ => match richest(&out["data"]) {
                    Form::Table(table) => {
                        items.push(OutItem::Table(table));
                        continue;
                    }
                    Form::Image(bytes) => {
                        if let Some(path) = image_file(&bytes) {
                            if let Some((w, h)) = self.pics.borrow_mut().size(&path) {
                                items.push(OutItem::Image(Image { at: 0, w: w as f32, h: h as f32, path, center: false }));
                                continue;
                            }
                        }
                        (text_of(&out["data"]["text/plain"]), Vec::new(), false)
                    }
                    Form::Markdown(text) => {
                        items.push(OutItem::Markdown(text));
                        continue;
                    }
                    Form::Latex(latex) => {
                        let built = maths.and_then(|m| m.borrow_mut().get(&formula(&latex), true).cloned());
                        if let Some(f) = built {
                            items.push(OutItem::Formula(Image { at: 0, w: f.w, h: f.h, path: f.key, center: true }));
                            continue;
                        }
                        let plain = text_of(&out["data"]["text/plain"]);
                        (if plain.is_empty() { latex } else { plain }, Vec::new(), false)
                    }
                    Form::Text(text) => {
                        let (text, attrs) = ansi(&text);
                        (text, attrs, false)
                    }
                },
            };
            let (mut kept, mut kept_attrs) = (String::new(), Vec::new());
            let (mut at, mut lines) = (0, 0);
            for line in text.trim_end_matches('\n').split('\n') {
                let start = at;
                at += line.len() + 1;
                if shown >= SHOWN_LINES {
                    hidden += 1;
                    continue;
                }
                // A carriage return starts the line over, as a terminal would.
                let trimmed = line.trim_end_matches('\r');
                let last = trimmed.rsplit('\r').next().unwrap_or("");
                let from = start + trimmed.len() - last.len();
                let cut = last.char_indices().nth(SHOWN_COLUMNS).map_or(last.len(), |(c, _)| c);
                if lines > 0 {
                    kept.push('\n');
                    kept_attrs.push(0);
                }
                kept += &last[..cut];
                if !attrs.is_empty() {
                    kept_attrs.extend_from_slice(&attrs[from..from + cut]);
                } else {
                    kept_attrs.resize(kept.len(), 0);
                }
                if cut < last.len() {
                    kept.push('\u{2026}');
                    kept_attrs.resize(kept.len(), 0);
                }
                shown += 1;
                lines += 1;
            }
            if lines > 0 {
                let (text, attrs) = if stderr {
                    let mut a = vec![0; "stderr:\n".len()];
                    a.extend(kept_attrs);
                    (format!("stderr:\n{kept}"), a)
                } else {
                    (kept, kept_attrs)
                };
                let attrs = if attrs.iter().all(|&a| a == 0) { Vec::new() } else { attrs };
                items.push(OutItem::Text { text, dim: false, error, attrs });
            }
        }
        if hidden > 0 {
            items.push(OutItem::Text {
                text: format!("\u{2026} {hidden} more lines"),
                dim: true,
                error: false,
                attrs: Vec::new(),
            });
        }
        items
    }

    /// Updates what a cell shows beside its text.
    fn refresh(&mut self, pane: &mut Pane, id: u64) {
        let code = pane.cells.iter().any(|c| c.id == id && c.tag == Tag::Code);
        self.stamp += 1;
        let decor = if code {
            let maths = pane.maths.clone();
            let mut output = self.data.get(&id).map_or(Vec::new(), |d| self.items(&d.outputs, maths.as_deref()));
            if let Some(p) = self.input.as_ref().filter(|p| p.cell == id) {
                let typed = if p.password { "\u{b7}".repeat(p.typed.chars().count()) } else { p.typed.clone() };
                output.push(OutItem::Input { prompt: p.prompt.clone(), typed });
            }
            if let Some((ms, ran_at)) = self.timing(id) {
                let time = ran_at as libc::time_t;
                let mut tm: libc::tm = unsafe { std::mem::zeroed() };
                unsafe {
                    libc::localtime_r(&time, &mut tm);
                }
                output.insert(
                    0,
                    OutItem::Text {
                        text: format!(
                            "Last run {:04}-{:02}-{:02} {:02}:{:02}:{:02} · {:.3}s",
                            tm.tm_year + 1900,
                            tm.tm_mon + 1,
                            tm.tm_mday,
                            tm.tm_hour,
                            tm.tm_min,
                            tm.tm_sec,
                            ms / 1000.0
                        ),
                        dim: true,
                        error: false,
                        attrs: Vec::new(),
                    },
                );
            }
            Decor { label: self.label(id), output, stamp: self.stamp }
        } else {
            Decor::default()
        };
        pane.decor.insert(id, decor);
        pane.redraw = true;
    }

    /// Gives new cells their label and drops what deleted cells left.
    /// Call after any edit to the pane.
    pub fn sync(&mut self, pane: &mut Pane) {
        let stale: Vec<u64> = pane
            .cells
            .iter()
            .filter(|c| {
                let want = if c.tag == Tag::Code { self.label(c.id) } else { String::new() };
                pane.decor.get(&c.id).is_none_or(|d| d.label != want)
            })
            .map(|c| c.id)
            .collect();
        for id in stale {
            self.refresh(pane, id);
        }
        if pane.decor.len() > pane.cells.len() * 2 + 16 {
            pane.decor.retain(|id, _| pane.cells.iter().any(|c| c.id == *id));
            self.data.retain(|id, _| pane.cells.iter().any(|c| c.id == *id));
        }
    }

    // ---- running ---------------------------------------------------------

    pub fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }

    pub fn kernel_started(&self) -> bool {
        self.kernel.is_some()
    }

    /// Clearing outputs also clears saved execution counts.
    pub fn can_clear_outputs(&self) -> bool {
        self.data.values().any(|d| !d.outputs.is_empty() || d.count.is_some())
    }

    /// The notebook's metadata, which names its kernel and language.
    pub fn metadata(&self) -> &Value {
        &self.root["metadata"]
    }

    /// The language the cells are written in: the chosen kernel's, or the metadata's.
    pub fn language(&self) -> String {
        match &self.choice {
            Some(choice) => choice.language(),
            None => kernelspec::notebook_language(self.metadata()),
        }
    }

    /// What the status bar calls the kernel.
    pub fn kernel_label(&self) -> String {
        match (&self.choice, &self.version) {
            (Some(Choice::Builtin) | None, Some(v)) if self.kernel.as_ref().is_some_and(|k| !k.is_jupyter()) => {
                format!("Python {v}")
            }
            (Some(choice), _) => choice.label(),
            (None, _) if self.language() == "python" => "Python".into(),
            (None, _) => self.metadata()["kernelspec"]["display_name"].as_str().unwrap_or("Kernel").to_string(),
        }
    }

    /// Uses another kernel from now on. The running one ends, as on a restart.
    pub fn set_choice(&mut self, pane: &mut Pane, choice: Choice) {
        self.restart(pane);
        if let Some(meta) = self.root.get_mut("metadata").filter(|m| m.is_object()) {
            if choice == Choice::Builtin {
                if !meta["lightdown"].is_object() {
                    meta["lightdown"] = json!({});
                }
                meta["lightdown"]["kernel"] = "builtin".into();
            } else if let Some(own) = meta.get_mut("lightdown").and_then(Value::as_object_mut) {
                own.remove("kernel");
            }
        } else {
            self.root["metadata"] = json!({});
            if choice == Choice::Builtin {
                self.root["metadata"]["lightdown"] = json!({"kernel": "builtin"});
            }
        }
        if self.language() != choice.language() {
            self.language_info = None;
        }
        self.choice = Some(choice);
        self.picked = true;
        self.kernel_note = None;
        pane.cell_language = self.language();
        pane.reparse();
    }

    /// The kernel, started on first use: the chosen one, or the one the
    /// notebook's metadata and the installed kernels point to.
    pub fn kernel(&mut self, python: &str, dir: &Path, wake: &Waker) -> Option<&mut Kernel> {
        if self.kernel.is_none() {
            let root = self.env_root.clone().unwrap_or_else(|| dir.to_path_buf());
            let choice = match self.choice.clone() {
                Some(choice) => Ok(choice),
                None => {
                    let specs = kernelspec::discover(python, &root);
                    kernelspec::choose(self.metadata(), &specs, kernelspec::has_package(python, "ipykernel"))
                }
            };
            let started = match choice {
                Err(why) => Err(why),
                Ok(Choice::Builtin) => {
                    self.choice = Some(Choice::Builtin);
                    Kernel::spawn_in(python, dir, &root, wake.clone()).map_err(|e| format!("Cannot start {python}: {e}"))
                }
                Ok(Choice::Jupyter(spec)) if !kernelspec::has_package(python, "jupyter_client") => {
                    let why = format!(
                        "{} needs jupyter_client in {python}: run `{python} -m pip install jupyter_client`",
                        spec.label()
                    );
                    if spec.language == "python" {
                        self.choice = Some(Choice::Builtin);
                        self.kernel_note = Some(format!("{why}. Using the built-in kernel."));
                        self.notice = self.kernel_note.clone();
                        Kernel::spawn_in(python, dir, &root, wake.clone()).map_err(|e| format!("Cannot start {python}: {e}"))
                    } else {
                        Err(why)
                    }
                }
                Ok(Choice::Jupyter(spec)) => {
                    let started = Kernel::spawn_jupyter(python, &spec, dir, &root, wake.clone());
                    self.choice = Some(Choice::Jupyter(spec));
                    started.map_err(|e| format!("Cannot start {python}: {e}"))
                }
            };
            match started {
                Ok(k) => (self.kernel, self.trouble) = (Some(k), None),
                Err(e) => self.trouble = Some(e),
            }
        }
        self.kernel.as_mut()
    }

    /// Answers the cell waiting for input with what was typed.
    pub fn submit_input(&mut self, pane: &mut Pane) {
        let Some(p) = self.input.take() else { return };
        if let Some(k) = &mut self.kernel {
            k.input(&p.typed);
        }
        let shown = if p.password { "\u{b7}".repeat(p.typed.chars().count()) } else { p.typed.clone() };
        self.data.entry(p.cell).or_default().outputs.push(json!({"output_type": "stream", "name": "stdout",
            "text": format!("{}{shown}\n", p.prompt)}));
        self.refresh(pane, p.cell);
    }

    /// Changes what is typed in answer to an input request.
    pub fn edit_input(&mut self, pane: &mut Pane, edit: impl FnOnce(&mut String)) {
        let Some(p) = &mut self.input else { return };
        edit(&mut p.typed);
        let cell = p.cell;
        self.refresh(pane, cell);
    }

    /// Queues a code cell to run. Returns whether it was one.
    pub fn run(&mut self, pane: &mut Pane, index: usize, python: &str, dir: &Path, wake: &Waker) -> bool {
        let Some(cell) = pane.cells.get(index).filter(|c| c.tag == Tag::Code) else { return false };
        let id = cell.id;
        let (s, e) = pane.cell_body(index);
        let code = pane.buf.text[s..e].to_string();
        if self.queue.iter().any(|q| q.0 == id) || self.running == Some(id) {
            return true;
        }
        let data = self.data.entry(id).or_default();
        data.outputs.clear();
        data.state = State::Queued;
        self.queue.push_back((id, code));
        self.pump(python, dir, wake);
        self.refresh(pane, id);
        if let Some(why) = self.trouble.clone() {
            self.fail(pane, &why);
        }
        true
    }

    fn pump(&mut self, python: &str, dir: &Path, wake: &Waker) {
        if self.running.is_some() || self.queue.is_empty() || self.kernel(python, dir, wake).is_none() {
            return;
        }
        let (id, code) = self.queue.pop_front().unwrap();
        self.kernel.as_mut().unwrap().execute(id, &code);
        self.running = Some(id);
        self.running_code = code;
        self.data.entry(id).or_default().state = State::Running;
    }

    /// Ends everything running or waiting, reporting `why` under each cell.
    fn fail(&mut self, pane: &mut Pane, why: &str) {
        let ids: Vec<u64> = self.running.take().into_iter().chain(self.queue.drain(..).map(|q| q.0)).collect();
        for id in ids {
            let data = self.data.entry(id).or_default();
            data.state = State::Idle;
            data.outputs.push(json!({"output_type": "error", "ename": "Kernel", "evalue": why, "traceback": []}));
            self.finished.push((id, false));
            self.refresh(pane, id);
        }
    }

    /// Stops the running cell and forgets the ones waiting.
    pub fn interrupt(&mut self, pane: &mut Pane) {
        for (id, _) in std::mem::take(&mut self.queue) {
            self.data.entry(id).or_default().state = State::Idle;
            self.finished.push((id, false));
            self.refresh(pane, id);
        }
        if let (Some(k), Some(_)) = (&mut self.kernel, self.running) {
            k.interrupt();
        }
    }

    /// Ends the interpreter; the next cell run starts a new one.
    pub fn restart(&mut self, pane: &mut Pane) {
        self.kernel = None;
        self.version = None;
        if let Some(p) = self.input.take() {
            self.refresh(pane, p.cell);
        }
        self.clear_pending.clear();
        self.variables.clear();
        self.fail(pane, "The kernel was restarted");
    }

    /// Draws again the cells with formulas, once TeX has set them.
    pub fn refresh_all(&mut self, pane: &mut Pane) {
        let ids: Vec<u64> = self
            .data
            .iter()
            .filter(|(_, d)| d.outputs.iter().any(|o| o["data"].get("text/latex").is_some()))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.refresh(pane, id);
        }
    }

    /// Forgets a kernel the notebook did not pick itself, so the next start
    /// chooses again: after the interpreter changed, say.
    pub fn reset_choice(&mut self) {
        if self.root["metadata"]["lightdown"]["kernel"] != "builtin" && !self.picked {
            self.choice = None;
            self.kernel_note = None;
        }
    }

    pub fn clear_outputs(&mut self, pane: &mut Pane) {
        let ids: Vec<u64> = pane.cells.iter().map(|c| c.id).collect();
        for id in ids {
            if let Some(d) = self.data.get_mut(&id).filter(|d| !d.outputs.is_empty() || d.count.is_some()) {
                d.outputs.clear();
                d.count = None;
                self.refresh(pane, id);
            }
        }
    }

    /// Takes in what the interpreter reported. Returns whether anything
    /// changed. `python` and `dir` are for starting the next queued cell.
    pub fn poll(&mut self, pane: &mut Pane, python: &str, dir: &Path, wake: &Waker) -> bool {
        let events = self.kernel.as_mut().map_or(Vec::new(), Kernel::poll);
        let changed = !events.is_empty();
        for ev in events {
            self.accept(pane, ev);
        }
        self.pump(python, dir, wake);
        if let Some(id) = self.running.filter(|_| changed) {
            self.refresh(pane, id);
        }
        changed
    }

    fn accept(&mut self, pane: &mut Pane, ev: Event) {
        let push = |nb: &mut Notebook, id: u64, out: Value| nb.data.entry(id).or_default().outputs.push(out);
        // A `clear_output(wait=True)` clears when the next output comes.
        if let Event::Stream { id, .. } | Event::Show { id, update: false, .. } | Event::Error { id, .. } = &ev {
            if self.clear_pending.remove(id) {
                self.data.entry(*id).or_default().outputs.clear();
            }
        }
        let id = match ev {
            Event::Variables(items) => {
                self.variables = items;
                return;
            }
            Event::Timing { id, elapsed_ms, ran_at } => {
                self.data.entry(id).or_default().timing = Some((elapsed_ms, ran_at));
                id
            }
            Event::Ready(version, language_info) => {
                self.version = Some(version);
                if let Some(info) = language_info.filter(Value::is_object) {
                    self.language_info = Some(info);
                }
                return;
            }
            Event::Clear { id, wait } => {
                if wait {
                    self.clear_pending.insert(id);
                    return;
                }
                self.data.entry(id).or_default().outputs.clear();
                id
            }
            Event::Input { id, prompt, password } => {
                self.input = Some(Prompt { cell: id, prompt, password, typed: String::new() });
                id
            }
            Event::Missing(why) => {
                // Python cells run on the built-in kernel instead; others cannot run.
                self.kernel = None;
                if self.language() == "python" {
                    self.choice = Some(Choice::Builtin);
                    self.kernel_note = Some(format!("{why}. Using the built-in kernel."));
                    self.notice = self.kernel_note.clone();
                    if let Some(id) = self.running.take() {
                        self.queue.push_front((id, std::mem::take(&mut self.running_code)));
                    }
                } else {
                    self.fail(pane, &why);
                    self.trouble = Some(why);
                }
                return;
            }
            Event::Matches { id, items } => {
                self.matches = Some((id, items));
                return;
            }
            Event::Info { id, info } => {
                self.infos.push((id, info));
                return;
            }
            Event::Stream { id, err, text } => {
                let name = if err { "stderr" } else { "stdout" };
                let outputs = &mut self.data.entry(id).or_default().outputs;
                match outputs.last_mut().filter(|o| o["output_type"] == "stream" && o["name"] == name) {
                    Some(last) => last["text"] = (text_of(&last["text"]) + &text).into(),
                    None => outputs.push(json!({"output_type": "stream", "name": name, "text": text})),
                }
                id
            }
            Event::Show { id, result, data, metadata, display_id, update } => {
                if update {
                    // Every output shown under this display, in any cell, takes the new data.
                    let Some(display_id) = display_id else { return };
                    let mut changed = Vec::new();
                    for (cell, d) in &mut self.data {
                        for out in d.outputs.iter_mut().filter(|o| o["transient"]["display_id"] == *display_id) {
                            out["data"] = Value::Object(data.clone());
                            out["metadata"] = metadata.clone();
                            changed.push(*cell);
                        }
                    }
                    changed.dedup();
                    for cell in changed {
                        self.refresh(pane, cell);
                    }
                    return;
                }
                let kind = if result { "execute_result" } else { "display_data" };
                let mut out = json!({"output_type": kind, "data": data, "metadata": metadata});
                if result {
                    out["execution_count"] = Value::Null;
                }
                if let Some(display_id) = display_id {
                    out["transient"] = json!({"display_id": display_id});
                }
                push(self, id, out);
                id
            }
            Event::Error { id, name, value, trace } => {
                push(self, id, json!({"output_type": "error", "ename": name, "evalue": value, "traceback": trace}));
                id
            }
            Event::Done { id, count, ok } => {
                let data = self.data.entry(id).or_default();
                (data.count, data.state) = (Some(count), State::Idle);
                for out in data.outputs.iter_mut().filter(|o| o["output_type"] == "execute_result") {
                    out["execution_count"] = count.into();
                }
                self.running = None;
                self.finished.push((id, ok));
                if self.input.as_ref().is_some_and(|p| p.cell == id) {
                    self.input = None;
                }
                id
            }
            Event::Dead(last) => {
                self.kernel = None;
                self.input = None;
                self.variables.clear();
                self.version = None;
                let why = if last.is_empty() {
                    "The kernel stopped".to_string()
                } else {
                    format!("The kernel stopped: {last}")
                };
                self.fail(pane, &why);
                self.trouble = Some(why);
                return;
            }
        };
        self.refresh(pane, id);
    }

    /// Blocks until nothing is running. For tests and headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub fn settle(&mut self, pane: &mut Pane, python: &str, dir: &Path, wake: &Waker) {
        while self.busy() {
            let Some(ev) = self.kernel.as_mut().and_then(Kernel::wait) else { break };
            self.accept(pane, ev);
            self.pump(python, dir, wake);
        }
    }

    pub fn take_finished(&mut self) -> Vec<(u64, bool)> {
        std::mem::take(&mut self.finished)
    }

    // ---- for the agent ---------------------------------------------------

    /// A cell's output as plain text, no longer than `max` bytes.
    pub fn output_text(&self, id: u64, max: usize) -> String {
        let mut text = String::new();
        for out in self.data.get(&id).map_or(&[][..], |d| &d.outputs[..]) {
            let (part, _, image) = output_parts(out);
            text += if image { "[image]\n" } else { &part };
            if !text.ends_with('\n') {
                text.push('\n');
            }
        }
        clip(text.trim_end(), max)
    }

    /// The notebook as the agent reads it: every cell with its number, kind,
    /// source and output.
    pub fn describe(&self, pane: &Pane, source_max: usize, output_max: usize) -> String {
        let state = match (&self.kernel, self.busy()) {
            (None, _) => "not started",
            (Some(_), true) => "busy",
            (Some(_), false) => "idle",
        };
        let mut out =
            format!("{} cells; kernel {state}; cursor in cell {}\n", pane.cells.len(), pane.cell_at(pane.buf.cur));
        for (i, cell) in pane.cells.iter().enumerate() {
            let (s, e) = pane.cell_body(i);
            let count =
                self.data.get(&cell.id).and_then(|d| d.count).map_or(String::new(), |n| format!(", run as [{n}]"));
            out += &format!(
                "\n--- cell {i} ({}{count}) ---\n{}\n",
                cell.tag.name(),
                clip(&pane.buf.text[s..e], source_max)
            );
            let shown = self.output_text(cell.id, output_max);
            if !shown.is_empty() {
                out += &format!("--- output of cell {i} ---\n{shown}\n");
            }
        }
        out
    }
}

/// Shortens a text to about `max` bytes, saying how much was left out.
pub fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let cut = crate::editor::floor(s, max);
    format!("{}\n[\u{2026} {} more bytes]", &s[..cut], s.len() - cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::Fonts;
    use crate::pane::Doc;

    const SAMPLE: &str = r##"{
 "cells": [
  {
   "cell_type": "markdown",
   "id": "aa11",
   "metadata": {},
   "source": [
    "# Title\n",
    "text"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": 2,
   "id": "bb22",
   "metadata": {
    "tags": [
     "keep"
    ]
   },
   "outputs": [
    {
     "name": "stdout",
     "output_type": "stream",
     "text": [
      "hi\n"
     ]
    }
   ],
   "source": [
    "print('hi')"
   ]
  }
 ],
 "metadata": {
  "kernelspec": {
   "display_name": "Python 3",
   "language": "python",
   "name": "python3"
  }
 },
 "nbformat": 4,
 "nbformat_minor": 5
}
"##;

    fn load(source: &str) -> (Notebook, Pane) {
        let pics = Rc::new(RefCell::new(Pics::new()));
        let mut pane = Pane::new(Rc::new(RefCell::new(Fonts::new())), pics.clone());
        let (mut nb, text, raw) = Notebook::open(source, pics).unwrap();
        pane.set_text(text, Doc::Cells);
        nb.attach(&mut pane, raw);
        (nb, pane)
    }

    #[test]
    fn notebooks_round_trip() {
        let (nb, pane) = load(SAMPLE);
        assert_eq!(pane.cells.len(), 2);
        assert_eq!(pane.buf.text, "\u{1e}markdown\n# Title\ntext\n\u{1e}code\nprint('hi')");
        assert_eq!(nb.to_json(&pane), SAMPLE);
        assert_eq!(nb.output_text(pane.cells[1].id, 100), "hi");
        assert_eq!(pane.decor[&pane.cells[1].id].label, "[2]");
    }

    #[test]
    fn outputs_follow_their_cells_through_edits() {
        let (mut nb, mut pane) = load(SAMPLE);
        let code = pane.cells[1].id;
        // A new cell above pushes the code cell down; it keeps its output.
        let at = pane.cell_span(0).0;
        assert!(pane.edit(at, at, "\u{1e}code\nx = 1\n", crate::editor::EditKind::Other));
        nb.sync(&mut pane);
        assert_eq!((pane.cells.len(), pane.cells[2].id), (3, code));
        assert_eq!(pane.decor[&pane.cells[0].id].label, "[ ]");
        assert!(nb.to_json(&pane).contains("\"hi\\n\""));
        // Headers cannot be damaged by ordinary edits.
        let header = pane.cell_span(2).0;
        assert!(!pane.edit(header + 1, header + 3, "", crate::editor::EditKind::Delete));
        assert!(!pane.edit(header + 5, header + 5, "x", crate::editor::EditKind::Type));
        // Deleting a whole cell drops it and nothing else.
        let (s, e) = pane.cell_span(1);
        assert!(pane.edit(s, e, "", crate::editor::EditKind::Other));
        assert_eq!((pane.cells.len(), pane.cells[1].id), (2, code));
    }

    #[test]
    fn cells_run_in_order() {
        if std::process::Command::new("python3").arg("-V").output().is_err() {
            return;
        }
        let (mut nb, mut pane) = load("");
        let wake: Waker = std::sync::Arc::new(|| {});
        let (s, e) = pane.cell_body(0);
        pane.edit(s, e, "x = 20\nprint('out')\nx + 1\n\u{1e}code\nx * 2", crate::editor::EditKind::Other);
        nb.sync(&mut pane);
        for i in 0..2 {
            assert!(nb.run(&mut pane, i, "python3", Path::new("."), &wake));
        }
        nb.settle(&mut pane, "python3", Path::new("."), &wake);
        let ids: Vec<u64> = pane.cells.iter().map(|c| c.id).collect();
        assert_eq!(nb.output_text(ids[0], 100), "out\n21");
        assert_eq!(nb.output_text(ids[1], 100), "40");
        assert!(pane.decor[&ids[1]].label.starts_with("[2]  "));
        assert!(nb.timing(ids[1]).is_some_and(|(ms, time)| ms >= 0.0 && time > 0.0));
        assert!(nb.variables.iter().any(|(name, kind, _)| name == "x" && kind == "int"));
        let json: Value = serde_json::from_str(&nb.to_json(&pane)).unwrap();
        assert!(json["cells"][1]["metadata"]["lightdown"]["elapsed_ms"].is_number());
        assert_eq!(nb.take_finished(), [(ids[0], true), (ids[1], true)]);
    }
}
#[cfg(test)]
mod table_output_tests {
    use super::*;
    use crate::font::Fonts;
    #[test]
    fn stored_html_tables_render_natively_and_roundtrip() {
        let source=json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[{"cell_type":"code","source":["frame"],"metadata":{},"execution_count":1,"outputs":[{"output_type":"execute_result","execution_count":1,"metadata":{},"data":{"text/plain":"fallback","text/html":["<table><tr><th>A</th></tr>","<tr><td>é &amp; x</td></tr></table>"]}}]}]}).to_string();
        let pics = Rc::new(RefCell::new(Pics::new()));
        let (mut nb, text, raw) = Notebook::open(&source, pics.clone()).unwrap();
        let mut pane = Pane::new(Rc::new(RefCell::new(Fonts::new())), pics);
        pane.set_text(text, crate::pane::Doc::Cells);
        nb.attach(&mut pane, raw);
        assert!(
            pane.decor
                .values()
                .any(|d| d.output.iter().any(|o| matches!(o,OutItem::Table(t) if t.rows[1][0]=="é & x")))
        );
        pane.set_rect(0.0, 0.0, 500.0, 400.0, 1.0);
        let mut pixels = vec![0; 500 * 400];
        pane.draw(&mut crate::gfx::Canvas::new(&mut pixels, 500, 400));
        let saved: Value = serde_json::from_str(&nb.to_json(&pane)).unwrap();
        assert!(saved["cells"][0]["outputs"][0]["data"]["text/html"].is_array());
    }
}
#[cfg(test)]
mod rich_output_tests {
    use super::*;
    use crate::font::Fonts;
    use crate::pane::Doc;

    fn load(source: &str) -> (Notebook, Pane) {
        let pics = Rc::new(RefCell::new(Pics::new()));
        let mut pane = Pane::new(Rc::new(RefCell::new(Fonts::new())), pics.clone());
        let (mut nb, text, raw) = Notebook::open(source, pics).unwrap();
        pane.set_text(text, Doc::Cells);
        nb.attach(&mut pane, raw);
        (nb, pane)
    }

    fn show(id: u64, data: Value, display_id: Option<&str>, update: bool) -> Event {
        Event::Show {
            id,
            result: false,
            data: data.as_object().unwrap().clone(),
            metadata: json!({}),
            display_id: display_id.map(str::to_string),
            update,
        }
    }

    #[test]
    fn terminal_colours_become_attributes() {
        let (text, attrs) = ansi("\u{1b}[31mErr\u{1b}[0m: \u{1b}[1;32mok\u{1b}[39m!");
        assert_eq!(text, "Err: ok!");
        assert_eq!(attrs[0], crate::md::SPECIAL);
        assert_eq!(attrs[3], 0);
        assert_eq!(attrs[5], crate::md::SOFT | crate::md::BOLD);
        assert_eq!(attrs[7], crate::md::BOLD);
        assert_eq!(ansi("plain"), ("plain".to_string(), Vec::new()));
        // 256 colours are skipped with their arguments.
        assert_eq!(ansi("\u{1b}[38;5;208mx\u{1b}[0m").0, "x");
    }

    #[test]
    fn the_richest_drawable_form_wins() {
        let table = json!({"text/html": "<table><tr><td>1</td></tr></table>", "image/png": "AAAA", "text/plain": "t"});
        assert!(matches!(richest(&table), Form::Table(_)));
        let html = json!({"text/html": "<div>hi</div>", "image/png": "iVBO", "text/plain": "t"});
        assert!(matches!(richest(&html), Form::Image(_)));
        assert_eq!(richest(&json!({"image/svg+xml": "<svg/>"})), Form::Image(b"<svg/>".to_vec()));
        assert_eq!(richest(&json!({"text/markdown": "**b**", "text/plain": "b"})), Form::Markdown("**b**".into()));
        assert_eq!(richest(&json!({"text/latex": "$x^2$", "text/plain": "x"})), Form::Latex("$x^2$".into()));
        let json_out = richest(&json!({"application/json": {"a": [1, 2]}, "text/plain": "{}"}));
        assert!(matches!(json_out, Form::Text(t) if t.contains("\"a\": [\n")));
        assert_eq!(richest(&json!({"text/html": "<b>x</b>", "text/plain": "x"})), Form::Text("x".into()));
        assert_eq!(formula("$$\\frac{1}{2}$$"), "\\frac{1}{2}");
        assert_eq!(formula("\\begin{equation}a=b\\end{equation}"), "a=b");
        assert_eq!(formula("\\[ y \\]"), "y");
    }

    #[test]
    fn displays_update_clear_and_ask_for_input() {
        let (mut nb, mut pane) = load("");
        let id = pane.cells[0].id;
        nb.accept(&mut pane, show(id, json!({"text/plain": "one", "text/markdown": "**one**"}), Some("d"), false));
        nb.accept(&mut pane, show(id, json!({"text/plain": "two"}), Some("d"), true));
        assert_eq!(nb.output_text(id, 100), "two");
        assert!(matches!(&pane.decor[&id].output[..], [OutItem::Text { text, .. }] if text == "two"));
        // A waiting clear leaves the output until the next one comes.
        nb.accept(&mut pane, Event::Clear { id, wait: true });
        assert_eq!(nb.output_text(id, 100), "two");
        nb.accept(&mut pane, Event::Stream { id, err: false, text: "three\n".into() });
        assert_eq!(nb.output_text(id, 100), "three");
        nb.accept(&mut pane, Event::Clear { id, wait: false });
        assert_eq!(nb.output_text(id, 100), "");
        // Markdown is drawn as markdown.
        nb.accept(&mut pane, show(id, json!({"text/plain": "m", "text/markdown": "# M"}), None, false));
        assert!(matches!(&pane.decor[&id].output[..], [OutItem::Markdown(m)] if m == "# M"));
        // input() shows a prompt under the cell; the answer is kept as output.
        nb.accept(&mut pane, Event::Input { id, prompt: "Pin: ".into(), password: true, });
        nb.edit_input(&mut pane, |t| t.push_str("1234"));
        assert!(matches!(pane.decor[&id].output.last(), Some(OutItem::Input { prompt, typed }) if prompt == "Pin: " && typed == "\u{b7}\u{b7}\u{b7}\u{b7}"));
        nb.submit_input(&mut pane);
        assert!(nb.input.is_none());
        assert!(nb.output_text(id, 100).ends_with("Pin: \u{b7}\u{b7}\u{b7}\u{b7}"));
        // Coloured tracebacks keep their colours rather than all turning red.
        nb.accept(&mut pane, Event::Error { id, name: "E".into(), value: "v".into(), trace: vec!["\u{1b}[31mE\u{1b}[0m: v".into()] });
        assert!(matches!(pane.decor[&id].output.last(), Some(OutItem::Text { text, error: false, attrs, .. }) if text == "E: v" && attrs[0] == crate::md::SPECIAL));
    }

    #[test]
    fn saving_records_the_kernel_and_its_language() {
        let (mut nb, mut pane) = load("");
        let id = pane.cells[0].id;
        nb.accept(&mut pane, show(id, json!({"text/plain": "x"}), Some("d"), false));
        let spec = crate::kernelspec::Spec {
            name: "ir".into(),
            display_name: "R".into(),
            language: "r".into(),
            argv: vec!["R".into()],
            dir: None,
            json: json!({}),
        };
        nb.set_choice(&mut pane, Choice::Jupyter(spec));
        assert_eq!(pane.cell_language, "r");
        nb.accept(&mut pane, Event::Ready("4.4.1".into(), Some(json!({"name": "R", "version": "4.4.1"}))));
        let saved: Value = serde_json::from_str(&nb.to_json(&pane)).unwrap();
        assert_eq!(saved["metadata"]["kernelspec"], json!({"display_name": "R", "language": "r", "name": "ir"}));
        assert_eq!(saved["metadata"]["language_info"]["name"], "R");
        assert!(saved["cells"][0]["outputs"][0].get("transient").is_none());
        // The built-in kernel, once picked, is remembered.
        nb.set_choice(&mut pane, Choice::Builtin);
        let saved: Value = serde_json::from_str(&nb.to_json(&pane)).unwrap();
        assert_eq!(saved["metadata"]["lightdown"]["kernel"], "builtin");
        let (reopened, _) = load(&nb.to_json(&pane));
        assert_eq!(
            crate::kernelspec::choose(reopened.metadata(), &[], true),
            Ok(Choice::Builtin)
        );
    }
}
#[cfg(test)]
mod jupyter_notebook_tests {
    use super::*;
    use crate::font::Fonts;
    use crate::pane::Doc;

    #[test]
    fn a_notebook_runs_on_ipykernel_with_rich_output() {
        let Some(python) = crate::kernel::jupyter_tests::jupyter_python() else { return };
        let pics = Rc::new(RefCell::new(Pics::new()));
        let mut pane = Pane::new(Rc::new(RefCell::new(Fonts::new())), pics.clone());
        let source = json!({"nbformat": 4, "nbformat_minor": 5,
            "metadata": {"kernelspec": {"name": "python3", "language": "python", "display_name": "Python 3"}},
            "cells": [{"cell_type": "code", "metadata": {}, "outputs": [], "execution_count": null,
                "source": "from IPython.display import Markdown, SVG, JSON\ndisplay(Markdown('# Hi'))\nSVG('<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"10\"/>')"}]});
        let (mut nb, text, raw) = Notebook::open(&source.to_string(), pics).unwrap();
        pane.set_text(text, Doc::Cells);
        nb.attach(&mut pane, raw);
        let wake: Waker = std::sync::Arc::new(|| {});
        let dir = std::env::temp_dir();
        assert!(nb.run(&mut pane, 0, &python, &dir, &wake));
        nb.settle(&mut pane, &python, &dir, &wake);
        assert!(matches!(nb.choice, Some(Choice::Jupyter(ref s)) if s.name == "python3"), "{:?}", nb.trouble);
        let id = pane.cells[0].id;
        let out = &pane.decor[&id].output;
        assert!(out.iter().any(|o| matches!(o, OutItem::Markdown(m) if m == "# Hi")));
        assert!(out.iter().any(|o| matches!(o, OutItem::Image(im) if im.w == 20.0)));
        assert_eq!(nb.data[&id].count, Some(1));
        let saved: Value = serde_json::from_str(&nb.to_json(&pane)).unwrap();
        assert_eq!(saved["metadata"]["language_info"]["name"], "python");
        assert_eq!(saved["metadata"]["kernelspec"]["name"], "python3");
    }
}
