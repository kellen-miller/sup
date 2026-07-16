from __future__ import annotations

import os
import shutil
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Mapping

from .jobs import Job
from .logs import cleanup_old_runs, create_run_dir


@dataclass(frozen=True)
class CommandResult:
    exit_code: int


@dataclass
class JobResult:
    job: Job
    status: str
    exit_code: int | None
    elapsed: float
    log_path: Path | None = None
    reason: str = ""


@dataclass(frozen=True)
class _PreparedJob:
    job: Job
    log_path: Path


CommandRunner = Callable[[Job], CommandResult]
CommandExists = Callable[[str], bool]
PathExists = Callable[[Path], bool]
StatusCallback = Callable[..., None]
SudoPreflight = Callable[[Job], bool]
STOP_TIMEOUT_SECONDS = 1.0


class Runner:
    def __init__(
        self,
        *,
        home: Path,
        command_runner: CommandRunner | None = None,
        command_exists: CommandExists | None = None,
        path_exists: PathExists | None = None,
        sudo_preflight: SudoPreflight | None = None,
        retention_days: int = 30,
        log_cleanup: bool = True,
        env: Mapping[str, str] | None = None,
    ) -> None:
        self.home = home
        self.command_runner = command_runner
        self.command_exists = command_exists or (
            lambda name: shutil.which(name) is not None
        )
        self.path_exists = path_exists or (lambda path: path.exists())
        self.sudo_preflight = sudo_preflight or self._sudo_preflight
        self.retention_days = retention_days
        self.log_cleanup = log_cleanup
        self.env = os.environ if env is None else env
        self._stop_requested = threading.Event()
        self._processes: set[subprocess.Popen[str]] = set()
        self._processes_lock = threading.Lock()

    def stop(self) -> None:
        self._stop_requested.set()
        with self._processes_lock:
            processes = list(self._processes)
        for process in processes:
            self._terminate_process(process)

    def run(
        self,
        jobs: list[Job],
        *,
        dry_run: bool = False,
        on_update: StatusCallback | None = None,
    ) -> list[JobResult]:
        if dry_run:
            results = [self._dry_run_result(job) for job in jobs]
            for result in results:
                self._emit(on_update, result.job.name, result.status, result)
            return results

        run_dir = create_run_dir(self.home)
        run_dir.mkdir(parents=True, exist_ok=True)
        if self.log_cleanup:
            cleanup_old_runs(
                self.home,
                run_dir,
                retention_days=self.retention_days,
            )

        results: list[JobResult] = []
        try:
            for job in [job for job in jobs if job.phase == "core"]:
                prepared = self._prepare(job, run_dir, on_update=on_update)
                results.append(
                    prepared
                    if isinstance(prepared, JobResult)
                    else self._execute(prepared, on_update=on_update)
                )

            parallel_jobs = [job for job in jobs if job.phase == "parallel"]
            parallel_results: dict[int, JobResult] = {}
            prepared_jobs: list[tuple[int, _PreparedJob]] = []
            for index, job in enumerate(parallel_jobs):
                prepared = self._prepare(job, run_dir, on_update=on_update)
                if isinstance(prepared, JobResult):
                    parallel_results[index] = prepared
                else:
                    prepared_jobs.append((index, prepared))

            if prepared_jobs:
                executor = ThreadPoolExecutor(max_workers=len(prepared_jobs))
                wait_for_executor = True
                futures = {}
                try:
                    futures = {
                        executor.submit(self._execute, prepared, on_update): index
                        for index, prepared in prepared_jobs
                    }
                    for future in as_completed(futures):
                        parallel_results[futures[future]] = future.result()
                except KeyboardInterrupt:
                    wait_for_executor = False
                    for future in futures:
                        future.cancel()
                    raise
                finally:
                    executor.shutdown(
                        wait=wait_for_executor,
                        cancel_futures=not wait_for_executor,
                    )
        except KeyboardInterrupt:
            self.stop()
            raise
        results.extend(parallel_results[index] for index in range(len(parallel_jobs)))
        return results

    @staticmethod
    def exit_code_for(results: Iterable[JobResult]) -> int:
        return 1 if any(result.status == "failed" for result in results) else 0

    def _dry_run_result(self, job: Job) -> JobResult:
        return JobResult(
            job=job,
            status="succeeded",
            exit_code=0,
            elapsed=0.0,
            reason="dry run",
        )

    def _prepare(
        self,
        job: Job,
        run_dir: Path,
        on_update: StatusCallback | None = None,
    ) -> _PreparedJob | JobResult:
        started = time.monotonic()
        missing = self._missing_requirements(job)
        log_path = run_dir / job.log_name
        if missing:
            status = "skipped" if job.optional else "failed"
            reason = (
                f"missing optional requirement: {', '.join(missing)}"
                if job.optional
                else f"missing required requirement: {', '.join(missing)}"
            )
            log_path.write_text(reason + "\n", encoding="utf-8")
            result = JobResult(
                job, status, None, time.monotonic() - started, log_path, reason
            )
            self._emit(on_update, job.name, status, result)
            return result

        if job.sudo_preflight and not self.sudo_preflight(job):
            reason = "sudo authentication unavailable"
            log_path.write_text(reason + "\n", encoding="utf-8")
            status = "skipped" if job.optional else "failed"
            result = JobResult(
                job, status, None, time.monotonic() - started, log_path, reason
            )
            self._emit(on_update, job.name, status, result)
            return result

        return _PreparedJob(job=job, log_path=log_path)

    def _execute(
        self,
        prepared: _PreparedJob,
        on_update: StatusCallback | None = None,
    ) -> JobResult:
        job = prepared.job
        started = time.monotonic()
        self._emit(on_update, job.name, "running", None)
        command_result = (
            self.command_runner(job)
            if self.command_runner is not None
            else self._run_subprocess(job, prepared.log_path, on_update=on_update)
        )
        status = "succeeded" if command_result.exit_code == 0 else "failed"
        result = JobResult(
            job,
            status,
            command_result.exit_code,
            time.monotonic() - started,
            prepared.log_path,
        )
        self._emit(on_update, job.name, status, result)
        return result

    def _missing_requirements(self, job: Job) -> list[str]:
        missing = [f"env:{name}" for name in job.required_env if not self.env.get(name)]
        if missing:
            return missing

        missing.extend(
            command
            for command in job.required_commands
            if not self.command_exists(command)
        )
        missing.extend(
            str(path) for path in job.required_paths if not self.path_exists(path)
        )
        return missing

    def _run_subprocess(
        self,
        job: Job,
        log_path: Path,
        *,
        on_update: StatusCallback | None = None,
    ) -> CommandResult:
        with log_path.open("w", encoding="utf-8", errors="replace") as log:
            with subprocess.Popen(
                job.command,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            ) as process:
                self._track_process(process)
                try:
                    assert process.stdout is not None
                    if self._stop_requested.is_set():
                        self._terminate_process(process)
                    for line in process.stdout:
                        log.write(line)
                        log.flush()
                        output = line.strip()
                        if output:
                            self._emit(
                                on_update,
                                job.name,
                                "running",
                                None,
                                output=output,
                            )
                    return CommandResult(process.wait())
                except KeyboardInterrupt:
                    self._terminate_process(process)
                    raise
                finally:
                    self._untrack_process(process)

    def _sudo_preflight(self, _job: Job | None = None) -> bool:
        return (
            subprocess.run(
                ["sudo", "-p", "sup sudo password: ", "-v"],
                check=False,
            ).returncode
            == 0
        )

    @staticmethod
    def _emit(
        callback: StatusCallback | None,
        name: str,
        status: str,
        result: JobResult | None,
        *,
        output: str | None = None,
    ) -> None:
        if callback is not None:
            if output is None:
                callback(name, status, result)
            else:
                callback(name, status, result, output)

    def _track_process(self, process: subprocess.Popen[str]) -> None:
        with self._processes_lock:
            self._processes.add(process)

    def _untrack_process(self, process: subprocess.Popen[str]) -> None:
        with self._processes_lock:
            self._processes.discard(process)

    @staticmethod
    def _terminate_process(process: subprocess.Popen[str]) -> None:
        if process.poll() is not None:
            return
        try:
            process.terminate()
            process.wait(timeout=STOP_TIMEOUT_SECONDS)
        except ProcessLookupError:
            return
        except subprocess.TimeoutExpired:
            try:
                process.kill()
                process.wait(timeout=STOP_TIMEOUT_SECONDS)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                return
