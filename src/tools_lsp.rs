//! Read-only language-server tools. Each job owns its server and can be stopped.
use super::*;
use crate::lsp;
use serde_json::json;

pub const SPECS: [ToolSpec; 7] = [
    ToolSpec { name: "lsp_diagnostics", description: "Ask the file's configured language server for diagnostics of its current contents, including unsaved open text. Returns LSP ranges in zero-based UTF-16 positions. An unavailable server or a timeout is an error, not a clean bill of health.", schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"# },
    ToolSpec { name: "lsp_definition", description: "Find definitions using the project's configured language server. line and column are one-based; column counts Unicode characters. Results use zero-based UTF-16 LSP positions.", schema: r#"{"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer","minimum":1},"column":{"type":"integer","minimum":1}},"required":["path","line","column"]}"# },
    ToolSpec { name: "lsp_references", description: "Find references, including declarations, using the project's configured language server. line and column are one-based Unicode positions. Results use zero-based UTF-16 LSP positions.", schema: r#"{"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer","minimum":1},"column":{"type":"integer","minimum":1}},"required":["path","line","column"]}"# },
    ToolSpec { name: "lsp_hover", description: "Read the language server's documentation and type for the symbol at a one-based line and Unicode column.", schema: r#"{"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer","minimum":1},"column":{"type":"integer","minimum":1}},"required":["path","line","column"]}"# },
    ToolSpec { name: "lsp_document_symbols", description: "List the language server's symbols in a file, including unsaved open text. Ranges are zero-based UTF-16 LSP positions.", schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"# },
    ToolSpec { name: "lsp_workspace_symbols", description: "Search the project's symbols by name with the language server of the file at path. An empty query lists what the server offers. Output is truncated at 38,000 characters.", schema: r#"{"type":"object","properties":{"path":{"type":"string"},"query":{"type":"string"}},"required":["path","query"]}"# },
    ToolSpec { name: "lsp_rename", description: "Rename the symbol at a one-based line and Unicode column everywhere the language server finds it, as one edit of each changed file. Needs edit permission; each file is checkpointed like edit_file. Refused if a target lies outside the project, has unsaved changes, or changed while the server worked.", schema: r#"{"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer","minimum":1},"column":{"type":"integer","minimum":1},"new_name":{"type":"string"}},"required":["path","line","column","new_name"]}"# },
];
impl Job {
    /// `extra` is the query of a workspace symbol search or the new name of a rename.
    #[allow(clippy::too_many_arguments)]
    pub fn language(root: PathBuf, path: PathBuf, text: String, action: String, line: usize, column: usize, extra: String, wake: Waker) -> Self {
        let (tx, done) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let notify = wake.clone();
        std::thread::spawn(move || {
            let result = query(&root,&path,&text,&action,line,column,&extra,&stop,wake);
            // A rename's edit is applied by the editor, which reports a summary instead.
            let limit = if action == "lsp_rename" { 16 << 20 } else { 38_000 };
            let _ = tx.send(match result { Ok(s) if s.len() > limit && action == "lsp_rename" => ("The rename edit exceeds 16 MB".into(),false), Ok(s) => (clip(&s,limit),true), Err(s) => (s,false) });
            notify();
        });
        Self { done, child: Arc::new(Mutex::new(None)), cancelled }
    }
}
#[allow(clippy::too_many_arguments)]
fn query(root: &Path, path: &Path, text: &str, action: &str, line: usize, column: usize, extra: &str, stop: &AtomicBool, wake: Waker) -> Result<String,String> {
    let (language,command) = lsp::server(root,path)?.ok_or("No language server installed or configured for this file")?;
    let at = if matches!(action, "lsp_diagnostics" | "lsp_document_symbols" | "lsp_workspace_symbols") { 0 } else {
        if line == 0 || column == 0 { return Err("Line and column start at 1".into()); }
        let start = if line == 1 { 0 } else { text.match_indices('\n').nth(line-2).ok_or("Line is outside the file")?.0+1 };
        let end = text[start..].find('\n').map_or(text.len(),|i| start+i);
        let chars = text[start..end].chars().count();
        if column-1 > chars { return Err("Column is outside the line".into()); }
        start+text[start..end].char_indices().nth(column-1).map_or(end-start,|(i,_)| i)
    };
    if action == "lsp_rename" && (extra.is_empty() || extra.chars().any(char::is_whitespace)) { return Err("new_name must be a name without spaces".into()); }
    let uri = lsp::uri(path);
    let method = match action {
        "lsp_hover" => "textDocument/hover", "lsp_document_symbols" => "textDocument/documentSymbol", "lsp_workspace_symbols" => "workspace/symbol",
        "lsp_definition" => "textDocument/definition", "lsp_references" => "textDocument/references", "lsp_rename" => "textDocument/rename", _ => "",
    };
    let mut params = json!({"textDocument":{"uri":uri},"position":lsp::position(text,at)});
    if action == "lsp_references" { params["context"] = json!({"includeDeclaration":true}); }
    if action == "lsp_workspace_symbols" { params = json!({"query":extra}); }
    if action == "lsp_rename" { params["newName"] = json!(extra); }
    if action == "lsp_document_symbols" { params = json!({"textDocument":{"uri":uri}}); }
    // The editor's running server answers at once when it holds the same text.
    let shared = if method.is_empty() { lsp::shared_query(root, &command, &uri, text, "textDocument/diagnostic", json!({"textDocument":{"uri":uri}}), stop) }
        else { lsp::shared_query(root, &command, &uri, text, method, params.clone(), stop) };
    if let Some(result) = shared {
        let result = result?;
        return Ok(if method.is_empty() { json!({"uri":uri,"diagnostics":result["items"]}) } else { json!({"uri":uri,"result":result}) }.to_string());
    }
    let client = lsp::Client::start_for_language(command,root.to_path_buf(),wake,&language);
    client.notify("textDocument/didOpen",json!({"textDocument":{"uri":uri,"languageId":language,"version":1,"text":text}}));
    if !method.is_empty() { client.request(1,method,params); }
    let start = std::time::Instant::now();
    loop {
        if stop.load(Ordering::SeqCst) { return Err("Language-server tool stopped".into()); }
        for v in client.poll() {
            if action == "lsp_diagnostics" && v["clientCapabilities"]["diagnosticProvider"].is_object() {
                client.request(1,"textDocument/diagnostic",json!({"textDocument":{"uri":lsp::uri(path)}}));
            }
            if action == "lsp_diagnostics" && v["id"] == 1 {
                if !v["error"].is_null() { return Err(v["error"].to_string()); }
                if v["result"]["kind"] != "full" { return Err("Expected a full diagnostic report".into()); }
                return Ok(json!({"uri":uri,"diagnostics":v["result"]["items"]}).to_string());
            }
            if let Some(e) = v["clientError"].as_str() { return Err(e.into()); }
            if v["id"] == 1 && !method.is_empty() {
                if !v["error"].is_null() { return Err(v["error"].to_string()); }
                return Ok(json!({"uri":uri,"result":v["result"]}).to_string());
            }
            if action == "lsp_diagnostics" && v["method"] == "textDocument/publishDiagnostics" && v["params"]["uri"] == uri {
                return Ok(json!({"uri":uri,"diagnostics":v["params"]["diagnostics"]}).to_string());
            }
        }
        if start.elapsed().as_secs() >= 20 { return Err("Language-server tool timed out before a result. Diagnostics are unknown.".into()); }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tools_use_real_stdio_frames_and_report_missing_servers() {
        let root = std::env::temp_dir().join(format!("ld-ab9-tool-lsp-{}",std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        let server = r#"import json,sys
while True:
 h={}
 while True:
  l=sys.stdin.buffer.readline()
  if not l:sys.exit()
  if l==b'\r\n':break
  k,v=l.decode().split(':',1);h[k.lower()]=v.strip()
 m=json.loads(sys.stdin.buffer.read(int(h['content-length'])))
 if 'id' not in m:continue
 r={'capabilities':{}} if m['method']=='initialize' else [{'uri':'file:///tmp/a.rs','range':{'start':m['params']['position'],'end':m['params']['position']}}]
 b=json.dumps({'id':m['id'],'result':r}).encode();sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(b)).encode()+b);sys.stdout.buffer.flush()
"#;
        std::fs::write(root.join("server.py"),server).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),json!({"rs":["python3",root.join("server.py")],"md":false}).to_string()).unwrap();
        let job = Job::language(root.clone(),root.join("a.rs"),"a😀b".into(),"lsp_definition".into(),1,3,String::new(),Arc::new(||{}));
        let (out,ok) = job.wait().unwrap(); assert!(ok,"{out}");
        let v: Value = serde_json::from_str(&out).unwrap(); assert_eq!(v["result"][0]["range"]["start"]["character"],3);
        let job = Job::language(root.clone(),root.join("a.md"),"text".into(),"lsp_diagnostics".into(),1,1,String::new(),Arc::new(||{}));
        assert!(!job.wait().unwrap().1);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    #[test]
    #[ignore = "requires clangd on PATH"]
    fn clangd_definitions_and_diagnostics() {
        let root = std::env::temp_dir().join(format!("ld-ab9-clangd-{}",std::process::id()));
        std::fs::create_dir_all(root.join(".lightdown")).unwrap();
        std::fs::write(root.join(".lightdown/lsp.json"),r#"{"c":["clangd","--background-index=false"]}"#).unwrap();
        let path = root.join("main.c");
        let text = "int helper(void) { return 1; }\nint main(void) { return helper(); }\n";
        std::fs::write(&path,text).unwrap();
        let job = Job::language(root.clone(),path.clone(),text.into(),"lsp_definition".into(),2,26,String::new(),Arc::new(||{}));
        let (out,ok) = job.wait().unwrap(); assert!(ok,"{out}");
        let v:Value = serde_json::from_str(&out).unwrap(); assert_eq!(v["result"][0]["range"]["start"]["line"],0,"{out}");
        let job = Job::language(root.clone(),path,"int main(void) { return missing; }\n".into(),"lsp_diagnostics".into(),1,1,String::new(),Arc::new(||{}));
        let (out,ok) = job.wait().unwrap(); assert!(ok,"{out}");
        assert!(out.contains("missing"),"{out}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
