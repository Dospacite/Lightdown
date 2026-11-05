//! Opening the inspector of a running Node process, which VS Code's js-debug
//! extension does before attaching by process ID: the standalone js-debug
//! server only connects to an inspector port and does not signal the process.
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// A listening TCP socket: whether it is IPv6, the address it is bound to and its port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Listener {
    v6: bool,
    loopback_or_any: bool,
    port: u16,
}

/// Parses `/proc/<pid>/net/tcp` or `tcp6` text, keeping listening sockets whose
/// inode is in `inodes`.
fn listeners(table: &str, v6: bool, inodes: &BTreeSet<String>) -> Vec<Listener> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            // local address, remote address, state ("0A" is LISTEN), ..., inode.
            if f.len() < 10 || f[3] != "0A" || !inodes.contains(f[9]) {
                return None;
            }
            let (address, port) = f[1].split_once(':')?;
            let port = u16::from_str_radix(port, 16).ok()?;
            let loopback_or_any = if v6 {
                address.chars().all(|c| c == '0') || address == "00000000000000000000000001000000"
            } else {
                address == "00000000" || address.ends_with("7F")
            };
            Some(Listener { v6, loopback_or_any, port })
        })
        .collect()
}

/// The TCP sockets the process listens on.
fn listening(pid: i32) -> BTreeSet<Listener> {
    let inodes: BTreeSet<String> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .filter_map(|t| t.to_str()?.strip_prefix("socket:[")?.strip_suffix(']').map(str::to_string))
        .collect();
    if inodes.is_empty() {
        return BTreeSet::new();
    }
    let mut out = BTreeSet::new();
    for (name, v6) in [("tcp", false), ("tcp6", true)] {
        if let Ok(table) = std::fs::read_to_string(format!("/proc/{pid}/net/{name}")) {
            out.extend(listeners(&table, v6, &inodes).into_iter().filter(|l| l.loopback_or_any));
        }
    }
    out
}

fn address(l: Listener) -> &'static str {
    if l.v6 { "::1" } else { "127.0.0.1" }
}

/// Whether a port answers as a V8 inspector does.
fn is_inspector(l: Listener) -> bool {
    let ip: std::net::IpAddr = address(l).parse().unwrap();
    let Ok(mut s) = TcpStream::connect_timeout(&SocketAddr::new(ip, l.port), Duration::from_millis(300)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    if s.write_all(b"GET /json/version HTTP/1.0\r\nHost: localhost\r\n\r\n").is_err() {
        return false;
    }
    let mut reply = Vec::new();
    let _ = s.take(8192).read_to_end(&mut reply);
    let reply = String::from_utf8_lossy(&reply);
    reply.contains("\"V8-Version\"") || reply.contains("\"Protocol-Version\"")
}

/// Asks Node process `pid` to open its inspector (SIGUSR1) and returns the
/// address and port to attach to. A process already started with `--inspect`
/// keeps its inspector, which is found by asking each port it listens on.
pub fn open_node_inspector(pid: i32) -> Result<(&'static str, u16), String> {
    if pid <= 0 || !std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return Err(format!("No process {pid}"));
    }
    let before = listening(pid);
    if unsafe { libc::kill(pid, libc::SIGUSR1) } != 0 {
        return Err(format!("Cannot signal process {pid}: {}", std::io::Error::last_os_error()));
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if let Some(&l) = listening(pid).difference(&before).next() {
            return Ok((address(l), l.port));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(&l) = listening(pid).iter().find(|&&l| is_inspector(l)) {
        return Ok((address(l), l.port));
    }
    Err(format!("Process {pid} did not open a Node inspector; is it a Node program?"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_sockets_are_read_from_proc_tables() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:240D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0 100 0 0 10 0\n\
   1: 0100007F:240D 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 112 1 0 20 4 30 10 -1\n\
   2: 00000000:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 113 1 0 100 0 0 10 0\n\
   3: 0101A8C0:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 114 1 0 100 0 0 10 0\n\
   4: 0100007F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 999 1 0 100 0 0 10 0\n";
        let inodes: BTreeSet<String> = ["111", "112", "113", "114"].map(String::from).into();
        let found: Vec<_> = listeners(table, false, &inodes).into_iter().filter(|l| l.loopback_or_any).collect();
        // 9229 on loopback and 3000 on any address; the established connection,
        // the LAN-only socket and the socket of another process are left out.
        assert_eq!(found.iter().map(|l| l.port).collect::<Vec<_>>(), [9229, 3000]);
    }

    #[test]
    fn missing_processes_are_refused() {
        assert!(open_node_inspector(0).is_err());
        assert!(open_node_inspector(i32::MAX).unwrap_err().contains("No process"));
    }

    #[test]
    fn a_running_node_program_opens_its_inspector() {
        let Ok(mut node) = std::process::Command::new("node")
            .args(["-e", "setInterval(() => {}, 1000)"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return; // Node is not installed.
        };
        std::thread::sleep(Duration::from_millis(300));
        let result = open_node_inspector(node.id() as i32);
        let _ = node.kill();
        let _ = node.wait();
        let (address, port) = result.unwrap();
        assert_eq!(address, "127.0.0.1");
        assert!(port > 0);
    }
}
