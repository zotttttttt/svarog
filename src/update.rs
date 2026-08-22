use crate::config::Paths;
use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const REPOSITORY: &str = "zotttttttt/svarog";
const SKIP_ENV: &str = "SVAROG_SKIP_UPDATE_CHECK";
const UPDATE_STATE_VERSION: u32 = 2;

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

pub type UpdateCheckReceiver = Receiver<Result<Option<AvailableUpdate>, String>>;

#[derive(Debug, Deserialize, Serialize)]
struct UpdateState {
    #[serde(default = "update_state_version")]
    version: u32,
    #[serde(default)]
    last_attempt_local_date: Option<String>,
    #[serde(default)]
    dismissed_version: Option<String>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            version: update_state_version(),
            last_attempt_local_date: None,
            dismissed_version: None,
        }
    }
}

fn update_state_version() -> u32 {
    UPDATE_STATE_VERSION
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

fn version_was_dismissed(state: &UpdateState, version: &Version) -> bool {
    let version = version.to_string();
    state.dismissed_version.as_deref() == Some(version.as_str())
}

pub fn start_scheduled_check(paths: &Paths) -> Result<Option<UpdateCheckReceiver>> {
    start_scheduled_check_for_date(paths, Local::now().date_naive())
}

fn start_scheduled_check_for_date(
    paths: &Paths,
    date: NaiveDate,
) -> Result<Option<UpdateCheckReceiver>> {
    if env::var_os(SKIP_ENV).is_some() {
        return Ok(None);
    }
    let mut state = load_state_recovering(paths);
    if !scheduled_check_due(&state, date) {
        return Ok(None);
    }
    state.last_attempt_local_date = Some(date.to_string());
    save_state(paths, &state)?;
    Ok(Some(check_latest_async()))
}

pub fn start_manual_check(paths: &Paths) -> Result<UpdateCheckReceiver> {
    let mut state = load_state_recovering(paths);
    state.last_attempt_local_date = Some(Local::now().date_naive().to_string());
    save_state(paths, &state)?;
    Ok(check_latest_async())
}

pub fn dismiss_version(paths: &Paths, version: &Version) -> Result<()> {
    let mut state = load_state_recovering(paths);
    state.dismissed_version = Some(version.to_string());
    save_state(paths, &state)
}

pub fn is_version_dismissed(paths: &Paths, version: &Version) -> bool {
    version_was_dismissed(&load_state_recovering(paths), version)
}

fn scheduled_check_due(state: &UpdateState, date: NaiveDate) -> bool {
    state.last_attempt_local_date.as_deref() != Some(date.to_string().as_str())
}

pub fn check_latest_async() -> UpdateCheckReceiver {
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

fn check_latest() -> Result<Option<AvailableUpdate>> {
    let api_url = env::var("SVAROG_RELEASE_API_URL")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"));
    check_latest_at(&api_url, env!("CARGO_PKG_VERSION"))
}

fn check_latest_at(api_url: &str, current_version: &str) -> Result<Option<AvailableUpdate>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(format!("svarog/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating update client")?;
    let body = client
        .get(api_url)
        .send()
        .context("contacting GitHub")?
        .error_for_status()
        .context("checking the latest GitHub release")?
        .bytes()
        .context("reading the latest GitHub release")?;
    parse_latest_release(&body, current_version)
}

fn parse_latest_release(body: &[u8], current_version: &str) -> Result<Option<AvailableUpdate>> {
    let release: LatestRelease =
        serde_json::from_slice(body).context("reading the latest GitHub release")?;
    available_update(current_version, &release.tag_name)
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
    let executable = env::current_exe().context("locating the current Svarog executable")?;
    download_and_run_installer(&installer_url, &executable)?;
    restart(&executable, args)
}

fn download_and_run_installer(installer_url: &str, executable: &Path) -> Result<()> {
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
    run_installer_bytes(&installer, executable)
}

fn run_installer_bytes(installer: &[u8], executable: &Path) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new().context("creating an installer file")?;
    file.write_all(installer)
        .context("writing the Svarog installer")?;

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
    Ok(())
}

fn rebuild_development() -> Result<()> {
    let launcher = env::var_os("SVAROG_DEV_LAUNCHER")
        .map(PathBuf::from)
        .context("this development build was not started through scripts/svarog")?;
    build_development_checkout(&launcher)?;
    let mut command = Command::new(&launcher);
    command.args(original_args());
    replace_command(command)
}

fn build_development_checkout(launcher: &Path) -> Result<()> {
    let status = Command::new(launcher)
        .arg("--build-only")
        .status()
        .context("rebuilding the development checkout")?;
    if !status.success() {
        bail!("development build exited with {status}");
    }
    Ok(())
}

fn original_args() -> Vec<OsString> {
    env::args_os().skip(1).collect()
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
    let state: UpdateState =
        toml::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;
    if state.version != UPDATE_STATE_VERSION {
        return Ok(UpdateState::default());
    }
    Ok(state)
}

fn load_state_recovering(paths: &Paths) -> UpdateState {
    load_state(paths).unwrap_or_default()
}

fn save_state(paths: &Paths, state: &UpdateState) -> Result<()> {
    paths.ensure()?;
    let path = state_file(paths);
    let contents = toml::to_string(state).context("serializing update state")?;
    let mut temp = tempfile::NamedTempFile::new_in(&paths.config_dir)
        .with_context(|| format!("creating temporary file in {}", paths.config_dir.display()))?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .context("securing temporary update state")?;
    temp.write_all(contents.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    temp.as_file()
        .sync_all()
        .with_context(|| format!("syncing {}", path.display()))?;
    temp.persist(&path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn restart(executable: &Path, args: &[OsString]) -> Result<()> {
    replace_command(restart_command(executable, args))
}

fn restart_command(executable: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(executable);
    command.args(args);
    command
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
    fn github_response_is_parsed_and_compared_semantically() {
        let update = parse_latest_release(br#"{"tag_name":"v0.6.3"}"#, "0.6.2")
            .unwrap()
            .unwrap();

        assert_eq!(update.version, Version::new(0, 6, 3));
        assert_eq!(update.tag, "v0.6.3");
    }

    #[test]
    fn downloaded_installer_receives_current_executable_directory() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("bin with spaces/svarog");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "old binary").unwrap();
        let log = root.path().join("installer.log");
        let installer = format!(
            "#!/usr/bin/env bash\nprintf '%s' \"$SVAROG_INSTALL_DIR\" > '{}'\n",
            log.display()
        );

        run_installer_bytes(installer.as_bytes(), &executable).unwrap();

        assert_eq!(
            fs::read_to_string(log).unwrap(),
            executable.parent().unwrap().display().to_string()
        );
    }

    #[test]
    fn installer_failure_is_returned_without_restarting() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("bin/svarog");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        let error =
            run_installer_bytes(b"#!/usr/bin/env bash\nexit 42\n", &executable).unwrap_err();

        assert!(error.to_string().contains("exit status: 42"));
    }

    #[test]
    fn failed_development_build_returns_without_replacing_the_process() {
        let root = tempfile::tempdir().unwrap();
        let launcher = root.path().join("svarog-launcher");
        fs::write(&launcher, "#!/usr/bin/env bash\nexit 42\n").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();

        let error = build_development_checkout(&launcher).unwrap_err();

        assert!(error.to_string().contains("exit status: 42"));
    }

    #[test]
    fn restart_command_preserves_arguments_without_disabling_future_daily_checks() {
        let args = vec![OsString::from("run"), OsString::from("two words")];
        let command = restart_command(Path::new("/tmp/svarog"), &args);

        assert_eq!(command.get_program(), "/tmp/svarog");
        assert_eq!(command.get_args().collect::<Vec<_>>(), args);
        assert!(!command.get_envs().any(|(key, _)| key == SKIP_ENV));
    }

    #[test]
    fn update_state_round_trips_outside_sqlite() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        let state = UpdateState {
            version: 2,
            last_attempt_local_date: Some("2026-08-22".into()),
            dismissed_version: Some("0.6.3".into()),
        };
        save_state(&paths, &state).unwrap();
        let loaded = load_state(&paths).unwrap();
        assert_eq!(
            loaded.last_attempt_local_date.as_deref(),
            Some("2026-08-22")
        );
        assert_eq!(loaded.dismissed_version.as_deref(), Some("0.6.3"));
        assert_eq!(
            fs::metadata(state_file(&paths))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(!paths.database_file.exists());
    }

    #[test]
    fn scheduled_checks_follow_local_calendar_dates_and_dismiss_exact_versions() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 22).unwrap();
        let state = UpdateState {
            version: 2,
            last_attempt_local_date: Some(date.to_string()),
            dismissed_version: Some("0.6.3".into()),
        };
        assert!(!scheduled_check_due(&state, date));
        assert!(scheduled_check_due(
            &state,
            NaiveDate::from_ymd_opt(2026, 8, 23).unwrap()
        ));
        assert!(version_was_dismissed(&state, &Version::new(0, 6, 3)));
        assert!(!version_was_dismissed(&state, &Version::new(0, 6, 4)));
    }

    #[test]
    fn persisted_dismissal_applies_only_to_the_declined_release() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));

        dismiss_version(&paths, &Version::new(0, 6, 3)).unwrap();

        assert!(is_version_dismissed(&paths, &Version::new(0, 6, 3)));
        assert!(!is_version_dismissed(&paths, &Version::new(0, 6, 4)));
    }

    #[test]
    fn legacy_and_corrupt_state_recover_as_due() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::from_root(root.path().join("svarog"));
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            state_file(&paths),
            "version = 1\nlast_checked_unix = 123\nlast_prompted_version = \"0.6.3\"\n",
        )
        .unwrap();
        let legacy = load_state(&paths).unwrap();
        assert_eq!(legacy.version, 2);
        assert!(legacy.last_attempt_local_date.is_none());

        fs::write(state_file(&paths), "not valid toml = [").unwrap();
        let recovered = load_state_recovering(&paths);
        assert_eq!(recovered.version, 2);
        assert!(scheduled_check_due(
            &recovered,
            NaiveDate::from_ymd_opt(2026, 8, 22).unwrap()
        ));
    }
}
