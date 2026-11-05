//! Save snapshots outside projects, on a lazy serial worker with bounded retention.
use super::*;
use serde_json::{Value, json};
use std::sync::{OnceLock, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};
const FILE_LIMIT: usize = 2 << 20;
const TOTAL_LIMIT: u64 = 128 << 20;
type Job = Box<dyn FnOnce() + Send>;
fn worker() -> &'static mpsc::SyncSender<Job> {
    static WORKER: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(8);
        std::thread::spawn(move || { for job in rx { job(); } });
        tx
    })
}
fn serialized<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    worker().send(Box::new(move || { let _ = tx.send(f()); })).map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}
fn home() -> PathBuf { config::data_dir().join("history") }
fn folder(base: &Path, path: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let hash = path.as_os_str().as_bytes().iter().fold(0xcbf29ce484222325u64, |n, b| (n ^ *b as u64).wrapping_mul(0x100000001b3));
    base.join(format!("{hash:016x}"))
}
fn entries(base: &Path, path: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(folder(base, path)).into_iter().flatten().flatten()
        .map(|e| e.path()).filter(|p| p.extension().is_some_and(|s| s == "json")).collect();
    files.sort(); files.reverse(); files
}
fn revision(file: &Path, path: &Path) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if v["path"].as_str() != path.to_str() { return Err("History path mismatch".into()); }
    Ok(v)
}
fn prune(base: &Path, cap: u64) {
    let mut files = Vec::new();
    for dir in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let mut local: Vec<_> = std::fs::read_dir(dir.path()).into_iter().flatten().flatten()
            .filter_map(|e| Some((e.path(), e.metadata().ok()?.len()))).collect();
        local.sort_by(|a, b| b.0.cmp(&a.0));
        for (i, (p, size)) in local.into_iter().enumerate() {
            if i >= 100 { let _ = std::fs::remove_file(p); } else { files.push((p, size)); }
        }
    }
    files.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
    let mut total: u64 = files.iter().map(|(_, n)| *n).sum();
    for (path, size) in files { if total <= cap { break; } if std::fs::remove_file(path).is_ok() { total -= size; } }
}
fn snapshot(base: &Path, path: &Path, text: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if text.len() > FILE_LIMIT { return Ok(()); }
    if entries(base, path).first().and_then(|p| revision(p, path).ok()).is_some_and(|v| v["text"] == text) { return Ok(()); }
    let dir = folder(base, path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for d in [base, dir.as_path()] { std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?; }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let file = dir.join(format!("{stamp:032}-{}.json", std::process::id()));
    let data = json!({"path":path, "time":stamp.to_string(), "text":text});
    config::write_private(&file, data.to_string().as_bytes()).map_err(|e| e.to_string())?;
    prune(base, TOTAL_LIMIT);
    Ok(())
}
pub(super) fn record(path: PathBuf, before: Option<String>, after: String, wake: crate::kernel::Waker) -> mpsc::Receiver<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    let failed = tx.clone();
    let result = worker().try_send(Box::new(move || {
        let result = (|| { if let Some(text) = before { snapshot(&home(), &path, &text)?; } snapshot(&home(), &path, &after) })();
        let _ = tx.send(result); wake();
    }));
    if result.is_err() { let _ = failed.send(Err("History queue full; this save was not snapshotted".into())); }
    rx
}
fn timestamp(v: &Value) -> String {
    let nanos = v["time"].as_str().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
    let seconds = (nanos / 1_000_000_000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm); }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, nanos / 1_000_000 % 1000)
}
pub(super) fn list(path: PathBuf) -> Result<Vec<(PathBuf, String)>, String> {
    serialized(move || Ok(entries(&home(), &path).into_iter().filter_map(|p| {
        let v = revision(&p, &path).ok()?;
        Some((p, format!("{} · {} bytes", timestamp(&v), v["text"].as_str()?.len())))
    }).collect()))
}
pub(super) fn text(path: PathBuf, file: PathBuf) -> Result<String, String> {
    serialized(move || {
        if file.parent() != Some(folder(&home(), &path).as_path()) { return Err("Invalid history revision".into()); }
        revision(&file, &path)?["text"].as_str().map(str::to_string).ok_or("Invalid history text".into())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_deduplicate_keep_original_and_prune_oldest() {
        let base = std::env::temp_dir().join(format!("ld-history-{}", std::process::id()));
        let path = base.join("outside project.txt");
        snapshot(&base, &path, "old\n").unwrap();
        snapshot(&base, &path, "old\n").unwrap();
        assert_eq!(entries(&base, &path).len(), 1);
        snapshot(&base, &path, "new\n").unwrap();
        let files = entries(&base, &path);
        assert_eq!(files.len(), 2);
        assert_eq!(revision(&files[0], &path).unwrap()["text"], "new\n");
        assert!(revision(&files[0], &base.join("other")).is_err());
        let one = std::fs::metadata(&files[0]).unwrap().len();
        prune(&base, one);
        assert_eq!(entries(&base, &path), vec![files[0].clone()]);
        std::fs::remove_dir_all(base).unwrap();
    }
}
