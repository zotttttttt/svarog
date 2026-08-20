use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);
const DELIVERY_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Eq, PartialEq)]
struct NotificationCommand {
    program: &'static str,
    args: Vec<String>,
}

pub fn notify(enabled: bool, title: &str, message: &str) -> bool {
    notify_with(enabled, title, message, deliver)
}

fn notify_with(
    enabled: bool,
    title: &str,
    message: &str,
    deliver: impl FnOnce(NotificationCommand) -> bool,
) -> bool {
    if !enabled {
        return false;
    }

    let Some(command) = notification_command(title, message) else {
        return false;
    };
    deliver(command)
}

fn deliver(command: NotificationCommand) -> bool {
    let (child_tx, child_rx) = mpsc::sync_channel::<Child>(1);
    if std::thread::Builder::new()
        .name("svarog-notification-reaper".to_string())
        .spawn(move || {
            if let Ok(mut child) = child_rx.recv() {
                let _ = wait_for_child(&mut child, DELIVERY_TIMEOUT);
            }
        })
        .is_err()
    {
        return false;
    }

    let child = Command::new(command.program)
        .args(command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    child.is_ok_and(|child| child_tx.send(child).is_ok())
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(remaining.min(DELIVERY_POLL_INTERVAL));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn notification_command(title: &str, message: &str) -> Option<NotificationCommand> {
    #[cfg(target_os = "macos")]
    {
        let script = format!("display notification {:?} with title {:?}", message, title);
        Some(NotificationCommand {
            program: "osascript",
            args: vec!["-e".to_string(), script],
        })
    }

    #[cfg(target_os = "linux")]
    {
        Some(NotificationCommand {
            program: "notify-send",
            args: vec![
                "--app-name".to_string(),
                "Svarog".to_string(),
                "--".to_string(),
                title.to_string(),
                message.to_string(),
            ],
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, message);
        None
    }
}

pub fn unavailable_reason() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        None
    }

    #[cfg(target_os = "linux")]
    {
        (!executable_on_path("notify-send", std::env::var_os("PATH").as_deref()))
            .then_some("notify-send not found on PATH")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Some("desktop notifications are unavailable on this platform")
    }
}

#[cfg(target_os = "linux")]
fn executable_on_path(command: &str, paths: Option<&std::ffi::OsStr>) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Some(paths) = paths else {
        return false;
    };
    std::env::split_paths(paths).any(|path| {
        std::fs::metadata(path.join(command))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_notifications_do_not_attempt_delivery() {
        assert!(!notify_with(
            false,
            "Svarog",
            "8 scapular squeezes",
            |_| panic!("disabled notifications must not attempt delivery")
        ));
    }

    #[test]
    fn enabled_notifications_forward_the_platform_command() {
        let expected = notification_command("Svarog", "8 scapular squeezes").unwrap();
        assert!(notify_with(
            true,
            "Svarog",
            "8 scapular squeezes",
            |command| command == expected
        ));
        assert!(!notify_with(true, "Svarog", "8 scapular squeezes", |_| {
            false
        }));
    }

    #[test]
    fn missing_delivery_helper_is_rejected() {
        assert!(!deliver(NotificationCommand {
            program: "svarog-test-missing-notification-helper",
            args: Vec::new(),
        }));
    }

    #[test]
    fn child_waiter_reports_success_and_enforces_its_deadline() {
        let mut quick = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        assert!(wait_for_child(&mut quick, Duration::from_secs(1)));

        let mut slow = Command::new("sh")
            .args(["-c", "exec sleep 10"])
            .spawn()
            .unwrap();
        let started = Instant::now();
        assert!(!wait_for_child(&mut slow, Duration::from_millis(20)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notifications_use_osascript() {
        assert_eq!(
            notification_command("Svarog", "8 scapular squeezes"),
            Some(NotificationCommand {
                program: "osascript",
                args: vec![
                    "-e".to_string(),
                    "display notification \"8 scapular squeezes\" with title \"Svarog\""
                        .to_string(),
                ],
            })
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_notifications_use_notify_send() {
        assert_eq!(
            notification_command("Svarog", "8 scapular squeezes"),
            Some(NotificationCommand {
                program: "notify-send",
                args: vec![
                    "--app-name".to_string(),
                    "Svarog".to_string(),
                    "--".to_string(),
                    "Svarog".to_string(),
                    "8 scapular squeezes".to_string(),
                ],
            })
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_capability_requires_an_executable_notify_send() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("notify-send");
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!executable_on_path(
            "notify-send",
            Some(root.path().as_os_str())
        ));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(executable_on_path(
            "notify-send",
            Some(root.path().as_os_str())
        ));
        assert!(!executable_on_path("notify-send", None));
    }
}
