//! Each fixture exercises the queries against its exact pinned parser.
use super::*;
const SAMPLES: &[(&str, &str)] = &[
    ("bash", "function sample() {\n echo hello\n}\n"),
    ("c", "struct Sample {\n int value;\n};\nint sample(int value) {\n return value;\n}\n"),
    ("cpp", "class Sample {\n int value;\n};\nint sample(int value) {\n return value;\n}\n"),
    ("css", ".sample {\n color: red;\n}\n"),
    ("go", "package sample\nfunc Sample(value int) int {\n return value\n}\n"),
    ("haskell", "class Sample a where\n  sample :: a -> a\n  sample x = x\n"),
    ("html", "<div>\n<p>sample</p>\n</div>\n"),
    ("java", "class Sample {\n int sample(int value) {\n return value;\n }\n}\n"),
    ("javascript", "class Sample {\n sample(value) {\n return value;\n }\n}\n"),
    ("json", "{\n \"sample\": {\n  \"value\": 1\n }\n}\n"),
    ("kotlin", "class Sample {\n fun sample(value: Int): Int {\n return value\n }\n}\n"),
    ("lua", "function sample(value)\n return value\nend\n"),
    ("markdown", "# Sample\ntext\n## Child\nmore\n"),
    ("php", "<?php\nclass Sample {\n function sample($value) {\n return $value;\n }\n}\n"),
    ("python", "class Sample:\n def sample(self, value):\n  return value\n"),
    ("ruby", "class Sample\n def sample(value)\n  value\n end\nend\n"),
    ("rust", "struct Sample {\n value: u8,\n}\nfn sample(value: u8) -> u8 {\n value\n}\n"),
    ("sql", "CREATE TABLE sample (\n value INT,\n other TEXT\n);\n"),
    ("swift", "class Sample {\n func sample(value: Int) -> Int {\n return value\n }\n}\n"),
    ("toml", "[sample]\nvalue = 1\nother = 2\n"),
    ("tsx", "class Sample {\n sample(value: number) {\n return <p>{value}</p>;\n }\n}\n"),
    ("typescript", "class Sample {\n sample(value: number) {\n return value;\n }\n}\n"),
    ("yaml", "sample:\n value: 1\n other: 2\n"),
    ("zig", "fn sample(value: u8) u8 {\n return value;\n}\n"),
];
#[test]
fn every_grammar_has_bundled_queries_and_a_sample() {
    for g in GRAMMARS {
        assert!(SAMPLES.iter().any(|(name, _)| *name == g.name), "{}", g.name);
        for kind in ["outline", "folds", "textobjects", "locals"] {
            assert!(!query_sets::bundled(g.name, kind).is_empty(), "{} {kind}", g.name);
        }
    }
}
#[test]
#[ignore = "Downloads all pinned parsers, compiles with cc, validates and executes every query set"]
fn grammars_build_all_query_samples() {
    super::tests::scratch();
    let mut errors = Vec::new();
    for &(name, text) in SAMPLES {
        eprintln!("checking {name}");
        let g = grammar(name).unwrap();
        let result = (|| -> Result<(), String> {
            if !installed(g) {
                install_sync(g)?;
            }
            let l = load(g, &directory().join(name))?;
            let p = parse(&l, text, None)?;
            if p.outline.is_empty() {
                return Err(format!("empty outline: {}", p.tree.root_node().to_sexp()));
            }
            if p.folds.is_empty() {
                return Err(format!("empty folds: {}", p.tree.root_node().to_sexp()));
            }
            let mut parser = Parser::new();
            parser.set_language(&l.language).unwrap();
            let edited = text.replacen("value", "values", 1);
            let delta = Delta::between(text, &edited);
            let incremental = parse(&l, &edited, Some((delta.input(text, &edited), &p.tree)))?;
            let fresh = parse(&l, &edited, None)?;
            assert_eq!(incremental.tree.root_node().to_sexp(), fresh.tree.root_node().to_sexp(), "{name}");
            assert_eq!(incremental.outline, fresh.outline, "{name}");
            assert_eq!(incremental.folds, fresh.folds, "{name}");
            assert_eq!(incremental.objects, fresh.objects, "{name}");
            assert_eq!(
                incremental.spans.iter().map(|s| (s.start, s.end, s.attr)).collect::<Vec<_>>(),
                fresh.spans.iter().map(|s| (s.start, s.end, s.attr)).collect::<Vec<_>>(),
                "{name}"
            );
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("{name}: {e}");
            errors.push(format!("{name}: {e}"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
