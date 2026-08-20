use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn run_bootstrap(path: &Path) -> Output {
    Command::new("/bin/bash")
        .arg(format!("{}/scripts/bootstrap", env!("CARGO_MANIFEST_DIR")))
        .env("PATH", path)
        .output()
        .unwrap()
}

#[test]
fn bootstrap_reports_an_existing_toolchain_without_mutating_it() {
    let root = tempfile::tempdir().unwrap();
    write_executable(&root.path().join("rustc"), "#!/bin/sh\necho 'rustc test'\n");
    write_executable(&root.path().join("cargo"), "#!/bin/sh\necho 'cargo test'\n");

    let output = run_bootstrap(root.path());

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Svarog prerequisites are installed.\nrustc test\ncargo test\n"
    );
}

#[test]
fn bootstrap_missing_toolchain_is_guidance_only() {
    let root = tempfile::tempdir().unwrap();
    let invocation_log = root.path().join("invocations");
    for command in ["curl", "rustup"] {
        write_executable(
            &root.path().join(command),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{command}' >> '{}'\n",
                invocation_log.display()
            ),
        );
    }

    let output = run_bootstrap(root.path());

    assert!(!output.status.success());
    assert!(!invocation_log.exists());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("verified prebuilt release"));
    assert!(stdout.contains("rustup is installed"));
    assert!(!stdout.contains("Install missing prerequisites"));
}

#[test]
fn bootstrap_source_contains_no_network_execution_path() {
    let source =
        fs::read_to_string(format!("{}/scripts/bootstrap", env!("CARGO_MANIFEST_DIR"))).unwrap();

    assert!(!source.contains("curl"));
    assert!(!source.contains("rustup update"));
    assert!(!source.contains("read -r"));
}
