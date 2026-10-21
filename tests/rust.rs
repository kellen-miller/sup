use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn fake_rustup(directory: &std::path::Path, body: &str) {
    let path = directory.join("rustup");
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RUSTUP_CALLS\"\n{body}\n"),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn failed_update_never_uninstalls_toolchains() {
    let directory = tempfile::tempdir().unwrap();
    fake_rustup(directory.path(), "case \"$1\" in update) exit 8;; esac");
    let calls = directory.path().join("calls");
    let output = Command::new(env!("CARGO_BIN_EXE_sup-rust-update"))
        .env("PATH", directory.path())
        .env("RUSTUP_CALLS", &calls)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(8));
    let calls = std::fs::read_to_string(calls).unwrap();
    assert_eq!(calls.trim(), "update stable nightly");
}

#[test]
fn successful_update_preserves_custom_channels_and_removes_numbered_releases() {
    let directory = tempfile::tempdir().unwrap();
    fake_rustup(
        directory.path(),
        "case \"$1 $2\" in 'toolchain list') printf 'stable-x86_64-unknown-linux-gnu (default)\\nnightly-x86_64-unknown-linux-gnu\\n1.80.0-x86_64-unknown-linux-gnu\\ncustom-local\\n' ;; 'default ') printf 'stable-x86_64-unknown-linux-gnu\\n' ;; 'run stable') exit 0 ;; 'run nightly') exit 0 ;; esac",
    );
    let calls = directory.path().join("calls");
    let output = Command::new(env!("CARGO_BIN_EXE_sup-rust-update"))
        .env("PATH", directory.path())
        .env("RUSTUP_CALLS", &calls)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = std::fs::read_to_string(calls).unwrap();
    assert!(calls.contains("toolchain uninstall 1.80.0-x86_64-unknown-linux-gnu"));
    assert!(!calls.contains("toolchain uninstall custom-local"));
}

#[test]
fn failed_compiler_validation_never_uninstalls_toolchains() {
    let directory = tempfile::tempdir().unwrap();
    fake_rustup(
        directory.path(),
        "case \"$1 $2\" in 'run nightly') exit 9;; esac",
    );
    let calls = directory.path().join("calls");
    let output = Command::new(env!("CARGO_BIN_EXE_sup-rust-update"))
        .env("PATH", directory.path())
        .env("RUSTUP_CALLS", &calls)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
    let calls = std::fs::read_to_string(calls).unwrap();
    assert!(!calls.contains("toolchain uninstall"));
}

#[test]
fn numbered_default_switches_before_any_uninstall() {
    let directory = tempfile::tempdir().unwrap();
    let switched = directory.path().join("switched");
    fake_rustup(
        directory.path(),
        &format!(
            "case \"$1 $2\" in 'toolchain list') printf '1.80.0-x86_64-unknown-linux-gnu\\ncustom-local\\n' ;; 'default ') if [ -e '{}' ]; then printf 'stable-x86_64-unknown-linux-gnu\\n'; else printf '1.80.0-x86_64-unknown-linux-gnu\\n'; fi ;; esac\ncase \"$1 $2\" in 'default stable') : > '{}' ;; esac",
            switched.display(),
            switched.display()
        ),
    );
    let calls = directory.path().join("calls");
    let output = Command::new(env!("CARGO_BIN_EXE_sup-rust-update"))
        .env("PATH", directory.path())
        .env("RUSTUP_CALLS", &calls)
        .output()
        .unwrap();
    assert!(output.status.success());
    let calls = std::fs::read_to_string(calls).unwrap();
    let switch = calls.find("default stable").unwrap();
    let uninstall = calls.find("toolchain uninstall 1.80.0-").unwrap();
    assert!(switch < uninstall);
    assert!(!calls.contains("toolchain uninstall custom-local"));
}
