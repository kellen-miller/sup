# Convert `sup` To Python

## Objective

Move the current zsh-implemented `sup` update workflow into the new Python
project at `/Users/kellen.miller/Development/github/kellen-miller/sup`, while
keeping a small zsh wrapper function named `sup`. The Python app should provide
a much better visual terminal experience and preserve the current update
semantics unless explicitly changed below.

## Confirmed User Decisions

- Keep the zsh `sup` function, but make it a thin wrapper around the Python app.
- The wrapper must not hardcode `/Users/kellen.miller`; it must detect
  `~/Development/github/kellen-miller/sup` or
  `~/development/github/kellen-miller/sup`.
- The wrapper should run `uv run --project "$repo" sup "$@"`.
- The Python repo should expose a console script entry point named `sup`.
- Use Python and `rich` for the visual output.
- Use a live terminal dashboard with one row per updater, status, elapsed time,
  and exit code.
- Hide noisy command output during normal operation.
- On failure, show the last N lines for the failed job.
- Preserve the current ordering for v1:
  - `core` phase runs sequentially:
    - `brew upgrade --greedy --force`
    - `brew cleanup --prune=all`
    - `zimfw upgrade`
  - `parallel` phase runs after `core`:
    - `zimfw update`
    - `gup update`
    - `gcloud components update --quiet`
    - `mas upgrade`
    - `npm update --global`
    - `sudo -n pnpm update --global`
    - `rustup update`
    - `cargo install-update --all --git`
    - `~/.agents/skills/update.py`
- Run all eligible jobs to completion.
- Return non-zero if any job fails.
- Group the final summary as succeeded, skipped, and failed.
- Do not stop the whole run on first failure.
- Add CLI controls:
  - `sup` runs everything
  - `sup --only skills,rustup`
  - `sup --skip brew,mas`
  - `sup --list`
  - `sup --dry-run`
- If the `pnpm` job is enabled, run `sudo -v` as a preflight before the live
  dashboard starts.
- Run the pnpm job with `sudo -n pnpm update --global` so it never blocks on a
  hidden password prompt.
- If sudo authentication is unavailable, mark the pnpm job failed or skipped
  cleanly and continue other jobs.
- Write per-run logs under `~/.cache/sup/runs/<timestamp>/`.
- Write one log file per job.
- Show the last 40 lines for failures by default.
- Add `--tail N`.
- Delete run logs older than 30 days at startup.
- Add `--log-retention-days N`.
- Add `--no-log-cleanup`.
- Never delete the current run directory.
- Missing optional tools should be skipped with a clear reason.
- Missing core tools should fail. For v1, core tools are `brew`, `uv`, and the
  Python runtime used by the wrapper. Optional/skippable tools are `zimfw`,
  `gup`, `gcloud`, `mas`, `npm`, `pnpm`, `rustup`, `cargo`,
  `cargo-install-update`, and the skills updater.

## Agent-Recommended Defaults

- Use only standard-library concurrency plus `rich`; avoid adding a second
  process orchestration dependency.
- Implement a small domain model around `Job`, `JobResult`, and `Runner` so
  the dashboard does not own command sequencing policy.
- Keep command output capture line-oriented and write to log files immediately
  to avoid large in-memory buffers during package-manager runs.
- Test command planning and status behavior with fake commands rather than
  running real package managers.

## Assumptions

- The `sup` repository should own the Python application and tests.
- The dotfiles repository should only keep the zsh wrapper function.
- The project should continue to use `uv` for dependency and script execution.
- The current `.venv` in the `sup` repo is local development state, not a
  required checked-in runtime artifact.

## Open Questions Or User Judgments

- None blocking. Visual styling details such as colors and exact status labels
  may be chosen during implementation as long as the dashboard remains compact
  and readable.

## Accepted Risks And Failure Modes

- Package-manager commands can take a long time and produce large outputs; logs
  must be written incrementally.
- Some commands may not exist on one machine; optional commands should not make
  the whole update unusable.
- `sudo` prompts hidden behind a live dashboard would look like a hang, so
  preflight plus `sudo -n` is required.
- A full `sup` run mutates the host system by upgrading packages. Tests and
  dry-runs must avoid invoking real update commands.

## Validation Expectations

- Python unit tests must cover development-root detection, CLI selection,
  command planning, log cleanup, missing-command behavior, failure aggregation,
  and dry-run behavior.
- A dry-run invocation should show planned jobs without running package
  managers.
- The zsh wrapper should pass `zsh -n`.
- The Python console script should run through `uv run --project <repo> sup`.

## Source Notes

This record is based on the current conversation after invoking
`$durable-feature-workflow` and resolving the Grill Me questions with the user.
Repository evidence inspected:

- `/Users/kellen.miller/Development/github/kellen-miller/sup/pyproject.toml`
- `/Users/kellen.miller/dotfiles/.config/zsh/functions.zsh`
- current `sup` function body in the dotfiles repo
