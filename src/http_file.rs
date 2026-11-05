//! `.http` request files, as REST clients write them: requests split by
//! `###` lines, each a request line, headers, a blank line and a body. They
//! are sent through curl, which writes the response to private files.
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// The largest body shown in the response tab.
const SHOWN: usize = 1 << 20;
const METHODS: [&str; 8] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE"];

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// The request line, counted from 1.
    pub line: usize,
}

pub fn is_http(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("http") || e.eq_ignore_ascii_case("rest"))
}

/// Splits a request line at spaces outside `{{ }}`.
fn words(line: &str) -> Vec<&str> {
    let (mut out, mut start, mut depth) = (Vec::new(), None, 0usize);
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"{{") {
            depth += 1;
            start.get_or_insert(i);
            i += 2;
            continue;
        }
        if b[i..].starts_with(b"}}") && depth > 0 {
            depth -= 1;
            i += 2;
            continue;
        }
        if b[i].is_ascii_whitespace() && depth == 0 {
            if let Some(s) = start.take() {
                out.push(&line[s..i]);
            }
        } else {
            start.get_or_insert(i);
        }
        i += 1;
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out
}

fn comment(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with('#') && !t.starts_with("###")) || t.starts_with("//")
}

/// `{{name}}` takes a file variable; `{{$env NAME}}` and `{{$dotenv NAME}}`
/// take a value from the project's `.env` or the environment.
fn substitute(text: &str, vars: &BTreeMap<String, String>, env: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let end = rest[i..].find("}}").ok_or("A {{ has no closing }}")?;
        let name = rest[i + 2..i + end].trim();
        let value = match name.split_once(char::is_whitespace) {
            Some(("$env" | "$dotenv", key)) => env(key.trim()),
            _ => vars.get(name).cloned(),
        };
        out.push_str(&value.ok_or_else(|| format!("Unknown variable {{{{{name}}}}}"))?);
        rest = &rest[i + end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The request in the block that holds byte `at`, with its variables
/// substituted. Variables defined above it, in any block, apply.
pub fn request_at(text: &str, at: usize, env: &dyn Fn(&str) -> Option<String>) -> Result<Request, String> {
    let mut vars = BTreeMap::new();
    let mut lines = Vec::new();
    let mut offset = 0;
    for (n, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        lines.push((n + 1, offset, line));
        offset += line.len() + 1;
    }
    let mut blocks: Vec<Vec<(usize, usize, &str)>> = vec![Vec::new()];
    // A separator line belongs to the request after it.
    let mut target = 0;
    for l in lines {
        if l.2.trim_start().starts_with("###") {
            blocks.push(Vec::new());
            if l.1 <= at {
                target += 1;
            }
            continue;
        }
        blocks.last_mut().unwrap().push(l);
    }
    for (index, block) in blocks.iter().enumerate() {
        let mut i = 0;
        // The preamble: blank lines, comments and variables.
        while let Some(&(_, _, line)) = block.get(i) {
            let t = line.trim();
            if t.is_empty() || comment(line) {
                i += 1;
            } else if let Some(def) = t.strip_prefix('@') {
                let (name, value) = def.split_once('=').ok_or_else(|| format!("Line {}: write a variable as @name = value", block[i].0))?;
                let name = name.trim();
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || "_-.".contains(c)) {
                    return Err(format!("Line {}: a variable name takes letters, digits, _, - and .", block[i].0));
                }
                vars.insert(name.to_string(), substitute(value.trim(), &vars, env)?);
                i += 1;
            } else {
                break;
            }
        }
        if index != target {
            continue;
        }
        let Some(&(line_no, _, request_line)) = block.get(i) else {
            return Err("There is no request here. Write one such as GET https://example.com".into());
        };
        let mut words = words(request_line).into_iter();
        let first = words.next().unwrap_or("");
        let (method, mut url) = if METHODS.contains(&first) {
            (first.to_string(), words.next().ok_or_else(|| format!("Line {line_no}: the request has no URL"))?.to_string())
        } else {
            ("GET".to_string(), first.to_string())
        };
        if words.next().is_some_and(|w| !w.starts_with("HTTP/")) {
            return Err(format!("Line {line_no}: expected METHOD URL, optionally followed by HTTP/1.1"));
        }
        i += 1;
        while let Some(&(_, _, line)) = block.get(i) {
            let t = line.trim();
            if !(t.starts_with('?') || t.starts_with('&')) {
                break;
            }
            url.push_str(t);
            i += 1;
        }
        let mut headers = Vec::new();
        while let Some(&(n, _, line)) = block.get(i) {
            i += 1;
            if line.trim().is_empty() {
                break;
            }
            if comment(line) {
                continue;
            }
            let (name, value) = line.split_once(':').ok_or_else(|| format!("Line {n}: expected a header, Name: value"))?;
            let name = substitute(name.trim(), &vars, env)?;
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {
                return Err(format!("Line {n}: {name:?} is not a header name"));
            }
            headers.push((name, substitute(value.trim(), &vars, env)?));
        }
        let rest: Vec<&str> = block[i.min(block.len())..].iter().map(|l| l.2).collect();
        let body = rest.join("\n").trim_end().to_string();
        let url = substitute(&url, &vars, env)?;
        let scheme = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
        if scheme.is_none_or(str::is_empty) || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!("Line {line_no}: the URL must start with http:// or https://"));
        }
        if headers.iter().any(|(_, v)| v.chars().any(|c| c == '\r' || c == '\n')) {
            return Err("A header value holds a line break".into());
        }
        return Ok(Request {
            method,
            url,
            headers,
            body: (!body.is_empty()).then(|| substitute(&body, &vars, env)).transpose()?,
            line: line_no,
        });
    }
    Err("There is no request here".into())
}

/// The curl command for a request. `dir` is a private folder that receives
/// the body sent, the headers and the body received.
pub fn command(req: &Request, dir: &Path) -> std::io::Result<Command> {
    let mut cmd = Command::new("curl");
    cmd.args(["--silent", "--show-error", "--globoff", "--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--location", "--max-redirs", "5", "--connect-timeout", "10", "--max-time", "60"])
        .args(["--max-filesize", "50M", "--noproxy", "localhost,127.0.0.1,::1"])
        .arg("--dump-header")
        .arg(dir.join("headers"))
        .arg("--output")
        .arg(dir.join("body"))
        .args(["--write-out", "%{json}\n"]);
    if req.method == "HEAD" {
        cmd.arg("--head");
    } else {
        cmd.args(["--request", &req.method]);
    }
    for (name, value) in &req.headers {
        cmd.arg("--header").arg(if value.is_empty() { format!("{name};") } else { format!("{name}: {value}") });
    }
    if let Some(body) = &req.body {
        crate::config::write_private(&dir.join("request"), body.as_bytes())?;
        cmd.arg("--data-binary").arg(format!("@{}", dir.join("request").display()));
    }
    cmd.arg("--").arg(&req.url);
    Ok(cmd)
}

fn size(n: u64) -> String {
    if n < 1024 { format!("{n} B") } else if n < 1 << 20 { format!("{:.1} KB", n as f64 / 1024.0) } else { format!("{:.1} MB", n as f64 / 1048576.0) }
}

/// The text of the response tab, from what curl printed and the files it
/// wrote. Also returns a one-line summary for the status bar.
pub fn report(req: &Request, printed: &str, dir: &Path) -> (String, String) {
    let info: Option<Value> = printed.lines().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).find(|v| v.get("http_code").is_some());
    let errors: Vec<&str> = printed.lines().filter(|l| !l.trim_start().starts_with('{') && !l.trim().is_empty()).collect();
    let code = info.as_ref().and_then(|v| v["http_code"].as_u64()).unwrap_or(0);
    let mut out = format!("{} {}\n", req.method, req.url);
    if code == 0 {
        let why = if errors.is_empty() { "curl gave no response".to_string() } else { errors.join("\n") };
        out += &format!("\nNo response: {why}\n");
        return (out, format!("HTTP request failed: {}", errors.first().copied().unwrap_or("no response")));
    }
    let v = info.unwrap();
    let ms = (v["time_total"].as_f64().unwrap_or(0.0) * 1000.0).round() as u64;
    let downloaded = v["size_download"].as_u64().unwrap_or(0);
    // After redirects, the headers file holds each response; show the last.
    let headers = std::fs::read(dir.join("headers")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let last = headers.replace("\r\n", "\n").split("\n\n").filter(|b| !b.trim().is_empty()).last().unwrap_or("").to_string();
    let status = last.lines().next().unwrap_or("").trim().to_string();
    let redirects = v["num_redirects"].as_u64().unwrap_or(0);
    out += &format!("{}\n{ms} ms · {}", if status.is_empty() { format!("HTTP {code}") } else { status }, size(downloaded));
    if redirects > 0 {
        out += &format!(" · {redirects} redirect{} to {}", if redirects == 1 { "" } else { "s" }, v["url_effective"].as_str().unwrap_or(""));
    }
    out += "\n\n";
    for line in last.lines().skip(1) {
        out += line.trim_end();
        out.push('\n');
    }
    let body = std::fs::read(dir.join("body")).unwrap_or_default();
    if !body.is_empty() {
        out.push('\n');
        let json = v["content_type"].as_str().is_some_and(|t| t.contains("json"));
        match std::str::from_utf8(&body[..body.len().min(SHOWN)]) {
            Ok(text) if json && body.len() <= SHOWN => {
                out += &serde_json::from_str::<Value>(text).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or_else(|| text.into());
            }
            Ok(text) => out += text,
            Err(e) if e.valid_up_to() + 4 >= SHOWN.min(body.len()) => out += std::str::from_utf8(&body[..e.valid_up_to()]).unwrap_or(""),
            Err(_) => out += &format!("[{} of binary data]", size(body.len() as u64)),
        }
        if body.len() > SHOWN {
            out += &format!("\n\n[Showing the first {} of {}]", size(SHOWN as u64), size(body.len() as u64));
        }
        out.push('\n');
    }
    (out, format!("HTTP {code} in {ms} ms"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn requests_split_on_separators_with_variables_headers_and_bodies() {
        let text = "@host = https://api.example.com\n@token = abc\n\n### List\nGET {{host}}/items\n  ?page=2\n  &size=10\nAccept: application/json\n# a comment\n\n###\n// Create\nPOST {{host}}/items HTTP/1.1\nAuthorization: Bearer {{token}}\nContent-Type: application/json\n\n{\"name\": \"{{token}}\"}\n\n\n### Plain\nhttps://example.com/{{$env USER_ID}}\n";
        let get = request_at(text, text.find("GET").unwrap(), &none).unwrap();
        assert_eq!((get.method.as_str(), get.url.as_str(), get.line), ("GET", "https://api.example.com/items?page=2&size=10", 5));
        assert_eq!(get.headers, vec![("Accept".into(), "application/json".into())]);
        assert_eq!(get.body, None);
        let post = request_at(text, text.find("Content-Type").unwrap(), &none).unwrap();
        assert_eq!(post.method, "POST");
        assert_eq!(post.headers[0], ("Authorization".into(), "Bearer abc".into()));
        assert_eq!(post.body.as_deref(), Some("{\"name\": \"abc\"}"));
        // The separator line belongs to the request after it.
        assert_eq!(request_at(text, text.find("###\n//").unwrap() + 1, &none).unwrap().method, "POST");
        let env = |k: &str| (k == "USER_ID").then(|| "42".to_string());
        assert_eq!(request_at(text, text.len() - 2, &env).unwrap().url, "https://example.com/42");
        assert!(request_at(text, text.len() - 2, &none).unwrap_err().contains("$env USER_ID"));
    }

    #[test]
    fn bad_requests_are_refused_before_curl_runs() {
        assert!(request_at("GET file:///etc/passwd\n", 0, &none).unwrap_err().contains("http://"));
        assert!(request_at("GET https://a.example/{{missing}}\n", 0, &none).unwrap_err().contains("missing"));
        assert!(request_at("GET https://a.example\nBad Header\n", 0, &none).unwrap_err().contains("Line 2"));
        assert!(request_at("GET https://a.example extra\n", 0, &none).is_err());
        assert!(request_at("# only a comment\n", 0, &none).is_err());
        let r = Request { method: "HEAD".into(), url: "https://a.example".into(), headers: vec![("X-Empty".into(), String::new())], body: None, line: 1 };
        let dir = std::env::temp_dir();
        let args: Vec<String> = command(&r, &dir).unwrap().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"--head".into()) && args.contains(&"X-Empty;".into()) && args.contains(&"=http,https".into()));
        assert_eq!(args.last().unwrap(), "https://a.example");
    }

    #[test]
    fn a_request_reaches_a_local_server_and_the_report_shows_status_headers_and_json() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            // Read the headers, then the body by its length.
            while !String::from_utf8_lossy(&seen).contains("\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
            }
            let text = String::from_utf8_lossy(&seen).into_owned();
            let length: usize = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
            while seen.len() < text.find("\r\n\r\n").unwrap() + 4 + length {
                let n = stream.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
            }
            let body = "{\"ok\":true,\"items\":[1,2]}";
            write!(stream, "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nX-Test: yes\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8_lossy(&seen).into_owned()
        });
        let text = format!("POST http://127.0.0.1:{port}/things\nContent-Type: application/json\n\n{{\"a\": 1}}\n");
        let req = request_at(&text, 0, &none).unwrap();
        let dir = std::env::temp_dir().join(format!("lightdown-http-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = command(&req, &dir).unwrap().output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /things HTTP/1.1") && received.ends_with("{\"a\": 1}"), "{received}");
        let (report, summary) = report(&req, &String::from_utf8_lossy(&out.stdout), &dir);
        assert!(summary.starts_with("HTTP 201 in "), "{summary}");
        assert!(report.contains("HTTP/1.1 201 Created") && report.contains("X-Test: yes"), "{report}");
        assert!(report.contains("\"items\": [\n    1,"), "{report}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_connection_reports_curls_error() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let req = request_at(&format!("GET http://127.0.0.1:{port}/\n"), 0, &none).unwrap();
        let dir = std::env::temp_dir().join(format!("lightdown-http-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = command(&req, &dir).unwrap().output().unwrap();
        let printed = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        let (report, summary) = report(&req, &printed, &dir);
        assert!(report.contains("No response") && summary.starts_with("HTTP request failed"), "{report}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
