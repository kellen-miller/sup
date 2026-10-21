//! Per-run askpass broker. Secrets move through one-use replies and are never cached.
use crate::config::{AuthMode, Job};
use crate::runner::Cancellation;
use nix::sys::resource::{Resource, setrlimit};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zeroize::{Zeroize, Zeroizing};

const SECRET_CAPACITY: usize = 1024;
const PROMPT_CAPACITY: usize = 2048;
const IO_TIMEOUT: Duration = Duration::from_secs(1);
const DIALOG_TIMEOUT: Duration = Duration::from_secs(120);
const SUDO_TIMEOUT: Duration = Duration::from_secs(2);
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const PROBE_DELAY: Duration = Duration::from_millis(250);
const SOCKET_ENV: &str = "SUP_AUTH_SOCKET";
const TOKEN_ENV: &str = "SUP_AUTH_TOKEN";

/// Fixed allocation, deliberately neither Clone nor Debug. All allocated bytes wipe on drop.
pub struct Password {
    bytes: Zeroizing<Vec<u8>>,
    used: usize,
}

impl Default for Password {
    fn default() -> Self {
        Self::new()
    }
}

impl Password {
    pub fn new() -> Self {
        Self {
            bytes: Zeroizing::new(vec![0; SECRET_CAPACITY]),
            used: 0,
        }
    }

    pub fn push(&mut self, byte: u8) -> Result<(), &'static str> {
        if self.used == SECRET_CAPACITY || matches!(byte, 0 | b'\r' | b'\n') {
            return Err("password is too long or contains a line break");
        }

        self.bytes[self.used] = byte;
        self.used += 1;
        Ok(())
    }

    pub fn backspace(&mut self) {
        if self.used > 0 {
            self.used -= 1;
            self.bytes[self.used].zeroize();
        }
    }

    pub fn len(&self) -> usize {
        self.used
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.used]
    }
}

pub enum PasswordReply {
    Submit(Password),
    Cancel,
}

/// Consuming the responder prevents a UI from submitting a second reply.
pub struct PasswordResponder {
    sender: mpsc::SyncSender<PasswordReply>,
}

impl PasswordResponder {
    pub fn respond(self, reply: PasswordReply) {
        let _ = self.sender.try_send(reply);
    }
}

pub struct AuthRequest {
    pub request_id: u64,
    pub registered_job: String,
    pub sanitized_prompt: String,
    pub one_use_reply: PasswordResponder,
    pub deadline: Instant,
    active: Arc<AtomicBool>,
}

impl AuthRequest {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst) && Instant::now() < self.deadline
    }
}

struct RegisteredJob {
    name: String,
    requests: u32,
    cancelled: Option<String>,
}

#[derive(Default)]
struct BrokerState {
    jobs: HashMap<String, RegisteredJob>,
    generation: u64,
    probe_due: Option<Instant>,
    refresh_due: Option<Instant>,
}

pub struct AuthSession {
    directory: Option<TempDir>,
    socket: PathBuf,
    helper: PathBuf,
    state: Arc<Mutex<BrokerState>>,
    stopped: Arc<AtomicBool>,
    broker: Option<JoinHandle<()>>,
    refresh: Option<JoinHandle<()>>,
}

/// Registration lifetime exactly covers preflight and execution, including output drain.
pub struct JobAuthorization {
    token: Option<String>,
    state: Arc<Mutex<BrokerState>>,
    socket: PathBuf,
    helper: PathBuf,
}

impl AuthSession {
    pub fn new(
        jobs: &[Job],
        requests: mpsc::SyncSender<AuthRequest>,
        cancellation: Cancellation,
    ) -> Result<Self, String> {
        if jobs.iter().all(|job| job.auth == AuthMode::None) {
            return Ok(Self {
                directory: None,
                socket: PathBuf::new(),
                helper: PathBuf::new(),
                state: Arc::new(Mutex::new(BrokerState::default())),
                stopped: Arc::new(AtomicBool::new(false)),
                broker: None,
                refresh: None,
            });
        }

        let helper = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .parent()
            .unwrap_or(Path::new("."))
            .join("sup-askpass");
        if !helper.is_absolute()
            || !fs::metadata(&helper)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        {
            return Err("sup-askpass is missing or not executable; run cargo build --bins or reinstall all binaries together".into());
        }

        disable_core_dumps()?;
        let temporary_root = std::env::temp_dir();
        let temporary_root = if temporary_root.is_absolute() {
            temporary_root
        } else {
            std::env::current_dir()
                .map_err(|error| error.to_string())?
                .join(temporary_root)
        };
        let mut directory = tempfile::Builder::new()
            .prefix("sa-")
            .tempdir_in(temporary_root)
            .map_err(|error| format!("cannot create auth directory: {error}"))?;
        // Darwin sockaddr_un.sun_path is 104 bytes, including its trailing NUL.
        if directory
            .path()
            .join("s")
            .as_os_str()
            .as_encoded_bytes()
            .len()
            >= 104
        {
            directory = tempfile::Builder::new()
                .prefix("sa-")
                .tempdir_in("/tmp")
                .map_err(|error| format!("cannot create short auth directory: {error}"))?;
        }

        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot secure auth directory: {error}"))?;
        let socket = directory.path().join("s");
        let listener = UnixListener::bind(&socket)
            .map_err(|error| format!("cannot bind auth socket: {error}"))?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("cannot secure auth socket: {error}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let state = Arc::new(Mutex::new(BrokerState::default()));
        let stopped = Arc::new(AtomicBool::new(false));
        let broker_state = state.clone();
        let broker_stop = stopped.clone();
        let broker_cancel = cancellation.clone();
        let broker = thread::spawn(move || {
            let ids = Arc::new(AtomicU64::new(1));
            let dialog = Arc::new(Mutex::new(()));
            let mut clients: Vec<JoinHandle<()>> = Vec::new();
            while !broker_stop.load(Ordering::SeqCst) && !broker_cancel.is_cancelled() {
                let mut index = 0;
                while index < clients.len() {
                    if clients[index].is_finished() {
                        let _ = clients.swap_remove(index).join();
                    } else {
                        index += 1;
                    }
                }

                match listener.accept() {
                    Ok((stream, _)) if clients.len() < 8 => {
                        let context = BrokerConnection {
                            io: SocketIo {
                                stream,
                                deadline: Instant::now() + IO_TIMEOUT,
                                stopped: broker_stop.clone(),
                                cancellation: broker_cancel.clone(),
                            },
                            state: broker_state.clone(),
                            dialog: dialog.clone(),
                            requests: requests.clone(),
                            ids: ids.clone(),
                        };
                        clients.push(thread::spawn(move || {
                            let _ = context.respond();
                        }));
                    }
                    Ok(_) => {} // Connection limit: close immediately.
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }

            broker_stop.store(true, Ordering::SeqCst);
            for client in clients {
                let _ = client.join();
            }
        });
        let refresh_state = state.clone();
        let refresh_stop = stopped.clone();
        let refresh = thread::spawn(move || {
            while !refresh_stop.load(Ordering::SeqCst) && !cancellation.is_cancelled() {
                let due = {
                    let state = refresh_state.lock().unwrap_or_else(|e| e.into_inner());
                    let due = state.probe_due.or(state.refresh_due);
                    due.filter(|due| *due <= Instant::now())
                        .map(|_| state.generation)
                };
                if let Some(generation) = due {
                    // A delivered password is only a reason to probe; it is not a ticket.
                    let success = sudo_status(
                        &["-n", "-v"],
                        None,
                        &refresh_state,
                        &refresh_stop,
                        &cancellation,
                        SUDO_TIMEOUT,
                    );
                    let mut state = refresh_state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.generation == generation {
                        state.probe_due = None;
                        state.refresh_due = success.then(|| Instant::now() + REFRESH_INTERVAL);
                    }
                }

                thread::sleep(Duration::from_millis(20));
            }
        });
        Ok(Self {
            directory: Some(directory),
            socket,
            helper,
            state,
            stopped,
            broker: Some(broker),
            refresh: Some(refresh),
        })
    }

    pub fn register(&self, job: &Job) -> Result<JobAuthorization, String> {
        let mut authorization = JobAuthorization {
            token: None,
            state: self.state.clone(),
            socket: self.socket.clone(),
            helper: PathBuf::new(),
        };
        if job.auth == AuthMode::None {
            return Ok(authorization);
        }

        if self.directory.is_none() {
            return Err("authentication session has no privileged jobs".into());
        }

        let mut random = [0; 32];
        getrandom::fill(&mut random)
            .map_err(|error| format!("cannot create auth token: {error}"))?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.jobs.insert(
            token.clone(),
            RegisteredJob {
                name: job.name.clone(),
                requests: 0,
                cancelled: None,
            },
        );
        authorization.token = Some(token);
        authorization.helper = self.helper.clone();
        Ok(authorization)
    }

    pub fn preflight(&self, job: &JobAuthorization, cancellation: &Cancellation) -> bool {
        let success = sudo_status(
            &["-A", "-v"],
            Some(job),
            &self.state,
            &self.stopped,
            cancellation,
            DIALOG_TIMEOUT,
        );
        if success {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.generation += 1;
            state.probe_due = None;
            state.refresh_due = Some(Instant::now() + REFRESH_INTERVAL);
        }

        success
    }
}

impl Drop for AuthSession {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(broker) = self.broker.take() {
            let _ = broker.join();
        }

        if let Some(refresh) = self.refresh.take() {
            let _ = refresh.join();
        }

        let _ = fs::remove_file(&self.socket);
        // TempDir removes the private directory even during unwinding.
        self.directory.take();
    }
}

impl JobAuthorization {
    pub fn configure(&self, command: &mut Command) {
        command
            .env_remove("SUDO_ASKPASS")
            .env_remove(SOCKET_ENV)
            .env_remove(TOKEN_ENV);
        if let Some(token) = &self.token {
            command
                .env("SUDO_ASKPASS", &self.helper)
                .env(SOCKET_ENV, &self.socket)
                .env(TOKEN_ENV, token);
        }
    }

    pub fn failure(&self) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.token
            .as_ref()
            .and_then(|token| state.jobs.get(token))
            .and_then(|job| job.cancelled.clone())
    }
}

impl Drop for JobAuthorization {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = &self.token {
            state.jobs.remove(token);
        }

        if state.jobs.is_empty() {
            state.generation += 1;
            state.probe_due = None;
            state.refresh_due = None;
        }
    }
}

fn disable_core_dumps() -> Result<(), String> {
    setrlimit(Resource::RLIMIT_CORE, 0, 0)
        .map_err(|error| format!("cannot disable core dumps: {error}"))
}

struct BrokerConnection {
    io: SocketIo,
    state: Arc<Mutex<BrokerState>>,
    dialog: Arc<Mutex<()>>,
    requests: mpsc::SyncSender<AuthRequest>,
    ids: Arc<AtomicU64>,
}

impl BrokerConnection {
    fn respond(mut self) -> io::Result<()> {
        self.io.stream.set_nonblocking(true)?;
        #[cfg(target_os = "macos")]
        let uid = nix::unistd::getpeereid(&self.io.stream)
            .map_err(io::Error::other)?
            .0;
        #[cfg(target_os = "linux")]
        let uid = nix::unistd::Uid::from_raw(
            nix::sys::socket::getsockopt(
                &self.io.stream,
                nix::sys::socket::sockopt::PeerCredentials,
            )
            .map_err(io::Error::other)?
            .uid(),
        );
        if uid != nix::unistd::getuid() {
            return Err(io::Error::other("auth peer rejected"));
        }

        let mut header = [0; 71];
        self.io.read_exact(&mut header)?;
        if &header[..5] != b"SUPA1" {
            return Err(io::Error::other("invalid auth protocol"));
        }

        let token = std::str::from_utf8(&header[5..69])
            .map_err(io::Error::other)?
            .to_owned();
        let length = u16::from_be_bytes([header[69], header[70]]) as usize;
        if length > PROMPT_CAPACITY {
            return Err(io::Error::other("auth prompt too long"));
        }

        let name = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let job = state
                .jobs
                .get_mut(&token)
                .ok_or_else(|| io::Error::other("unknown auth token"))?;
            if job.cancelled.is_some() || job.requests >= 32 {
                return Err(io::Error::other("auth token cancelled or exhausted"));
            }

            job.requests += 1;
            job.name.clone()
        };
        let mut prompt = vec![0; length];
        self.io.read_exact(&mut prompt)?;
        self.io.deadline = Instant::now() + DIALOG_TIMEOUT;
        // Waiting for another dialog is cancellable and bounded too.
        let _dialog = loop {
            self.io.check()?;
            if !self.job_active(&token) || self.io.disconnected() {
                return Err(io::Error::other("auth job ended"));
            }

            if let Ok(dialog) = self.dialog.try_lock() {
                break dialog;
            }

            thread::sleep(Duration::from_millis(20));
        };
        let (reply, receiver) = mpsc::sync_channel(1);
        let active = Arc::new(AtomicBool::new(true));
        let request = AuthRequest {
            request_id: self.ids.fetch_add(1, Ordering::SeqCst),
            registered_job: name,
            sanitized_prompt: sanitize_prompt(&prompt),
            one_use_reply: PasswordResponder { sender: reply },
            deadline: self.io.deadline,
            active: active.clone(),
        };
        let outcome = if self.requests.try_send(request).is_err() {
            Err(io::Error::other("authentication reader unavailable"))
        } else {
            loop {
                if let Err(error) = self.io.check() {
                    break Err(error);
                }

                if !self.job_active(&token) || self.io.disconnected() {
                    break Err(io::Error::other("auth job ended"));
                }

                match receiver.recv_timeout(Duration::from_millis(20)) {
                    Ok(PasswordReply::Submit(password)) => {
                        // Serialize the final authorization check with token revocation.
                        // The write is bounded, and no secret enters shared state.
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if !state
                            .jobs
                            .get(&token)
                            .is_some_and(|job| job.cancelled.is_none())
                        {
                            break Err(io::Error::other("auth job ended"));
                        }

                        self.io.deadline = Instant::now() + IO_TIMEOUT;
                        let length = (password.used as u16).to_be_bytes();
                        let result = self
                            .io
                            .write_all(&[1, length[0], length[1]])
                            .and_then(|()| self.io.write_all(password.as_bytes()));
                        if result.is_ok() {
                            state.generation += 1;
                            state.probe_due = Some(Instant::now() + PROBE_DELAY);
                            state.refresh_due = None;
                        }

                        break result;
                    }
                    Ok(PasswordReply::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break Err(io::Error::other("authentication cancelled"));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        };
        active.store(false, Ordering::SeqCst);
        if outcome.is_err() {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = state.jobs.get_mut(&token) {
                job.cancelled = Some("authentication cancelled, unavailable, or expired".into());
            }
        }

        outcome
    }

    fn job_active(&self, token: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .get(token)
            .is_some_and(|job| job.cancelled.is_none())
    }
}

struct SocketIo {
    stream: UnixStream,
    deadline: Instant,
    stopped: Arc<AtomicBool>,
    cancellation: Cancellation,
}

impl SocketIo {
    fn check(&self) -> io::Result<()> {
        if self.stopped.load(Ordering::SeqCst)
            || self.cancellation.is_cancelled()
            || Instant::now() >= self.deadline
        {
            return Err(io::Error::other("authentication cancelled or expired"));
        }

        Ok(())
    }

    fn disconnected(&self) -> bool {
        let mut fd = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe {
            libc::poll(&mut fd, 1, 0);
        }
        fd.revents & (libc::POLLHUP | libc::POLLERR) != 0
    }

    fn read_exact(&mut self, mut bytes: &mut [u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            self.check()?;
            match self.stream.read(bytes) {
                Ok(0) => return Err(io::Error::other("auth connection closed")),
                Ok(count) => {
                    bytes = &mut bytes[count..];
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }

        Ok(())
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            self.check()?;
            match self.stream.write(bytes) {
                Ok(0) => return Err(io::Error::other("auth connection closed")),
                Ok(count) => {
                    bytes = &bytes[count..];
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }

        Ok(())
    }
}

fn sanitize_prompt(bytes: &[u8]) -> String {
    let mut text = String::new();
    let mut escape = false;
    let mut csi = false;
    let mut osc = false;
    for character in String::from_utf8_lossy(bytes).chars() {
        if osc {
            if character == '\x07' {
                osc = false;
            }
            if escape && character == '\\' {
                osc = false;
            }
            escape = character == '\x1b';
            continue;
        }

        if csi {
            if ('\x40'..='\x7e').contains(&character) {
                csi = false;
            }
            continue;
        }

        if escape {
            csi = character == '[';
            osc = character == ']';
            escape = false;
            continue;
        }

        if character == '\x1b' {
            escape = true;
            continue;
        }
        if !character.is_control() && text.len() + character.len_utf8() <= 256 {
            text.push(character);
        }
    }

    text
}

/// Askpass entry point: its stdout contains only the current supplied password.
pub fn askpass(prompt: &str) -> Result<(), String> {
    disable_core_dumps()?;
    let socket = std::env::var_os(SOCKET_ENV)
        .ok_or("authentication-bootstrap error: missing private broker coordinates")?;
    let token = std::env::var(TOKEN_ENV)
        .map_err(|_| "authentication-bootstrap error: missing private job token")?;
    if !Path::new(&socket).is_absolute()
        || token.len() != 64
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("authentication-bootstrap error: invalid broker coordinates".into());
    }

    let address = nix::sys::socket::UnixAddr::new(Path::new(&socket))
        .map_err(|_| "authentication-bootstrap error: invalid socket path")?;
    let descriptor = nix::sys::socket::socket(
        nix::sys::socket::AddressFamily::Unix,
        nix::sys::socket::SockType::Stream,
        nix::sys::socket::SockFlag::empty(),
        None,
    )
    .map_err(|_| "authentication-bootstrap error: cannot create socket")?;
    let stream = UnixStream::from(descriptor);
    stream
        .set_nonblocking(true)
        .map_err(|_| "cannot configure auth socket")?;
    if unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err("cannot secure auth socket".into());
    }

    nix::sys::socket::connect(stream.as_raw_fd(), &address)
        .map_err(|_| "authentication-bootstrap error: broker unavailable or busy")?;

    let mut io = SocketIo {
        stream,
        deadline: Instant::now() + IO_TIMEOUT,
        stopped: Arc::new(AtomicBool::new(false)),
        cancellation: Cancellation::new(),
    };
    let prompt = prompt.as_bytes();
    let prompt = &prompt[..prompt.len().min(PROMPT_CAPACITY)];
    let length = (prompt.len() as u16).to_be_bytes();
    io.write_all(b"SUPA1")
        .and_then(|()| io.write_all(token.as_bytes()))
        .and_then(|()| io.write_all(&length))
        .and_then(|()| io.write_all(prompt))
        .map_err(|_| "authentication request failed")?;
    io.deadline = Instant::now() + DIALOG_TIMEOUT + IO_TIMEOUT;
    let mut header = [0; 3];
    io.read_exact(&mut header)
        .map_err(|_| "authentication cancelled or expired")?;
    let length = u16::from_be_bytes([header[1], header[2]]) as usize;
    if header[0] != 1 || length > SECRET_CAPACITY {
        return Err("invalid authentication response".into());
    }

    let mut password = Password::new();
    io.read_exact(&mut password.bytes[..length])
        .map_err(|_| "authentication response failed")?;
    password.used = length;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    stdout
        .write_all(password.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .map_err(|_| "cannot deliver authentication response")?;
    Ok(())
}

// One process owner reaps preflight, probe and keepalive children, including stopped groups.
fn sudo_status(
    arguments: &[&str],
    authorization: Option<&JobAuthorization>,
    state: &Mutex<BrokerState>,
    stopped: &AtomicBool,
    cancellation: &Cancellation,
    timeout: Duration,
) -> bool {
    // Do not start a refresh after the last privileged registration has ended.
    let active = state.lock().unwrap_or_else(|e| e.into_inner());
    if stopped.load(Ordering::SeqCst)
        || cancellation.is_cancelled()
        || !active.jobs.values().any(|job| job.cancelled.is_none())
        || authorization.is_some_and(|authorization| {
            authorization
                .token
                .as_ref()
                .and_then(|token| active.jobs.get(token))
                .is_none_or(|job| job.cancelled.is_some())
        })
    {
        return false;
    }

    let mut command = Command::new("sudo");
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("SUDO_ASKPASS")
        .env_remove(SOCKET_ENV)
        .env_remove(TOKEN_ENV)
        .process_group(0);
    if let Some(authorization) = authorization {
        authorization.configure(&mut command);
    }
    let Ok(child) = command.spawn() else {
        return false;
    };
    drop(active);
    drop(command);
    let pid = child.id() as i32;
    drop(child);
    cancellation.register(pid);
    let deadline = Instant::now() + timeout;
    let mut terminate_at = None;
    let mut success = false;
    let mut reaped = false;
    loop {
        let no_jobs = state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .is_empty();
        let cancelled = cancellation.is_cancelled()
            || stopped.load(Ordering::SeqCst)
            || no_jobs
            || authorization.is_some_and(|authorization| authorization.failure().is_some());
        if (cancelled || Instant::now() >= deadline) && terminate_at.is_none() {
            unsafe {
                libc::kill(-pid, libc::SIGCONT);
                libc::kill(-pid, libc::SIGTERM);
            }
            terminate_at = Some(Instant::now() + Duration::from_millis(100));
        }

        let mut status = 0;
        let waited = if reaped {
            0
        } else {
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) }
        };
        if waited == pid {
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                reaped = true;
                success = terminate_at.is_none()
                    && libc::WIFEXITED(status)
                    && libc::WEXITSTATUS(status) == 0;
            } else if libc::WIFSTOPPED(status) {
                unsafe {
                    libc::kill(-pid, libc::SIGCONT);
                    libc::kill(-pid, libc::SIGTERM);
                }
                terminate_at = Some(Instant::now() + Duration::from_millis(100));
            }
        } else if waited == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
            reaped = true;
        }

        if terminate_at.is_some_and(|at| Instant::now() >= at) {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            terminate_at = None;
            if reaped {
                break;
            }
        }

        if reaped && terminate_at.is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    cancellation.unregister(pid);
    success
}

/// Production plain-mode consumer. Terminal state and all input buffers have one owner.
pub struct PlainPrompt {
    tty: Option<File>,
}

impl Default for PlainPrompt {
    fn default() -> Self {
        Self::new()
    }
}

impl PlainPrompt {
    pub fn new() -> Self {
        Self {
            tty: if std::io::stdin().is_terminal() {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/dev/tty")
                    .ok()
            } else {
                None
            },
        }
    }

    pub fn respond(&mut self, request: AuthRequest, cancellation: &Cancellation) {
        let reply = self
            .read(&request, cancellation)
            .unwrap_or(PasswordReply::Cancel);
        request.one_use_reply.respond(reply);
    }

    fn read(
        &mut self,
        request: &AuthRequest,
        cancellation: &Cancellation,
    ) -> io::Result<PasswordReply> {
        let Some(tty) = &mut self.tty else {
            return Ok(PasswordReply::Cancel);
        };
        if !request.is_active() || cancellation.is_cancelled() {
            return Ok(PasswordReply::Cancel);
        }
        let original = nix::sys::termios::tcgetattr(&*tty).map_err(io::Error::other)?;
        let mut mode = original.clone();
        mode.local_flags
            .remove(nix::sys::termios::LocalFlags::ECHO | nix::sys::termios::LocalFlags::ICANON);
        mode.control_chars[nix::sys::termios::SpecialCharacterIndices::VMIN as usize] = 0;
        mode.control_chars[nix::sys::termios::SpecialCharacterIndices::VTIME as usize] = 0;
        nix::sys::termios::tcsetattr(&*tty, nix::sys::termios::SetArg::TCSANOW, &mode)
            .map_err(io::Error::other)?;
        let terminal = PromptTerminal { tty, original };
        write!(
            terminal.tty,
            "[{}] {} (Escape cancels): ",
            request.registered_job, request.sanitized_prompt
        )?;
        terminal.tty.flush()?;
        let mut password = Password::new();
        let mut scratch = Zeroizing::new([0; 1]);
        loop {
            if !request.is_active() || cancellation.is_cancelled() {
                return Ok(PasswordReply::Cancel);
            }
            let mut fd = libc::pollfd {
                fd: terminal.tty.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut fd, 1, 20) };
            if ready == -1 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io::Error::last_os_error());
            }

            if fd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                return Ok(PasswordReply::Cancel);
            }
            if ready == 0 {
                continue;
            }
            if terminal.tty.read(&mut scratch[..])? == 0 {
                continue;
            }
            match scratch[0] {
                b'\r' | b'\n' => return Ok(PasswordReply::Submit(password)),
                27 => return Ok(PasswordReply::Cancel),
                3 => {
                    cancellation.cancel();
                    return Ok(PasswordReply::Cancel);
                }
                8 | 127 => password.backspace(),
                byte => {
                    password.push(byte).map_err(io::Error::other)?;
                }
            }

            scratch.zeroize();
        }
    }
}

struct PromptTerminal<'a> {
    tty: &'a mut File,
    original: nix::sys::termios::Termios,
}

impl Drop for PromptTerminal<'_> {
    fn drop(&mut self) {
        let _ = nix::sys::termios::tcflush(&*self.tty, nix::sys::termios::FlushArg::TCIFLUSH);
        let _ = nix::sys::termios::tcsetattr(
            &*self.tty,
            nix::sys::termios::SetArg::TCSANOW,
            &self.original,
        );
        let _ = writeln!(self.tty);
    }
}
