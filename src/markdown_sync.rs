//! Map SyncTeX's generated TeX lines back to the markdown build snapshot.
use std::path::{Path, PathBuf};
use serde_json::{Value, json};

pub struct Map {
    pub source: PathBuf,
    pub generated: PathBuf,
    pub text: String,
    lines: Vec<usize>,
}
impl Map {
    pub fn new(source: PathBuf, generated: PathBuf, text: String, tex: &str) -> Self {
        let mut current = 1;
        let lines = tex.lines().map(|line| {
            if let Some(n) = line.strip_prefix("%lightdown-source:").and_then(|s| s.parse().ok()) { current = n; }
            current
        }).collect();
        Self { source, generated, text, lines }
    }
    pub fn save(&self, pdf: &Path) -> std::io::Result<()> {
        crate::config::write_private(&pdf.with_extension("lightdown-map.json"),
            serde_json::to_string(&json!({"source": self.source, "generated": self.generated,
                "text": self.text, "lines": self.lines}))?.as_bytes())
    }
    pub fn open(pdf: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(pdf.with_extension("lightdown-map.json")).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        Some(Self { source: PathBuf::from(v["source"].as_str()?), generated: PathBuf::from(v["generated"].as_str()?),
            text: v["text"].as_str()?.into(),
            lines: v["lines"].as_array()?.iter().map(|v| v.as_u64().map(|v| v as usize)).collect::<Option<_>>()? })
    }
    pub fn inverse(&self, line: usize) -> Option<usize> { self.lines.get(line.saturating_sub(1)).copied() }
    pub fn forward(&self, line: usize) -> Option<usize> {
        self.lines.iter().position(|n| *n >= line).map(|n| n+1)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_blocks_map_to_markdown_lines_without_changing_export() {
        let source = "# Heading\n\nFirst paragraph.\nSecond line.\n\n```python\nprint('x')\n```\n";
        let tex = crate::export::latex_mapped(source, None, "t");
        let map = Map::new("source.md".into(), "generated.tex".into(), source.into(), &tex);
        let generated = tex.lines().position(|l| l.starts_with("Second line")).unwrap()+1;
        assert_eq!(map.inverse(generated), Some(4));
        let generated = tex.lines().position(|l| l.starts_with("print")).unwrap()+1;
        assert_eq!(map.inverse(generated), Some(6));
        assert!(map.forward(3).is_some());
        assert!(!crate::export::latex(source, None, "t").contains("%lightdown-source:"));
    }
}
