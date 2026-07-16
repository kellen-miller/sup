from __future__ import annotations

import getpass
import time
from collections import defaultdict, deque
from pathlib import Path
from typing import Callable, Iterable

from rich import box
from rich.cells import cell_len
from rich.console import Console, ConsoleOptions, Group
from rich.control import Control
from rich.live import Live
from rich.panel import Panel
from rich.rule import Rule
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

PHASE_LABEL = {
    "core": "core phase",
    "parallel": "parallel phase",
}

STATUS_SORT = {
    "running": 0,
    "failed": 1,
    "queued": 2,
    "skipped": 3,
    "succeeded": 4,
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
                planning_jobs_view(jobs, statuses),
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
        self.recent_output: deque[tuple[str, str]] = deque(maxlen=OUTPUT_TAIL_LINES)
        self.frame = 0
        self._live: Live | None = None
        self._clock = clock or time.monotonic
        self._last_refresh_at: float | None = None
        self._auth_overlay_jobs: tuple[Job, ...] = ()
        self._auth_overlay_error: str | None = None

    def __enter__(self) -> "LiveDashboard":
        self._live = Live(
            self,
            console=self.console,
            screen=True,
            auto_refresh=False,
            vertical_overflow="crop",
        )
        try:
            self._live.__enter__()
        except BaseException:
            alt_screen_known = bool(getattr(self._live, "_alt_screen", False))
            stop_failed = False
            try:
                self._live.stop()
            except BaseException:
                stop_failed = True
            self.console.show_cursor(True)
            if not alt_screen_known or stop_failed:
                self.console.set_alt_screen(False)
            self._live = None
            raise
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
        had_output = bool(self.recent_output)
        if output is not None:
            line = output.strip()
            if line:
                self.recent_output.append((name, line))
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

    def read_password(
        self,
        jobs: Iterable[Job],
        *,
        error: str | None = None,
        reader: Callable[[], str] | None = None,
    ) -> str:
        self._auth_overlay_jobs = tuple(jobs)
        self._auth_overlay_error = error
        self._refresh_live(force=True)
        try:
            column, row = self._password_cursor_position()
            self.console.control(
                Control.show_cursor(True),
                Control.move_to(column, row),
            )
            self.console.file.flush()
            return (reader or (lambda: getpass.getpass("")))()
        finally:
            self.console.control(Control.show_cursor(False))
            self._auth_overlay_jobs = ()
            self._auth_overlay_error = None
            self._refresh_live(force=True)

    def _password_cursor_position(self) -> tuple[int, int]:
        size = self.console.size
        options = self.console.options.update(width=size.width, height=size.height)
        renderable = self.render()
        if not isinstance(renderable, OverlayRenderable):
            raise RuntimeError("password modal is not active")
        return renderable.password_cursor_position(self.console, options)

    def __rich__(self):
        return self.render(advance=True)

    def render(self, *, advance: bool = False):
        frame = self.frame
        if advance and any(status == "running" for status in self.statuses.values()):
            self.frame += 1
        has_auth_overlay = bool(self._auth_overlay_jobs)
        dashboard = focused_dashboard(
            self.jobs,
            self.statuses,
            elapsed=self.elapsed,
            exit_codes=self.exit_codes,
            recent_output=self.recent_output,
            frame=frame,
            height=self.console.size.height,
            dimmed=has_auth_overlay,
        )
        if not has_auth_overlay:
            return dashboard
        return OverlayRenderable(
            dashboard,
            auth_overlay_panel(
                self._auth_overlay_jobs,
                error=self._auth_overlay_error,
            ),
            height=self.console.size.height,
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
    def __init__(self, base, overlay, *, height: int) -> None:
        self.base = base
        self.overlay = overlay
        self.height = height

    def __rich_console__(self, console: Console, options: ConsoleOptions):
        (
            base_lines,
            base_width,
            base_height,
            overlay_lines,
            overlay_width,
            overlay_height,
            overlay_top,
            overlay_left,
        ) = self._layout(console, options)
        if base_width <= 0 or base_height <= 0:
            return

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

    def password_cursor_position(
        self, console: Console, options: ConsoleOptions
    ) -> tuple[int, int]:
        (
            _base_lines,
            _base_width,
            _base_height,
            overlay_lines,
            _overlay_width,
            overlay_height,
            overlay_top,
            overlay_left,
        ) = self._layout(console, options)
        for row, line in enumerate(overlay_lines[:overlay_height]):
            text = "".join(segment.text for segment in line if not segment.control)
            prompt_start = text.find("Password:")
            if prompt_start >= 0:
                prompt_end = prompt_start + len("Password:")
                return overlay_left + cell_len(text[:prompt_end]), overlay_top + row
        raise RuntimeError("password prompt is not visible in the terminal viewport")

    def _layout(self, console: Console, options: ConsoleOptions):
        base_lines = console.render_lines(self.base, options, pad=True)
        base_width = options.max_width
        base_height = max(1, self.height)
        base_lines = base_lines[:base_height]
        base_lines.extend(
            [[Segment(" " * base_width)] for _ in range(base_height - len(base_lines))]
        )
        if base_width <= 0 or base_height <= 0:
            return base_lines, base_width, base_height, [], 0, 0, 0, 0

        overlay_width_limit = max(1, min(OVERLAY_MAX_WIDTH, base_width - 4))
        overlay_options = options.update(width=overlay_width_limit, height=None)
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
        return (
            base_lines,
            base_width,
            base_height,
            overlay_lines,
            overlay_width,
            overlay_height,
            overlay_top,
            overlay_left,
        )


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
    body.append("Password:", style=theme_style(TOKYONIGHT["cyan"], bold=True))
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


def focused_dashboard(
    jobs: list[Job],
    statuses: dict[str, str],
    *,
    elapsed: dict[str, str],
    exit_codes: dict[str, str],
    recent_output: Iterable[tuple[str, str]],
    frame: int,
    height: int,
    dimmed: bool,
) -> Group:
    counts = status_counts(statuses.values())
    subtitle = (
        f"queued {counts['queued']} | running {counts['running']} | "
        f"ok {counts['succeeded']} | skipped {counts['skipped']} | "
        f"failed {counts['failed']}"
    )
    ordered_jobs = sorted_jobs(jobs, statuses)
    job_capacity = max(0, height - 3)
    omitted = max(0, len(ordered_jobs) - job_capacity)
    if omitted:
        visible_jobs = ordered_jobs[: max(0, job_capacity - 1)]
        omitted = len(ordered_jobs) - len(visible_jobs)
        used_job_rows = len(visible_jobs) + 1
    else:
        visible_jobs = ordered_jobs
        used_job_rows = len(visible_jobs)

    items = [
        dashboard_status_text(subtitle, dimmed=dimmed),
        mission_meter(statuses.values(), dimmed=dimmed),
        focused_jobs_table(
            visible_jobs,
            statuses,
            elapsed=elapsed,
            exit_codes=exit_codes,
            frame=frame,
            omitted=omitted,
            dimmed=dimmed,
        ),
    ]
    spare_rows = max(0, height - (3 + used_job_rows))
    output_rows = min(OUTPUT_TAIL_LINES, max(0, spare_rows - 1))
    recent = list(recent_output)[-output_rows:] if output_rows else []
    if recent:
        items.append(output_dock(recent, dimmed=dimmed))
    return Group(*items)


def focused_jobs_table(
    jobs: list[Job],
    statuses: dict[str, str],
    *,
    elapsed: dict[str, str],
    exit_codes: dict[str, str],
    frame: int,
    omitted: int,
    dimmed: bool,
) -> Table:
    table = Table(
        box=None,
        expand=True,
        padding=(0, 1),
        collapse_padding=True,
        header_style=theme_style(TOKYONIGHT["cyan"], bold=True, dimmed=dimmed),
    )
    table.add_column("signal", no_wrap=True)
    table.add_column("phase", width=6, no_wrap=True)
    table.add_column("job", ratio=1, min_width=8, overflow="ellipsis", no_wrap=True)
    table.add_column("trajectory", width=12, no_wrap=True)
    table.add_column("time", justify="right", no_wrap=True)
    table.add_column("exit", justify="right", no_wrap=True)
    for job in jobs:
        status = statuses.get(job.name, "queued")
        table.add_row(
            status_label(status, dimmed=dimmed),
            phase_tag(job.phase, dimmed=dimmed),
            Text(
                job.name,
                style=theme_style(TOKYONIGHT["fg"], bold=True, dimmed=dimmed),
                overflow="ellipsis",
                no_wrap=True,
            ),
            progress_bar(status, width=12, frame=frame, dimmed=dimmed),
            Text(
                elapsed.get(job.name, "-"),
                style=theme_style(TOKYONIGHT["fg"], dimmed=dimmed),
            ),
            Text(
                exit_codes.get(job.name, "-"),
                style=theme_style(TOKYONIGHT["fg"], dimmed=dimmed),
            ),
        )
    if omitted:
        table.add_row(
            Text("…", style=theme_style(TOKYONIGHT["muted"], dimmed=dimmed)),
            "",
            Text(
                f"… {omitted} jobs hidden",
                style=theme_style(TOKYONIGHT["muted"], bold=True, dimmed=dimmed),
                overflow="ellipsis",
                no_wrap=True,
            ),
            "",
            "",
            "",
        )
    return table


def output_dock(
    recent_output: Iterable[tuple[str, str]], *, dimmed: bool = False
) -> Group:
    lines = [
        Text(
            "recent output",
            style=theme_style(TOKYONIGHT["purple"], bold=True, dimmed=dimmed),
            no_wrap=True,
        )
    ]
    for job_name, output in recent_output:
        line = Text(no_wrap=True, overflow="ellipsis")
        line.append(
            f"{job_name}: ",
            style=theme_style(TOKYONIGHT["cyan"], bold=True, dimmed=dimmed),
        )
        line.append(output, style=theme_style(TOKYONIGHT["fg"], dimmed=dimmed))
        lines.append(line)
    return Group(*lines)


def planning_jobs_view(
    jobs: list[Job],
    statuses: dict[str, str],
) -> Group:
    sections = []
    for phase in job_phases(jobs):
        phase_jobs = [job for job in jobs if job.phase == phase]
        sections.append(phase_heading(phase))
        sections.append(planning_jobs_table(phase_jobs, statuses))
    return Group(*sections)


def job_phases(jobs: list[Job]) -> list[str]:
    phases: list[str] = []
    for job in jobs:
        if job.phase not in phases:
            phases.append(job.phase)
    return phases


def phase_heading(phase: str) -> Rule:
    label = PHASE_LABEL.get(phase, phase)
    return Rule(
        Text(label, style=theme_style(TOKYONIGHT["purple"], bold=True)),
        style=TOKYONIGHT["muted"],
    )


def planning_jobs_table(
    jobs: list[Job],
    statuses: dict[str, str],
) -> Table:
    table = Table(
        box=box.SIMPLE_HEAVY,
        expand=True,
        show_lines=False,
        header_style=theme_style(TOKYONIGHT["cyan"], bold=True),
        border_style=TOKYONIGHT["muted"],
    )
    table.add_column("signal", no_wrap=True)
    table.add_column("stage", width=6, no_wrap=True)
    table.add_column(
        "job / command",
        ratio=1,
        min_width=27,
        overflow="ellipsis",
        no_wrap=True,
    )
    table.add_column("trajectory", width=12, no_wrap=True)
    table.add_column("recent output", ratio=2, min_width=24, overflow="fold")
    table.add_column("time", justify="right", no_wrap=True)
    table.add_column("exit", justify="right", no_wrap=True)

    for job in sorted_jobs(jobs, statuses):
        status = statuses.get(job.name, "queued")
        table.add_row(
            status_label(status),
            phase_tag(job.phase),
            job_cell(job),
            progress_bar(status, width=12),
            Text("-", style=TOKYONIGHT["muted"]),
            Text("-", style=TOKYONIGHT["fg"]),
            Text("-", style=TOKYONIGHT["fg"]),
        )
    return table


def sorted_jobs(jobs: list[Job], statuses: dict[str, str]) -> list[Job]:
    job_index = {job.name: index for index, job in enumerate(jobs)}
    return sorted(
        jobs,
        key=lambda job: (
            STATUS_SORT.get(statuses.get(job.name, "queued"), 1),
            job_index[job.name],
        ),
    )


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


def job_cell(job: Job) -> Text:
    cell = Text()
    cell.append(
        job.name,
        style=theme_style(TOKYONIGHT["fg"], bold=True),
    )
    cell.append("\n")
    cell.append(
        display_command(job.command),
        style=TOKYONIGHT["muted"],
    )
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
