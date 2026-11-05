//! Bounded newline JSON and Content-Length transport. Pipes are read and written off the UI thread.
use crate::kernel::Waker;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

pub struct Process {
    child: Child,
    tx: Option<Sender<Value>>,
    rx: Receiver<Result<Value, String>>,
}
impl Process {
    pub fn start(cmd: Command, wake: Waker) -> Result<Self, String> {
        Self::start_mode(cmd, wake, false)
    }
    /// LSP Content-Length framing over the same bounded process transport.
    pub fn start_framed(cmd: Command, wake: Waker) -> Result<Self, String> {
        Self::start_mode(cmd, wake, true)
    }
    fn start_mode(mut cmd: Command, wake: Waker, framed: bool) -> Result<Self, String> {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, commands) = mpsc::channel::<Value>();
        let (events, rx) = mpsc::sync_channel(16);
        let writer_events = events.clone();
        let writer_wake = wake.clone();
        std::thread::spawn(move || {
            for v in commands {
                if write_message(&mut input, &v, framed).is_err() {
                    let _ = writer_events.send(Err("Process input closed".into()));
                    writer_wake();
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                match read_message(&mut reader, framed) {
                    Ok(Some(value)) => { if events.send(Ok(value)).is_err() { return; } wake(); }
                    Ok(None) => break,
                    Err(error) => { let _ = events.send(Err(error)); break; }
                }
            }
            let _ = events.send(Err("Process exited".into()));
            wake();
        });
        Ok(Self { child, tx: Some(tx), rx })
    }
    pub fn send(&self, v: Value) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(v);
        }
    }
    pub fn poll(&self) -> Vec<Result<Value, String>> {
        self.rx.try_iter().collect()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.tx.take();
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Streamable HTTP POST transport. curl owns TLS; network I/O runs off the UI thread.
pub struct Http {
    tx: Option<Sender<Value>>,
    rx: Receiver<Result<Value, String>>,
    child: std::sync::Arc<std::sync::Mutex<Option<Child>>>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
fn http_response(raw: &str) -> Result<(String, Vec<Value>), String> {
    let mut body = raw;
    let mut session = String::new();
    while body.starts_with("HTTP/") {
        let (head, rest) = body.split_once("\r\n\r\n").or_else(|| body.split_once("\n\n")).ok_or("Incomplete MCP HTTP headers")?;
        let code = head.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok()).ok_or("Invalid MCP HTTP status")?;
        if !(200..300).contains(&code) && code != 100 { return Err(format!("MCP HTTP status {code}")); }
        for line in head.lines() {
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("mcp-session-id") { session = value.trim().into(); }
            }
        }
        body = rest;
    }
    let mut messages = Vec::new();
    if body.trim_start().starts_with('{') {
        messages.push(serde_json::from_str(body.trim()).map_err(|e| format!("MCP HTTP JSON: {e}"))?);
    } else {
        let mut data = String::new();
        for line in body.lines().chain(std::iter::once("")) {
            if line.is_empty() && !data.is_empty() {
                messages.push(serde_json::from_str(data.trim()).map_err(|e| format!("MCP SSE JSON: {e}"))?);
                data.clear();
            } else if let Some(value) = line.strip_prefix("data:") { data += value.trim_start(); data.push('\n'); }
        }
        if messages.is_empty() && !body.trim().is_empty() { return Err("MCP HTTP returned neither JSON nor event-stream data".into()); }
    }
    Ok((session, messages))
}
fn curl_quote(s: &str) -> Result<String, String> {
    if s.chars().any(char::is_control) { return Err("MCP HTTP URL or headers contain control characters".into()); }
    Ok(s.replace('\\', "\\\\").replace('"', "\\\""))
}
impl Http {
    pub fn start(url: &str, headers: &Value, wake: Waker) -> Result<Self, String> {
        let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or("MCP URL must use HTTP or HTTPS")?;
        if host.is_empty() || host.split('/').next().unwrap_or("").contains('@') { return Err("Invalid MCP HTTP URL".into()); }
        let mut config = format!("url = \"{}\"\nheader = \"Content-Type: application/json\"\nheader = \"Accept: application/json, text/event-stream\"\n", curl_quote(url)?);
        for (k, v) in headers.as_object().into_iter().flatten() {
            if !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') { return Err("Invalid MCP header name".into()); }
            config += &format!("header = \"{}: {}\"\n", k, curl_quote(v.as_str().ok_or("MCP headers must be strings")?)?);
        }
        let (tx, requests) = mpsc::channel::<Value>();
        let (events, rx) = mpsc::sync_channel(16);
        let child = std::sync::Arc::new(std::sync::Mutex::new(None::<Child>));
        let slot = child.clone();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            use std::os::unix::process::CommandExt;
            use std::sync::atomic::Ordering;
            let mut session = String::new();
            let mut version = String::from("2025-06-18");
            for request in requests {
                if stop.load(Ordering::SeqCst) { break; }
                let result = (|| {
                    let mut cfg = config.clone();
                    cfg += &format!("data = \"{}\"\n", curl_quote(&request.to_string())?);
                    if !session.is_empty() { cfg += &format!("header = \"Mcp-Session-Id: {}\"\n", curl_quote(&session)?); }
                    cfg += &format!("header = \"MCP-Protocol-Version: {}\"\n", curl_quote(&version)?);
                    let mut cmd = Command::new("curl");
                    cmd.args(["-q", "--silent", "--show-error", "--no-buffer", "--include", "--max-time", "60", "--max-filesize", "8388608", "--proto", "=http,https", "--request", "POST", "--config", "-"]);
                    cmd.process_group(0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
                    let mut c = cmd.spawn().map_err(|e| format!("MCP curl: {e}"))?;
                    let input = c.stdin.take().unwrap();
                    let output = c.stdout.take().unwrap();
                    *slot.lock().unwrap() = Some(c);
                    let mut input = input;
                    input.write_all(cfg.as_bytes()).map_err(|e| e.to_string())?;
                    drop(input);
                    if stop.load(Ordering::SeqCst) { if let Some(c) = slot.lock().unwrap().as_mut() { let _ = c.kill(); } }
                    let mut bytes = Vec::new();
                    output.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                    if bytes.len() > 8 * 1024 * 1024 { if let Some(c) = slot.lock().unwrap().as_mut() { let _ = c.kill(); } }
                    let status = slot.lock().unwrap().take().ok_or("MCP request canceled")?.wait().map_err(|e| e.to_string())?;
                    if !status.success() { return Err(format!("MCP HTTP request failed: {status}")); }
                    if bytes.len() > 8 * 1024 * 1024 { return Err("MCP HTTP response exceeded 8 MB".into()); }
                    let (new_session, messages) = http_response(&String::from_utf8_lossy(&bytes))?;
                    if !new_session.is_empty() { session = new_session; }
                    for message in messages {
                        if let Some(v) = message["result"]["protocolVersion"].as_str() { version = v.into(); }
                        if events.send(Ok(message)).is_err() { return Err("MCP disconnected".into()); }
                        wake();
                    }
                    Ok::<_, String>(())
                })();
                if let Err(e) = result { let _ = events.send(Err(e)); wake(); break; }
            }
            if let Some(mut c) = slot.lock().unwrap().take() { let _ = c.kill(); let _ = c.wait(); }
        });
        Ok(Self { tx: Some(tx), rx, child, stopped })
    }
    pub fn send(&self, value: Value) { if let Some(tx) = &self.tx { let _ = tx.send(value); } }
    pub fn poll(&self) -> Vec<Result<Value, String>> { self.rx.try_iter().collect() }
}
impl Drop for Http {
    fn drop(&mut self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        self.tx.take();
        if let Some(c) = self.child.lock().unwrap().as_mut() { let _ = c.kill(); }
    }
}
#[cfg(test)]
mod http_tests {
    use super::*;
    #[test]
    fn session_json_sse_and_http_errors() {
        let (session, messages) = http_response("HTTP/1.1 200 OK\r\nMcp-Session-Id: abc\r\nContent-Type: text/event-stream\r\n\r\nevent: message\ndata: {\"id\":1,\"result\":{}}\n\n").unwrap();
        assert_eq!(session, "abc"); assert_eq!(messages[0]["id"], 1);
        assert!(http_response("HTTP/1.1 403 Forbidden\r\n\r\nno").is_err());
        assert!(http_response("HTTP/1.1 202 Accepted\r\n\r\n").unwrap().1.is_empty());
        assert!(curl_quote("secret\nheader").is_err());
    }
}

fn write_message(w: &mut impl Write, value: &Value, framed: bool) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if framed { write!(w, "Content-Length: {}\r\n\r\n", bytes.len())?; }
    w.write_all(&bytes)?;
    if !framed { w.write_all(b"\n")?; }
    w.flush()
}
fn read_message(r: &mut impl BufRead, framed: bool) -> Result<Option<Value>, String> {
    const MAX: usize = 8*1024*1024;
    if !framed {
        let mut line = String::new();
        let n = r.take((MAX+1) as u64).read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 { return Ok(None); }
        if n > MAX { return Err("Process message exceeded 8 MB".into()); }
        return serde_json::from_str(&line).map(Some).map_err(|e| e.to_string());
    }
    let mut length = None;
    let mut headers = 0;
    loop {
        let mut line = String::new();
        if r.by_ref().take(8193).read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return if headers == 0 { Ok(None) } else { Err("Truncated process headers".into()) };
        }
        headers += line.len();
        if headers > 8192 { return Err("Process headers exceeded 8 KB".into()); }
        if line.trim().is_empty() { break; }
        if let Some((name,value)) = line.split_once(':') && name.eq_ignore_ascii_case("Content-Length") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.filter(|n| *n <= MAX).ok_or("Invalid process Content-Length")?;
    let mut bytes = vec![0;length];
    r.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string())
}
#[cfg(test)]
mod framing_tests {
    use super::*;
    #[test]
    fn framed_and_newline_messages_are_bounded_and_round_trip_unicode() {
        let value = serde_json::json!({"text":"café 🦀"});
        for framed in [true,false] {
            let mut out = Vec::new(); write_message(&mut out,&value,framed).unwrap();
            assert_eq!(read_message(&mut std::io::Cursor::new(out),framed).unwrap(),Some(value.clone()));
        }
        for raw in ["Content-Length: 999999999\r\n\r\n", "Content-Length: 5\r\n\r\n{}", "garbage\n\n"] {
            assert!(read_message(&mut std::io::Cursor::new(raw),true).is_err());
        }
    }
}
