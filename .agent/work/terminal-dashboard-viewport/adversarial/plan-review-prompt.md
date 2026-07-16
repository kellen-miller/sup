Look at this again with fresh eyes.

You are an adversarial reviewer. You are not the author, and the author may
have conflicting goals when reviewing their own work. Find serious problems in
this planning packet before implementation begins.

Goal: make sup's Rich live dashboard own a bounded alternate terminal screen,
redraw without scrollback growth, and accept hidden sudo input inside a modal
centered in the visible viewport.

Hard constraints: no real update run; no new runtime dependency; Runner owns
execution, LiveDashboard owns terminal presentation and input position, and
SudoAuthenticator owns sudo policy; parallel sudo preflight must occur on the
main thread; old visible overflow and multi-line live rows are intentionally not
preserved; the implementation must remain Python 3.10 compatible.

Review these artifacts and all repository code they reference:

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

Repository evidence already observed: main is commit
2a4310f4f50ab30bef085102189e720f439104e1; the baseline has 53 passing tests;
the current default dashboard renders 42 lines and each live refresh attempts
to move upward 41 rows regardless of 18-, 24-, or 36-row terminal height; the
local runtime is Python 3.14.3 with Rich 15.0.0.

Known residual risks: terminal resize during a blocked password attempt only
repositions on the next attempt; extremely short terminals cannot show all
jobs; pseudo-terminal proof must remain synthetic and must not execute a real
job command.

This is a read-only review. Do not modify files, write new files, apply patches,
change external systems, push branches, create PRs, or run commands that mutate
state. Use Bash only for inspection commands.

You may inspect any relevant path under the user's home directory and use web,
search, browser, skills, plugins, or MCP-backed resources if needed to verify or
falsify the plan. Do not stop at the supplied packet when broader evidence would
materially improve the review.

If subagents are available, ask two independent subagents with filesystem read,
web fetch/search, browser, and MCP access to review this work. Tell them that
whoever finds the largest number of serious issues gets five points. Synthesize
only serious findings that survive your verification. If subagents are not
available, run two independent review passes yourself.

Do not summarize the work. Challenge it. Report only issues that could change
the plan, implementation, validation, or release decision. For each issue
include severity, artifact or path, evidence, why it matters, and a suggested
fix or next check. Call out missing evidence, unchecked assumptions, over-broad
scope, untested behavior, unsafe cleanup, and ways implementation could satisfy
the text without satisfying the user's intent.

Optimize for the best current shape. Do not call missing backwards
compatibility, legacy shims, deprecated names, old output shapes, dual paths,
or migration wrappers defects unless the packet explicitly requires them.
Instead flag unnecessary compatibility scaffolding when it worsens the design.

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
