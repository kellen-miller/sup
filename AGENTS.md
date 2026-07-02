# AGENTS.md

## Project

`sup` is a Python 3.10+ CLI for running local update jobs from `config.yaml`.
Keep the project portable: do not hardcode a contributor's home directory,
repository path, machine-specific commands, or personal skills layout.

## Development Commands

Use `uv` for local execution and packaging:

```bash
uv run --project . sup --dry-run
uv run python -m unittest discover -s tests
uvx ruff format .
uvx ruff check .
uv build
```

Before claiming work is complete, run:

```bash
uvx ruff format --check .
uvx ruff check .
uv run python -m unittest discover -s tests
uv run python -m py_compile src/sup/*.py tests/test_sup.py
git diff --check
```

## Runtime Safety

Do not run `sup` without `--dry-run` unless the user explicitly asks for a real
update run. Real runs can upgrade local package managers and developer tools.

## Config Guidance

The default config file is `config.yaml`. Keep jobs data-driven and portable:

- Use `$HOME` for home-relative paths.
- Use environment variables such as `$SUP_SKILLS_UPDATE` for optional personal
  integrations.
- Add `required_env`, `required_commands`, or `required_paths` so missing local
  setup is skipped or failed intentionally.
- Keep `optional: true` for jobs that depend on tools not everyone has.

## CI

Pull requests run `.github/workflows/ruff.yml`, which checks Ruff formatting and
linting. Keep local Ruff output clean before pushing.
