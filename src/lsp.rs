//! JSON-RPC over stdio. The worker owns the process so stalled servers never block input.
use crate::kernel::Waker;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub fn uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}
pub fn path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let s = uri.strip_prefix("file://")?;
    if !s.starts_with('/') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(s.as_bytes()[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}
pub fn position(text: &str, at: usize) -> Value {
    let at = crate::editor::floor(text, at);
    let start = text[..at].rfind('\n').map_or(0, |p| p + 1);
    json!({"line":text[..at].bytes().filter(|b| *b == b'\n').count(),"character":text[start..at].encode_utf16().count()})
}
pub fn offset(text: &str, pos: &Value) -> Option<usize> {
    let line = pos["line"].as_u64()? as usize;
    let column = pos["character"].as_u64()? as usize;
    let start = if line == 0 { 0 } else { text.match_indices('\n').nth(line - 1)?.0 + 1 };
    let end = text[start..].find('\n').map_or(text.len(), |p| start + p);
    let mut units = 0;
    for (i, c) in text[start..end].char_indices() {
        if units == column {
            return Some(start + i);
        }
        units += c.len_utf16();
    }
    (units == column).then_some(end)
}
pub fn edits(text: &str, changes: &[Value]) -> Result<String, String> {
    let mut ranges = Vec::new();
    for e in changes {
        let a = offset(text, &e["range"]["start"]).ok_or("Invalid edit start")?;
        let b = offset(text, &e["range"]["end"]).ok_or("Invalid edit end")?;
        if a > b {
            return Err("Reversed edit range".into());
        }
        ranges.push((a, b, e["newText"].as_str().ok_or("Missing replacement")?));
    }
    ranges.sort_by_key(|r| (r.0, r.1));
    if ranges.windows(2).any(|w| w[0].1 > w[1].0) {
        return Err("Overlapping server edits".into());
    }
    let mut out = text.to_string();
    for (a, b, s) in ranges.into_iter().rev() {
        out.replace_range(a..b, s);
    }
    Ok(out)
}
/// Configuration is argv, never shell text. Extension entries override language IDs.
pub fn server(root: &Path, path: &Path) -> Result<Option<(String, Vec<String>)>, String> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let id = crate::lsp_registry::language(path);
    let override_config = server_settings(root,id)?;
    if override_config == false { return Ok(None); }
    if let Some(cmd) = override_config["command"].as_str() {
        let mut command = vec![cmd.to_string()];
        command.extend(override_config["args"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
        return Ok(Some((id.into(),command)));
    }
    let config = root.join(".lightdown/lsp.json");
    if config.exists() {
        let bytes = std::fs::read(&config).map_err(|e| e.to_string())?;
        if bytes.len() > 512 * 1024 { return Err("lsp.json exceeds 512 KB".into()); }
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("lsp.json: {e}"))?;
        let entry = v.get(ext).or_else(|| v.get(format!(".{ext}")).or_else(|| v.get(id)));
        if let Some(entry) = entry {
            if entry == false || entry.is_null() { return Ok(None); }
            let args = entry.as_array().or_else(|| entry["command"].as_array()).ok_or("Server command must be an argv array")?;
            let command: Option<Vec<String>> = args.iter().map(|s| s.as_str().map(str::to_string)).collect();
            let command = command.filter(|v| !v.is_empty() && !v[0].is_empty()).ok_or("Invalid server argv")?;
            let id = entry["languageId"].as_str().unwrap_or(id).to_string();
            return Ok(Some((id,command)));
        }
    }
    let servers: Vec<_> = crate::lsp_registry::for_language(id).collect();
    for server in &servers {
        if let Some(command) = crate::lsp_registry::local(root,server.binary) {
            let mut cmd = vec![command.to_string_lossy().into_owned()];
            cmd.extend(server.args.iter().map(|s| s.to_string()));
            return Ok(Some((id.into(),cmd)));
        }
    }
    for server in &servers {
        if let Some(command) = crate::lsp_registry::command(root,server) { return Ok(Some((id.into(),command))); }
    }
    Ok(servers.first().map(|s| (id.into(),std::iter::once(s.binary.to_string()).chain(s.args.iter().map(|s| s.to_string())).collect())))
}
/// Per-language options share project settings, while lsp.json stays compatible.
pub fn server_settings(root: &Path, id: &str) -> Result<Value,String> {
    let p = root.join(".lightdown/settings.json");
    if !p.exists() { return Ok(Value::Null); }
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    if bytes.len() > 512 * 1024 { return Err("settings.json exceeds 512 KB".into()); }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("settings.json: {e}"))?;
    Ok(v["language_servers"][id].clone())
}
fn read_frame(r: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some((key, v)) = line.split_once(':')
            && key.eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse::<usize>().ok();
        }
    }
    let n = length.filter(|n| *n <= 16 * 1024 * 1024)?;
    let mut bytes = vec![0; n];
    r.read_exact(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}
fn write_frame(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", bytes.len())?;
    w.write_all(&bytes)?;
    w.flush()
}
/// Agent requests have their own reply channels, so they cannot consume editor replies.
struct Broker {
    tx: Sender<Value>,
    replies: Mutex<std::collections::HashMap<u64, Sender<Value>>>,
    opened: Mutex<std::collections::HashMap<String, String>>,
    capabilities: Mutex<Value>,
    diagnostics: Mutex<std::collections::HashMap<String, Value>>,
}
type Registry = std::collections::HashMap<(PathBuf, Vec<String>), std::sync::Weak<Broker>>;
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: std::sync::OnceLock<Mutex<Registry>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
/// Reuse only a synchronized document. Otherwise the isolated worker preserves unsaved text.
pub fn shared_query(root: &Path, command: &[String], uri: &str, text: &str, method: &str, params: Value, stop: &AtomicBool) -> Option<Result<Value, String>> {
    shared_query_within(root, command, uri, text, method, params, stop, 30)
}
#[allow(clippy::too_many_arguments)]
pub fn shared_query_within(root: &Path, command: &[String], uri: &str, text: &str, method: &str, params: Value, stop: &AtomicBool, seconds: u64) -> Option<Result<Value, String>> {
    let broker = registry().lock().unwrap().get(&(root.to_path_buf(), command.to_vec()))?.upgrade()?;
    if broker.opened.lock().unwrap().get(uri).is_none_or(|sent| sent != text) { return None; }
    if method == "textDocument/diagnostic" && !broker.capabilities.lock().unwrap()["diagnosticProvider"].is_object() {
        // Push-only servers: wait for the report on this text, which clears on each change.
        let started = std::time::Instant::now();
        return Some(loop {
            if let Some(report) = broker.diagnostics.lock().unwrap().get(uri).cloned() { break Ok(report); }
            if stop.load(Ordering::SeqCst) { break Err("Language-server tool stopped".into()); }
            if started.elapsed().as_secs() >= seconds { break Err("Running language server published no diagnostics in time. Diagnostics are unknown.".into()); }
            std::thread::sleep(std::time::Duration::from_millis(20));
        });
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 40);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    broker.replies.lock().unwrap().insert(id, tx);
    let _ = broker.tx.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
    let started = std::time::Instant::now();
    let result = loop {
        if stop.load(Ordering::SeqCst) { break Err("Language-server tool stopped".into()); }
        if started.elapsed().as_secs() >= seconds { break Err("Running language server timed out".into()); }
        match rx.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(v) => break if v["error"].is_null() { Ok(v["result"].clone()) } else { Err(v["error"].to_string()) },
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err("Language server disconnected".into()),
            Err(_) => {}
        }
    };
    broker.replies.lock().unwrap().remove(&id);
    Some(result)
}
pub struct Client {
    tx: Sender<Value>,
    broker: Arc<Broker>,
    rx: Receiver<Value>,
    process: Arc<Mutex<Option<std::process::Child>>>,
    cancelled: Arc<AtomicBool>,
}
impl Client {
    pub fn start_for_language(command: Vec<String>, root: PathBuf, wake: Waker, language: &str) -> Self {
        let options = server_settings(&root,language).unwrap_or(Value::Null);
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let broker = Arc::new(Broker { tx:tx.clone(), replies:Default::default(), opened:Default::default(), capabilities:Mutex::new(Value::Null), diagnostics:Default::default() });
        let routing = broker.clone();
        let process = Arc::new(Mutex::new(None));
        let owned = process.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = cancelled.clone();
        std::thread::spawn(move || {
            let run = || -> Result<(), String> {
                use std::process::{Command, Stdio};
                let remote = crate::remote::active(&root);
                let mut cmd = if let Some(remote) = &remote {
                    if command.is_empty() { return Err("Empty server command".into()); }
                    remote.command(&format!("exec {}", command.iter().map(|s| crate::remote::quote(s)).collect::<Vec<_>>().join(" ")), false)
                } else {
                    let mut cmd = Command::new(command.first().ok_or("Empty server command")?);
                    cmd.args(&command[1..]);
                    // The project's .env and any direnv, mise or Nix environment.
                    crate::environment::apply(&mut cmd, &root, None);
                    cmd
                };
                let send = |input: &mut std::process::ChildStdin, value: &Value| {
                    let value = remote.as_ref().map_or_else(|| value.clone(), |r| r.map_lsp(value, &root, true));
                    write_frame(input, &value)
                };
                let mut child = cmd
                    .current_dir(&root)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("{}: {e}", command[0]))?;
                let mut input = child.stdin.take().unwrap();
                let output = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();
                let log_events = events.clone();
                let log_wake = wake.clone();
                std::thread::spawn(move || {
                    use std::io::Read;
                    let mut reader = stderr;
                    let mut chunk = [0; 2048];
                    let mut total = 0;
                    while let Ok(n) = reader.read(&mut chunk) {
                        if n == 0 { break; }
                        if total < 65536 {
                            let _ = log_events.send(json!({"clientLog":String::from_utf8_lossy(&chunk[..n])}));
                            log_wake();
                        }
                        total += n;
                    }
                });
                *owned.lock().unwrap() = Some(child);
                if stopped.load(Ordering::SeqCst) {
                    let _ = owned.lock().unwrap().as_mut().unwrap().kill();
                }
                let (incoming, frames) = mpsc::channel();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(output);
                    while let Some(v) = read_frame(&mut reader) {
                        if incoming.send(v).is_err() {
                            break;
                        }
                    }
                });
                let result = (|| -> Result<(), String> {
                    send(&mut input,&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"processId":if remote.is_some() { Value::Null } else { json!(std::process::id()) },"rootUri":uri(&root),"initializationOptions":options["initialization_options"],"capabilities":{"window":{"workDoneProgress":true},"workspace":{"applyEdit":true,"workspaceEdit":{"documentChanges":true,"resourceOperations":["create","rename","delete"],"failureHandling":"transactional"},"fileOperations":{"dynamicRegistration":true,"willRename":true,"didRename":true,"willCreate":true,"didCreate":true,"willDelete":true,"didDelete":true},"symbol":{},"semanticTokens":{"refreshSupport":true},"diagnostics":{"refreshSupport":true}},"general":{"positionEncodings":["utf-16"]},"textDocument":{"definition":{"linkSupport":true},"implementation":{"linkSupport":true},"typeDefinition":{"linkSupport":true},"declaration":{"linkSupport":true},"publishDiagnostics":{"versionSupport":true},"completion":{"completionItem":{"snippetSupport":true},"completionList":{"itemDefaults":["editRange","insertTextFormat"]}},"signatureHelp":{"signatureInformation":{"parameterInformation":{"labelOffsetSupport":true}}},"codeLens":{},"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix","refactor","source"]}},"resolveSupport":{"properties":["edit"]}},"callHierarchy":{},"typeHierarchy":{},"semanticTokens":semantic::capability(),"diagnostic":{},"documentSymbol":{"hierarchicalDocumentSymbolSupport":true},"inlayHint":{}}}}})).map_err(|e| e.to_string())?;
                    let started = std::time::Instant::now();
                    let mut ready = false;
                    let mut queued = Vec::new();
                    loop {
                        match commands.recv_timeout(std::time::Duration::from_millis(10)) {
                            Ok(v) if ready => send(&mut input, &v).map_err(|e| e.to_string())?,
                            Ok(v) => queued.push(v),
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(_) => {}
                        }
                        while let Ok(v) = frames.try_recv() {
                            let v = remote.as_ref().map_or_else(|| v.clone(), |r| r.map_lsp(&v, &root, false));
                            if v.get("method").is_none() && let Some(reply) = v["id"].as_u64().and_then(|id| routing.replies.lock().unwrap().remove(&id)) {
                                let _ = reply.send(v);
                                continue;
                            }
                            if v["method"] == "textDocument/publishDiagnostics" {
                                if let Some(uri) = v["params"]["uri"].as_str() {
                                    routing.diagnostics.lock().unwrap().insert(uri.into(), json!({"kind":"full","items":v["params"]["diagnostics"]}));
                                }
                            }
                            if v["id"] == 0 && v.get("method").is_none() {
                                if !v["error"].is_null() {
                                    return Err(v["error"].to_string());
                                }
                                ready = true;
                                *routing.capabilities.lock().unwrap() = v["result"]["capabilities"].clone();
                                let _ = events.send(json!({"clientCapabilities":v["result"]["capabilities"]}));
                                wake();
                                send(&mut input, &json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
                                    .map_err(|e| e.to_string())?;
                                if !options["settings"].is_null() { send(&mut input,&json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":options["settings"]}})).map_err(|e| e.to_string())?; }
                                for v in queued.drain(..) {
                                    send(&mut input, &v).map_err(|e| e.to_string())?;
                                }
                            } else if v.get("method").is_some() && v.get("id").is_some() {
                                if v["method"] == "workspace/applyEdit" || v["method"] == "client/registerCapability" || v["method"] == "client/unregisterCapability" {
                                    let _ = events.send(v.clone()); wake();
                                    if v["method"] == "workspace/applyEdit" { continue; }
                                    send(&mut input,&json!({"jsonrpc":"2.0","id":v["id"],"result":null})).map_err(|e| e.to_string())?;
                                    continue;
                                }
                                let response = match v["method"].as_str() {
                                    Some("window/workDoneProgress/create") => {
                                        json!({"jsonrpc":"2.0","id":v["id"],"result":null})
                                    }
                                    // The editor asks again after a refresh request.
                                    Some(m @ ("workspace/diagnostic/refresh" | "workspace/semanticTokens/refresh" | "workspace/inlayHint/refresh")) => {
                                        let _ = events.send(json!({"method":m}));
                                        wake();
                                        json!({"jsonrpc":"2.0","id":v["id"],"result":null})
                                    }
                                    Some("workspace/configuration") => {
                                        json!({"jsonrpc":"2.0","id":v["id"],"result":v["params"]["items"].as_array().into_iter().flatten().map(|item| { let mut value = &options["settings"]; for part in item["section"].as_str().unwrap_or("").split('.').filter(|s| !s.is_empty()) { value = &value[part]; } value.clone() }).collect::<Vec<_>>()})
                                    }
                                    Some("workspace/workspaceFolders") => {
                                        json!({"jsonrpc":"2.0","id":v["id"],"result":[{"uri":uri(&root),"name":root.file_name().unwrap_or_default().to_string_lossy()}]})
                                    }
                                    _ => {
                                        json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"Client method unsupported"}})
                                    }
                                };
                                send(&mut input, &response).map_err(|e| e.to_string())?;
                            } else {
                                let _ = events.send(v);
                                wake();
                            }
                        }
                        if !ready && started.elapsed().as_secs() > 20 {
                            return Err("Language server initialize timed out".into());
                        }
                        if owned.lock().unwrap().as_mut().unwrap().try_wait().map_err(|e| e.to_string())?.is_some() {
                            return Err("Language server exited".into());
                        }
                    }
                    Ok(())
                })();
                if let Some(mut child) = owned.lock().unwrap().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                result
            };
            if let Err(why) = run() {
                let _ = events.send(json!({"clientError":why}));
                wake();
            }
        });
        Self { tx, broker, rx, process, cancelled }
    }
    /// Lets agent queries reach this client. Only the editor's own clients are shared.
    pub fn share(&self, root: &Path, command: &[String]) {
        registry().lock().unwrap().insert((root.to_path_buf(), command.to_vec()), Arc::downgrade(&self.broker));
    }
    pub fn notify(&self, method: &str, params: Value) {
        if let Some(uri) = params["textDocument"]["uri"].as_str() {
            let text = if method == "textDocument/didOpen" { params["textDocument"]["text"].as_str() }
                else if method == "textDocument/didChange" { params["contentChanges"][0]["text"].as_str() } else { None };
            if let Some(text) = text { self.broker.opened.lock().unwrap().insert(uri.into(),text.into()); self.broker.diagnostics.lock().unwrap().remove(uri); }
        }
        let _ = self.tx.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }
    pub fn request(&self, id: u64, method: &str, params: Value) {
        let _ = self.tx.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
    }
    pub fn respond(&self, id: Value, result: Value) {
        let _ = self.tx.send(json!({"jsonrpc":"2.0","id":id,"result":result}));
    }
    pub fn poll(&self) -> Vec<Value> {
        self.rx.try_iter().collect()
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.broker.opened.lock().unwrap().clear();
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.process.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_positions_and_atomic_edits() {
        let s = "a😀é\nnext";
        assert_eq!(position(s, 5), json!({"line":0,"character":3}));
        assert_eq!(offset(s, &json!({"line":0,"character":3})), Some(5));
        assert!(offset(s, &json!({"line":0,"character":2})).is_none());
        assert_eq!(path(&uri(Path::new("/tmp/a #é.rs"))), Some(PathBuf::from("/tmp/a #é.rs")));
        let e = json!([{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"X"}]);
        assert_eq!(edits(s, e.as_array().unwrap()).unwrap(), "aXé\nnext");
        assert!(edits(s, &[e[0].clone(), e[0].clone()]).is_err());
    }
    #[test]
    fn framing_uses_bytes_and_accepts_headers() {
        let v = json!({"text":"é"});
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &v).unwrap();
        assert_eq!(read_frame(&mut std::io::Cursor::new(bytes)), Some(v));
        assert!(read_frame(&mut std::io::Cursor::new(b"Content-Length: 999999999\r\n\r\n")).is_none());
    }
}

#[cfg(test)]
mod configuration_tests {
    use super::*;
    #[test]
    fn arbitrary_extensions_strict_argv_disabled_and_legacy_ids() {
        let root = std::env::temp_dir().join(format!("ld-ab9-lsp-config-{}",std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),r#"{"md":{"command":["marksman","server"],"languageId":"markdown"},"rust":["ra"],"py":false,".sh":["bash-language-server","start"],"bad":["x",3]}"#).unwrap();
        assert_eq!(server(&root,Path::new("x.md")).unwrap().unwrap(),("markdown".into(),vec!["marksman".into(),"server".into()]));
        assert_eq!(server(&root,Path::new("x.rs")).unwrap().unwrap().1,vec!["ra"]);
        assert!(server(&root,Path::new("x.py")).unwrap().is_none());
        assert_eq!(server(&root,Path::new("x.sh")).unwrap().unwrap().1[1],"start");
        assert!(server(&root,Path::new("x.bad")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Flatten server symbols without changing the local outline parsers.
pub fn document_symbols(text: &str, value: &Value) -> Vec<crate::outline::Item> {
    fn visit(text: &str, values: &Value, depth: usize, out: &mut Vec<crate::outline::Item>) {
        if depth > 64 { return; }
        for symbol in values.as_array().into_iter().flatten().take(10000) {
            if out.len() >= 10000 { return; }
            let position = symbol.pointer("/selectionRange/start").or_else(|| symbol.pointer("/location/range/start"));
            if let (Some(name), Some(at)) = (symbol["name"].as_str(), position.and_then(|p| offset(text,p))) {
                out.push(crate::outline::Item { depth, title:name.chars().take(1024).collect(), at });
            }
            visit(text, &symbol["children"], depth+1, out);
        }
    }
    let mut out = Vec::new();
    visit(text,value,0,&mut out);
    out.sort_by_key(|i| i.at);
    out
}

#[cfg(test)]
mod symbol_tests {
    use super::*;
    #[test]
    fn nested_symbols_use_utf16_selection_ranges_and_flat_symbols_work() {
        let range = json!({"start":{"line":0,"character":3},"end":{"line":0,"character":4}});
        let symbols = json!([{"name":"outer","selectionRange":range,"children":[{"name":"inner","selectionRange":range}]}, {"name":"flat","location":{"range":range}}]);
        let items = document_symbols("a😀b", &symbols);
        assert_eq!(items.len(),3);
        assert_eq!(items[0].at,5);
        assert_eq!(items[1].depth,1);
        assert_eq!(items[2].depth,0);
    }
}

#[path = "lsp_semantic.rs"]
pub mod semantic;
#[path = "lsp_save.rs"]
mod saving;
pub use saving::save_actions;
