# Terminal Dashboard Viewport Decision

## Objective

Make a real `sup` run behave as one bounded terminal dashboard. While jobs are
running, the display must redraw in place without growing scrollback, remain
inside the current terminal dimensions, and accept sudo credentials from a
modal centered in the visible viewport. When the run ends, `sup` must restore
the user's normal terminal and print the final summary once.

## Worktree and provenance

- Worktree: `/Users/kellen/development/github/kellen-miller/sup/.worktrees/fix-interactive-terminal`
- Branch: `codex/fix-interactive-terminal`
- Base ref and commit: `main` at `2a4310f4f50ab30bef085102189e720f439104e1`
- Upstream: none; the branch is local only.
- Source: the user's feedback and design approvals in the current Codex task,
  plus repository inspection of `src/sup/display.py`, `src/sup/cli.py`,
  `src/sup/runner.py`, `tests/test_sup.py`, `config.yaml`, and commit
  `d21a894df9cc70a8f989a525f79d4cd38ed16153`.

## Confirmed user decisions

- A real run may own the terminal's alternate screen while the dashboard is
  active. It must restore the original screen before printing the final
  summary.
- Use the focused viewport design: one compact physical row per visible job,
  with a small active-output dock using spare rows.
- The dashboard must adapt to terminal height. The output dock shrinks first;
  completed jobs collapse into an omitted-count row when more space is needed.
  Running and failed work receives the highest visibility priority.
- The sudo prompt is a real modal interaction, not a message at the bottom of
  the dashboard. It is centered using the visible terminal dimensions, and the
  hidden-input cursor is placed on the modal's `Password:` row.
- Password input must run on the main thread before any parallel command is
  submitted. With manual Rich refreshes, that makes the modal stable for the
  whole read without adding a second redraw-deferral state machine.
- The design, ownership split, error handling, and validation approach were
  explicitly approved by the user.

## Agent-recommended defaults

- Keep `Runner` responsible for process execution, logs, and status events.
- Deepen `LiveDashboard` so its small interface owns alternate-screen
  lifecycle, viewport selection, rendering, modal geometry, cursor placement,
  and hidden input.
- Keep `SudoAuthenticator` responsible for sudo-ticket checks, retry count,
  and password validation. Main-thread job preparation removes the concurrent
  authentication path, so do not retain a lock that no longer protects a race.
- Keep commands in dry-run output and logs, but remove the second command line
  from each live dashboard row. The focused live view optimizes current state,
  not command inspection.
- Maintain one globally ordered rolling output tail labeled by job instead of a
  multi-line output cell on every job row.
- Develop against the active Rich 15.0.0 while preserving the declared Rich
  13.0 and Python 3.10 floors. Use standard-library `getpass` and terminal
  support; add no runtime dependency.
- Support ordinary interactive terminals at 18 rows and taller as the primary
  acceptance surface. On extremely short terminals, preserve a valid bounded
  frame and prioritize running and failed jobs rather than promising that every
  job can be visible simultaneously.

## Assumptions

- The password is entered through an interactive terminal. A non-interactive
  invocation may render without terminal control sequences as Rich already
  does, but this work does not add a headless credential mechanism.
- Unicode status icons may occupy different widths in unusual terminals. Tests
  therefore assert frame bounds and semantic content rather than pixel-perfect
  glyph positions.
- The existing maximum of three sudo attempts remains appropriate.
- The default job set in `config.yaml` is representative for sizing tests, but
  viewport logic must work for arbitrary configured job counts.

## Non-goals

- Do not run Homebrew, Mac App Store, or any other real update as part of
  development or validation.
- Do not change phase semantics, command concurrency after preparation,
  subprocess capture, configuration, log retention, or the final grouped
  summary.
- Do not add mouse input, keyboard navigation, selectable pages, or a new TUI
  framework.
- Do not preserve visible live overflow, scrollback copies of intermediate
  tables, multi-line live job rows, or the old bottom-of-terminal password
  interaction. Those behaviors conflict with the approved design.

## Accepted risks and failure modes

- A terminal can be too short to display every job. The bounded fallback is an
  omitted-count row after priority selection, never unbounded overflow.
- Resizing during a password read cannot relocate an already-blocked input call
  continuously. The prompt must be correctly positioned when the attempt
  begins, and the next render or retry must use the new dimensions.
- All parallel jobs are prepared before any is submitted, so a background job
  cannot emit output while sudo input is active. This ordering is a correctness
  constraint, not an incidental implementation detail.
- An interrupt or exception during input could otherwise strand the alternate
  screen or hidden cursor. Context-managed cleanup and explicit interrupt tests
  are required before acceptance.
- A sudo prompt inside a worker thread cannot be reliably interrupted by a
  signal delivered to Python's main thread. Parallel-job preflight therefore
  moves to the main thread; only prepared commands enter the worker pool.

## Module, interface, and seam decisions

`src/sup/display.py` contains the display module. Its caller-facing interface
remains centered on `LiveDashboard`: enter it, send `update` events, and request
a hidden password. All terminal geometry and redraw policy stay behind that
interface. Internal helpers may calculate row budgets, select visible jobs,
compose the output dock, and locate the password cursor; they are not new
caller-facing adapters.

`src/sup/cli.py` contains the authentication module. `SudoAuthenticator`
delegates each visual password attempt to `LiveDashboard` and keeps ticket,
retry, and validation policy. It never learns cursor coordinates or
alternate-screen sequencing. Its existing lock is removed because every
preflight now runs serially on the main thread.

`src/sup/runner.py` continues to own execution sequencing. Repository inspection
exposed one contract gap: a parallel job currently performs its sudo preflight
inside a worker thread, but terminal input and process signals belong on the
main thread. The runner will prepare all parallel jobs, including sudo
preflights, serially on the main thread before submitting ready commands to the
thread pool. It remains unaware of terminal geometry and emits status and
output events without knowing whether they are drawn or collapsed.

The public behavior seam is a forced-terminal `rich.console.Console` with a
declared width and height. Tests and the harmless terminal harness exercise the
same `LiveDashboard` interface used by the CLI.

## Validation expectations

- A failing test must first reproduce unbounded 42-line frames and refreshes
  that move upward 41 rows at width 120; width 80 renders 44 lines before the
  fix.
- New unit tests must prove alternate-screen configuration, bounded rendering,
  priority selection, completed-job collapse, output-dock shrinkage, password
  cursor placement, retry errors, and cleanup after interrupt.
- A harmless pseudo-terminal harness must exercise dashboard events and the
  real standard-library `getpass` path at representative sizes without
  invoking a real update job. It must prove echo suppression, exact cursor
  placement, raw control ordering, and alternate-screen cleanup for normal,
  SIGINT, and SIGTERM exits.
- Run every command required by `AGENTS.md`: Ruff format checking, Ruff lint,
  unit tests, Python compilation, and `git diff --check`. Also execute the full
  suite on Python 3.10 with the declared Rich 13.0 floor.

## Documentation and architecture records

No `CONTEXT.md` or ADR is warranted. This is a local CLI presentation repair,
the approved terminology already matches the repository (`job`, `phase`,
`status`, `dashboard`, and `sudo authentication`), and the durable decision and
execution records live in this work item.

## Rollout and rollback

The change rolls out as the next `sup` code update. There is no data migration
or compatibility period. Rollback is a normal Git revert of the implementation
commit; logs and configuration remain compatible because their formats do not
change.

## Open questions or user judgments

None. The grill and visual comparison resolved the product and implementation
shape needed to write an executable plan.
