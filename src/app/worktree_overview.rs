//! Private, process-owned window state. Dead processes never count as open.
use std::{path::{Path, PathBuf}, os::unix::fs::{DirBuilderExt, OpenOptionsExt}, io::Write};
fn directory() -> PathBuf { crate::config::data_dir().join("windows") }
pub(super) fn publish(value: &str) -> Option<PathBuf> {
    let dir = directory();
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).ok()?;
    let path = dir.join(format!("{}.json", std::process::id()));
    let temp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temp).ok()?;
    file.write_all(value.as_bytes()).ok()?;
    std::fs::rename(temp, &path).ok()?;
    Some(path)
}
fn live(v: &serde_json::Value, path: &Path) -> bool {
    v["path"].as_str().is_some_and(|p| Path::new(p) == path) && alive(v)
}
fn alive(v: &serde_json::Value) -> bool {
    v["pid"].as_u64().is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists())
}
/// Whether a window has the worktree open and its agent is working, and
/// what its threads cost. Files of windows that died are removed.
pub(super) fn windows(path: &Path) -> String {
    let (mut open, mut busy, mut live_cost) = (false, false, 0.0);
    for entry in std::fs::read_dir(directory()).into_iter().flatten().flatten().take(1024) {
        if !entry.metadata().is_ok_and(|m| m.len() < 4096) { continue; }
        let Ok(v) = std::fs::read(entry.path()).map(|b| serde_json::from_slice::<serde_json::Value>(&b)) else { continue };
        let Ok(v) = v else { continue };
        if !alive(&v) { let _ = std::fs::remove_file(entry.path()); continue; }
        if live(&v, path) {
            open = true;
            busy |= v["busy"] == true;
            live_cost += v["cost"].as_f64().unwrap_or(0.0);
        }
    }
    let window = match (open, busy) { (false, _) => "no window", (true, false) => "open, agent idle", (true, true) => "open, agent working" };
    let cost = crate::agent::Thread::recorded_cost(path).or(open.then_some(live_cost));
    match cost {
        Some(c) => format!("{window} \u{b7} ${c:.4}"),
        None => format!("{window} \u{b7} no cost recorded"),
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn rejects_dead_or_other_project() {
        let v = serde_json::json!({"path":"/tmp/a","pid":std::process::id()});
        assert!(live(&v, Path::new("/tmp/a")));
        assert!(!live(&v, Path::new("/tmp/b")));
        assert!(!live(&serde_json::json!({"path":"/tmp/a","pid":4294967295u64}), Path::new("/tmp/a")));
        assert!(windows(Path::new("/nonexistent/worktree")).starts_with("no window"));
    }
}
