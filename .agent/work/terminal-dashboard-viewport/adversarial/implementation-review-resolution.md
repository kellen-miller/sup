# Adversarial Implementation Review Resolution

The implementation review reported one medium and four low findings. Each was
verified against the repository before disposition. The review contained no
critical or high finding, so the adversarial-review workflow does not require a
second Claude pass after these bounded fixes.

## M1: Short-terminal password prompt abort

Accepted and fixed. `OverlayRenderable.password_cursor_position` now raises
`EOFError` when the password cell cannot fit in the viewport. The existing sudo
authentication boundary treats that as unavailable input, allowing `Runner` to
preserve required-job failure and optional-job skip behavior without a
traceback. `test_sudo_overlay_treats_invisible_prompt_as_unavailable` covers an
80x6 viewport and verifies the validator is never called.

## L1: Narrow terminal height budget

Accepted and fixed. The dashboard status line and mission meter now use Rich
`Text` with `no_wrap=True` and ellipsis overflow, matching the one-row budget
already reserved for each. `test_live_dashboard_budgets_narrow_viewports`
proves the pre-crop render fits at 40x18, 30x12, and one-, two-, and three-row
heights. A 504-geometry sweep from 20 to 120 columns and one to 24 rows found
zero pre-crop overflow cases.

## L2: Parallel elapsed time includes preparation

Accepted and fixed. `_PreparedJob` no longer records a preparation timestamp;
`Runner._execute` starts elapsed timing immediately before the running update
and command execution. `test_parallel_elapsed_excludes_serial_preflight_time`
uses a deterministic clock to prove a 100-second preflight wait is excluded
while the one-second command interval remains.

## L3: Cursor unit test shares the render pipeline

Accepted as a coverage note, with no production change. The unit test checks
the display helper's internal consistency, while the independent PTY byte
oracle remains the authoritative cursor-placement proof. The implementation
review independently verified exact placement across 198 terminal geometries.

## L4: Vacuous PTY cursor match

Accepted and fixed. The harness now requires both a discovered `Password:` cell
and an absolute cursor move before comparing them.
`test_missing_password_prompt_does_not_match_missing_cursor_move` proves two
missing values no longer pass.

## Validation after resolution

The four focused regression tests failed before the implementation changes and
pass afterward. The full local suite now passes 69 tests, Ruff formatting and
linting pass, and `git diff --check` is clean. The complete floor, PTY, build,
compile, and safe dry-run validation remains recorded in the ExecPlan closeout.
