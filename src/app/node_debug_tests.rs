//! Opt-in integration checks against the real standalone js-debug server.
//! LIGHTDOWN_TEST_JS_DEBUG=/path/to/js-debug/src/dapDebugServer.js
use super::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn wait(a: &mut App, condition: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !condition(a) && start.elapsed() < Duration::from_secs(20) {
        a.debug_poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(condition(a), "message={:?}\nsnapshot={}\noutput={}", a.message, a.debugger.snapshot, a.debugger.output);
}
fn config(request: &str, launch: Value) -> Value {
    let script = std::env::var("LIGHTDOWN_TEST_JS_DEBUG").expect("Set LIGHTDOWN_TEST_JS_DEBUG to dapDebugServer.js");
    json!({"adapter":["node",script,"{port}","127.0.0.1"],"transport":"tcp","request":request,"launch":launch,"exceptionFilters":["all"]})
}
const PROGRAM: &str = "function add(value) {\n  const result = value + 1;\n  return result;\n}\nfunction run() {\n  const answer = 41;\n  const object = {answer};\n  const total = add(answer);\n  console.log(total);\n  throw new Error('boom');\n}\nrun();\n";
fn inspect(a: &mut App, source: &Path) {
    wait(a, |a| {
        a.debugger.paused
            && a.debugger.snapshot["stack"][0]["line"] == 8
            && a.debugger.snapshot["variables"].as_array().is_some_and(|v| !v.is_empty())
    });
    assert_eq!(a.debugger.snapshot["stack"][0]["path"], source.to_string_lossy().as_ref());
    assert!(a.debugger.snapshot["threads"][0]["id"].as_i64().unwrap() > (1 << 40));
    let thread = a.debugger.snapshot["threads"][0]["id"].as_i64().unwrap();
    a.debugger.snapshot["variables"] = json!([]);
    a.native_thread(thread).unwrap();
    wait(a, |a| a.debugger.snapshot["variables"].as_array().is_some_and(|vs| vs.iter().any(|v| v["name"] == "object")));
    let object = a.debugger.snapshot["variables"].as_array().unwrap().iter().find(|v| v["name"] == "object").unwrap()["variablesReference"].as_i64().unwrap();
    a.native_expand(object).unwrap();
    wait(a, |a| {
        a.debugger.snapshot["expanded"][object.to_string()]
            .as_array()
            .is_some_and(|v| v.iter().any(|v| v["name"] == "answer" && v["value"] == "41"))
    });
    a.native_evaluate("answer + 1");
    wait(a, |a| a.debugger.output.contains("42\n"));
    a.native_command("step");
    wait(a, |a| a.debugger.paused && a.debugger.snapshot["stack"][0]["line"] == 2);
    a.native_command("out");
    wait(a, |a| a.debugger.paused && a.debugger.snapshot["stack"][0]["line"] == 9);
    a.native_command("next");
    wait(a, |a| a.debugger.paused && a.debugger.snapshot["stack"][0]["line"] == 10);
    a.native_command("continue");
    wait(a, |a| {
        a.debugger.paused && super::debug_exceptions::summary(&a.debugger.snapshot).is_some_and(|s| s.contains("boom"))
    });
}
#[test]
#[ignore = "requires Node and standalone js-debug; set LIGHTDOWN_TEST_JS_DEBUG"]
fn real_node_launch_steps_variables_exceptions_and_pid_attach() {
    let root = std::env::temp_dir().join(format!("ld-real-node-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let source = root.join("main.js");
    std::fs::write(&source, PROGRAM).unwrap();
    let mut a = App::new(Some(source.clone()));
    a.resize(1200, 800, 1.0);
    a.debugger.points = vec![(source.clone(), 8)];
    a.native_launch(config("launch", json!({"type":"pwa-node","program":source,"cwd":root,"console":"internalConsole","skipFiles":["<node_internals>/**"]})));
    inspect(&mut a, &source);
    a.native_command("continue");
    wait(&mut a, |a| a.debugger.dap.is_none());
    // PID attach enables the inspector on a plain running Node process.
    std::fs::write(&source, PROGRAM.replace("run();\n", "setInterval(() => { try { run(); } catch {} }, 300);\n"))
        .unwrap();
    let mut process = std::process::Command::new("node")
        .arg(&source)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    struct Kill<'a>(&'a mut std::process::Child);
    impl Drop for Kill<'_> {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let guard = Kill(&mut process);
    std::thread::sleep(Duration::from_millis(500));
    let mut a = App::new(Some(source.clone()));
    a.resize(1200, 800, 1.0);
    a.debugger.points = vec![(source.clone(), 8)];
    a.native_launch(config(
        "attach",
        json!({"type":"pwa-node","processId":guard.0.id().to_string(),"cwd":root,"skipFiles":["<node_internals>/**"]}),
    ));
    inspect(&mut a, &source);
    a.native_command("stop");
    drop(guard);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "requires Node, tsc and standalone js-debug; set LIGHTDOWN_TEST_JS_DEBUG"]
fn real_typescript_source_maps() {
    let root = std::env::temp_dir().join(format!("ld-real-ts-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let source = root.join("main.ts");
    std::fs::write(&source, PROGRAM.replace("add(value)", "add(value: number)")).unwrap();
    assert!(
        std::process::Command::new("tsc")
            .arg(&source)
            .args(["--sourceMap", "--skipLibCheck", "--target", "es2020", "--outDir"])
            .arg(root.join("out"))
            .status()
            .unwrap()
            .success()
    );
    let mut a = App::new(Some(source.clone()));
    a.resize(1200, 800, 1.0);
    a.debugger.points = vec![(source.clone(), 8)];
    a.native_launch(config("launch", json!({"type":"pwa-node","program":root.join("out/main.js"),"cwd":root,"console":"internalConsole","outFiles":[root.join("out/**/*.js")],"sourceMaps":true,"skipFiles":["<node_internals>/**"]})));
    inspect(&mut a, &source);
    a.native_command("continue");
    wait(&mut a, |a| a.debugger.dap.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn nested_targets_route_colliding_ids_and_child_exit_keeps_parent() {
    let root = std::env::temp_dir().join(format!("ld-child-dap-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let source = root.join("main.js");
    std::fs::write(&source, "// fixture\nlet owner = 1;\n").unwrap();
    let adapter = root.join("adapter.py");
    std::fs::write(&adapter, include_str!("../../assets/test_dap_children.py")).unwrap();
    let mut a = App::new(Some(source.clone()));
    a.resize(1200, 800, 1.0);
    a.debugger.points = vec![(source.clone(), 2)];
    a.native_launch(
        json!({"adapter":["python3",adapter,source,"{port}"],"transport":"tcp","launch":{},"exceptionFilters":["all"]}),
    );
    let mut previous = None;
    for owner in ["one", "two"] {
        wait(&mut a, |a| a.debugger.paused && a.debugger.snapshot["variables"][0]["value"] == owner);
        assert!(a.debugger.dap.is_some());
        let thread =
            a.debugger.snapshot["threads"].as_array().unwrap().iter().find(|t| t["name"] == owner).unwrap()["id"]
                .as_i64()
                .unwrap();
        assert_ne!(Some(thread), previous);
        previous = Some(thread);
        let reference = a.debugger.snapshot["variables"][0]["variablesReference"].as_i64().unwrap();
        a.native_expand(reference).unwrap();
        wait(&mut a, |a| a.debugger.snapshot["expanded"][reference.to_string()][0]["value"] == owner);
        assert_eq!(a.debugger.snapshot["expanded"][reference.to_string()][0]["variablesReference"], 0);
        a.native_evaluate("owner");
        wait(&mut a, |a| a.debugger.output.contains(&format!("owner-{owner}\n")));
        assert!(a.debugger.output.contains(&format!("points-{owner}\n")));
        assert!(a.debugger.output.contains(&format!("filters-{owner}\n")));
        a.native_command("continue");
    }
    wait(&mut a, |a| a.debugger.dap.is_none());
    std::fs::remove_dir_all(root).unwrap();
}
