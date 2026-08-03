use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::source_fingerprint;

const SOURCE_ROOT: &str = env!("SVAROG_SOURCE_ROOT");
const SOURCE_FINGERPRINT: &str = env!("SVAROG_SOURCE_FINGERPRINT");
const SKIP_ENV: &str = "SVAROG_SKIP_SELF_UPDATE";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdatePolicy {
    Ask,
    Always,
    Never,
}

pub fn maybe_update() {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let policy = match update_policy(env::var_os("SVAROG_UPDATE").as_deref()) {
        Ok(policy) => policy,
        Err(message) => {
            eprintln!("Warning: {message}; continuing without checking for an update.");
            return;
        }
    };

    if env::var_os(SKIP_ENV).is_some()
        || env::var_os("SVAROG_RECOMMENDER").is_some()
        || !should_check(
            &args,
            policy,
            io::stdin().is_terminal(),
            io::stdout().is_terminal(),
        )
    {
        return;
    }

    let root = Path::new(SOURCE_ROOT);
    if !root.is_dir() {
        return;
    }

    let current = match source_fingerprint::fingerprint(root) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("Warning: could not check the local Svarog source for updates: {error}");
            return;
        }
    };
    if current == SOURCE_FINGERPRINT {
        return;
    }

    if policy == UpdatePolicy::Ask && !confirm_update() {
        return;
    }

    eprintln!("Installing the changed local Svarog checkout...");
    if let Err(error) = install(root) {
        eprintln!("Warning: could not install the local Svarog update: {error}");
        eprintln!("Continuing with the currently installed version.");
        return;
    }

    let replacement = match installed_binary() {
        Some(path) if path.is_file() => path,
        _ => {
            eprintln!("Warning: cargo finished, but the installed Svarog binary was not found.");
            eprintln!("Continuing with the currently running version.");
            return;
        }
    };

    if let Err(error) = replace_process(&replacement, &args) {
        eprintln!("Warning: could not restart the updated Svarog binary: {error}");
        eprintln!("Continuing with the currently running version.");
    }
}

fn update_policy(value: Option<&OsStr>) -> Result<UpdatePolicy, String> {
    match value.and_then(OsStr::to_str).unwrap_or("ask") {
        "ask" => Ok(UpdatePolicy::Ask),
        "always" => Ok(UpdatePolicy::Always),
        "never" => Ok(UpdatePolicy::Never),
        value => Err(format!(
            "SVAROG_UPDATE must be ask, always, or never (received {value:?})"
        )),
    }
}

fn should_check(
    args: &[OsString],
    policy: UpdatePolicy,
    stdin_terminal: bool,
    stdout_terminal: bool,
) -> bool {
    if policy == UpdatePolicy::Never {
        return false;
    }

    if let Some(command) = args.first().and_then(|value| value.to_str()) {
        if matches!(command, "codex-hook" | "daemon" | "event" | "stop")
            || matches!(command, "--help" | "-h" | "--version" | "-V" | "help")
        {
            return false;
        }
    }

    policy == UpdatePolicy::Always || (stdin_terminal && stdout_terminal)
}

fn confirm_update() -> bool {
    eprint!("Local Svarog source has changed. Install it before continuing? [Y/n] ");
    if io::stderr().flush().is_err() {
        return false;
    }

    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(_) => !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no"),
        Err(error) => {
            eprintln!("Warning: could not read update choice: {error}");
            false
        }
    }
}

fn install(root: &Path) -> io::Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(cargo)
        .arg("install")
        .arg("--locked")
        .arg("--path")
        .arg(root)
        .arg("--force")
        .env(SKIP_ENV, "1")
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "cargo install exited with {status}"
        )))
    }
}

fn installed_binary() -> Option<PathBuf> {
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    Some(
        cargo_home
            .join("bin")
            .join(format!("svarog{}", env::consts::EXE_SUFFIX)),
    )
}

#[cfg(unix)]
fn replace_process(binary: &Path, args: &[OsString]) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    let error = Command::new(binary).args(args).env(SKIP_ENV, "1").exec();
    Err(error)
}

#[cfg(not(unix))]
fn replace_process(binary: &Path, args: &[OsString]) -> io::Result<()> {
    let status = Command::new(binary)
        .args(args)
        .env(SKIP_ENV, "1")
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn policy_defaults_to_ask_and_parses_overrides() {
        assert_eq!(update_policy(None).unwrap(), UpdatePolicy::Ask);
        assert_eq!(
            update_policy(Some(OsStr::new("always"))).unwrap(),
            UpdatePolicy::Always
        );
        assert_eq!(
            update_policy(Some(OsStr::new("never"))).unwrap(),
            UpdatePolicy::Never
        );
        assert!(update_policy(Some(OsStr::new("sometimes"))).is_err());
    }

    #[test]
    fn ask_only_checks_interactive_user_commands() {
        assert!(should_check(
            &args(&["demo"]),
            UpdatePolicy::Ask,
            true,
            true
        ));
        assert!(should_check(&[], UpdatePolicy::Ask, true, true));
        assert!(!should_check(
            &args(&["run"]),
            UpdatePolicy::Ask,
            false,
            true
        ));
        assert!(!should_check(
            &args(&["run"]),
            UpdatePolicy::Ask,
            true,
            false
        ));
    }

    #[test]
    fn always_supports_noninteractive_user_commands() {
        assert!(should_check(
            &args(&["status"]),
            UpdatePolicy::Always,
            false,
            false
        ));
        assert!(!should_check(
            &args(&["status"]),
            UpdatePolicy::Never,
            true,
            true
        ));
    }

    #[test]
    fn internal_and_metadata_commands_never_check() {
        for command in [
            "codex-hook",
            "daemon",
            "event",
            "stop",
            "--help",
            "-h",
            "--version",
            "-V",
            "help",
        ] {
            assert!(!should_check(
                &args(&[command]),
                UpdatePolicy::Always,
                true,
                true
            ));
        }
    }
}
