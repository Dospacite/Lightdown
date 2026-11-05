//! A pseudo-terminal with a shell on the far side of it. What the shell
//! writes is read on a thread of its own; what is typed is written on
//! another, so that a program that stops reading never holds up the window.

use std::ffi::CStr;
use std::fs::File;
use std::io::{Error, Read, Result, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};

pub struct Pty {
    master: File,
    pid: i32,
    input: Sender<Vec<u8>>,
}

fn check(ret: i32) -> Result<i32> {
    if ret < 0 { Err(Error::last_os_error()) } else { Ok(ret) }
}

fn winsize(cols: usize, rows: usize) -> libc::winsize {
    libc::winsize {
        ws_row: rows.min(u16::MAX as usize) as u16,
        ws_col: cols.min(u16::MAX as usize) as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// The user's shell, or `sh`.
pub fn shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty() && Path::new(s).is_file()).unwrap_or_else(|| "/bin/sh".into())
}

impl Pty {
    /// Starts `program` in `dir` on a new terminal of `cols` by `rows`.
    /// `output` is given what it writes, on the reading thread, and returns
    /// what is to be written back; `gone` is called once it has ended.
    pub fn spawn(
        program: &str,
        dir: &Path,
        root: &Path,
        (cols, rows): (usize, usize),
        output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        gone: impl FnOnce() + Send + 'static,
    ) -> Result<Pty> {
        Self::spawn_command(None, program, dir, root, (cols, rows), output, move |_| gone())
    }

    pub fn spawn_command(
        command: Option<Command>, program: &str, dir: &Path, root: &Path,
        (cols, rows): (usize, usize),
        mut output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        gone: impl FnOnce(Option<i32>) + Send + 'static,
    ) -> Result<Pty> {
        let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
        // SAFETY: plain calls on a descriptor this function owns; the name
        // buffer is large enough and nul-terminated by ptsname_r.
        let (master, slave) = unsafe {
            let master = OwnedFd::from_raw_fd(check(libc::posix_openpt(flags))?);
            check(libc::grantpt(master.as_raw_fd()))?;
            check(libc::unlockpt(master.as_raw_fd()))?;
            let mut name = [0 as libc::c_char; 128];
            let err = libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len());
            if err != 0 {
                return Err(Error::from_raw_os_error(err));
            }
            let slave = OwnedFd::from_raw_fd(check(libc::open(CStr::from_ptr(name.as_ptr()).as_ptr(), flags))?);
            let size = winsize(cols, rows);
            check(libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size))?;
            // Backspace takes back a whole character, not a byte of one.
            let mut tio: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave.as_raw_fd(), &mut tio) == 0 {
                tio.c_iflag |= libc::IUTF8;
                libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &tio);
            }
            (master, slave)
        };
        let mut cmd = if let Some(command) = command { command } else if let Some(mut remote) = crate::remote::active(root) {
            if let Ok(rel) = dir.strip_prefix(root) { remote.path = remote.path.join(rel); }
            remote.command("exec \"${SHELL:-sh}\"", true)
        } else {
            // A local shell gets the prompt and command marks, if it is one
            // shell integration knows.
            let mut cmd = Command::new(program);
            crate::shell_integration::configure(&mut cmd, program);
            cmd
        };
        cmd.current_dir(dir)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave))
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "Lightdown")
            .env_remove("LINES")
            .env_remove("COLUMNS");
        crate::environment::apply(&mut cmd, root, None);
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                // A session of its own, with this terminal as its own.
                check(libc::setsid())?;
                check(libc::ioctl(0, libc::TIOCSCTTY, 0))?;
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        // The parent's ends of the terminal's far side close with `cmd`.
        drop(cmd);
        let pid = child.id() as i32;
        let master = File::from(master);
        let (mut reader, mut writer) = (master.try_clone()?, master.try_clone()?);
        let (input, typed) = channel::<Vec<u8>>();
        std::thread::spawn(move || {
            for bytes in typed {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        let replies = input.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 1 << 16];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let reply = output(&buf[..n]);
                        if !reply.is_empty() {
                            let _ = replies.send(reply);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    // EIO: nothing holds the far side open any more.
                    Err(_) => break,
                }
            }
            let code = child.wait().ok().and_then(|s| s.code());
            gone(code);
        });
        Ok(Pty { master, pid, input })
    }

    pub fn pid(&self) -> i32 { self.pid }

    pub fn write(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self.input.send(bytes.to_vec());
        }
    }

    pub fn resize(&self, cols: usize, rows: usize) {
        let size = winsize(cols, rows);
        // SAFETY: an ioctl on a descriptor this owns, with the right type.
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
    }

    /// Whether the shell itself has the terminal, rather than a program
    /// it is running.
    pub fn idle(&self) -> bool {
        // SAFETY: a query on a descriptor this owns.
        unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) == self.pid }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // The terminal hangs up: the shell, and what it runs, are told so.
        // SAFETY: sends a signal to the process group this started.
        unsafe { libc::kill(-self.pid, libc::SIGHUP) };
    }
}
