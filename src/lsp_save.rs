//! Save actions return text only. The save pipeline owns the single buffer edit.
use super::*;
const TIMEOUT: u64 = 10;
/// Runs the configured code action kinds over `text`. The editor's running
/// server is used when its copy of the file is exactly `text`; otherwise a
/// server is started for this save and stopped afterwards.
pub fn save_actions(root: &Path, path: &Path, text: &str, kinds: &[String], wake: Waker) -> Result<String,String> {
    // Files without a server save as they are.
    let Some((language, command)) = server(root,path)? else { return Ok(text.to_string()) };
    let uri = uri(path);
    let range = json!({"start":{"line":0,"character":0},"end":position(text,text.len())});
    let request = json!({"textDocument":{"uri":uri},"range":range,"context":{"diagnostics":[],"only":kinds,"triggerKind":1}});
    let stop = AtomicBool::new(false);
    if let Some(actions) = shared_query_within(root, &command, &uri, text, "textDocument/codeAction", request.clone(), &stop, TIMEOUT) {
        let ask = |method: &str, params: Value| shared_query_within(root, &command, &uri, text, method, params, &stop, TIMEOUT).unwrap_or_else(|| Err("Language server changed during save".into()));
        return combine(text, &uri, kinds, &actions?, None, &ask);
    }
    let client = Client::start_for_language(command, root.to_path_buf(), wake,&language);
    client.notify("textDocument/didOpen",json!({"textDocument":{"uri":uri,"languageId":language,"version":1,"text":text}}));
    let started = std::time::Instant::now();
    let next = std::cell::Cell::new(1u64);
    let ask = |method: &str, params: Value| -> Result<Value,String> {
        let id = next.get();
        next.set(id + 1);
        client.request(id, method, params);
        loop {
            for v in client.poll() {
                if let Some(error) = v["clientError"].as_str() { return Err(error.into()); }
                if v["id"] == id {
                    if !v["error"].is_null() { return Err(v["error"].to_string()); }
                    return Ok(v["result"].clone());
                }
            }
            if started.elapsed().as_secs() >= TIMEOUT { return Err("Code actions on save timed out".into()); }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    let actions = ask("textDocument/codeAction", request)?;
    combine(text, &uri, kinds, &actions, Some(1), &ask)
}
/// Applies every wanted action whose edits do not overlap those of an earlier
/// one; all edits refer to the same original text.
fn combine(text: &str, uri: &str, kinds: &[String], actions: &Value, version: Option<u64>, ask: &dyn Fn(&str, Value) -> Result<Value,String>) -> Result<String,String> {
    let mut accepted: Vec<Value> = Vec::new();
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for action in actions.as_array().into_iter().flatten().take(128) {
        if !action["disabled"].is_null() { continue; }
        let kind = action["kind"].as_str().unwrap_or("");
        if !kinds.iter().any(|k| kind == k || kind.starts_with(&format!("{k}."))) { continue; }
        let mut action = action.clone();
        if action["edit"].is_null() && !action["data"].is_null() {
            action = ask("codeAction/resolve", action)?;
        }
        if !action["command"].is_null() { return Err("Save action needs an unsupported server command".into()); }
        let changes = current_file_edits(&action["edit"], uri, version)?;
        let mut ranges = Vec::new();
        for e in &changes {
            let a = offset(text, &e["range"]["start"]).ok_or("Invalid edit start")?;
            let b = offset(text, &e["range"]["end"]).ok_or("Invalid edit end")?;
            ranges.push((a, b));
        }
        // Two actions touching the same text cannot both apply to the original; the first wins.
        if ranges.iter().any(|&(a, b)| taken.iter().any(|&(c, d)| a < d && c < b || (a == b && a == c) || (c == d && c == a))) { continue; }
        taken.extend(ranges);
        accepted.extend(changes);
    }
    edits(text, &accepted)
}
fn current_file_edits(edit: &Value, uri: &str, version: Option<u64>) -> Result<Vec<Value>,String> {
    let mut out = Vec::new();
    for (target, changes) in edit["changes"].as_object().into_iter().flatten() {
        if target != uri { return Err("Save action edits another file; refused".into()); }
        out.extend(changes.as_array().ok_or("Invalid save edits")?.clone());
    }
    for change in edit["documentChanges"].as_array().into_iter().flatten() {
        if !change["kind"].is_null() || change["textDocument"]["uri"] != uri {
            return Err("Save action contains file operations or another file; refused".into());
        }
        if let (Some(found), Some(expected)) = (change["textDocument"]["version"].as_u64(), version) && found != expected {
            return Err("Save action version mismatch".into());
        }
        out.extend(change["edits"].as_array().ok_or("Invalid save edits")?.clone());
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_actions_refuse_other_files_operations_and_stale_versions() {
        assert!(current_file_edits(&json!({"changes":{"file:///other":[]}}),"file:///a",Some(1)).is_err());
        assert!(current_file_edits(&json!({"documentChanges":[{"kind":"create","uri":"file:///a"}]}),"file:///a",Some(1)).is_err());
        assert!(current_file_edits(&json!({"documentChanges":[{"textDocument":{"uri":"file:///a","version":2},"edits":[]}]}),"file:///a",Some(1)).is_err());
        assert!(current_file_edits(&json!({"changes":{"file:///a":[]}}),"file:///a",Some(1)).is_ok());
    }
    #[test]
    fn overlapping_actions_keep_the_first_and_others_combine() {
        let edit = |l: u64, a: u64, b: u64, t: &str| json!({"edit":{"changes":{"file:///a":[{"range":{"start":{"line":l,"character":a},"end":{"line":l,"character":b}},"newText":t}]}}});
        let mut first = edit(0, 0, 3, "use b;"); first["kind"] = json!("source.organizeImports");
        let mut clash = edit(0, 1, 2, "X"); clash["kind"] = json!("source.fixAll");
        let mut other = edit(1, 0, 3, "two"); other["kind"] = json!("source.fixAll.ruff");
        let mut unwanted = edit(1, 0, 0, "!"); unwanted["kind"] = json!("refactor");
        let kinds = ["source.organizeImports".to_string(), "source.fixAll".to_string()];
        let none = |_: &str, _: Value| -> Result<Value,String> { unreachable!() };
        let out = combine("abc\none", "file:///a", &kinds, &json!([first, clash, other, unwanted]), None, &none).unwrap();
        assert_eq!(out, "use b;\ntwo");
        let mut command = edit(0, 0, 0, ""); command["kind"] = json!("source.fixAll"); command["command"] = json!({"command":"x"});
        assert!(combine("abc", "file:///a", &kinds, &json!([command]), None, &none).is_err());
    }
}
