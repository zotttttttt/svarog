use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct LauncherFixture {
    _root: tempfile::TempDir,
    fake_bin: PathBuf,
    cargo_home: PathBuf,
    result: PathBuf,
    install_log: PathBuf,
    new_binary: PathBuf,
}

impl LauncherFixture {
    fn new(with_existing_binary: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let fake_bin = root.path().join("bin");
        let cargo_home = root.path().join("cargo-home");
        let result = root.path().join("result.txt");
        let install_log = root.path().join("install.log");
        let new_binary = root.path().join("new-svarog");
        fs::create_dir_all(&fake_bin).unwrap();

        for command in [
            "awk", "bash", "chmod", "cksum", "cp", "dirname", "find", "mkdir", "mv", "sed", "sort",
        ] {
            link_command(&fake_bin, command);
        }
        write_executable(&fake_bin.join("rustc"), "#!/bin/sh\nexit 0\n");
        write_executable(
            &fake_bin.join("cargo"),
            r#"#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  echo "cargo test"
  exit 0
fi
if [ "${1:-}" = "install" ]; then
  printf 'install\n' >> "$SVAROG_TEST_INSTALL_LOG"
  if [ "${SVAROG_TEST_INSTALL_FAIL:-0}" = "1" ]; then
    exit 42
  fi
  mkdir -p "$CARGO_HOME/bin"
  cp "$SVAROG_TEST_NEW_BINARY" "$CARGO_HOME/bin/svarog"
  chmod 755 "$CARGO_HOME/bin/svarog"
  exit 0
fi
exit 2
"#,
        );
        write_executable(
            &new_binary,
            r#"#!/bin/sh
printf 'new\n' > "$SVAROG_TEST_RESULT"
for argument in "$@"; do
  printf '<%s>\n' "$argument" >> "$SVAROG_TEST_RESULT"
done
"#,
        );
        if with_existing_binary {
            write_executable(
                &fake_bin.join("svarog"),
                r#"#!/bin/sh
printf 'old\n' > "$SVAROG_TEST_RESULT"
for argument in "$@"; do
  printf '<%s>\n' "$argument" >> "$SVAROG_TEST_RESULT"
done
"#,
            );
        }

        Self {
            _root: root,
            fake_bin,
            cargo_home,
            result,
            install_log,
            new_binary,
        }
    }

    fn run(&self, arguments: &[&str], install_fails: bool) -> Output {
        let path = self.fake_bin.display().to_string();
        Command::new("bash")
            .arg(format!("{}/scripts/svarog", env!("CARGO_MANIFEST_DIR")))
            .args(arguments)
            .env("PATH", path)
            .env("HOME", self._root.path())
            .env("CARGO_HOME", &self.cargo_home)
            .env("SVAROG_TEST_RESULT", &self.result)
            .env("SVAROG_TEST_INSTALL_LOG", &self.install_log)
            .env("SVAROG_TEST_NEW_BINARY", &self.new_binary)
            .env(
                "SVAROG_TEST_INSTALL_FAIL",
                if install_fails { "1" } else { "0" },
            )
            .output()
            .unwrap()
    }

    fn remove_rust(&self) {
        fs::remove_file(self.fake_bin.join("rustc")).unwrap();
        fs::remove_file(self.fake_bin.join("cargo")).unwrap();
    }
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn link_command(directory: &Path, command: &str) {
    let output = Command::new("sh")
        .args(["-c", &format!("command -v {command}")])
        .output()
        .unwrap();
    assert!(output.status.success(), "missing test command: {command}");
    let target = String::from_utf8(output.stdout).unwrap();
    symlink(target.trim(), directory.join(command)).unwrap();
}

#[test]
fn launcher_updates_changed_checkout_and_forwards_exact_arguments() {
    let fixture = LauncherFixture::new(true);

    let output = fixture.run(&["--update", "demo", "two words"], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        "new\n<demo>\n<two words>\n"
    );
    assert_eq!(
        fs::read_to_string(&fixture.install_log).unwrap(),
        "install\n"
    );
    assert!(fixture.cargo_home.join(".svarog-install-state").exists());
}

#[test]
fn launcher_skips_install_when_fingerprint_matches() {
    let fixture = LauncherFixture::new(true);
    assert!(fixture.run(&["--update", "status"], false).status.success());

    let output = fixture.run(&["demo"], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&fixture.install_log).unwrap(),
        "install\n"
    );
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        "new\n<demo>\n"
    );
}

#[test]
fn launcher_can_run_existing_binary_without_updating() {
    let fixture = LauncherFixture::new(true);

    let output = fixture.run(&["status"], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        "old\n<status>\n"
    );
    assert!(!fixture.install_log.exists());
    assert!(!fixture.cargo_home.join(".svarog-install-state").exists());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("scripts/svarog --update"));
}

#[test]
fn launcher_can_run_existing_binary_without_rust() {
    let fixture = LauncherFixture::new(true);
    fixture.remove_rust();

    let output = fixture.run(&["status"], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        "old\n<status>\n"
    );
    assert!(!fixture.install_log.exists());
}

#[test]
fn launcher_installs_when_binary_is_missing() {
    let fixture = LauncherFixture::new(false);

    let output = fixture.run(&[], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(fs::read_to_string(&fixture.result).unwrap(), "new\n");
    assert_eq!(
        fs::read_to_string(&fixture.install_log).unwrap(),
        "install\n"
    );
}

#[test]
fn launcher_requires_rust_only_when_a_build_is_needed() {
    let fixture = LauncherFixture::new(false);
    fixture.remove_rust();

    let output = fixture.run(&[], false);

    assert!(!output.status.success());
    assert!(!fixture.result.exists());
    assert!(!fixture.install_log.exists());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("verified prebuilt release"));
    assert!(stdout.contains("https://rustup.rs"));
}

#[test]
fn explicit_update_does_not_fall_back_when_rust_is_missing() {
    let fixture = LauncherFixture::new(true);
    fixture.remove_rust();

    let output = fixture.run(&["--update", "status"], false);

    assert!(!output.status.success());
    assert!(!fixture.result.exists());
    assert!(!fixture.install_log.exists());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("Rust is required only when building"));
}

#[test]
fn failed_install_does_not_write_state_or_run_existing_binary() {
    let fixture = LauncherFixture::new(true);

    let output = fixture.run(&["--update", "demo"], true);

    assert_eq!(output.status.code(), Some(42));
    assert!(!fixture.cargo_home.join(".svarog-install-state").exists());
    assert!(!fixture.result.exists());
}
