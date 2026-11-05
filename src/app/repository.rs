//! Git plumbing shared by the menus and the editor. All callers run it on workers.
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn command(dir: &Path) -> Command {
    let mut cmd = Command::new("timeout");
    cmd.args(["-k", "5", "120", "git", "--no-pager", "-c", "color.ui=false", "-c", "core.quotePath=false"])
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .stdin(Stdio::null());
    cmd
}

pub(super) fn output<I, S>(dir: &Path, args: I) -> Result<Output, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    command(dir).args(args).output().map_err(|e| format!("Cannot run Git: {e}"))
}

pub(super) fn run<I, S>(dir: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = output(dir, args)?;
    if !out.status.success() {
        return Err(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
            .trim()
            .to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Clone, Debug)]
pub(super) struct Repo {
    pub top: PathBuf,
}
impl Repo {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let top = run(dir, ["rev-parse", "--show-toplevel"])?;
        Ok(Self { top: PathBuf::from(top.trim_end_matches('\n')) })
    }
    pub fn relative<'a>(&self, path: &'a Path) -> Result<&'a str, String> {
        path.strip_prefix(&self.top)
            .ok()
            .and_then(|p| p.to_str())
            .ok_or_else(|| "File is outside this repository".into())
    }
    pub fn has_head(&self) -> bool {
        run(&self.top, ["rev-parse", "--verify", "HEAD"]).is_ok()
    }
    pub fn blob(&self, revision: &str, path: &Path) -> Result<String, String> {
        let name = self.relative(path)?;
        let bytes = output(&self.top, ["show", &format!("{revision}:{name}")])?;
        if !bytes.status.success() {
            return Err(String::from_utf8_lossy(&bytes.stderr).into_owned());
        }
        if bytes.stdout.contains(&0) {
            return Err("Binary file has no text view".into());
        }
        String::from_utf8(bytes.stdout).map_err(|_| "File is not UTF-8 text".into())
    }
    pub fn baseline(&self, revision: &str, path: &Path) -> Result<String, String> {
        // An added file has an empty base. Other failures must remain visible.
        let name = self.relative(path)?;
        let exists = if revision.is_empty() {
            run(&self.top, ["ls-files", "--error-unmatch", "--", name]).is_ok()
        } else {
            run(&self.top, ["cat-file", "-e", &format!("{revision}:{name}")]).is_ok()
        };
        if !exists {
            if revision == "HEAD"
                && let Some(old) = super::git::ask(&self.top).and_then(|s| s.renamed_from.get(path).cloned())
            {
                return self.blob(revision, &old);
            }
            return Ok(String::new());
        }
        self.blob(revision, path)
    }
    pub fn file_diff(&self, path: &Path, current: &str, staged: bool) -> Result<String, String> {
        let before = self.baseline(if staged { "" } else { "HEAD" }, path)?;
        let label = self.relative(path)?;
        let patch = text_diff(path, &before, current, 3)?;
        Ok(format!("# {label} against {}\n{}", if staged { "index" } else { "HEAD" }, patch))
    }
    pub fn branches(&self) -> Result<Vec<Branch>, String> {
        let text = run(
            &self.top,
            ["for-each-ref", "--format=%(refname:short)%00%(HEAD)%00%(upstream:track)%00", "refs/heads/"],
        )?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\0');
                Some(Branch {
                    name: fields.next()?.into(),
                    current: fields.next()? == "*",
                    tracking: fields.next()?.into(),
                })
            })
            .collect())
    }
    pub fn history(&self, path: &Path) -> Result<Vec<Commit>, String> {
        let text = run(
            &self.top,
            ["log", "--follow", "-n", "200", "--format=%H%x00%as%x00%an%x00%s", "--", self.relative(path)?],
        )?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut f = line.splitn(4, '\0');
                Some(Commit {
                    id: f.next()?.into(),
                    date: f.next()?.into(),
                    author: f.next()?.into(),
                    subject: f.next()?.into(),
                })
            })
            .collect())
    }
    pub fn review(&self, base: &str) -> Result<(String, Vec<PathBuf>), String> {
        run(&self.top, ["check-ref-format", &format!("refs/heads/{base}")])?;
        let merge = run(&self.top, ["merge-base", "HEAD", base])?;
        let merge = merge.trim();
        let names = output(&self.top, ["diff", "--name-only", "-z", merge, "HEAD", "--"])?;
        if !names.status.success() {
            return Err(String::from_utf8_lossy(&names.stderr).into());
        }
        let paths = names
            .stdout
            .split(|&b| b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| self.top.join(OsStr::from_bytes(b)))
            .collect();
        Ok((merge.to_string(), paths))
    }
}

#[derive(Clone, Debug)]
pub(super) struct Branch {
    pub name: String,
    pub current: bool,
    pub tracking: String,
}
#[derive(Clone, Debug)]
pub(super) struct Commit {
    pub id: String,
    pub date: String,
    pub author: String,
    pub subject: String,
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Git's diff algorithm handles large files without an editor-side quadratic table.
pub(super) fn diff(before: &str, after: &str, context: usize) -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lightdown-diff-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    let scratch = Scratch(dir);
    std::fs::write(scratch.0.join("before"), before).map_err(|e| e.to_string())?;
    std::fs::write(scratch.0.join("after"), after).map_err(|e| e.to_string())?;
    let out = output(
        &scratch.0,
        [
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            &format!("--unified={context}"),
            "--",
            "before",
            "after",
        ],
    )?;
    if !matches!(out.status.code(), Some(0 | 1)) {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The editor displays LF internally. Compare that display without inventing EOL edits.
pub(super) fn text_diff(path: &Path, before: &str, after: &str, context: usize) -> Result<String, String> {
    let cr = super::saving::Rules::read(path).cr();
    let normalize = |text: &str| {
        let text = text.replace("\r\n", "\n");
        if cr { text.replace('\r', "\n") } else { text }
    };
    diff(&normalize(before), &normalize(after), context)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mark {
    Added,
    Modified,
    Deleted,
}

pub(super) fn marks(patch: &str) -> Vec<(usize, Mark)> {
    let mut out = Vec::new();
    for line in patch.lines().filter(|s| s.starts_with("@@ ")) {
        let mut fields = line.split_whitespace();
        let range = |s: &str| -> Option<(usize, usize)> {
            let (start, count) = s.get(1..)?.split_once(',').unwrap_or((&s[1..], "1"));
            Some((start.parse().ok()?, count.parse().ok()?))
        };
        let Some((_, removed)) = fields.nth(1).and_then(range) else { continue };
        let Some((start, added)) = fields.next().and_then(range) else { continue };
        if added == 0 {
            out.push((start.saturating_sub(1), Mark::Deleted));
        }
        for i in 0..added {
            out.push((start.saturating_sub(1) + i, if i < removed { Mark::Modified } else { Mark::Added }));
        }
    }
    out
}

#[derive(Clone, Debug)]
pub(super) struct Conflict {
    pub start: usize,
    pub end: usize,
    pub current: String,
    pub incoming: String,
    pub base: Option<String>,
}
pub(super) fn conflicts(text: &str) -> Vec<Conflict> {
    let mut out = Vec::new();
    let mut start = None;
    let (mut ours, mut theirs, mut ancestor) = (0, None, None);
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let marker = line.trim_end_matches(['\r', '\n']);
        if marker.starts_with("<<<<<<< ") && start.is_none() {
            start = Some(offset);
            ours = offset + line.len();
            theirs = None;
            ancestor = None;
        } else if start.is_some() && marker.starts_with("||||||| ") && theirs.is_none() {
            ancestor = Some(offset);
        } else if start.is_some() && marker == "=======" && theirs.is_none() {
            theirs = Some((offset, offset + line.len()));
        } else if marker.starts_with(">>>>>>> ")
            && let (Some(start), Some((separator, incoming))) = (start.take(), theirs.take())
        {
            out.push(Conflict {
                start,
                end: offset + line.len(),
                current: text[ours..ancestor.unwrap_or(separator)].into(),
                incoming: text[incoming..offset].into(),
                base: ancestor.map(|a| { let begin = a + text[a..].find('\n').map_or(0, |n| n + 1); text[begin..separator].into() }),
            });
        }
        offset += line.len();
    }
    out
}
