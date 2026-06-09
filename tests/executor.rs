//! Executor integration tests — these spawn the real backing shell.

use std::time::Duration;

use llmsh::executor::Executor;

#[test]
fn cd_persists_across_commands() {
    let mut exec = Executor::new().unwrap();
    let tmp = std::env::temp_dir().canonicalize().unwrap();
    let code = exec.run_builtin(&["cd".into(), tmp.display().to_string()]);
    assert_eq!(code, 0);
    assert_eq!(exec.cwd(), &tmp);

    let (code, out) = exec.run_captured("pwd", Duration::from_secs(10));
    assert_eq!(code, 0);
    assert!(out.contains(&tmp.display().to_string()), "pwd was: {out}");
}

#[test]
fn exported_var_visible_in_child() {
    let mut exec = Executor::new().unwrap();
    let code = exec.run_builtin(&["export".into(), "LLMSH_TEST=hello123".into()]);
    assert_eq!(code, 0);

    let (code, out) = exec.run_captured("echo $LLMSH_TEST", Duration::from_secs(10));
    assert_eq!(code, 0);
    assert!(out.contains("hello123"), "output was: {out}");
}

#[test]
fn unset_removes_var() {
    let mut exec = Executor::new().unwrap();
    exec.run_builtin(&["export".into(), "LLMSH_GONE=x".into()]);
    exec.run_builtin(&["unset".into(), "LLMSH_GONE".into()]);
    let (_, out) = exec.run_captured("echo [$LLMSH_GONE]", Duration::from_secs(10));
    assert!(out.contains("[]"), "output was: {out}");
}

#[test]
fn cd_dash_swaps_directories() {
    let mut exec = Executor::new().unwrap();
    let a = std::env::temp_dir().canonicalize().unwrap();
    let b = std::path::Path::new("/").canonicalize().unwrap();

    exec.run_builtin(&["cd".into(), a.display().to_string()]);
    exec.run_builtin(&["cd".into(), b.display().to_string()]);
    let code = exec.run_builtin(&["cd".into(), "-".into()]);
    assert_eq!(code, 0);
    assert_eq!(exec.cwd(), &a);
}

#[test]
fn captured_output_truncates() {
    let mut exec = Executor::new().unwrap();
    // Produce far more than 8000 characters.
    let (code, out) = exec.run_captured(
        "for i in $(seq 1 5000); do echo this-is-a-line-$i; done",
        Duration::from_secs(30),
    );
    assert_eq!(code, 0);
    assert!(out.contains("truncated"), "expected truncation marker");
    // The tail should be retained.
    assert!(out.contains("line-5000"), "tail missing");
}

#[test]
fn timeout_kills_child() {
    let mut exec = Executor::new().unwrap();
    let (code, out) = exec.run_captured("sleep 10", Duration::from_millis(300));
    assert_eq!(code, 137);
    assert!(out.contains("timed out"), "output was: {out}");
}

#[test]
fn exit_code_recorded_in_history() {
    let mut exec = Executor::new().unwrap();
    exec.run_captured("false", Duration::from_secs(5));
    assert_eq!(exec.last_exit, 1);
    let (cmd, code) = exec.history.back().unwrap();
    assert_eq!(cmd, "false");
    assert_eq!(*code, 1);
}
