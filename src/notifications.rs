#[cfg(all(target_os = "macos", not(test)))]
use std::process::{Command, Stdio};

pub fn notify(enabled: bool, title: &str, message: &str) -> bool {
    if !enabled {
        return false;
    }

    #[cfg(all(target_os = "macos", not(test)))]
    {
        let script = format!("display notification {:?} with title {:?}", message, title);
        Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(any(not(target_os = "macos"), test))]
    {
        let _ = (title, message);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_notifications_do_not_attempt_delivery() {
        assert!(!notify(false, "Svarog", "8 scapular squeezes"));
    }
}
