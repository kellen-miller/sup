Reviewer A completed too (its notification was just delayed). All three independent passes — my own empirical verification, Reviewer A, and Reviewer B — converge on the same core conclusions and add complementary findings. I've verified each surviving issue against the actual code and Rich 15.0.0 behavior. Here is the synthesized review.

---

# Adversarial Review — Terminal Dashboard Viewport packet

**Method:** three independent passes (two subagents + my own), each grounded in read-only execution against the real repo and Rich 15.0.0 pulled from the uv cache. Findings below survived my re-verification; I've dropped anything I couldn't reproduce.

## Verified sound — do not re-litigate these (stated so the not-defects are explicit)
- **Diagnosis is correct.** 13 default jobs → **42 base lines** at width 120/160 (44 at width 80); `vertical_overflow="visible"` records shape `(120, 42)` and `position_cursor()` emits **41** `ESC[1A` rows. Reproduced exactly.
- **The fix works.** `Live(screen=True, auto_refresh=False, vertical_overflow="crop")` on the default dashboard at 18 rows emits paired alt-screen controls (`?1049h`/`?1049l`), **zero relative cursor-ups** (uses `ESC[H` home), and self-crops to the viewport. The plan is fundamentally implementable.
- **Cleanup structure is sound.** `Live.stop()` restores screen + cursor inside a `finally`, so the `with LiveDashboard(...)` in `cli.py` covers normal/exception/KeyboardInterrupt paths structurally (the gap is *proof*, see M3 — not structure).
- Main-thread-preflight rationale is valid (Python signals only reach the main thread); `d21a894` provenance is correct; no new dependency; no 3.11+ syntax is mandated; `meta.json` stage/state is consistent with Progress.

No Critical issue exists. The serious issues are validation integrity, one spec self-contradiction, and now-dead scaffolding.

---

## HIGH

### H1 — The headline behavior (hidden, echo-suppressed input landing on the `Password:` row) is proven only with fakes; the real `getpass` path is executed by no test
**Artifact:** `execplan.md` L179–184, L288–294, L310–320; `cli.py` L166–168; `getpass.unix_getpass`
**Evidence:** `grep getpass tests/test_sup.py` → none; `grep console.input tests/…` → none; all four `authenticate*` calls inject `password_reader` (L663/702/765/807). The plan switches the production default from `console.input("", password=True)` to `getpass.getpass` (execplan L260–262) yet the harness "performs one fake password attempt with an injected reader" (L289). `getpass.unix_getpass` opens **`/dev/tty` as its own fd** — a *different* file object from the stdout the display writes the cursor-move to. The one novel real-terminal interaction — cursor-move-on-stdout flushed/ordered before `getpass` reads on `/dev/tty` — is exercised nowhere.
**Why it matters:** With echo off (getpass's actual job), cursor position is cosmetic; echo suppression is the property that protects the password, and it is untested. `password_cursor_in_viewport` is a bounds check — a reader reading at row 0, echo left on, or a `/dev/tty`-fallback all pass every stated check. The PTY harness is *capable* of the real proof but deliberately avoids it.
**Fix:** Add one PTY case that drives the **real** `read_password`/`getpass`, writing a password to the master side (no job command → still within the "synthetic proof" constraint), asserting (a) the typed bytes never appear in captured child output and (b) the cursor `(x,y)` equals the cell immediately after `Password:`.

### H2 — The harness's headline metric `max_frame_height ≤ height` is a tautology under `screen=True` and cannot fail even if the row-budget is broken
**Artifact:** `execplan.md` L294, L310–320, L226–227
**Evidence:** With `screen=True`, `Live.renderable` wraps content in `Screen(...)`, which crops to exactly the console height via `Segment.set_shape`. I rendered the current 42-line dashboard through that path at 18/24 → output is **exactly 18/24 lines**. So `max_frame_height ≤ height` is *always* true, independent of whether the internal budgeting works.
**Why it matters:** The metric measures the "last safety net" the plan explicitly says is *not* the mechanism under test. A lazy implementation that keeps the old 42-line layout and lets Rich clip it passes "bounded frames" while silently hiding jobs.
**Fix:** Assert on content (the omitted-count row text; that no priority job row is clipped) or measure the pre-crop budgeted renderable height and assert it already fits before Rich crops.

### H3 — Validation is gameable: the redesign silently invalidates ≥8 existing display/CLI test sites the plan never enumerates, and gives no post-change test count
**Artifact:** `execplan.md` L235–238 ("existing summary and runner tests remain green"), L447–466 (interface drops `show_auth_overlay`); `tests/test_sup.py`
**Evidence (grep-confirmed):** L1082 asserts `Live(..., vertical_overflow="visible")` (plan changes to screen+crop); L961–962 assert `"core phase"`/`"parallel phase"` (plan removes phase sections); L997 `assertIsNone(renderable.title)` (plan removes the Panel → `render()` no longer has `.title` → AttributeError); L1026/L1036 `test_sudo_auth_overlay_does_not_reflow_dashboard` centers against the full 160-wide base (plan re-centers on the visible viewport); **6** `show_auth_overlay` FakeDashboard/call sites (L652/692/753/795/1016/1036) that the new `read_password` interface breaks.
**Why it matters:** "Existing summary and runner tests remain green" is quietly true only by *omitting* display+CLI. The baseline is stated as "53 tests" with no target after the change, so a faithful implementer can't tell a correct rewrite from silently deleting inconvenient coverage — the exact "satisfy the text, not the intent" failure the review asks about.
**Fix:** Enumerate the specific tests to rewrite/remove and why; state the expected new count; require the rewritten geometry/overlay tests to *independently* locate the `Password:` row via rendered text rather than reusing the implementation's own geometry helper.

---

## MEDIUM

### M1 — Spec self-contradiction: Milestone 3 forces the modal render *after* marking input active, which Milestone 2 says suppresses refreshes → a literal implementation never draws the modal
**Artifact:** `execplan.md` L258–260 ("mark input active, force one synchronous render") vs L232–233 ("`update` must skip the physical refresh when input is active")
**Evidence:** Today a single `_refresh_live(force=...)` serves `update`, `show_auth_overlay`, and `clear_auth_overlay`. If the input-active guard is added there (the natural reading), setting active *before* the forced render suppresses the modal's own render; the cursor is then positioned against a frame that was never drawn.
**Why it matters:** A novice faithful to the written order breaks the primary feature while "satisfying" the plan. Force-vs-active precedence is unspecified.
**Fix:** Specify render-then-mark-active, or that suppression applies only to background `update()` calls (force must beat active). State the invariant.

### M2 — The reused unit-test seam doesn't reproduce viewport bounding *or* overlay centering; both depend on the live `Screen` path the unit tests never use
**Artifact:** `test_sup.py` L875–883 helper; `display.py` `OverlayRenderable` L304–321; `execplan.md` L162–166, L267–271
**Evidence:** For a forced console with `height=18`, `console.options.height` is **None** (only `max_height` set), so `console.print(dashboard.render())` emits **42 lines, uncropped** (reproduced), and `OverlayRenderable` centers against the natural content height, not the viewport. Cropping/padding-to-viewport happen only in the `screen=True` path.
**Why it matters:** Milestone 1's "frames never exceed terminal height" and "centered in the visible viewport" assertions, measured through this seam, validate against the wrong reference height — they can pass while the live behavior is wrong (or fail even after a correct fix).
**Fix:** Require `render()`/overlay geometry to size and pad from `self.console.height` independent of `options.height`; assert budgeted height ≤ H and overlay top offset relative to `console.height`. Give the existing `terminal_console` helper a `height` parameter (today it sets only width; default height is 25).

### M3 — Alt-screen restoration on interrupt is structurally sound but proven by no real-terminal test
**Artifact:** `cli.py` L93–106; `test_sup.py` L852–867; `execplan.md` L286–308 (harness runs only the clean path)
**Evidence:** The existing interrupt test patches `sup.cli.Runner` and redirects stdout to a StringIO; on a non-TTY `Console`, `Live` is a no-op, so `?1049h/l` is never emitted or checked. The PTY harness — the only real terminal — exercises only refresh → fake password → clean exit.
**Why it matters:** `screen=True` is new, and a stranded alternate screen on SIGTERM is exactly what the plan's own Idempotence section calls "blocking." Nothing catches it automatically.
**Fix:** Extend the PTY harness to deliver SIGINT/SIGTERM to the child mid-run and assert a paired `?1049l` before exit.

### M4 — The plan's own main-thread serialization makes the SudoAuthenticator lock *and* the redraw-deferral machinery dead weight, and renders one acceptance criterion unreachable in a real run
**Artifact:** `execplan.md` L229–233, L240–253, L395–398; `decision.md` L92–104; `cli.py` L132/138; `runner.py` L98–99
**Evidence:** With all preflight serialized on the main thread *before* any parallel command is submitted, no two `authenticate()` calls ever overlap (lock guards nothing) and no worker emits output while a prompt is active — so "Background output during input changes stored state without a refresh" can only be produced by manually calling `update()` behind a fake reader.
**Why it matters:** Three overlapping mechanisms (auth `Lock` + input-active flag + pending-refresh) all defend a race the plan itself eliminates — directly against the Ousterhout "fewer knobs" lens the plan invokes. The plan defends the deferral as future-proofing (L130–131); that's arguable, but as scoped it's unexercised complexity and an acceptance criterion that overstates what a real run demonstrates.
**Fix:** Drop the lock and/or the deferral, or explicitly mark them defensive with mechanism-only tests; reword the criterion as a synthetic invariant. Resolve jointly with M5.

### M5 — Load-bearing runner ordering is unspecified: "prepare each parallel job, then submit ready ones" doesn't say prepare-all-then-submit vs interleaved
**Artifact:** `execplan.md` L240–248
**Evidence:** The two readings differ materially: prepare-all-then-submit (no concurrent producer during a prompt — the premise M4 relies on) vs prepare-and-submit-each (earlier parallel jobs run and stream while a later job prompts → pipe-reader starvation risk, and the deferral genuinely needed). The plan leans toward the former ("submits only ready parallel command work") but never states it.
**Why it matters:** This determines correctness of both the concurrency claim and whether a slow human password stalls all parallel starts, yet the plan claims novice-implementability.
**Fix:** State explicitly: prepare all jobs (requirement checks + preflight) on the main thread, then submit all ready commands.

### M6 — "Python 3.10 compatible" is a hard constraint with zero executing gate
**Artifact:** `pyproject.toml` L6/L19–20; `.github/workflows/ruff.yml`; `execplan.md` L357–362, L441–443
**Evidence (verified):** CI runs **only** `ruff format --check` + `ruff check` — no `unittest`, no `py_compile`, no version matrix. Local runtime is 3.14; `py_compile`/unittest both run on 3.14. `ruff target-version=py310` catches some syntax but not 3.11+ stdlib APIs (`datetime.UTC`, `typing.Self`, `tomllib`, `ExceptionGroup`).
**Why it matters:** A stated hard constraint reduces to a manual "please avoid 3.14 syntax" note; a stray 3.11+ API in the new display/harness code ships silently to 3.10 users.
**Fix:** Add a 3.10 env to validation (`uv run --python 3.10 python -m py_compile …`/unittest) or a CI matrix job; state it in Milestone 5.

### M7 — At the primary 18-row acceptance size with the default 13 jobs, the "active-output dock" (a headline feature) is nearly absent, so "labels recent output by job" can't be demonstrated there
**Artifact:** `decision.md` L27–30, L57–59; `execplan.md` L387–391
**Evidence (projection):** Compact chrome (status line + meter + table header/rule ≈ 4) + 13 one-line job rows ≈ **17** lines, leaving ~0–1 rows of the "up-to-three" dock at height 18; the completed-job-collapse fires almost immediately.
**Why it matters:** The Validation section asserts the dock "with the default configured jobs" without naming a height; at 18 rows that acceptance essentially can't hold.
**Fix:** Demonstrate the dock at 24/36 rows or with a reduced job set, and state the height explicitly. Confirm the real compact layout height at 18×default before writing the acceptance.

---

## LOW

### L1 — The prescribed red/green command omits `RunnerTest`, where the main-thread-preflight tests (a hard constraint) must live
**Artifact:** `execplan.md` L342–346 (`DisplayTest`, `CliTest` only) vs L186–192 (new `threading.get_ident()` preflight tests). Test classes are `Selection/Log/Runner/Cli/Display`. **Fix:** add `tests.test_sup.RunnerTest` so the riskiest ownership change is exercised in the quick loop, not only in the final `discover`.

### L2 — The plan never names the load-bearing `screen=True` kwarg; `vertical_overflow="crop"` is in fact redundant under it
**Artifact:** `execplan.md` L201–205. **Evidence:** In screen mode Rich uses `Control.home()` (absolute); `crop` has no effect (verified 0 relative cursor-ups either way). A novice could implement `crop`-only, satisfying "no scrollback growth" **without** owning the alternate screen. **Fix:** name `Live(screen=True, auto_refresh=False)` and the exact constructor assertion; note `crop` is only a non-screen fallback.

### L3 — Root-cause evidence (42 lines / 41 up) is width-120; the harness's own primary acceptance width (80) renders 44 pre-fix
**Artifact:** `execplan.md` L58–60, L437–439 vs L350–351. The reproduction a novice runs at 80×18 won't match the quoted figures. **Fix:** derive the probe at 80×18 or state the width alongside the numbers.

### L4 — Main-thread serial prep emits all parallel prep skips/fails before any parallel `running`, changing cross-job event interleaving vs "preserve … status events"
**Artifact:** `execplan.md` L252, L401–404; `runner.py` L96–135. Correctness is unaffected (dashboard state is keyed by job name), but the claim is imprecise. **Fix:** reword to "preserve per-job transitions and final result order"; confirm no test asserts cross-job interleaving.

### L5 — Two minor cleanups
`clear_auth_overlay` stays public and is double-invoked (cli.py `finally` + `read_password`'s own `finally`) though `read_password` now owns the modal lifecycle (`display.py`/`execplan.md` L459–466) — make it internal or drop it. And non-interactive `getpass` raises `EOFError` → an uncaught traceback after screen restore (pre-existing with `console.input`, but omitted from the residual-risk list) — catch it and treat as auth-unavailable.

---

**Bottom line:** the core fix is verified feasible, but the plan should not proceed to implementation until the validation is made non-gameable (H2, H3), the headline hidden-input behavior gets a real end-to-end proof (H1), the Milestone-3 render/suppress contradiction is resolved (M1), and the test seam / runner ordering / 3.10 gate are pinned down (M2, M5, M6). These are inexpensive plan edits, not a redesign.

---ADVERSARIAL_REVIEW_STATUS---
ISSUES_FOUND: 15
CRITICAL_COUNT: 0
HIGH_COUNT: 3
MEDIUM_COUNT: 7
LOW_COUNT: 5
CONFIDENCE: HIGH
BLOCKING: true
SUMMARY: Core fix verified sound, but validation is gameable and the headline hidden-input behavior + one self-contradictory render/suppress step are unproven — revise the plan's acceptance before implementing.
---END_ADVERSARIAL_REVIEW_STATUS---
