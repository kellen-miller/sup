# Bound the Live Terminal Dashboard

This ExecPlan is a living document. The sections `Progress`, `Surprises &
Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to
date as work proceeds.

Maintain this document in accordance with `.agent/PLANS.md`. The intent and
provenance source is
`.agent/work/terminal-dashboard-viewport/decision.md`; this plan repeats every
fact needed for implementation so a reader does not need the earlier
conversation.

## Purpose / Big Picture

After this change, a real `sup` run owns one alternate terminal screen while
jobs execute. Status changes redraw in that screen without filling scrollback,
every frame stays inside the terminal's current width and height, and a sudo
request places hidden input inside a modal centered in the visible viewport.
When the run finishes or is interrupted, the user's original terminal returns
and the final summary prints once.

Today `LiveDashboard` in `src/sup/display.py` renders 42 lines for the default
jobs and constructs Rich `Live` with `vertical_overflow="visible"`. Rich then
records all 42 lines as the live-render height even on an 18- or 24-row
terminal. Every refresh attempts to move upward and erase 41 rows, so the
terminal scrolls and old tables remain visible. The sudo overlay is centered
against that oversized 42-line renderable, while `getpass` reads at the live
render's terminal cursor. The three user-visible symptoms therefore share one
root cause: the display module does not own a bounded viewport or the input
cursor within it.

The complexity dividend is a deeper display module. Callers send job events and
request password input; they no longer need to coordinate overflow, alternate
screen state, modal geometry, or cursor movement. `Runner` remains ignorant of
presentation, and `SudoAuthenticator` remains ignorant of terminal coordinates.

## Progress

- [x] (2026-07-15 22:58Z) Reproduced the oversized live render and traced it to
  `vertical_overflow="visible"` introduced by commit `d21a894`.
- [x] (2026-07-15 22:58Z) Confirmed the focused alternate-screen design,
  centered hidden-input modal, ownership split, failure behavior, and validation
  strategy with the user.
- [x] (2026-07-15 22:58Z) Created the decision record, metadata, repository
  `PLANS.md`, and initial ExecPlan in the isolated worktree.
- [x] (2026-07-16 02:07Z) Added viewport, resize, semantic selection, output
  dock, password-coordinate, cleanup, runner-thread, and EOF behavior tests;
  the full suite now contains 63 test methods.
- [x] (2026-07-16 02:07Z) Implemented the compact focused dashboard in Rich's
  alternate screen with height-aware job selection and a global output dock.
- [x] (2026-07-16 02:07Z) Moved modal rendering, absolute cursor placement,
  hidden input, and cleanup into `LiveDashboard.read_password`; retained sudo
  ticket, retry, and validation policy in `SudoAuthenticator`.
- [x] (2026-07-16 01:52Z) Split runner preparation from execution; the entire
  parallel batch now completes requirement checks and sudo preflights on the
  main thread before ready commands are submitted to workers.
- [x] (2026-07-16 02:07Z) Added and ran the synthetic PTY harness for password
  input at 80x18 and 120x24 and signal cleanup at 80x18.
- [x] (2026-07-16 02:51Z) Ran the required validation and all three reviews
  against this plan and `decision.md`. The adversarial implementation review's
  one medium and four low findings were verified and resolved or recorded; the
  final suite contains 69 tests and every acceptance command exits zero.

## Surprises & Discoveries

- Observation: the default dashboard is 42 rendered lines at width 120 and 44
  lines at width 80, independent of terminal height.
  Evidence: width-120 forced-terminal consoles at heights 18, 24, 36, and 50
  all rendered 42 base lines before Rich overflow handling.
- Observation: three live renders produced 123 single-row cursor-up commands at
  heights 18, 24, and 36.
  Evidence: each render erased 41 rows because `vertical_overflow="visible"`
  never crops the recorded Rich `LiveRender` shape.
- Observation: Python 3.14's `getpass.getpass` opens `/dev/tty`, disables echo,
  reads at the terminal's current cursor, writes a newline, and restores terminal
  attributes. The prompt string may be empty, but input still occurs wherever
  the live render left the cursor.
  Evidence: local source inspection of the active Python 3.14.3 runtime.
- Observation: `Runner.run` currently submits the entire parallel `_run_one`
  operation to `ThreadPoolExecutor`, including `sudo_preflight`.
  Evidence: `src/sup/runner.py` lines 101-124 submit `_run_one`, while lines
  174-182 invoke the preflight. A real SIGINT is delivered to the main Python
  thread and cannot safely unwind a worker blocked in `/dev/tty` input.
- Observation: Rich's `screen=True` path always shapes output to the viewport,
  so a PTY assertion that merely checks frame height is tautological. The
  pre-crop render budget and semantic row selection must be tested separately.
  Evidence: the current 42-line dashboard produces exactly 18 terminal rows
  when wrapped in Rich `Screen`, even though important content is clipped.
- Observation: the existing sudo keepalive was intentionally removed by commit
  `78f8934` in favor of event-based preflight checks. This work keeps that
  behavior and does not reintroduce a timer or retain password material.
- Observation: the runner tracer initially observed the first parallel command
  starting after only the first job's requirement check and sudo preflight;
  the remaining jobs had not begun preparation.
  Evidence: `test_parallel_jobs_prepare_on_main_before_worker_execution` failed
  with only `requirement:first` and `preflight:first` before the first command.
- Observation: after splitting preparation and execution, all 16 runner tests
  pass. Three prepared commands rendezvous concurrently on three worker thread
  identifiers while all requirement and preflight callbacks use the main
  thread, and a skipped preflight result remains in configured order.
  Evidence: `uv run python -m unittest tests.test_sup.RunnerTest`.
- Observation: the 13 configured jobs render in 16 pre-crop rows at widths 80
  and 120, leaving the Rich screen crop as a fallback instead of the layout
  mechanism. A 20-job test resizes from 120x24 to 80x18 and reports exactly six
  hidden jobs without rebuilding the dashboard.
  Evidence: `DisplayTest` viewport and mutable-size tests.
- Observation: a signal sent as soon as alternate-screen entry appears can
  interrupt Rich inside `Live.__enter__`, before Python can invoke the context's
  exit path. That is not representative of interrupting a running dashboard.
  Evidence: the first harness signal attempt entered the alternate screen but
  had no paired exit. The harness now emits an invisible ready control from
  inside the active context; SIGINT and SIGTERM then both pair screen and cursor
  controls and place the final marker after screen exit.
- Observation: formal spec review found a narrower entry-window failure: a
  signal after Rich emitted alternate-screen entry but before assigning its
  internal state could make `Live.stop()` unaware that cleanup was needed.
  Evidence: a red regression console raised from `set_alt_screen(True)` after
  emitting the control; `LiveDashboard.__enter__` now explicitly restores the
  cursor and exits the alternate screen when Rich cannot do so itself.
- Observation: scanning the composed dashboard for the first `Password:` was
  ambiguous when a job name contained that text above the modal.
  Evidence: the red decoy-name test moved to row 4, column 28 instead of the
  modal at row 11, column 44. Cursor discovery now scans only the actual overlay
  segments used by the shared overlay layout.
- Observation: the PTY harness initially supplied `height` in Rich render
  options while measuring its pre-crop frame, making the evidence susceptible
  to the exact cropping tautology the plan prohibited.
  Evidence: `budgeted_frame_fits` now renders with unbounded-height console
  options while the dashboard independently reads `console.size.height` for
  its own budget. Raw checks also require screen entry before the first frame
  and a home move after entry.
- Observation: a password modal cannot expose its `Password:` cell in a
  terminal six rows tall or shorter, and the original coordinate failure
  escaped as an uncaught `RuntimeError`.
  Evidence: the adversarial review reproduced the abort at 80x6. The display
  now reports unavailable input as `EOFError`, which the sudo boundary already
  maps to required-job failure or optional-job skip without a traceback.
- Observation: the status summary and mission meter wrapped below roughly 58
  columns even though the row budget counted each as one row.
  Evidence: the pre-fix 40x18 render used 19 or more rows and the harness failed
  `budgeted_frame_fits`. Both texts now ellipsize without wrapping; unit cases
  at 40x18 and 30x12 and a real password PTY at 40x18 pass. A final sweep also
  exposed a four-row minimum at one-, two-, and three-row heights; compact
  status-only fallbacks removed it, and all 504 checked geometries from 20 to
  120 columns and one to 24 rows now fit before Rich cropping.
- Observation: recording elapsed time during `_prepare` caused an instant
  parallel command to inherit the time spent waiting for a later job's sudo
  prompt.
  Evidence: the adversarial probe reported 0.406 seconds for an instant command
  beside a 0.4-second preflight. A deterministic regression now proves only the
  interval inside `_execute` is reported.

## Decision Log

- Decision: use Rich's alternate screen for real live runs and restore the
  normal screen before summary output.
  Rationale: this is the direct terminal primitive for one bounded in-place
  interface and matches the user's approved behavior.
  Date/Author: 2026-07-15 / user and Codex.
- Decision: replace phase-separated multi-line rows with one compact table and
  a global labeled output dock.
  Rationale: the default job set must fit ordinary terminal heights; commands
  remain available in dry-run output and logs.
  Date/Author: 2026-07-15 / user and Codex.
- Decision: keep execution, display, and sudo policy in `Runner`,
  `LiveDashboard`, and `SudoAuthenticator`, respectively.
  Rationale: each module retains one coherent responsibility, while terminal
  sequencing becomes local to the display module.
  Date/Author: 2026-07-15 / user and Codex.
- Decision: do not add redraw-deferral or authentication-lock state around
  hidden input.
  Rationale: the runner prepares all jobs before submitting any parallel
  command, so no producer exists during the prompt. Manual refresh plus
  main-thread input provides the approved stable modal with fewer states.
  This supersedes the earlier defensive deferral idea after adversarial review
  proved that scenario unreachable in the selected scheduler.
  Date/Author: 2026-07-15 / Codex adversarial-review resolution.
- Decision: prepare parallel jobs and run sudo preflights serially on the main
  thread, complete preparation of the whole parallel batch, and only then
  submit all ready commands.
  Rationale: terminal input and signal cleanup must share the main thread;
  worker threads should only run already-prepared commands.
  Date/Author: 2026-07-15 / Codex planning improvement.
- Decision: do not print the grouped job summary after an interrupted run.
  Rationale: interruption leaves no complete result set, and existing CLI
  behavior intentionally prints a dedicated interruption notice after terminal
  restoration. The formal review claim broadened the ordinary-completion
  summary requirement beyond the approved execution semantics.
  Date/Author: 2026-07-16 / Codex formal-review resolution.
- Decision: reject new render-state and shared table/test-stub abstractions from
  the standards smell pass, while removing unused planning-table parameters.
  Rationale: focused live rendering and command-rich dry-run rendering are
  intentionally different views; bundling state or test doubles would add
  indirection without a second production consumer.
  Date/Author: 2026-07-16 / Codex formal-review resolution.
- Decision: treat a password prompt that cannot fit in the current viewport as
  unavailable input rather than attempting an off-screen or uncentered prompt.
  Rationale: hidden input must never detach from its visible label; existing
  auth policy already handles unavailable input safely and preserves job
  requiredness semantics.
  Date/Author: 2026-07-16 / Codex adversarial-review resolution.
- Decision: make fixed-budget header rows no-wrap and measure command elapsed
  time only inside `_execute`.
  Rationale: the display's row accounting must remain true at narrow widths,
  and human authentication time is not command execution time.
  Date/Author: 2026-07-16 / Codex adversarial-review resolution.

## Outcomes & Retrospective

Implementation, behavioral validation, and review are complete. The dashboard
uses a bounded focused alternate screen, including narrow terminals; password
entry lands on the modal's rendered prompt cell with real PTY echo suppression;
and an impossibly short password viewport degrades to safe auth-unavailable
behavior. Runner preparation guarantees prompting stays on the main thread
before worker execution, while elapsed time measures execution only.

A fresh-eyes recent-work pass found no correctness, scope, or
simplicity-boundary defect. Formal review found and resolved entry cleanup,
prompt ambiguity, portability, and validation-evidence defects. The independent
adversarial review found no critical or high issue; its short-viewport,
narrow-width, elapsed-time, and PTY-oracle findings are fixed, while its note
about the cursor unit test sharing the render pipeline is covered by the
independent raw-byte PTY oracle.

Final evidence is 69 passing tests locally and on Python 3.10 with Rich 13.0.0,
Ruff format and lint, byte compilation, `git diff --check`, `uv build`, a safe
`sup --dry-run`, the four canonical PTY runs, and an additional 40x18 password
PTY run. Every command exits zero; PTY output shows paired alternate-screen and
cursor controls, exact password cursor placement, suppressed echo, one final
marker after screen exit, and zero relative cursor-up rows.

## Context and Orientation

The repository is a Python 3.10+ CLI. Work from the repository root on branch
`codex/fix-interactive-terminal`, based on `main` commit
`2a4310f4f50ab30bef085102189e720f439104e1`. The branch has no upstream.

`src/sup/cli.py` parses arguments, creates the Rich `Console`, enters
`LiveDashboard`, creates `SudoAuthenticator`, runs jobs, and prints the final
summary. `SudoAuthenticator` checks the cached sudo ticket, prompts up to three
times, and validates with `sudo -S -p "" -v`. Its current lock becomes
unnecessary once every preflight is serialized on the main thread.

`src/sup/runner.py` executes core jobs sequentially and currently puts each
entire parallel `_run_one` call in a thread pool. It sends `queued`, `running`,
`succeeded`, `failed`, or `skipped` status plus output lines through the
`on_update` callback. Because `_run_one` also performs requirement checks and
sudo preflight, a parallel job can currently block a worker on terminal input.
This plan completes preparation of the entire parallel batch on the main thread
and only then submits ready command execution to workers. Therefore no worker
can emit while a later job is asking for a password.

`src/sup/display.py` defines `LiveDashboard` and all Rich renderables. A Rich
`Live` object repeatedly replaces one rendered region. An alternate screen is a
terminal-owned buffer that can be discarded to reveal the user's original
screen. A viewport is the currently visible width and height reported by the
console. The current display uses `vertical_overflow="visible"`, two phase
tables, a two-line job cell, and up to three output lines per job; that
combination exceeds ordinary viewport height.

`tests/test_sup.py` is the unit-test suite. It already has forced-terminal
helpers, display rendering assertions, sudo retry tests, and `Live` constructor
assertions. Extend those public behavior seams rather than testing private Rich
implementation details wherever possible. The final suite must contain at
least 63 tests (53 existing plus at least 10 new methods); existing test methods
may be rewritten for the new contract but not silently deleted.

`tests/terminal_harness.py` will be a harmless executable harness created by
this work. It must instantiate the dashboard with synthetic `Job` objects and
events only. It may exercise Python's real `getpass.getpass` against its own
pseudo-terminal, but must never call `sup.cli.main`, `Runner.run`, Homebrew,
`mas`, sudo validation, or any configured update command.

## Plan of Work

### Milestone 1: Lock the viewport behavior with failing tests

Extend `tests/test_sup.py` before production changes. Give `terminal_console`
an explicit `height` parameter, construct forced-terminal `Console` instances
with explicit width and height, and use the default configured jobs to reproduce
the failure. Add a direct-render helper that measures pre-crop Rich segment
lines without depending on ANSI styling. That helper must read the dashboard's
own current console height rather than relying on `Console.options.height`,
which is `None` outside Rich's live `Screen` path.

Assert the exact load-bearing configuration
`Live(dashboard, console=console, screen=True, auto_refresh=False,
vertical_overflow="crop")`; `screen=True`, not crop alone, owns the alternate
screen and removes relative cursor-up redraws. Assert that the pre-crop
renderable at heights 18, 24, and 36 never exceeds the corresponding terminal
height. Add focused-layout
assertions for a single compact row per job, a labeled rolling output dock, and
the removal of command text from the live table while keeping dry-run command
rendering unchanged.

Use a small mutable-size `Console` test adapter to change width and height
between two dashboard updates. The second frame must use the new row budget and
new overlay center without reconstructing `LiveDashboard`. This proves the
approved resize behavior rather than only testing several fixed consoles.

Add selection tests with more jobs than the row budget. The priority order must
be running, failed, queued, skipped, then succeeded, preserving configuration
order within a status. The view must spend rows on job state first, shrink the
output dock when needed, and render one omitted-count row when jobs cannot all
fit. This replaces the current priority map that groups every terminal status
together and the current phase-section rendering.

Add password tests through the `LiveDashboard` interface. A prompt at 80x18 and
120x24 must calculate a cursor position exactly at the cell after the rendered
`Password:` label. The test must independently find that label in rendered
segments rather than calling the implementation's coordinate helper. Unit
tests may inject a reader to prove retry and exception cleanup; the PTY milestone
below must separately execute real `getpass` and prove echo suppression.

Add runner tests that record `threading.get_ident()` inside the parallel sudo
preflight callback and inside command runners. The preflight identifier must
equal the test's main-thread identifier, command runners must execute in worker
threads, a failed optional preflight must still yield a skipped result, and
ready non-sudo commands must remain parallel. These tests fail against the
current `_run_one` submission shape.

Run the targeted tests and record the expected pre-implementation failures in
`Surprises & Discoveries`. The failures should identify current visible
overflow, unbounded pre-crop height, absent cursor positioning, and worker-thread
preflight; unrelated failures indicate the test seam is wrong and must be
corrected before implementation.

Rewrite, rather than discard, the existing tests whose old contract is
deliberately replaced: `test_live_dashboard_renders_phase_sections`,
`test_live_dashboard_omits_duplicate_sup_chrome`,
`test_live_dashboard_renders_sudo_auth_overlay`,
`test_sudo_auth_overlay_does_not_reflow_dashboard`, and
`test_live_dashboard_refreshes_only_on_updates`. Update the four CLI
`FakeDashboard` definitions used by `test_sudo_overlay_reads_password_and_clears`,
`test_sudo_overlay_trusts_successful_validation`,
`test_sudo_authenticator_reuses_valid_ticket_without_prompt`, and
`test_sudo_authenticator_prompts_again_after_ticket_expires` to expose
`read_password` instead of the old show/clear pair. Record the final test count;
it must be at least 63.

### Milestone 2: Deepen `LiveDashboard` around one bounded screen

Change `LiveDashboard.__enter__` in `src/sup/display.py` to construct Rich
`Live` with `screen=True`, `auto_refresh=False`, and
`vertical_overflow="crop"`. `screen=True` is the load-bearing alternate-screen
ownership; crop is retained as a safe fallback but is redundant in that path.
Keep the current event-driven refresh throttle.
`LiveDashboard.__exit__` must reliably restore the original screen through
Rich's context cleanup.

Replace `jobs_view` and phase-separated `jobs_table` rendering with one focused
table containing one physical row per job: status signal, phase, job name,
trajectory, elapsed time, and exit code. Do not render the command or per-job
multi-line output in this table. Keep `render_dry_run` able to show commands by
giving it a non-live planning table rather than preserving a compatibility mode
inside the live table. Remove the outer live `Panel`, its vertical padding, and
the phase `Rule` rows from the real-run dashboard; retaining that chrome would
consume the height saved by compacting the job cells. Keep the dry-run and final
summary panels unchanged.

Concentrate height policy in internal helpers owned by the display module. Read
the current `self.console.size` for every render; given its height and the fixed
status/table chrome, calculate the job-row
budget and output-row budget. Select visible jobs by running, failed, queued,
skipped, then succeeded status and stable configuration order. When selection
omits jobs, reserve one job row for text such as `… 4 jobs hidden`; the message
must stay truthful even when the viewport omits queued work as well as completed
work.
Use all remaining rows, up to three, for a global `deque` of `(job_name, line)`
recent-output events. If no output row fits, omit the dock. Render the final
group without exceeding the declared console height; Rich cropping is the last
safety net, not the primary sizing mechanism.

Do not add an input-active flag, pending-refresh state, or a lock held across
input. Preparation ordering guarantees there is no concurrent producer while
the main thread is reading a password. If a future scheduler permits that race,
it must add a tested coordination mechanism as part of that scheduler change.

This milestone is complete when focused-layout and pre-crop viewport tests pass
at all declared dimensions, raw output contains alternate-screen controls and
zero relative cursor-up redraws, the named old-contract tests above are rewritten
for the focused view, and all summary and runner tests remain green.

Before moving credential input into the display, split job preparation from
command execution in `src/sup/runner.py`. An internal preparation method must
perform requirement checks, create the same skipped or failed `JobResult`, and
perform sudo preflight. `Runner.run` calls preparation directly for each core
job and executes a ready core job immediately. For the parallel phase it must
prepare the entire configured batch serially on the main thread, collect all
ready jobs, and only after the last preparation finishes submit all ready
command work to `ThreadPoolExecutor`. Preserve configured result order, each
job's status transitions, log paths, optional-job semantics, interrupt
cancellation, and process termination behavior; cross-job event interleaving is
allowed to change because preparation is now deliberately serialized. Do not
add a public `prepared` flag or a second runner mode; the sequencing stays
internal to the runner module.

This preparation change is complete when every sudo callback runs on the main
thread, ready commands still use worker threads, skipped preparation results
remain in configured result order with completed command results, and all
existing runner tests pass.

### Milestone 3: Put hidden input on the modal's password row

Add a `LiveDashboard.read_password(jobs, *, error=None, reader=None) -> str`
interface in `src/sup/display.py`. The method must set modal state, force one
synchronous render, locate the rendered cell immediately after `Password:`,
show and move the terminal cursor to that absolute viewport coordinate, flush
the console output, and only then invoke the injected reader or Python
`getpass.getpass` with an empty textual prompt. In a `finally` block, hide the
input cursor, clear modal state, and force one clean dashboard refresh. There is
no input-active guard that may suppress the modal's own render.

Keep modal positioning inside the viewport-aware renderable. Render and pad or
crop the base dashboard to the console's current height before centering the
overlay. Determine the password coordinate from the actual rendered overlay
segments rather than duplicating wrapping math in `src/sup/cli.py`. A failed
attempt may add an error line and change modal height, so every attempt must
render and locate the prompt anew.

Refactor `authenticate_sudo_with_overlay` in `src/sup/cli.py` to ask the
dashboard to read each password while retaining ticket checks, validation, and
the three-attempt policy. Remove the `SudoAuthenticator` lock because all
preflights are now main-thread serial operations. Preserve
an injected zero-argument password reader as a test seam by passing it through
to `LiveDashboard.read_password`; do not expose coordinates or terminal-control
objects to the authentication module. `read_password` owns modal cleanup, so do
not retain a public `clear_auth_overlay` method or duplicate cleanup in the CLI.
Catch `EOFError` from non-interactive `getpass` and treat it as unavailable
authentication without a traceback; allow `KeyboardInterrupt` to follow the
existing interrupt path.

This milestone is complete when retry tests show the error inside the same
centered modal, the cursor matches the rendered prompt cell, passwords remain
absent from logs and captured display output, EOF is handled cleanly, and
KeyboardInterrupt restores the terminal.

### Milestone 4: Exercise terminal controls without a real update

Create `tests/terminal_harness.py`. Use only standard-library pseudo-terminal
support, Rich, `LiveDashboard`, and synthetic jobs. The harness accepts
`--width`, `--height`, and a `--scenario` of `password`, `sigint`, or `sigterm`.
The password scenario runs several synthetic status/output refreshes and then
performs a real standard-library `getpass` read. Interrupt scenarios deliver the
named signal after alternate-screen entry and prove cleanup. No scenario invokes
sudo or a configured command.

Implement the harness as a parent/child process in one file. The parent opens a
pseudo-terminal, applies the requested dimensions with the standard terminal
window-size ioctl, starts the child, and captures the child's raw terminal
bytes. In the password scenario, the parent waits until terminal echo is
disabled, writes a fixed test secret to the PTY master, and verifies those bytes
never appear in child output. The child renders synthetic events and prints one
`SUP-HARNESS-FINAL` marker plus a JSON result only after leaving
`LiveDashboard`. Parse alternate-screen enter (`CSI ? 1049 h`), exit
(`CSI ? 1049 l`), home/absolute cursor moves, cursor show/hide controls, and
relative cursor-up (`CSI n A`) controls from the captured bytes. Strip styling
from the modal frame and independently calculate the display cell immediately
after `Password:`; compare it to the final absolute move before input. Do not
build a general terminal emulator.

Run the password scenario at 80x18 and 120x24, and interrupt scenarios at 80x18.
Each must exit zero. Raw output must enter the alternate screen before any live
frame, use home/absolute positioning with zero relative cursor-up redraw rows,
pair cursor and alternate-screen controls, exit the alternate screen before the
single final marker, and restore it on SIGINT and SIGTERM. The password scenario
must additionally prove exact prompt-cell placement and echo suppression. The
harness is not allowed to load `config.yaml` or execute a `Job.command`; its
construction must make that safety property obvious in review.

Use this stable JSON schema so the evidence is machine-checkable:

    {
      "alternate_screen_entered": true,
      "alternate_screen_exited": true,
      "budgeted_frame_fits": true,
      "cursor_controls_paired": true,
      "final_marker_count": 1,
      "height": 18,
      "password_cursor_matches_prompt": true,
      "password_echo_suppressed": true,
      "relative_cursor_up_rows": 0,
      "screen_exit_before_final": true,
      "scenario": "password",
      "width": 80
    }

`budgeted_frame_fits` comes from independently measuring the dashboard's
pre-crop renderable, not from counting Rich `Screen` rows. Unit tests must also
assert semantic content: an omitted-count row and running/failed priority rows
remain visible in a deliberately overfull frame. At height 24, a reduced-job
fixture must demonstrate the labeled output dock; the default 13-job layout at
height 18 is allowed to spend every spare row on job state and omit the dock.

### Milestone 5: Validate and review the completed behavior

Update this plan's living sections with test counts, harness evidence, any
terminal-library surprises, and the final outcome. Review `git diff` against
`decision.md`: no real update execution, no new dependency, no display policy in
`Runner`, no worker-thread terminal input, no terminal geometry in
`SudoAuthenticator`, and no unjustified
compatibility path for the old overflowing view.

Run the complete repository validation suite. Then perform the workflow's
recent-work review, formal review against `main` and this work item, and
adversarial implementation review. Fix verified findings and repeat affected
checks before setting `meta.json` to `stage="implementation"` and
`state="completed"`.

## Concrete Steps

Run every command from the repository root.

First run the new targeted tests during the red and green cycles:

    uv run python -m unittest \
      tests.test_sup.RunnerTest \
      tests.test_sup.DisplayTest \
      tests.test_sup.CliTest

Run the harmless terminal harness at both acceptance sizes:

    uv run python tests/terminal_harness.py --scenario password --width 80 --height 18
    uv run python tests/terminal_harness.py --scenario password --width 120 --height 24
    uv run python tests/terminal_harness.py --scenario sigint --width 80 --height 18
    uv run python tests/terminal_harness.py --scenario sigterm --width 80 --height 18

Each harness run must produce one JSON object with successful boolean fields and
exit zero using the schema in Milestone 4. Record the actual transcripts here
after the harness exists.

Actual harness evidence (2026-07-16), abbreviated to the dimension, scenario,
and non-obvious control assertions; every other schema boolean was also true:

    password 80x18: cursor_match=true echo_suppressed=true cursor_up_rows=0
    password 120x24: cursor_match=true echo_suppressed=true cursor_up_rows=0
    password 40x18: cursor_match=true echo_suppressed=true cursor_up_rows=0
    sigint 80x18: screen_exit_before_final=true cursor_controls_paired=true
    sigterm 80x18: screen_exit_before_final=true cursor_controls_paired=true

Run the repository-required validation commands:

    uvx ruff format --check .
    uvx ruff check .
    uv run python -m unittest discover -s tests
    uv run python -m py_compile src/sup/*.py tests/test_sup.py tests/terminal_harness.py
    git diff --check

Run the suite in an isolated Python 3.10 environment with the declared minimum
Rich release. `uv` may install the managed interpreter on first use:

    uv run --python 3.10 --isolated --with-editable . \
      --with 'rich==13.0.0' python -m unittest discover -s tests

Also run the safe CLI dry-run to verify command inspection remains available:

    uv run --project . sup --dry-run

Never run `sup` without `--dry-run` during this work. A real run can upgrade
local package managers and developer tools.

## Validation and Acceptance

The implementation is accepted only when all of the following behavior is
observable.

A forced terminal at heights 18, 24, and 36 produces a pre-crop renderable no
taller than its viewport. Repeated status and output updates use absolute
home/cursor positioning with zero relative cursor-up redraw rows and do not
append table copies. Enter and exit controls for the alternate screen are
paired, and the grouped final summary is printed on the normal screen exactly
once.

Changing a mutable test console from 120x24 to 80x18 between refreshes changes
the next frame's row budget and modal center without reconstructing the
dashboard. The raw terminal harness proves the same geometry at process level.

With the default configured jobs, the focused dashboard uses one line per job
when space permits and removes recent output before hiding important job state.
At height 24 with a reduced job fixture, the dock labels recent output by job.
With an artificially small row budget, running and failed jobs appear before
queued and completed jobs, configuration order remains stable within each
status, and one omitted-count row explains hidden jobs.

When sudo authentication is requested, the modal is centered against the
visible width and height. The terminal cursor used for hidden input is on the
exact rendered cell after `Password:`, not merely somewhere in the viewport or
after the dashboard. A real PTY/getpass read disables echo, and the fixed test
secret never appears in captured output. Invalid input displays the error in the
centered modal, and success, exhaustion, EOF, KeyboardInterrupt, SIGTERM, and
other exceptions restore cursor and screen state.

Core and parallel sudo preflights execute on the main Python thread. The entire
parallel batch is prepared before any ready command is submitted. Prepared
parallel commands still execute concurrently in worker threads, and preparation
failures preserve the existing optional skip versus required failure behavior,
per-job transitions, and configured result order.

All passwords remain absent from dashboard text, logs, failure reasons, harness
JSON, and test diagnostics. No new runtime dependency appears in
`pyproject.toml` or `uv.lock`. The full required validation suite, the Python
3.10/Rich 13 floor suite, all four planned harness runs, and the additional
narrow-width harness run exit zero. The unit-test count is 69 and no
old-contract test is silently deleted.

## Idempotence and Recovery

Unit tests, the harness, Ruff, compilation, and dry-run commands are safe to
repeat. They use synthetic jobs or `--dry-run` and must not modify package
managers. If a test leaves the terminal in alternate-screen mode, run
`reset` in that terminal and treat the cleanup failure as blocking; fix the
context cleanup before continuing.

If cursor-coordinate logic proves unreliable across forced consoles and the
pseudo-terminal harness, stop after two implementation hypotheses and revisit
the display architecture rather than adding terminal-specific branches. The
approved rollback is to revert the implementation commit; configuration and log
formats do not change.

## Artifacts and Notes

Baseline before implementation:

    .....................................................
    ----------------------------------------------------------------------
    Ran 53 tests in 0.153s

    OK

Root-cause probe before implementation:

    width=120 height=18 base_render_lines=42 cursor_up_rows_per_refresh=41
    width=120 height=24 base_render_lines=42 cursor_up_rows_per_refresh=41
    width=120 height=36 base_render_lines=42 cursor_up_rows_per_refresh=41
    width=80  height=18 base_render_lines=44

The local runtime used for source inspection is Python 3.14.3 with Rich 15.0.0.
The project remains compatible with its declared Python 3.10+ floor; do not use
3.14-only language syntax.

## Interfaces and Dependencies

The main interface stays `LiveDashboard` in `src/sup/display.py`:

    class LiveDashboard:
        def __enter__(self) -> "LiveDashboard": ...
        def __exit__(self, exc_type, exc, tb) -> None: ...
        def update(
            self,
            name: str,
            status: str,
            result: JobResult | None = None,
            output: str | None = None,
        ) -> None: ...
        def read_password(
            self,
            jobs: Iterable[Job],
            *,
            error: str | None = None,
            reader: Callable[[], str] | None = None,
        ) -> str: ...

This interface hides viewport budgeting, job selection, output-tail allocation,
Rich live-screen configuration, overlay geometry, and cursor placement.
`src/sup/cli.py` must not import Rich `Control`, terminal dimensions,
or display geometry helpers.

Internal display helpers should accept data and return renderables or selection
results so the interface is testable without a real update. Do not add a public
adapter when production and tests can both use `LiveDashboard` with different
`Console` and password-reader inputs.

Use only the existing runtime dependencies in `pyproject.toml`: PyYAML and Rich.
Use `getpass` and pseudo-terminal modules from the Python standard library.
Keep syntax and library calls compatible with Python 3.10 and Rich 13.0.

Plan revision note (2026-07-15): created the initial self-contained ExecPlan
from the approved terminal-dashboard decision and current repository evidence.

Plan revision note (2026-07-15, improvement pass 1): corrected the regression
commit provenance and made the compact-height mechanism explicit by removing
live-only panel and phase-rule chrome and using a status-neutral omission row.

Plan revision note (2026-07-15, improvement pass 2): traced password input into
the parallel executor and added a main-thread preparation seam so SIGINT and
terminal cleanup cannot leave a worker blocked in hidden input. Also required
unconditional modal cleanup around the authentication loop.

Plan revision note (2026-07-15, improvement pass 3): made resize proof and the
pseudo-terminal evidence concrete, including a parent/child capture design,
terminal controls to inspect, and a stable JSON acceptance schema.

Plan revision note (2026-07-15, adversarial resolution): verified the core
alternate-screen mechanism, replaced tautological frame-count evidence with
pre-crop and semantic assertions, added real PTY/getpass and signal cleanup
proof, enumerated old-contract test rewrites, added Python 3.10/Rich 13
validation, and removed unreachable lock/redraw-deferral scaffolding.
