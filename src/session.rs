use crate::config::{load_or_default, RuntimeEnv};
use crate::hooks;
use crate::models::Agent;
use anyhow::{bail, Context, Result};
use std::process::{Command, Stdio};

pub fn run(agent: Agent, env: &RuntimeEnv) -> Result<()> {
    let paths = &env.paths;
    let config = load_or_default(paths)?;
    let store = crate::storage::Store::open(&paths.database_file)?;
    store.insert_session(agent, None)?;
    let _ = hooks::install(env, agent);
    let agent_command = match agent {
        Agent::Codex => config.agents.codex_command,
        Agent::Claude => "claude".to_string(),
        Agent::Droid => "droid".to_string(),
        Agent::FactoryDroid => "factory-droid".to_string(),
        Agent::OpenClaw => "openclaw".to_string(),
        Agent::Custom => std::env::var("SVAROG_AGENT_COMMAND").unwrap_or_else(|_| "sh".to_string()),
    };

    ensure_tmux()?;

    let session_name = format!("svarog-{}", agent.as_str());
    let current_exe = std::env::current_exe().context("locating svarog executable")?;
    let tui_command = format!("{} run", current_exe.display());

    let has_session = Command::new("tmux")
        .args(["has-session", "-t", &session_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);

    if !has_session {
        run_tmux(&["new-session", "-d", "-s", &session_name, &agent_command])?;
        run_tmux(&[
            "split-window",
            "-h",
            "-p",
            "20",
            "-t",
            &session_name,
            &tui_command,
        ])?;
        run_tmux(&["select-pane", "-t", &format!("{session_name}:0.0")])?;
    }

    // Keep mouse handling local to the Svarog session. This lets users click
    // between panes without changing their global tmux configuration or
    // overriding any of their existing key bindings.
    run_tmux(&["set-option", "-t", &session_name, "mouse", "on"])?;

    run_tmux(&["attach-session", "-t", &session_name])
}

fn ensure_tmux() -> Result<()> {
    ensure_tmux_command("tmux")
}

fn ensure_tmux_command(command: &str) -> Result<()> {
    let status = Command::new(command)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        _ => bail!(
            "`svarog session` requires tmux. Install it with Homebrew or your system package manager."
        ),
    }
}

fn run_tmux(args: &[&str]) -> Result<()> {
    let status = Command::new("tmux")
        .args(args)
        .status()
        .context("running tmux")?;
    if !status.success() {
        bail!("tmux command failed: tmux {}", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tmux_reports_session_specific_installation_hint() {
        let error = ensure_tmux_command("svarog-test-missing-tmux-executable").unwrap_err();
        let message = error.to_string();

        assert!(message.contains("`svarog session` requires tmux"));
        assert!(message.contains("Homebrew or your system package manager"));
    }
}
