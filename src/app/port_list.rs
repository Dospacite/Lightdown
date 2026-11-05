//! Server addresses seen in this window's terminals and jobs, and SSH
//! forwards to them. Nothing is forwarded without an explicit command.
use super::*;
use std::process::{Child, Stdio};

pub(super) struct Detected {
    url: String,
    port: u16,
    /// The terminal or task it was printed in.
    source: String,
    /// Printed by a command running on the SSH host.
    remote: bool,
}

/// A managed `ssh -N -L`, ended when dropped.
struct Forward {
    root: PathBuf,
    host: String,
    local: u16,
    port: u16,
    child: Child,
}

impl Drop for Forward {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
pub(super) struct PortList {
    found: Vec<Detected>,
    forwards: Vec<Forward>,
    /// Terminals scanned last time, to forget closed ones.
    terms: Vec<u64>,
}

/// `want` if it is free on this machine, else any free port.
fn free_port(want: u16) -> Option<u16> {
    if want >= 1024 && std::net::TcpListener::bind(("127.0.0.1", want)).is_ok() {
        return Some(want);
    }
    std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?.local_addr().ok().map(|a| a.port())
}

impl App {
    /// Takes in addresses the terminals have printed.
    pub(super) fn ports_poll(&mut self) {
        let ssh = crate::remote::active(&self.root).is_some_and(|r| r.container.is_none());
        let open: Vec<u64> = self.terms.iter().map(|t| t.agent_id).collect();
        for id in std::mem::take(&mut self.remote_projects.ports.terms) {
            if !open.contains(&id) {
                crate::ports::forget(id);
            }
        }
        self.remote_projects.ports.terms = open;
        for t in &self.terms {
            for (url, port) in crate::ports::take(t.agent_id) {
                let remote = ssh && t.dir.starts_with(&self.root);
                let list = &mut self.remote_projects.ports.found;
                list.retain(|d| !(d.port == port && d.remote == remote));
                if list.len() >= 64 {
                    list.remove(0);
                }
                list.push(Detected { url, port, source: t.title(), remote });
            }
        }
    }
    /// Ends forwards whose ssh has exited, and says why.
    pub(super) fn forwards_tick(&mut self) {
        let list = &mut self.remote_projects.ports.forwards;
        let mut i = 0;
        while i < list.len() {
            if let Ok(Some(_)) = list[i].child.try_wait() {
                let mut f = list.remove(i);
                let mut why = String::new();
                if let Some(mut err) = f.child.stderr.take() {
                    use std::io::Read;
                    let _ = err.by_ref().take(4096).read_to_string(&mut why);
                }
                self.message = Some(format!("Port forward to {}:{} ended: {}", f.host, f.port, tools_line(&why)));
                self.redraw = true;
            } else {
                i += 1;
            }
        }
    }
    pub(super) fn stop_forwards(&mut self, root: &Path) {
        self.remote_projects.ports.forwards.retain(|f| f.root != root);
    }
    pub(super) fn show_ports(&mut self) {
        let mut entries = Vec::new();
        for (i, f) in self.remote_projects.ports.forwards.iter().enumerate() {
            entries.push(Entry::new(
                &format!("port:menu:fwd:{i}"),
                &format!("http://localhost:{}", f.local),
                &format!("Forwarded to {}:{}", f.host, f.port),
            ));
        }
        if let Some((ports, host)) = self.container_ports() {
            for port in ports {
                let how = if host { "host network" } else { "published" };
                entries.push(Entry::new(&format!("port:menu:ctr:{port}"), &format!("http://localhost:{port}"), &format!("Container forwardPorts, {how}")));
            }
        }
        for (i, d) in self.remote_projects.ports.found.iter().enumerate().rev() {
            let place = if d.remote { "on the SSH host" } else { "on this machine" };
            entries.push(Entry::new(&format!("port:menu:{i}"), &d.url, &format!("From {}, {place}", d.source)));
        }
        if entries.is_empty() {
            self.message = Some("No server addresses seen in terminal or task output yet".into());
            return;
        }
        self.menu = Some((MenuKind::Tasks, Menu::new("Detected ports", entries, "")));
    }
    /// Offers the remote ports seen so far for forwarding.
    pub(super) fn forward_menu(&mut self) {
        if !crate::remote::active(&self.root).is_some_and(|r| r.container.is_none()) {
            self.message = Some("Connect the SSH project first".into());
            return;
        }
        let forwarded: Vec<u16> = self.remote_projects.ports.forwards.iter().filter(|f| f.root == self.root).map(|f| f.port).collect();
        let entries: Vec<_> = self
            .remote_projects
            .ports
            .found
            .iter()
            .filter(|d| d.remote && !forwarded.contains(&d.port))
            .map(|d| Entry::new(&format!("port:forward:{}", d.port), &format!("Port {}", d.port), &format!("{} from {}", d.url, d.source)))
            .collect();
        if entries.is_empty() {
            self.message = Some("No new ports seen in remote terminals or tasks; start a server there first".into());
            return;
        }
        self.menu = Some((MenuKind::Tasks, Menu::new("Forward a remote port", entries, "")));
    }
    fn start_forward(&mut self, port: u16) -> Result<String, String> {
        let r = crate::remote::active(&self.root).filter(|r| r.container.is_none()).ok_or("Connect the SSH project first")?;
        if self.remote_projects.ports.forwards.len() >= 8 {
            return Err("At most eight ports can be forwarded".into());
        }
        let local = free_port(port).ok_or("No free local port")?;
        let child = r
            .forward(local, port)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("ssh: {e}"))?;
        self.remote_projects.ports.forwards.push(Forward { root: self.root.clone(), host: r.host.clone(), local, port, child });
        Ok(format!("Forwarding http://localhost:{local} to {}:{port}; Ports: Show detected ports stops it", r.host))
    }
    /// Acts on a `port:` menu entry.
    pub(super) fn port_pick(&mut self, key: &str) {
        if let Some(port) = key.strip_prefix("forward:").and_then(|p| p.parse().ok()) {
            self.message = Some(self.start_forward(port).unwrap_or_else(|e| e));
        } else if let Some(i) = key.strip_prefix("stop:").and_then(|i| i.parse::<usize>().ok()) {
            if i < self.remote_projects.ports.forwards.len() {
                let f = self.remote_projects.ports.forwards.remove(i);
                self.message = Some(format!("Stopped forwarding {}:{}", f.host, f.port));
            }
        } else if let Some(url) = key.strip_prefix("open:") {
            let opened = cfg!(test) || std::process::Command::new("xdg-open").arg(url).spawn().is_ok();
            self.message = Some(if opened { format!("Opened {url}") } else { "Cannot run xdg-open".into() });
        } else if let Some(url) = key.strip_prefix("copy:") {
            self.copy = Some(url.to_string());
            self.message = Some(format!("Copied {url}"));
        } else if let Some(which) = key.strip_prefix("menu:") {
            self.port_menu(which);
        }
    }
    /// What can be done with one listed address.
    fn port_menu(&mut self, which: &str) {
        let mut entries = Vec::new();
        let (title, url) = if let Some(i) = which.strip_prefix("fwd:").and_then(|i| i.parse::<usize>().ok()) {
            let Some(f) = self.remote_projects.ports.forwards.get(i) else { return };
            entries.push(Entry::new(&format!("port:stop:{i}"), "Stop forwarding", &format!("End ssh -L to {}:{}", f.host, f.port)));
            (format!("Port {} on {}", f.port, f.host), format!("http://localhost:{}", f.local))
        } else if let Some(port) = which.strip_prefix("ctr:") {
            (format!("Container port {port}"), format!("http://localhost:{port}"))
        } else {
            let Some(d) = which.parse::<usize>().ok().and_then(|i| self.remote_projects.ports.found.get(i)) else { return };
            if d.remote {
                match self.remote_projects.ports.forwards.iter().position(|f| f.port == d.port && f.root == self.root) {
                    Some(i) => return self.port_menu(&format!("fwd:{i}")),
                    None => entries.push(Entry::new(&format!("port:forward:{}", d.port), "Forward to this machine", "Run ssh -N -L for this port")),
                }
            }
            (format!("{} from {}", d.url, d.source), d.url.clone())
        };
        let remote_only = entries.iter().any(|e| e.key.starts_with("port:forward:"));
        if !remote_only {
            entries.insert(0, Entry::new(&format!("port:open:{url}"), "Open in browser", &url));
        }
        entries.insert(usize::from(!remote_only), Entry::new(&format!("port:copy:{url}"), "Copy address", &url));
        self.menu = Some((MenuKind::Tasks, Menu::new(&title, entries, "")));
    }
}

fn tools_line(text: &str) -> String {
    let line = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("ssh exited");
    crate::tools::clip_line(line, 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn printed_addresses_are_listed_with_their_source() {
        let root = std::env::temp_dir().join(format!("ld-ports-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join(".lightdown/tasks.json"), r#"{"tasks":[{"name":"serve","shell":"echo ' * Running on http://127.0.0.1:5000'"}]}"#).unwrap();
        let mut a = App::new(Some(root.join("a.txt")));
        a.resize(900, 700, 1.0);
        a.start_named_task("serve").unwrap();
        let start = std::time::Instant::now();
        while a.remote_projects.ports.found.is_empty() && start.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            a.remote_poll();
        }
        let d = &a.remote_projects.ports.found[0];
        assert_eq!((d.url.as_str(), d.port, d.source.as_str(), d.remote), ("http://127.0.0.1:5000", 5000, "serve", false));
        a.show_ports();
        let (_, menu) = a.menu.as_ref().unwrap();
        assert_eq!(menu.entries.len(), 1);
        a.port_pick("menu:0");
        let keys: Vec<_> = a.menu.as_ref().unwrap().1.entries.iter().map(|e| e.key.clone()).collect();
        assert_eq!(keys, ["port:open:http://127.0.0.1:5000", "port:copy:http://127.0.0.1:5000"]);
        a.start_task("port:copy:http://127.0.0.1:5000");
        assert_eq!(a.copy.as_deref(), Some("http://127.0.0.1:5000"));
        a.forward_menu();
        assert!(a.message.as_deref().unwrap().contains("SSH"));
        a.stop_task();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_free_local_port_is_chosen() {
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = held.local_addr().unwrap().port();
        let got = free_port(busy).unwrap();
        assert_ne!(got, busy);
    }
}
