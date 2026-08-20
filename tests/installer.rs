use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const VERSION: &str = "9.8.7";
const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct InstallerFixture {
    _root: tempfile::TempDir,
    fake_bin: PathBuf,
    release_dir: PathBuf,
    install_dir: PathBuf,
    download_log: PathBuf,
    installer: PathBuf,
    target: String,
}

impl InstallerFixture {
    fn new(os: &str, arch: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let fake_bin = root.path().join("fake bin");
        let release_dir = root.path().join("release files");
        let install_dir = root.path().join("installed bin");
        let download_log = root.path().join("downloads.log");
        let installer = root.path().join("svarog-installer.sh");
        fs::create_dir_all(&fake_bin).unwrap();
        fs::create_dir_all(&release_dir).unwrap();

        for command in [
            "awk", "gzip", "install", "mkdir", "mktemp", "mv", "rm", "tar",
        ] {
            link_command(&fake_bin, command);
        }
        write_executable(
            &fake_bin.join("uname"),
            r#"#!/bin/sh
case "${1:-}" in
  -s) printf '%s\n' "$SVAROG_TEST_OS" ;;
  -m) printf '%s\n' "$SVAROG_TEST_ARCH" ;;
  *) exit 2 ;;
esac
"#,
        );
        write_executable(
            &fake_bin.join("curl"),
            r#"#!/bin/sh
if [ "${SVAROG_TEST_DOWNLOAD_FAIL:-0}" = "1" ]; then
  exit 22
fi
output=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      shift
      output="$1"
      ;;
    http://*|https://*)
      url="$1"
      ;;
  esac
  shift
done
file="${url##*/}"
printf '%s\n' "$url" >> "$SVAROG_TEST_DOWNLOAD_LOG"
/bin/cp "$SVAROG_TEST_RELEASE_DIR/$file" "$output"
"#,
        );
        let checksum_script = r#"#!/bin/sh
last=""
for argument in "$@"; do
  last="$argument"
done
printf '%s  %s\n' "$SVAROG_TEST_ACTUAL_CHECKSUM" "$last"
"#;
        write_executable(&fake_bin.join("sha256sum"), checksum_script);
        write_executable(&fake_bin.join("shasum"), checksum_script);

        let target = target_for(os, arch).unwrap().to_owned();
        let generated =
            fs::read_to_string(format!("{}/scripts/install", env!("CARGO_MANIFEST_DIR")))
                .unwrap()
                .replace("@SVAROG_VERSION@", VERSION)
                .replace("@SVAROG_SHA256_AARCH64_APPLE_DARWIN@", CHECKSUM)
                .replace("@SVAROG_SHA256_X86_64_APPLE_DARWIN@", CHECKSUM)
                .replace("@SVAROG_SHA256_X86_64_UNKNOWN_LINUX_GNU@", CHECKSUM);
        write_executable(&installer, &generated);
        let fixture = Self {
            _root: root,
            fake_bin,
            release_dir,
            install_dir,
            download_log,
            installer,
            target,
        };
        fixture.write_release(VERSION);
        fixture
    }

    fn write_release(&self, binary_version: &str) {
        let package = format!("svarog-{VERSION}-{}", self.target);
        let staging = self._root.path().join("staging");
        let package_dir = staging.join(&package);
        fs::create_dir_all(&package_dir).unwrap();
        write_executable(
            &package_dir.join("svarog"),
            &format!("#!/bin/sh\nprintf '%s\\n' 'svarog {binary_version}'\n"),
        );
        let archive = format!("{package}.tar.gz");
        let status = Command::new("tar")
            .args(["-C", staging.to_str().unwrap(), "-czf"])
            .arg(self.release_dir.join(&archive))
            .arg(&package)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn run(&self, os: &str, arch: &str) -> Output {
        self.command(os, arch).output().unwrap()
    }

    fn command(&self, os: &str, arch: &str) -> Command {
        let mut command = Command::new("/bin/bash");
        command
            .arg(&self.installer)
            .env("PATH", &self.fake_bin)
            .env("HOME", self._root.path().join("home"))
            .env("SVAROG_INSTALL_DIR", &self.install_dir)
            .env("SVAROG_RELEASE_BASE_URL", "https://example.test/release")
            .env("SVAROG_TEST_OS", os)
            .env("SVAROG_TEST_ARCH", arch)
            .env("SVAROG_TEST_RELEASE_DIR", &self.release_dir)
            .env("SVAROG_TEST_DOWNLOAD_LOG", &self.download_log)
            .env("SVAROG_TEST_ACTUAL_CHECKSUM", CHECKSUM);
        command
    }
}

fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("Darwin", "arm64" | "aarch64") => Some("aarch64-apple-darwin"),
        ("Darwin", "x86_64") => Some("x86_64-apple-darwin"),
        ("Linux", "x86_64" | "amd64") => Some("x86_64-unknown-linux-gnu"),
        _ => None,
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
fn installer_maps_every_supported_platform_to_its_release_archive() {
    for (os, arch) in [
        ("Darwin", "arm64"),
        ("Darwin", "x86_64"),
        ("Linux", "x86_64"),
    ] {
        let fixture = InstallerFixture::new(os, arch);
        let output = fixture.run(os, arch);

        assert!(output.status.success(), "{output:?}");
        let installed = fixture.install_dir.join("svarog");
        assert_eq!(
            Command::new(installed)
                .arg("--version")
                .output()
                .unwrap()
                .stdout,
            format!("svarog {VERSION}\n").as_bytes()
        );
        let downloads = fs::read_to_string(&fixture.download_log).unwrap();
        assert!(downloads.contains(&format!("svarog-{VERSION}-{}.tar.gz", fixture.target)));
        assert!(!downloads.contains("SHA256SUMS"));
    }
}

#[test]
fn installer_replaces_an_existing_binary_in_a_path_with_spaces() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    fs::create_dir_all(&fixture.install_dir).unwrap();
    write_executable(
        &fixture.install_dir.join("svarog"),
        "#!/bin/sh\necho 'old binary'\n",
    );

    let output = fixture.run("Linux", "x86_64");

    assert!(output.status.success(), "{output:?}");
    let installed_output = Command::new(fixture.install_dir.join("svarog"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(installed_output.stdout, b"svarog 9.8.7\n");
}

#[test]
fn installer_prints_path_guidance_without_editing_shell_files() {
    let fixture = InstallerFixture::new("Linux", "x86_64");

    let output = fixture.run("Linux", "x86_64");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("is not currently on PATH"));
    assert!(stdout.contains("export PATH="));
    assert!(stdout.contains("Start now:"));
    assert!(!fixture._root.path().join("home/.profile").exists());
    assert!(!fixture._root.path().join("home/.zshrc").exists());
    assert!(!fixture._root.path().join("home/.bashrc").exists());
}

#[test]
fn installer_defaults_to_home_local_bin() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    let output = fixture
        .command("Linux", "x86_64")
        .env_remove("SVAROG_INSTALL_DIR")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(fixture
        ._root
        .path()
        .join("home/.local/bin/svarog")
        .is_file());
}

#[test]
fn installer_rejects_unsupported_platforms_before_downloading() {
    let fixture = InstallerFixture::new("Linux", "x86_64");

    let output = fixture.run("FreeBSD", "x86_64");

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("does not provide a release binary"));
    assert!(!fixture.download_log.exists());
}

#[test]
fn installer_rejects_relative_install_directories() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    let output = fixture
        .command("Linux", "x86_64")
        .env("SVAROG_INSTALL_DIR", "relative/bin")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("must be an absolute path"));
}

#[test]
fn installer_stops_on_a_checksum_mismatch_without_replacing_the_binary() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    fs::create_dir_all(&fixture.install_dir).unwrap();
    let installed = fixture.install_dir.join("svarog");
    write_executable(&installed, "#!/bin/sh\necho 'keep me'\n");

    let output = fixture
        .command("Linux", "x86_64")
        .env(
            "SVAROG_TEST_ACTUAL_CHECKSUM",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("Checksum verification failed"));
    assert_eq!(
        fs::read_to_string(installed).unwrap(),
        "#!/bin/sh\necho 'keep me'\n"
    );
}

#[test]
fn installer_stops_when_the_embedded_checksum_is_missing() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    let template = fs::read_to_string(format!("{}/scripts/install", env!("CARGO_MANIFEST_DIR")))
        .unwrap()
        .replace("@SVAROG_VERSION@", VERSION);
    write_executable(&fixture.installer, &template);

    let output = fixture.run("Linux", "x86_64");

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("does not contain a valid embedded checksum"));
    assert!(!fixture.install_dir.join("svarog").exists());
}

#[test]
fn installer_stops_when_the_verified_archive_is_malformed() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    let archive = format!("svarog-{VERSION}-{}.tar.gz", fixture.target);
    fs::write(fixture.release_dir.join(archive), "not a tar archive").unwrap();

    let output = fixture.run("Linux", "x86_64");

    assert!(!output.status.success());
    assert!(!fixture.install_dir.join("svarog").exists());
}

#[test]
fn installer_stops_when_the_archive_version_is_wrong() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    fixture.write_release("1.0.0");

    let output = fixture.run("Linux", "x86_64");

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("reported an unexpected version"));
    assert!(!fixture.install_dir.join("svarog").exists());
}

#[test]
fn installer_surfaces_download_failures() {
    let fixture = InstallerFixture::new("Linux", "x86_64");
    let output = fixture
        .command("Linux", "x86_64")
        .env("SVAROG_TEST_DOWNLOAD_FAIL", "1")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(22));
    assert!(!fixture.install_dir.join("svarog").exists());
}
