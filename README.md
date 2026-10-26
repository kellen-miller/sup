# sup

`sup` is a Rust CLI for running local update jobs from a validated TOML config. It keeps job output in per-run logs and provides a terminal dashboard with separate live output panes.

## Install

Install the three binaries (`sup`, `sup-askpass`, and `sup-rust-update`) from this checkout:

```bash
cargo install --path . --locked
```

The package declares Rust 1.89 as its minimum supported version. Development and CI use the latest stable toolchain, with `rustfmt` and `clippy` installed.

## Commands

```text
sup [--config FILE] [--only SELECTOR]... [--skip SELECTOR]... [--no-tui]
sup run
sup list
sup plan
sup config init [--output FILE]
sup config migrate --from FILE --to FILE
```

Running `sup` or `sup run` executes the selected jobs. `sup list` prints job names. `sup plan` shows the config source, selected order, phase, auth mode, and command without running jobs. Selectors accept job names or aliases; repeated and comma-separated selectors are supported, and skipped jobs take precedence. `--tail`, `--log-retention-days`, and `--no-log-cleanup` control output and log retention. `--no-tui` uses the plain reporter.

`sup list` replaces `--list`; `sup plan` replaces `--dry-run`. The Python entry point is removed.

The dashboard shows one labeled output pane per running job below the status area. Every finished pane shows its result on an opaque overlay for half a second, then closes, including failures, and remaining panes expand. Success uses a green check mark; failure uses a red cross. Crowded terminals use pages: `[` and `]` change pages; Tab and Shift-Tab select a pane on the current page. Up/Down, Page Up/Page Down, and the mouse wheel scroll the selected pane; End resumes following its output. Each pane keeps its own scroll position. Resize the terminal to change the grid. Password prompts take precedence over navigation.

Live panes preserve command colors and text styles. In interactive dashboard mode, SUP requests color from piped child commands through missing `HOMEBREW_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE`, and `CARGO_TERM_COLOR` settings. Existing values and `NO_COLOR` are respected; `HOMEBREW_NO_COLOR` prevents the Homebrew request. Plain mode and redirected or noninteractive runs add no color settings. Terminal control actions are discarded from the display; disk logs keep the original output.

## Configuration

By default, `sup` reads `$XDG_CONFIG_HOME/sup/config.toml`, or `$HOME/.config/sup/config.toml` when `XDG_CONFIG_HOME` is unset. It never searches the current directory. If the default file is absent, embedded defaults from the shipped `config.toml` are used and shown by `sup plan`. An explicit missing `--config` path is an error.

Create a user config with `sup config init`. It creates the destination exclusively and does not overwrite an existing file. TOML has schema version 1 and supports aliases, run retention settings, and jobs with argv commands, requirements, phase, optionality, log name, and authentication policy. Commands are argument vectors, not shell strings; use `sh -c` explicitly when a shell is required.

See [config.toml](config.toml) for the complete default jobs and field examples.

Migrate a legacy YAML config once, without overwriting either file:

```bash
sup config migrate --from old-config.yaml --to config.toml
sup --config config.toml plan
```

The migration preserves command strings and settings. Direct Homebrew install, upgrade, and reinstall commands with legacy sudo preflight are changed to on-demand authentication with a notice. YAML is accepted only by this migration command; TOML is the runtime format.

The optional `skills` job requires `SUP_SKILLS_ROOT` to point to a checkout containing `Cargo.toml`, `Cargo.lock`, and `deps.yaml`. Set that environment variable in the shell that runs `sup`; the job is skipped when its optional requirements are absent.

## Authentication

Jobs configured for sudo ask through the in-dashboard password prompt only when a command requests authentication. SUP passes the password to that individual askpass request, then clears its buffer; it does not save or replay passwords. During active privileged jobs, SUP may refresh the operating-system sudo ticket noninteractively. Homebrew can invalidate a ticket before privileged work, and operating-system policy or an installer can still require another prompt. SUP does not change sudoers, PAM, or Homebrew.

Use `auth = "on-demand"` for Homebrew, `"preflight"` to validate sudo immediately before a job, or `"none"` to disable SUP authentication for that job. Plain terminal mode asks without echoing input. Without a terminal, commands can run until they need a password; SUP then fails that request rather than hanging.

## Logs

Each run writes logs under `~/.cache/sup/runs/<run-id>/`. Logs contain complete child output; active panes share a bounded in-memory history. After all jobs finish, a failure report lists every failed job, its exit code when available, reason, full log path, and recent output. `--tail N` limits each excerpt to N lines; `--tail 0` hides excerpts. Closing panes or trimming their history does not remove disk logs. Old run directories are cleaned after 30 days by default. Use `--log-retention-days N` to change the window or `--no-log-cleanup` to disable cleanup for a run.

## Development

CI uses the bundled Rust policy from [CI #21](https://github.com/kellen-miller/ci/pull/21), pinned in `.github/workflows/rust.yml`. Use the same revision locally; plain Cargo formatting and Clippy do not load that policy. Python 3.11+ and Bash are needed for the external check script, alongside Cargo, rustfmt and Clippy. SUP itself has no Python dependency.

Prepare a policy checkout once:

```bash
sup_ci_checkout="$(mktemp -d)"
git clone --filter=blob:none https://github.com/kellen-miller/ci.git "$sup_ci_checkout"
git -C "$sup_ci_checkout" checkout --detach 9478f2f23f217048a86cb1916fb78b312f20afa7
```

From SUP's checkout, run the same checks as CI:

```bash
SETUP_TOOLCHAIN=false WORKING_DIRECTORY="$PWD" \
  CHECKS="fmt clippy test build" BUILD_PROFILE=release \
  bash "$sup_ci_checkout/.github/actions/rust-check/check.sh"
uvx yamllint==1.38.0 --strict .
actionlint
git diff --check
cargo run --locked -- plan
cargo run --locked -- list
```

The action loads its own formatting, compiler and Clippy settings without copying policy into SUP. Keep the local checkout revision aligned with the workflow pin. Tests use explicit temporary configs and synthetic commands. Do not execute configured update jobs unless the user explicitly asks for a real update run; use `sup plan` to preview them.

GitHub Actions runs the shared policy, tests and release builds on Linux and macOS, plus YAML, workflow and Renovate checks. YAML lint is pinned independently from the Rust package. YAML rules use two-space indentation, block collections, and no line-length limit.
