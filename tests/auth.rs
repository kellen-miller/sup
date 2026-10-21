use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SECRET: &[u8] = b"unique-fixture-secret42\n";

struct Fixture {
    directory: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let bin = directory.path().join("bin");
        let config = directory.path().join("fixture.toml");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&bin).unwrap();
        executable(&bin.join("sudo"), include_str!("fixtures/auth-sudo.sh"));
        executable(&bin.join("brew"), include_str!("fixtures/auth-brew.sh"));
        Self {
            directory,
            home,
            bin,
            config,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn jobs(&self, text: &str) {
        fs::write(&self.config, format!("version = 1\n{text}")).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sup"));
        command
            .arg("--config")
            .arg(&self.config)
            .arg("--no-tui")
            .arg("run")
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("PATH", &self.bin)
            .env("TERM", "xterm-256color")
            .env("FIXTURE_ROOT", self.root())
            .current_dir(&self.home);
        command
    }

    fn ui_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sup"));
        command
            .arg("--config")
            .arg(&self.config)
            .arg("run")
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("PATH", &self.bin)
            .env("FIXTURE_ROOT", self.root())
            .current_dir(&self.home);
        command
    }

    fn trace(&self) -> String {
        fs::read_to_string(self.root().join("trace")).unwrap_or_default()
    }

    fn assert_no_secret(&self, output: &[u8]) {
        let secret = &SECRET[..SECRET.len() - 1];
        assert!(
            !output.windows(secret.len()).any(|bytes| bytes == secret),
            "secret echoed"
        );
        let mut directories = vec![self.root().to_path_buf()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    directories.push(entry.path());
                } else {
                    let bytes = fs::read(entry.path()).unwrap();
                    assert!(
                        !bytes.windows(secret.len()).any(|bytes| bytes == secret),
                        "secret persisted in {:?}",
                        entry.path()
                    );
                }
            }
        }
    }

    fn assert_socket_removed(&self) {
        if let Ok(coordinates) = fs::read_to_string(self.root().join("coordinates")) {
            let socket = Path::new(coordinates.lines().next().unwrap());
            assert!(!socket.exists(), "socket leaked");
            assert!(
                !socket.parent().unwrap().exists(),
                "private directory leaked"
            );
        }
    }
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn job(name: &str, auth: &str, command: &str) -> String {
    format!(
        "[[jobs]]\nname = '{name}'\nphase = 'core'\nauth = '{auth}'\noptional = false\ncommand = {command}\n"
    )
}

struct PtyRun {
    child: Child,
    master: Option<File>,
    observer: Option<File>,
    original: TerminalSettings,
    output: Vec<u8>,
    root: PathBuf,
}

impl PtyRun {
    fn spawn(command: Command) -> Self {
        let root = PathBuf::from(
            command
                .get_envs()
                .find(|(key, _)| *key == "FIXTURE_ROOT")
                .unwrap()
                .1
                .unwrap(),
        );
        // The shell keeps the controlling terminal alive while recording restoration.
        let mut owner = Command::new("/bin/sh");
        for marker in ["terminal-ready", "terminal-release"] {
            let _ = fs::remove_file(root.join(marker));
        }

        owner.args(["-c", "trap '' INT HUP QUIT; \"$@\" < /dev/tty & pid=$!; echo $pid > \"$FIXTURE_ROOT/sup-pid\"; wait $pid; status=$?; : > \"$FIXTURE_ROOT/terminal-ready\"; while [ ! -f \"$FIXTURE_ROOT/terminal-release\" ]; do /bin/sleep 0.01; done; exit $status", "pty-owner"])
            .arg(command.get_program()).args(command.get_args()).env_clear()
            .envs(command.get_envs().filter_map(|(key, value)| value.map(|value| (key, value))))
            .current_dir(command.get_current_dir().unwrap());
        let mut command = owner;
        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut size,
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let observer = unsafe { File::from_raw_fd(slave) };
        for descriptor in [master.as_raw_fd(), observer.as_raw_fd()] {
            assert_ne!(
                unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) },
                -1
            );
        }

        assert_ne!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
            -1
        );
        command
            .stdin(Stdio::from(observer.try_clone().unwrap()))
            .stdout(Stdio::from(observer.try_clone().unwrap()))
            .stderr(Stdio::from(observer.try_clone().unwrap()));
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

        let original = TerminalSettings::read(&observer);
        Self {
            child: command.spawn().unwrap(),
            original,
            master: Some(master),
            observer: Some(observer),
            output: Vec::new(),
            root,
        }
    }

    fn read(&mut self) {
        let mut chunk = [0; 4096];
        loop {
            match self.master.as_mut().unwrap().read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => self.output.extend_from_slice(&chunk[..count]),
            }
        }
    }

    fn input(&mut self, mut bytes: &[u8], deadline: Instant) {
        while !bytes.is_empty() {
            // Darwin PTYs can accept far less than a password-sized write.
            // Drain output and retry partial/nonblocking writes under the test deadline.
            self.read();
            assert!(Instant::now() < deadline, "PTY input stalled");
            match self
                .master
                .as_mut()
                .unwrap()
                .write(&bytes[..bytes.len().min(64)])
            {
                Ok(0) => panic!("PTY input closed"),
                Ok(count) => bytes = &bytes[count..],
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("PTY input failed: {error}"),
            }
        }
    }

    fn until_prompt(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.read();
            if String::from_utf8_lossy(&self.output).contains("(Escape cancels):") {
                let mode = nix::sys::termios::tcgetattr(self.observer.as_ref().unwrap()).unwrap();
                assert!(
                    !mode
                        .local_flags
                        .contains(nix::sys::termios::LocalFlags::ECHO)
                );
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "exited before prompt: {}",
                String::from_utf8_lossy(&self.output)
            );
            assert!(
                Instant::now() < deadline,
                "no prompt: {}",
                String::from_utf8_lossy(&self.output)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn until_tui_prompt(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.read();
            if String::from_utf8_lossy(&self.output).contains("Password:") {
                let mode = nix::sys::termios::tcgetattr(self.observer.as_ref().unwrap()).unwrap();
                assert!(
                    !mode
                        .local_flags
                        .contains(nix::sys::termios::LocalFlags::ECHO)
                );
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "exited before TUI modal: {}",
                String::from_utf8_lossy(&self.output)
            );
            assert!(
                Instant::now() < deadline,
                "no TUI modal: {}",
                String::from_utf8_lossy(&self.output)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish_tui_prompt(&mut self, secret: &[u8], timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        let mut submitted = false;
        let mut restored = false;
        let mut resized = false;
        loop {
            self.read();
            let output = String::from_utf8_lossy(&self.output);
            if !submitted && output.contains("Password:") {
                let mode = nix::sys::termios::tcgetattr(self.observer.as_ref().unwrap()).unwrap();
                assert!(
                    !mode
                        .local_flags
                        .contains(nix::sys::termios::LocalFlags::ECHO)
                );
                if !resized {
                    let size = libc::winsize {
                        ws_row: 30,
                        ws_col: 100,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    };
                    assert_eq!(
                        unsafe {
                            libc::ioctl(
                                self.master.as_ref().unwrap().as_raw_fd(),
                                libc::TIOCSWINSZ,
                                &size,
                            )
                        },
                        0
                    );
                    resized = true;
                }
                self.input(b"\x1b[A", deadline);
                self.input(&secret[..secret.len().saturating_sub(1)], deadline);
                self.input(b"\r", deadline);
                submitted = true;
            }
            if !restored && self.root.join("terminal-ready").exists() {
                assert_eq!(
                    self.original,
                    TerminalSettings::read(self.observer.as_ref().unwrap()),
                    "terminal flags leaked"
                );
                fs::write(self.root.join("terminal-release"), "").unwrap();
                restored = true;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                self.read();
                assert!(
                    submitted,
                    "password modal never appeared: {}",
                    String::from_utf8_lossy(&self.output)
                );
                assert!(restored, "terminal owner exited before inspection");
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "TUI auth timed out: {}",
                String::from_utf8_lossy(&self.output)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(&mut self, replies: &[&[u8]], timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        let mut replied = 0;
        let mut restored = false;
        loop {
            self.read();
            let prompts = String::from_utf8_lossy(&self.output)
                .matches("(Escape cancels):")
                .count();
            if prompts > replied && replied < replies.len() {
                self.input(replies[replied], deadline);
                replied += 1;
            }

            if !restored && self.root.join("terminal-ready").exists() {
                assert_eq!(
                    self.original,
                    TerminalSettings::read(self.observer.as_ref().unwrap()),
                    "terminal flags leaked"
                );
                fs::write(self.root.join("terminal-release"), "").unwrap();
                restored = true;
            }

            if let Some(status) = self.child.try_wait().unwrap() {
                self.read();
                assert!(restored, "terminal owner exited before inspection");
                return status;
            }

            assert!(
                Instant::now() < deadline,
                "CLI hung: {}",
                String::from_utf8_lossy(&self.output)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for PtyRun {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }

        let pid = fs::read_to_string(self.root.join("sup-pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<i32>().ok())
            .unwrap_or(self.child.id() as i32);
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }

        let _ = fs::write(self.root.join("terminal-release"), "");
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            self.read();
            thread::sleep(Duration::from_millis(10));
        }

        if self.child.try_wait().ok().flatten().is_none() {
            // Exact fixture-owned groups cover descendants after their parent exits.
            let groups = fs::read_to_string(self.root.join("job-groups")).unwrap_or_default();
            for group in groups
                .lines()
                .filter_map(|line| line.trim().parse::<i32>().ok())
            {
                if group > 0 {
                    unsafe {
                        libc::kill(-group, libc::SIGCONT);
                        libc::kill(-group, libc::SIGKILL);
                    }
                }
            }

            let deadline = Instant::now() + Duration::from_secs(1);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                self.read();
                thread::sleep(Duration::from_millis(10));
            }
        }

        // Closing both parent descriptors releases Darwin's PTY close/drain wait.
        // Reap only after releasing these resources, including during panic cleanup.
        self.master.take();
        self.observer.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TerminalSettings {
    input: libc::tcflag_t,
    output: libc::tcflag_t,
    control: libc::tcflag_t,
    local: libc::tcflag_t,
    characters: [libc::cc_t; libc::NCCS],
    input_speed: libc::speed_t,
    output_speed: libc::speed_t,
    #[cfg(target_os = "linux")]
    line: libc::cc_t,
}

impl TerminalSettings {
    fn read(terminal: &File) -> Self {
        let mut mode = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(
            unsafe { libc::tcgetattr(terminal.as_raw_fd(), mode.as_mut_ptr()) },
            0
        );
        let mode = unsafe { mode.assume_init() };
        Self {
            input: mode.c_iflag,
            output: mode.c_oflag,
            control: mode.c_cflag,
            // PENDIN is transient kernel input-reprocessing state, not a user setting.
            // All echo/canonical/signal flags and every control character remain checked.
            local: mode.c_lflag & !libc::PENDIN,
            characters: mode.c_cc,
            input_speed: unsafe { libc::cfgetispeed(&mode) },
            output_speed: unsafe { libc::cfgetospeed(&mode) },
            #[cfg(target_os = "linux")]
            line: mode.c_line,
        }
    }
}

fn bounded_output(mut command: Command) -> Output {
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGCONT);
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }

            let _ = child.wait();
            panic!("noninteractive CLI hung");
        }
        thread::sleep(Duration::from_millis(10));
    }

    child.wait_with_output().unwrap()
}

#[test]
#[should_panic(expected = "terminal flags leaked")]
fn terminal_restoration_detects_changed_echo() {
    let fixture = Fixture::new();
    fixture.jobs(&job(
        "change-terminal",
        "none",
        "['/bin/sh', '-c', 'trap \"\" TTOU; /bin/stty -echo < /dev/tty']",
    ));
    let mut run = PtyRun::spawn(fixture.command());
    run.finish(&[], Duration::from_secs(5));
}

#[test]
fn prompt_timeout_panic_cleans_and_reaps_owned_processes() {
    let fixture = Fixture::new();
    fixture.jobs(&job("sudo", "on-demand", "['sudo', '-A', '-v']"));
    let mut run = PtyRun::spawn(fixture.command());
    run.until_prompt();
    let owner = run.child.id() as i32;
    let sup: i32 = fs::read_to_string(fixture.root().join("sup-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        run.finish(&[], Duration::from_millis(30));
    }));
    assert!(failure.is_err(), "missing test timeout");
    for pid in [owner, sup] {
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "fixture process {pid} survived cleanup"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    fixture.assert_socket_removed();
}

#[test]
fn cli_homebrew_reset_prompts_on_demand_without_preflight_or_secret_echo() {
    let fixture = Fixture::new();
    fixture.jobs(&job("brew", "on-demand", "['brew']"));
    // Fake Homebrew invalidates this prior authorization before its first sudo.
    fs::write(fixture.root().join("ticket-stale"), "").unwrap();
    let mut run = PtyRun::spawn(fixture.command());
    let status = run.finish(&[SECRET], Duration::from_secs(5));
    assert!(status.success(), "{}", String::from_utf8_lossy(&run.output));
    let trace = fixture.trace();
    assert!(
        trace.starts_with("brew-start\nreset\nsudo -A -v"),
        "{trace}"
    );
    assert_eq!(trace.matches("askpass\n").count(), 1, "{trace}");
    assert_eq!(trace.matches("sudo -n -v").count(), 1, "{trace}");
    assert!(
        trace
            .lines()
            .filter(|line| line.starts_with("sudo -n"))
            .all(|line| line.ends_with("coords=0")),
        "{trace}"
    );
    assert!(!String::from_utf8_lossy(&run.output).contains("untrusted"));
    assert!(!String::from_utf8_lossy(&run.output).contains("\x1b[31m"));
    fixture.assert_no_secret(&run.output);
    fixture.assert_socket_removed();
}

#[test]
fn dashboard_password_modal_masks_input_resizes_and_restores_terminal() {
    let fixture = Fixture::new();
    fixture.jobs(&job("brew", "on-demand", "['brew']"));
    let mut run = PtyRun::spawn(fixture.ui_command());
    assert!(
        run.finish_tui_prompt(SECRET, Duration::from_secs(5))
            .success()
    );
    assert!(String::from_utf8_lossy(&run.output).contains("fixture-brew-complete"));
    fixture.assert_no_secret(&run.output);
    fixture.assert_socket_removed();
}

#[test]
fn sudo_owns_wrong_password_retry_and_expiry_requests_a_fresh_password() {
    let fixture = Fixture::new();
    fixture.jobs(&job("brew", "on-demand", "['brew']"));
    let mut command = fixture.command();
    command.env("FIXTURE_EXPIRE", "yes");
    let mut run = PtyRun::spawn(command);
    assert!(
        run.finish(&[b"wrong\n", SECRET, SECRET], Duration::from_secs(5))
            .success(),
        "{}",
        String::from_utf8_lossy(&run.output)
    );
    assert_eq!(fixture.trace().matches("askpass\n").count(), 3);
    assert_eq!(fixture.trace().matches("retry\n").count(), 1);
    fixture.assert_no_secret(&run.output);
    fixture.assert_socket_removed();
}

#[test]
fn noninteractive_on_demand_runs_without_sudo_but_actual_request_fails() {
    let fixture = Fixture::new();
    fixture.jobs(&job("no-sudo", "on-demand", "['/bin/echo', 'safe']"));
    let result = bounded_output(fixture.command());
    assert!(result.status.success());
    assert!(fixture.trace().is_empty());
    fixture.jobs(&job("needs-sudo", "on-demand", "['sudo', '-A', '-v']"));
    let result = bounded_output(fixture.command());
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("authentication cancelled"));
    fixture.assert_socket_removed();
}

#[test]
fn preflight_is_just_in_time_and_noninteractive_unavailable_optional_jobs_skip() {
    let fixture = Fixture::new();
    fixture.jobs(&job(
        "preflight",
        "preflight",
        "['/bin/echo', 'authorized-command']",
    ));
    let mut run = PtyRun::spawn(fixture.command());
    assert!(run.finish(&[SECRET], Duration::from_secs(5)).success());
    assert!(fixture.trace().starts_with("sudo -A -v"));
    assert!(String::from_utf8_lossy(&run.output).contains("authorized-command"));
    // Fresh no-tty scope cannot reuse the PTY ticket.
    fixture.jobs(
        &job("optional", "preflight", "['/bin/echo', 'must-not-run']")
            .replace("optional = false", "optional = true"),
    );
    let output = bounded_output(fixture.command());
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Skipped"),
        "trace={} stdout={} stderr={}",
        fixture.trace(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("must-not-run"));
    fixture.jobs(&job(
        "required",
        "preflight",
        "['/bin/echo', 'must-not-run']",
    ));
    assert_eq!(bounded_output(fixture.command()).status.code(), Some(1));
}

#[test]
fn scrubbed_broker_environment_fails_promptly_with_attributed_bootstrap_error() {
    let fixture = Fixture::new();
    fixture.jobs(&job("scrubber", "on-demand", "['/bin/sh', '-c', '/usr/bin/env -i SUDO_ASKPASS=\"$SUDO_ASKPASS\" \"$SUDO_ASKPASS\" Password:']"));
    let output = bounded_output(fixture.command());
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("[scrubber]"));
    assert!(text.contains("authentication-bootstrap error"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Escape cancels"));
}

#[test]
fn standalone_sup_explicitly_rejects_missing_sibling_before_any_auth_job() {
    let fixture = Fixture::new();
    let copy = fixture.bin.join("sup");
    fs::copy(env!("CARGO_BIN_EXE_sup"), &copy).unwrap();
    fixture.jobs(&job(
        "guarded",
        "on-demand",
        "['/bin/echo', 'must-not-run']",
    ));
    let command = fixture.command();
    // Reconstruct the same isolated invocation with the copied executable.
    let mut copied = Command::new(copy);
    copied
        .args(command.get_args())
        .env_clear()
        .envs(
            command
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .current_dir(&fixture.home);
    let output = bounded_output(copied);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cargo build --bins"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("must-not-run"));
    assert!(fixture.trace().is_empty());
}

#[test]
fn long_tmpdir_uses_private_short_socket_and_cleans_it() {
    let fixture = Fixture::new();
    fixture.jobs(&job("sudo", "on-demand", "['sudo', '-A', '-v']"));
    let long = fixture.root().join("long-temporary-root-".repeat(8));
    fs::create_dir(&long).unwrap();
    let mut command = fixture.command();
    command.env("TMPDIR", &long);
    let mut run = PtyRun::spawn(command);
    run.until_prompt();
    let coordinates = fs::read_to_string(fixture.root().join("coordinates")).unwrap();
    let socket = Path::new(coordinates.lines().next().unwrap());
    assert!(socket.starts_with("/tmp"));
    assert!(socket.as_os_str().as_encoded_bytes().len() < 104);
    assert_eq!(
        fs::metadata(socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(socket.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(run.finish(&[SECRET], Duration::from_secs(5)).success());
    fixture.assert_socket_removed();
}

fn rejected_request(socket: &Path, token: &str) {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut frame = b"SUPA1".to_vec();
    frame.extend_from_slice(token.as_bytes());
    frame.extend_from_slice(&0_u16.to_be_bytes());
    // Rejection may race the last write and close/reset immediately.
    let _ = stream.write_all(&frame);
    let mut response = [0; 3];
    match stream.read(&mut response) {
        Ok(count) => assert_eq!(count, 0, "rejected token received a response"),
        Err(error) => assert!(matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
        )),
    }
}

#[test]
fn unknown_and_stale_tokens_are_rejected_and_stuck_clients_have_deadlines() {
    let fixture = Fixture::new();
    fixture.jobs(
        &(job("sudo", "on-demand", "['sudo', '-A', '-v']")
            + &job("hold", "on-demand", "['/bin/sleep', '1.8']")),
    );
    let mut run = PtyRun::spawn(fixture.command());
    run.until_prompt();
    let coordinates = fs::read_to_string(fixture.root().join("coordinates")).unwrap();
    let mut lines = coordinates.lines();
    let socket = Path::new(lines.next().unwrap());
    let token = lines.next().unwrap();
    rejected_request(socket, &"a".repeat(64));
    let mut stuck = UnixStream::connect(socket).unwrap();
    stuck.write_all(b"SUP").unwrap();
    stuck
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    run.input(SECRET, Instant::now() + Duration::from_secs(5));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        run.read();
        if String::from_utf8_lossy(&run.output).contains("Running hold") {
            break;
        }
        assert!(Instant::now() < deadline, "hold job never started");
        thread::sleep(Duration::from_millis(10));
    }

    rejected_request(socket, token);
    let mut byte = [0];
    assert!(matches!(stuck.read(&mut byte), Ok(0)));
    assert!(run.finish(&[], Duration::from_secs(5)).success());
    fixture.assert_socket_removed();
}

#[test]
fn escape_permanently_revokes_job_and_terminates_its_process_group() {
    let fixture = Fixture::new();
    fixture.jobs(&job("cancel", "on-demand", "['sudo', '-A', '-v']"));
    let mut command = fixture.command();
    command.env("FIXTURE_CANCEL_RETRY", "yes");
    let mut run = PtyRun::spawn(command);
    assert_eq!(
        run.finish(&[b"\x1b"], Duration::from_secs(5)).code(),
        Some(1)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.output)
            .matches("(Escape cancels):")
            .count(),
        1
    );
    assert!(
        fixture.trace().contains("later-denied=1"),
        "{}",
        fixture.trace()
    );
    fixture.assert_socket_removed();
}

#[test]
fn cancelled_descendant_request_is_reaped_after_its_parent_exits() {
    let fixture = Fixture::new();
    fixture.jobs(&job(
        "descendant",
        "on-demand",
        "['/bin/sh', '-c', 'sudo -A -v & exit 0']",
    ));
    let mut command = fixture.command();
    command.env("FIXTURE_CANCEL_RETRY", "yes");
    let mut run = PtyRun::spawn(command);
    assert_eq!(
        run.finish(&[b"\x1b"], Duration::from_secs(3)).code(),
        Some(1)
    );
    assert!(
        fixture.trace().contains("later-denied=1"),
        "{}",
        fixture.trace()
    );
    fixture.assert_socket_removed();
}

#[test]
fn terminal_signals_restore_plain_prompt_and_dashboard() {
    for tui in [false, true] {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            let fixture = Fixture::new();
            fixture.jobs(&job("sudo", "on-demand", "['sudo', '-A', '-v']"));
            let command = if tui {
                fixture.ui_command()
            } else {
                fixture.command()
            };
            let mut run = PtyRun::spawn(command);
            if tui {
                run.until_tui_prompt();
            } else {
                run.until_prompt();
            }
            let pid: i32 = fs::read_to_string(fixture.root().join("sup-pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
            assert_eq!(run.finish(&[], Duration::from_secs(5)).code(), Some(130));
            if tui {
                let output = String::from_utf8_lossy(&run.output);
                assert!(output.contains("\x1b[?25h"), "cursor was not restored");
                assert!(
                    output.contains("\x1b[?1049l"),
                    "alternate screen was not restored"
                );
            }
            fixture.assert_socket_removed();
        }
    }
}

#[test]
fn failed_ticket_probe_does_not_repeat_without_new_authentication() {
    let fixture = Fixture::new();
    fixture.jobs(&job("brew", "on-demand", "['brew']"));
    let mut command = fixture.command();
    command
        .env("FIXTURE_PROBE", "fail")
        .env("FIXTURE_DURATION", "1.5");
    let mut run = PtyRun::spawn(command);
    assert!(run.finish(&[SECRET], Duration::from_secs(5)).success());
    assert_eq!(
        fixture.trace().matches("sudo -n -v").count(),
        1,
        "{}",
        fixture.trace()
    );
}

#[test]
fn probe_timeout_and_cancellation_reap_the_probe_process_group() {
    for cancel in [false, true] {
        let fixture = Fixture::new();
        fixture.jobs(&job("brew", "on-demand", "['brew']"));
        let mut command = fixture.command();
        command
            .env("FIXTURE_PROBE", "hang")
            .env("FIXTURE_DURATION", "3");
        let mut run = PtyRun::spawn(command);
        run.until_prompt();
        run.input(SECRET, Instant::now() + Duration::from_secs(5));
        let deadline = Instant::now() + Duration::from_secs(3);
        let pid_file = fixture.root().join("probe-pid");
        while !pid_file.exists() {
            run.read();
            assert!(Instant::now() < deadline, "probe never started");
            thread::sleep(Duration::from_millis(10));
        }

        if cancel {
            let pid: i32 = fs::read_to_string(fixture.root().join("sup-pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
        }

        let status = run.finish(&[], Duration::from_secs(5));
        assert_eq!(status.code(), Some(if cancel { 130 } else { 0 }));
        let pid: i32 = fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "probe survived");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert_eq!(fixture.trace().matches("sudo -n -v").count(), 1);
        fixture.assert_socket_removed();
    }
}

#[test]
fn refresh_runs_at_thirty_seconds_and_stops_after_failure_during_privileged_work() {
    let fixture = Fixture::new();
    fixture.jobs(&job("brew", "on-demand", "['brew']"));
    let mut command = fixture.command();
    command
        .env("FIXTURE_REFRESH", "fail")
        .env("FIXTURE_DURATION", "62");
    let mut run = PtyRun::spawn(command);
    assert!(run.finish(&[SECRET], Duration::from_secs(67)).success());
    // Exactly one delayed probe and one failed 30-second refresh. No subsequent polling.
    assert_eq!(
        fixture.trace().matches("sudo -n -v").count(),
        2,
        "{}",
        fixture.trace()
    );
    assert!(
        fixture
            .trace()
            .lines()
            .filter(|line| line.starts_with("sudo -n"))
            .all(|line| line.ends_with("coords=0"))
    );
}

#[test]
fn parent_scoped_ticket_cannot_be_preserved_by_sup_probe() {
    let fixture = Fixture::new();
    fixture.jobs(&job("brew", "on-demand", "['brew']"));
    let mut command = fixture.command();
    command
        .env("FIXTURE_SCOPE", "parent")
        .env("FIXTURE_EXPIRE", "yes");
    let mut run = PtyRun::spawn(command);
    assert!(
        run.finish(&[SECRET, SECRET], Duration::from_secs(5))
            .success()
    );
    assert_eq!(
        fixture.trace().matches("askpass\n").count(),
        2,
        "{}",
        fixture.trace()
    );
    assert!(
        fixture.trace().contains("probe-ticket=absent"),
        "{}",
        fixture.trace()
    );
}

#[test]
fn simultaneous_jobs_serialize_real_dialogs_without_sharing_secret_replies() {
    let fixture = Fixture::new();
    let first = job("one", "on-demand", "['sudo', '-A', '-v']")
        .replace("phase = 'core'", "phase = 'parallel'");
    let second = job("two", "on-demand", "['sudo', '-A', '-v']")
        .replace("phase = 'core'", "phase = 'parallel'");
    fixture.jobs(&(first + &second));
    let mut command = fixture.command();
    command.env("FIXTURE_ALWAYS_PROMPT", "yes");
    let mut run = PtyRun::spawn(command);
    assert!(
        run.finish(&[SECRET, SECRET], Duration::from_secs(5))
            .success()
    );
    let output = String::from_utf8_lossy(&run.output);
    assert_eq!(output.matches("(Escape cancels):").count(), 2, "{output}");
    assert!(output.contains("[one] Password:"), "{output}");
    assert!(output.contains("[two] Password:"), "{output}");
    fixture.assert_no_secret(&run.output);
}

#[test]
fn refresh_is_disarmed_when_only_unrelated_work_remains() {
    let fixture = Fixture::new();
    fixture.jobs(
        &(job("preflight", "preflight", "['/bin/echo', 'ready']")
            + &job("unrelated", "none", "['/bin/sleep', '31']")),
    );
    let mut run = PtyRun::spawn(fixture.command());
    assert!(run.finish(&[SECRET], Duration::from_secs(36)).success());
    assert_eq!(
        fixture.trace().matches("sudo -n -v").count(),
        0,
        "{}",
        fixture.trace()
    );
}

#[test]
fn plain_prompt_overflow_fails_and_ctrl_c_cancels_the_run() {
    for input in [vec![b'x'; 1025], vec![3]] {
        let fixture = Fixture::new();
        fixture.jobs(&job("sudo", "on-demand", "['sudo', '-A', '-v']"));
        let mut run = PtyRun::spawn(fixture.command());
        let status = run.finish(&[&input], Duration::from_secs(5));
        assert_eq!(status.code(), Some(if input == [3] { 130 } else { 1 }));
        fixture.assert_socket_removed();
    }
}

#[test]
fn askpass_with_scrubbed_environment_has_no_native_prompt_or_stdout() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sup-askpass"));
    command
        .env_clear()
        .env("SUDO_ASKPASS", env!("CARGO_BIN_EXE_sup-askpass"))
        .arg("Password:");
    let output = bounded_output(command);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("authentication-bootstrap error"));
}

#[test]
fn a_new_terminal_session_cannot_reuse_another_terminals_fixture_ticket() {
    let fixture = Fixture::new();
    fixture.jobs(&job("sudo", "on-demand", "['sudo', '-A', '-v']"));
    // Retain the first PTY object so its device cannot be recycled for the second run.
    let mut first = PtyRun::spawn(fixture.command());
    assert!(first.finish(&[SECRET], Duration::from_secs(5)).success());
    let mut second = PtyRun::spawn(fixture.command());
    assert_eq!(
        second.finish(&[b"\x1b"], Duration::from_secs(5)).code(),
        Some(1),
        "trace={} second={}",
        fixture.trace(),
        String::from_utf8_lossy(&second.output)
    );
    assert_eq!(fixture.trace().matches("askpass\n").count(), 2);
    fixture.assert_no_secret(&first.output);
    fixture.assert_no_secret(&second.output);
}
