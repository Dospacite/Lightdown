//! Disk symbols are parsed on the existing project-index background thread.
use super::*;
use std::time::SystemTime;

struct Cached {
    path: PathBuf,
    modified: SystemTime,
    bytes: u64,
    grammar: &'static str,
    items: Vec<crate::outline::Item>,
}
#[derive(Default)]
struct Index {
    generation: u64,
    entries: VecDeque<Cached>,
    loaded: HashMap<&'static str, Result<Arc<Loaded>, String>>,
}
static INDEX: OnceLock<Mutex<Index>> = OnceLock::new();

/// Call only from the project indexer. Unsupported or failed grammars retain lexical symbols.
pub fn symbols(path: &Path, text: &str) -> Option<Vec<crate::outline::Item>> {
    if text.len() > MAX {
        return None;
    }
    let language = path.extension()?.to_str()?.to_ascii_lowercase();
    let g = for_language(&language)?;
    if !installed(g) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let generation = state().generation;
    let mut index = INDEX.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if index.generation != generation {
        index.entries.clear();
        index.loaded.clear();
        index.generation = generation;
    }
    if let Some(i) = index
        .entries
        .iter()
        .position(|e| e.path == path && e.modified == modified && e.bytes == meta.len() && e.grammar == g.name)
    {
        let entry = index.entries.remove(i)?;
        let result = entry.items.clone();
        index.entries.push_back(entry);
        return Some(result);
    }
    let l = index
        .loaded
        .entry(g.name)
        .or_insert_with(|| load(g, &directory().join(g.name)).map(Arc::new))
        .as_ref()
        .ok()?
        .clone();
    let result = parse(&l, text, None).ok()?.outline;
    index.entries.retain(|e| e.path != path);
    while index.entries.len() >= CACHE_ENTRIES || index.entries.iter().map(|e| e.items.len()).sum::<usize>() > 100_000 {
        index.entries.pop_front();
    }
    index.entries.push_back(Cached {
        path: path.into(),
        modified,
        bytes: meta.len(),
        grammar: g.name,
        items: result.clone(),
    });
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Downloads JSON grammar and compiles with cc"]
    fn grammars_build_project_cache_tracks_file_changes() {
        super::super::tests::ready("json");
        let path = std::env::temp_dir().join(format!("lightdown-symbol-cache-{}.json", std::process::id()));
        let first = "{\"first\": {\"nested\": 1}}";
        std::fs::write(&path, first).unwrap();
        let items = symbols(&path, first).unwrap();
        assert_eq!(items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), ["\"first\"", "\"nested\""]);
        // Same stamp reuses the parsed entries.
        assert_eq!(symbols(&path, "invalid but same file metadata").unwrap(), items);
        let second = "{\"second_longer\": 2}";
        std::fs::write(&path, second).unwrap();
        assert_eq!(symbols(&path, second).unwrap()[0].title, "\"second_longer\"");
        std::fs::remove_file(path).unwrap();
    }
}
