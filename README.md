# sup

`sup` is a configurable local update runner. It executes jobs from a YAML
config, captures per-job logs, and shows a Rich terminal dashboard while work is
running. The dashboard shows a bounded rolling output tail for each active job.

## Install

This project uses `uv`.

```bash
uv run --project . sup --help
```

For a shell alias or function, point your wrapper at this repository and run:

```bash
uv run --project /path/to/sup sup "$@"
```

## Usage

```bash
uv run --project . sup --dry-run
uv run --project . sup --list
uv run --project . sup --only rustup,skills
uv run --project . sup --skip brew,mas
```

By default, `sup` reads `config.yaml` from the repository root. Use
`--config /path/to/config.yaml` to run with a different config.

## Config

The config file contains optional selector aliases and a required `jobs` block:

```yaml
aliases:
  rust:
    - rustup

jobs:
  - name: rustup
    label: Rust toolchain
    phase: parallel
    command: ["rustup", "update"]
    required_commands: ["rustup"]
    optional: true
    log_name: rustup.log
```

Supported job keys:

- `name`: stable selector used by `--only` and `--skip`.
- `label`: display label. Defaults to `name`.
- `phase`: `core` runs sequentially before `parallel` jobs.
- `command`: argument vector to execute.
- `required_commands`: commands that must exist in `PATH`.
- `required_paths`: filesystem paths that must exist.
- `required_env`: environment variables that must be set.
- `optional`: skipped when requirements are missing if `true`; failed if
  `false`.
- `log_name`: filename under the current run log directory.
- `sudo_preflight`: authenticate sudo through the dashboard overlay before
  executing the job.

`$HOME`, `$VAR`, and `${VAR}` placeholders in `command` and `required_paths`
are expanded when the config is loaded. Missing required environment variables
cause optional jobs to be skipped before execution.

The default `skills` job runs the public agent-skills project's locked
`skillctl` environment when the checkout is installed:

```bash
uv run --project . sup --only skills
```

It clears any inherited virtual environment, then calls:

```bash
env -u VIRTUAL_ENV uv run --project "$HOME/.agents/skills" --locked skillctl sync
```

If `uv` or the skills project's `pyproject.toml`, `uv.lock`, or `deps.yaml` is
missing, the optional `skills` job is skipped.

The Node job closes pnpm's stdin so global updates cannot open build-approval
prompts while output is captured. Existing build approvals still apply;
unreviewed scripts stay disabled and are reported by pnpm. Approve required
scripts separately with `pnpm approve-builds --global`.

The Mac App Store job uses `mas upgrade --accurate --no-check-min-os --verbose`.
It checks App Store download metadata, avoiding false negatives from missing
or stale catalog entries. The App Store handles OS compatibility and may show
dialogs for incompatible apps or account issues.

## Logs

Each real run writes logs under:

```text
~/.cache/sup/runs/<timestamp>/
```

Old run directories are cleaned up by default after 30 days. Use
`--log-retention-days N` to change the retention window or `--no-log-cleanup`
to disable cleanup for a run.

## Development

```bash
uv run python -m unittest discover -s tests
uvx ruff format --check .
uvx ruff check .
uv build
```

The GitHub Actions workflow runs Ruff formatting and linting on pull requests.
