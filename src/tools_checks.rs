//! The agent's verification tools: the Problems list and test runs, with
//! test output turned into bounded, structured results.
use crate::agent::ToolSpec;
use serde_json::{Value, json};

pub const SPECS: [ToolSpec; 2] = [
    ToolSpec {
        name: "problems",
        description: "List the project's current problems, as the Problems tab shows them: language-server diagnostics, task errors, failed tests and LaTeX build errors. Each has source, file, line, severity and message. Up to 200 entries.",
        schema: r#"{"type":"object","properties":{}}"#,
    },
    ToolSpec {
        name: "run_tests",
        description: "Run the project's tests with its runner (cargo test, or pytest in the project's Python). Give `id` for one test or a name filter (a Cargo test path or a pytest node ID), `file` for one test file, or neither for all. Returns counts, each test's status, and failure messages. Tests run against files on disk.",
        schema: r#"{"type":"object","properties":{"id":{"type":"string","description":"Test ID or name filter"},"file":{"type":"string","description":"Test file, relative to the project"}}}"#,
    },
];

const MAX_TESTS: usize = 200;
const MAX_MESSAGE: usize = 2000;

/// Failure text from cargo's `---- name stdout ----` sections.
fn cargo_failures(out: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in out.lines() {
        if let Some(name) = line.strip_prefix("---- ").and_then(|l| l.strip_suffix(" stdout ----")) {
            found.extend(current.take());
            current = Some((name.to_string(), String::new()));
        } else if line == "failures:" || line.starts_with("test result:") {
            found.extend(current.take());
        } else if let Some((_, text)) = &mut current && text.len() < MAX_MESSAGE {
            text.push_str(line);
            text.push('\n');
        }
    }
    found.extend(current);
    found
}

/// Turns cargo test or `pytest -v -rfE` output into counts, statuses and
/// failure messages, failures first.
pub fn test_results(out: &str, ok: bool) -> Value {
    let mut tests: Vec<(String, &str, String)> = Vec::new();
    for line in out.lines() {
        // cargo: "test path::name ... ok"
        if let Some(rest) = line.strip_prefix("test ") && let Some((id, status)) = rest.split_once(" ... ") {
            let status = match status.split_whitespace().next() { Some("ok") => "passed", Some("ignored") => "skipped", Some("FAILED") => "failed", _ => continue };
            tests.push((id.to_string(), status, String::new()));
            continue;
        }
        // pytest -v: "tests/test_a.py::test_x PASSED [ 50%]"
        if let Some((id, rest)) = line.split_once(' ') && id.contains("::") {
            let status = match rest.split_whitespace().next() { Some("PASSED" | "XPASS") => "passed", Some("SKIPPED" | "XFAIL") => "skipped", Some("FAILED" | "ERROR") => "failed", _ => continue };
            if !tests.iter().any(|t| t.0 == id) { tests.push((id.to_string(), status, String::new())); }
        }
    }
    for (name, text) in cargo_failures(out) {
        if let Some(t) = tests.iter_mut().find(|t| t.0 == name) { t.2 = text.trim_end().to_string(); }
    }
    // pytest's short summary: "FAILED tests/test_a.py::test_x - AssertionError: ..."
    for line in out.lines() {
        let Some(rest) = line.strip_prefix("FAILED ").or_else(|| line.strip_prefix("ERROR ")) else { continue };
        let (id, message) = rest.split_once(" - ").unwrap_or((rest, ""));
        match tests.iter_mut().find(|t| t.0 == id) {
            Some(t) => { t.1 = "failed"; if t.2.is_empty() { t.2 = message.to_string(); } }
            None => tests.push((id.to_string(), "failed", message.to_string())),
        }
    }
    let count = |s: &str| tests.iter().filter(|t| t.1 == s).count();
    let (passed, failed, skipped) = (count("passed"), count("failed"), count("skipped"));
    tests.sort_by_key(|t| t.1 != "failed");
    let total = tests.len();
    let list: Vec<Value> = tests.into_iter().take(MAX_TESTS).map(|(id, status, message)| {
        let mut v = json!({"id": id, "status": status});
        if !message.is_empty() { v["message"] = json!(crate::nb::clip(&message, MAX_MESSAGE)); }
        v
    }).collect();
    let mut result = json!({"ok": ok, "passed": passed, "failed": failed, "skipped": skipped, "tests": list});
    if total > MAX_TESTS { result["truncated"] = json!(format!("{} more tests not listed", total - MAX_TESTS)); }
    // Without per-test lines (a build error, a crash), the output says why.
    if total == 0 || (!ok && failed == 0) {
        result["output"] = json!(crate::nb::clip(out.trim(), 12_000));
    }
    result
}

/// The Problems list as the agent sees it: (source, file, line, severity, message).
pub fn problems(list: &[(String, Option<String>, usize, &str, String)]) -> Value {
    let entries: Vec<Value> = list.iter().take(200).map(|(source, file, line, severity, message)| {
        json!({"source": source, "file": file, "line": line, "severity": severity, "message": crate::nb::clip(message, 1000)})
    }).collect();
    let mut v = json!({"count": list.len(), "problems": entries});
    if list.len() > 200 { v["truncated"] = json!(true); }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cargo_output_becomes_statuses_and_failure_messages() {
        let out = "running 3 tests\ntest a::passes ... ok\ntest a::skipped ... ignored\ntest a::fails ... FAILED\n\nfailures:\n\n---- a::fails stdout ----\n\nthread 'a::fails' panicked at src/a.rs:4:5:\nfixture failure\n\nfailures:\n    a::fails\n\ntest result: FAILED. 1 passed; 1 failed; 1 ignored\n[exit status 101]";
        let v = test_results(out, false);
        assert_eq!((v["passed"].as_u64(), v["failed"].as_u64(), v["skipped"].as_u64()), (Some(1), Some(1), Some(1)));
        assert_eq!(v["tests"][0]["id"], "a::fails");
        assert!(v["tests"][0]["message"].as_str().unwrap().contains("fixture failure"));
        assert!(v.get("output").is_none());
    }
    #[test]
    fn pytest_output_becomes_statuses_and_failure_messages() {
        let out = "tests/test_a.py::test_ok PASSED [ 33%]\ntests/test_a.py::test_bad FAILED [ 66%]\ntests/test_a.py::test_skip SKIPPED (no) [100%]\n=== short test summary info ===\nFAILED tests/test_a.py::test_bad - assert 1 == 2\n";
        let v = test_results(out, false);
        assert_eq!((v["passed"].as_u64(), v["failed"].as_u64(), v["skipped"].as_u64()), (Some(1), Some(1), Some(1)));
        assert_eq!(v["tests"][0]["message"], "assert 1 == 2");
    }
    #[test]
    fn build_errors_and_long_runs_are_bounded() {
        let v = test_results("error[E0425]: cannot find value `x`\n[exit status 101]", false);
        assert!(v["output"].as_str().unwrap().contains("E0425"));
        let many: String = (0..500).map(|i| format!("test t{i} ... ok\n")).collect();
        let v = test_results(&many, true);
        assert_eq!(v["tests"].as_array().unwrap().len(), MAX_TESTS);
        assert_eq!(v["passed"], 500);
        assert!(v["truncated"].is_string());
        let long = format!("test x ... FAILED\n---- x stdout ----\n{}\nfailures:\n", "y".repeat(10_000));
        assert!(test_results(&long, false)["tests"][0]["message"].as_str().unwrap().len() < 3000);
    }
    #[test]
    fn problems_are_bounded() {
        let list: Vec<_> = (0..250).map(|i| ("LSP".to_string(), Some("a.rs".to_string()), i, "error", "m".repeat(5000))).collect();
        let v = problems(&list);
        assert_eq!(v["problems"].as_array().unwrap().len(), 200);
        assert_eq!(v["count"], 250);
        assert!(v["problems"][0]["message"].as_str().unwrap().len() < 1100);
    }
}
