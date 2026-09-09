# Verification — lane `quick-open-focus`

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/quick-open-focus`
(TREE-OK; `jj diff -r @ --stat` = 3 files / 377 insertions, identical before and
after every probe). Logs: `.../scratchpad/quick-open-focus-verify-logs/`.
All undo done with `cp` + `shasum -a 256` (search_ui.rs and the invariant body
both back to their original digests, verified).

## Claim 1 — red for the right reason — CONFIRMED
- Baseline: `01-baseline-green.log:PASS ... closing_quick_open_hands_keyboard_focus_back`,
  `Summary [6.465s] 1 test run: 1 passed`.
- Reverted ONLY the hand-back inside `close` (`self.restore_focus.take()` /
  `window.focus`), keeping the signature: `02-red-prefix.log:177` FAIL, panic at
  `quick_open_returns_focus_windowed.rs:157` with
  `Fail("... window-focused editor(s) in the committed frame = [], engine block's
  editable_text mounted: true ...")` — the engine-vs-window mismatch, not a harness
  error.
- Restored (sha `eace08dc…` matches) and the test is green again (later runs).

## Claim 2 — a Skipped outcome fails the test — CONFIRMED
- The assertion is `assert!(matches!(outcome, InvariantResult::Ok))`
  (`quick_open_returns_focus_windowed.rs:157-160`); the invariant body only maps
  `Skip("")` → `Ok` when `window.len()==1 && window[0]==engine_id`
  (`window_focus_matches_engine_focus.rs:116-117,143`).
- Artificial skip injected at the top of `check` (`return Skipped("ARTIFICIAL SKIP
  PROBE")`): `03-skip-probe.log:198` — FAIL, `... must measure and pass, got
  Skipped("ARTIFICIAL SKIP PROBE")`. Restored (sha `94230eaf…`).
- Note on the task's suggestion: removing a cap from `Needs` would NOT deselect
  (selection is a subset check against the supplied CapMap), so the skip had to be
  forced in the body.

## Claim 3 — behavioural probes — MIXED
Scratch test `frontends/gpui/tests/zz_verify_probe_quick_open_focus.rs`
(deleted afterwards; `ls | grep -c zz_verify` = 0, `jj diff --stat` unchanged).

(a) CONFIRMED — `06-probes.log:192-198`: baseline `window_focused =
["block:chord-target"]`; cmd-K; typed `c h o` into the overlay; Escape →
`window_focused = ["block:chord-target"]`; typed `z` consumed; SQL shows
`content = "zChord target row"` — the char landed in editor A at caret 0, not
elsewhere.

(d) CONFIRMED (no stale-handle hazard) — `06-probes.log:199-211`: two open→Escape
rounds both restore `["block:chord-target"]`. Deleting the focused block while the
overlay is open then pressing Escape: no panic, modal closes
(`PROBE-D2: escape after delete = Ok(())`).

(b) REFUTED as stated — `06-probes.log:183-191`. With no focused editor,
`restore_focus` is `None`, so after Escape `window_focused = []` and a typed `y`
is never consumed within 3s. Focus does NOT go to the caret block or main panel.
CONTROL: the same keystroke was already unconsumed BEFORE the overlay was opened
(`PROBE-B CONTROL: typed 'w' ... never consumed`), so this is not a regression —
but the claim "focus goes to the caret block / main panel and a keystroke is not
swallowed" is false.

(c) REFUTED — `10-probes-c4.log:199-204`. Overlay opened over the focused row,
typed `chord`, pressed Enter (`enter = Ok(())`): modal closes, but
`window_focused = []` while `engine focus = Some(EntityUri("block:chord-target"))`,
and a typed `v` is NOT consumed within 5s. After the navigation the target renders
as a plain `text` widget (no `editable_text`), so the oracle SKIPS
(`PROBE-C: focus invariant after enter = Skipped("... has no mounted
editable_text")`). The state is unrecoverable by the driver's own click-to-focus:
the next `send_key_chord` fails with `block:chord-target's editable_text never took
window focus within 2s of click-to-focus` (`10-probes-c4.log:206`).
The lane report's sentence "on the navigating paths the target's own focus binding
fires afterwards and takes focus from here, so the hand-back is the same one rule on
every path" is not true in the windowed tier. (Same symptom pre-fix; a GAP, not a
regression.)

## Claim 4 — click-away — CONFIRMED as a GAP, but the stated REASON is wrong
- No test covers quick-open click-away: only `window_chord_reentrant_dispatch.rs`
  and the new test reference `search_modal_open`; nothing drives
  `search_ui.rs:482`'s `on_mouse_down_out`. GAP confirmed.
- The lane's reason is factually wrong. `SimUserDriver::click_entity`
  (`frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs:413-433`) resolves a
  center from bounds and then calls `raw_click`, which dispatches a REAL
  `gpui::MouseDownEvent` + `MouseUpEvent` at that point (same file, 225-253). The
  capability the report says is missing already exists; `frontends/gpui/tests/
  settings_integrations_setfield_popup_windowed.rs:475-493` drives
  `on_mouse_down_out` today with a raw click at arbitrary coordinates
  (`click_at(app, window, inert, ...)`).

## Claim 5 — gates — CONFIRMED
- `cargo fmt --check`: clean (`01-baseline-green.log`, `--- FMT-OK ---`).
- Windowed binary, positional filter: `01-baseline-green.log`,
  `Summary [6.465s] 1 test run: 1 passed`.
- `just keystone-smoke`: `11-keystone-smoke.log`,
  `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
- `cargo nextest run --no-fail-fast -p holon-frontend -p holon-app`:
  `12-frontend-app.log:843`, `Summary [78.619s] 758 tests run: 758 passed
  (1 slow), 1 skipped` — empty failure set, trivially a subset of the allowed set.
- Bugfunnel entry `2026-09-03-closing-quick-open-never-returns-keyboard-focus.md`:
  `status: OPEN` → `status: FIXED`, names
  `closing_quick_open_hands_keyboard_focus_back`.
  `/usr/bin/python3 scripts/bugfunnel.py check` → `648 entries, 0 problems`.

## DEFECTS / GAPS
1. Enter-navigates close path leaves NO window focus and swallows keystrokes
   (probe (c) above). The `close` hand-back does not cover it, the covering
   oracle SKIPS there (target loses its `editable_text` after navigation), and the
   lane report states the opposite as fact.
2. Opening/closing the overlay with nothing focused leaves focus nowhere
   (`restore_focus = None`). Not a regression, but the fix is a no-op on that path.
3. Click-away (`on_mouse_down_out`) close is untested for quick-open. The lane's
   justification ("click_entity … not a real mouse-down at a point") is wrong;
   `raw_click`/`click_at` is a real mouse-down and is already used for this exact
   purpose elsewhere in the tree.
4. Deleting the focused block while the overlay is open then pressing Escape leaves
   `engine.focused_block() = Some(block:chord-target)` for a block that no longer
   exists, with `window_focused = []` and keystrokes dropped (no panic).
5. The tracked bugfunnel entry cites `lane-logs/red-quickopen-focus.log` and
   `lane-logs/green-quickopen-final.log`, which are UNTRACKED in this workspace
   (`jj file list lane-logs` shows only `models-sha256-baseline.txt`) — the
   references dangle once this lands.
6. The second assertion of the covering test (`send_raw_keystroke_until_handled`)
   proves only that gpui CONSUMED the keystroke, not where it landed; the
   destination is pinned solely by the `window_focused` equality above it.

# Rev 2 re-verification

Same workspace; tree restored after every probe (`jj diff -r @ --stat` = 6 files /
812 insertions before and after; `search_ui.rs` sha `ee815ca2…`, invariant body
sha `94230eaf…`). Logs `r2-*` in the same log dir.

## Rev-2 Claim 1 — click-away red-first — CONFIRMED
- Baseline: `r2-01-baseline.log:175-180` — `--- FMT-OK ---` and
  `Summary [31.236s] 4 tests run: 4 passed, 0 skipped`.
- `SimUserDriver::click_point` (`sim_windowed_replay.rs:217-222`) calls
  `raw_click`, which dispatches a real `gpui::MouseDownEvent` + `MouseUpEvent`
  (225-253). The harness file is UNMODIFIED by this lane
  (`jj diff -r @ --stat frontends/gpui/tests/pbt_harness/` = 0 files).
- Hand-back removed from `close` (cp + sha, restored to `ee815ca2…`):
  `r2-02-red.log:175,192` — `FAIL … clicking_away_from_quick_open_hands_keyboard_focus_back`
  panicking at line 318 with
  `inv-window-focus-matches-engine-focus: Fail("… window-focused editor(s) in the
  committed frame = [], engine block's editable_text mounted: true …")`.
  Escape rung fails the same way (198,215). Restored → 4/4 green (r2-01, and
  again in r2-03 for the two hand-back rungs).
- The tracked fixture log `docs/Testing/fixture-logs-2026-09-08/quick-open-focus-red.txt`
  matches my independent red exactly (2 FAIL / 2 PASS, same rung split).

## Rev-2 Claim 2 — the two guards — CONFIRMED
- They cannot go red today: in the hand-back-removed run
  (`r2-02-red.log:221-222`) `enter_navigating_out_of_quick_open_leaves_no_zombie_focus`
  and `quick_open_round_trip_without_a_focused_editor_is_neutral` both PASS. The
  lane states this itself; it is a guard, not a pin.
- Not vacuous:
  * every rung routes the oracle through `Fixture::focus_invariant`, which
    asserts `!matches!(outcome, InvariantResult::Fail(_))` (test file 318-320) —
    demonstrated firing in the red run.
  * exact-skip guard: I mutated only the skip string in the invariant body
    (`"engine focus {engine_id} has no mounted editable_text"` →
    `"… DIFFERENT SKIP REASON PROBE"`). `r2-03-skipreason.log:178,195` — the Enter
    rung FAILS: `the skip must name the missing destination editor, got "[…]
    engine focus block:chord-target DIFFERENT SKIP REASON PROBE"`. Body restored
    (sha `94230eaf…`).
  * the Enter rung additionally asserts `window_focused().is_empty()` and
    `engine.focused_block() == destination`; the no-focus rung asserts
    `before == after` on keystroke consumption plus `focused_block() == None` and
    an empty `window_focused()` — real zombie tripwires, though `before` is
    `false` today so that equality reduces to "must stay false".

## Rev-2 Claim 3 — entry citations and scope — CONFIRMED
- `2026-09-03-closing-quick-open-never-returns-keyboard-focus.md` cites no
  `lane-logs/*` path and no screenshot; it names the four test functions and
  `docs/Testing/fixture-logs-2026-09-08/quick-open-focus-red.txt`, which is
  TRACKED in this change (`jj diff -r @ --stat`). The dogfood transcript is now
  quoted inline.
- Scope is stated honestly: the "Fixed" section names Escape and click-away as
  the hand-back rungs and labels the other two as different states; "## Still
  open" says the hand-back is a no-op on the Enter and no-focus paths and points
  at `2026-09-08-quick-open-enter-navigation-leaves-no-editable-focus`.

## Rev-2 Claim 4 — the two OPEN entries — CONFIRMED
Both carry the skill's frontmatter (`id` = filename stem, `date`, `gap`,
`secondary`, `status: OPEN`, one-sentence `summary`) and the four required
sections (Bug / Root cause / Missing piece / Remedy), with `file:line` citations
(`block_profile.yaml:71-74`, `reactive.rs`) and named evidence.
`scripts/bugfunnel.py check` → `650 entries, 0 problems`.
One factual correction available to the lane: the dangle entry's open question
("did the probe's delete travel `dispatch_intent`?") is answerable — my probe
used `TestEnvironment::delete_block`, which goes through
`test_ctx().execute_op("block", "delete", …)` (`test_environment.rs:2447-2456`),
i.e. the BACKEND op, not the frontend dispatch path. That supports the entry's
"missing out-of-band channel" hypothesis.

## Rev-2 Claim 5 — gates — CONFIRMED
- `cargo fmt --all -- --check`: `r2-01-baseline.log:2` `--- FMT-OK ---`.
- Windowed binary: `r2-01-baseline.log:180` `Summary [31.236s] 4 tests run:
  4 passed, 0 skipped`.
- `just keystone-smoke`: `r2-04-gates.log:202` `test result: ok. 4 passed;
  0 failed; 0 ignored; 0 measured; 0 filtered out`.
- `-p holon-frontend -p holon-app`: `r2-04-gates.log:1045` `Summary [50.431s]
  758 tests run: 758 passed (1 slow), 1 skipped` — empty failure set.
- `bugfunnel.py check`: `650 entries, 0 problems`.

## Rev-2 remaining gaps (no remedies)
1. Enter-navigate is still a live product defect (keystrokes dropped after a
   quick-open jump); only its absence-of-zombie is pinned.
2. Two of the four rungs cannot go red against the current implementation.
3. `2026-09-08-engine-focus-dangles-on-deleted-block` is unrooted; the deletion
   route is now known (backend op, see above) but the entry does not record it.
4. The keystone open/close TRANSITION and Gherkin vocabulary are still absent.
