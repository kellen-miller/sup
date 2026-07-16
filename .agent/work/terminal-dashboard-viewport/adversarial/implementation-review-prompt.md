Look at this again with fresh eyes.

You are an adversarial reviewer. You are not the author, and the author may have
conflicting goals when reviewing their own work. Your job is to find serious
problems in the implementation diff, tests, validation evidence, and rollout
notes for the `sup` terminal dashboard repair.

This is a read-only review. Do not modify files, write new files, apply patches,
change external systems, push branches, create PRs, or run commands that mutate
state. Use Bash only for inspection commands. Do not run `sup` without
`--dry-run`; a real run upgrades local package managers.

Repository and review surface:

- current repository root: the current working directory
- base commit: `2a4310f4f50ab30bef085102189e720f439104e1`
- review diff: `git diff 2a4310f4f50ab30bef085102189e720f439104e1...HEAD`
- commits: `96ac5bd`, `0fa05d0`, and `87f786c`
- intent: `.agent/work/terminal-dashboard-viewport/decision.md`
- executable contract: `.agent/work/terminal-dashboard-viewport/execplan.md`
- metadata: `.agent/work/terminal-dashboard-viewport/meta.json`
- prior planning review and resolution:
  `.agent/work/terminal-dashboard-viewport/adversarial/plan-review.md` and
  `.agent/work/terminal-dashboard-viewport/adversarial/plan-review-resolution.md`
- implementation: `src/sup/display.py`, `src/sup/cli.py`, and
  `src/sup/runner.py`
- tests and PTY evidence: `tests/test_sup.py` and
  `tests/terminal_harness.py`

The desired outcome is one bounded alternate-screen dashboard that redraws in
place; one physical row per visible job plus a spare-row output dock; a centered
sudo modal whose real hidden-input cursor lands immediately after the modal's
rendered `Password:` label; all job preparation and sudo preflight on the main
thread before parallel command submission; normal, EOF, exception, SIGINT, and
SIGTERM cleanup that restores cursor and screen before normal-screen output.

Constraints: preserve Python 3.10 and Rich 13.0, configured result order,
logging and optional-job semantics, dry-run command visibility, and existing
interruption behavior. Add no dependency, sudo keepalive, redraw-deferral state,
worker-thread prompt, command text in live rows, compatibility view, or
machine-specific path. Do not demand legacy overflow behavior or shims.

Fresh validation after formal-review fixes:

- Ruff formatting and lint: pass
- unit discovery: 65 tests pass
- Python 3.10 with Rich 13.0.0: 65 tests pass
- py_compile and `git diff --check`: pass
- `uv build`: pass
- safe `sup --dry-run`: pass and still renders commands
- PTY password at 80x18 and 120x24: exact prompt cursor, echo suppression,
  alternate-screen cleanup, independent pre-crop frame bound, zero relative
  cursor-up rows, final marker after screen exit
- PTY SIGINT and SIGTERM at 80x18: paired screen/cursor controls, zero relative
  cursor-up rows, final marker after screen exit

Formal review already found and the author fixed: cleanup when Rich entry is
interrupted after alternate-screen emission; a `Password:` decoy job name
confusing cursor discovery; contributor-specific paths; a cropped PTY
frame-height measurement; incomplete raw ordering assertions; and unused
dry-run table configurability. The author rejected printing a grouped job
summary after an interrupted run because no complete result set exists and the
pre-existing CLI contract is a dedicated interruption notice after cleanup.

You may inspect any relevant paths under `$HOME`, neighboring repositories,
documentation, web sources, browser-accessible pages, or MCP-backed resources
needed to verify or falsify the work. Do not stop at this packet when broader
evidence would materially improve the review.

If subagents are available, ask two independent subagents with filesystem read,
web fetch/search, browser, and MCP access to review this work. Tell them that
whoever finds the largest number of serious issues gets five points. Synthesize
only serious findings that survive your review. If subagents are unavailable,
run two independent review passes yourself before synthesizing.

Do not summarize the work. Challenge it. Report only issues that could change
the implementation, validation, or release decision. For each issue include
severity (critical/high/medium/low), path, evidence, why it matters, and a fix or
next check. Call out missing evidence, unchecked assumptions, unsafe terminal
state, concurrency gaps, input leakage, tautological tests, scope expansion,
and places where the code satisfies text but misses intent. Do not invent nits.

End with this exact status block:

---ADVERSARIAL_REVIEW_STATUS---
ISSUES_FOUND: <number>
CRITICAL_COUNT: <number>
HIGH_COUNT: <number>
MEDIUM_COUNT: <number>
LOW_COUNT: <number>
CONFIDENCE: HIGH | MEDIUM | LOW
BLOCKING: true | false
SUMMARY: <one line>
---END_ADVERSARIAL_REVIEW_STATUS---
