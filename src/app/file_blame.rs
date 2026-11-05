//! Whole-file blame: a column at the left of the gutter naming the commit of
//! each run of lines. It blames the buffer as it stands, on a worker, a
//! moment after the last edit.
use super::repository::{self as repo, Repo};
use super::*;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Each line's commit id and its label.
type Lines = Vec<(String, String)>;

/// How long edits rest before the file is blamed again.
const SETTLE: Duration = Duration::from_millis(400);

/// Why blame gave nothing: the file is outside a repository, or Git could
/// not blame it (say, it is untracked).
#[derive(Debug, PartialEq)]
pub(super) enum Failure {
    NoRepository,
    Git(String),
}

#[derive(Default)]
pub(super) struct State {
    pub enabled: bool,
    /// The file and version last asked about.
    key: Option<(PathBuf, u64)>,
    pending: Option<Instant>,
    work: Option<Receiver<(PathBuf, u64, Result<Lines, Failure>)>>,
}

impl State {
    /// Asks again on the next tick, after a commit or a toggle.
    pub(super) fn stale(&mut self) {
        self.key = None;
    }
}

/// A day's date from seconds since 1970, in UTC.
fn date_string(seconds: i64) -> String {
    let z = seconds.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let day = z - era * 146097;
    let year = (day - day / 1460 + day / 36524 - day / 146096) / 365;
    let doy = day - (365 * year + year / 4 - year / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = year + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn uncommitted(id: &str) -> bool {
    id.bytes().all(|b| b == b'0')
}

/// Reads `git blame --line-porcelain`: each line's commit, labelled with its
/// short hash, author and date.
fn parse(text: &str) -> Lines {
    let (mut id, mut author, mut date) = (String::new(), String::new(), String::new());
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.starts_with('\t') {
            let label = if uncommitted(&id) {
                "Uncommitted".into()
            } else {
                format!("{} {author} {date}", &id[..id.len().min(7)])
            };
            lines.push((id.clone(), label));
        } else if let Some(a) = line.strip_prefix("author ") {
            author = a.into();
        } else if let Some(t) = line.strip_prefix("author-time ") {
            date = date_string(t.trim().parse().unwrap_or(0));
        } else if let Some(first) = line.split(' ').next()
            && first.len() == 40
            && first.bytes().all(|b| b.is_ascii_hexdigit())
        {
            id = first.into();
        }
    }
    lines
}

/// Blames `text` as the current contents of `path`.
pub(super) fn read(root: &Path, path: &Path, text: &str) -> Result<Lines, Failure> {
    use std::io::Write;
    use std::process::Stdio;
    let r = Repo::open(root).map_err(|_| Failure::NoRepository)?;
    let relative = r.relative(path).map_err(|_| Failure::NoRepository)?;
    // The buffer holds LF; a CRLF file on disk is blamed as CRLF.
    let disk = std::fs::read(path).unwrap_or_default();
    let owned = if disk.windows(2).any(|w| w == b"\r\n") && !text.contains("\r\n") {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    };
    let mut child = repo::command(&r.top)
        .args(["blame", "--line-porcelain", "--contents", "-", "--", relative])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Failure::Git(e.to_string()))?;
    let mut input = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || input.write_all(owned.as_bytes()));
    let out = child.wait_with_output().map_err(|e| Failure::Git(e.to_string()))?;
    let _ = writer.join();
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().next().unwrap_or("git blame failed").trim_start_matches("fatal: ");
        return Err(Failure::Git(why.to_string()));
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

impl App {
    /// Turns whole-file blame on or off.
    pub(super) fn file_blame_toggle(&mut self) {
        let state = &mut self.git_actions.file_blame;
        state.enabled = !state.enabled;
        state.stale();
        self.doc.file_blame.clear();
        self.message = Some(format!("Blame for file {}", if state.enabled { "on" } else { "off" }));
        self.redraw = true;
    }

    /// Takes a finished blame, and starts one once edits have rested.
    pub(super) fn file_blame_tick(&mut self) {
        let result = self.git_actions.file_blame.work.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some((path, version, result)) = result {
            self.git_actions.file_blame.work = None;
            if self.doc.path.as_ref() == Some(&path) && self.doc.buf.version == version {
                match result {
                    Ok(lines) => self.doc.file_blame = lines,
                    Err(Failure::NoRepository) => {
                        self.git_actions.file_blame.enabled = false;
                        self.doc.file_blame.clear();
                        self.message = Some("Blame for file is off: the file is not in a Git repository".into());
                    }
                    Err(Failure::Git(why)) => {
                        self.doc.file_blame.clear();
                        self.message = Some(format!("No blame for this file: {why}"));
                    }
                }
                self.redraw = true;
            }
        }
        let state = &mut self.git_actions.file_blame;
        if !state.enabled {
            self.doc.file_blame.clear();
            return;
        }
        let Some(path) = self.doc.path.clone().filter(|_| !self.doc.read_only && self.nb.is_none() && self.reader.is_none())
        else {
            self.doc.file_blame.clear();
            return;
        };
        let key = (path.clone(), self.doc.buf.version);
        if state.key.as_ref() != Some(&key) {
            // Another file loses the old labels; an edit keeps them until the
            // new ones come, since most lines keep their commits.
            if state.key.as_ref().is_none_or(|(p, _)| *p != path) {
                self.doc.file_blame.clear();
            }
            state.key = Some(key.clone());
            state.pending = Some(Instant::now());
            let wake = self.waker.clone();
            std::thread::spawn(move || {
                std::thread::sleep(SETTLE);
                wake();
            });
        }
        let due = state.pending.is_some_and(|at| at.elapsed() >= SETTLE || cfg!(test));
        if state.work.is_some() || !due {
            return;
        }
        state.pending = None;
        let (root, text, wake) = (self.root.clone(), self.doc.buf.text.clone(), self.waker.clone());
        let (tx, rx) = mpsc::channel();
        state.work = Some(rx);
        let version = key.1;
        std::thread::spawn(move || {
            let result = read(&root, &path, &text);
            let _ = tx.send((path, version, result));
            wake();
        });
    }

    /// Waits for blame work, for tests and headless frames.
    #[cfg(any(test, feature = "dump"))]
    pub(super) fn file_blame_settle(&mut self) {
        let start = Instant::now();
        loop {
            self.file_blame_tick();
            let state = &self.git_actions.file_blame;
            if !(state.work.is_some() || state.pending.is_some()) || start.elapsed().as_secs() >= 10 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A click on the blame column opens that run's commit.
    pub(super) fn file_blame_click(&mut self, x: f32, y: f32) -> bool {
        let Some((bx, by, bw, bh)) = self.doc.blame_rect() else { return false };
        if x < bx || x >= bx + bw || y < by || y >= by + bh {
            return false;
        }
        let line = self.doc.visible.iter().rev().find(|(_, top)| y >= *top).map(|(line, _)| *line);
        if let Some(id) = line.and_then(|i| self.doc.file_blame.get(i)).map(|(id, _)| id.clone()) {
            if uncommitted(&id) {
                self.message = Some("These lines are not committed yet".into());
            } else {
                self.git_action(git_actions::Action::RepoDetails(id));
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commits_and_uncommitted_lines() {
        let a = "a".repeat(40);
        let text = format!(
            "{a} 1 1 2\nauthor Ada\nauthor-time 86400\nsummary one\n\tline\n{a} 2 2\nauthor Ada\nauthor-time 86400\n\tnext\n{} 0 3 1\nauthor Not Committed Yet\n\tedit\n",
            "0".repeat(40)
        );
        assert_eq!(date_string(0), "1970-01-01");
        assert_eq!(date_string(951_782_400), "2000-02-29");
        let lines = parse(&text);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].1, "aaaaaaa Ada 1970-01-02");
        assert_eq!(lines[0].0, lines[1].0);
        assert_eq!(lines[2].1, "Uncommitted");
    }
}
