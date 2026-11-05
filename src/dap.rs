//! Bounded DAP framing over adapter stdio or a loopback TCP connection.
mod inspector;
mod sessions;
pub use inspector::open_node_inspector;
use crate::kernel::Waker;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc::{self, Sender, Receiver}};
fn read(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut length = None;
    for _ in 0..64 {
        let mut header = Vec::new();
        if reader.by_ref().take(1025).read_until(b'\n', &mut header).map_err(|e| e.to_string())? == 0 { return Ok(None); }
        if header.len() > 1024 { return Err("DAP header exceeds 1 KB".into()); }
        let header = String::from_utf8(header).map_err(|e| e.to_string())?;
        if header.trim().is_empty() {
            let n = length.filter(|n| *n <= 8 << 20).ok_or("Missing or oversized DAP Content-Length")?;
            let mut bytes = vec![0; n]; reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
            trace("<-", &bytes);
            return serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string());
        }
        if let Some((k,v)) = header.split_once(':') && k.eq_ignore_ascii_case("Content-Length") { length = v.trim().parse::<usize>().ok(); }
    }
    Err("Too many DAP headers".into())
}
/// Appends each DAP message to the file `LIGHTDOWN_DAP_TRACE` names, if any:
/// `->` for what Lightdown sends, `<-` for what an adapter sends.
fn trace(direction: &str, bytes: &[u8]) {
    let Ok(path) = std::env::var("LIGHTDOWN_DAP_TRACE") else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{direction} {}", String::from_utf8_lossy(bytes));
    }
}
fn write(writer: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    trace("->", &bytes);
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?; writer.flush()
}
pub struct Client { tx: Sender<Value>, rx: Receiver<Value>, child: Arc<Mutex<Option<Child>>>, stopped: Arc<std::sync::atomic::AtomicBool> }
impl Client {
    pub fn start(argv: Vec<String>, tcp: bool, root: PathBuf, wake: Waker) -> Self {
        let (tx, input) = mpsc::channel(); let (output, rx) = mpsc::sync_channel(16);
        let child = Arc::new(Mutex::new(None::<Child>)); let owned = child.clone();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false)); let cancel = stopped.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                use std::os::unix::process::CommandExt;
                let port = if tcp { std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?.local_addr().map_err(|e| e.to_string())?.port() } else { 0 };
                let argv: Vec<_> = argv.iter().map(|s| s.replace("{port}", &port.to_string())).collect();
                let mut cmd = Command::new(argv.first().ok_or("Empty adapter command")?);
                cmd.args(&argv[1..]).current_dir(&root).process_group(0)
                    .stdin(if tcp { Stdio::null() } else { Stdio::piped() }).stdout(if tcp { Stdio::null() } else { Stdio::piped() }).stderr(Stdio::null());
                crate::environment::apply(&mut cmd, &root, None);
                let mut process = cmd.spawn().map_err(|e| e.to_string())?;
                let stdin = process.stdin.take(); let stdout = process.stdout.take();
                *owned.lock().unwrap() = Some(process);
                let mut shutdown = None;
                let (writer, reader): (Box<dyn Write + Send>, Box<dyn Read + Send>) = if tcp {
                    let start = std::time::Instant::now();
                    let socket = loop {
                        if cancel.load(std::sync::atomic::Ordering::SeqCst) { return Err("DAP stopped".into()); }
                        match std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)) {
                            Ok(s) => break s,
                            Err(_) if start.elapsed().as_secs() < 10 => std::thread::sleep(std::time::Duration::from_millis(20)),
                            Err(e) => return Err(format!("Cannot connect to adapter: {e}")),
                        }
                    };
                    socket.set_nodelay(true).ok();
                    shutdown = Some(socket.try_clone().map_err(|e| e.to_string())?);
                    (Box::new(socket.try_clone().map_err(|e| e.to_string())?), Box::new(socket))
                } else { (Box::new(stdin.unwrap()), Box::new(stdout.unwrap())) };
                sessions::run(writer, reader, shutdown, port, input, output.clone(), wake.clone(), cancel.clone())?;
                Ok(())
            })();
            if let Some(mut process) = owned.lock().unwrap().take() {
                unsafe { libc::kill(-(process.id() as i32), libc::SIGKILL); }
                let _ = process.kill(); let _ = process.wait();
            }
            if let Err(e) = result { let _ = output.send(json!({"clientError":e})); wake(); }
        });
        Self { tx, rx, child, stopped }
    }
    pub fn send(&self, value: Value) { let _ = self.tx.send(value); }
    pub fn poll(&self) -> Vec<Value> { self.rx.try_iter().collect() }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(p) = self.child.lock().unwrap().as_mut() {
            unsafe { libc::kill(-(p.id() as i32), libc::SIGKILL); } let _ = p.kill();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_handles_unicode_multiple_frames_and_limits() {
        let mut out = Vec::new(); write(&mut out, &json!({"text":"κόσμε"})).unwrap(); write(&mut out, &json!({"seq":2})).unwrap();
        let mut input = std::io::Cursor::new(out); assert_eq!(read(&mut input).unwrap().unwrap()["text"], "κόσμε"); assert_eq!(read(&mut input).unwrap().unwrap()["seq"], 2);
        assert!(read(&mut std::io::Cursor::new(b"Content-Length: 99999999\r\n\r\n")).is_err());
        assert!(read(&mut std::io::Cursor::new(b"Content-Length: 4\r\n\r\n{")).is_err());
    }
}
