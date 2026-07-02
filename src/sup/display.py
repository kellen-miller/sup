from __future__ import annotations

from collections import defaultdict
from pathlib import Path
from typing import Iterable

from rich import box
from rich.console import Console, Group
from rich.live import Live
from rich.panel import Panel
from rich.table import Table
from rich.text import Text

from .jobs import Job
from .logs import tail_lines
from .runner import JobResult


TOKYONIGHT = {
    "bg": "#1a1b26",
    "panel": "#24283b",
    "blue": "#7aa2f7",
    "cyan": "#7dcfff",
    "green": "#9ece6a",
    "orange": "#ff9e64",
    "purple": "#bb9af7",
    "red": "#f7768e",
    "yellow": "#e0af68",
    "fg": "#c0caf5",
    "muted": "#565f89",
}

STATUS_ICON = {
    "queued": "🌌",
    "running": "🛰️",
    "succeeded": "✅",
    "failed": "❌",
    "skipped": "⏭️",
}

STATUS_STYLE = {
    "queued": TOKYONIGHT["muted"],
    "running": TOKYONIGHT["cyan"],
    "succeeded": TOKYONIGHT["green"],
    "failed": TOKYONIGHT["red"],
    "skipped": TOKYONIGHT["yellow"],
}

PHASE_TAG = {
    "core": "[core]",
    "parallel": "[par]",
}


def render_dry_run(jobs: list[Job], *, console: Console) -> None:
    statuses = {job.name: "queued" for job in jobs}
    console.print(
        themed_panel(
            Group(
                header_text(
                    "🚀 ORBITAL PREFLIGHT",
                    (
                        f"TokyoNight palette {TOKYONIGHT['blue']} / {TOKYONIGHT['purple']} | "
                        f"{len(jobs)} jobs armed"
                    ),
                ),
                mission_meter(statuses.values()),
                jobs_table(jobs, statuses),
            ),
            title="SUP MISSION CONTROL",
            border_style=TOKYONIGHT["blue"],
        )
    )


class LiveDashboard:
    def __init__(self, jobs: list[Job], *, console: Console) -> None:
        self.jobs = jobs
        self.console = console
        self.statuses = {job.name: "queued" for job in jobs}
        self.elapsed = {job.name: "-" for job in jobs}
        self.exit_codes = {job.name: "-" for job in jobs}
        self.frame = 0
        self._live: Live | None = None

    def __enter__(self) -> "LiveDashboard":
        self._live = Live(self, console=self.console, refresh_per_second=8)
        self._live.__enter__()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        if self._live is not None:
            self._live.__exit__(exc_type, exc, tb)

    def update(self, name: str, status: str, result: JobResult | None = None) -> None:
        self.statuses[name] = status
        if result is not None:
            self.elapsed[name] = f"{result.elapsed:.1f}s"
            self.exit_codes[name] = (
                "-" if result.exit_code is None else str(result.exit_code)
            )
        if self._live is not None:
            self._live.update(self)

    def __rich__(self) -> Panel:
        return self.render(advance=True)

    def render(self, *, advance: bool = False) -> Panel:
        frame = self.frame
        if advance and any(status == "running" for status in self.statuses.values()):
            self.frame += 1
        counts = status_counts(self.statuses.values())
        subtitle = (
            f"queued {counts['queued']} | running {counts['running']} | "
            f"ok {counts['succeeded']} | skipped {counts['skipped']} | failed {counts['failed']}"
        )
        return themed_panel(
            Group(
                header_text("sup", subtitle),
                mission_meter(self.statuses.values()),
                jobs_table(
                    self.jobs,
                    self.statuses,
                    elapsed=self.elapsed,
                    exit_codes=self.exit_codes,
                    frame=frame,
                ),
            ),
            title="sup",
            border_style=TOKYONIGHT["purple"],
        )


def render_summary(
    results: Iterable[JobResult], *, console: Console, tail_count: int
) -> None:
    grouped: dict[str, list[JobResult]] = defaultdict(list)
    for result in results:
        grouped[result.status].append(result)

    console.print(
        themed_panel(
            Group(
                header_text(
                    "🪐 MISSION REPORT",
                    "final telemetry grouped by flight outcome",
                ),
                summary_table(grouped),
            ),
            title="SUP DEBRIEF",
            border_style=TOKYONIGHT["green"]
            if not grouped.get("failed")
            else TOKYONIGHT["red"],
        )
    )

    for result in grouped.get("failed", []):
        lines = (
            tail_lines(result.log_path, tail_count)
            if isinstance(result.log_path, Path)
            else []
        )
        failure_body = (
            "\n".join(lines) if lines else result.reason or "No log output captured."
        )
        console.print(
            Panel(
                failure_body,
                title=f"❌ {result.job.name} failure tail",
                border_style=TOKYONIGHT["red"],
                style=TOKYONIGHT["fg"],
                box=box.ROUNDED,
            )
        )


def themed_panel(renderable, *, title: str, border_style: str) -> Panel:
    return Panel(
        renderable,
        title=f"[bold {TOKYONIGHT['fg']}]{title}[/]",
        border_style=border_style,
        style=TOKYONIGHT["fg"],
        box=box.DOUBLE,
        padding=(1, 2),
    )


def header_text(title: str, subtitle: str) -> Text:
    text = Text()
    text.append(title, style=f"bold {TOKYONIGHT['blue']}")
    text.append("\n")
    text.append(subtitle, style=TOKYONIGHT["muted"])
    return text


def jobs_table(
    jobs: list[Job],
    statuses: dict[str, str],
    *,
    elapsed: dict[str, str] | None = None,
    exit_codes: dict[str, str] | None = None,
    frame: int = 0,
) -> Table:
    elapsed = elapsed or {}
    exit_codes = exit_codes or {}
    table = Table(
        box=box.SIMPLE_HEAVY,
        expand=True,
        show_lines=False,
        header_style=f"bold {TOKYONIGHT['cyan']}",
        border_style=TOKYONIGHT["muted"],
    )
    table.add_column("signal", no_wrap=True)
    table.add_column("stage", width=6, no_wrap=True)
    table.add_column("job / command", ratio=1, min_width=27, overflow="fold")
    table.add_column("trajectory", width=12, no_wrap=True)
    table.add_column("time", justify="right", no_wrap=True)
    table.add_column("exit", justify="right", no_wrap=True)

    for job in jobs:
        status = statuses.get(job.name, "queued")
        table.add_row(
            status_label(status),
            phase_tag(job.phase),
            job_cell(job),
            progress_bar(status, width=12, frame=frame),
            elapsed.get(job.name, "-"),
            exit_codes.get(job.name, "-"),
        )
    return table


def summary_table(grouped: dict[str, list[JobResult]]) -> Table:
    table = Table(
        box=box.SIMPLE_HEAVY,
        expand=True,
        header_style=f"bold {TOKYONIGHT['cyan']}",
        border_style=TOKYONIGHT["muted"],
    )
    table.add_column("signal", no_wrap=True)
    table.add_column("count", justify="right", no_wrap=True)
    table.add_column("jobs")
    for status in ("succeeded", "skipped", "failed"):
        results = grouped.get(status, [])
        names = ", ".join(result.job.name for result in results) or "-"
        table.add_row(status_label(status), str(len(results)), names)
    return table


def status_label(status: str) -> Text:
    style = STATUS_STYLE.get(status, TOKYONIGHT["fg"])
    icon = STATUS_ICON.get(status, "•")
    return Text(f"{icon} {status}", style=f"bold {style}")


def phase_tag(phase: str) -> Text:
    return Text(PHASE_TAG.get(phase, phase), style=f"bold {TOKYONIGHT['purple']}")


def job_cell(job: Job) -> Text:
    cell = Text()
    cell.append(job.name, style=f"bold {TOKYONIGHT['fg']}")
    cell.append("\n")
    cell.append(display_command(job.command), style=TOKYONIGHT["muted"])
    return cell


def display_command(command: Iterable[str]) -> str:
    home = str(Path.home())
    parts = []
    for part in command:
        if part == home:
            parts.append("~")
        elif part.startswith(f"{home}/"):
            parts.append(shorten_home_path(f"~/{part[len(home) + 1 :]}"))
        else:
            parts.append(part)
    return " ".join(quote_for_display(part) for part in parts)


def shorten_home_path(value: str) -> str:
    if len(value) <= 18 or "/" not in value:
        return value
    parts = value.split("/")
    if len(parts) <= 3:
        return value
    return "/".join([parts[0], "…", parts[-1]])


def quote_for_display(value: str) -> str:
    if any(char.isspace() for char in value):
        return repr(value)
    return value


def progress_bar(status: str, width: int = 24, *, frame: int = 0) -> Text:
    style = STATUS_STYLE.get(status, TOKYONIGHT["blue"])
    if status == "running":
        pattern = "░▒▓█▓▒"
        offset = frame % len(pattern)
        animated = pattern[offset:] + pattern[:offset]
        bar = (animated * ((width // len(animated)) + 1))[:width]
    elif status == "succeeded":
        bar = "█" * width
    elif status == "failed":
        bar = "█" * max(1, width // 3) + "░" * (width - max(1, width // 3))
    elif status == "skipped":
        bar = "━" * width
    else:
        bar = "█" + "░" * (width - 1)
    return Text(bar, style=f"bold {style}")


def mission_meter(statuses: Iterable[str], width: int = 24) -> Text:
    values = list(statuses)
    counts = status_counts(values)
    total = len(values)
    complete = counts["succeeded"] + counts["skipped"] + counts["failed"]
    filled = 0 if total == 0 else round(width * complete / total)
    bar = "█" * filled + "░" * (width - filled)
    text = Text()
    text.append("mission burn ", style=TOKYONIGHT["muted"])
    text.append(f"{complete}/{total} ", style=f"bold {TOKYONIGHT['cyan']}")
    text.append(bar, style=f"bold {TOKYONIGHT['blue']}")
    text.append(
        f"  ✅ {counts['succeeded']}  ⏭️ {counts['skipped']}  ❌ {counts['failed']}",
        style=TOKYONIGHT["muted"],
    )
    return text


def status_counts(statuses: Iterable[str]) -> dict[str, int]:
    counts = {status: 0 for status in STATUS_ICON}
    for status in statuses:
        counts[status] = counts.get(status, 0) + 1
    return counts
