use std::process::Command;

fn run(args: &[&str]) -> std::process::ExitStatus {
    Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(args)
        .status()
        .expect("spawn rune")
}

#[test]
fn uncaught_error_exits_nonzero() {
    assert!(!run(&["-e", "null.x"]).success());
    assert!(!run(&["-e", "let ="]).success());
}

#[test]
fn successful_script_exits_zero() {
    assert!(run(&["-e", "print('ok')"]).success());
}

#[test]
fn timers_run_after_eval_in_cli() {
    let output = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["-e", "setTimeout(function () { print('timer ran'); }, 0);"])
        .output()
        .expect("spawn rune");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("timer ran"));
}

#[test]
fn fetch_blocks_private_hosts_by_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args([
            "-e",
            "fetch('http://127.0.0.1:1/').catch(function (e) { print(e); });",
        ])
        .output()
        .expect("spawn rune");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("private/loopback"),
        "unexpected output: {stdout}"
    );
}

#[test]
fn build_and_run_application_bundle() {
    let dir = std::env::temp_dir().join(format!("rune-bundle-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let source = dir.join("app.js");
    let bundle = dir.join("app.runeapp");
    std::fs::write(
        &source,
        "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }\nprint('fib', fib(12));\n",
    )
    .expect("write source");
    let build = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args([
            "build",
            source.to_str().unwrap(),
            "--out",
            bundle.to_str().unwrap(),
        ])
        .output()
        .expect("spawn build");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["run", bundle.to_str().unwrap()])
        .output()
        .expect("spawn run");
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("fib 144"), "unexpected output: {stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_and_run_module_bundle() {
    let dir = std::env::temp_dir().join(format!("rune-esm-bundle-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let dep = dir.join("dep.mjs");
    let main = dir.join("main.mjs");
    let bundle = dir.join("app.runeapp");
    std::fs::write(
        &dep,
        "export let x = 41;\nexport function bump() { x = x + 1; }\n",
    )
    .expect("write dep");
    std::fs::write(
        &main,
        "import { x, bump } from './dep.mjs';\nbump();\nprint('x', x);\n",
    )
    .expect("write main");
    let build = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args([
            "build",
            main.to_str().unwrap(),
            "--out",
            bundle.to_str().unwrap(),
        ])
        .output()
        .expect("spawn build");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["run", bundle.to_str().unwrap()])
        .output()
        .expect("spawn run");
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("x 42"), "unexpected output: {stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn instruction_budget_stops_infinite_loop() {
    let output = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["--max-instructions=5000", "-e", "while (true) {}"])
        .output()
        .expect("spawn rune");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("instruction budget exceeded"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn time_budget_stops_infinite_loop() {
    let output = Command::new(env!("CARGO_BIN_EXE_rune"))
        .args(["--timeout-ms=20", "-e", "while (true) {}"])
        .output()
        .expect("spawn rune");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("time budget exceeded"),
        "unexpected stderr: {stderr}"
    );
}
