from __future__ import annotations

import argparse
import signal
import subprocess
import sys
import threading
from collections.abc import Callable, Iterable
from pathlib import Path

from rich.console import Console

from .display import LiveDashboard, render_dry_run, render_summary
from .jobs import Job, config_path, load_jobs_config, resolve_job_selection
from .runner import Runner


PasswordReader = Callable[[], str]
SudoValidator = Callable[[str], bool]
SudoTicketCheck = Callable[[], bool]
INTERRUPTED_EXIT_CODE = 130


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Update local development tools.")
    parser.add_argument(
        "--only", action="append", help="Run only selected jobs or groups."
    )
    parser.add_argument("--skip", action="append", help="Skip selected jobs or groups.")
    parser.add_argument(
        "--list", action="store_true", help="List configured jobs and exit."
    )
    parser.add_argument(
        "--config",
        type=Path,
        default=config_path(),
        help="Path to sup YAML config.",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="Show planned jobs only."
    )
    parser.add_argument(
        "--tail", type=int, default=40, help="Failure log lines to show."
    )
    parser.add_argument(
        "--log-retention-days",
        type=int,
        default=30,
        help="Delete run logs older than this many days.",
    )
    parser.add_argument(
        "--no-log-cleanup",
        action="store_true",
        help="Do not delete old run logs before running.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)
    try:
        jobs_config = load_jobs_config(args.config)
    except ValueError as exc:
        parser.error(str(exc))
    jobs = jobs_config.jobs
    console = Console()

    if args.list:
        for job in jobs:
            print(job.name)
        return 0

    try:
        selected = resolve_job_selection(
            jobs,
            aliases=jobs_config.aliases,
            only=args.only,
            skip=args.skip,
        )
    except ValueError as exc:
        parser.error(str(exc))

    if args.dry_run:
        render_dry_run(selected, console=console)
        return 0

    runner = Runner(
        home=Path.home(),
        retention_days=args.log_retention_days,
        log_cleanup=not args.no_log_cleanup,
    )
    previous_sigterm = signal.signal(signal.SIGTERM, raise_keyboard_interrupt)
    try:
        with LiveDashboard(selected, console=console) as dashboard:
            sudo_jobs = [job for job in selected if job.sudo_preflight]
            authenticator = SudoAuthenticator(sudo_jobs, dashboard=dashboard)
            sudo_ok = authenticator.authenticate()
            runner.sudo_preflight = (
                authenticator.authenticate if sudo_ok else lambda: False
            )
            results = runner.run(selected, on_update=dashboard.update)
    except KeyboardInterrupt:
        runner.stop()
        console.print("\n[bold yellow]Run interrupted; stopping active jobs.[/]")
        return INTERRUPTED_EXIT_CODE
    finally:
        signal.signal(signal.SIGTERM, previous_sigterm)
    render_summary(results, console=console, tail_count=args.tail)
    return Runner.exit_code_for(results)


def raise_keyboard_interrupt(_signum: int, _frame: object | None) -> None:
    raise KeyboardInterrupt


class SudoAuthenticator:
    def __init__(
        self,
        sudo_jobs: Iterable[Job],
        *,
        dashboard: LiveDashboard,
        sudo_ticket_available: SudoTicketCheck | None = None,
        password_reader: PasswordReader | None = None,
        validator: SudoValidator | None = None,
        max_attempts: int = 3,
    ) -> None:
        self.jobs = tuple(sudo_jobs)
        self.dashboard = dashboard
        self.sudo_ticket_available = sudo_ticket_available
        self.password_reader = password_reader
        self.validator = validator
        self.max_attempts = max_attempts
        self._lock = threading.Lock()

    def authenticate(self) -> bool:
        if not self.jobs:
            return True
        with self._lock:
            return authenticate_sudo_with_overlay(
                self.jobs,
                dashboard=self.dashboard,
                sudo_ticket_available=self.sudo_ticket_available,
                password_reader=self.password_reader,
                validator=self.validator,
                max_attempts=self.max_attempts,
            )


def authenticate_sudo_with_overlay(
    sudo_jobs: Iterable[Job],
    *,
    dashboard: LiveDashboard,
    sudo_ticket_available: SudoTicketCheck | None = None,
    password_reader: PasswordReader | None = None,
    validator: SudoValidator | None = None,
    max_attempts: int = 3,
) -> bool:
    jobs = list(sudo_jobs)
    if not jobs:
        return True

    sudo_ticket_available = sudo_ticket_available or has_sudo_ticket
    if sudo_ticket_available():
        return True

    password_reader = password_reader or (
        lambda: dashboard.console.input("", password=True)
    )
    validator = validator or validate_sudo_password
    error: str | None = None
    for _ in range(max_attempts):
        dashboard.show_auth_overlay(jobs, error=error)
        password = password_reader()
        if validator(password):
            dashboard.clear_auth_overlay()
            return True
        error = "Authentication failed. Try again."

    dashboard.clear_auth_overlay()
    return False


def has_sudo_ticket() -> bool:
    try:
        result = subprocess.run(
            ["sudo", "-n", "-v"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except FileNotFoundError:
        return False
    return result.returncode == 0


def validate_sudo_password(password: str) -> bool:
    try:
        result = subprocess.run(
            ["sudo", "-S", "-p", "", "-v"],
            input=f"{password}\n",
            text=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except FileNotFoundError:
        return False
    return result.returncode == 0


if __name__ == "__main__":
    raise SystemExit(main())
