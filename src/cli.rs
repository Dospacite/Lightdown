//! CLI targets keep native paths and accept a one-based line and column.
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}
impl Target {
    pub fn parse(arg: std::ffi::OsString) -> Self {
        let mut path = arg.to_str().filter(|s| s.starts_with("file://"))
            .map(|s| PathBuf::from(crate::pane::percent_decode(&s[7..])))
            .unwrap_or_else(|| PathBuf::from(arg));
        let (mut line, mut column) = (1, 1);
        // An existing filename containing colons always wins.
        if !path.exists() {
            if let Some(s) = path.to_str() {
                if let Some((prefix, n)) = s.rsplit_once(':') {
                    if let Ok(last) = n.parse::<usize>() {
                        let (file, l, c) = match prefix.rsplit_once(':') {
                            Some((file, l)) if l.parse::<usize>().is_ok() => (file, l.parse().unwrap(), last),
                            _ => (prefix, last, 1),
                        };
                        if !file.is_empty() {
                            (line, column) = (l.max(1), c.max(1));
                            path = PathBuf::from(file);
                        }
                    }
                }
            }
        }
        path = path.canonicalize().unwrap_or_else(|_| std::path::absolute(&path).unwrap_or(path));
        Self { path, line, column }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_positions_uris_and_native_names() {
        let t = Target::parse("source.rs:12:5".into());
        assert!(t.path.ends_with("source.rs"));
        assert_eq!((t.line, t.column), (12, 5));
        let t = Target::parse("source.rs:0".into());
        assert_eq!((t.line, t.column), (1, 1));
        let t = Target::parse("file:///tmp/a%20b.rs:20".into());
        assert_eq!(t.path, PathBuf::from("/tmp/a b.rs"));
        assert_eq!(t.line, 20);
        use std::os::unix::ffi::OsStringExt;
        assert!(Target::parse(std::ffi::OsString::from_vec(vec![b'a', 255])).path.is_absolute());
    }
    #[test]
    fn existing_colons_are_part_of_the_name() {
        let p = std::env::temp_dir().join(format!("ld-colon-{}:12", std::process::id()));
        std::fs::write(&p, "hi").unwrap();
        let t = Target::parse(p.clone().into_os_string());
        assert_eq!(t.path, p);
        assert_eq!(t.line, 1);
        std::fs::remove_file(p).unwrap();
    }
}
