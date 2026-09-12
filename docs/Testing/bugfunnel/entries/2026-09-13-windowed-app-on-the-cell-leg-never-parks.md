---
id: 2026-09-13-windowed-app-on-the-cell-leg-never-parks
date: 2026-09-13
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A windowed GPUI app booted on the editor-cell leg never reaches a parked
  state, so `settle`'s repeated 30 s budgets exhaust the test timeout.
---

## Bug

Two different windowed fixtures, booted with `env.enable_block_cell_registry()`
(the editor-cell leg), stop making progress and are killed at the 120 s nextest
cap. One `cargo test` run without that cap was still going at 420 s, so this is
not slowness.

The symptom is the same in both, at two different moments:

- `frontends/gpui/tests/task_keyword_blur_windowed.rs`
  `promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_loro` stalls
  **before any keystroke**, at the boot barrier:
  `[keyword-blur-boot] rows not both painted yet; 64 elements` and
  `[GPUI] pre-warm timeout — window will open with loading state`. On the old
  leg the same test passes in 7.7 s.
  Logs: `lane-logs/hang-baseline-1789264561.log` (old leg, pass),
  `lane-logs/hang-cellleg-1789264812.log` (cell leg, 120 s timeout with a
  shortened 6 s settle budget).
- `frontends/gpui/tests/undo_survives_blur_windowed.rs`
  `undone_content_edit_survives_clicking_another_row_loro` stalls **after** the
  undo has fully landed. Both SqlOnly arms pass in ~7.6 s in the same run.
  Log: `lane-logs/inc2-windowed-green2-1789284326.log`.

Found by the `cell-undo` lane (D115.A increment 2) while flipping the headline
windowed fixture to the cell leg — i.e. outside any automated test, by an agent
driving the fixtures.

## Root cause

**Not established. This entry records the SYMPTOM.** What the measurements rule
out is recorded here so the next probe does not repeat them.

Ruled out, each by direct measurement rather than reasoning:

| Candidate | Evidence against |
|---|---|
| The undo itself blocks | Instrumenting the four stages of `TextUndo::undo` shows all of them complete on a run that then times out (`lane-logs/probe-win-1789283026.log`) |
| A lock deadlock | A `sample` of the hung process shows every thread parked and the main loop in `run_until_parked` (`lane-logs/sample-1789266513.txt`) |
| Re-entrancy on the undo manager's mutex | Making every read path `try_lock` and panic on re-entry produced 2 timeouts and 0 panics over 8 runs (`lane-logs/h1-1789282712.log`) |
| The document write guard | Removing it from the undo path does not change the hang |
| Cell-leg delta delivery to the editor | Three headless pins pass, including the undo shape (`crates/holon-app/tests/cell_leg_delta_delivery.rs`) |
| The GPUI delta subscription | Instrumenting `frontends/gpui/src/views/editor_view.rs:685-694` shows the undo's delta waking the task and running through to the window update, on a run that then times out |

So the delta arrives and is applied, and nothing is blocked. What remains is
that the run loop never goes idle: `settle` (which waits for
`run_until_parked`) keeps finding work, and its repeated 30 s budgets add up
past the test cap.

**Candidate next probe:** find what keeps the run loop busy on the cell leg — a
re-render loop, a stream that never terminates, or a timer that re-arms.
Instrument `run_until_parked`'s pending-task set, or count renders per settle,
rather than instrumenting the CRDT side again (the table above shows that side
is clean).

## Missing piece

No gate runs the windowed fixtures on the editor-cell leg, and no assertion
anywhere bounds "the app reaches a parked state". The cell leg is exercised
headlessly (where there is no run loop to park) and the windowed fixtures are
exercised on the on-blur leg, so the combination that fails is the one
combination nothing runs. The keystone PBT cannot reproduce it: it is headless
and has no GPUI run loop, which is what makes this ENVIRONMENT rather than
COVERAGE or ORACLE.

## Exposure

**No user is affected today.** The GPUI application boots the on-blur leg in
production (D113.a); only these test fixtures opt into the cell registry. The
defect blocks turning the cell leg on, which is increment 5 of the cell-undo
lane (D115.A) and is that increment's stated entry condition.

## Remedy

Open. Increment 2 of the cell-undo lane is complete headless and its headline
windowed rung stays on the on-blur leg until this is fixed; the fixture is
untouched in the landed tree. Related: risk row 1 of
`~/.claude/plans/cell-undo-design.md`, which predicted the
`task_keyword_blur_windowed` hang, and
`2026-09-11-cmd-z-restores-nothing-after-typing-through-the-editor-cell`, whose
symptom this blocks the windowed proof of.
