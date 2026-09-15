use crate::collector_auth;
use crate::config::{CodingAgentSelection, RuntimeEnv};
use crate::models::{Agent, LifecycleHookEvent};
use anyhow::{bail, Context, Result};
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
            println!("# Claude Code lifecycle hooks read JSON on stdin");
            println!("svarog lifecycle-hook claude");
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
            println!("token_file=\"${{SVAROG_HOME:-$HOME/.config/svarog}}/collector.token\"");
            println!("curl -sS -X POST http://127.0.0.1:8787/events \\");
            println!("  -H 'content-type: application/json' \\");
            println!("  -H \"authorization: Bearer $(tr -d '\\n' < \"$token_file\")\" \\");
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
    install_global(env, Agent::Codex)
}

pub fn install_global_claude(env: &RuntimeEnv) -> Result<PathBuf> {
    install_global(env, Agent::Claude)
}

#[cfg(test)]
fn install_codex_hook_config(codex_home: &Path, script: &Path) -> Result<PathBuf> {
    let path = codex_home.join("hooks.json");
    let contents = updated_settings(&path, Agent::Codex, true, script)?
        .context("enabled integration did not produce settings")?;
    fs::create_dir_all(codex_home).with_context(|| format!("creating {}", codex_home.display()))?;
    atomic_write_user_only(&path, &contents)?;
    Ok(path)
}

fn install_global(env: &RuntimeEnv, agent: Agent) -> Result<PathBuf> {
    let script = install(env, agent)?;
    let path = settings_path(env, agent)?;
    if let Some(contents) = updated_settings(&path, agent, true, &script)? {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        atomic_write_user_only(&path, &contents)?;
    }
    Ok(path)
}

pub fn reconcile(env: &RuntimeEnv, selection: CodingAgentSelection) -> Result<()> {
    let codex_script = install(env, Agent::Codex)?;
    let claude_script = install(env, Agent::Claude)?;
    let specs = [
        (Agent::Codex, selection.includes(Agent::Codex), codex_script),
        (
            Agent::Claude,
            selection.includes(Agent::Claude),
            claude_script,
        ),
    ];
    let mut prepared = Vec::new();
    for (agent, enabled, script) in specs {
        let path = settings_path(env, agent)?;
        let original = fs::read(&path).ok();
        let updated = updated_settings(&path, agent, enabled, &script)?;
        prepared.push((path, original, updated));
    }

    let mut written: Vec<usize> = Vec::new();
    for (index, (path, _original, updated)) in prepared.iter().enumerate() {
        let Some(updated) = updated else {
            continue;
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        if let Err(error) = atomic_write_user_only(path, updated) {
            for index in written.into_iter().rev() {
                let (written_path, previous, _) = &prepared[index];
                match previous {
                    Some(previous) => {
                        let _ = atomic_write_user_only(written_path, previous);
                    }
                    None => {
                        let _ = fs::remove_file(written_path);
                    }
                }
            }
            return Err(error.context("reconciling coding-agent hooks"));
        }
        written.push(index);
    }
    Ok(())
}

pub fn is_configured(env: &RuntimeEnv, selection: CodingAgentSelection) -> Result<bool> {
    for agent in [Agent::Codex, Agent::Claude] {
        let path = settings_path(env, agent)?;
        if selection.includes(agent) {
            let script = env
                .paths
                .config_dir
                .join("hooks")
                .join(format!("{}-event.sh", agent.as_str()));
            if !script.is_file() {
                return Ok(false);
            }
            if updated_settings(&path, agent, true, &script)?.is_some() {
                return Ok(false);
            }
        } else if path.exists() {
            let script = PathBuf::new();
            if updated_settings(&path, agent, false, &script)?.is_some() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn settings_path(env: &RuntimeEnv, agent: Agent) -> Result<PathBuf> {
    match agent {
        Agent::Codex => Ok(env.codex_home.join("hooks.json")),
        Agent::Claude => Ok(env.claude_config_dir.join("settings.json")),
        _ => bail!("{agent} does not have a managed lifecycle integration"),
    }
}

fn updated_settings(
    path: &Path,
    agent: Agent,
    enabled: bool,
    script: &Path,
) -> Result<Option<Vec<u8>>> {
    let original = if path.exists() {
        Some(fs::read(path).with_context(|| format!("reading {}", path.display()))?)
    } else {
        None
    };
    if original.is_none() && !enabled {
        return Ok(None);
    }
    if !enabled
        && original
            .as_deref()
            .is_some_and(|contents| !contains_svarog_marker(contents, agent))
    {
        return Ok(None);
    }
    let mut root = match original.as_deref() {
        Some(contents) => serde_json::from_slice::<Value>(contents).with_context(|| {
            format!(
                "parsing {}; repair the existing {} hook configuration and retry",
                path.display(),
                agent_name(agent)
            )
        })?,
        None => json!({}),
    };
    if !root.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    validate_hook_shape(&root, path)?;
    if enabled {
        ensure_svarog_hook(&mut root, agent, script);
    } else {
        remove_all_svarog_hooks(&mut root, agent);
    }
    let contents = serde_json::to_string_pretty(&root).context("serializing hook settings")?;
    let contents = format!("{contents}\n").into_bytes();
    if original.as_deref() == Some(contents.as_slice()) {
        Ok(None)
    } else {
        Ok(Some(contents))
    }
}

fn validate_hook_shape(root: &Value, path: &Path) -> Result<()> {
    let Some(hooks) = root.get("hooks") else {
        return Ok(());
    };
    let hooks = hooks
        .as_object()
        .with_context(|| format!("{}.hooks must contain a JSON object", path.display()))?;
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "Stop",
        "SessionEnd",
        "PreToolUse",
    ] {
        if hooks.get(event).is_some_and(|entries| !entries.is_array()) {
            bail!("{}.hooks.{event} must contain a JSON array", path.display());
        }
    }
    Ok(())
}

fn contains_svarog_marker(contents: &[u8], agent: Agent) -> bool {
    let text = String::from_utf8_lossy(contents);
    text.contains(&format!("{}-event.sh", agent.as_str()))
        || text.contains(&format!("svarog event --agent {}", agent.as_str()))
}

fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "Codex",
        Agent::Claude => "Claude Code",
        _ => "coding agent",
    }
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

fn ensure_svarog_hook(root: &mut Value, agent: Agent, script: &Path) {
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
    let command = shell_quote(&script.display().to_string());
    let session_matcher = if agent == Agent::Claude {
        "startup|resume|clear|compact|fork"
    } else {
        "startup|resume|clear|compact"
    };
    for (event, matcher, timeout) in [
        ("SessionStart", session_matcher, 5),
        ("UserPromptSubmit", "", 5),
        ("Stop", "", 5),
        ("SessionEnd", "", 3),
    ] {
        ensure_agent_event_hook(hooks, agent, event, matcher, timeout, &command);
    }
    remove_svarog_hook(hooks, agent, "PreToolUse");
}

fn ensure_agent_event_hook(
    hooks: &mut Value,
    agent: Agent,
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
    entries.retain(|entry| !is_svarog_hook(entry, agent));
    entries.push(entry);
}

fn remove_svarog_hook(hooks: &mut Value, agent: Agent, event: &str) {
    let Some(event_hooks) = hooks.as_object_mut().unwrap().get_mut(event) else {
        return;
    };
    let Some(entries) = event_hooks.as_array_mut() else {
        return;
    };
    entries.retain(|entry| !is_svarog_hook(entry, agent));
}

fn remove_all_svarog_hooks(root: &mut Value, agent: Agent) {
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    for entries in hooks.values_mut().filter_map(Value::as_array_mut) {
        entries.retain(|entry| !is_svarog_hook(entry, agent));
    }
}

fn is_svarog_hook(value: &Value, agent: Agent) -> bool {
    let script_name = format!("{}-event.sh", agent.as_str());
    let legacy = format!("svarog event --agent {}", agent.as_str());
    value
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| {
                        command.contains(&script_name) || command.contains(&legacy)
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
    if matches!(agent, Agent::Codex | Agent::Claude) {
        return format!(
            r#"#!/usr/bin/env sh
set -eu
{exports}

exec {executable} lifecycle-hook {agent}
"#,
            exports = exports,
            executable = shell_quote(&executable.display().to_string()),
            agent = agent.as_str(),
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

pub async fn ingest_lifecycle(env: &RuntimeEnv, agent: Agent) -> Result<()> {
    if !matches!(agent, Agent::Codex | Agent::Claude) {
        bail!("{agent} does not provide supported lifecycle hook input");
    }
    if std::env::var_os("SVAROG_RECOMMENDER").is_some() {
        println!("{{}}");
        return Ok(());
    }
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if let Ok(payload) = serde_json::from_str::<LifecycleHookEvent>(&input) {
        let url = format!("http://{}/hooks/{}", env.daemon_addr, agent.as_str());
        let token = collector_auth::load(&env.paths).ok();
        if let Ok(client) = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_millis(500))
            .build()
        {
            if let Some(token) = token {
                let _ = client
                    .post(url)
                    .bearer_auth(token.as_str())
                    .json(&payload)
                    .send()
                    .await;
            }
        }
    }
    println!("{{}}");
    Ok(())
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Paths, RuntimeMode};
    use tempfile::tempdir;

    fn test_env(root: &Path) -> RuntimeEnv {
        RuntimeEnv {
            mode: RuntimeMode::Dev,
            paths: Paths::from_root(root.join("svarog")),
            codex_home: root.join("codex"),
            claude_config_dir: root.join("claude"),
            daemon_addr: "127.0.0.1:18787".parse().unwrap(),
            dry_run: false,
        }
    }

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
            "'/tmp/codex-event.sh'"
        );
        assert_eq!(session_end[1]["hooks"][0]["timeout"], 3);
    }

    #[test]
    fn codex_hook_command_shell_quotes_metacharacter_paths() {
        let script = Path::new("/tmp/space ' $(touch nope); `nope`/codex-event.sh");
        let mut root = json!({});
        ensure_svarog_hook(&mut root, Agent::Codex, script);
        let command = root["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(command, shell_quote(&script.display().to_string()));
        assert!(command.starts_with('\''));
        assert!(command.ends_with('\''));
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
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"svarog event --agent codex --event tool_start"}]}]}}"#,
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

        assert!(script.contains("exec '/usr/local/bin/svarog' lifecycle-hook codex"));
        assert!(!script.contains("nohup"));
    }

    #[test]
    fn codex_hook_payload_does_not_forward_prompt_text() {
        let payload: LifecycleHookEvent = serde_json::from_str(
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

    #[test]
    fn claude_prompt_id_is_sanitized_into_the_shared_turn_field() {
        let payload: LifecycleHookEvent = serde_json::from_str(
            r#"{
                "session_id":"session-1",
                "prompt_id":"prompt-1",
                "cwd":"/work/svarog",
                "hook_event_name":"UserPromptSubmit",
                "prompt":"private prompt text",
                "transcript_path":"/private/transcript.jsonl"
            }"#,
        )
        .unwrap();
        let forwarded = serde_json::to_value(payload).unwrap();

        assert_eq!(forwarded["turn_id"], "prompt-1");
        assert!(forwarded.get("prompt").is_none());
        assert!(forwarded.get("transcript_path").is_none());
    }

    #[test]
    fn reconcile_installs_both_agents_and_removes_only_deselected_hooks() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.codex_home).unwrap();
        fs::write(
            env.codex_home.join("hooks.json"),
            r#"{"theme":"dark","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]}}"#,
        )
        .unwrap();
        fs::create_dir_all(&env.claude_config_dir).unwrap();
        fs::write(
            env.claude_config_dir.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(git status)"]}}"#,
        )
        .unwrap();

        reconcile(&env, CodingAgentSelection::All).unwrap();

        let codex: Value =
            serde_json::from_slice(&fs::read(env.codex_home.join("hooks.json")).unwrap()).unwrap();
        let claude: Value =
            serde_json::from_slice(&fs::read(env.claude_config_dir.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(codex["theme"], "dark");
        assert_eq!(codex["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(claude["permissions"]["allow"][0], "Bash(git status)");
        assert_eq!(
            claude["hooks"]["SessionStart"][0]["matcher"],
            "startup|resume|clear|compact|fork"
        );
        assert_eq!(claude["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 3);
        assert!(is_configured(&env, CodingAgentSelection::All).unwrap());

        reconcile(&env, CodingAgentSelection::Claude).unwrap();

        let codex: Value =
            serde_json::from_slice(&fs::read(env.codex_home.join("hooks.json")).unwrap()).unwrap();
        assert_eq!(codex["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(
            codex["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo keep"
        );
        assert!(is_configured(&env, CodingAgentSelection::Claude).unwrap());
    }

    #[test]
    fn reconcile_prevalidates_both_agent_configs_before_writing() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.codex_home).unwrap();
        let original = br#"{"theme":"keep"}"#;
        fs::write(env.codex_home.join("hooks.json"), original).unwrap();
        fs::create_dir_all(&env.claude_config_dir).unwrap();
        fs::write(
            env.claude_config_dir.join("settings.json"),
            "{ \"command\": \"claude-event.sh\", invalid",
        )
        .unwrap();

        let error = reconcile(&env, CodingAgentSelection::All).unwrap_err();

        assert!(error.to_string().contains("Claude Code"));
        assert_eq!(
            fs::read(env.codex_home.join("hooks.json")).unwrap(),
            original
        );
    }

    #[test]
    fn global_claude_install_is_idempotent() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());

        let first = install_global_claude(&env).unwrap();
        let contents = fs::read(&first).unwrap();
        let second = install_global_claude(&env).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(second).unwrap(), contents);
    }
}
