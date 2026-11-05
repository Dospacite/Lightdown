//! Project stdio MCP tools. Connection is explicit; every call uses the run policy.
use crate::{
    agent::{Protocol, Request},
    kernel::Waker,
    rpc::{Process, Http},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub struct Tool {
    pub alias: String,
    pub name: String,
    pub description: String,
    pub schema: Value,
    server: usize,
}
enum Transport { Stdio(Process), Http(Http) }
impl Transport {
    fn send(&self, v: Value) { match self { Self::Stdio(p) => p.send(v), Self::Http(p) => p.send(v) } }
    fn poll(&self) -> Vec<Result<Value, String>> { match self { Self::Stdio(p) => p.poll(), Self::Http(p) => p.poll() } }
}
struct Server {
    process: Option<Transport>,
    ready: bool,
    deadline: Option<Instant>,
    pages: usize,
}
#[derive(Default)]
pub struct Servers {
    pub root: PathBuf,
    pub tools: Vec<Tool>,
    servers: Vec<Server>,
    next_id: u64,
    pending: Option<(usize, u64, Instant)>,
}
pub enum Event {
    Status(String),
    Result(String, bool),
}
impl Servers {
    pub fn connect(root: &Path, wake: Waker) -> Result<Self, String> { Self::connect_mode(root, wake, false) }
    pub fn connect_auto(root: &Path, wake: Waker) -> Result<Self, String> { Self::connect_mode(root, wake, true) }
    fn connect_mode(root: &Path, wake: Waker, auto: bool) -> Result<Self, String> {
        let text = std::fs::read_to_string(root.join(".lightdown/mcp.json"))
            .map_err(|e| format!("Read .lightdown/mcp.json: {e}"))?;
        let cfg: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let configs = cfg["mcpServers"].as_object().ok_or("Expected a mcpServers object")?;
        if configs.len() > 16 {
            return Err("At most 16 MCP servers are supported".into());
        }
        let mut out = Self { root: root.to_path_buf(), ..Self::default() };
        for (name, v) in configs {
            if v["disabled"] == true || auto && v["autoConnect"] != true {
                continue;
            }
            let process = if let Some(url) = v["url"].as_str() {
                Transport::Http(Http::start(url, &v["headers"], wake.clone()).map_err(|e| format!("{name}: {e}"))?)
            } else {
                let exe = v["command"].as_str().ok_or_else(|| format!("{name}: expected command or url"))?;
                let mut cmd = Command::new(exe);
                for arg in v["args"].as_array().into_iter().flatten() { cmd.arg(arg.as_str().ok_or("MCP args must be strings")?); }
                cmd.current_dir(root);
                crate::environment::apply(&mut cmd, root, None);
                for (k, v) in v["env"].as_object().into_iter().flatten() { cmd.env(k, v.as_str().ok_or("MCP env values must be strings")?); }
                Transport::Stdio(Process::start(cmd, wake.clone()).map_err(|e| format!("{name}: {e}"))?)
            };
            process.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"lightdown","version":env!("CARGO_PKG_VERSION")}}}));
            out.servers.push(Server {
                process: Some(process),
                ready: false,
                deadline: Some(Instant::now() + Duration::from_secs(20)),
                pages: 0,
            });
        }
        Ok(out)
    }
    pub fn due(&self) -> Option<Instant> {
        self.servers.iter().filter_map(|s| s.deadline).chain(self.pending.map(|p| p.2)).min()
    }
    pub fn call(&mut self, alias: &str, args: Value) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("An MCP call is already running".into());
        }
        let tool = self.tools.iter().find(|t| t.alias == alias).ok_or("MCP tool is no longer connected")?;
        self.next_id += 1;
        let id = self.next_id + 100;
        self.servers[tool.server]
            .process
            .as_ref()
            .ok_or("MCP server disconnected")?
            .send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool.name,"arguments":args}}));
        self.pending = Some((tool.server, id, Instant::now() + Duration::from_secs(60)));
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        for (i, s) in self.servers.iter_mut().enumerate() {
            let timed_out = s.deadline.is_some_and(|d| Instant::now() >= d)
                || self.pending.is_some_and(|p| p.0 == i && Instant::now() >= p.2);
            let mut events = s.process.as_ref().map_or(Vec::new(), Transport::poll);
            if timed_out {
                events.push(Err("MCP server timed out. Disconnect and reconnect to retry.".into()));
            }
            for event in events {
                let v = match event {
                    Ok(v) => v,
                    Err(e) => {
                        s.process = None;
                        s.ready = false;
                        s.deadline = None;
                        self.tools.retain(|t| t.server != i);
                        if self.pending.is_some_and(|p| p.0 == i) {
                            self.pending = None;
                            out.push(Event::Result(e.clone(), false));
                        }
                        out.push(Event::Status(e));
                        continue;
                    }
                };
                if s.process.is_none() {
                    continue;
                }
                if v["method"] == "notifications/tools/list_changed" && s.ready {
                    self.tools.retain(|t| t.server != i);
                    s.ready = false; s.pages = 0;
                    s.deadline = Some(Instant::now() + Duration::from_secs(20));
                    s.process.as_ref().unwrap().send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}));
                }
                if v.get("method").is_some() {
                    if v.get("id").is_some() {
                        s.process.as_ref().unwrap().send(json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"Client requests are unsupported"}}));
                    }
                    continue;
                }
                if v["id"] == 0 {
                    if !v["error"].is_null()
                        || !["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
                            .contains(&v["result"]["protocolVersion"].as_str().unwrap_or(""))
                    {
                        s.process = None;
                        s.deadline = None;
                        out.push(Event::Status("MCP initialization failed or returned an unsupported version".into()));
                        continue;
                    }
                    s.process.as_ref().unwrap().send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
                    s.process.as_ref().unwrap().send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}));
                } else if v["id"] == 1 {
                    if !v["error"].is_null() {
                        s.deadline = None;
                        out.push(Event::Status(format!("MCP tools/list: {}", v["error"])));
                        continue;
                    }
                    for t in v["result"]["tools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(256usize.saturating_sub(self.tools.len()))
                    {
                        let Some(name) = t["name"].as_str() else { continue };
                        if !t["inputSchema"].is_object() {
                            continue;
                        }
                        let label: String = name
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                            .take(40)
                            .collect();
                        let alias = format!("mcp_{i}_{}_{label}", self.tools.len());
                        self.tools.push(Tool {
                            alias,
                            name: name.into(),
                            description: t["description"].as_str().unwrap_or(name).into(),
                            schema: t["inputSchema"].clone(),
                            server: i,
                        });
                    }
                    s.pages += 1;
                    if let Some(cursor) =
                        v["result"]["nextCursor"].as_str().filter(|_| s.pages < 16 && self.tools.len() < 256)
                    {
                        s.process
                            .as_ref()
                            .unwrap()
                            .send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"cursor":cursor}}));
                    } else {
                        s.ready = true;
                        s.deadline = None;
                        out.push(Event::Status(format!("MCP connected: {} tools", self.tools.len())));
                    }
                } else if self.pending.is_some_and(|p| p.0 == i && v["id"] == p.1) {
                    self.pending = None;
                    let ok = v["error"].is_null() && v["result"]["isError"] != true;
                    let result = if v["error"].is_null() { &v["result"] } else { &v["error"] };
                    out.push(Event::Result(crate::nb::clip(&result.to_string(), 60_000), ok));
                }
            }
        }
        out
    }
    pub fn refresh(&mut self) {
        for (i, server) in self.servers.iter_mut().enumerate() {
            if !server.ready { continue; }
            self.tools.retain(|t| t.server != i);
            server.ready = false; server.pages = 0;
            server.deadline = Some(Instant::now() + Duration::from_secs(20));
            if let Some(p) = &server.process { p.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})); }
        }
    }
    pub fn add_tools(&self, req: &mut Request) {
        let list = req.body["tools"].as_array_mut().unwrap();
        for t in &self.tools {
            if !self.servers[t.server].ready {
                continue;
            }
            list.push(match req.protocol {
                Protocol::Chat => json!({"type":"function","function":{"name":t.alias,"description":t.description,"parameters":t.schema}}),
                Protocol::Messages => json!({"name":t.alias,"description":t.description,"input_schema":t.schema}),
                Protocol::Responses => json!({"type":"function","name":t.alias,"description":t.description,"parameters":t.schema}),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn http_sessions_sse_tools_calls_refresh_and_explicit_auto_connect() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for n in 0..5 {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                let head_end = loop { let mut b = [0]; socket.read_exact(&mut b).unwrap(); bytes.push(b[0]); if bytes.ends_with(b"\r\n\r\n") { break bytes.len(); } };
                let head = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
                let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length: ")).unwrap().parse().unwrap();
                bytes.resize(head_end + length, 0); socket.read_exact(&mut bytes[head_end..]).unwrap();
                let v: Value = serde_json::from_slice(&bytes[head_end..]).unwrap();
                if n > 0 { assert!(head.contains("mcp-session-id: fixture-session")); }
                let result = match v["method"].as_str().unwrap() {
                    "initialize" => json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}}}),
                    "notifications/initialized" => { socket.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(); continue; },
                    "tools/list" => json!({"tools":[{"name":"http_echo","inputSchema":{"type":"object"}}]}),
                    "tools/call" => json!({"content":[{"type":"text","text":"http works"}]}),
                    _ => panic!("Unexpected method"),
                };
                let body = json!({"jsonrpc":"2.0","id":v["id"],"result":result}).to_string();
                let body = if n == 2 { format!("event: message\ndata: {body}\n\n") } else { body };
                let response = format!("HTTP/1.1 200 OK\r\nMcp-Session-Id: fixture-session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).unwrap();
            }
        });
        let root = std::env::temp_dir().join(format!("ld-mcp-http-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join(".lightdown/mcp.json"), json!({"mcpServers":{"fixture":{"url":endpoint}}}).to_string()).unwrap();
        assert!(Servers::connect_auto(&root, Arc::new(|| {})).unwrap().servers.is_empty());
        let mut s = Servers::connect(&root, Arc::new(|| {})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.tools.is_empty() && Instant::now() < deadline { s.poll(); std::thread::sleep(Duration::from_millis(5)); }
        assert_eq!(s.tools.len(), 1);
        s.call(&s.tools[0].alias.clone(), json!({})).unwrap();
        let mut ok = false;
        while !ok && Instant::now() < deadline { ok = s.poll().iter().any(|e| matches!(e, Event::Result(t, true) if t.contains("http works"))); std::thread::sleep(Duration::from_millis(5)); }
        assert!(ok);
        s.refresh();
        while s.tools.is_empty() && Instant::now() < deadline { s.poll(); std::thread::sleep(Duration::from_millis(5)); }
        assert_eq!(s.tools.len(), 1);
        server.join().unwrap(); drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_stdio_discovery_pagination_calls_errors_and_timeouts() {
        let root = std::env::temp_dir().join(format!("ld-mcp-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let script = r#"import json, sys
for line in sys.stdin:
    r=json.loads(line)
    m=r.get('method')
    if m=='initialize': result={'protocolVersion':'2025-06-18','capabilities':{'tools':{}}}
    elif m=='tools/list':
        if r['params'].get('cursor'): result={'tools':[{'name':'fail','inputSchema':{'type':'object'}}]}
        else: result={'tools':[{'name':'echo','description':'Echo input','inputSchema':{'type':'object'}}],'nextCursor':'page2'}
    elif m=='tools/call': result={'content':[{'type':'text','text':r['params']['arguments'].get('text','')}],'isError':r['params']['name']=='fail'}
    else: continue
    print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#;
        std::fs::write(
            root.join(".lightdown/mcp.json"),
            json!({"mcpServers":{"fixture":{"command":"python3","args":["-u","-c",script]}}}).to_string(),
        )
        .unwrap();
        let mut s = Servers::connect(&root, Arc::new(|| {})).unwrap();
        let start = Instant::now();
        while s.servers.iter().any(|s| !s.ready) && start.elapsed().as_secs() < 5 {
            s.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(s.tools.len(), 2);
        let mut req = Request {
            url: String::new(),
            headers: Vec::new(),
            protocol: Protocol::Messages,
            body: json!({"tools":[]}),
        };
        s.add_tools(&mut req);
        assert_eq!(req.body["tools"].as_array().unwrap().len(), 2);
        for (index, expected) in [(0, true), (1, false)] {
            s.call(&s.tools[index].alias.clone(), json!({"text":"hello"})).unwrap();
            let start = Instant::now();
            let mut result = None;
            while result.is_none() && start.elapsed().as_secs() < 5 {
                for e in s.poll() {
                    if let Event::Result(text, ok) = e {
                        result = Some((text, ok));
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let (text, ok) = result.unwrap();
            assert_eq!(ok, expected);
            assert!(text.contains("hello"));
        }
        s.pending = Some((0, 999, Instant::now()));
        assert!(s.poll().iter().any(|e| matches!(e, Event::Result(_, false))));
        assert!(s.tools.is_empty());
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
}
