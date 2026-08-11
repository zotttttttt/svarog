use crate::config::RuntimeEnv;
use crate::models::{Agent, CodexHookEvent};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn print(agent: Agent) {
    match agent {
        Agent::Codex => {
            println!("# Codex lifecycle hooks read JSON on stdin");
            println!("svarog codex-hook");
        }
        Agent::Claude => {
            println!("# Claude Code lifecycle hook command");
            println!("svarog event --agent claude --event tool_start --duration 120");
        }
        Agent::Droid => {
            println!("# Factory Droid / Droid lifecycle hook command");
            println!("svarog event --agent droid --event task_start --duration 120");
        }
        Agent::FactoryDroid => {
            println!("# Factory Droid lifecycle hook command");
            println!("svarog event --agent factory-droid --event task_start --duration 120");
        }
        Agent::OpenClaw => {
            println!("# OpenClaw lifecycle hook command");
            println!("svarog event --agent openclaw --event task_start --duration 120");
        }
        Agent::Custom => {
            println!("# Generic hook API");
            println!("curl -sS -X POST http://127.0.0.1:8787/events \\");
            println!("  -H 'content-type: application/json' \\");
            println!(
                "  -d '{{\"agent\":\"custom\",\"event\":\"busy\",\"expected_duration_sec\":120}}'"
            );
        }
    }
}

pub fn install(env: &RuntimeEnv, agent: Agent) -> Result<PathBuf> {
    let paths = &env.paths;
    paths.ensure()?;
    let hook_dir = paths.config_dir.join("hooks");
    fs::create_dir_all(&hook_dir).with_context(|| format!("creating {}", hook_dir.display()))?;
    let path = hook_dir.join(format!("{}-event.sh", agent.as_str()));
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("svarog"));
    let contents = hook_script(agent, &env.env_pairs(), &executable);
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;

    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions)?;

    Ok(path)
}

pub fn install_global_codex(env: &RuntimeEnv) -> Result<PathBuf> {
    let script = install(env, Agent::Codex)?;
    install_codex_hook_config(&env.codex_home, &script)
}

fn install_codex_hook_config(codex_home: &Path, script: &Path) -> Result<PathBuf> {
    fs::create_dir_all(codex_home).with_context(|| format!("creating {}", codex_home.display()))?;
    let path = codex_home.join("hooks.json");
    let mut root = if path.exists() {
        let contents =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str::<Value>(&contents).with_context(|| {
            format!(
                "parsing {}; repair the existing Codex hook configuration and retry",
                path.display()
            )
        })?
    } else {
        json!({})
    };

    ensure_svarog_hook(&mut root, script);

    let contents = serde_json::to_string_pretty(&root).context("serializing Codex hooks")?;
    atomic_write_user_only(&path, format!("{contents}\n").as_bytes())?;
    Ok(path)
}

fn atomic_write_user_only(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("securing temporary file in {}", parent.display()))?;
    temp.write_all(contents)
        .with_context(|| format!("writing {}", path.display()))?;
    temp.as_file()
        .sync_all()
        .with_context(|| format!("syncing {}", path.display()))?;
    temp.persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn ensure_svarog_hook(root: &mut Value, script: &Path) {
    if !root.is_object() {
        *root = json!({});
    }
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let command = format!("\"{}\"", script.display());
    for (event, matcher, timeout) in [
        ("SessionStart", "startup|resume|clear|compact", 5),
        ("UserPromptSubmit", "", 5),
        ("Stop", "", 5),
        ("SessionEnd", "", 3),
    ] {
        ensure_codex_event_hook(hooks, event, matcher, timeout, &command);
    }
    remove_svarog_hook(hooks, "PreToolUse");
}

fn ensure_codex_event_hook(
    hooks: &mut Value,
    event: &str,
    matcher: &str,
    timeout: u64,
    command: &str,
) {
    let event_hooks = hooks
        .as_object_mut()
        .unwrap()
        .entry(event)
        .or_insert_with(|| json!([]));
    if !event_hooks.is_array() {
        *event_hooks = json!([]);
    }

    let mut entry = json!({
        "matcher": "Read|Glob|Grep|List|Bash|apply_patch|Edit|Write|mcp__.*",
        "hooks": [
            {
                "type": "command",
                "command": command,
                "timeout": timeout,
                "statusMessage": "Svarog is watching for forge time"
            }
        ]
    });
    if matcher.is_empty() {
        entry.as_object_mut().unwrap().remove("matcher");
    } else {
        entry["matcher"] = json!(matcher);
    }

    let entries = event_hooks.as_array_mut().unwrap();
    entries.retain(|entry| !is_svarog_hook(entry));
    entries.push(entry);
}

fn remove_svarog_hook(hooks: &mut Value, event: &str) {
    let Some(event_hooks) = hooks.as_object_mut().unwrap().get_mut(event) else {
        return;
    };
    let Some(entries) = event_hooks.as_array_mut() else {
        return;
    };
    entries.retain(|entry| !is_svarog_hook(entry));
}

fn is_svarog_hook(value: &Value) -> bool {
    value
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| {
                        command.contains("svarog") || command.contains("codex-event.sh")
                    })
            })
        })
}

fn hook_script(agent: Agent, env_pairs: &[(&'static str, String)], executable: &Path) -> String {
    let event = match agent {
        Agent::Claude => "tool_start",
        Agent::Codex => "tool_start",
        Agent::Droid | Agent::FactoryDroid | Agent::OpenClaw => "task_start",
        Agent::Custom => "busy",
    };
    let exports = env_pairs
        .iter()
        .map(|(key, value)| format!("export {key}={}", shell_quote(value)))
        .collect::<Vec<_>>()
        .join("\n");
    if agent == Agent::Codex {
        return format!(
            r#"#!/usr/bin/env sh
set -eu
{exports}

exec {executable} codex-hook
"#,
            exports = exports,
            executable = shell_quote(&executable.display().to_string())
        );
    }
    format!(
        r#"#!/usr/bin/env sh
set -eu
{exports}

duration="${{SVAROG_DURATION:-${{1:-120}}}}"
event="${{SVAROG_EVENT:-{event}}}"
project="${{SVAROG_PROJECT:-${{PWD##*/}}}}"

nohup {executable} event --agent {agent} --event "$event" --duration "$duration" --project "$project" >/dev/null 2>&1 &
exit 0
"#,
        agent = agent.as_str(),
        event = event,
        exports = exports,
        executable = shell_quote(&executable.display().to_string())
    )
}

pub async fn ingest_codex(env: &RuntimeEnv) -> Result<()> {
    if std::env::var_os("SVAROG_RECOMMENDER").is_some() {
        println!("{{}}");
        return Ok(());
    }
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if let Ok(payload) = serde_json::from_str::<CodexHookEvent>(&input) {
        let url = format!("http://{}/hooks/codex", env.daemon_addr);
        if let Ok(client) = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_millis(500))
            .build()
        {
            let _ = client.post(url).json(&payload).send().await;
        }
    }
    println!("{{}}");
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn codex_hook_config_preserves_existing_hooks() {
        let root = tempdir().unwrap().keep();
        let codex_home = root.join(".codex");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            codex_home.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo done"}]}]}}"#,
        )
        .unwrap();

        let path =
            install_codex_hook_config(&codex_home, Path::new("/tmp/codex-event.sh")).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();

        assert_eq!(
            value["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo done"
        );
        assert!(value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("codex-event.sh"));
        assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(value["hooks"]["SessionEnd"].as_array().unwrap().len(), 1);
        assert_eq!(value["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 3);
        assert_eq!(value["hooks"]["Stop"][1]["hooks"][0]["timeout"], 5);
        assert!(value["hooks"].get("PreToolUse").is_none());
        assert_eq!(
            fs::metadata(codex_home.join("hooks.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn codex_hook_config_replaces_old_svarog_session_end_timeout() {
        let root = tempdir().unwrap().keep();
        let codex_home = root.join(".codex");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            codex_home.join("hooks.json"),
            r#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"echo keep","timeout":4}]},{"hooks":[{"type":"command","command":"old codex-event.sh","timeout":5}]}]}}"#,
        )
        .unwrap();

        let path =
            install_codex_hook_config(&codex_home, Path::new("/tmp/codex-event.sh")).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let session_end = value["hooks"]["SessionEnd"].as_array().unwrap();

        assert_eq!(session_end.len(), 2);
        assert_eq!(session_end[0]["hooks"][0]["command"], "echo keep");
        assert_eq!(session_end[0]["hooks"][0]["timeout"], 4);
        assert_eq!(
            session_end[1]["hooks"][0]["command"],
            "\"/tmp/codex-event.sh\""
        );
        assert_eq!(session_end[1]["hooks"][0]["timeout"], 3);
    }

    #[test]
    fn malformed_codex_hook_config_is_preserved() {
        let root = tempdir().unwrap();
        let codex_home = root.path().join(".codex");
        fs::create_dir_all(&codex_home).unwrap();
        let path = codex_home.join("hooks.json");
        fs::write(&path, "{ not valid json").unwrap();

        let error =
            install_codex_hook_config(&codex_home, Path::new("/tmp/codex-event.sh")).unwrap_err();

        assert!(error.to_string().contains("repair the existing Codex hook"));
        assert_eq!(fs::read_to_string(path).unwrap(), "{ not valid json");
    }

    #[test]
    fn codex_hook_config_removes_old_svarog_pre_tool_hook() {
        let root = tempdir().unwrap().keep();
        let codex_home = root.join(".codex");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            codex_home.join("hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"old svarog"}]}]}}"#,
        )
        .unwrap();

        let path =
            install_codex_hook_config(&codex_home, Path::new("/tmp/codex-event.sh")).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();

        assert!(value["hooks"]["PreToolUse"].as_array().unwrap().is_empty());
        assert_eq!(
            value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn codex_hook_config_preserves_unrelated_pre_tool_hooks() {
        let root = tempdir().unwrap().keep();
        let codex_home = root.join(".codex");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            codex_home.join("hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo keep"}]}]}}"#,
        )
        .unwrap();

        let path =
            install_codex_hook_config(&codex_home, Path::new("/tmp/codex-event.sh")).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();

        assert_eq!(
            value["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo keep"
        );
        assert_eq!(
            value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn codex_hook_script_forwards_stdin_to_ingestion_command() {
        let script = hook_script(Agent::Codex, &[], Path::new("/usr/local/bin/svarog"));

        assert!(script.contains("exec '/usr/local/bin/svarog' codex-hook"));
        assert!(!script.contains("nohup"));
    }

    #[test]
    fn codex_hook_payload_does_not_forward_prompt_text() {
        let payload: CodexHookEvent = serde_json::from_str(
            r#"{
                "session_id":"session-1",
                "turn_id":"turn-1",
                "cwd":"/work/svarog",
                "hook_event_name":"UserPromptSubmit",
                "prompt":"private prompt text"
            }"#,
        )
        .unwrap();
        let forwarded = serde_json::to_value(payload).unwrap();

        assert!(forwarded.get("prompt").is_none());
        assert_eq!(forwarded["cwd"], "/work/svarog");
    }
}
