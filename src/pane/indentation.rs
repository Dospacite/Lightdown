//! Bounded indentation inference. EditorConfig overrides this hint.
pub(super) fn detect(text: &str) -> Option<String> {
    let (mut tabs, mut spaces) = (0, 0);
    let mut widths = Vec::new();
    let mut previous = 0;
    for line in text.lines().take(2000) {
        if line.trim().is_empty() { continue; }
        if line.starts_with('\t') { tabs += 1; continue; }
        let n = line.bytes().take_while(|&b| b == b' ').count();
        if n > 0 {
            spaces += 1;
            widths.push(n);
            if n > previous { widths.push(n - previous); }
        }
        previous = n;
    }
    if tabs > spaces { return Some("\t".into()); }
    if spaces == 0 { return None; }
    // Prefer the largest supported unit that divides every observed depth.
    let width = [8, 4, 2].into_iter().find(|w| widths.iter().all(|n| n % w == 0))?;
    Some(" ".repeat(width))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_units_and_ignores_blank_lines() {
        for n in [2, 4, 8] {
            assert_eq!(detect(&format!("root\n{}child\n{}nested\n   \n", " ".repeat(n), " ".repeat(n*2))), Some(" ".repeat(n)));
        }
        assert_eq!(detect("root\n\tchild\n\t\tnested"), Some("\t".into()));
        assert_eq!(detect("no indentation"), None);
        assert_eq!(detect("   odd"), None);
    }
}
