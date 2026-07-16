Confirmed — job A (instant command, no sudo) reports **elapsed=0.406s**, entirely absorbed from job B's 0.4s preflight running on the main thread during serial prep. In the old worker-side model A would show ~0s. Reviewer A's new finding is real.

All three passes (mine, A, B) have now converged, so here is the **final consolidated synthesis** (supersedes the interim status block above). Reviewer A returned late but corroborated the two robustness issues a third time and added the `elapsed` drift; both reviewers independently rated the too-short crash MEDIUM, so I elevate it.

## Consolidated findings (each reproduced by me)

**M1 — Sudo prompt on a ≤6-row terminal aborts the entire run with an uncaught `RuntimeError`.** `display.py:373` → `_password_cursor_position` (`:208`) → `read_password` (`:195`), before the reader; escapes `authenticate_sudo_with_overlay` (`EOFError`-only, `cli.py:172`), `runner.run` (`KeyboardInterrupt`-only, `:144`), and `main` (`KeyboardInterrupt`-only, `cli.py:100`). PTY `--width 80 --height 6` → `child_exit_code=1`, `final_marker_count=0`, echo never disabled; ≤6 crash, ≥7 OK. On a short pane with no cached ticket the first core sudo job (`brew-upgrade`) crashes → **no jobs run**, traceback printed (screen *is* restored — not stranded). The dashboard has a graceful omitted-row fallback for short terminals; the modal has none. All three passes found this; A and B rated MEDIUM. *Fix: catch at `cli.py:166` → treat as auth-unavailable, or clamp the cursor in-viewport.*

**L1 — Height budget isn't the bounding mechanism below ~50–58 cols.** `focused_dashboard` (`display.py:478,501`) assumes the status subtitle (`:489`) and mission meter (`:490`) are one row each, but both wrap: 40×18 → 19–21 pre-crop lines (`options.height` is `None`, so a genuine measurement); the harness's own `budgeted_frame_fits` returns `False` at width 40. Benign — `vertical_overflow="crop"` bounds it, cursor stays exact (verified 38–58), no scrollback/crash — but it falsifies the "budget is the mechanism" claim and can silently truncate the dock / under-report "N jobs hidden" at narrow widths the sanctioned 80/120/160 runs never exercise. *Fix: `no_wrap` the status/meter or fold their wrapped height into the budget; add a narrow-width harness case.*

**L2 — Parallel `elapsed` now includes the serial prepare/auth wait (telemetry drift).** `_prepare` stamps `started` on the main thread (`runner.py:169`); `_execute` computes `elapsed` later in a worker (`:215`). Since all parallel prepares (incl. a blocking `read_password`) finish before any `_execute`, early-prepared jobs' displayed time is inflated by the whole prep window. Reproduced: an instant non-sudo job shows **0.406s** next to a job with a 0.4s preflight. Cosmetic (only the "time" column/summary); worst case is a trivial job showing the human's full password-entry time (e.g. `sup --only mas`). Arguably inherent to the approved prepare-all-then-submit design, but a real change from the old worker-side stamping. *Fix (if undesired): stamp execution start in `_execute` and keep prep time separate.*

**L3 — Cursor *unit* test proves self-consistency, not correctness.** `test_sup.py:1246-1296` derives "expected" from the same render pipeline that emits the move; real independent proof is only the PTY harness. Coverage caveat, not a bug.

**L4 — Harness `password_cursor_matches_prompt` is vacuously true when prompt and move are both absent** (`terminal_harness.py:271` → `None == None`). Backstopped today by the `child_exit_code`/`final_marker_count` guards (confirmed at height 6). *Fix: assert both are non-`None` before comparing.*

## Verified sound (challenged, survived — all three passes agree)
Non-tautological pre-crop budget; real `getpass` with proven echo suppression and exact cursor placement across 198 geometries; correct cleanup on normal/EOF/exception/interrupted-entry/SIGINT/SIGTERM (one paired `?1049l`, no stranding, screen restored before all normal-screen output); no worker runs during any prompt; result order/optional-skip/interrupt-stop preserved; no leakage; no scope violations; 65 tests pass on 3.14/Rich15 **and** 3.10/Rich13 with no base test deleted. (Only `uv build` I did not independently re-run — packaging-only, low risk.) Nothing here blocks release.

---ADVERSARIAL_REVIEW_STATUS---
ISSUES_FOUND: 5
CRITICAL_COUNT: 0
HIGH_COUNT: 0
MEDIUM_COUNT: 1
LOW_COUNT: 4
CONFIDENCE: HIGH
BLOCKING: false
SUMMARY: Core repair verified sound and non-gameable across three independent passes; only a ≤6-row modal crash (MEDIUM) plus narrow-width budget, elapsed-drift, and two test-integrity caveats (LOW) remain — none release-blocking.
---END_ADVERSARIAL_REVIEW_STATUS---
