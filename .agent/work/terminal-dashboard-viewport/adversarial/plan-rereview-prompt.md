Look at this revised planning packet again with fresh eyes.

You are an adversarial reviewer. This is the mandatory second pass after the
first review found 3 high, 7 medium, and 5 low issues. Read the original review
and the author's resolution in addition to the planning packet:

- .agent/work/terminal-dashboard-viewport/adversarial/plan-review.md
- .agent/work/terminal-dashboard-viewport/adversarial/plan-review-resolution.md
- .agent/PLANS.md
- .agent/work/terminal-dashboard-viewport/decision.md
- .agent/work/terminal-dashboard-viewport/meta.json
- .agent/work/terminal-dashboard-viewport/execplan.md
- src/sup/display.py
- src/sup/cli.py
- src/sup/runner.py
- src/sup/jobs.py
- tests/test_sup.py
- pyproject.toml
- AGENTS.md

Goal: make sup's Rich live dashboard own a bounded alternate terminal screen,
redraw without scrollback growth, and accept hidden sudo input inside a modal
centered in the visible viewport.

Hard constraints: no real update run; no new runtime dependency; Runner owns
execution, LiveDashboard owns terminal presentation and input position, and
SudoAuthenticator owns sudo policy; the entire parallel batch must complete
main-thread preparation before ready commands enter the worker pool; old visible
overflow and multi-line live rows are intentionally not preserved; the
implementation must remain compatible with Python 3.10 and Rich 13.0.

This is a read-only review. Do not modify files, write new files, apply patches,
change external systems, push branches, create PRs, or run commands that mutate
state. Use Bash only for inspection commands.

Verify whether every original finding is genuinely resolved and look for new
serious contradictions introduced by the fixes. Pay special attention to whether
the PTY/getpass evidence is implementable without executing a real update, the
pre-crop budget is independently testable, interrupt cleanup is observable, the
test-count rule is non-gameable, and the simplified no-lock/no-deferral design is
consistent with the exact runner ordering. Do not repeat a finding that the
revised text actually resolves.

An earlier attempt to rendezvous with background subagents hit the Claude CLI's
10-minute print-mode ceiling and returned no review. Do not spawn background
tasks or subagents in this pass. Run two independent review passes yourself,
then synthesize only findings that survive your own verification.

Report only issues that could still change the plan, implementation, validation,
or release decision. For each issue include severity, artifact or path, evidence,
why it matters, and a suggested fix or next check.

End with exactly this status block:

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
