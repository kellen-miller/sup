# AGENTS.md

## Project

`sup` is a Rust CLI for local update jobs. It supports macOS and Linux, reads TOML at runtime, and accepts YAML only through explicit migration. Keep the project portable: do not hardcode a contributor's home directory, repository path, machine-specific commands, or personal skills layout.

## Development Commands

Use Cargo for local execution and packaging:

```bash
cargo run -- plan
cargo run -- list
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release --bins
uvx yamllint==1.38.0 --strict .
actionlint
```

Before claiming work is complete, run:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release --bins
uvx yamllint==1.38.0 --strict .
actionlint
git diff --check
```

## Runtime Safety

Do not execute configured update jobs unless the user explicitly asks for a real update run. Preview jobs with `sup plan`. Tests may execute only explicit temporary configs containing local synthetic commands, with isolated HOME, cache, config, and PATH.

## Config Guidance

The default config is `$XDG_CONFIG_HOME/sup/config.toml`, or `$HOME/.config/sup/config.toml` when XDG_CONFIG_HOME is unset. If no default config exists, embedded `config.toml` jobs are used. Never discover config from the working directory.

- Use `$HOME` for home-relative paths.
- Use environment variables such as `$SUP_SKILLS_ROOT` for optional personal integrations.
- Add `required_env`, `required_commands`, or `required_paths` so missing local setup is skipped or failed intentionally.
- Keep `optional = true` for jobs that depend on tools not everyone has.
- YAML parsing belongs only to `sup config migrate`; TOML is the runtime format.

## CI

Pull requests run the `Rust Format`, `Rust Clippy`, `Rust Tests (Linux)`, `Rust Tests (macOS)`, and `YAML lint` checks. Shared reusable workflow and Renovate checks remain enabled. Keep local checks clean before pushing.
