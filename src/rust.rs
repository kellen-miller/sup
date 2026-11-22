use std::process::{Command, Stdio};

pub fn update() -> i32 {
    let update = Command::new("rustup")
        .args(["update", "stable", "nightly"])
        .stdin(Stdio::null())
        .status();
    let code = match update {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("sup-rust-update: cannot run rustup: {error}");
            return 1;
        }
    };
    if code != 0 {
        return code;
    }
    let validation = validate_channels();
    if validation != 0 {
        return validation;
    }
    remove_obsolete_toolchains()
}

fn validate_channels() -> i32 {
    for channel in ["stable", "nightly"] {
        let status = Command::new("rustup")
            .args(["run", channel, "rustc", "--version"])
            .stdin(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => return status.code().unwrap_or(1),
            Err(error) => {
                eprintln!("sup-rust-update: cannot verify {channel}: {error}");
                return 1;
            }
        }
    }
    0
}

fn remove_obsolete_toolchains() -> i32 {
    let installed = match rustup_stdout(&["toolchain", "list"], "list toolchains") {
        Ok(stdout) => stdout,
        Err(code) => return code,
    };
    let default = match rustup_stdout(&["default"], "inspect default toolchain") {
        Ok(stdout) => stdout,
        Err(code) => return code,
    };
    let installed_names: Vec<&str> = installed
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let default_name = default.split_whitespace().next().unwrap_or_default();
    if installed_names.is_empty() || default_name.is_empty() {
        eprintln!("sup-rust-update: rustup returned an empty toolchain inventory");
        return 1;
    }
    if numbered(default_name) {
        let status = Command::new("rustup")
            .args(["default", "stable"])
            .stdin(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => return status.code().unwrap_or(1),
            Err(error) => {
                eprintln!("sup-rust-update: cannot switch default to stable: {error}");
                return 1;
            }
        }
    }
    // A rust-toolchain.toml in the working directory pins the active toolchain; other sup jobs
    // inherit that directory, so removing it forces concurrent re-downloads that race in rustup.
    let active = match rustup_stdout(&["show", "active-toolchain"], "inspect active toolchain") {
        Ok(stdout) => stdout,
        Err(code) => return code,
    };
    let active_name = active.split_whitespace().next().unwrap_or_default();
    for name in installed_names {
        if !numbered(name) || name == active_name {
            continue;
        }
        let status = Command::new("rustup")
            .args(["toolchain", "uninstall", name])
            .stdin(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => return status.code().unwrap_or(1),
            Err(error) => {
                eprintln!("sup-rust-update: cannot uninstall {name}: {error}");
                return 1;
            }
        }
    }
    0
}

/// Runs a read-only rustup query; on failure, reports rustup's stderr and returns its exit code.
fn rustup_stdout(args: &[&str], action: &str) -> Result<String, i32> {
    match Command::new("rustup").args(args).stdin(Stdio::null()).output() {
        Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Ok(output) => {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            Err(output.status.code().unwrap_or(1))
        }
        Err(error) => {
            eprintln!("sup-rust-update: cannot {action}: {error}");
            Err(1)
        }
    }
}

fn numbered(name: &str) -> bool {
    let release = name.split_once('-').map_or(name, |(release, _)| release);
    let mut pieces = release.split('.');
    let mut count = 0;
    pieces.all(|part| {
        count += 1;
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
    }) && count >= 2
}
