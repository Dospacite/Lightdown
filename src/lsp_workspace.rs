//! Ordered WorkspaceEdit planning, validation and rollback before touching editor tabs.
use crate::lsp;
use serde_json::Value;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    File(Vec<u8>, u32),
    Directory,
}
#[derive(Default)]
pub struct Plan {
    pub before: BTreeMap<PathBuf, Option<Node>>,
    pub after: BTreeMap<PathBuf, Option<Node>>,
    pub moves: Vec<(PathBuf, PathBuf)>,
    pub deleted: Vec<PathBuf>,
}
fn target(root: &Path, uri: &str) -> Result<PathBuf, String> {
    let p = lsp::path(uri).ok_or("Workspace edit needs a local file URI")?;
    if p.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || !p.starts_with(root)
        || p == root
    {
        return Err("Workspace edit is outside this project".into());
    }
    let mut ancestor = Some(p.as_path());
    while let Some(a) = ancestor {
        if a.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Workspace edit cannot follow symlinks".into());
        }
        if a == root {
            break;
        }
        ancestor = a.parent();
    }
    Ok(p)
}
impl Plan {
    fn load(&mut self, p: &Path) -> Result<(), String> {
        if self.before.contains_key(p) {
            return Ok(());
        }
        let node = match std::fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_symlink() => return Err("Workspace edit cannot follow symlinks".into()),
            Ok(m) if m.is_dir() => Some(Node::Directory),
            Ok(m) if m.is_file() && m.len() <= 32 << 20 => {
                Some(Node::File(std::fs::read(p).map_err(|e| e.to_string())?, m.permissions().mode()))
            }
            Ok(_) => return Err("Workspace edit target is not a regular file or is too large".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        self.before.insert(p.into(), node.clone());
        self.after.entry(p.into()).or_insert(node.clone());
        if node == Some(Node::Directory) {
            for entry in std::fs::read_dir(p).map_err(|e| e.to_string())? {
                self.load(&entry.map_err(|e| e.to_string())?.path())?;
            }
        }
        if self.before.len() > 10000
            || self
                .before
                .values()
                .flatten()
                .map(|n| match n {
                    Node::File(b, _) => b.len(),
                    _ => 0,
                })
                .sum::<usize>()
                > 64 << 20
        {
            return Err("Workspace edit exceeds 64 MB or 10,000 files".into());
        }
        Ok(())
    }
    fn parents(&mut self, root: &Path, p: &Path) -> Result<(), String> {
        if let Some(parent) = p.parent().filter(|p| *p != root) {
            self.parents(root, parent)?;
            // Avoid recursively loading unrelated siblings just to inspect a parent.
            if !parent.exists() && !self.after.contains_key(parent) {
                self.before.insert(parent.into(), None);
                self.after.insert(parent.into(), Some(Node::Directory));
            }
            if parent.is_file() || self.after.get(parent) == Some(&None) {
                return Err("Workspace edit parent is not a directory".into());
            }
        }
        Ok(())
    }
    pub fn build(root: &Path, edit: &Value, buffers: &BTreeMap<PathBuf, (u64, String)>) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let mut plan = Self::default();
        let mut touched = std::collections::HashSet::new();
        let mut changes = Vec::new();
        if let Some(c) = edit.get("documentChanges") {
            changes = c.as_array().ok_or("Invalid documentChanges")?.clone();
        } else if let Some(c) = edit.get("changes") {
            for (uri, edits) in c.as_object().ok_or("Invalid changes")? {
                changes.push(serde_json::json!({"textDocument":{"uri":uri},"edits":edits}));
            }
        }
        for change in changes {
            let kind = change["kind"].as_str().unwrap_or("text");
            let uri = if kind == "rename" {
                &change["oldUri"]
            } else if kind == "text" {
                &change["textDocument"]["uri"]
            } else {
                &change["uri"]
            };
            let path = target(&root, uri.as_str().ok_or("Missing edit URI")?)?;
            plan.load(&path)?;
            match kind {
                "text" => {
                    let Some(Node::File(bytes, mode)) = plan.after.get(&path).cloned().flatten() else {
                        return Err("Text edit target does not exist".into());
                    };
                    let mut text =
                        String::from_utf8(bytes).map_err(|_| "Text edit target is not UTF-8")?.replace("\r\n", "\n");
                    let mut original = path.clone();
                    for (old, new) in plan.moves.iter().rev() {
                        if let Ok(rest) = original.strip_prefix(new) {
                            original = if rest.as_os_str().is_empty() { old.clone() } else { old.join(rest) };
                        }
                    }
                    if !touched.contains(&path) {
                        if let Some((_, buffer)) = buffers.get(&original) {
                            text = buffer.clone();
                        }
                    }
                    if let Some(version) = change["textDocument"]["version"].as_u64() {
                        if buffers.get(&original).is_none_or(|(v, _)| *v != version) {
                            return Err("Workspace edit version is stale".into());
                        }
                    }
                    touched.insert(path.clone());
                    let edits = change["edits"].as_array().ok_or("Missing text edits")?;
                    let out = lsp::edits(&text, edits)?;
                    plan.after.insert(path, Some(Node::File(out.into_bytes(), mode)));
                }
                "create" => {
                    let exists = plan.after[&path].is_some();
                    let overwrite = change["options"]["overwrite"] == true;
                    if exists && !overwrite {
                        if change["options"]["ignoreIfExists"] == true {
                            continue;
                        }
                        return Err("CreateFile target exists".into());
                    }
                    if plan.after[&path] == Some(Node::Directory) {
                        return Err("CreateFile target is a directory".into());
                    }
                    plan.parents(&root, &path)?;
                    let mode = match plan.after.get(&path) {
                        Some(Some(Node::File(_, mode))) => *mode,
                        _ => 0o644,
                    };
                    touched.insert(path.clone());
                    plan.after.insert(path, Some(Node::File(Vec::new(), mode)));
                }
                "rename" => {
                    let new = target(&root, change["newUri"].as_str().ok_or("Missing new URI")?)?;
                    if new == path {
                        continue;
                    }
                    if new.starts_with(&path) {
                        return Err("Cannot move a folder into itself".into());
                    }
                    plan.load(&new)?;
                    if plan.after[&path].is_none() {
                        return Err("RenameFile source is missing".into());
                    }
                    if plan.after[&new].is_some() && change["options"]["overwrite"] != true {
                        if change["options"]["ignoreIfExists"] == true {
                            continue;
                        }
                        return Err("RenameFile target exists".into());
                    }
                    plan.parents(&root, &new)?;
                    let removed: Vec<_> = plan.after.keys().filter(|p| p.starts_with(&new)).cloned().collect();
                    for p in removed {
                        plan.after.insert(p, None);
                    }
                    let moved: Vec<_> = plan
                        .after
                        .iter()
                        .filter(|(p, n)| p.starts_with(&path) && n.is_some())
                        .map(|(p, n)| (p.clone(), n.clone()))
                        .collect();
                    for (old, mut node) in moved {
                        if !touched.contains(&old) {
                            if let (Some((_, text)), Some(Node::File(_, mode))) = (buffers.get(&old), node.as_ref()) {
                                node = Some(Node::File(text.as_bytes().to_vec(), *mode));
                            }
                        }
                        let dest = new.join(old.strip_prefix(&path).unwrap());
                        plan.load(&dest)?;
                        touched.insert(dest.clone());
                        touched.insert(old.clone());
                        plan.after.insert(dest, node);
                        plan.after.insert(old, None);
                    }
                    plan.moves.push((path, new));
                }
                "delete" => {
                    if plan.after[&path].is_none() {
                        if change["options"]["ignoreIfNotExists"] == true {
                            continue;
                        }
                        return Err("DeleteFile target is missing".into());
                    }
                    let entries: Vec<_> = plan.after.keys().filter(|p| p.starts_with(&path)).cloned().collect();
                    if entries.len() > 1 && change["options"]["recursive"] != true {
                        return Err("DeleteFile needs recursive for a nonempty directory".into());
                    }
                    for p in entries {
                        plan.after.insert(p, None);
                    }
                    plan.deleted.push(path);
                }
                _ => return Err(format!("Unknown workspace operation {kind}")),
            }
        }
        Ok(plan)
    }
    /// Call checkpoints before this. On an I/O error restore all captured paths.
    pub fn apply(&self) -> Result<(), String> {
        for (p, expected) in &self.before {
            if p.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("Workspace target became a symlink before applying".into());
            }
            let actual = if p.is_file() {
                Some(Node::File(
                    std::fs::read(p).map_err(|e| e.to_string())?,
                    std::fs::metadata(p).map_err(|e| e.to_string())?.permissions().mode(),
                ))
            } else if p.is_dir() {
                Some(Node::Directory)
            } else {
                None
            };
            if &actual != expected {
                return Err("Workspace edit target changed before applying".into());
            }
        }
        if let Err(e) = write_nodes(&self.after) {
            return match write_nodes(&self.before) {
                Ok(()) => Err(format!("Workspace edit rolled back: {e}")),
                Err(rollback) => Err(format!("Workspace edit failed: {e}; rollback failed: {rollback}")),
            };
        }
        Ok(())
    }
}
fn write_nodes(nodes: &BTreeMap<PathBuf, Option<Node>>) -> Result<(), String> {
    for (p, n) in nodes.iter().rev() {
        if n.is_none() {
            let result = if p.is_dir() { std::fs::remove_dir(p) } else { std::fs::remove_file(p) };
            if let Err(e) = result {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(e.to_string());
                }
            }
        }
    }
    for (p, n) in nodes {
        match n {
            Some(Node::Directory) => std::fs::create_dir_all(p).map_err(|e| e.to_string())?,
            Some(Node::File(bytes, mode)) => {
                std::fs::create_dir_all(p.parent().ok_or("Missing parent")?).map_err(|e| e.to_string())?;
                std::fs::write(p, bytes).map_err(|e| e.to_string())?;
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(*mode)).map_err(|e| e.to_string())?;
            }
            None => {}
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn versions_conflicts_symlinks_and_disk_races_leave_files_untouched() {
        let root = std::env::temp_dir().join(format!("ld-workspace-validation-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.rs");
        let b = root.join("b.rs");
        std::fs::write(&a, "abc").unwrap();
        std::fs::write(&b, "keep").unwrap();
        let rename = json!({"documentChanges":[{"kind":"rename","oldUri":lsp::uri(&a),"newUri":lsp::uri(&b)}]});
        assert!(Plan::build(&root, &rename, &BTreeMap::new()).is_err());
        let mut overwrite = rename.clone();
        overwrite["documentChanges"][0]["options"] = json!({"overwrite":true,"ignoreIfExists":true});
        let plan = Plan::build(&root, &overwrite, &BTreeMap::new()).unwrap();
        std::fs::write(&a, "later").unwrap();
        assert!(plan.apply().is_err());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "keep");
        let text_edit = json!({"documentChanges":[{"textDocument":{"uri":lsp::uri(&a),"version":2},"edits":[]}]});
        let buffers = BTreeMap::from([(a.clone(), (1, "unsaved".into()))]);
        assert!(Plan::build(&root, &text_edit, &buffers).is_err());
        let link = root.join("link");
        std::os::unix::fs::symlink(&b, &link).unwrap();
        assert!(
            Plan::build(&root, &json!({"documentChanges":[{"kind":"delete","uri":lsp::uri(&link)}]}), &BTreeMap::new())
                .is_err()
        );
        let delete = json!({"documentChanges":[{"kind":"delete","uri":lsp::uri(&root.join("missing")),"options":{"ignoreIfNotExists":true}}]});
        Plan::build(&root, &delete, &BTreeMap::new()).unwrap().apply().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ordered_create_rename_edit_delete_and_options_are_atomic() {
        let root = std::env::temp_dir().join(format!("ld-workspace-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.rs");
        let b = root.join("sub/b.rs");
        let edit = json!({"documentChanges":[{"kind":"create","uri":lsp::uri(&a)},{"kind":"rename","oldUri":lsp::uri(&a),"newUri":lsp::uri(&b)},{"textDocument":{"uri":lsp::uri(&b)},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"hi😀"}]}]});
        Plan::build(&root, &edit, &BTreeMap::new()).unwrap().apply().unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "hi😀");
        let invalid =
            json!({"documentChanges":[{"kind":"delete","uri":lsp::uri(&b)},{"kind":"create","uri":"file:///outside"}]});
        assert!(Plan::build(&root, &invalid, &BTreeMap::new()).is_err());
        assert!(b.exists());
        let ignore = json!({"documentChanges":[{"kind":"create","uri":lsp::uri(&b),"options":{"ignoreIfExists":true}},{"kind":"delete","uri":lsp::uri(&root.join("sub")),"options":{"recursive":true}}]});
        Plan::build(&root, &ignore, &BTreeMap::new()).unwrap().apply().unwrap();
        assert!(!b.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
