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
