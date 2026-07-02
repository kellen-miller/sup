from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from string import Template
from typing import Any, Iterable, Literal, Mapping

import yaml


Phase = Literal["core", "parallel"]


@dataclass(frozen=True)
class Job:
    name: str
    label: str
    phase: Phase
    command: tuple[str, ...]
    required_commands: tuple[str, ...]
    optional: bool
    log_name: str
    required_paths: tuple[Path, ...] = ()
    required_env: tuple[str, ...] = ()
    sudo_preflight: bool = False


@dataclass(frozen=True)
class JobsConfig:
    jobs: list[Job]
    aliases: dict[str, tuple[str, ...]]


def config_path() -> Path:
    return Path(__file__).resolve().parents[2] / "config.yaml"


def load_jobs_config(
    path: Path,
    home: Path | None = None,
    env: Mapping[str, str] | None = None,
) -> JobsConfig:
    home = Path.home() if home is None else home
    env = os.environ if env is None else env
    if not path.is_file():
        raise ValueError(f"jobs config not found: {path}")
    data = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError("jobs config must be a mapping")

    aliases = parse_aliases(data.get("aliases", {}))
    jobs_data = data.get("jobs")
    if not isinstance(jobs_data, list) or not jobs_data:
        raise ValueError("jobs config must contain at least one job")

    jobs = [parse_job(item, home=home, env=env) for item in jobs_data]
    validate_jobs_config(jobs, aliases)
    return JobsConfig(jobs=jobs, aliases=aliases)


def parse_aliases(value: Any) -> dict[str, tuple[str, ...]]:
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise ValueError("aliases must be a mapping")
    aliases: dict[str, tuple[str, ...]] = {}
    for name, targets in value.items():
        if not isinstance(name, str) or not name:
            raise ValueError("alias names must be non-empty strings")
        if not isinstance(targets, list) or not all(
            isinstance(item, str) for item in targets
        ):
            raise ValueError(f"alias {name} must be a list of job names")
        aliases[name] = tuple(targets)
    return aliases


def parse_job(value: Any, *, home: Path, env: Mapping[str, str]) -> Job:
    if not isinstance(value, dict):
        raise ValueError("each job must be a mapping")
    name = required_str(value, "name")
    phase = required_str(value, "phase")
    if phase not in ("core", "parallel"):
        raise ValueError(f"job {name} has invalid phase: {phase}")
    command = required_str_list(value, "command")
    required_env = tuple(str_list(value, "required_env"))
    required_paths = tuple(
        Path(expand_placeholders(item, home, env))
        for item in str_list(value, "required_paths")
    )
    log_name = value.get("log_name") or f"{name}.log"
    return Job(
        name=name,
        label=value.get("label") or name,
        phase=phase,
        command=tuple(expand_placeholders(item, home, env) for item in command),
        required_commands=tuple(str_list(value, "required_commands")),
        optional=bool(value.get("optional", True)),
        log_name=required_type(log_name, "log_name", str),
        required_paths=required_paths,
        required_env=required_env,
        sudo_preflight=bool(value.get("sudo_preflight", False)),
    )


def validate_jobs_config(jobs: list[Job], aliases: dict[str, tuple[str, ...]]) -> None:
    names = [job.name for job in jobs]
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        raise ValueError(f"duplicate job name: {', '.join(duplicates)}")
    job_names = set(names)
    for alias, targets in aliases.items():
        missing = [target for target in targets if target not in job_names]
        if missing:
            raise ValueError(
                f"alias {alias} references unknown job: {', '.join(missing)}"
            )


def expand_placeholders(value: str, home: Path, env: Mapping[str, str]) -> str:
    variables = {**env, "HOME": str(home)}
    return Template(value).safe_substitute(variables)


def required_str(data: dict[str, Any], key: str) -> str:
    value = data.get(key)
    return required_type(value, key, str)


def required_str_list(data: dict[str, Any], key: str) -> list[str]:
    value = data.get(key)
    if (
        not isinstance(value, list)
        or not value
        or not all(isinstance(item, str) for item in value)
    ):
        raise ValueError(f"{key} must be a non-empty list of strings")
    return value


def str_list(data: dict[str, Any], key: str) -> list[str]:
    value = data.get(key, [])
    if value is None:
        return []
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise ValueError(f"{key} must be a list of strings")
    return value


def required_type(value: Any, key: str, expected_type: type):
    if not isinstance(value, expected_type):
        raise ValueError(f"{key} must be {expected_type.__name__}")
    return value


def split_selectors(values: Iterable[str] | None) -> list[str]:
    selectors: list[str] = []
    for value in values or []:
        selectors.extend(part.strip() for part in value.split(",") if part.strip())
    return selectors


def expand_selector(
    selector: str, job_names: set[str], aliases: dict[str, tuple[str, ...]]
) -> set[str]:
    if selector in job_names:
        return {selector}
    if selector in aliases:
        return set(aliases[selector])
    raise ValueError(f"unknown selector: {selector}")


def resolve_job_selection(
    jobs: list[Job],
    *,
    aliases: dict[str, tuple[str, ...]],
    only: Iterable[str] | None,
    skip: Iterable[str] | None,
) -> list[Job]:
    job_names = {job.name for job in jobs}
    only_selectors = split_selectors(only)
    skip_selectors = split_selectors(skip)

    selected_names = set(job_names)
    if only_selectors:
        selected_names = set()
        for selector in only_selectors:
            selected_names.update(expand_selector(selector, job_names, aliases))

    for selector in skip_selectors:
        selected_names.difference_update(expand_selector(selector, job_names, aliases))

    return [job for job in jobs if job.name in selected_names]
