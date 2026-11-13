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
