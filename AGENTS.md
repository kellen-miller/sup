# AGENTS.md

## Project

`sup` is a Rust CLI for local update jobs. It supports macOS and Linux, reads TOML at runtime, and accepts YAML only through explicit migration. Keep the project portable: do not hardcode a contributor's home directory, repository path, machine-specific commands, or personal skills layout.

## Development Commands

Use Cargo for runtime previews and the pinned shared CI script for validation. Rust policy comes from the exact `kellen-miller/ci` revision in `.github/workflows/rust.yml`; plain `cargo fmt` and `cargo clippy` do not apply it. Do not copy policy files or add inline lint suppressions. Bash and Python 3.11+ are external validation tools, not SUP runtime dependencies.

Prepare a checkout once (see README):

```bash
sup_ci_checkout="$(mktemp -d)"
git clone --filter=blob:none https://github.com/kellen-miller/ci.git "$sup_ci_checkout"
git -C "$sup_ci_checkout" checkout --detach 9478f2f23f217048a86cb1916fb78b312f20afa7
```

Before claiming work complete, run from SUP's checkout:

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

The policy enforces 120-column stable formatting, warnings-as-errors, error handling, documented unsafe boundaries and function complexity/size limits. Favor cohesive lifecycle code over helper chains when resolving diagnostics. Keep the local policy checkout aligned with the action pin.

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
