#![cfg(test)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use sup::config::{AuthMode, Job};
use sup::runner::{self, Cancellation, JobStatus, RunEvent, RunOptions};

fn job(name: &str, phase: &str, command: &[&str]) -> Job {
    Job {
        name: name.into(),
        label: name.into(),
        phase: phase.into(),
        command: command.iter().map(|value| (*value).into()).collect(),
        required_commands: Vec::new(),
        required_paths: Vec::new(),
        required_env: Vec::new(),
        optional: false,
        log_name: format!("{name}.log"),
        auth: AuthMode::None,
    }
}

fn options(home: PathBuf) -> RunOptions {
    RunOptions {
        home,
        retention_days: 30,
        cleanup_logs: true,
    }
}

#[test]
fn core_jobs_continue_after_failure_and_results_keep_config_order() {
    let home = tempfile::tempdir().unwrap();
    let jobs = vec![
        job("bad", "core", &["/bin/sh", "-c", "exit 7"]),
        job("good", "core", &["/bin/sh", "-c", "printf 'out'; printf 'err' >&2"]),
        job("later", "parallel", &["/bin/sh", "-c", "printf later"]),
    ];
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let results = runner::run(
        &jobs,
        options(home.path().into()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let observed = events.join().unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, JobStatus::Failed);
    assert_eq!(results[0].exit_code, Some(7));
    assert_eq!(results[1].status, JobStatus::Succeeded);
    assert_eq!(results[2].job.name, "later");
    let output = std::fs::read_to_string(results[1].log_path.as_ref().unwrap()).unwrap();
    assert!(output.contains("out"));
    assert!(output.contains("err"));
    assert!(
        observed
            .iter()
            .any(|event| matches!(event, RunEvent::Output { job, .. } if job == "good"))
    );
}

#[test]
fn missing_requirements_skip_optional_and_fail_required() {
    let home = tempfile::tempdir().unwrap();
    let mut optional = job("optional", "core", &["/bin/true"]);
    optional.optional = true;
    optional
        .required_commands
        .push("sup-command-that-does-not-exist".into());
    let mut required = job("required", "core", &["/bin/true"]);
    required.required_paths.push(home.path().join("absent"));
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let results = runner::run(
        &[optional, required],
        options(home.path().into()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let _ = events.join().unwrap();
    assert_eq!(results[0].status, JobStatus::Skipped);
    assert_eq!(results[1].status, JobStatus::Failed);
}

#[test]
fn large_newline_free_output_is_fully_logged() {
    let home = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let results = runner::run(
        &[job(
            "large",
            "core",
            &["/bin/sh", "-c", "head -c 524288 /dev/zero | tr '\\000' x"],
        )],
        options(home.path().into()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let _ = events.join().unwrap();
    assert_eq!(
        std::fs::metadata(results[0].log_path.as_ref().unwrap()).unwrap().len(),
        524_288
    );
}

#[test]
fn disconnected_event_receiver_does_not_truncate_job_logs() {
    let home = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    drop(rx);
    let output_size = 128 * 1024;
    let result = runner::run(
        &[job(
            "disconnected",
            "core",
            &["/bin/sh", "-c", "head -c 131072 /dev/zero | tr '\\000' x"],
        )],
        options(home.path().into()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(result[0].log_path.as_ref().unwrap()).unwrap().len(),
        output_size
    );
}

#[test]
fn unicode_split_across_pipe_reads_is_rendered_without_replacement() {
    let home = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let result = runner::run(
        &[job(
            "unicode",
            "core",
            &[
                "/bin/sh",
                "-c",
                "printf '\\342'; sleep 0.03; printf '\\202'; sleep 0.03; printf '\\254'",
            ],
        )],
        options(home.path().into()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let observed = events.join().unwrap();
    let output = observed
        .iter()
        .filter_map(|event| match event {
            RunEvent::Output { job, text } if job == "unicode" => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(output, "€");
    assert_eq!(
        std::fs::read(result[0].log_path.as_ref().unwrap()).unwrap(),
        "€".as_bytes()
    );
}

#[test]
fn cancellation_terminates_the_process_group_promptly() {
    let home = tempfile::tempdir().unwrap();
    let descendant_pid = home.path().join("descendant-pid");
    let home_path = home.path().to_path_buf();
    let shell = format!("sleep 30 & echo $! > {}; wait", descendant_pid.display());
    let cancellation = Cancellation::default();
    let worker_cancel = cancellation.clone();
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let worker = thread::spawn(move || {
        runner::run(
            &[job("tree", "core", &["/bin/sh", "-c", shell.as_str()])],
            options(home_path),
            tx,
            mpsc::sync_channel(8).0,
            worker_cancel,
        )
        .unwrap()
    });
    let started = Instant::now();
    let launch_deadline = Instant::now() + Duration::from_secs(2);
    while std::fs::read_to_string(&descendant_pid).map_or(true, |value| value.trim().is_empty())
        && Instant::now() < launch_deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        std::fs::read_to_string(&descendant_pid).is_ok_and(|value| !value.trim().is_empty()),
        "fixture descendant did not start"
    );
    cancellation.cancel();
    let results = worker.join().unwrap();
    let _ = events.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(results[0].status, JobStatus::Cancelled);
    let pid: i32 = std::fs::read_to_string(descendant_pid).unwrap().trim().parse().unwrap();
    let stopped = Instant::now();
    loop {
        // SAFETY: signal zero probes this recorded process ID without sending a signal or using pointers.
        if unsafe { libc::kill(pid, 0) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            break;
        }
        assert!(stopped.elapsed() < Duration::from_secs(3), "descendant still exists");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn parallel_jobs_overlap() {
    let home = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let started = Instant::now();
    let results = runner::run(
        &[
            job("first", "parallel", &["/bin/sh", "-c", "sleep 0.35"]),
            job("second", "parallel", &["/bin/sh", "-c", "sleep 0.35"]),
        ],
        options(home.path().into()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let _ = events.join().unwrap();
    assert!(started.elapsed() < Duration::from_millis(650));
    assert_eq!(results[0].status, JobStatus::Succeeded);
    assert_eq!(results[1].status, JobStatus::Succeeded);
}

#[test]
fn core_runs_before_parallel_and_results_keep_input_order() {
    let home = tempfile::tempdir().unwrap();
    let jobs = vec![
        job("parallel", "parallel", &["/bin/true"]),
        job("core", "core", &["/bin/true"]),
    ];
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let results = runner::run(
        &jobs,
        options(home.path().into()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let events = events.join().unwrap();
    assert_eq!(results[0].job.name, "parallel");
    assert_eq!(results[1].job.name, "core");
    let started: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::Started(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(started, ["core", "parallel"]);
}

#[test]
fn logs_are_private_and_cleanup_does_not_follow_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let home = tempfile::tempdir().unwrap();
    let runs = home.path().join(".cache/sup/runs");
    std::fs::create_dir_all(&runs).unwrap();
    let target = home.path().join("outside");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep"), "safe").unwrap();
    let old = runs.join("old");
    std::fs::create_dir(&old).unwrap();
    std::fs::write(old.join("remove"), "old").unwrap();
    symlink(&target, runs.join("linked")).unwrap();
    let (tx, rx) = mpsc::sync_channel(128);
    let events = thread::spawn(move || rx.into_iter().collect::<Vec<RunEvent>>());
    let mut retention = options(home.path().into());
    retention.retention_days = 0;
    let results = runner::run(
        &[job("private", "core", &["/bin/true"])],
        retention,
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let _ = events.join().unwrap();
    let log = results[0].log_path.as_ref().unwrap();
    assert_eq!(std::fs::metadata(log).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        std::fs::metadata(log.parent().unwrap()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(target.join("keep").exists());
    assert!(!old.exists());
}

#[test]
fn every_result_emits_one_finished_event_including_spawn_failures() {
    let home = tempfile::tempdir().unwrap();
    let jobs = vec![
        job("core-failure", "core", &["/sup-fixture-command-does-not-exist"]),
        job("parallel-failure", "parallel", &["/sup-fixture-command-does-not-exist"]),
        job("success", "parallel", &["/bin/sh", "-c", "exit 0"]),
    ];
    let (tx, rx) = mpsc::sync_channel(128);
    let results = runner::run(
        &jobs,
        options(home.path().to_path_buf()),
        tx,
        mpsc::sync_channel(8).0,
        Cancellation::default(),
    )
    .unwrap();
    let finished: Vec<_> = rx
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::Finished(result) => Some(*result),
            _ => None,
        })
        .collect();
    assert_eq!(finished.len(), jobs.len(), "{results:?}");
    for result in results {
        let matching: Vec<_> = finished
            .iter()
            .filter(|event| event.job.name == result.job.name)
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].status, result.status);
    }
}
