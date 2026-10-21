use std::process::Command;

#[test]
fn plan_and_list_do_not_execute_synthetic_commands() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let marker = dir.path().join("executed");
    let config = dir.path().join("synthetic.toml");
    std::fs::write(
        &config,
        format!(
            "version = 1\n[[jobs]]\nname = 'safe'\nphase = 'core'\ncommand = ['/bin/sh', '-c', 'touch {}']\n",
            marker.display()
        ),
    )
    .unwrap();

    for command in ["plan", "list"] {
        let result = Command::new(env!("CARGO_BIN_EXE_sup"))
            .arg("--config")
            .arg(&config)
            .arg(command)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &xdg)
            .env("XDG_CACHE_HOME", dir.path().join("cache"))
            .env("PATH", &bin)
            .current_dir(&home)
            .output()
            .unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
    assert!(!marker.exists());
}

#[test]
fn migration_exclusively_creates_output() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let input = dir.path().join("legacy.yaml");
    let output = dir.path().join("new.toml");
    std::fs::write(
        &input,
        "jobs:\n  - name: one\n    phase: core\n    command: [echo, hi]\n",
    )
    .unwrap();
    let first = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args([
            "config",
            "migrate",
            "--from",
            input.to_str().unwrap(),
            "--to",
            output.to_str().unwrap(),
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(first.status.success());
    let second = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args([
            "config",
            "migrate",
            "--from",
            input.to_str().unwrap(),
            "--to",
            output.to_str().unwrap(),
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(std::fs::read_to_string(input).unwrap().contains("name: one"));
}

#[test]
fn explicit_false_preflight_migration_plans_through_cli() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&xdg).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let input = dir.path().join("legacy.yaml");
    let output = dir.path().join("migrated.toml");
    let original = "jobs:\n  - name: harmless\n    phase: core\n    command: [echo, hi]\n    sudo_preflight: false\n";
    std::fs::write(&input, original).unwrap();
    let migrated = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args(["config", "migrate", "--from"])
        .arg(&input)
        .arg("--to")
        .arg(&output)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    assert_eq!(std::fs::read_to_string(&input).unwrap(), original);
    let planned = Command::new(env!("CARGO_BIN_EXE_sup"))
        .arg("--config")
        .arg(&output)
        .arg("plan")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(planned.status.success(), "{}", String::from_utf8_lossy(&planned.stderr));
    assert!(String::from_utf8_lossy(&planned.stdout).contains("harmless"));
}

#[test]
fn init_and_migration_leave_existing_and_symlink_destinations_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&xdg).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let existing = dir.path().join("existing.toml");
    std::fs::write(&existing, "keep me").unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args(["config", "init", "--output"])
        .arg(&existing)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(!init.status.success());
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep me");

    let input = dir.path().join("legacy.yaml");
    let target = dir.path().join("target.toml");
    let link = dir.path().join("link.toml");
    std::fs::write(
        &input,
        "jobs:\n  - name: one\n    phase: core\n    command: [echo, hi]\n",
    )
    .unwrap();
    std::fs::write(&target, "keep target").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &link).unwrap();
    let migrate = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args(["config", "migrate", "--from"])
        .arg(&input)
        .arg("--to")
        .arg(&link)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(!migrate.status.success());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep target");
}

#[test]
fn concurrent_migrations_publish_one_complete_no_clobber_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&xdg).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let input = dir.path().join("legacy.yaml");
    let output = dir.path().join("migrated.toml");
    let original = "jobs:\n  - name: one\n    phase: core\n    command: [echo, hi]\n";
    std::fs::write(&input, original).unwrap();
    let first = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args(["config", "migrate", "--from"])
        .arg(&input)
        .arg("--to")
        .arg(&output)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args(["config", "migrate", "--from"])
        .arg(&input)
        .arg("--to")
        .arg(&output)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", &bin)
        .current_dir(&home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert_ne!(first.status.success(), second.status.success());
    let published = std::fs::read_to_string(&output).unwrap();
    let parsed: toml::Value = toml::from_str(&published).unwrap();
    assert_eq!(parsed["jobs"][0]["name"].as_str(), Some("one"));
    assert_eq!(std::fs::read_to_string(&input).unwrap(), original);
}

#[test]
fn run_uses_only_the_explicit_synthetic_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let config = dir.path().join("fixture.toml");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        &config,
        "version = 1\n[[jobs]]\nname = 'safe'\nphase = 'core'\ncommand = ['/bin/sh', '-c', 'printf fixture-output']\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sup"))
        .arg("--config")
        .arg(&config)
        .arg("--no-tui")
        .arg("run")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("fixture-output"));
    assert!(home.join(".cache/sup/runs").exists());

    let empty_home = dir.path().join("empty-home");
    std::fs::create_dir(&empty_home).unwrap();
    let empty = Command::new(env!("CARGO_BIN_EXE_sup"))
        .arg("--config")
        .arg(&config)
        .arg("--skip")
        .arg("safe")
        .arg("run")
        .env("HOME", &empty_home)
        .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&empty_home)
        .output()
        .unwrap();
    assert!(empty.status.success());
    assert!(!empty_home.join(".cache/sup/runs").exists());
}

#[cfg(unix)]
#[test]
fn tty_stopped_child_fails_and_is_terminated() {
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let config = dir.path().join("fixture.toml");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        &config,
        "version = 1\n[[jobs]]\nname = 'tty'\nphase = 'core'\ncommand = ['/bin/cat', '/dev/tty']\n\n[[jobs]]\nname = 'sleeper'\nphase = 'core'\ncommand = ['/bin/sleep', '30']\n",
    )
    .unwrap();
    let mut master = -1;
    let mut slave = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    assert_ne!(unsafe { libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK) }, -1);
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let stdin = slave.try_clone().unwrap();
    let stdout = slave.try_clone().unwrap();
    let stderr = slave;
    let mut command = Command::new(env!("CARGO_BIN_EXE_sup"));
    command
        .arg("--config")
        .arg(&config)
        .arg("--no-tui")
        .arg("run")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&home)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1
                || libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) == -1
                || libc::tcsetpgrp(0, libc::getpgrp()) == -1
            {
                return Err(std::io::Error::last_os_error());
            }
            libc::signal(libc::SIGTTIN, libc::SIG_DFL);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let started = Instant::now();
    let mut output = Vec::new();
    let mut interrupted = false;
    loop {
        let mut chunk = [0; 2048];
        match (&master).read(&mut chunk) {
            Ok(count) if count > 0 => output.extend_from_slice(&chunk[..count]),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => {}
        }
        if !interrupted && String::from_utf8_lossy(&output).contains("Running sleeper") {
            assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
            interrupted = true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(130));
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stopped tty child hung: {}",
            String::from_utf8_lossy(&output)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = String::from_utf8_lossy(&output);
    assert!(interrupted, "second job never started: {output}");
    assert!(output.contains("needs terminal input"), "{output}");
}

#[test]
fn plain_output_keeps_crlf_text_and_strips_hyperlink_sequences() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    std::fs::write(
        &config,
        r#"version = 1
[[jobs]]
name = 'output'
phase = 'core'
command = ['/bin/sh', '-c', '''printf '\033]8;;https://example.test\033\\link\033]8;;\033\\\r\nnext\r\n' ''']
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sup"))
        .args(["--no-tui", "--config"])
        .arg(config)
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .current_dir(home.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("link"), "{text}");
    assert!(text.contains("next"), "{text}");
    assert!(!text.contains("example.test"), "{text}");
    assert!(!text.contains('\x1b'), "{text}");
}
