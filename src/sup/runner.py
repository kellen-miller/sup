from __future__ import annotations

import os
import shutil
import subprocess
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


CommandRunner = Callable[[Job], CommandResult]
CommandExists = Callable[[str], bool]
PathExists = Callable[[Path], bool]
StatusCallback = Callable[[str, str, JobResult | None], None]


class Runner:
    def __init__(
        self,
        *,
        home: Path,
        command_runner: CommandRunner | None = None,
        command_exists: CommandExists | None = None,
        path_exists: PathExists | None = None,
        sudo_preflight: Callable[[], bool] | None = None,
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
        for job in [job for job in jobs if job.phase == "core"]:
            results.append(self._run_one(job, run_dir, on_update=on_update))

        parallel_jobs = [job for job in jobs if job.phase == "parallel"]
        with ThreadPoolExecutor(max_workers=max(1, len(parallel_jobs))) as executor:
            futures = {
                executor.submit(self._run_one, job, run_dir, on_update): job
                for job in parallel_jobs
            }
            parallel_results = [future.result() for future in as_completed(futures)]
        results.extend(
            sorted(
                parallel_results,
                key=lambda result: [job.name for job in parallel_jobs].index(
                    result.job.name
                ),
            )
        )
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

    def _run_one(
        self,
        job: Job,
        run_dir: Path,
        on_update: StatusCallback | None = None,
    ) -> JobResult:
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

        if job.sudo_preflight and not self.sudo_preflight():
            reason = "sudo authentication unavailable"
            log_path.write_text(reason + "\n", encoding="utf-8")
            status = "skipped" if job.optional else "failed"
            result = JobResult(
                job, status, None, time.monotonic() - started, log_path, reason
            )
            self._emit(on_update, job.name, status, result)
            return result

        self._emit(on_update, job.name, "running", None)
        command_result = (
            self.command_runner(job)
            if self.command_runner is not None
            else self._run_subprocess(job, log_path)
        )
        status = "succeeded" if command_result.exit_code == 0 else "failed"
        result = JobResult(
            job,
            status,
            command_result.exit_code,
            time.monotonic() - started,
            log_path,
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

    def _run_subprocess(self, job: Job, log_path: Path) -> CommandResult:
        with log_path.open("w", encoding="utf-8", errors="replace") as log:
            process = subprocess.Popen(
                job.command,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            assert process.stdout is not None
            for line in process.stdout:
                log.write(line)
                log.flush()
            return CommandResult(process.wait())

    def _sudo_preflight(self) -> bool:
        return subprocess.run(["sudo", "-n", "-v"], check=False).returncode == 0

    @staticmethod
    def _emit(
        callback: StatusCallback | None,
        name: str,
        status: str,
        result: JobResult | None,
    ) -> None:
        if callback is not None:
            callback(name, status, result)
