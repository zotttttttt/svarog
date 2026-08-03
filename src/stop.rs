use crate::config::{Paths, RuntimeEnv};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

const SHUTDOWN_WAIT_ATTEMPTS: usize = 40;
const SHUTDOWN_WAIT_INTERVAL: Duration = Duration::from_millis(50);

pub fn run(current_env: &RuntimeEnv) -> Result<()> {
    let mut lock_files = BTreeSet::new();
    lock_files.insert(current_env.paths.data_dir.join("tui.lock"));
    lock_files.insert(Paths::load()?.data_dir.join("tui.lock"));
    lock_files.insert(RuntimeEnv::load_demo()?.paths.data_dir.join("tui.lock"));

    let mut stopped_runtimes = 0;
    let mut warnings = Vec::new();
    for lock_file in lock_files {
        match stop_runtime(&lock_file) {
            Ok(true) => stopped_runtimes += 1,
            Ok(false) => {}
            Err(error) => warnings.push(error.to_string()),
        }
    }

    let stopped_tmux_sessions = match stop_tmux_sessions() {
        Ok(count) => count,
        Err(error) => {
            warnings.push(error.to_string());
            0
        }
    };

    for warning in warnings {
        eprintln!("Warning: {warning}");
    }
    if stopped_runtimes == 0 && stopped_tmux_sessions == 0 {
        println!("No running Svarog sessions found.");
    } else {
        println!(
            "Stopped {stopped_runtimes} Svarog runtime(s) and {stopped_tmux_sessions} tmux session(s)."
        );
    }
    Ok(())
}

fn stop_runtime(lock_file: &Path) -> Result<bool> {
    stop_runtime_with_signal(lock_file, signal_runtime)
}

fn stop_runtime_with_signal<F>(lock_file: &Path, signal: F) -> Result<bool>
where
    F: FnOnce(i32) -> Result<bool>,
{
    let Some(mut file) = open_existing_lock(lock_file)? else {
        return Ok(false);
    };
    if try_lock(&file)? {
        unlock(&file);
        return Ok(false);
    }

    let pid = read_pid(&mut file)
        .with_context(|| format!("reading active runtime from {}", lock_file.display()))?;
    if pid == std::process::id() as i32 {
        return Ok(false);
    }
    if !signal(pid)? {
        return Ok(false);
    }

    for _ in 0..SHUTDOWN_WAIT_ATTEMPTS {
        thread::sleep(SHUTDOWN_WAIT_INTERVAL);
        if try_lock(&file)? {
            unlock(&file);
            return Ok(true);
        }
    }
    anyhow::bail!("Svarog runtime PID {pid} did not stop within 2 seconds")
}

fn signal_runtime(pid: i32) -> Result<bool> {
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        return Err(error).with_context(|| format!("stopping Svarog runtime PID {pid}"));
    }
    Ok(true)
}

fn open_existing_lock(path: &Path) -> Result<Option<File>> {
    if !path.exists() {
        return Ok(None);
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map(Some)
        .with_context(|| format!("opening {}", path.display()))
}

fn read_pid(file: &mut File) -> Result<i32> {
    let mut value = String::new();
    file.read_to_string(&mut value)?;
    let pid = value.trim().parse::<i32>().context("invalid runtime PID")?;
    if pid <= 1 {
        anyhow::bail!("invalid runtime PID");
    }
    Ok(pid)
}

fn try_lock(file: &File) -> Result<bool> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    ) {
        Ok(false)
    } else {
        Err(error).context("checking Svarog runtime lock")
    }
}

fn unlock(file: &File) {
    let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

fn stop_tmux_sessions() -> Result<usize> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .context("listing tmux sessions")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no server running") || stderr.contains("failed to connect") {
            return Ok(0);
        }
        anyhow::bail!("could not list tmux sessions: {}", stderr.trim());
    }

    let names = svarog_tmux_sessions(&String::from_utf8_lossy(&output.stdout));
    let mut stopped = 0;
    for name in names {
        let status = Command::new("tmux")
            .args(["kill-session", "-t", &name])
            .status()
            .with_context(|| format!("stopping tmux session {name}"))?;
        if status.success() {
            stopped += 1;
        } else {
            anyhow::bail!("could not stop tmux session {name}");
        }
    }
    Ok(stopped)
}

fn svarog_tmux_sessions(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|name| name.starts_with("svarog-"))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn tmux_filter_selects_only_svarog_owned_sessions() {
        assert_eq!(
            svarog_tmux_sessions("work\nsvarog-codex\nsvarog-claude\nother\n"),
            vec!["svarog-codex", "svarog-claude"]
        );
    }

    #[test]
    fn missing_and_stale_runtime_locks_are_not_stopped() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing.lock");
        assert!(!stop_runtime(&missing).unwrap());

        let stale = root.path().join("stale.lock");
        std::fs::write(&stale, "999999\n").unwrap();
        assert!(!stop_runtime(&stale).unwrap());
    }

    #[test]
    fn malformed_unlocked_runtime_lock_is_ignored() {
        let root = tempdir().unwrap();
        let lock = root.path().join("tui.lock");
        std::fs::write(&lock, "not-a-pid\n").unwrap();

        assert!(!stop_runtime(&lock).unwrap());
    }

    #[test]
    fn live_runtime_lock_is_signaled_and_waited_for() {
        let root = tempdir().unwrap();
        let lock = root.path().join("tui.lock");
        let mut holder = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&lock)
            .unwrap();
        assert!(try_lock(&holder).unwrap());
        writeln!(holder, "4242").unwrap();

        let stopped = stop_runtime_with_signal(&lock, |pid| {
            assert_eq!(pid, 4242);
            unlock(&holder);
            Ok(true)
        })
        .unwrap();

        assert!(stopped);
    }

    #[test]
    fn duplicate_runtime_targets_are_deduplicated_by_path() {
        let path = std::path::PathBuf::from("/tmp/svarog/tui.lock");
        let targets = BTreeSet::from([path.clone(), path]);

        assert_eq!(targets.len(), 1);
    }
}
