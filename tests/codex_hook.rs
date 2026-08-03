use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run_codex_hook(extra_env: Option<(&str, &str)>, input: &str) -> Output {
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_svarog"));
    command
        .arg("codex-hook")
        .env("SVAROG_HOME", root.path().join("svarog"))
        .env("CODEX_HOME", root.path().join("codex"))
        .env("SVAROG_DAEMON_ADDR", "127.0.0.1:9")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some((key, value)) = extra_env {
        command.env(key, value);
    }

    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn codex_hook_exits_cleanly_when_collector_is_unavailable() {
    let output = run_codex_hook(
        None,
        r#"{"session_id":"session-1","turn_id":"turn-1","cwd":"/work/svarog","hook_event_name":"UserPromptSubmit"}"#,
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

#[test]
fn codex_hook_ignores_svarog_recommender_sessions() {
    let output = run_codex_hook(Some(("SVAROG_RECOMMENDER", "1")), "not even json");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
}
