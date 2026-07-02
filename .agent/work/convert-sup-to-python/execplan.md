# Convert `sup` To A Python Rich Dashboard

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

This plan follows `.agent/PLANS.md` in the repository root. That file defines the required shape for execution plans and requires this document to remain self-contained.

## Purpose / Big Picture

Today the `sup` command is a zsh function in the dotfiles repository. It runs a fixed sequence of package-manager updates, starts several commands in the background, and waits for them. That works, but the behavior and output are hard to inspect: command output interleaves, failures are not summarized clearly, and the orchestration policy lives inside an interactive shell function.

After this change, the repository at `/Users/kellen.miller/Development/github/kellen-miller/sup` will own a Python command-line app named `sup`. The zsh function will remain only as a portable wrapper that finds the repo under `~/Development/github/kellen-miller/sup` or `~/development/github/kellen-miller/sup` and runs `uv run --project "$repo" sup "$@"`. The Python app will preserve the current update ordering while presenting a `rich` live dashboard, structured summaries, per-job logs, cleanup of old logs, command selection flags, and safe sudo handling for the pnpm update.

The complexity dividend is that command sequencing, optional-command checks, logging, failure aggregation, and terminal display move behind one tested Python boundary. Future changes should involve editing data-driven job definitions and tests, not rewriting shell backgrounding logic.

## Progress

- [x] (2026-07-02 14:38Z) Resolved user decisions through the durable workflow grill phase.
- [x] (2026-07-02 14:38Z) Created `.agent/PLANS.md`, `decision.md`, `meta.json`, and this initial ExecPlan.
- [x] (2026-07-02 14:42Z) Activated the implementation goal and set `meta.json` to `stage="implementation"`, `state="active"`.
- [x] (2026-07-02 14:44Z) Added Python package structure, `rich` dependency, `hatchling` build backend, and `sup` console entry point.
- [x] (2026-07-02 14:44Z) Added unittest coverage for job ordering, selectors, log cleanup, missing commands, sudo preflight, status updates, failure aggregation, and `--list`.
- [x] (2026-07-02 14:45Z) Implemented job definitions, selector aliases, log handling, runner, and subprocess capture.
- [x] (2026-07-02 14:47Z) Implemented a `rich.Live` dashboard and final grouped summary.
- [x] (2026-07-02 14:47Z) Updated the dotfiles zsh wrapper to call `uv run --project "$repo" sup "$@"`.
- [x] (2026-07-02 14:48Z) Ran validation commands and captured evidence in this plan.
- [x] (2026-07-02 14:51Z) Ran a review-style pass, fixed sudo preflight timing and cargo subcommand detection, and reran tests.
- [x] (2026-07-02 14:52Z) Ran final verification commands after review fixes and cleaned generated Python caches.

## Surprises & Discoveries

- Observation: The `sup` repo currently has only a minimal `pyproject.toml` and IDE/local environment files.
  Evidence: `find /Users/kellen.miller/Development/github/kellen-miller/sup -maxdepth 2 -type f` showed only `.gitignore`, `pyproject.toml`, and `sup.iml` outside local tool directories.

- Observation: The current `sup` behavior is entirely in `/Users/kellen.miller/dotfiles/.config/zsh/functions.zsh`.
  Evidence: the function runs `brew upgrade`, `brew cleanup`, `zimfw upgrade`, then starts the remaining updaters in the background and calls `wait`.

- Observation: The repo uses a Python `.gitignore` that already ignores `.venv`, so the existing local virtual environment should not become part of this implementation.
  Evidence: `.gitignore` contains `.venv` and `venv/` entries.

- Observation: The current `pyproject.toml` has only `[project]` metadata and no build backend.
  Evidence: `pyproject.toml` contains `name`, `version`, `requires-python`, and empty `dependencies`, but no `[build-system]`.

- Observation: The first implementation wired sudo preflight inside the runner after the live dashboard opened.
  Evidence: review of `src/sup/cli.py` showed `LiveDashboard` entered before `runner.run`, while `runner.run` performed the `sudo -v` path through `_run_one`.

- Observation: The cargo update job must check both `cargo` and the `cargo-install-update` subcommand binary.
  Evidence: the command is `cargo install-update --all --git`; cargo subcommands are backed by binaries named like `cargo-install-update`, and the user explicitly listed `cargo-install-update` as optional/skippable.

## Decision Log

- Decision: Preserve the current two-phase execution model for v1.
  Rationale: This keeps the behavioral change focused on implementation language, output quality, logging, and control flags instead of changing which tools run when.
  Date/Author: 2026-07-02 / Codex from user-confirmed decision.

- Decision: Use `rich` for terminal output.
  Rationale: `rich` provides live tables, status rendering, and readable summaries without requiring a full-screen terminal UI framework.
  Date/Author: 2026-07-02 / Codex from user-confirmed decision.

- Decision: Keep the zsh function as a wrapper and use `uv run --project "$repo" sup "$@"`.
  Rationale: This preserves the user's shell entry point while making Python packaging, dependency resolution, and script execution the responsibility of the `sup` repo.
  Date/Author: 2026-07-02 / Codex from user-confirmed decision.

- Decision: Run `sudo -v` before the dashboard and use `sudo -n` for pnpm.
  Rationale: Hidden sudo prompts inside captured subprocess output can make the dashboard appear stuck. Preflight makes the interactive prompt explicit before the live UI starts, and `sudo -n` prevents blocking later.
  Date/Author: 2026-07-02 / Codex from user-confirmed decision.

- Decision: Treat most tools as optional and skip missing optional commands.
  Rationale: The command should remain useful across machines with different installed ecosystems while still reporting skipped jobs clearly.
  Date/Author: 2026-07-02 / Codex from user-confirmed decision.

- Decision: Support both exact job names and group aliases in `--only` and `--skip`.
  Rationale: The confirmed examples use `--skip brew,mas`, while the actual Homebrew work is split into `brew-upgrade` and `brew-cleanup`. Group aliases keep the CLI humane without hiding the dashboard rows.
  Date/Author: 2026-07-02 / Codex plan improvement.

- Decision: Preflight sudo in `sup.cli` before opening `LiveDashboard`.
  Rationale: The user explicitly wanted `sudo -v` before the dashboard starts. Prompting after the live UI starts can make the command appear stuck.
  Date/Author: 2026-07-02 / Codex review fix.

- Decision: Require `cargo-install-update` for the cargo job in addition to `cargo`.
  Rationale: If the cargo subcommand is absent, the optional job should skip cleanly instead of running and failing as an avoidable missing-tool error.
  Date/Author: 2026-07-02 / Codex review fix.

## Outcomes & Retrospective

No implementation outcomes yet. This section must be updated after each major milestone and at completion.

- Outcome: The Python application, tests, live dashboard path, and zsh wrapper are implemented. Real package-manager upgrades have not been run, by design; validation used tests and dry-runs to avoid mutating the host.
  Date/Author: 2026-07-02 / Codex.

- Outcome: The review pass found and fixed two bounded issues: sudo preflight timing and missing `cargo-install-update` detection.
  Date/Author: 2026-07-02 / Codex.

- Outcome: Final verification passed after review fixes. The work item is complete without running real package-manager upgrades.
  Date/Author: 2026-07-02 / Codex.

## Context and Orientation

There are two repositories involved.

The new application repository is `/Users/kellen.miller/Development/github/kellen-miller/sup`. Its current `pyproject.toml` declares a Python project named `sup` with version `0.1.0`, Python `>=3.10`, and no dependencies. It does not yet contain a package directory, command-line entry point, or tests.

The current shell entry point lives in `/Users/kellen.miller/dotfiles/.config/zsh/functions.zsh`. The `sup` function there currently runs:

    brew upgrade --greedy --force
    brew cleanup --prune=all
    zimfw upgrade
    zimfw update &
    gup update &
    gcloud components update --quiet &
    mas upgrade &
    npm update --global &
    sudo pnpm update --global &
    rustup update &
    cargo install-update --all --git &
    ~/.agents/skills/update.py &
    wait

The Python application should not hardcode `/Users/kellen.miller`. Runtime paths should use the current user's home directory. The zsh wrapper should check `"$HOME/Development/github/kellen-miller/sup"` and `"$HOME/development/github/kellen-miller/sup"` and pick whichever exists.

A "job" in this plan means one command or command group shown as one row in the dashboard. For example, `brew-upgrade` is a job for `brew upgrade --greedy --force`; `skills` is a job for `~/.agents/skills/update.py`.

A "core" job is one that runs sequentially before parallel work begins. A "parallel" job starts after all core jobs finish and runs concurrently with other parallel jobs.

A "missing optional command" means the executable for an optional job is absent from `PATH`, or the job's required local script such as `~/.agents/skills/update.py` is absent. Missing optional commands should produce a skipped result, not a failed overall run.

## Plan of Work

First, build the Python project structure in the `sup` repo. Update `pyproject.toml` to depend on `rich`, declare `hatchling` as the build backend, and expose a console script named `sup`. Create a `src/sup/` package with modules for the command-line interface, job definitions, command execution, log handling, and terminal display. This keeps policy in one place and prevents the display code from owning command sequencing.

Second, add tests before implementation. Use Python's built-in `unittest` or `pytest` if the project chooses to add it. The tests should not invoke real package managers. They should exercise pure functions and fake command runners. Cover parsing `--only` and `--skip`, including group aliases like `brew` that expand to `brew-upgrade` and `brew-cleanup`; preserving core and parallel phases; creating log paths under a supplied home directory; deleting old log run directories while preserving current logs; skipping missing optional commands; requiring core commands; aggregating failures into a non-zero return code; and planning `sudo -v` plus `sudo -n` for pnpm.

Third, implement the runner. Represent jobs as data with stable names, display labels, phase, command arguments, required executable or path, optional/core behavior, and log file name. The runner should evaluate selected jobs, perform preflight checks, create the current run log directory under `~/.cache/sup/runs/<timestamp>/`, run core jobs sequentially, run parallel jobs concurrently, stream output into each job's log file, and return a structured `JobResult` for every selected job.

Fourth, implement the `rich` output. The live dashboard should show a compact table with job name, phase, status, elapsed time, and exit code. It should not print live subprocess output by default. After all jobs finish, it should print a grouped summary of succeeded, skipped, and failed jobs. For failures, it should print the last `--tail` lines from each failed job log. The default tail count is 40.

Fifth, update the dotfiles zsh wrapper. Replace the shell implementation body with a small function that finds the repo under `~/Development` or `~/development`, checks that `uv` exists, and runs `uv run --project "$repo" sup "$@"`. Keep the function name `sup`.

Finally, validate in layers. Run the Python tests, run the Python command with `--list`, run `--dry-run`, run `uv run --project /Users/kellen.miller/Development/github/kellen-miller/sup sup --dry-run`, and run `zsh -n /Users/kellen.miller/dotfiles/.config/zsh/functions.zsh`. Do not run a full real `sup` upgrade as validation unless the user explicitly asks, because it mutates the machine.

## Concrete Steps

Work from the `sup` repo for Python changes:

    cd /Users/kellen.miller/Development/github/kellen-miller/sup

Add source and test directories:

    mkdir -p src/sup tests

Update `pyproject.toml` so it includes:

    [project]
    name = "sup"
    version = "0.1.0"
    requires-python = ">=3.10"
    dependencies = [
        "rich>=13.0",
    ]

    [project.scripts]
    sup = "sup.cli:main"

    [build-system]
    requires = ["hatchling"]
    build-backend = "hatchling.build"

If using `pytest`, add it as a development dependency through the project's chosen `uv` workflow. If avoiding extra test dependencies, use `unittest` and keep only `rich` as a runtime dependency.

Create tests that prove behavior without real package-manager calls. The first test run should fail because the package does not exist yet:

    cd /Users/kellen.miller/Development/github/kellen-miller/sup
    uv run python -m unittest discover -s tests

Expected before implementation: import failures or failing assertions for missing `sup` modules.

Implement enough code to pass each test in small steps. Keep the public command entry point at `src/sup/cli.py::main`.

Update the zsh wrapper in the dotfiles repo:

    cd /Users/kellen.miller/dotfiles

Edit `.config/zsh/functions.zsh` so `function sup()` becomes a wrapper equivalent to:

    function sup() {
        local repo
        if [[ -d "$HOME/Development/github/kellen-miller/sup" ]]; then
            repo="$HOME/Development/github/kellen-miller/sup"
        elif [[ -d "$HOME/development/github/kellen-miller/sup" ]]; then
            repo="$HOME/development/github/kellen-miller/sup"
        else
            echo "sup repo not found under ~/Development or ~/development" >&2
            return 1
        fi

        if ! command -v uv >/dev/null 2>&1; then
            echo "uv is required to run sup" >&2
            return 1
        fi

        uv run --project "$repo" sup "$@"
    }

Validate after each meaningful slice:

    cd /Users/kellen.miller/Development/github/kellen-miller/sup
    uv run python -m unittest discover -s tests
    uv run --project . sup --list
    uv run --project . sup --dry-run

    cd /Users/kellen.miller/dotfiles
    zsh -n .config/zsh/functions.zsh

## Validation and Acceptance

Acceptance requires observable behavior, not only code presence.

The Python test suite must pass:

    cd /Users/kellen.miller/Development/github/kellen-miller/sup
    uv run python -m unittest discover -s tests

The installed package entry point must import through the project environment:

    uv run --project . python -c "import sup.cli; print(sup.cli.__name__)"

Expected output:

    sup.cli

The command must list stable job names:

    uv run --project . sup --list

Expected output should include at least:

    brew-upgrade
    brew-cleanup
    zimfw-upgrade
    zimfw-update
    gup
    gcloud
    mas
    npm
    pnpm
    rustup
    cargo-install-update
    skills

The dry-run command must not run package managers and should show planned jobs grouped by phase:

    uv run --project . sup --dry-run

Expected behavior: output identifies the `core` phase, the `parallel` phase, and the commands that would run.

The selection flags must work:

    uv run --project . sup --only skills,rustup --dry-run
    uv run --project . sup --skip brew,mas --dry-run

Expected behavior: only selected jobs appear for `--only`; skipped jobs do not appear for `--skip`. The `brew` selector must behave as a group alias for both `brew-upgrade` and `brew-cleanup`.

The zsh wrapper must parse:

    cd /Users/kellen.miller/dotfiles
    zsh -n .config/zsh/functions.zsh

Final acceptance: calling `sup --dry-run` from an interactive shell should dispatch through the zsh wrapper to the Python app without using the old shell implementation.

## Idempotence and Recovery

The implementation should be safe to run repeatedly. `--dry-run` must not mutate package managers or write per-job logs beyond any minimal command startup behavior. Log cleanup must never delete the current run directory. Log cleanup should delete only directories under `~/.cache/sup/runs/` older than the configured retention window.

If the zsh wrapper is wrong, recovery is to restore the previous function body from git or run `uv run --project "$HOME/Development/github/kellen-miller/sup" sup` directly while fixing the wrapper.

If a full real `sup` run fails, the command should preserve logs under `~/.cache/sup/runs/<timestamp>/` and print the failed job names plus log tails. Re-running with `--only <failed-job>` should support focused retry.

## Artifacts and Notes

The decision record for this work item is `.agent/work/convert-sup-to-python/decision.md`.

The Python application repo is `/Users/kellen.miller/Development/github/kellen-miller/sup`.

The zsh wrapper lives in `/Users/kellen.miller/dotfiles/.config/zsh/functions.zsh`.

The skills updater invoked by the `skills` job is `~/.agents/skills/update.py`, which already exists as a symlink to the dotfiles-managed updater.

Validation evidence from 2026-07-02:

    cd /Users/kellen.miller/Development/github/kellen-miller/sup
    uv run python -m unittest discover -s tests
    ............
    ----------------------------------------------------------------------
    Ran 12 tests in 0.018s
    OK

    uv run --project . python -c "import sup.cli; print(sup.cli.__name__)"
    sup.cli

    uv run --project . sup --list
    brew-upgrade
    brew-cleanup
    zimfw-upgrade
    zimfw-update
    gup
    gcloud
    mas
    npm
    pnpm
    rustup
    cargo-install-update
    skills

    zsh -n /Users/kellen.miller/dotfiles/.config/zsh/functions.zsh
    # exit 0

    uv run --project . sup --only skills,rustup --dry-run
    # rendered a dry-run table containing only rustup and skills

    uv run --project . sup --skip brew,mas --dry-run
    # rendered a dry-run table without brew-upgrade, brew-cleanup, or mas

    zsh -lc 'source /Users/kellen.miller/dotfiles/.config/zsh/functions.zsh; sup --only skills --dry-run'
    # rendered a dry-run table containing only skills

Final verification evidence from 2026-07-02 after review fixes:

    cd /Users/kellen.miller/Development/github/kellen-miller/sup
    uv run python -m unittest discover -s tests
    .............
    ----------------------------------------------------------------------
    Ran 13 tests in 0.031s
    OK

    uv run python -m py_compile src/sup/*.py tests/test_sup.py
    # exit 0

    uv run --project . python -c "import sup.cli; print(sup.cli.__name__)"
    sup.cli

    uv run --project . sup --list
    brew-upgrade
    brew-cleanup
    zimfw-upgrade
    zimfw-update
    gup
    gcloud
    mas
    npm
    pnpm
    rustup
    cargo-install-update
    skills

    uv run --project . sup --dry-run
    # rendered all core and parallel jobs without running package managers

    uv run --project . sup --only skills,rustup --dry-run
    # rendered only rustup and skills

    uv run --project . sup --skip brew,mas --dry-run
    # rendered no brew-upgrade, brew-cleanup, or mas jobs

    zsh -lc 'source /Users/kellen.miller/dotfiles/.config/zsh/functions.zsh; sup --only skills --dry-run'
    # rendered only skills through the wrapper

    git diff --check
    # exit 0 for the sup repo and for the dotfiles wrapper diff

## Interfaces and Dependencies

The final Python package should expose a console script named `sup` through `pyproject.toml`:

    [project.scripts]
    sup = "sup.cli:main"

Use `rich` for display. Keep command orchestration independent from `rich` so tests can validate behavior without terminal rendering.

Use `hatchling` as the build backend in `pyproject.toml`. This makes the `src/sup/` layout and console entry point explicit for `uv run --project . sup`.

Define job data in a module such as `src/sup/jobs.py`. The interface should make job definitions easy to inspect:

    Job(
        name="brew-upgrade",
        label="Homebrew upgrade",
        phase="core",
        command=("brew", "upgrade", "--greedy", "--force"),
        required=("brew",),
        optional=False,
        log_name="brew-upgrade.log",
    )

Also define selector aliases in the same module or next to the selection code:

    SELECTOR_ALIASES = {
        "brew": ("brew-upgrade", "brew-cleanup"),
        "zimfw": ("zimfw-upgrade", "zimfw-update"),
        "cargo": ("cargo-install-update",),
    }

Exact job names and aliases should both be accepted by `--only` and `--skip`. Unknown selectors should produce a clear argument error before any update command runs.

Define results in a module such as `src/sup/runner.py`:

    JobResult(
        job=job,
        status="succeeded" | "failed" | "skipped",
        exit_code=0 | nonzero | None,
        started_at=...,
        ended_at=...,
        log_path=...,
        reason=...,
    )

The runner should hide sequencing policy from the CLI. The CLI should parse arguments, create configuration, call the runner, and return `0` only when no selected job failed. The display module should render runner state, not decide which jobs run.
