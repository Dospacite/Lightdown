//! Sends the request under the cursor in a `.http` file and shows the
//! response in a read-only tab. Requests run on this machine, never at launch.
use super::*;
use crate::http_file::{self, Request};
use crate::tools::Job;

#[derive(Default)]
pub(super) struct HttpRequests {
    pending: Option<(Request, PathBuf, Job)>,
    sent: u64,
}

impl App {
    pub(super) fn http_send(&mut self) {
        if self.http.pending.is_some() {
            self.message = Some("A request is already running. HTTP: Stop request cancels it".into());
            return;
        }
        if self.nb.is_some() || self.reader.is_some() || !self.doc.path.as_deref().is_some_and(http_file::is_http) {
            self.message = Some("Open a .http or .rest file to send a request".into());
            return;
        }
        let dotenv = crate::environment::dotenv(&self.root);
        let env = |name: &str| dotenv.get(name).cloned().or_else(|| std::env::var(name).ok());
        let request = match http_file::request_at(&self.doc.buf.text, self.doc.buf.cur, &env) {
            Ok(r) => r,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        self.http.sent += 1;
        let dir = crate::config::cache_dir().join("http").join(format!("{}-{}", std::process::id(), self.http.sent));
        let made = (|| {
            use std::os::unix::fs::DirBuilderExt;
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
            http_file::command(&request, &dir)
        })();
        match made {
            Ok(cmd) => {
                self.message = Some(format!("Sending {} {}…", request.method, request.url));
                self.http.pending = Some((request, dir, Job::start(cmd, self.waker.clone())));
            }
            Err(e) => self.message = Some(format!("Cannot prepare the request: {e}")),
        }
    }

    pub(super) fn http_stop(&mut self) {
        match self.http.pending.take() {
            Some((_, dir, job)) => {
                job.stop();
                let _ = std::fs::remove_dir_all(dir);
                self.message = Some("Request stopped".into());
            }
            None => self.message = Some("No request is running".into()),
        }
    }

    pub(super) fn http_poll(&mut self) {
        let Some((out, _)) = self.http.pending.as_ref().and_then(|(_, _, job)| job.poll()) else { return };
        let (request, dir, _) = self.http.pending.take().unwrap();
        let (text, summary) = http_file::report(&request, &out, &dir);
        let _ = std::fs::remove_dir_all(&dir);
        self.open_output(text);
        self.doc.output_name = Some("Response".into());
        self.message = Some(summary);
        self.redraw = true;
    }

    #[cfg(any(test, feature = "dump"))]
    pub(super) fn http_settle(&mut self) {
        let start = std::time::Instant::now();
        while self.http.pending.is_some() && start.elapsed().as_secs() < 70 {
            self.http_poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn sending_from_a_file_opens_the_response_in_a_read_only_tab() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello").unwrap();
            request
        });
        let root = std::env::temp_dir().join(format!("ld-http-app-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".env"), "TOKEN=from-dotenv\n").unwrap();
        let path = root.join("api.http");
        std::fs::write(&path, format!("### Hello\nGET http://127.0.0.1:{port}/hi\nAuthorization: Bearer {{{{$dotenv TOKEN}}}}\n")).unwrap();
        let mut a = App::new(Some(path));
        a.resize(1000, 800, 1.0);
        a.doc.select(20, 20);
        a.http_send();
        a.http_settle();
        assert!(server.join().unwrap().contains("Authorization: Bearer from-dotenv"));
        assert_eq!(a.message.as_deref().map(|m| m.starts_with("HTTP 200 in ")), Some(true), "{:?}", a.message);
        assert!(a.doc.read_only && a.doc.buf.text.contains("HTTP/1.1 200 OK") && a.doc.buf.text.ends_with("hello\n"));
        assert_eq!(a.doc.output_name.as_deref(), Some("Response"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn other_files_and_bad_requests_say_why_without_sending() {
        let mut a = App::new(None);
        a.resize(1000, 800, 1.0);
        a.http_send();
        assert!(a.message.as_deref().unwrap().contains(".http"));
        let root = std::env::temp_dir().join(format!("ld-http-bad-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("bad.http");
        std::fs::write(&path, "GET ftp://example.com\n").unwrap();
        let mut a = App::new(Some(path));
        a.resize(1000, 800, 1.0);
        a.http_send();
        assert!(a.http.pending.is_none() && a.message.as_deref().unwrap().contains("http://"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
