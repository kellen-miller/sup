# Corepack Package Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Corepack the sole pnpm and Yarn dispatcher while `sup`
automatically tracks `pnpm@latest` for unpinned directories.

**Architecture:** Replace the independent npm and pnpm parallel jobs with one
ordered shell job so npm updates Corepack before Corepack selects the latest
pnpm and pnpm updates its global packages. Preserve the existing `npm` and
`pnpm` selectors as aliases, and stop Homebrew link repair from managing pnpm.

**Tech Stack:** Python 3.10+, YAML, `unittest`, Homebrew, npm, Corepack, pnpm

## Global Constraints

- Corepack owns the `pnpm`, `pnpx`, `yarn`, and `yarnpkg` shims.
- Homebrew owns Node and does not install or relink pnpm.
- Unpinned pnpm tracks `pnpm@latest`, including new major releases.
- Existing project package-manager pins and lockfiles remain unchanged.
- A failed npm, Corepack, or pnpm command stops the ordered update sequence.
- Do not run a real `sup` update; live migration uses focused commands only.

---

### Task 1: Specify the Corepack-Owned Update Workflow

**Files:**
- Modify: `tests/test_sup.py`
- Test: `tests/test_sup.py`

**Interfaces:**
- Consumes: `load_jobs_config(path) -> JobsConfig` and
  `resolve_job_selection(jobs, aliases, only, skip) -> list[Job]`
- Produces: regression coverage for `node-package-managers`, the `npm` and
  `pnpm` selector aliases, and Node-only Homebrew relinking

- [ ] **Step 1: Update the default-order assertion**

Replace the expected parallel job names `"npm", "pnpm"` with:

```python
"node-package-managers",
```

- [ ] **Step 2: Specify Node-only Homebrew link repair**

Rename the link test and use this expected command:

```python
expected_command = (
    "sh",
    "-c",
    'if brew list --formula node >/dev/null 2>&1; then '
    'brew link --overwrite node; fi',
)
```

- [ ] **Step 3: Specify the ordered package-manager job**

Replace the current pnpm job test with:

```python
def test_node_package_managers_update_in_order_without_sudo(self):
    jobs_config = load_jobs_config(config_path())
    job = next(
        job for job in jobs_config.jobs if job.name == "node-package-managers"
    )

    self.assertEqual(
        job.command,
        (
            "sh",
            "-c",
            "npm update --global && "
            "corepack install --global pnpm@latest && "
            "pnpm update --global",
        ),
    )
    self.assertEqual(job.required_commands, ("sh", "npm", "corepack", "pnpm"))
    self.assertFalse(job.sudo_preflight)
    self.assertNotIn("sudo", job.required_commands)
```

- [ ] **Step 4: Specify selector compatibility**

Add this assertion to `test_selection_accepts_exact_names_and_aliases`:

```python
for selector in ("npm", "pnpm", "node-package-managers"):
    with self.subTest(selector=selector):
        selected = resolve_job_selection(
            jobs,
            aliases=jobs_config.aliases,
            only=[selector],
            skip=[],
        )
        self.assertEqual([job.name for job in selected], ["node-package-managers"])
```

- [ ] **Step 5: Run focused tests and verify RED**

```bash
uv run python -m unittest \
  tests.test_sup.SelectionTest.test_default_jobs_preserve_core_then_parallel_order \
  tests.test_sup.SelectionTest.test_brew_node_link_is_repaired_around_upgrade \
  tests.test_sup.SelectionTest.test_node_package_managers_update_in_order_without_sudo \
  tests.test_sup.SelectionTest.test_selection_accepts_exact_names_and_aliases
```

Expected: failures because `config.yaml` still defines Homebrew pnpm relinking
and independent npm and pnpm jobs.

### Task 2: Implement the Corepack-Owned Configuration

**Files:**
- Modify: `config.yaml`
- Test: `tests/test_sup.py`

**Interfaces:**
- Consumes: regression expectations from Task 1
- Produces: the `node-package-managers` job and compatible npm/pnpm selectors

- [ ] **Step 1: Add selector aliases**

```yaml
  npm:
    - node-package-managers
  pnpm:
    - node-package-managers
```

- [ ] **Step 2: Remove pnpm from both Homebrew link-repair commands**

```yaml
    command:
      - sh
      - -c
      - if brew list --formula node >/dev/null 2>&1; then brew link --overwrite node; fi
```

- [ ] **Step 3: Replace the independent npm and pnpm jobs**

```yaml
  - name: node-package-managers
    label: Node package managers
    phase: parallel
    command:
      - sh
      - -c
      - npm update --global && corepack install --global pnpm@latest && pnpm update --global
    required_commands: ["sh", "npm", "corepack", "pnpm"]
    optional: true
    log_name: node-package-managers.log
```

- [ ] **Step 4: Run focused tests and verify GREEN**

Run the four-test command from Task 1. Expected: four tests pass.

- [ ] **Step 5: Run the complete unit suite**

```bash
uv run python -m unittest discover -s tests
```

Expected: all tests pass.

- [ ] **Step 6: Commit the configuration change**

```bash
git add config.yaml tests/test_sup.py
git commit -m "fix: serialize package manager updates"
```

### Task 3: Migrate and Verify the Live Toolchain

**Files:**
- No repository files modified

**Interfaces:**
- Consumes: the Corepack-owned update workflow from Task 2
- Produces: a local installation where Corepack owns package-manager shims

- [ ] **Step 1: Record pre-migration ownership**

```bash
which -a pnpm yarn corepack
ls -l /opt/homebrew/bin/{pnpm,pnpx,yarn,yarnpkg,corepack}
brew list --versions pnpm
npm ls --global --depth=0
pnpm list --global --depth=0
```

Expected: Homebrew pnpm, npm-installed Corepack, and legacy `@pnpm/exe` appear.

- [ ] **Step 2: Remove competing pnpm installations**

```bash
pnpm remove --global @pnpm/exe
brew uninstall pnpm
```

Expected: Homebrew removes pnpm and pnpm removes the legacy executable package.

- [ ] **Step 3: Update and activate Corepack**

```bash
npm install --global corepack@latest
corepack enable
corepack install --global pnpm@latest
hash -r
```

Expected: Corepack owns pnpm/Yarn shims and selects the latest stable pnpm.

- [ ] **Step 4: Verify shim ownership and version routing**

```bash
ls -l /opt/homebrew/bin/{pnpm,pnpx,yarn,yarnpkg,corepack}
(cd /tmp && pnpm --version)
(cd /Users/kellen/development/github/kellen-miller/chief && pnpm --version)
(cd /Users/kellen/development/github/kellen-miller/venari && pnpm --version)
(cd /Users/kellen/development/github/kellen-miller/ts-proto && yarn --version)
```

Expected: `/tmp` reports latest pnpm, `chief` reports `11.9.0`, `venari`
reports `11.8.0`, and `ts-proto` reports Yarn `4.4.0`.

- [ ] **Step 5: Verify dry-run output**

```bash
uv run --project . sup --dry-run
uv run --project . sup --dry-run --only npm
uv run --project . sup --dry-run --only pnpm
```

Expected: both selectors resolve to the one ordered job; no update runs.

### Task 4: Run Final Repository Verification

**Files:**
- Verify: `config.yaml`
- Verify: `tests/test_sup.py`

**Interfaces:**
- Consumes: all repository changes
- Produces: completion evidence required by `AGENTS.md`

- [ ] **Step 1: Run formatting and lint checks**

```bash
uvx ruff format --check .
uvx ruff check .
```

- [ ] **Step 2: Run tests and compile checks**

```bash
uv run python -m unittest discover -s tests
uv run python -m py_compile src/sup/*.py tests/test_sup.py
```

- [ ] **Step 3: Run package and diff checks**

```bash
uv build
git diff --check
git status --short --branch
```

Expected: all commands exit successfully and only intended commits are ahead of
`origin/main`.
