//! Normalize downloaded Actions logs before using the existing task parser.
use std::path::Path;
pub(super) fn clean(log: &str, root: &Path) -> String {

    let mut out = String::new();
    for line in log.lines() {
        let text = line.splitn(3, '\t').nth(2).unwrap_or(line);
        let text = text.split_once(' ').filter(|(stamp, _)| stamp.contains('T') && stamp.ends_with('Z')).map_or(text, |(_, rest)| rest);
        let mut text = text.to_string();
        // Runner checkout names need not match a local worktree directory name.
        while let Some(start) = text.find("/home/runner/work/") {
            let tail = &text[start + "/home/runner/work/".len()..];
            let Some((_, tail)) = tail.split_once('/') else { break };
            let Some((_, rest)) = tail.split_once('/') else { break };
            let end = text.len() - rest.len();
            text.replace_range(start..end, &format!("{}/", root.display()));
        }
        out.push_str(&text);
        out.push('\n');
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiler_and_traceback_entries_use_existing_parser() {
        let root = std::env::temp_dir().join(format!("ld-ci-{}",std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "bad").unwrap();
        let log = clean("job\tstep\t2026-01-01T00:00:00Z /home/runner/work/project/project/src/a.rs:2:3: error", &root);
        let entries = super::super::tasks::diagnostics(&root, &log);
        assert_eq!(entries.len(),1); assert_eq!(entries[0].line,2);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn removes_job_step_and_timestamp_and_maps_runner() {
        assert_eq!(clean("build\ttest\t2026-01-01T00:00:00Z /home/runner/work/demo/demo/src/a.rs:2:3: bad", Path::new("/tmp/demo")), "/tmp/demo/src/a.rs:2:3: bad\n");
    }
}
