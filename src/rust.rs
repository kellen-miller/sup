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
    let installed = match Command::new("rustup")
        .args(["toolchain", "list"])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
        Ok(output) => {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            return output.status.code().unwrap_or(1);
        }
        Err(error) => {
            eprintln!("sup-rust-update: cannot list toolchains: {error}");
            return 1;
        }
    };
    let default = match Command::new("rustup").arg("default").stdin(Stdio::null()).output() {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
        Ok(output) => {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            return output.status.code().unwrap_or(1);
        }
        Err(error) => {
            eprintln!("sup-rust-update: cannot inspect default toolchain: {error}");
            return 1;
        }
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
    for name in installed_names {
        if !numbered(name) {
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

fn numbered(name: &str) -> bool {
    let release = name.split_once('-').map_or(name, |(release, _)| release);
    let mut pieces = release.split('.');
    let mut count = 0;
    pieces.all(|part| {
        count += 1;
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
    }) && count >= 2
}
