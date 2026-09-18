---
id: 2026-09-18-navigate-focus-noop-click-has-no-emission
date: 2026-09-18
gap: FALSE-ALARM
secondary: COVERAGE
status: FIXED
summary: >-
  The TUI driver's post-click emission barrier demanded a CDC tick from a
  sidebar click that prod makes idempotent, so a `NavigateFocus` onto the
  region's already-current page red deterministically.
---

## Bug

`frontends/tui` `tui_ui_pbt` panics on a deterministic one-transition case —
`HOLON_PBT_WEIGHTS=NavigateFocus:2000 PBT_NUM_STEPS=1 PROPTEST_CASES=1` when the
draw picks `block:structural-page`, the page the windowed boot already focused:

```
[NavigationProvider] focus: region=main, block_id=Some("block:structural-page")
[NavigationProvider] focus: idempotent re-focus on Some("block:structural-page") — no-op
[SutFocusWrite::apply_navigate_focus] sidebar click_entity(left_sidebar,
  block:structural-page) failed: await_chord_settled: engine did not quiesce:
  emissions did not quiesce after dispatch — CDC pipeline stuck?: emissions did not
  advance past tick 1 within 2s
  — crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2881
```

Captured in `lane-logs/base3C-r19.log` and `lane-logs/base3C-r21.log` (load
65-136). The provider's own DEBUG line names the mechanism: the click's
`navigation.focus` was a no-op. Found by the `tui-novel-reds-triage` lane's
sweep as one of four load-correlated residuals, root-caused and fixed by the
`fix-navigate-noop` lane under ruling D144.a.

## Root cause

`NavigationProvider::focus` returns before writing anything when the region's
cursor already targets the block (`crates/holon/src/navigation/provider.rs`, the
`current_block == block_id` guard) — no history row, no closed row, no SQL
write. No write means no CDC emission, and the TUI driver's
`await_chord_settled` (`frontends/tui/src/user_driver.rs`) bars on exactly that
tick (`HeadlessInputRouter::wait_for_quiescence`,
`crates/holon-frontend/src/user_driver.rs`), so its precondition was
unsatisfiable by construction. Nothing in the product is wrong: an idempotent
re-focus emitting nothing is the intended behaviour, and the reference model
already recorded the no-op for the history rows.

## Missing piece

The driver rung had no model of the no-op: it treated every `click_entity` as
an emission-producing interaction, and the generator cannot exclude the
already-focused target from the drawn `NavigateFocus` set (the sidebar target
set is state-dependent).

## Remedy

FIXED. `apply_navigate_focus_via` reads `current_focus(main)` — the same view
its own post-click assertion checks — and skips the click and the barrier when
it already targets the block; `NavigateFocus::apply_to_ref` skips on the same
predicate so both sides record the no-op. Green: `lane-logs/greenA-r4.log`
(load 8.07) passes with the guard's
`[apply_navigate_focus] main already sits on the target — no click to drive
block_id=block:structural-page`. Teeth kept: with the emission observation
broken (`HOLON_PBT_QUIESCENCE_TIMEOUT_MS=1`) a REAL navigation still reds —
6 of 8 runs, `barrier=1` and `skipped=0`, on `block:forward-edge-page` /
`block:368857d2-…` (`lane-logs/teeth-r*.log`).
