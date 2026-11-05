use super::*;

fn named(name: &str, place: Place) -> &'static ConfigFile {
    FILES.iter().find(|f| f.name == name && f.place == place).unwrap()
}

fn errors(name: &str, place: Place, text: &str) -> Vec<String> {
    validate(text, &named(name, place).schema()).into_iter().filter(|m| !m.warning).map(|m| m.message).collect()
}

#[test]
fn every_example_passes_its_own_schema() {
    for f in FILES {
        let marks = validate(&f.example(), &f.schema());
        assert!(marks.is_empty(), "{} {:?}: {marks:?}", f.name, f.place);
    }
}

#[test]
fn syntax_types_ranges_enums_and_required_keys_are_marked() {
    let p = Place::Project;
    let m = validate("{\"ruler\": 80,}", &named("settings.json", p).schema());
    assert!(m[0].message.starts_with("JSON: Trailing comma"), "{m:?}");
    assert_eq!(m[0].start, 13, "at the brace after the comma");
    assert_eq!(errors("settings.json", p, r#"{"font_size": 99}"#), ["`font_size` must be at most 40"]);
    assert_eq!(errors("settings.json", p, r#"{"ruler": 81}"#), ["`ruler` must be one of 0, 80, 100"]);
    assert_eq!(errors("settings.json", p, r#"{"format_on_save": 3}"#), ["`format_on_save` must be true or false"]);
    assert_eq!(errors("remote.json", p, "{}"), ["The file needs `host`", "The file needs `path`"]);
    // Unknown and repeated keys are warnings, with a guess at the meant key.
    let m = validate("{\"rulr\": 80, \"ruler\": 80, \"ruler\": 100}", &named("settings.json", p).schema());
    assert!(m.iter().all(|m| m.warning));
    assert!(m[0].message.contains("did you mean `ruler`"), "{m:?}");
    assert!(m[1].message.contains("repeated"));
    // Marks point at the value, nested.
    let text = r#"{"configurations": [{"name": "a", "command": "x", "args": [1]}]}"#;
    let m = validate(text, &named("run.json", p).schema());
    assert_eq!(&text[m[0].start..m[0].end], "1");
    assert_eq!(m[0].message, "`configurations[0].args[0]` must be a string");
}

#[test]
fn global_settings_are_lenient_like_their_loader() {
    let m = validate(r#"{"font_size": 99, "keys": {"save": ["ctrl+s"], "nope": []}}"#, &named("settings.json", Place::User).schema());
    assert_eq!(m.len(), 2, "{m:?}");
    assert!(m.iter().all(|m| m.warning));
    assert!(m[0].message.contains("clamps"));
    assert!(m[1].message.contains("Unknown key `keys.nope`"));
}

#[test]
fn files_are_matched_by_place() {
    let dir = crate::config::config_dir();
    assert_eq!(file_for(&dir.join("settings.json")).unwrap().place, Place::User);
    assert_eq!(file_for(Path::new("/p/.lightdown/settings.json")).unwrap().place, Place::Project);
    assert!(file_for(Path::new("/p/settings.json")).is_none());
    assert!(file_for(&dir.join("lsp.json")).is_none());
    assert_eq!(file_for(&dir.join("snippets.json")).unwrap().place, Place::User);
}

#[test]
fn completion_offers_keys_at_their_level_and_enum_values() {
    let s = named("settings.json", Place::User).schema();
    let text = "{\"auto_save\": \"off\", \"projects\": {\"/p\": {\"ed";
    let at = spot(text, text.len()).unwrap();
    assert!(at.on_key && at.quoted);
    let items = complete(&s, &at, "ed");
    assert_eq!(items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["edit"]);
    let text = "{\"ruler\": ";
    let items = complete(&s, &spot(text, text.len()).unwrap(), "");
    assert_eq!(items.iter().map(|i| i.insert.as_str()).collect::<Vec<_>>(), ["0", "80", "100"]);
    let text = "{\"theme\": \"l";
    let items = complete(&s, &spot(text, text.len()).unwrap(), "l");
    assert_eq!(items[0].insert, "light");
    // Keys already written are not offered again; shortcut ids are.
    let text = "{\"theme\": \"dark\", ";
    let items = complete(&s, &spot(text, text.len()).unwrap(), "");
    assert!(!items.iter().any(|i| i.label == "theme") && items.iter().any(|i| i.insert == "\"auto_save\""));
    let text = "{\"keys\": {\"sav";
    let items = complete(&s, &spot(text, text.len()).unwrap(), "sav");
    assert!(items.iter().any(|i| i.label == "save" && i.detail.starts_with("File: Save")));
    let d = named("debug.json", Place::Project).schema();
    let text = "{\"transport\": ";
    let items = complete(&d, &spot(text, text.len()).unwrap(), "");
    assert_eq!(items.iter().map(|i| i.insert.as_str()).collect::<Vec<_>>(), ["\"stdio\"", "\"tcp\""]);
    let c = named("commands.json", Place::Project).schema();
    let text = "[{\"name\": \"x\", \"output\": \"";
    let items = complete(&c, &spot(text, text.len()).unwrap(), "");
    assert_eq!(items.len(), 3);
    let text = "{\"format_on_save\": t";
    let items = complete(&s, &spot(text, text.len()).unwrap(), "t");
    assert_eq!(items[0].insert, "true");
}

#[test]
fn hover_describes_a_key_at_its_level() {
    let s = named("settings.json", Place::User).schema();
    let text = "{\n  \"projects\": {\"/p\": {\"edit\": \"ask\"}},\n  \"ruler\": 80\n}";
    let (key, schema) = key_at(&s, text, text.find("edit").unwrap() + 1).unwrap();
    assert_eq!(key, "edit");
    assert!(describe(&schema).contains("One of: \"ask\", \"allow\", \"deny\""));
    let (_, schema) = key_at(&s, text, text.find("ruler").unwrap()).unwrap();
    assert!(describe(&schema).starts_with("The column of the ruler"));
    assert!(key_at(&s, text, text.find("ask").unwrap()).is_none(), "values are not keys");
}

#[test]
fn snippet_bodies_and_patterns_are_checked() {
    let p = Place::Project;
    assert_eq!(errors("snippets.json", p, r#"{"rs": [{"prefix": "x", "body": "${1:no"}]}"#), ["`rs[0].body`: Unclosed snippet default"]);
    assert_eq!(errors("snippets.json", p, r#"{".rs": []}"#).len(), 1);
    assert_eq!(errors("grammar.json", p, r#"{"url": "https://example.com"}"#).len(), 1);
    assert_eq!(errors("mcp.json", p, r#"{"mcpServers": {"a": {"args": []}}}"#), ["`mcpServers.a` needs `command` or `url`"]);
    assert!(errors("mcp.json", p, r#"{"mcpServers": {"a": {"disabled": true}}}"#).is_empty());
    assert_eq!(errors("commands.json", p, r#"[{"name": "a", "command": "x", "key": "k"}]"#).len(), 1);
    assert_eq!(errors("run.json", p, r#"{"configurations": [{"name": "a", "command": "x"}, {"name": "a", "command": "y"}]}"#), ["Two items have name \"a\""]);
}

#[test]
fn remote_paths_and_hosts_agree_with_the_loader() {
    let s = named("remote.json", Place::Project).schema();
    for host in ["devbox", "user@build-machine", "a.b_c-d", "-bad", "a@b@c", "sp ace", "", "x;y"] {
        for path in ["/srv/project", "/a/../b", "relative", "/a/..b/.c/.", "/", "/a\u{7}b", "/.."] {
            let v = serde_json::json!({"host": host, "path": path});
            let text = v.to_string();
            let ok = validate(&text, &s).is_empty();
            assert_eq!(ok, crate::remote::Remote::parse(&v).is_ok(), "{text}");
        }
    }
}

#[test]
fn files_written_by_lightdown_and_its_documented_forms_pass() {
    // The settings as Lightdown saves them, so a key added there without a
    // schema entry shows up here rather than as a warning in the editor.
    let saved = serde_json::to_string_pretty(&crate::config::Config::default().saved()).unwrap();
    let marks = validate(&saved, &named("settings.json", Place::User).schema());
    assert!(marks.is_empty(), "{marks:?}");
    let p = Place::Project;
    let ok = |name: &str, text: &str| {
        let marks = validate(text, &named(name, p).schema());
        assert!(marks.is_empty(), "{name}: {text}: {marks:?}");
    };
    ok("settings.json", r#"{"check": ["cargo", "clippy"], "check_attempts": 3, "code_actions_on_save": ["source.organizeImports"]}"#);
    ok("settings.json", r#"{"check": "cargo check && cargo test"}"#);
    ok("debug.json", r#"{"adapter": ["lldb-dap"], "transport": "stdio", "launch": {"program": "/p/app"}}"#);
    ok("debug.json", r#"{"type": "gdb", "preLaunchTask": "make build", "launch": {"program": "/p/app"}}"#);
    ok("run.json", r#"{"configurations": [{"name": "a", "command": "x"}, {"name": "both", "compound": ["a"]}]}"#);
    ok("remote.json", r#"{"host": "h", "path": "/srv", "pushOnSave": true}"#);
    let ws = validate(r#"{"folders": [{"path": "app"}, {"path": "../lib", "name": "library"}]}"#, &schema(Path::new("/w/x.lightdown-workspace")).unwrap());
    assert!(ws.is_empty(), "{ws:?}");
    ok("container.json", r#"{"build": {"dockerfile": "Dockerfile"}, "runArgs": ["--network=host"], "remoteUser": "dev"}"#);
    // And the mistakes the loaders refuse.
    assert!(!errors("debug.json", p, r#"{"launch": {}}"#).is_empty(), "needs adapter, type or configurations");
    assert!(!errors("debug.json", p, r#"{"configurations": [{"name": "a", "launch": {}}]}"#).is_empty());
    assert!(!errors("tasks.json", p, r#"{"tasks": [{"name": "a"}]}"#).is_empty(), "needs argv or shell");
    assert!(!errors("tasks.json", p, r#"{"tasks": [{"name": "a", "argv": ["x"], "watch": ["../up"]}]}"#).is_empty());
    assert!(!errors("tasks.json", p, r#"{"tasks": [{"name": "a", "argv": ["x"], "background": {}}]}"#).is_empty());
    assert!(!errors("settings.json", p, r#"{"check_attempts": 11}"#).is_empty());
    assert!(!errors("container.json", p, r#"{"workspaceFolder": "/w"}"#).is_empty(), "needs image or build");
}

#[test]
fn every_schema_pattern_compiles() {
    // A pattern that does not compile is skipped when checking, so it would
    // quietly accept everything.
    fn walk(v: &Value, name: &str) {
        match v {
            Value::Object(m) => {
                if let Some(p) = m.get("pattern").and_then(Value::as_str) {
                    assert!(regex::Regex::new(p).is_ok(), "{name}: {p}");
                }
                m.values().for_each(|v| walk(v, name));
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, name)),
            _ => {}
        }
    }
    for f in FILES {
        walk(&f.schema(), f.name);
    }
}

#[test]
fn keymap_schema_validates_contexts_sequences_and_command_ids() {
    let s=named("keymap.json",Place::User).schema();
    for text in [r#"[{"context":"Editor && mode == source","bindings":{"ctrl-k ctrl-s":"save","ctrl-alt-s":null}}]"#,
        r#"[{"context":"Explorer","bindings":{"r":"rename"}}]"#] {assert!(validate(text,&s).is_empty());}
    for text in [r#"[{"context":"Editor &&","bindings":{}}]"#,r#"[{"bindings":{"ctrl-x":"does_not_exist"}}]"#,
        r#"[{"bindings":{"not-a-key":"save"}}]"#] {assert!(validate(text,&s).iter().any(|m|!m.warning),"{text}");}
}

#[test]
fn legacy_keys_schema_accepts_both_spellings_and_chords() {
    let s=named("settings.json",Place::User).schema();
    for code in ["ctrl+s","ctrl-k ctrl-s","C-x C-s","ctrl+shift+s"] {
        let text=serde_json::json!({"keys":{"save":[code]}}).to_string();
        assert!(validate(&text,&s).is_empty(),"{code}");
    }
    let text=r#"{"keys":{"save":["not-a-key"]}}"#;
    assert!(validate(text,&s).iter().any(|m|m.message.contains("key sequence")));
}
