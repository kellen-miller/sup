# Adversarial Plan Review Resolution

The plan review reported 3 high, 7 medium, and 5 low findings. Every finding is
resolved in `decision.md` and `execplan.md`; none is accepted as residual debt.

## High findings

- H1 resolved: the PTY harness now drives real standard-library `getpass`, waits
  for terminal echo to disable before writing the test secret, proves the secret
  is not echoed, and compares the absolute cursor move with an independently
  located `Password:` cell.
- H2 resolved: viewport acceptance measures the pre-crop renderable and semantic
  row selection. Rich `Screen` height is no longer treated as evidence of a
  correct budget.
- H3 resolved: all old-contract tests and CLI fakes that must be rewritten are
  named. The final suite must contain at least 63 tests, and existing methods may
  not be silently deleted.

## Medium findings

- M1 resolved: `read_password` renders the modal before reading and has no
  input-active guard that can suppress that render.
- M2 resolved: forced consoles carry explicit height, direct rendering reads the
  live console size, and overlay assertions are relative to that height.
- M3 resolved: PTY scenarios deliver SIGINT and SIGTERM and require paired
  alternate-screen cleanup.
- M4 resolved: redraw deferral and the authentication lock are removed because
  the selected scheduler makes their race unreachable.
- M5 resolved: the runner must prepare the entire parallel batch before
  submitting any ready command.
- M6 resolved: validation executes the full suite on Python 3.10 with Rich
  13.0.0 in an isolated uv environment.
- M7 resolved: the output dock is demonstrated at height 24 with a reduced job
  fixture; height 18 may spend every spare row on job state.

## Low findings

- L1 resolved: `RunnerTest` is part of the targeted red/green command.
- L2 resolved: the exact `screen=True` Rich constructor contract is named and
  distinguished from the redundant crop fallback.
- L3 resolved: root-cause evidence states width 120 for 42/41 and records the
  44-line width-80 observation.
- L4 resolved: the plan preserves per-job transitions and configured result
  order, not old cross-job event interleaving.
- L5 resolved: `read_password` owns modal cleanup, the public clear method is
  removed, and EOF is handled as unavailable authentication without a traceback.
