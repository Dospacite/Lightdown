//! Line coverage read from the reports test tools write: lcov
//! (`cargo llvm-cov --lcov`, `coverage lcov`, most JavaScript tools) and
//! coverage.py's JSON (`coverage json`).
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Where reports are looked for, from the project's folder. The newest wins.
pub const PLACES: [&str; 7] = [
    "lcov.info",
    "coverage.lcov",
    "coverage/lcov.info",
    "target/lcov.info",
    "target/llvm-cov/lcov.info",
    "coverage.json",
    "coverage/coverage.json",
];
/// The largest report read.
const MOST: u64 = 128 << 20;

#[derive(Debug, Default)]
pub struct Report {
    pub path: PathBuf,
    pub written: Option<SystemTime>,
    /// Per file, its measured lines counted from 0, in order, and whether
    /// each ran.
    pub files: HashMap<PathBuf, Vec<(usize, bool)>>,
}

impl Report {
    /// Lines that ran and lines measured, over every file.
    pub fn totals(&self) -> (usize, usize) {
        self.files.values().flatten().fold((0, 0), |(hit, all), (_, ran)| (hit + *ran as usize, all + 1))
    }
}

/// `a/./b/../c` as `a/c`, without asking the disk.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    out
}

/// The newest report in the project, if there is one.
pub fn find(root: &Path) -> Option<(PathBuf, SystemTime)> {
    PLACES
        .iter()
        .map(|p| root.join(p))
        .filter_map(|p| Some((std::fs::metadata(&p).ok().filter(|m| m.is_file())?.modified().ok()?, p)))
        .max()
        .map(|(t, p)| (p, t))
}

fn add(files: &mut HashMap<PathBuf, BTreeMap<usize, bool>>, root: &Path, name: &str, line: u64, ran: bool) {
    if line == 0 || name.is_empty() {
        return;
    }
    let path = tidy(&root.join(name));
    let seen = files.entry(path).or_default().entry(line as usize - 1).or_insert(false);
    *seen |= ran;
}

fn lcov(text: &str, root: &Path, files: &mut HashMap<PathBuf, BTreeMap<usize, bool>>) {
    let mut file = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("SF:") {
            file = name.trim().to_string();
        } else if let Some(data) = line.strip_prefix("DA:") {
            let mut parts = data.split(',');
            let (Some(n), Some(hits)) = (parts.next(), parts.next()) else { continue };
            // Some tools write hit counts as floats or past u64.
            let ran = hits.trim().parse::<f64>().is_ok_and(|h| h > 0.0);
            if let Ok(n) = n.trim().parse() {
                add(files, root, &file, n, ran);
            }
        } else if line.trim() == "end_of_record" {
            file.clear();
        }
    }
}

fn coverage_py(v: &Value, root: &Path, files: &mut HashMap<PathBuf, BTreeMap<usize, bool>>) -> Result<(), String> {
    let all = v["files"].as_object().ok_or("coverage.json has no \"files\" table")?;
    for (name, f) in all {
        for (key, ran) in [("executed_lines", true), ("missing_lines", false)] {
            for n in f[key].as_array().into_iter().flatten().filter_map(Value::as_u64) {
                add(files, root, name, n, ran);
            }
        }
    }
    Ok(())
}

/// Reads a report. Relative file names are taken from the project's folder.
pub fn read(root: &Path, path: &Path) -> Result<Report, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > MOST {
        return Err(format!("{} is over 128 MB", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut files = HashMap::new();
    if path.extension().is_some_and(|e| e == "json") {
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        coverage_py(&v, root, &mut files)?;
    } else {
        lcov(&String::from_utf8_lossy(&bytes), root, &mut files);
    }
    Ok(Report {
        path: path.to_path_buf(),
        written: meta.modified().ok(),
        files: files.into_iter().map(|(p, lines)| (p, lines.into_iter().collect())).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcov_records_merge_and_resolve_against_the_project() {
        let root = Path::new("/project");
        let text = "TN:\nSF:src/lib.rs\nDA:1,3\nDA:2,0\nDA:4,1.5\nend_of_record\nSF:/project/./src/lib.rs\nDA:2,7\nDA:9,0,abcd\nend_of_record\nSF:../outside.rs\nDA:1,1\nend_of_record\n";
        let mut files = HashMap::new();
        lcov(text, root, &mut files);
        let lib: Vec<_> = files[Path::new("/project/src/lib.rs")].iter().map(|(l, r)| (*l, *r)).collect();
        assert_eq!(lib, vec![(0, true), (1, true), (3, true), (8, false)]);
        assert!(files.contains_key(Path::new("/outside.rs")));
    }

    #[test]
    fn coverage_py_json_and_the_newest_report_are_found() {
        let root = std::env::temp_dir().join(format!("ld-coverage-{}", std::process::id()));
        std::fs::create_dir_all(root.join("coverage")).unwrap();
        std::fs::write(root.join("coverage/lcov.info"), "SF:a.py\nDA:1,1\nend_of_record\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let json = r#"{"meta":{},"files":{"pkg/app.py":{"executed_lines":[1,2,5],"missing_lines":[3],"excluded_lines":[4]}}}"#;
        std::fs::write(root.join("coverage.json"), json).unwrap();
        let (path, _) = find(&root).unwrap();
        assert_eq!(path, root.join("coverage.json"));
        let report = read(&root, &path).unwrap();
        assert_eq!(report.files[&root.join("pkg/app.py")], vec![(0, true), (1, true), (2, false), (4, true)]);
        assert_eq!(report.totals(), (3, 4));
        std::fs::write(root.join("coverage.json"), "{\"nope\": 1}").unwrap();
        assert!(read(&root, &root.join("coverage.json")).unwrap_err().contains("files"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
