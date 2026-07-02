from __future__ import annotations

import argparse
import sys
from pathlib import Path

from rich.console import Console

from .display import LiveDashboard, render_dry_run, render_summary
from .jobs import config_path, load_jobs_config, resolve_job_selection
from .runner import Runner


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
        help="Path to jobs YAML config.",
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
    sudo_ok: bool | None = None
    if any(job.sudo_preflight for job in selected):
        sudo_ok = runner.sudo_preflight()
        runner.sudo_preflight = lambda: bool(sudo_ok)

    with LiveDashboard(selected, console=console) as dashboard:
        results = runner.run(selected, on_update=dashboard.update)
    render_summary(results, console=console, tail_count=args.tail)
    return Runner.exit_code_for(results)


if __name__ == "__main__":
    raise SystemExit(main())
