#[cfg(not(test))]
use std::process::{Command, Stdio};

#[derive(Debug, Eq, PartialEq)]
struct NotificationCommand {
    program: &'static str,
    args: Vec<String>,
}

pub fn notify(enabled: bool, title: &str, message: &str) -> bool {
    if !enabled {
        return false;
    }

    #[cfg(not(test))]
    {
        let Some(command) = notification_command(title, message) else {
            return false;
        };
        Command::new(command.program)
            .args(command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(test)]
    {
        let _ = (title, message);
        false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_notifications_do_not_attempt_delivery() {
        assert!(!notify(false, "Svarog", "8 scapular squeezes"));
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
}
