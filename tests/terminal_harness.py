from __future__ import annotations

import argparse
import errno
import fcntl
import io
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time
import unicodedata

from rich.console import Console

from sup.display import LiveDashboard
from sup.jobs import Job


FINAL_MARKER = "SUP-HARNESS-FINAL"
TEST_SECRET = "sup-harness-secret"
ALT_SCREEN_ENTER = b"\x1b[?1049h"
ALT_SCREEN_EXIT = b"\x1b[?1049l"
CURSOR_SHOW = b"\x1b[?25h"
CURSOR_HIDE = b"\x1b[?25l"
SIGNAL_READY = b"\x1b]999;SUP-HARNESS-READY\x07"
CSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]")
ABSOLUTE_MOVE = re.compile(rb"\x1b\[(\d+);(\d+)H")
RELATIVE_UP = re.compile(rb"\x1b\[(\d*)A")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Exercise sup in a synthetic PTY.")
    parser.add_argument(
        "--scenario",
        required=True,
        choices=("password", "sigint", "sigterm"),
    )
    parser.add_argument("--width", required=True, type=int)
    parser.add_argument("--height", required=True, type=int)
    return parser


def synthetic_jobs(count: int = 16) -> list[Job]:
    return [
        Job(
            name=f"job-{index:02d}",
            label=f"Synthetic job {index:02d}",
            phase="core" if index < 4 else "parallel",
            command=("synthetic-command-must-not-run",),
            required_commands=(),
            optional=True,
            log_name=f"job-{index:02d}.log",
            sudo_preflight=index == 0,
        )
        for index in range(count)
    ]


def apply_window_size(fd: int, *, width: int, height: int) -> None:
    packed = struct.pack("HHHH", height, width, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, packed)


def run_child(*, scenario: str, width: int, height: int) -> int:
    apply_window_size(sys.stdout.fileno(), width=width, height=height)
    os.environ["TERM"] = "xterm-256color"
    if scenario == "sigterm":
        signal.signal(signal.SIGTERM, raise_keyboard_interrupt)

    console = Console(force_terminal=True)
    jobs = synthetic_jobs()
    try:
        with LiveDashboard(jobs, console=console) as dashboard:
            dashboard.update(jobs[0].name, "running", output="checking metadata")
            dashboard.update(jobs[1].name, "failed", output="synthetic failure")
            dashboard.update(jobs[2].name, "running", output="downloading archive")
            if scenario == "password":
                password = dashboard.read_password(jobs[:1])
                if password != TEST_SECRET:
                    raise RuntimeError("synthetic password mismatch")
            else:
                os.write(sys.stdout.fileno(), SIGNAL_READY)
                signal.pause()
    except KeyboardInterrupt:
        if scenario == "password":
            raise

    print(FINAL_MARKER, flush=True)
    return 0


def raise_keyboard_interrupt(_signum: int, _frame: object | None) -> None:
    raise KeyboardInterrupt


def echo_is_disabled(fd: int) -> bool:
    return not bool(termios.tcgetattr(fd)[3] & termios.ECHO)


def capture_child(*, scenario: str, width: int, height: int) -> tuple[bytes, int, bool]:
    child_pid, master_fd = pty.fork()
    if child_pid == 0:
        try:
            code = run_child(scenario=scenario, width=width, height=height)
        except BaseException:
            import traceback

            traceback.print_exc()
            code = 1
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(code)

    apply_window_size(master_fd, width=width, height=height)
    os.set_blocking(master_fd, False)
    captured = bytearray()
    echo_disabled = False
    input_sent = False
    signal_sent = False
    child_status: int | None = None
    deadline = time.monotonic() + 10
    try:
        while time.monotonic() < deadline:
            readable, _, _ = select.select([master_fd], [], [], 0.05)
            if readable:
                try:
                    chunk = os.read(master_fd, 65536)
                except OSError as exc:
                    if exc.errno != errno.EIO:
                        raise
                    chunk = b""
                if chunk:
                    captured.extend(chunk)

            if scenario == "password" and not input_sent:
                try:
                    echo_disabled = echo_is_disabled(master_fd)
                except termios.error:
                    echo_disabled = False
                if echo_disabled:
                    os.write(master_fd, TEST_SECRET.encode() + b"\n")
                    input_sent = True
            elif scenario != "password" and not signal_sent:
                if SIGNAL_READY in captured:
                    signum = signal.SIGINT if scenario == "sigint" else signal.SIGTERM
                    os.kill(child_pid, signum)
                    signal_sent = True

            waited_pid, status = os.waitpid(child_pid, os.WNOHANG)
            if waited_pid == child_pid:
                child_status = status
                break
        else:
            os.kill(child_pid, signal.SIGKILL)
            _, child_status = os.waitpid(child_pid, 0)
            raise TimeoutError("synthetic PTY child did not exit within 10 seconds")

        while True:
            readable, _, _ = select.select([master_fd], [], [], 0)
            if not readable:
                break
            try:
                chunk = os.read(master_fd, 65536)
            except OSError as exc:
                if exc.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            captured.extend(chunk)
    finally:
        os.close(master_fd)

    assert child_status is not None
    return bytes(captured), os.waitstatus_to_exitcode(child_status), echo_disabled


def cell_width(value: str) -> int:
    width = 0
    for character in value:
        if unicodedata.combining(character):
            continue
        width += 2 if unicodedata.east_asian_width(character) in {"W", "F"} else 1
    return width


def prompt_cell(raw: bytes) -> tuple[int, int] | None:
    prompt_end = raw.rfind(b"Password:")
    if prompt_end < 0:
        return None
    prompt_end += len(b"Password:")
    home = raw.rfind(b"\x1b[H", 0, prompt_end)
    if home < 0:
        return None
    plain = CSI.sub(b"", raw[home + len(b"\x1b[H") : prompt_end]).decode(
        "utf-8", errors="replace"
    )
    lines = plain.splitlines()
    if not lines:
        return None
    return len(lines), cell_width(lines[-1]) + 1


def final_password_move(raw: bytes) -> tuple[int, int] | None:
    moves = ABSOLUTE_MOVE.findall(raw)
    if not moves:
        return None
    row, column = moves[-1]
    return int(row), int(column)


def budgeted_frame_fits(*, width: int, height: int) -> bool:
    console = Console(
        file=io.StringIO(),
        force_terminal=True,
        width=width,
        height=height,
        _environ={},
    )
    jobs = synthetic_jobs()
    dashboard = LiveDashboard(jobs, console=console)
    dashboard.update(jobs[0].name, "running", output="checking metadata")
    dashboard.update(jobs[1].name, "failed", output="synthetic failure")
    options = console.options.update(width=width, height=height)
    return len(console.render_lines(dashboard.render(), options, pad=False)) <= height


def analyze(
    raw: bytes,
    *,
    scenario: str,
    width: int,
    height: int,
    echo_disabled: bool,
) -> dict[str, bool | int | str]:
    enter_count = raw.count(ALT_SCREEN_ENTER)
    exit_count = raw.count(ALT_SCREEN_EXIT)
    show_count = raw.count(CURSOR_SHOW)
    hide_count = raw.count(CURSOR_HIDE)
    final_position = raw.find(FINAL_MARKER.encode())
    exit_position = raw.rfind(ALT_SCREEN_EXIT)
    password_scenario = scenario == "password"
    expected_prompt = prompt_cell(raw) if password_scenario else None
    actual_move = final_password_move(raw) if password_scenario else None
    return {
        "alternate_screen_entered": enter_count == 1,
        "alternate_screen_exited": exit_count == 1,
        "budgeted_frame_fits": budgeted_frame_fits(width=width, height=height),
        "cursor_controls_paired": show_count == hide_count and show_count > 0,
        "final_marker_count": raw.count(FINAL_MARKER.encode()),
        "height": height,
        "password_cursor_matches_prompt": (
            expected_prompt == actual_move if password_scenario else True
        ),
        "password_echo_suppressed": (
            echo_disabled and TEST_SECRET.encode() not in raw
            if password_scenario
            else True
        ),
        "relative_cursor_up_rows": sum(
            int(value or b"1") for value in RELATIVE_UP.findall(raw)
        ),
        "screen_exit_before_final": (
            exit_position >= 0 and final_position > exit_position
        ),
        "scenario": scenario,
        "width": width,
    }


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    raw, child_code, echo_disabled = capture_child(
        scenario=args.scenario,
        width=args.width,
        height=args.height,
    )
    result = analyze(
        raw,
        scenario=args.scenario,
        width=args.width,
        height=args.height,
        echo_disabled=echo_disabled,
    )
    print(json.dumps(result, sort_keys=True))
    boolean_checks = {
        key
        for key in result
        if key
        not in {
            "final_marker_count",
            "height",
            "relative_cursor_up_rows",
            "scenario",
            "width",
        }
    }
    failures = [key for key in boolean_checks if result[key] is not True]
    if result["final_marker_count"] != 1:
        failures.append("final_marker_count")
    if result["relative_cursor_up_rows"] != 0:
        failures.append("relative_cursor_up_rows")
    if child_code != 0:
        print(f"child_exit_code={child_code}", file=sys.stderr)
        return child_code
    if failures:
        print(f"failed_checks={','.join(failures)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
