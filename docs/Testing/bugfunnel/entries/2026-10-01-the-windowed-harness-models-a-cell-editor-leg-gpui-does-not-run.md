---
id: 2026-10-01-the-windowed-harness-models-a-cell-editor-leg-gpui-does-not-run
date: 2026-10-01
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The windowed GPUI harness builds its reference from `Wiring::full()`, whose
  implied editor leg is Cell, so `editor_cell_attached()` is true there while
  GPUI commits every keystroke through the dispatch leg.
---

## Bug
Found by the verifier of the editor-leg-axis lane (`lane-logs/adm0b-verify.md`, O3). The
headless keystone now draws the editor leg (`EditorLeg`, `crates/holon-pbt-core/src/wiring.rs`)
independently of Loro. The windowed harness does not: it never sets `editor_leg`.

## Root cause
`crates/holon-integration-tests/src/pbt/window_slice/builders.rs` (`fresh_reference_state(
Wiring::full())`) takes the leg the storage implies: Cell, because Loro is wired. The windowed
SUT is real GPUI, and `frontends/gpui/src/di.rs` installs no block cell registry, so the SUT
runs the dispatch leg. The reference answers `RefLifecycle::editor_cell_attached()` true
(`crates/holon-integration-tests/src/pbt/ref_caps/boot.rs`) for a SUT that has no cell.

## Missing piece
The windowed wiring does not name the leg GPUI actually runs (`EditorLeg::Dispatch`).

## Remedy
Open. No windowed transition gates on `editor_cell_attached()` today (windowed slash input goes
through real GPUI keystrokes), so nothing reads the wrong answer yet. Set
`with_editor_leg(EditorLeg::Dispatch)` on the windowed wirings so the first gate that reads it
sees the production leg.
