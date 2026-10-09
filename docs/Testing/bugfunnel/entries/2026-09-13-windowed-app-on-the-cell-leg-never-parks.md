---
id: 2026-09-13-windowed-app-on-the-cell-leg-never-parks
date: 2026-09-13
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
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

**A self-deadlock on the GPUI thread, not a busy run loop.** The editor's
remote-delta task (`frontends/gpui/src/views/editor_view.rs`, the
`cell.remote_deltas()` loop in `EditorView::new`) built its directive in one
statement:

```rust
let directive = this.controller.lock().unwrap()
    .remote_converge_directive()
    .map(|d| ConvergeDirective { target: this.project_authority(&d.target), ..d });
```

The `MutexGuard` temporary lives until the end of the `let`, and
`project_authority` locks the same `std::sync::Mutex<EditorViewModel>` again
inside the `.map` closure. The second lock never returns. The path runs only
when a cell is attached (`cell_for_remote` is `Some`, so only the editor-cell
leg) and a remote delta leaves the view-model buffer behind the cell
(`remote_converge_directive` returns `Some`). Depending on when the first such
delta arrives, the stall shows at the boot barrier or after the undo.

Measured, `lane-logs/park/` in the `cell-undo-park` lane:

- `repro1.log`: `task_keyword_blur_windowed …_loro` with
  `enable_block_cell_registry()` exits 124 at 300 s with 5 log lines. The first
  30 s `settle` never returns, so one `run_until_parked` call blocks.
- `sample3.txt`: the process is at 0.3 % CPU. The only `__psynch_mutexwait` in
  the process is on the test (GPUI) thread:
  `pump_cycle → HeadlessAppContext::run_until_parked → TestScheduler::tick →
  EditorView::new remote-delta task → EditorView::project_authority →
  Mutex<EditorViewModel>::lock`.

An earlier sample showed the main loop "in `run_until_parked`" and was read as
"nothing is blocked". The frame under it was a mutex wait.

## Missing piece

The defect is in GPUI view code (`EditorView`), and the keystone PBT is
headless: its editor mirror never runs `EditorView`, so it cannot reach the
lock. That makes the primary gap ENVIRONMENT. The windowed fixtures can reach
it, but none of them booted the editor-cell leg, so the only combination that
fails was one that nothing ran (secondary COVERAGE). No assertion bounds "the
app reaches a parked state", so a deadlock of this kind shows up only as a
test timeout.

## Exposure

**No user is affected today.** The GPUI application boots the on-blur leg in
production (D113.a); only these test fixtures opt into the cell registry. The
defect blocks turning the cell leg on, which is increment 5 of the cell-undo
lane (D115.A) and is that increment's stated entry condition.

## Remedy

FIXED. Bind the directive in its own statement so that the guard drops before
`project_authority` locks again (`editor_view.rs`, the `remote_delta`
directive). The windowed `_loro` arms of `task_keyword_blur_windowed` and
`undo_survives_blur_windowed` now boot the editor-cell leg
(`env.enable_block_cell_registry()`). They pass in about 20 s with the fix and
hang (exit 124 at 240 s) without it (`teeth-*.log`, `green-*.log`).

The general park bound is still open (see Missing piece).
