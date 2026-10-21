use crate::auth::{AuthRequest, AuthSession};
use crate::config::{AuthMode, Job};
use crate::logs;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_CHUNK: usize = 16 * 1024;
const TERMINATE_AFTER: Duration = Duration::from_secs(1);

static SIGNAL_CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub home: PathBuf,
    pub retention_days: i64,
    pub cleanup_logs: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobStatus {
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct JobResult {
    pub job: Job,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub elapsed: Duration,
    pub log_path: Option<PathBuf>,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub enum RunEvent {
    RunDirectory(PathBuf),
    Started(String),
    Output { job: String, text: String },
    Finished(Box<JobResult>),
}

#[derive(Clone, Default)]
pub struct Cancellation {
    requested: Arc<AtomicBool>,
    active_groups: Arc<Mutex<HashSet<i32>>>,
}

impl Cancellation {
    pub fn new() -> Self {
        SIGNAL_CANCELLED.store(false, Ordering::SeqCst);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            // SAFETY: the handler only stores to an atomic flag; cleanup stays in the runner.
            unsafe {
                libc::signal(signal, signal_handler as *const () as libc::sighandler_t);
            }
        }
        Self::default()
    }

    pub fn cancel(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.terminate_groups();
    }

    pub fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::SeqCst) || SIGNAL_CANCELLED.load(Ordering::SeqCst)
    }

    pub(crate) fn register(&self, pid: i32) {
        let mut groups = self.active_groups.lock().unwrap_or_else(|e| e.into_inner());
        groups.insert(pid);
        if self.is_cancelled() {
            drop(groups);
            self.cancel();
        }
    }

    pub(crate) fn unregister(&self, pid: i32) {
        self.active_groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&pid);
    }

    fn terminate_groups(&self) {
        let groups = self.active_groups.lock().unwrap_or_else(|e| e.into_inner());
        for pid in groups.iter().copied() {
            // SAFETY: registered pids are process-group leaders spawned by this runner.
            unsafe {
                libc::kill(-pid, libc::SIGCONT);
                libc::kill(-pid, libc::SIGTERM);
            }
        }
    }
}

extern "C" fn signal_handler(_: libc::c_int) {
    SIGNAL_CANCELLED.store(true, Ordering::SeqCst);
}

/// Runs jobs in configured order, executing core jobs before parallel jobs.
///
/// # Errors
///
/// Returns an error when run-log setup or authentication setup fails.
pub fn run(
    jobs: &[Job],
    options: RunOptions,
    events: mpsc::SyncSender<RunEvent>,
    auth_requests: mpsc::SyncSender<AuthRequest>,
    cancellation: Cancellation,
) -> Result<Vec<JobResult>, String> {
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let run_dir =
        logs::create_run_dir(&options.home).map_err(|error| format!("cannot create run directory: {error}"))?;
    notify(&events, RunEvent::RunDirectory(run_dir.clone()));
    if options.cleanup_logs {
        logs::cleanup_old_runs(&options.home, &run_dir, options.retention_days)
            .map_err(|error| format!("cannot clean old run logs: {error}"))?;
    }

    let auth = AuthSession::new(jobs, auth_requests, cancellation.clone())?;
    let mut results = vec![None; jobs.len()];
    for (index, job) in jobs.iter().enumerate().filter(|(_, job)| job.phase == "core") {
        let result = if cancellation.is_cancelled() {
            cancelled(job, "run cancelled")
        } else {
            match prepare(job, &run_dir, &cancellation) {
                Ok(Some((path, file))) => execute(job, path, file, &events, &auth, &cancellation),
                Ok(None) => requirement_result(job, &run_dir),
                Err(result) => *result,
            }
        };
        notify(&events, RunEvent::Finished(Box::new(result.clone())));
        results[index] = Some(result);
    }

    let mut prepared = Vec::new();
    for (index, job) in jobs.iter().enumerate().filter(|(_, job)| job.phase == "parallel") {
        let result = if cancellation.is_cancelled() {
            Some(cancelled(job, "run cancelled"))
        } else {
            match prepare(job, &run_dir, &cancellation) {
                Ok(Some((path, file))) => {
                    prepared.push((index, job, path, file));
                    None
                }
                Ok(None) => Some(requirement_result(job, &run_dir)),
                Err(result) => Some(*result),
            }
        };
        if let Some(result) = result {
            notify(&events, RunEvent::Finished(Box::new(result.clone())));
            results[index] = Some(result);
        }
    }

    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(prepared.len());
        for (index, job, path, file) in prepared {
            let events = events.clone();
            let cancellation = cancellation.clone();
            let auth = &auth;
            workers.push((
                index,
                job,
                scope.spawn(move || {
                    let result = execute(job, path, file, &events, auth, &cancellation);
                    notify(&events, RunEvent::Finished(Box::new(result.clone())));
                    result
                }),
            ));
        }
        for (index, job, worker) in workers {
            results[index] = Some(worker.join().unwrap_or_else(|_| {
                let result = failed(job, "parallel job worker panicked", None, None, Duration::ZERO);
                notify(&events, RunEvent::Finished(Box::new(result.clone())));
                result
            }));
        }
    });
    Ok(results.into_iter().flatten().collect())
}

fn notify(events: &mpsc::SyncSender<RunEvent>, event: RunEvent) -> bool {
    match events.send(event) {
        Ok(()) => true,
        Err(_) => {
            // The UI may close while jobs are still running; keep execution and log capture alive.
            false
        }
    }
}

pub fn exit_code(results: &[JobResult], cancellation: &Cancellation) -> i32 {
    if cancellation.is_cancelled() {
        130
    } else if results.iter().any(|result| result.status == JobStatus::Failed) {
        1
    } else {
        0
    }
}

pub fn displayed_command(job: &Job) -> Vec<String> {
    let mut command = job.command.clone();
    if command.first().is_some_and(|value| value == "sup-rust-update")
        && let Ok(executable) = std::env::current_exe()
    {
        command[0] = executable
            .parent()
            .unwrap_or(Path::new("."))
            .join("sup-rust-update")
            .to_string_lossy()
            .into_owned();
    }
    command
}

fn prepare(job: &Job, run_dir: &Path, cancellation: &Cancellation) -> Result<Option<(PathBuf, File)>, Box<JobResult>> {
    let mut missing = Vec::new();
    for variable in &job.required_env {
        if std::env::var_os(variable).is_none_or(|value| value.is_empty()) {
            missing.push(format!("env:{variable}"));
        }
    }
    for command in &job.required_commands {
        if !command_exists(command) {
            missing.push(format!("command:{command}"));
        }
    }
    for path in &job.required_paths {
        if !path.exists() {
            missing.push(format!("path:{}", path.display()));
        }
    }
    if !missing.is_empty() {
        return Ok(None);
    }
    if cancellation.is_cancelled() {
        return Err(Box::new(cancelled(job, "run cancelled during preflight")));
    }
    match logs::create_job_log(run_dir, &job.log_name) {
        Ok((path, file)) => Ok(Some((path, file))),
        Err(error) => Err(Box::new(failed(
            job,
            &format!("cannot create job log: {error}"),
            Some(1),
            None,
            Duration::ZERO,
        ))),
    }
}

fn requirement_result(job: &Job, run_dir: &Path) -> JobResult {
    let status = if job.optional {
        JobStatus::Skipped
    } else {
        JobStatus::Failed
    };
    let reason = if job.optional {
        "missing optional requirement"
    } else {
        "missing required requirement"
    };
    let log_path = match logs::create_job_log(run_dir, &job.log_name) {
        Ok((path, mut file)) => {
            if let Err(error) = writeln!(file, "{reason}") {
                eprintln!("sup: cannot write requirement result to {}: {error}", path.display());
            }
            Some(path)
        }
        Err(_) => None,
    };
    JobResult {
        job: job.clone(),
        status,
        exit_code: None,
        elapsed: Duration::ZERO,
        log_path,
        reason: reason.into(),
    }
}

fn execute(
    job: &Job,
    log_path: PathBuf,
    log: File,
    events: &mpsc::SyncSender<RunEvent>,
    auth: &AuthSession,
    cancellation: &Cancellation,
) -> JobResult {
    let (authorization, resolved) = match authorize_job(job, &log_path, auth, cancellation) {
        Ok(job) => job,
        Err(result) => return *result,
    };
    notify(events, RunEvent::Started(job.name.clone()));
    let mut command = Command::new(&resolved[0]);
    command.args(&resolved[1..]).stdin(Stdio::null());
    authorization.configure(&mut command);
    let (mut reader, writer) = match output_pipe() {
        Ok(pipe) => pipe,
        Err(error) => return failed(job, &error, Some(1), Some(log_path), Duration::ZERO),
    };
    let stderr_writer = match writer.try_clone() {
        Ok(writer) => writer,
        Err(error) => {
            return failed(
                job,
                &format!("cannot duplicate output pipe: {error}"),
                Some(1),
                Some(log_path),
                Duration::ZERO,
            );
        }
    };
    command.stdout(Stdio::from(writer)).stderr(Stdio::from(stderr_writer));
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    #[cfg(unix)]
    command.process_group(0);
    let started = Instant::now();
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return failed(
                job,
                &format!("cannot start {:?}: {error}", resolved),
                Some(1),
                Some(log_path),
                started.elapsed(),
            );
        }
    };
    drop(command);
    let pid = child.id() as i32;
    drop(child);
    cancellation.register(pid);
    let log = Arc::new(Mutex::new(log));
    let reader_log = log.clone();
    let reader_job = job.name.clone();
    let reader_events = events.clone();
    let reader_thread = thread::spawn(move || read_output(&mut reader, reader_log, reader_job, reader_events));
    let (exit_code, failure) = monitor_child(pid, reader_thread, log.clone(), &authorization, cancellation);
    cancellation.unregister(pid);

    let elapsed = started.elapsed();
    if cancellation.is_cancelled() {
        return JobResult {
            job: job.clone(),
            status: JobStatus::Cancelled,
            exit_code,
            elapsed,
            log_path: Some(log_path),
            reason: "run cancelled".into(),
        };
    }
    if let Some(reason) = failure {
        return failed(job, &reason, exit_code, Some(log_path), elapsed);
    }
    let code = exit_code.unwrap_or(1);
    JobResult {
        job: job.clone(),
        status: if code == 0 {
            JobStatus::Succeeded
        } else {
            JobStatus::Failed
        },
        exit_code: Some(code),
        elapsed,
        log_path: Some(log_path),
        reason: String::new(),
    }
}

fn authorize_job(
    job: &Job,
    log_path: &Path,
    auth: &AuthSession,
    cancellation: &Cancellation,
) -> Result<(crate::auth::JobAuthorization, Vec<String>), Box<JobResult>> {
    let authorization = match auth.register(job) {
        Ok(authorization) => authorization,
        Err(reason) => {
            return Err(Box::new(failed(
                job,
                &reason,
                Some(1),
                Some(log_path.to_path_buf()),
                Duration::ZERO,
            )));
        }
    };
    if job.auth == AuthMode::Preflight && !auth.preflight(&authorization, cancellation) {
        if cancellation.is_cancelled() {
            return Err(Box::new(cancelled(job, "run cancelled during authentication")));
        }

        let mut result = failed(
            job,
            "sudo preflight authorization unavailable",
            Some(1),
            Some(log_path.to_path_buf()),
            Duration::ZERO,
        );
        if job.optional {
            result.status = JobStatus::Skipped;
        }
        return Err(Box::new(result));
    }

    if cancellation.is_cancelled() {
        return Err(Box::new(cancelled(job, "run cancelled before execution")));
    }

    let resolved = match resolved_command(job) {
        Ok(command) => command,
        Err(reason) => {
            return Err(Box::new(failed(
                job,
                &reason,
                Some(1),
                Some(log_path.to_path_buf()),
                Duration::ZERO,
            )));
        }
    };
    Ok((authorization, resolved))
}

fn output_pipe() -> Result<(File, File), String> {
    let mut descriptors = [-1; 2];
    // SAFETY: `descriptors` points to two writable integers as required by pipe(2).
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } == -1 {
        return Err(format!("cannot create output pipe: {}", io::Error::last_os_error()));
    }
    // SAFETY: pipe(2) initialized each descriptor and ownership transfers to File.
    let reader = unsafe { File::from_raw_fd(descriptors[0]) };
    // SAFETY: pipe(2) initialized each descriptor and ownership transfers to File.
    let writer = unsafe { File::from_raw_fd(descriptors[1]) };
    for fd in [reader.as_raw_fd(), writer.as_raw_fd()] {
        // SAFETY: fd is an open descriptor owned by reader or writer.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(format!("cannot secure output pipe: {}", io::Error::last_os_error()));
        }
    }
    Ok((reader, writer))
}

fn monitor_child(
    pid: i32,
    reader_thread: thread::JoinHandle<Result<(), String>>,
    log: Arc<Mutex<File>>,
    authorization: &crate::auth::JobAuthorization,
    cancellation: &Cancellation,
) -> (Option<i32>, Option<String>) {
    let mut exit_code = None;
    let mut failure = None;
    let mut terminate_at = None;
    let mut reaped = false;
    loop {
        if let Some(reason) = authorization.failure()
            && failure.is_none()
        {
            failure = Some(reason);
            terminate_process_group(pid);
            terminate_at = Some(Instant::now() + TERMINATE_AFTER);
        }

        if cancellation.is_cancelled() && terminate_at.is_none() {
            cancellation.terminate_groups();
            terminate_at = Some(Instant::now() + TERMINATE_AFTER);
        }
        if let Some(reason) = poll_child(pid, &mut reaped, &mut exit_code) {
            failure.get_or_insert(reason);
            terminate_process_group(pid);
            terminate_at = Some(Instant::now() + TERMINATE_AFTER);
        }
        if terminate_at.is_some_and(|deadline| Instant::now() >= deadline) {
            // SAFETY: pid remains the registered process-group leader until EOF and unregister.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            terminate_at = None;
        }
        // Descendants may still hold output and make askpass requests after the
        // immediate child exits. Keep cancellation/revocation ownership until EOF.
        if reaped && terminate_at.is_none() && reader_thread.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    cancellation.unregister(pid);
    match reader_thread.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            failure.get_or_insert(error);
        }
        Err(_) => {
            failure.get_or_insert_with(|| "output reader failed".into());
        }
    }
    if let Some(reason) = authorization.failure() {
        failure.get_or_insert(reason);
    }

    if let Err(error) = log.lock().unwrap_or_else(|e| e.into_inner()).flush() {
        failure.get_or_insert_with(|| format!("cannot flush job log: {error}"));
    }
    (exit_code, failure)
}

fn poll_child(pid: i32, reaped: &mut bool, exit_code: &mut Option<i32>) -> Option<String> {
    let mut status = 0;
    let waited = if *reaped {
        0
    } else {
        // SAFETY: status points to writable storage and pid is the spawned child.
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) }
    };
    if waited == pid {
        if libc::WIFEXITED(status) {
            *exit_code = Some(libc::WEXITSTATUS(status));
            *reaped = true;
        } else if libc::WIFSIGNALED(status) {
            *exit_code = Some(128 + libc::WTERMSIG(status));
            *reaped = true;
        } else if libc::WIFSTOPPED(status) {
            let signal = libc::WSTOPSIG(status);
            if signal == libc::SIGTTIN || signal == libc::SIGTTOU {
                return Some("job needs terminal input; interactive child input is unsupported".into());
            }
        }
    } else if waited == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINTR) {
            if error.raw_os_error() == Some(libc::ECHILD) {
                *reaped = true;
                *exit_code = Some(1);
            }
            return Some(format!("cannot wait for child: {error}"));
        }
    }
    None
}

fn terminate_process_group(pid: i32) {
    // SAFETY: pid is a process-group leader created by this runner.
    unsafe {
        libc::kill(-pid, libc::SIGCONT);
        libc::kill(-pid, libc::SIGTERM);
    }
}

fn read_output<R: Read>(
    mut stream: R,
    log: Arc<Mutex<File>>,
    job: String,
    events: mpsc::SyncSender<RunEvent>,
) -> Result<(), String> {
    let mut bytes = vec![0; OUTPUT_CHUNK];
    let mut display_pending = Vec::new();
    let mut display_connected = true;
    loop {
        let count = stream
            .read(&mut bytes)
            .map_err(|error| format!("cannot read child output: {error}"))?;
        if count == 0 {
            if !display_pending.is_empty() {
                let text = String::from_utf8_lossy(&display_pending).into_owned();
                if display_connected {
                    notify(&events, RunEvent::Output { job, text });
                }
            }
            return Ok(());
        }
        log.lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&bytes[..count])
            .map_err(|error| format!("cannot write job log: {error}"))?;
        display_pending.extend_from_slice(&bytes[..count]);
        match std::str::from_utf8(&display_pending) {
            Ok(text) => {
                if !text.is_empty() && display_connected {
                    display_connected = notify(
                        &events,
                        RunEvent::Output {
                            job: job.clone(),
                            text: text.to_owned(),
                        },
                    );
                }
                display_pending.clear();
            }
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text = String::from_utf8_lossy(&display_pending[..valid]).into_owned();
                    if display_connected {
                        display_connected = notify(&events, RunEvent::Output { job: job.clone(), text });
                    }
                    display_pending.drain(..valid);
                }
            }
            Err(_) => {
                let text = String::from_utf8_lossy(&display_pending).into_owned();
                if display_connected {
                    display_connected = notify(&events, RunEvent::Output { job: job.clone(), text });
                }
                display_pending.clear();
            }
        }
    }
}

fn resolved_command(job: &Job) -> Result<Vec<String>, String> {
    let mut command = job.command.clone();
    if command[0] == "sup-rust-update" {
        let executable =
            std::env::current_exe().map_err(|error| format!("cannot resolve current executable: {error}"))?;
        let helper = executable.parent().unwrap_or(Path::new(".")).join("sup-rust-update");
        if !helper.is_file() {
            return Err("sup-rust-update helper is missing; build or install all binaries".into());
        }
        command[0] = helper.to_string_lossy().into_owned();
    }
    Ok(command)
}

fn command_exists(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|path| path.join(command).is_file()))
}

fn failed(job: &Job, reason: &str, exit_code: Option<i32>, log_path: Option<PathBuf>, elapsed: Duration) -> JobResult {
    JobResult {
        job: job.clone(),
        status: JobStatus::Failed,
        exit_code,
        elapsed,
        log_path,
        reason: reason.into(),
    }
}

fn cancelled(job: &Job, reason: &str) -> JobResult {
    JobResult {
        job: job.clone(),
        status: JobStatus::Cancelled,
        exit_code: None,
        elapsed: Duration::ZERO,
        log_path: None,
        reason: reason.into(),
    }
}
