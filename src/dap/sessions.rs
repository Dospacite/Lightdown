//! Multiplex js-debug target connections behind one editor session. IDs in
//! DAP are connection-local. Child IDs are translated before reaching UI.
use super::*;
use std::collections::BTreeMap;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

type Writer = Box<dyn Write + Send>;
type Reader = Box<dyn Read + Send>;

#[derive(Default)]
struct Ids {
    forward: BTreeMap<(usize, i64), i64>,
    back: BTreeMap<i64, (usize, i64)>,
    next: i64,
}
impl Ids {
    fn export(&mut self, session: usize, id: i64) -> i64 {
        if session == 0 || id < 0 {
            return id;
        }
        *self.forward.entry((session, id)).or_insert_with(|| {
            self.next += 1;
            let global = (1_i64 << 40) + self.next;
            self.back.insert(global, (session, id));
            global
        })
    }
    fn import(&self, id: i64) -> (usize, i64) {
        self.back.get(&id).copied().unwrap_or((0, id))
    }
    fn field(&mut self, session: usize, obj: &mut Value, key: &str) {
        if let Some(id) = obj[key].as_i64() {
            if key != "variablesReference" || id != 0 {
                obj[key] = json!(self.export(session, id));
            }
        }
    }
    fn remove(&mut self, session: usize) {
        self.forward.retain(|&(s, _), _| s != session);
        self.back.retain(|_, (s, _)| *s != session);
    }
}
struct Link {
    writer: Writer,
    socket: Option<TcpStream>,
    launch: Option<(String, Value)>,
    started: Instant,
    ready: bool,
    /// Breakpoints and configurationDone were sent, which happens once.
    configured: bool,
}
impl Drop for Link {
    fn drop(&mut self) {
        if let Some(s) = &self.socket {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}
#[derive(Clone)]
enum Reply {
    Editor(u64),
    Internal(String),
    Threads(u64),
}
struct Threads {
    remaining: usize,
    values: Vec<Value>,
}
struct Mux {
    links: BTreeMap<usize, Link>,
    next_link: usize,
    seq: u64,
    pending: BTreeMap<(usize, u64), Reply>,
    threads: BTreeMap<u64, Threads>,
    thread_ids: Ids,
    frame_ids: Ids,
    variable_ids: Ids,
    initialize: Value,
    breakpoints: BTreeMap<String, Value>,
    exceptions: Option<Value>,
    port: u16,
    active: usize,
    incoming: SyncSender<(usize, Value)>,
    output: SyncSender<Value>,
    wake: Waker,
}
fn reader(session: usize, stream: Reader, tx: SyncSender<(usize, Value)>) {
    std::thread::spawn(move || {
        let mut stream = BufReader::new(stream);
        loop {
            let value = match read(&mut stream) {
                Ok(Some(v)) => v,
                result => json!({"clientError":result.err().unwrap_or("Debug adapter exited".into())}),
            };
            let done = value.get("clientError").is_some();
            if tx.send((session, value)).is_err() || done {
                break;
            }
        }
    });
}
impl Mux {
    fn emit(&self, v: Value) {
        let _ = self.output.send(v);
        (self.wake)();
    }
    fn send(&mut self, session: usize, mut v: Value, reply: Option<Reply>) -> Result<(), String> {
        let link = self.links.get_mut(&session).ok_or("Debug target has exited")?;
        self.seq += 1;
        v["seq"] = json!(self.seq);
        if let Some(reply) = reply {
            self.pending.insert((session, self.seq), reply);
        }
        write(&mut link.writer, &v).map_err(|e| e.to_string())
    }
    fn request(&mut self, session: usize, command: &str, arguments: Value, reply: Reply) -> Result<(), String> {
        self.send(session, json!({"type":"request","command":command,"arguments":arguments}), Some(reply))
    }
    fn internal(&mut self, session: usize, command: &str, arguments: Value) -> Result<(), String> {
        self.request(session, command, arguments, Reply::Internal(command.into()))
    }
    fn child(&mut self, parent: usize, request: &Value) -> Result<(), String> {
        let args = &request["arguments"];
        let result = (|| -> Result<(), String> {
            if self.port == 0 {
                return Err("startDebugging needs a TCP adapter".into());
            }
            if self.links.len() >= 64 {
                return Err("Debug target limit reached (64)".into());
            }
            let command = args["request"]
                .as_str()
                .filter(|v| ["launch", "attach"].contains(v))
                .ok_or("Child request must be launch or attach")?;
            if !args["configuration"]["__pendingTargetId"].is_string() {
                return Err("Child configuration has no __pendingTargetId".into());
            }
            let socket = TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], self.port)),
                Duration::from_secs(2),
            )
            .map_err(|e| e.to_string())?;
            socket.set_nodelay(true).ok();
            self.next_link += 1;
            let id = self.next_link;
            reader(id, Box::new(socket.try_clone().map_err(|e| e.to_string())?), self.incoming.clone());
            let mut config = args["configuration"].clone();
            config["request"] = json!(command);
            self.links.insert(
                id,
                Link {
                    writer: Box::new(socket.try_clone().map_err(|e| e.to_string())?),
                    socket: Some(socket),
                    launch: Some((command.into(), config)),
                    started: Instant::now(),
                    ready: false,
                    configured: false,
                },
            );
            self.internal(id, "initialize", self.initialize.clone())?;
            Ok(())
        })();
        self.send(
            parent,
            json!({"type":"response","request_seq":request["seq"],"command":"startDebugging",
            "success":result.is_ok(),"message":result.err()}),
            None,
        )
    }
    fn editor(&mut self, mut v: Value) -> Result<(), String> {
        if v["type"] != "request" {
            return self.send(0, v, None);
        }
        let command = v["command"].as_str().unwrap_or("").to_string();
        let id = v["seq"].as_u64().unwrap_or(0);
        if command == "initialize" {
            v["arguments"]["supportsStartDebuggingRequest"] = json!(self.port != 0);
            self.initialize = v["arguments"].clone();
        }
        if command == "setBreakpoints" {
            if let Some(path) = v["arguments"]["source"]["path"].as_str() {
                self.breakpoints.insert(path.into(), v["arguments"].clone());
            }
        }
        if command == "setExceptionBreakpoints" {
            self.exceptions = Some(v["arguments"].clone());
        }
        if matches!(command.as_str(), "setBreakpoints" | "setExceptionBreakpoints") {
            // Children not yet configured get the current breakpoints when they are.
            let children: Vec<_> =
                self.links.iter().filter(|(i, l)| **i != 0 && l.configured).map(|(&i, _)| i).collect();
            for session in children {
                self.internal(session, &command, v["arguments"].clone())?;
            }
        }
        if command == "threads" {
            let sessions: Vec<_> =
                self.links.iter().filter(|(id, l)| **id == 0 || l.ready).map(|(&id, _)| id).collect();
            self.threads.insert(id, Threads { remaining: sessions.len(), values: Vec::new() });
            for session in sessions {
                self.request(session, "threads", v["arguments"].clone(), Reply::Threads(id))?;
            }
            return Ok(());
        }
        let mut target = None;
        for (key, ids) in
            [("threadId", &self.thread_ids), ("frameId", &self.frame_ids), ("variablesReference", &self.variable_ids)]
        {
            if let Some(raw) = v["arguments"][key].as_i64() {
                let (session, local) = ids.import(raw);
                v["arguments"][key] = json!(local);
                target = Some(session);
            }
        }
        let session = target.unwrap_or(if matches!(command.as_str(), "evaluate" | "pause") { self.active } else { 0 });
        if !self.links.contains_key(&session) {
            self.emit(json!({"type":"response","request_seq":id,"command":command,"success":false,"message":"Debug target has exited"}));
            return Ok(());
        }
        self.send(session, v, Some(Reply::Editor(id)))
    }
    fn translate(&mut self, session: usize, v: &mut Value) {
        self.thread_ids.field(session, &mut v["body"], "threadId");
        if let Some(a) = v["body"]["threads"].as_array_mut() {
            for t in a {
                self.thread_ids.field(session, t, "id");
            }
        }
        if let Some(a) = v["body"]["stackFrames"].as_array_mut() {
            for f in a {
                self.frame_ids.field(session, f, "id");
            }
        }
        if let Some(a) = v["body"]["scopes"].as_array_mut() {
            for s in a {
                self.variable_ids.field(session, s, "variablesReference");
            }
        }
        if let Some(a) = v["body"]["variables"].as_array_mut() {
            for s in a {
                self.variable_ids.field(session, s, "variablesReference");
            }
        }
        self.variable_ids.field(session, &mut v["body"], "variablesReference");
    }
    fn incoming(&mut self, session: usize, mut v: Value) -> Result<(), String> {
        if !self.links.contains_key(&session) {
            return Ok(());
        }
        if v.get("clientError").is_some() {
            if session == 0 {
                self.emit(v);
            } else {
                self.remove(session);
            }
            return Ok(());
        }
        if v["type"] == "request" {
            if v["command"] == "startDebugging" {
                return self.child(session, &v);
            }
            return self.send(session, json!({"type":"response","request_seq":v["seq"],"command":v["command"],"success":false,"message":"Use internalConsole; reverse request is unsupported"}), None);
        }
        if v["event"] == "output" && v["body"]["category"] == "telemetry" {
            return Ok(());
        }
        if v["event"] == "stopped" {
            self.frame_ids.remove(session);
            self.variable_ids.remove(session);
        }
        self.translate(session, &mut v);
        if session != 0 && v["event"] == "initialized" {
            return self.configure(session);
        }
        if v["event"] == "stopped" {
            self.active = session;
        }
        if session != 0 && (v["event"] == "terminated" || v["event"] == "exited") {
            if v["event"] == "terminated" {
                self.remove(session);
            }
            return Ok(());
        }
        if v["type"] == "response" {
            let Some(reply) = v["request_seq"].as_u64().and_then(|id| self.pending.remove(&(session, id))) else {
                return Ok(());
            };
            match reply {
                Reply::Editor(id) => v["request_seq"] = json!(id),
                Reply::Internal(command) => {
                    if v["success"] == false {
                        return Err(format!("Child {command}: {}", v["message"].as_str().unwrap_or("adapter error")));
                    }
                    if command == "initialize" {
                        let (command, config) = self
                            .links
                            .get_mut(&session)
                            .and_then(|l| l.launch.take())
                            .ok_or("Missing child configuration")?;
                        self.internal(session, &command, config)?;
                    }
                    if command == "configurationDone" {
                        if let Some(l) = self.links.get_mut(&session) {
                            l.ready = true;
                        }
                    }
                    return Ok(());
                }
                Reply::Threads(id) => {
                    let values = v["body"]["threads"].as_array().cloned().unwrap_or_default();
                    self.finish_threads(id, values);
                    return Ok(());
                }
            }
        }
        self.emit(v);
        Ok(())
    }
    /// Sends a child its breakpoints and configurationDone on its first
    /// `initialized`. js-debug answers the child's launch only after that, and
    /// then sends `initialized` again; configuring a second time re-sends the
    /// breakpoints while the program already runs, and they bind too late.
    fn configure(&mut self, session: usize) -> Result<(), String> {
        let Some(l) = self.links.get_mut(&session) else { return Ok(()) };
        if l.configured {
            return Ok(());
        }
        l.configured = true;
        let points: Vec<_> = self.breakpoints.values().cloned().collect();
        for args in points {
            self.internal(session, "setBreakpoints", args)?;
        }
        if let Some(args) = self.exceptions.clone() {
            self.internal(session, "setExceptionBreakpoints", args)?;
        }
        self.internal(session, "configurationDone", json!({}))
    }
    fn finish_threads(&mut self, id: u64, values: Vec<Value>) {
        if let Some(group) = self.threads.get_mut(&id) {
            group.remaining = group.remaining.saturating_sub(1);
            group.values.extend(values);
            if group.remaining == 0 {
                let group = self.threads.remove(&id).unwrap();
                self.emit(json!({"type":"response","request_seq":id,"command":"threads","success":true,"body":{"threads":group.values}}));
            }
        }
    }
    fn remove(&mut self, session: usize) {
        self.links.remove(&session);
        let outstanding: Vec<_> = self.pending.keys().filter(|(s, _)| *s == session).copied().collect();
        for key in outstanding {
            if let Some(Reply::Threads(id)) = self.pending.remove(&key) {
                self.finish_threads(id, Vec::new());
            }
        }
        // Keep exported IDs as tombstones: a late UI request must fail rather
        // than accidentally reaching a different target with the same local ID.
        if self.active == session {
            self.active = 0;
        }
    }
}

pub(super) fn run(
    writer: Writer,
    stream: Reader,
    socket: Option<TcpStream>,
    port: u16,
    input: Receiver<Value>,
    output: SyncSender<Value>,
    wake: Waker,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(32);
    reader(0, stream, tx.clone());
    let mut mux = Mux {
        links: BTreeMap::new(),
        next_link: 0,
        seq: 0,
        pending: BTreeMap::new(),
        threads: BTreeMap::new(),
        thread_ids: Ids::default(),
        frame_ids: Ids::default(),
        variable_ids: Ids::default(),
        initialize: Value::Null,
        breakpoints: BTreeMap::new(),
        exceptions: None,
        port,
        active: 0,
        incoming: tx,
        output,
        wake,
    };
    mux.links.insert(0, Link { writer, socket, launch: None, started: Instant::now(), ready: true, configured: true });
    while !cancel.load(Ordering::SeqCst) {
        // Bounded batches keep UI commands responsive under heavy output.
        for _ in 0..32 {
            match input.try_recv() {
                Ok(v) => mux.editor(v)?,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok((id, v)) => mux.incoming(id, v)?,
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if mux.links.iter().any(|(&id, l)| id != 0 && !l.ready && l.started.elapsed().as_secs() > 20) {
            return Err("Child DAP initialization timed out".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_local_ids_do_not_collide() {
        let mut ids = Ids::default();
        assert_eq!(ids.export(0, 7), 7);
        assert_ne!(ids.export(1, 0), 0);
        let a = ids.export(1, 7);
        let b = ids.export(2, 7);
        assert_ne!(a, b);
        assert_ne!(a, 7);
        assert_eq!(ids.import(a), (1, 7));
        assert_eq!(ids.import(b), (2, 7));
        assert_eq!(ids.export(1, 7), a);
        ids.remove(1);
        assert_eq!(ids.import(b), (2, 7));
    }
}
