use crate::config::{Paths, RuntimeEnv, RuntimeMode};
use anyhow::{bail, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPOSITORY: &str = "zotttttttt/svarog";
const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;
const SKIP_ENV: &str = "SVAROG_SKIP_UPDATE_CHECK";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub tag: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateRequest {
    Development,
    Release(AvailableUpdate),
}

#[derive(Debug, Deserialize, Serialize)]
struct UpdateState {
    #[serde(default = "update_state_version")]
    version: u32,
    #[serde(default)]
    last_checked_unix: i64,
    #[serde(default)]
    last_prompted_version: Option<String>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            version: update_state_version(),
            last_checked_unix: 0,
            last_prompted_version: None,
        }
    }
}

fn update_state_version() -> u32 {
    1
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

pub fn maybe_prompt_startup(env: &RuntimeEnv) {
    if env.mode != RuntimeMode::Production
        || env::var_os(SKIP_ENV).is_some()
        || !io::stdin().is_terminal()
        || !io::stderr().is_terminal()
        || !startup_command_is_interactive()
    {
        return;
    }

    let mut state = load_state(&env.paths).unwrap_or_default();
    let now = unix_now();
    if !check_is_due(state.last_checked_unix, now) {
        return;
    }

    let available = match check_latest() {
        Ok(available) => available,
        Err(_) => return,
    };
    state.last_checked_unix = now;

    let Some(available) = available else {
        let _ = save_state(&env.paths, &state);
        return;
    };
    if version_was_prompted(&state, &available.version) {
        let _ = save_state(&env.paths, &state);
        return;
    }

    state.last_prompted_version = Some(available.version.to_string());
    let _ = save_state(&env.paths, &state);
    if !confirm(&format!(
        "Svarog {} is available. Install it before continuing? [Y/n] ",
        available.version
    )) {
        return;
    }

    eprintln!("Updating Svarog to {}...", available.version);
    if let Err(error) = install_release(&available, &original_args()) {
        eprintln!("Warning: could not update Svarog: {error:#}");
        eprintln!("Continuing with the currently installed version.");
    }
}

fn check_is_due(last_checked_unix: i64, now: i64) -> bool {
    now.saturating_sub(last_checked_unix) >= CHECK_INTERVAL_SECS
}

fn version_was_prompted(state: &UpdateState, version: &Version) -> bool {
    let version = version.to_string();
    state.last_prompted_version.as_deref() == Some(version.as_str())
}

pub fn check_latest_async() -> Receiver<Result<Option<AvailableUpdate>, String>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(check_latest().map_err(|error| format!("{error:#}")));
    });
    receiver
}

pub fn perform(request: UpdateRequest) -> Result<()> {
    match request {
        UpdateRequest::Development => rebuild_development(),
        UpdateRequest::Release(available) => {
            println!("Updating Svarog to {}...", available.version);
            install_release(&available, &original_args())
        }
    }
}

pub fn is_development_checkout() -> bool {
    env::var_os("SVAROG_DEV_LAUNCHER").is_some()
}

pub fn current_version_label(development_checkout: bool) -> String {
    if development_checkout {
        format!("{} [dev]", env!("CARGO_PKG_VERSION"))
    } else {
        env!("CARGO_PKG_VERSION").to_string()
    }
}

fn startup_command_is_interactive() -> bool {
    match env::args_os()
        .nth(1)
        .and_then(|value| value.into_string().ok())
    {
        None => true,
        Some(command) => matches!(command.as_str(), "run"),
    }
}

fn check_latest() -> Result<Option<AvailableUpdate>> {
    let api_url = env::var("SVAROG_RELEASE_API_URL")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(format!("svarog/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating update client")?;
    let release: LatestRelease = client
        .get(api_url)
        .send()
        .context("contacting GitHub")?
        .error_for_status()
        .context("checking the latest GitHub release")?
        .json()
        .context("reading the latest GitHub release")?;
    available_update(env!("CARGO_PKG_VERSION"), &release.tag_name)
}

fn available_update(current: &str, tag: &str) -> Result<Option<AvailableUpdate>> {
    let current = Version::parse(current).context("parsing the installed Svarog version")?;
    let version_text = tag.strip_prefix('v').unwrap_or(tag);
    let version =
        Version::parse(version_text).with_context(|| format!("parsing release version {tag:?}"))?;
    Ok((version > current).then(|| AvailableUpdate {
        version,
        tag: tag.to_string(),
    }))
}

fn install_release(update: &AvailableUpdate, args: &[OsString]) -> Result<()> {
    let installer_url = env::var("SVAROG_INSTALLER_URL").unwrap_or_else(|_| {
        format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/svarog-installer.sh",
            update.tag
        )
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("svarog/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating installer client")?;
    let installer = client
        .get(installer_url)
        .send()
        .context("downloading the Svarog installer")?
        .error_for_status()
        .context("downloading the Svarog installer")?
        .bytes()
        .context("reading the Svarog installer")?;
    let mut file = tempfile::NamedTempFile::new().context("creating an installer file")?;
    file.write_all(&installer)
        .context("writing the Svarog installer")?;

    let executable = env::current_exe().context("locating the current Svarog executable")?;
    let install_dir = executable
        .parent()
        .context("the current Svarog executable has no parent directory")?;
    let status = Command::new("bash")
        .arg(file.path())
        .env("SVAROG_INSTALL_DIR", install_dir)
        .env(SKIP_ENV, "1")
        .status()
        .context("running the Svarog installer")?;
    if !status.success() {
        bail!("the Svarog installer exited with {status}");
    }
    restart(&executable, args)
}

fn rebuild_development() -> Result<()> {
    let launcher = env::var_os("SVAROG_DEV_LAUNCHER")
        .map(PathBuf::from)
        .context("this development build was not started through scripts/svarog")?;
    let mut command = Command::new(&launcher);
    command.arg("--update").args(original_args());
    replace_command(command)
}

fn original_args() -> Vec<OsString> {
    env::args_os().skip(1).collect()
}

fn confirm(prompt: &str) -> bool {
    eprint!("{prompt}");
    if io::stderr().flush().is_err() {
        return false;
    }
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .is_ok_and(|_| !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no"))
}

fn state_file(paths: &Paths) -> PathBuf {
    paths.config_dir.join("update-state.toml")
}

fn load_state(paths: &Paths) -> Result<UpdateState> {
    let path = state_file(paths);
    if !path.exists() {
        return Ok(UpdateState::default());
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parsing {}", path.display()))
}

fn save_state(paths: &Paths, state: &UpdateState) -> Result<()> {
    fs::create_dir_all(&paths.config_dir)
        .with_context(|| format!("creating {}", paths.config_dir.display()))?;
    let path = state_file(paths);
    let contents = toml::to_string(state).context("serializing update state")?;
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn restart(executable: &Path, args: &[OsString]) -> Result<()> {
    let mut command = Command::new(executable);
    command.args(args).env(SKIP_ENV, "1");
    replace_command(command)
}

#[cfg(unix)]
fn replace_command(mut command: Command) -> Result<()> {
    use std::os::unix::process::CommandExt;
    Err(command.exec()).context("restarting Svarog")
}

#[cfg(not(unix))]
fn replace_command(mut command: Command) -> Result<()> {
    let status = command.status().context("restarting Svarog")?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_semantic_release_is_available() {
        let update = available_update("0.6.2", "v0.6.3").unwrap().unwrap();
        assert_eq!(update.version, Version::new(0, 6, 3));
        assert_eq!(update.tag, "v0.6.3");
        assert!(available_update("0.6.3", "v0.6.3").unwrap().is_none());
        assert!(available_update("0.7.0", "v0.6.3").unwrap().is_none());
    }

    #[test]
    fn update_state_round_trips_outside_sqlite() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let state = UpdateState {
            version: 1,
            last_checked_unix: 123,
            last_prompted_version: Some("0.6.3".into()),
        };
        save_state(&paths, &state).unwrap();
        let loaded = load_state(&paths).unwrap();
        assert_eq!(loaded.last_checked_unix, 123);
        assert_eq!(loaded.last_prompted_version.as_deref(), Some("0.6.3"));
        assert!(!paths.database_file.exists());
    }

    #[test]
    fn startup_checks_are_daily_and_each_release_prompts_once() {
        assert!(!check_is_due(100, 100 + CHECK_INTERVAL_SECS - 1));
        assert!(check_is_due(100, 100 + CHECK_INTERVAL_SECS));

        let state = UpdateState {
            version: 1,
            last_checked_unix: 0,
            last_prompted_version: Some("0.6.3".into()),
        };
        assert!(version_was_prompted(&state, &Version::new(0, 6, 3)));
        assert!(!version_was_prompted(&state, &Version::new(0, 6, 4)));
    }
}
