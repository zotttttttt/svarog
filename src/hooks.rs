use crate::collector_auth;
use crate::config::{CodingAgentSelection, RuntimeEnv};
use crate::models::{Agent, LifecycleHookEvent};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use yaml_edit::{Document, SequenceBuilder, YamlFile, YamlKind, YamlNode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationStatus {
    pub configured: bool,
    pub warnings: Vec<String>,
}

struct PreparedSettings {
    path: PathBuf,
    target: PathBuf,
    original: Option<Vec<u8>>,
    updated: Option<Vec<u8>>,
    remove: bool,
}

const PI_EXTENSION_MARKER: &str = "// Managed by Svarog. Do not edit.";

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
        Agent::Pi => {
            println!("# Pi lifecycle events are sent by the managed Svarog extension");
            println!("svarog lifecycle-hook pi");
        }
        Agent::Hermes => {
            println!("# Hermes lifecycle hooks read JSON on stdin");
            println!("svarog lifecycle-hook hermes");
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

pub fn install_global_pi(env: &RuntimeEnv) -> Result<PathBuf> {
    install_global(env, Agent::Pi)
}

pub fn install_global_hermes(env: &RuntimeEnv) -> Result<PathBuf> {
    install_global(env, Agent::Hermes)
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
    if agent == Agent::Hermes {
        let prepared = prepare_hermes_settings(env, true, &script)?;
        apply_prepared(&prepared, atomic_write_resolved_user_only, restore_prepared)?;
        return Ok(path);
    }
    if agent == Agent::Pi {
        let (contents, _) = updated_pi_extension(&path, true, &script)?;
        if let Some(contents) = contents {
            let parent = path
                .parent()
                .with_context(|| format!("{} has no parent directory", path.display()))?;
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
            atomic_write_user_only(&path, &contents)?;
        }
        return Ok(path);
    }
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
    let pi_script = install(env, Agent::Pi)?;
    let hermes_script = install(env, Agent::Hermes)?;
    let specs = [
        (Agent::Codex, selection.includes(Agent::Codex), codex_script),
        (
            Agent::Claude,
            selection.includes(Agent::Claude),
            claude_script,
        ),
        (Agent::Pi, selection.includes(Agent::Pi), pi_script),
        (
            Agent::Hermes,
            selection.includes(Agent::Hermes),
            hermes_script,
        ),
    ];
    let mut prepared = Vec::new();
    for (agent, enabled, script) in specs {
        let path = settings_path(env, agent)?;
        let original = read_optional(&path)?;
        if agent == Agent::Hermes {
            prepared.extend(prepare_hermes_settings(env, enabled, &script)?);
            continue;
        }
        let (updated, remove) = if agent == Agent::Pi {
            updated_pi_extension(&path, enabled, &script)?
        } else {
            (updated_settings(&path, agent, enabled, &script)?, false)
        };
        let target = resolve_write_target(&path)?;
        prepared.push(PreparedSettings {
            path,
            target,
            original,
            updated,
            remove,
        });
    }

    for item in prepared.iter().filter(|item| item.updated.is_some()) {
        let parent = item
            .target
            .parent()
            .with_context(|| format!("{} has no parent directory", item.target.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    apply_prepared(&prepared, atomic_write_resolved_user_only, restore_prepared)
}

fn apply_prepared(
    prepared: &[PreparedSettings],
    mut write: impl FnMut(&Path, &[u8]) -> Result<()>,
    mut restore: impl FnMut(&PreparedSettings) -> Result<()>,
) -> Result<()> {
    for item in prepared.iter().filter(|item| item.updated.is_some()) {
        let parent = item
            .target
            .parent()
            .with_context(|| format!("{} has no parent directory", item.target.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut written = Vec::new();
    for (index, item) in prepared.iter().enumerate() {
        let result = if item.remove {
            match fs::remove_file(&item.target) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => {
                    Err(error).with_context(|| format!("removing {}", item.path.display()))
                }
            }
        } else if let Some(updated) = &item.updated {
            write(&item.target, updated).map(|_| true)
        } else {
            Ok(false)
        };
        let changed = match result {
            Ok(changed) => changed,
            Err(error) => {
                let mut rollback_errors = Vec::new();
                for index in written.into_iter().rev() {
                    if let Err(rollback_error) = restore(&prepared[index]) {
                        rollback_errors.push(rollback_error.to_string());
                    }
                }
                if rollback_errors.is_empty() {
                    return Err(
                        error.context("reconciling coding-agent hooks; previous settings restored")
                    );
                }
                bail!(
                    "reconciling coding-agent hooks failed: {error}; rollback also failed: {}",
                    rollback_errors.join("; ")
                );
            }
        };
        if changed {
            written.push(index);
        }
    }
    Ok(())
}

fn restore_prepared(item: &PreparedSettings) -> Result<()> {
    match &item.original {
        Some(original) => atomic_write_resolved_user_only(&item.target, original),
        None => match fs::remove_file(&item.target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("removing {}", item.path.display())),
        },
    }
}

pub fn is_configured(env: &RuntimeEnv, selection: CodingAgentSelection) -> Result<bool> {
    for agent in [Agent::Codex, Agent::Claude, Agent::Pi, Agent::Hermes] {
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
            if !settings_are_current(&path, agent, true, &script)? {
                return Ok(false);
            }
            if agent == Agent::Hermes && !hermes_allowlist_is_current(env, true, &script)? {
                return Ok(false);
            }
        } else {
            if path.exists() {
                let script = PathBuf::new();
                if !settings_are_current(&path, agent, false, &script)? {
                    return Ok(false);
                }
            }
            if agent == Agent::Hermes && !hermes_allowlist_is_current(env, false, &PathBuf::new())?
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn settings_are_current(path: &Path, agent: Agent, enabled: bool, script: &Path) -> Result<bool> {
    if agent == Agent::Hermes {
        let updated = updated_hermes_config(path, enabled, script)?;
        return match updated {
            None => Ok(true),
            Some(updated) => Ok(read_optional(path)?.as_deref() == Some(updated.as_slice())),
        };
    }
    if agent == Agent::Pi {
        let (updated, remove) = updated_pi_extension(path, enabled, script)?;
        if remove {
            return Ok(false);
        }
        return match updated {
            None => Ok(true),
            Some(updated) => Ok(read_optional(path)?.as_deref() == Some(updated.as_slice())),
        };
    }
    let Some(updated) = updated_settings(path, agent, enabled, script)? else {
        return Ok(true);
    };
    let Some(original) = read_optional(path)? else {
        return Ok(false);
    };
    let original: Value =
        serde_json::from_slice(&original).with_context(|| format!("parsing {}", path.display()))?;
    let updated: Value =
        serde_json::from_slice(&updated).context("parsing updated hook settings")?;
    Ok(original == updated)
}

pub fn integration_status(
    env: &RuntimeEnv,
    selection: CodingAgentSelection,
) -> Result<IntegrationStatus> {
    let configured = is_configured(env, selection)?;
    let warnings = if selection.includes(Agent::Claude) {
        claude_hook_warnings(env)
    } else {
        Vec::new()
    };
    Ok(IntegrationStatus {
        configured,
        warnings,
    })
}

fn settings_path(env: &RuntimeEnv, agent: Agent) -> Result<PathBuf> {
    match agent {
        Agent::Codex => Ok(env.codex_home.join("hooks.json")),
        Agent::Claude => Ok(env.claude_config_dir.join("settings.json")),
        Agent::Pi => Ok(env.pi_config_dir.join("extensions").join("svarog.ts")),
        Agent::Hermes => Ok(env.hermes_home.join("config.yaml")),
        _ => bail!("{agent} does not have a managed lifecycle integration"),
    }
}

const HERMES_EVENTS: [(&str, u64); 4] = [
    ("on_session_start", 5),
    ("pre_llm_call", 5),
    ("on_session_end", 5),
    ("on_session_finalize", 3),
];

fn prepare_hermes_settings(
    env: &RuntimeEnv,
    enabled: bool,
    script: &Path,
) -> Result<Vec<PreparedSettings>> {
    let config_path = env.hermes_home.join("config.yaml");
    let allowlist_path = env.hermes_home.join("shell-hooks-allowlist.json");
    let mut prepared = Vec::new();
    for (path, updated) in [
        (
            config_path.clone(),
            updated_hermes_config(&config_path, enabled, script)?,
        ),
        (
            allowlist_path.clone(),
            updated_hermes_allowlist(&allowlist_path, enabled, script)?,
        ),
    ] {
        let original = read_optional(&path)?;
        let target = resolve_write_target(&path)?;
        prepared.push(PreparedSettings {
            path,
            target,
            original,
            updated,
            remove: false,
        });
    }
    Ok(prepared)
}

fn updated_hermes_config(path: &Path, enabled: bool, script: &Path) -> Result<Option<Vec<u8>>> {
    let original = read_optional(path)?;
    if original.is_none() && !enabled {
        return Ok(None);
    }
    let command = shell_quote(&script.display().to_string());
    if original.is_none() {
        let contents = hermes_hooks_document(&command)?.to_string().into_bytes();
        return Ok(Some(contents));
    }
    let input = original
        .as_deref()
        .map(String::from_utf8_lossy)
        .map(|value| value.into_owned())
        .unwrap_or_else(|| "{}\n".to_string());
    let yaml = YamlFile::from_str(&input).with_context(|| {
        format!(
            "parsing {}; repair the existing Hermes hook configuration and retry",
            path.display()
        )
    })?;
    let document = yaml
        .document()
        .with_context(|| format!("{} must contain one YAML document", path.display()))?;
    let root = document
        .as_mapping()
        .with_context(|| format!("{} must contain a YAML mapping", path.display()))?;
    ensure_unique_yaml_key(&root, "hooks", path)?;
    let repaired_misplaced_hooks = repair_misplaced_hermes_hooks(&root)?;
    if root
        .get("hooks")
        .is_some_and(|value| is_empty_yaml_scalar(&value) && repaired_misplaced_hooks)
    {
        root.remove("hooks");
    }
    if root.get("hooks").is_none() {
        if enabled {
            root.set("hooks", hermes_hooks_mapping(&command)?);
        }
        let updated = yaml.to_string().into_bytes();
        return Ok((original.as_deref() != Some(updated.as_slice())).then_some(updated));
    }
    let hooks_value = root.get("hooks").expect("checked above");
    let hooks = hooks_value.as_mapping().with_context(|| {
        format!(
            "{}.hooks must contain a YAML mapping; found {}",
            path.display(),
            yaml_kind_name(&hooks_value)
        )
    })?;
    for (event, _) in HERMES_EVENTS {
        ensure_unique_yaml_key(hooks, event, path)?;
    }
    if hooks.is_empty() && enabled {
        root.set("hooks", hermes_hooks_mapping(&command)?);
        let updated = yaml.to_string().into_bytes();
        return Ok((original.as_deref() != Some(updated.as_slice())).then_some(updated));
    }
    for (event, timeout) in HERMES_EVENTS {
        if hooks.get(event).is_none() {
            if enabled {
                hooks.set(event, hermes_hook_sequence(&command, timeout)?);
            }
            continue;
        }
        let entries = hooks.get_sequence(event).with_context(|| {
            format!(
                "{}.hooks.{event} must contain a YAML sequence",
                path.display()
            )
        })?;
        let mut replacement = SequenceBuilder::new();
        for entry in entries.values() {
            if !is_managed_hermes_hook(&entry) {
                replacement = replacement.item(entry);
            }
        }
        if enabled {
            replacement = replacement.item(hermes_hook_entry(&command, timeout)?);
        }
        hooks.set(
            event,
            replacement
                .build_document()
                .as_sequence()
                .context("built Hermes hook list was not a sequence")?,
        );
    }
    let updated = yaml.to_string().into_bytes();
    Ok((original.as_deref() != Some(updated.as_slice())).then_some(updated))
}

fn hermes_hook_entry(command: &str, timeout: u64) -> Result<yaml_edit::Mapping> {
    let command = serde_json::to_string(command)?;
    let document = Document::from_str(&format!("{{command: {command}, timeout: {timeout}}}"))
        .context("building Hermes hook entry")?;
    document
        .as_mapping()
        .context("built Hermes hook entry was not a mapping")
}

fn repair_misplaced_hermes_hooks(root: &yaml_edit::Mapping) -> Result<bool> {
    let mut repaired = false;
    for (event, _) in HERMES_EVENTS {
        let occurrences = root.find_all_entries_by_key(event).collect::<Vec<_>>();
        for occurrence in occurrences {
            let Some(value) = occurrence.value_node() else {
                continue;
            };
            let Some(entries) = value.as_sequence() else {
                continue;
            };
            let values = entries.values().collect::<Vec<_>>();
            if !values.iter().any(is_managed_hermes_hook) {
                continue;
            }
            repaired = true;
            let mut replacement = SequenceBuilder::new();
            let mut kept = 0;
            for entry in values {
                if !is_managed_hermes_hook(&entry) {
                    replacement = replacement.item(entry);
                    kept += 1;
                }
            }
            if kept == 0 {
                occurrence.remove();
            } else {
                occurrence.set_value(
                    replacement
                        .build_document()
                        .as_sequence()
                        .context("built repaired Hermes hook list was not a sequence")?,
                    false,
                );
            }
        }
    }
    Ok(repaired)
}

fn is_managed_hermes_hook(entry: &YamlNode) -> bool {
    entry
        .as_mapping()
        .and_then(|mapping| mapping.get("command"))
        .and_then(|value| value.as_scalar().map(|scalar| scalar.as_string()))
        .is_some_and(|value| is_hermes_command(&value))
}

fn is_empty_yaml_scalar(value: &YamlNode) -> bool {
    value
        .as_scalar()
        .is_some_and(|scalar| scalar.as_string().is_empty())
}

fn yaml_kind_name(value: &YamlNode) -> &'static str {
    match value.kind() {
        YamlKind::Scalar if is_empty_yaml_scalar(value) => "null",
        YamlKind::Scalar => "a scalar",
        YamlKind::Mapping => "a mapping",
        YamlKind::Sequence => "a sequence",
        YamlKind::Alias => "an alias",
        YamlKind::Tagged(_) => "a tagged value",
        YamlKind::Document => "a document",
    }
}

fn ensure_unique_yaml_key(mapping: &yaml_edit::Mapping, key: &str, path: &Path) -> Result<()> {
    let matches = mapping
        .iter()
        .filter(|(candidate, _)| {
            candidate
                .as_scalar()
                .is_some_and(|scalar| scalar.as_string() == key)
        })
        .count();
    if matches > 1 {
        bail!(
            "{} contains duplicate YAML key {key:?}; remove the duplicate and retry",
            path.display()
        );
    }
    Ok(())
}

fn hermes_hook_sequence(command: &str, timeout: u64) -> Result<yaml_edit::Sequence> {
    SequenceBuilder::new()
        .item(hermes_hook_entry(command, timeout)?)
        .build_document()
        .as_sequence()
        .context("built Hermes hook list was not a sequence")
}

fn hermes_hooks_mapping(command: &str) -> Result<yaml_edit::Mapping> {
    hermes_hooks_document(command)?
        .as_mapping()
        .and_then(|root| root.get_mapping("hooks"))
        .context("built Hermes hooks value was not a mapping")
}

fn hermes_hooks_document(command: &str) -> Result<Document> {
    let command = serde_json::to_string(command)?;
    let mut text = String::from("hooks:\n");
    for (event, timeout) in HERMES_EVENTS {
        text.push_str(&format!(
            "  {event}:\n    - {{command: {command}, timeout: {timeout}}}\n"
        ));
    }
    Document::from_str(&text).context("building Hermes hooks mapping")
}

fn updated_hermes_allowlist(path: &Path, enabled: bool, script: &Path) -> Result<Option<Vec<u8>>> {
    let original = read_optional(path)?;
    if original.is_none() && !enabled {
        return Ok(None);
    }
    let mut root = match original.as_deref() {
        Some(contents) => serde_json::from_slice::<Value>(contents)
            .with_context(|| format!("parsing {}", path.display()))?,
        None => json!({"approvals": []}),
    };
    let object = root
        .as_object_mut()
        .with_context(|| format!("{} must contain a JSON object", path.display()))?;
    let approvals = object
        .entry("approvals")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("{}.approvals must contain a JSON array", path.display()))?;
    let command = shell_quote(&script.display().to_string());
    let existing = approvals
        .iter()
        .filter(|entry| entry.get("command").and_then(Value::as_str) == Some(command.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    approvals.retain(|entry| {
        !entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(is_hermes_command)
    });
    if enabled {
        let approved_at = chrono::Utc::now().to_rfc3339();
        for (event, _) in HERMES_EVENTS {
            approvals.push(
                existing
                    .iter()
                    .find(|entry| entry.get("event").and_then(Value::as_str) == Some(event))
                    .cloned()
                    .unwrap_or_else(|| {
                        json!({
                            "event": event,
                            "command": command,
                            "approved_at": approved_at,
                        })
                    }),
            );
        }
    }
    let contents = format!("{}\n", serde_json::to_string_pretty(&root)?).into_bytes();
    Ok((original.as_deref() != Some(contents.as_slice())).then_some(contents))
}

fn hermes_allowlist_is_current(env: &RuntimeEnv, enabled: bool, script: &Path) -> Result<bool> {
    let path = env.hermes_home.join("shell-hooks-allowlist.json");
    let Some(original) = read_optional(&path)? else {
        return Ok(!enabled);
    };
    let root: Value = serde_json::from_slice(&original)?;
    let approvals = root
        .get("approvals")
        .and_then(Value::as_array)
        .with_context(|| format!("{}.approvals must contain a JSON array", path.display()))?;
    let command = shell_quote(&script.display().to_string());
    let managed = approvals
        .iter()
        .filter(|entry| {
            entry
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(is_hermes_command)
        })
        .collect::<Vec<_>>();
    if !enabled {
        return Ok(managed.is_empty());
    }
    Ok(HERMES_EVENTS.iter().all(|(event, _)| {
        managed.iter().any(|entry| {
            entry.get("event").and_then(Value::as_str) == Some(*event)
                && entry.get("command").and_then(Value::as_str) == Some(command.as_str())
        })
    }))
}

fn is_hermes_command(command: &str) -> bool {
    command.contains("hermes-event.sh") || command.contains("svarog lifecycle-hook hermes")
}

fn updated_pi_extension(
    path: &Path,
    enabled: bool,
    script: &Path,
) -> Result<(Option<Vec<u8>>, bool)> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!(
            "{} is a symlink; remove it before installing the Svarog Pi extension",
            path.display()
        );
    }
    let original = read_optional(path)?;
    let managed = original
        .as_deref()
        .is_some_and(|contents| String::from_utf8_lossy(contents).starts_with(PI_EXTENSION_MARKER));
    if enabled {
        if original.is_some() && !managed {
            bail!(
                "{} already exists and is not managed by Svarog",
                path.display()
            );
        }
        let updated = pi_extension(script).into_bytes();
        if original.as_deref() == Some(updated.as_slice()) {
            Ok((None, false))
        } else {
            Ok((Some(updated), false))
        }
    } else if managed {
        Ok((None, true))
    } else {
        Ok((None, false))
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
        Agent::Pi => "Pi",
        Agent::Hermes => "Hermes Agent",
        _ => "coding agent",
    }
}

fn atomic_write_user_only(path: &Path, contents: &[u8]) -> Result<()> {
    let target = resolve_write_target(path)?;
    atomic_write_resolved_user_only(&target, contents)
}

fn atomic_write_resolved_user_only(path: &Path, contents: &[u8]) -> Result<()> {
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

fn resolve_write_target(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::canonicalize(path).with_context(|| {
                format!(
                    "resolving settings symlink {}; repair the dangling link and retry",
                    path.display()
                )
            })?;
            if !target.is_file() {
                bail!(
                    "settings symlink {} must point to a regular file",
                    path.display()
                );
            }
            Ok(target)
        }
        Ok(metadata) if !metadata.is_file() => {
            bail!("{} must be a regular file", path.display())
        }
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn claude_hook_warnings(env: &RuntimeEnv) -> Vec<String> {
    let mut warnings = Vec::new();
    let settings = env.claude_config_dir.join("settings.json");
    match json_bool(&settings, "disableAllHooks") {
        Ok(Some(true)) => warnings.push(
            "Claude Code user settings set disableAllHooks=true; a higher-precedence project setting may override it"
                .to_string(),
        ),
        Ok(_) => {}
        Err(error) => warnings.push(format!(
            "could not inspect Claude Code hook-disable setting: {error}"
        )),
    }

    match file_managed_claude_setting("allowManagedHooksOnly") {
        Ok(Some(true)) => warnings.push(
            "Claude Code file-based policy sets allowManagedHooksOnly=true, so user hooks are ignored"
                .to_string(),
        ),
        Ok(_) => {}
        Err(error) => warnings.push(format!(
            "could not inspect Claude Code managed hook policy: {error}"
        )),
    }
    warnings
}

fn json_bool(path: &Path, key: &str) -> Result<Option<bool>> {
    let Some(contents) = read_optional(path)? else {
        return Ok(None);
    };
    let root: Value =
        serde_json::from_slice(&contents).with_context(|| format!("parsing {}", path.display()))?;
    Ok(root.get(key).and_then(Value::as_bool))
}

fn file_managed_claude_setting(key: &str) -> Result<Option<bool>> {
    let Some(root) = managed_claude_settings_dir() else {
        return Ok(None);
    };
    file_managed_claude_setting_in(&root, key)
}

fn file_managed_claude_setting_in(root: &Path, key: &str) -> Result<Option<bool>> {
    let mut paths = vec![root.join("managed-settings.json")];
    let dropins = root.join("managed-settings.d");
    match fs::read_dir(&dropins) {
        Ok(entries) => {
            let mut entries = entries
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().and_then(|value| value.to_str()) == Some("json")
                        && !path
                            .file_name()
                            .and_then(|value| value.to_str())
                            .is_some_and(|value| value.starts_with('.'))
                })
                .collect::<Vec<_>>();
            entries.sort();
            paths.extend(entries);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("reading {}", dropins.display())),
    }
    let mut effective = None;
    for path in paths {
        if let Some(value) = json_bool(&path, key)? {
            effective = Some(value);
        }
    }
    Ok(effective)
}

fn managed_claude_settings_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return Some(PathBuf::from("/Library/Application Support/ClaudeCode"));
    }
    #[cfg(target_os = "linux")]
    {
        return Some(PathBuf::from("/etc/claude-code"));
    }
    #[allow(unreachable_code)]
    None
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
        Agent::Claude | Agent::Codex | Agent::Pi | Agent::Hermes => "tool_start",
        Agent::Droid | Agent::FactoryDroid | Agent::OpenClaw => "task_start",
        Agent::Custom => "busy",
    };
    let exports = env_pairs
        .iter()
        .map(|(key, value)| format!("export {key}={}", shell_quote(value)))
        .collect::<Vec<_>>()
        .join("\n");
    if matches!(
        agent,
        Agent::Codex | Agent::Claude | Agent::Pi | Agent::Hermes
    ) {
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

fn pi_extension(script: &Path) -> String {
    let script = serde_json::to_string(&script.display().to_string()).unwrap();
    format!(
        r#"{marker}
import {{ spawn }} from "node:child_process";
import {{ randomUUID }} from "node:crypto";
import type {{ ExtensionAPI, ExtensionContext }} from "@earendil-works/pi-coding-agent";

const hook = {script};

export default function (pi: ExtensionAPI) {{
  const pendingTurns = new Set<string>();
  let delivery: Promise<void> = Promise.resolve();

  const payload = (
    ctx: ExtensionContext,
    hookEventName: string,
    turnId?: string,
    reason?: string,
  ) => ({{
    session_id: ctx.sessionManager.getSessionId(),
    turn_id: turnId,
    cwd: ctx.cwd,
    hook_event_name: hookEventName,
    source: "pi",
    reason,
  }});

  const deliver = (value: object): Promise<void> => {{
    delivery = delivery
      .then(
        () =>
          new Promise<void>((resolve) => {{
            let finished = false;
            const child = spawn(hook, [], {{ stdio: ["pipe", "ignore", "ignore"] }});
            const done = () => {{
              if (!finished) {{
                finished = true;
                resolve();
              }}
            }};
            child.once("error", done);
            child.once("close", done);
            child.stdin.on("error", done);
            child.stdin.end(JSON.stringify(value));
            const timer = setTimeout(() => {{
              child.kill();
              done();
            }}, 1000);
            timer.unref();
          }}),
      )
      .catch(() => undefined);
    return delivery;
  }};

  const stopPending = async (ctx: ExtensionContext, reason?: string) => {{
    for (const turnId of pendingTurns) {{
      await deliver(payload(ctx, "Stop", turnId, reason));
    }}
    pendingTurns.clear();
  }};

  pi.on("session_start", async (event, ctx) => {{
    await deliver(payload(ctx, "SessionStart", undefined, event.reason));
  }});

  pi.on("before_agent_start", async (_event, ctx) => {{
    const turnId = randomUUID();
    pendingTurns.add(turnId);
    await deliver(payload(ctx, "UserPromptSubmit", turnId));
  }});

  pi.on("agent_settled", async (_event, ctx) => {{
    await stopPending(ctx, "settled");
  }});

  pi.on("session_shutdown", async (event, ctx) => {{
    await stopPending(ctx, event.reason);
    await deliver(payload(ctx, "SessionEnd", undefined, event.reason));
  }});
}}
"#,
        marker = PI_EXTENSION_MARKER,
        script = script,
    )
}

pub async fn ingest_lifecycle(env: &RuntimeEnv, agent: Agent) -> Result<()> {
    if !matches!(
        agent,
        Agent::Codex | Agent::Claude | Agent::Pi | Agent::Hermes
    ) {
        bail!("{agent} does not provide supported lifecycle hook input");
    }
    if std::env::var_os("SVAROG_RECOMMENDER").is_some() {
        println!("{{}}");
        return Ok(());
    }
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if let Ok(payload) = parse_lifecycle_payload(agent, &input) {
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

fn parse_lifecycle_payload(agent: Agent, input: &str) -> Result<LifecycleHookEvent> {
    if agent != Agent::Hermes {
        return serde_json::from_str(input).context("parsing lifecycle hook payload");
    }
    let root: Value = serde_json::from_str(input).context("parsing Hermes hook payload")?;
    let event = root
        .get("hook_event_name")
        .and_then(Value::as_str)
        .context("Hermes hook payload is missing hook_event_name")?;
    let hook_event_name = match event {
        "on_session_start" => "SessionStart",
        "pre_llm_call" => "UserPromptSubmit",
        "on_session_end" => "Stop",
        "on_session_finalize" => "SessionEnd",
        _ => bail!("unsupported Hermes lifecycle event"),
    };
    let extra = root.get("extra").and_then(Value::as_object);
    let text = |key: &str| {
        extra
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Ok(LifecycleHookEvent {
        session_id: root
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        turn_id: text("turn_id"),
        cwd: root
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        hook_event_name: hook_event_name.to_owned(),
        source: Some("hermes".to_owned()),
        reason: text("reason").or_else(|| {
            (event == "on_session_end").then(|| {
                let completed = extra
                    .and_then(|value| value.get("completed"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let interrupted = extra
                    .and_then(|value| value.get("interrupted"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if interrupted {
                    "interrupted"
                } else if completed {
                    "completed"
                } else {
                    "incomplete"
                }
                .to_owned()
            })
        }),
    })
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
            pi_config_dir: root.join("pi"),
            hermes_home: root.join("hermes"),
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
    fn official_claude_prompt_payload_is_sanitized() {
        let payload: LifecycleHookEvent = serde_json::from_str(
            r#"{
                "session_id":"session-1",
                "cwd":"/work/svarog",
                "hook_event_name":"UserPromptSubmit",
                "prompt":"private prompt text",
                "transcript_path":"/private/transcript.jsonl"
            }"#,
        )
        .unwrap();
        let forwarded = serde_json::to_value(payload).unwrap();

        assert!(forwarded["turn_id"].is_null());
        assert!(forwarded.get("prompt").is_none());
        assert!(forwarded.get("transcript_path").is_none());
    }

    #[test]
    fn official_claude_lifecycle_payloads_keep_only_shared_fields() {
        for input in [
            r#"{"session_id":"session-1","transcript_path":"/private/transcript.jsonl","cwd":"/work/svarog","permission_mode":"default","hook_event_name":"SessionStart","source":"startup","model":"claude-sonnet-4-5"}"#,
            r#"{"session_id":"session-1","transcript_path":"/private/transcript.jsonl","cwd":"/work/svarog","permission_mode":"default","hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"private response","background_tasks":[],"session_crons":[]}"#,
            r#"{"session_id":"session-1","transcript_path":"/private/transcript.jsonl","cwd":"/work/svarog","hook_event_name":"SessionEnd","reason":"other"}"#,
        ] {
            let payload: LifecycleHookEvent = serde_json::from_str(input).unwrap();
            payload.validate().unwrap();
            let forwarded = serde_json::to_value(payload).unwrap();

            assert!(forwarded.get("transcript_path").is_none());
            assert!(forwarded.get("permission_mode").is_none());
            assert!(forwarded.get("model").is_none());
            assert!(forwarded.get("last_assistant_message").is_none());
            assert!(forwarded.get("background_tasks").is_none());
            assert!(forwarded.get("session_crons").is_none());
        }
    }

    #[test]
    fn hermes_payloads_map_lifecycle_and_discard_private_content() {
        let cases = [
            ("on_session_start", "SessionStart", None),
            ("pre_llm_call", "UserPromptSubmit", Some("turn-1")),
            ("on_session_end", "Stop", Some("turn-1")),
            ("on_session_finalize", "SessionEnd", None),
        ];
        for (event, expected, turn_id) in cases {
            let input = json!({
                "hook_event_name": event,
                "session_id": "session-1",
                "cwd": "/work/svarog",
                "tool_input": {"secret": "private tool input"},
                "extra": {
                    "turn_id": turn_id,
                    "user_message": "private prompt",
                    "conversation_history": ["private history"],
                    "assistant_response": "private response",
                    "completed": true
                }
            });
            let payload = parse_lifecycle_payload(Agent::Hermes, &input.to_string()).unwrap();
            payload.validate().unwrap();
            assert_eq!(payload.hook_event_name, expected);
            assert_eq!(payload.turn_id.as_deref(), turn_id);
            assert_eq!(payload.source.as_deref(), Some("hermes"));
            let forwarded = serde_json::to_value(payload).unwrap();
            for private in [
                "tool_input",
                "user_message",
                "conversation_history",
                "assistant_response",
            ] {
                assert!(forwarded.get(private).is_none());
            }
        }
    }

    #[test]
    fn hermes_reconcile_preserves_yaml_comments_hooks_and_scoped_approvals() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.hermes_home).unwrap();
        let config_path = env.hermes_home.join("config.yaml");
        let allowlist_path = env.hermes_home.join("shell-hooks-allowlist.json");
        fs::write(
            &config_path,
            "# keep this comment\nmodel:\n  default: test-model\nhooks:\n  pre_llm_call:\n    - {command: \"echo keep\", timeout: 9}\n  outbound: []\n",
        )
        .unwrap();
        fs::write(
            &allowlist_path,
            r#"{"approvals":[{"event":"pre_llm_call","command":"echo keep","approved_at":"earlier"}]}"#,
        )
        .unwrap();

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();

        let first_config = fs::read(&config_path).unwrap();
        let first_allowlist = fs::read(&allowlist_path).unwrap();
        let config = String::from_utf8(first_config.clone()).unwrap();
        assert!(config.contains("# keep this comment"));
        assert!(config.contains("default: test-model"));
        assert!(config.contains("command: \"echo keep\""));
        assert!(config.contains("outbound: []"));
        for event in HERMES_EVENTS.map(|(event, _)| event) {
            assert!(config.contains(event));
        }
        let allowlist: Value = serde_json::from_slice(&first_allowlist).unwrap();
        assert_eq!(allowlist["approvals"].as_array().unwrap().len(), 5);
        assert!(allowlist["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["command"] == "echo keep"));
        assert!(is_configured(&env, CodingAgentSelection::Hermes).unwrap());

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();
        assert_eq!(fs::read(&config_path).unwrap(), first_config);
        assert_eq!(fs::read(&allowlist_path).unwrap(), first_allowlist);

        reconcile(&env, CodingAgentSelection::Codex).unwrap();
        let config = fs::read_to_string(&config_path).unwrap();
        assert!(config.contains("echo keep"));
        assert!(!config.contains("hermes-event.sh"));
        let allowlist: Value = serde_json::from_slice(&fs::read(&allowlist_path).unwrap()).unwrap();
        assert_eq!(allowlist["approvals"].as_array().unwrap().len(), 1);
        assert_eq!(allowlist["approvals"][0]["command"], "echo keep");
    }

    #[test]
    fn hermes_reconcile_creates_a_valid_idempotent_config() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();

        let config_path = env.hermes_home.join("config.yaml");
        let first = fs::read_to_string(&config_path).unwrap();
        let yaml = YamlFile::from_str(&first).unwrap();
        let root = yaml.document().unwrap().as_mapping().unwrap();
        let hooks = root.get_mapping("hooks").unwrap();
        for (event, _) in HERMES_EVENTS {
            assert!(root.get(event).is_none());
            assert_eq!(hooks.get_sequence(event).unwrap().values().count(), 1);
        }
        assert!(is_configured(&env, CodingAgentSelection::Hermes).unwrap());

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();
        assert_eq!(fs::read_to_string(config_path).unwrap(), first);
    }

    #[test]
    fn hermes_reconcile_adds_a_nested_mapping_to_an_existing_config() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.hermes_home).unwrap();
        let config_path = env.hermes_home.join("config.yaml");
        fs::write(
            &config_path,
            "# keep this comment\nmodel:\n  default: test-model\n",
        )
        .unwrap();

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();

        let first = fs::read_to_string(&config_path).unwrap();
        assert!(first.contains("# keep this comment"));
        assert!(first.contains("default: test-model"));
        let yaml = YamlFile::from_str(&first).unwrap();
        let root = yaml.document().unwrap().as_mapping().unwrap();
        let hooks = root.get_mapping("hooks").unwrap();
        for (event, _) in HERMES_EVENTS {
            assert!(
                root.get(event).is_none(),
                "{event} escaped the hooks mapping"
            );
            let entries = hooks.get_sequence(event).unwrap();
            assert_eq!(entries.values().count(), 1);
        }
        assert!(is_configured(&env, CodingAgentSelection::Hermes).unwrap());

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();
        assert_eq!(fs::read_to_string(config_path).unwrap(), first);
    }

    #[test]
    fn hermes_reconcile_repairs_misplaced_svarog_hooks_and_preserves_other_entries() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.hermes_home).unwrap();
        let config_path = env.hermes_home.join("config.yaml");
        fs::write(
            &config_path,
            "# keep this comment\nmodel: test-model\non_session_start:\n  - {command: \"'/old/hermes-event.sh'\", timeout: 5}\n  - {command: \"echo keep\", timeout: 9}\non_session_start:\n  - {command: \"'/older/hermes-event.sh'\", timeout: 5}\npre_llm_call:\n  - {command: \"'/old/hermes-event.sh'\", timeout: 5}\non_session_end:\n  - {command: \"'/old/hermes-event.sh'\", timeout: 5}\non_session_finalize:\n  - {command: \"'/old/hermes-event.sh'\", timeout: 3}\nhooks:\n",
        )
        .unwrap();

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();

        let first = fs::read_to_string(&config_path).unwrap();
        assert!(first.contains("# keep this comment"));
        let yaml = YamlFile::from_str(&first).unwrap();
        let root = yaml.document().unwrap().as_mapping().unwrap();
        let hooks = root.get_mapping("hooks").unwrap();
        let misplaced = root.get_sequence("on_session_start").unwrap();
        assert_eq!(misplaced.values().count(), 1);
        assert!(misplaced
            .values()
            .next()
            .is_some_and(|entry| !is_managed_hermes_hook(&entry)));
        for (event, _) in HERMES_EVENTS {
            if event != "on_session_start" {
                assert!(root.get(event).is_none());
            }
            let entries = hooks.get_sequence(event).unwrap();
            assert_eq!(entries.values().count(), 1);
            assert!(entries.values().next().is_some_and(|entry| {
                is_managed_hermes_hook(&entry)
                    && entry
                        .as_mapping()
                        .and_then(|mapping| mapping.get("command"))
                        .and_then(|value| value.as_scalar().map(|scalar| scalar.as_string()))
                        .is_some_and(|command| !command.contains("/old/"))
            }));
        }

        reconcile(&env, CodingAgentSelection::Hermes).unwrap();
        assert_eq!(fs::read_to_string(config_path).unwrap(), first);
    }

    #[test]
    fn hermes_deselection_cleans_up_misplaced_svarog_hooks() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.hermes_home).unwrap();
        let config_path = env.hermes_home.join("config.yaml");
        fs::write(
            &config_path,
            "model: test-model\non_session_start:\n  - {command: \"'/old/hermes-event.sh'\", timeout: 5}\nhooks:\n",
        )
        .unwrap();

        reconcile(&env, CodingAgentSelection::Codex).unwrap();

        let config = fs::read_to_string(config_path).unwrap();
        let yaml = YamlFile::from_str(&config).unwrap();
        let root = yaml.document().unwrap().as_mapping().unwrap();
        assert!(root.get("hooks").is_none());
        assert!(root.get("on_session_start").is_none());
        assert!(config.contains("model: test-model"));
    }

    #[test]
    fn non_mapping_hermes_hooks_are_preserved_with_an_accurate_error() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.hermes_home).unwrap();
        let config_path = env.hermes_home.join("config.yaml");
        let allowlist_path = env.hermes_home.join("shell-hooks-allowlist.json");
        fs::write(&config_path, "hooks: []\n").unwrap();
        fs::write(&allowlist_path, r#"{"approvals":[]}"#).unwrap();

        let error = reconcile(&env, CodingAgentSelection::Hermes).unwrap_err();

        assert!(error.to_string().contains("found a sequence"));
        assert_eq!(fs::read_to_string(config_path).unwrap(), "hooks: []\n");
        assert_eq!(
            fs::read_to_string(allowlist_path).unwrap(),
            r#"{"approvals":[]}"#
        );
    }

    #[test]
    fn malformed_hermes_config_is_preserved_without_touching_allowlist() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.hermes_home).unwrap();
        let config_path = env.hermes_home.join("config.yaml");
        let allowlist_path = env.hermes_home.join("shell-hooks-allowlist.json");
        fs::write(&config_path, "hooks: [not closed").unwrap();
        fs::write(&allowlist_path, r#"{"approvals":[]}"#).unwrap();

        let error = reconcile(&env, CodingAgentSelection::Hermes).unwrap_err();

        assert!(error
            .to_string()
            .contains("repair the existing Hermes hook"));
        assert_eq!(
            fs::read_to_string(&config_path).unwrap(),
            "hooks: [not closed"
        );
        assert_eq!(
            fs::read_to_string(&allowlist_path).unwrap(),
            r#"{"approvals":[]}"#
        );
    }

    #[test]
    fn duplicate_hermes_hook_keys_are_rejected_without_changes() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.hermes_home).unwrap();
        let config_path = env.hermes_home.join("config.yaml");
        let original = "hooks:\n  pre_llm_call: []\n  pre_llm_call: []\n";
        fs::write(&config_path, original).unwrap();

        let error = reconcile(&env, CodingAgentSelection::Hermes).unwrap_err();

        assert!(error.to_string().contains("duplicate YAML key"));
        assert_eq!(fs::read_to_string(config_path).unwrap(), original);
    }

    #[test]
    fn reconcile_installs_all_agents_and_removes_only_deselected_hooks() {
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
        let pi_path = env.pi_config_dir.join("extensions/svarog.ts");
        let pi = fs::read_to_string(&pi_path).unwrap();
        assert!(pi.starts_with(PI_EXTENSION_MARKER));
        for event in [
            "session_start",
            "before_agent_start",
            "agent_settled",
            "session_shutdown",
        ] {
            assert!(pi.contains(event));
        }
        assert!(!pi.contains("_event.prompt"));
        assert!(!pi.contains("_event.images"));
        assert_eq!(
            fs::metadata(&pi_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(is_configured(&env, CodingAgentSelection::All).unwrap());

        reconcile(&env, CodingAgentSelection::Claude).unwrap();

        let codex: Value =
            serde_json::from_slice(&fs::read(env.codex_home.join("hooks.json")).unwrap()).unwrap();
        assert_eq!(codex["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(
            codex["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo keep"
        );
        assert!(!pi_path.exists());
        assert!(is_configured(&env, CodingAgentSelection::Claude).unwrap());
    }

    #[test]
    fn pi_install_preserves_unowned_extensions_and_rejects_name_collisions() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        let extensions = env.pi_config_dir.join("extensions");
        fs::create_dir_all(&extensions).unwrap();
        fs::write(extensions.join("mine.ts"), "export default () => {};").unwrap();
        fs::write(extensions.join("svarog.ts"), "user-owned").unwrap();

        let error = reconcile(&env, CodingAgentSelection::Pi).unwrap_err();

        assert!(error.to_string().contains("not managed by Svarog"));
        assert_eq!(
            fs::read_to_string(extensions.join("svarog.ts")).unwrap(),
            "user-owned"
        );
        assert_eq!(
            fs::read_to_string(extensions.join("mine.ts")).unwrap(),
            "export default () => {};"
        );
    }

    #[test]
    fn pi_install_rejects_a_symlinked_extension() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        let extensions = env.pi_config_dir.join("extensions");
        fs::create_dir_all(&extensions).unwrap();
        let target = root.path().join("target.ts");
        fs::write(&target, "user-owned").unwrap();
        std::os::unix::fs::symlink(&target, extensions.join("svarog.ts")).unwrap();

        let error = install_global_pi(&env).unwrap_err();

        assert!(error.to_string().contains("is a symlink"));
        assert_eq!(fs::read_to_string(target).unwrap(), "user-owned");
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
    fn reconcile_preflights_destinations_before_writing_settings() {
        let root = tempdir().unwrap();
        let mut env = test_env(root.path());
        fs::create_dir_all(&env.codex_home).unwrap();
        let original = br#"{"theme":"keep"}"#;
        fs::write(env.codex_home.join("hooks.json"), original).unwrap();
        let blocked_parent = root.path().join("not-a-directory");
        fs::write(&blocked_parent, "blocked").unwrap();
        env.claude_config_dir = blocked_parent.join("claude");

        assert!(reconcile(&env, CodingAgentSelection::All).is_err());
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

    #[test]
    fn claude_install_preserves_absolute_and_relative_settings_symlinks() {
        for relative in [false, true] {
            let root = tempdir().unwrap();
            let env = test_env(root.path());
            fs::create_dir_all(&env.claude_config_dir).unwrap();
            let target_dir = root.path().join("dotfiles");
            fs::create_dir_all(&target_dir).unwrap();
            let target = target_dir.join("claude-settings.json");
            fs::write(&target, r#"{"theme":"dark"}"#).unwrap();
            let link = env.claude_config_dir.join("settings.json");
            let link_target = if relative {
                PathBuf::from("../dotfiles/claude-settings.json")
            } else {
                target.clone()
            };
            std::os::unix::fs::symlink(link_target, &link).unwrap();

            install_global_claude(&env).unwrap();

            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            let value: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
            assert_eq!(value["theme"], "dark");
            assert_eq!(
                value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
                1
            );
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn claude_install_rejects_a_dangling_settings_symlink() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        fs::create_dir_all(&env.claude_config_dir).unwrap();
        let link = env.claude_config_dir.join("settings.json");
        std::os::unix::fs::symlink("missing.json", &link).unwrap();

        let error = install_global_claude(&env).unwrap_err();

        assert!(error.to_string().contains("dangling link"));
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    }

    #[test]
    fn reconcile_rolls_back_a_completed_write() {
        let root = tempdir().unwrap();
        let first = root.path().join("first.json");
        let second = root.path().join("second.json");
        fs::write(&first, b"before").unwrap();
        fs::write(&second, b"before").unwrap();
        let prepared = vec![
            PreparedSettings {
                path: first.clone(),
                target: first.clone(),
                original: Some(b"before".to_vec()),
                updated: Some(b"after".to_vec()),
                remove: false,
            },
            PreparedSettings {
                path: second.clone(),
                target: second.clone(),
                original: Some(b"before".to_vec()),
                updated: Some(b"after".to_vec()),
                remove: false,
            },
        ];
        let mut writes = 0;

        let error = apply_prepared(
            &prepared,
            |path, contents| {
                writes += 1;
                if writes == 2 {
                    bail!("injected write failure");
                }
                atomic_write_resolved_user_only(path, contents)
            },
            restore_prepared,
        )
        .unwrap_err();

        assert!(error.to_string().contains("previous settings restored"));
        assert_eq!(fs::read(first).unwrap(), b"before");
        assert_eq!(fs::read(second).unwrap(), b"before");
    }

    #[test]
    fn reconcile_reports_write_and_rollback_failures() {
        let item = PreparedSettings {
            path: PathBuf::from("first.json"),
            target: PathBuf::from("first.json"),
            original: Some(b"before".to_vec()),
            updated: Some(b"after".to_vec()),
            remove: false,
        };
        let prepared = [
            item,
            PreparedSettings {
                path: PathBuf::from("second.json"),
                target: PathBuf::from("second.json"),
                original: None,
                updated: Some(b"after".to_vec()),
                remove: false,
            },
        ];
        let mut writes = 0;

        let error = apply_prepared(
            &prepared,
            |_path, _contents| {
                writes += 1;
                if writes == 2 {
                    bail!("write failed");
                }
                Ok(())
            },
            |_item| bail!("restore failed"),
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("write failed"));
        assert!(message.contains("restore failed"));
    }

    #[test]
    fn claude_status_warns_without_marking_installed_hooks_missing() {
        let root = tempdir().unwrap();
        let env = test_env(root.path());
        reconcile(&env, CodingAgentSelection::Claude).unwrap();
        let path = env.claude_config_dir.join("settings.json");
        let mut settings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        settings["disableAllHooks"] = json!(true);
        fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();

        let status = integration_status(&env, CodingAgentSelection::Claude).unwrap();

        assert!(status.configured);
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("disableAllHooks=true")));
    }

    #[test]
    fn managed_hook_policy_uses_sorted_dropin_precedence() {
        let root = tempdir().unwrap();
        fs::write(
            root.path().join("managed-settings.json"),
            r#"{"allowManagedHooksOnly":true}"#,
        )
        .unwrap();
        assert_eq!(
            file_managed_claude_setting_in(root.path(), "allowManagedHooksOnly").unwrap(),
            Some(true)
        );
        let dropins = root.path().join("managed-settings.d");
        fs::create_dir(&dropins).unwrap();
        fs::write(
            dropins.join("10-enable-user-hooks.json"),
            r#"{"allowManagedHooksOnly":false}"#,
        )
        .unwrap();

        assert_eq!(
            file_managed_claude_setting_in(root.path(), "allowManagedHooksOnly").unwrap(),
            Some(false)
        );
    }
}
