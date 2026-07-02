from __future__ import annotations

import shutil
from collections import deque
from datetime import datetime, timezone
from pathlib import Path


def create_run_dir(home: Path, timestamp: str | None = None) -> Path:
    timestamp = timestamp or datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return home / ".cache" / "sup" / "runs" / timestamp


def cleanup_old_runs(
    home: Path,
    current_run_dir: Path,
    *,
    retention_days: int,
    now: datetime | None = None,
) -> list[Path]:
    now = now or datetime.now(timezone.utc)
    runs_dir = home / ".cache" / "sup" / "runs"
    if retention_days < 0 or not runs_dir.is_dir():
        return []

    cutoff = now.timestamp() - (retention_days * 24 * 60 * 60)
    removed: list[Path] = []
    for candidate in sorted(runs_dir.iterdir()):
        if candidate.resolve() == current_run_dir.resolve():
            continue
        if not candidate.is_dir():
            continue
        if candidate.stat().st_mtime < cutoff:
            shutil.rmtree(candidate)
            removed.append(candidate)
    return removed


def tail_lines(path: Path, count: int) -> list[str]:
    if count <= 0 or not path.is_file():
        return []
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        return [line.rstrip("\n") for line in deque(handle, maxlen=count)]
