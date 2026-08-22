use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct LauncherFixture {
    _root: tempfile::TempDir,
    fake_bin: PathBuf,
    target_dir: PathBuf,
    dev_root: PathBuf,
    result: PathBuf,
    build_log: PathBuf,
    new_binary: PathBuf,
}

impl LauncherFixture {
    fn new(with_existing_binary: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let fake_bin = root.path().join("bin");
        let target_dir = root.path().join("target");
        let dev_root = root.path().join("dev-root");
        let result = root.path().join("result.txt");
        let build_log = root.path().join("build.log");
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
if [ "${1:-}" = "build" ]; then
  printf 'build\n' >> "$SVAROG_TEST_BUILD_LOG"
  if [ "${SVAROG_TEST_BUILD_FAIL:-0}" = "1" ]; then
    exit 42
  fi
  mkdir -p "$CARGO_TARGET_DIR/release"
  cp "$SVAROG_TEST_NEW_BINARY" "$CARGO_TARGET_DIR/release/svarog"
  chmod 755 "$CARGO_TARGET_DIR/release/svarog"
  exit 0
fi
exit 2
"#,
        );
        write_executable(
            &new_binary,
            r#"#!/bin/sh
printf 'new\n' > "$SVAROG_TEST_RESULT"
printf 'mode=<%s>\n' "${SVAROG_MODE:-}" >> "$SVAROG_TEST_RESULT"
printf 'home=<%s>\n' "${SVAROG_HOME:-}" >> "$SVAROG_TEST_RESULT"
for argument in "$@"; do
  printf '<%s>\n' "$argument" >> "$SVAROG_TEST_RESULT"
done
"#,
        );
        if with_existing_binary {
            fs::create_dir_all(target_dir.join("release")).unwrap();
            write_executable(
                &target_dir.join("release/svarog"),
                r#"#!/bin/sh
printf 'old\n' > "$SVAROG_TEST_RESULT"
printf 'mode=<%s>\n' "${SVAROG_MODE:-}" >> "$SVAROG_TEST_RESULT"
printf 'home=<%s>\n' "${SVAROG_HOME:-}" >> "$SVAROG_TEST_RESULT"
for argument in "$@"; do
  printf '<%s>\n' "$argument" >> "$SVAROG_TEST_RESULT"
done
"#,
            );
        }

        Self {
            _root: root,
            fake_bin,
            target_dir,
            dev_root,
            result,
            build_log,
            new_binary,
        }
    }

    fn run(&self, arguments: &[&str], install_fails: bool) -> Output {
        self.run_with_policy("always", arguments, install_fails)
    }

    fn run_with_policy(&self, policy: &str, arguments: &[&str], build_fails: bool) -> Output {
        let path = self.fake_bin.display().to_string();
        Command::new("bash")
            .arg(format!("{}/scripts/svarog", env!("CARGO_MANIFEST_DIR")))
            .args(arguments)
            .env("PATH", path)
            .env("HOME", self._root.path())
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .env("SVAROG_DEV_ROOT", &self.dev_root)
            .env("SVAROG_UPDATE", policy)
            .env("SVAROG_TEST_RESULT", &self.result)
            .env("SVAROG_TEST_BUILD_LOG", &self.build_log)
            .env("SVAROG_TEST_NEW_BINARY", &self.new_binary)
            .env(
                "SVAROG_TEST_BUILD_FAIL",
                if build_fails { "1" } else { "0" },
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
        format!(
            "new\nmode=<dev>\nhome=<{}/svarog>\n<demo>\n<two words>\n",
            fixture.dev_root.display()
        )
    );
    assert_eq!(fs::read_to_string(&fixture.build_log).unwrap(), "build\n");
    assert!(fixture.dev_root.join(".source-fingerprint").exists());
}

#[test]
fn launcher_skips_install_when_fingerprint_matches() {
    let fixture = LauncherFixture::new(true);
    assert!(fixture.run(&["--update", "status"], false).status.success());

    let output = fixture.run(&["demo"], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(fs::read_to_string(&fixture.build_log).unwrap(), "build\n");
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        format!(
            "new\nmode=<dev>\nhome=<{}/svarog>\n<demo>\n",
            fixture.dev_root.display()
        )
    );
}

#[test]
fn launcher_can_run_existing_binary_without_updating() {
    let fixture = LauncherFixture::new(true);

    let output = fixture.run_with_policy("never", &["status"], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        format!(
            "old\nmode=<dev>\nhome=<{}/svarog>\n<status>\n",
            fixture.dev_root.display()
        )
    );
    assert!(!fixture.build_log.exists());
    assert!(!fixture.dev_root.join(".source-fingerprint").exists());
}

#[test]
fn launcher_can_run_existing_binary_without_rust() {
    let fixture = LauncherFixture::new(true);
    fixture.remove_rust();

    let output = fixture.run_with_policy("never", &["status"], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        format!(
            "old\nmode=<dev>\nhome=<{}/svarog>\n<status>\n",
            fixture.dev_root.display()
        )
    );
    assert!(!fixture.build_log.exists());
}

#[test]
fn launcher_installs_when_binary_is_missing() {
    let fixture = LauncherFixture::new(false);

    let output = fixture.run(&[], false);

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&fixture.result).unwrap(),
        format!(
            "new\nmode=<dev>\nhome=<{}/svarog>\n",
            fixture.dev_root.display()
        )
    );
    assert_eq!(fs::read_to_string(&fixture.build_log).unwrap(), "build\n");
}

#[test]
fn launcher_requires_rust_only_when_a_build_is_needed() {
    let fixture = LauncherFixture::new(false);
    fixture.remove_rust();

    let output = fixture.run(&[], false);

    assert!(!output.status.success());
    assert!(!fixture.result.exists());
    assert!(!fixture.build_log.exists());
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
    assert!(!fixture.build_log.exists());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("Rust is required only when building"));
}

#[test]
fn failed_build_does_not_write_state_or_run_existing_binary() {
    let fixture = LauncherFixture::new(true);

    let output = fixture.run(&["--update", "demo"], true);

    assert_eq!(output.status.code(), Some(42));
    assert!(!fixture.dev_root.join(".source-fingerprint").exists());
    assert!(!fixture.result.exists());
}

#[test]
fn build_only_updates_the_checkout_without_launching_it() {
    let fixture = LauncherFixture::new(true);

    let output = fixture.run(&["--build-only"], false);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&fixture.build_log).unwrap(), "build\n");
    assert!(fixture.dev_root.join(".source-fingerprint").exists());
    assert!(!fixture.result.exists());
}

#[test]
fn build_only_rejects_application_arguments() {
    let fixture = LauncherFixture::new(true);

    let output = fixture.run(&["--build-only", "status"], false);

    assert_eq!(output.status.code(), Some(2));
    assert!(!fixture.build_log.exists());
    assert!(!fixture.result.exists());
}
