//! Local server addresses seen in terminal and job output. Output threads
//! feed whole lines through `detect`; the window takes what was found.
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct Scan {
    line: Vec<u8>,
    found: Vec<(String, u16)>,
}

fn registry() -> &'static Mutex<BTreeMap<u64, Scan>> {
    static SCANS: OnceLock<Mutex<BTreeMap<u64, Scan>>> = OnceLock::new();
    SCANS.get_or_init(Default::default)
}

/// Takes output from terminal `id`, and scans each completed line.
pub fn feed(id: u64, bytes: &[u8]) {
    let mut all = registry().lock().unwrap();
    if !all.contains_key(&id) && all.len() >= 256 {
        let oldest = *all.keys().next().unwrap();
        all.remove(&oldest);
    }
    let scan = all.entry(id).or_default();
    for &b in bytes {
        if b == b'\n' || b == b'\r' {
            if !scan.line.is_empty() {
                if let Some(found) = detect(&String::from_utf8_lossy(&scan.line))
                    && !scan.found.contains(&found)
                    && scan.found.len() < 32
                {
                    scan.found.push(found);
                }
                scan.line.clear();
            }
        } else if scan.line.len() < 1024 {
            scan.line.push(b);
        }
    }
}

/// What terminal `id` has shown since the last call.
pub fn take(id: u64) -> Vec<(String, u16)> {
    registry().lock().unwrap().get_mut(&id).map(|s| std::mem::take(&mut s.found)).unwrap_or_default()
}

/// Forgets a terminal that has closed.
pub fn forget(id: u64) {
    registry().lock().unwrap().remove(&id);
}

fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

fn port(digits: &str) -> Option<u16> {
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u16>().ok().filter(|p| *p > 0 && digits.len() <= 5)
}

/// A local address on one line of output: an address to open and its port.
/// Wildcard hosts are shown as `localhost`.
pub fn detect(line: &str) -> Option<(String, u16)> {
    let line = plain(line);
    const HOSTS: [&str; 7] = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]", "[0:0:0:0:0:0:0:0]", "::"];
    for (i, _) in line.match_indices(':') {
        let before = &line[..i];
        let Some(host) = HOSTS.iter().find(|h| before.ends_with(*h)) else { continue };
        let start = i - host.len();
        // The host must be a whole word: `xlocalhost:1` is not ours.
        if line[..start].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            if !line[..start].ends_with("://") {
                continue;
            }
        }
        let Some(port) = port(&line[i + 1..]) else { continue };
        let scheme = if line[..start].ends_with("https://") { "https" } else { "http" };
        let rest = &line[i + 1 + port.to_string().len()..];
        let path: String = if rest.starts_with('/') {
            rest.chars().take_while(|c| !c.is_whitespace() && !"\"'<>)]".contains(*c)).collect()
        } else {
            String::new()
        };
        let path = path.trim_end_matches(['.', ',', ';']);
        let shown = if matches!(*host, "localhost" | "127.0.0.1" | "[::1]") { *host } else { "localhost" };
        return Some((format!("{scheme}://{shown}:{port}{path}"), port));
    }
    let lower = line.to_ascii_lowercase();
    if lower.contains("listen") || lower.contains("serving") || lower.contains("running") {
        for word in ["port ", "port: ", "port="] {
            if let Some(at) = lower.find(word)
                && let Some(port) = port(&lower[at + word.len()..])
            {
                return Some((format!("http://localhost:{port}"), port));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_common_server_announcements() {
        let cases = [
            ("  ➜  Local:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m", "http://localhost:5173/", 5173),
            (" * Running on http://127.0.0.1:5000", "http://127.0.0.1:5000", 5000),
            ("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ...", "http://localhost:8000/", 8000),
            ("Listening on 0.0.0.0:3000.", "http://localhost:3000", 3000),
            ("server listening on port 8080", "http://localhost:8080", 8080),
            ("Uvicorn running on https://[::]:8443/docs (Press CTRL+C)", "https://localhost:8443/docs", 8443),
        ];
        for (line, url, port) in cases {
            assert_eq!(detect(line), Some((url.to_string(), port)), "{line}");
        }
        for line in ["error: src/main.rs:12:5", "connecting to db.example.com:5432", "ratio 0.0.0.0:0", "port 99999 listen"] {
            assert_eq!(detect(line), None, "{line}");
        }
    }
    #[test]
    fn lines_split_across_reads_are_joined() {
        let id = u64::MAX - 7;
        feed(id, b"Local: http://local");
        assert!(take(id).is_empty());
        feed(id, b"host:4321/\r\nagain http://localhost:4321/\n");
        assert_eq!(take(id), vec![("http://localhost:4321/".to_string(), 4321)]);
        forget(id);
    }
}
