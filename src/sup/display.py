from __future__ import annotations

import time
from collections import defaultdict, deque
from pathlib import Path
from typing import Callable, Iterable

from rich import box
from rich.console import Console, ConsoleOptions, Group
from rich.live import Live
from rich.panel import Panel
from rich.segment import Segment
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
    "muted": "#8f97c7",
}

OUTPUT_TAIL_LINES = 3
REFRESH_INTERVAL_SECONDS = 0.25
OVERLAY_MAX_WIDTH = 76

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
    def __init__(
        self,
        jobs: list[Job],
        *,
        console: Console,
        clock: Callable[[], float] | None = None,
    ) -> None:
        self.jobs = jobs
        self.console = console
        self.statuses = {job.name: "queued" for job in jobs}
        self.elapsed = {job.name: "-" for job in jobs}
        self.exit_codes = {job.name: "-" for job in jobs}
        self.output_lines: dict[str, deque[str]] = {
            job.name: deque(maxlen=OUTPUT_TAIL_LINES) for job in jobs
        }
        self.frame = 0
        self._live: Live | None = None
        self._clock = clock or time.monotonic
        self._last_refresh_at: float | None = None
        self._auth_overlay_jobs: tuple[Job, ...] = ()
        self._auth_overlay_error: str | None = None

    def __enter__(self) -> "LiveDashboard":
        self._live = Live(self, console=self.console, auto_refresh=False)
        self._live.__enter__()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        if self._live is not None:
            self._live.__exit__(exc_type, exc, tb)

    def update(
        self,
        name: str,
        status: str,
        result: JobResult | None = None,
        output: str | None = None,
    ) -> None:
        self.statuses[name] = status
        output_added = False
        had_output = bool(self.output_lines.get(name))
        if output is not None:
            line = output.strip()
            if line:
                self.output_lines.setdefault(
                    name,
                    deque(maxlen=OUTPUT_TAIL_LINES),
                ).append(line)
                output_added = True
        if result is not None:
            self.elapsed[name] = f"{result.elapsed:.1f}s"
            self.exit_codes[name] = (
                "-" if result.exit_code is None else str(result.exit_code)
            )
        self._refresh_live(force=output is None or (output_added and not had_output))

    def _refresh_live(self, *, force: bool) -> None:
        if self._live is None:
            return

        now = self._clock()
        refresh = (
            force
            or self._last_refresh_at is None
            or now - self._last_refresh_at >= REFRESH_INTERVAL_SECONDS
        )
        self._live.update(self, refresh=refresh)
        if refresh:
            self._last_refresh_at = now

    def show_auth_overlay(
        self,
        jobs: Iterable[Job],
        *,
        error: str | None = None,
    ) -> None:
        self._auth_overlay_jobs = tuple(jobs)
        self._auth_overlay_error = error
        self._refresh_live(force=True)

    def clear_auth_overlay(self) -> None:
        self._auth_overlay_jobs = ()
        self._auth_overlay_error = None
        self._refresh_live(force=True)

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
        has_auth_overlay = bool(self._auth_overlay_jobs)
        items = [
            dashboard_status_text(subtitle, dimmed=has_auth_overlay),
            mission_meter(self.statuses.values(), dimmed=has_auth_overlay),
            jobs_table(
                self.jobs,
                self.statuses,
                elapsed=self.elapsed,
                exit_codes=self.exit_codes,
                output_lines=self.output_lines,
                frame=frame,
                dimmed=has_auth_overlay,
            ),
        ]
        dashboard = themed_panel(
            Group(*items),
            title=None,
            border_style=TOKYONIGHT["purple"],
            panel_box=box.ROUNDED,
        )
        if not has_auth_overlay:
            return dashboard
        return OverlayRenderable(
            dashboard,
            auth_overlay_panel(
                self._auth_overlay_jobs,
                error=self._auth_overlay_error,
            ),
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


def themed_panel(
    renderable,
    *,
    title: str | None,
    border_style: str,
    panel_box=box.DOUBLE,
) -> Panel:
    panel_title = f"[bold {TOKYONIGHT['fg']}]{title}[/]" if title else None
    return Panel(
        renderable,
        title=panel_title,
        border_style=border_style,
        style=TOKYONIGHT["fg"],
        box=panel_box,
        padding=(1, 2),
    )


def header_text(title: str, subtitle: str) -> Text:
    text = Text()
    text.append(title, style=f"bold {TOKYONIGHT['blue']}")
    text.append("\n")
    text.append(subtitle, style=TOKYONIGHT["muted"])
    return text


class OverlayRenderable:
    def __init__(self, base, overlay) -> None:
        self.base = base
        self.overlay = overlay

    def __rich_console__(self, console: Console, options: ConsoleOptions):
        base_lines = console.render_lines(self.base, options, pad=True)
        base_width, base_height = Segment.get_shape(base_lines)
        if base_width <= 0 or base_height <= 0:
            return

        overlay_width_limit = max(1, min(OVERLAY_MAX_WIDTH, base_width - 4))
        overlay_options = options.update(width=overlay_width_limit)
        overlay_lines = console.render_lines(
            self.overlay,
            overlay_options,
            pad=False,
        )
        overlay_width, overlay_height = Segment.get_shape(overlay_lines)
        overlay_width = min(overlay_width, base_width)
        overlay_height = min(overlay_height, base_height)
        overlay_top = max(0, (base_height - overlay_height) // 2)
        overlay_left = max(0, (base_width - overlay_width) // 2)

        for line_number, base_line in enumerate(base_lines):
            line = Segment.adjust_line_length(base_line, base_width, pad=True)
            overlay_index = line_number - overlay_top
            if 0 <= overlay_index < overlay_height:
                overlay_line = Segment.adjust_line_length(
                    overlay_lines[overlay_index],
                    overlay_width,
                    pad=True,
                )
                line = overlay_segments(
                    line,
                    overlay_line,
                    start=overlay_left,
                    width=overlay_width,
                    total_width=base_width,
                )
            yield from line
            yield Segment.line()


def overlay_segments(
    line: list[Segment],
    overlay: list[Segment],
    *,
    start: int,
    width: int,
    total_width: int,
) -> list[Segment]:
    end = min(total_width, start + width)
    left = list(Segment.divide(line, [start]))[0] if start > 0 else []
    right = (
        list(Segment.divide(line, [end, total_width]))[1] if end < total_width else []
    )
    return left + overlay + right


def auth_overlay_panel(jobs: Iterable[Job], *, error: str | None = None) -> Panel:
    job_names = ", ".join(job.name for job in jobs)
    body = Text()
    body.append(
        "sudo authentication required",
        style=theme_style(TOKYONIGHT["fg"], bold=True),
    )
    body.append("\n")
    body.append(
        "Administrator credentials are needed before running:",
        style=theme_style(TOKYONIGHT["muted"]),
    )
    body.append("\n")
    body.append(job_names, style=theme_style(TOKYONIGHT["cyan"], bold=True))
    body.append("\n\n")
    body.append("Password: ", style=theme_style(TOKYONIGHT["cyan"], bold=True))
    body.append("hidden input active", style=theme_style(TOKYONIGHT["fg"]))
    if error:
        body.append("\n")
        body.append(error, style=theme_style(TOKYONIGHT["red"], bold=True))

    return Panel.fit(
        body,
        title=f"[bold {TOKYONIGHT['orange']}]sudo[/]",
        border_style=TOKYONIGHT["orange"],
        style=TOKYONIGHT["fg"],
        box=box.ROUNDED,
        padding=(1, 3),
    )


def dashboard_status_text(subtitle: str, *, dimmed: bool = False) -> Text:
    return Text(subtitle, style=theme_style(TOKYONIGHT["fg"], dimmed=dimmed))


def jobs_table(
    jobs: list[Job],
    statuses: dict[str, str],
    *,
    elapsed: dict[str, str] | None = None,
    exit_codes: dict[str, str] | None = None,
    output_lines: dict[str, deque[str]] | None = None,
    frame: int = 0,
    dimmed: bool = False,
) -> Table:
    elapsed = elapsed or {}
    exit_codes = exit_codes or {}
    output_lines = output_lines or {}
    table = Table(
        box=box.SIMPLE_HEAVY,
        expand=True,
        show_lines=False,
        header_style=theme_style(TOKYONIGHT["cyan"], bold=True, dimmed=dimmed),
        border_style=theme_style(TOKYONIGHT["muted"], dimmed=dimmed),
    )
    table.add_column("signal", no_wrap=True)
    table.add_column("stage", width=6, no_wrap=True)
    table.add_column("job / command", ratio=1, min_width=27, overflow="fold")
    table.add_column("trajectory", width=12, no_wrap=True)
    table.add_column("recent output", ratio=2, min_width=24, overflow="fold")
    table.add_column("time", justify="right", no_wrap=True)
    table.add_column("exit", justify="right", no_wrap=True)

    for job in jobs:
        status = statuses.get(job.name, "queued")
        table.add_row(
            status_label(status, dimmed=dimmed),
            phase_tag(job.phase, dimmed=dimmed),
            job_cell(job, dimmed=dimmed),
            progress_bar(status, width=12, frame=frame, dimmed=dimmed),
            output_cell(output_lines.get(job.name, ()), dimmed=dimmed),
            Text(
                elapsed.get(job.name, "-"),
                style=theme_style(TOKYONIGHT["fg"], dimmed=dimmed),
            ),
            Text(
                exit_codes.get(job.name, "-"),
                style=theme_style(TOKYONIGHT["fg"], dimmed=dimmed),
            ),
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


def status_label(status: str, *, dimmed: bool = False) -> Text:
    status_style = STATUS_STYLE.get(status, TOKYONIGHT["fg"])
    icon = STATUS_ICON.get(status, "•")
    return Text(
        f"{icon} {status}",
        style=theme_style(status_style, bold=True, dimmed=dimmed),
    )


def phase_tag(phase: str, *, dimmed: bool = False) -> Text:
    return Text(
        PHASE_TAG.get(phase, phase),
        style=theme_style(TOKYONIGHT["purple"], bold=True, dimmed=dimmed),
    )


def job_cell(job: Job, *, dimmed: bool = False) -> Text:
    cell = Text()
    cell.append(
        job.name,
        style=theme_style(TOKYONIGHT["fg"], bold=True, dimmed=dimmed),
    )
    cell.append("\n")
    cell.append(
        display_command(job.command),
        style=theme_style(TOKYONIGHT["muted"], dimmed=dimmed),
    )
    return cell


def output_cell(lines: Iterable[str], *, dimmed: bool = False) -> Text:
    recent = [line.strip() for line in lines if line.strip()]
    if not recent:
        return Text("-", style=theme_style(TOKYONIGHT["muted"], dimmed=dimmed))

    cell = Text()
    for index, line in enumerate(recent):
        if index:
            cell.append("\n")
        cell.append("› ", style=theme_style(TOKYONIGHT["cyan"], dimmed=dimmed))
        cell.append(line, style=theme_style(TOKYONIGHT["fg"], dimmed=dimmed))
    return cell


def display_command(command: Iterable[str]) -> str:
    parts = [display_command_part(part) for part in command]
    return " ".join(quote_for_display(part) for part in parts)


def display_command_part(part: str) -> str:
    if "=" in part and not part.startswith("-"):
        key, value = part.split("=", 1)
        if key.isidentifier() and key.upper() == key:
            return f"{key}={shorten_assignment_value(value)}"
    return shorten_path(part)


def shorten_assignment_value(value: str) -> str:
    if ":" not in value:
        return shorten_path(value)
    parts = [shorten_path(part) for part in value.split(":") if part]
    if len(parts) <= 3:
        return ":".join(parts)
    return ":".join([parts[0], parts[1], "…", parts[-1]])


def shorten_path(value: str) -> str:
    home = str(Path.home())
    if value == home:
        return "~"
    if value.startswith(f"{home}/"):
        return shorten_home_path(f"~/{value[len(home) + 1 :]}")
    return value


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


def progress_bar(
    status: str,
    width: int = 24,
    *,
    frame: int = 0,
    dimmed: bool = False,
) -> Text:
    bar_style = STATUS_STYLE.get(status, TOKYONIGHT["blue"])
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
    return Text(bar, style=theme_style(bar_style, bold=True, dimmed=dimmed))


def mission_meter(
    statuses: Iterable[str],
    width: int = 24,
    *,
    dimmed: bool = False,
) -> Text:
    values = list(statuses)
    counts = status_counts(values)
    total = len(values)
    complete = counts["succeeded"] + counts["skipped"] + counts["failed"]
    filled = 0 if total == 0 else round(width * complete / total)
    bar = "█" * filled + "░" * (width - filled)
    text = Text()
    text.append("mission burn ", style=theme_style(TOKYONIGHT["muted"], dimmed=dimmed))
    text.append(
        f"{complete}/{total} ",
        style=theme_style(TOKYONIGHT["cyan"], bold=True, dimmed=dimmed),
    )
    text.append(
        bar,
        style=theme_style(TOKYONIGHT["blue"], bold=True, dimmed=dimmed),
    )
    text.append(
        f"  ✅ {counts['succeeded']}  ⏭️ {counts['skipped']}  ❌ {counts['failed']}",
        style=theme_style(TOKYONIGHT["muted"], dimmed=dimmed),
    )
    return text


def theme_style(color: str, *, bold: bool = False, dimmed: bool = False) -> str:
    parts = []
    if dimmed:
        parts.append("dim")
    if bold:
        parts.append("bold")
    parts.append(color)
    return " ".join(parts)


def status_counts(statuses: Iterable[str]) -> dict[str, int]:
    counts = {status: 0 for status in STATUS_ICON}
    for status in statuses:
        counts[status] = counts.get(status, 0) + 1
    return counts
