---
id: 2026-10-21-windowed-clickblock-finds-no-bounds-under-load
date: 2026-10-21
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Under load the windowed driver clicks a block whose bounds the frame it took as
  settled has not registered yet, and the click gesture test fails with no product
  defect shown.
---

## Bug
`holon-gpui::gpui_compose_sut_windowed windowed_composed_sut_drives_a_click_gesture_sequence_green`
fails with `[ClickBlock] no registered bounds for block:c1 in the settled frame`
(`crates/holon-integration-tests/src/pbt/driver_input.rs:302`). A landing gate
met it in `just windowed-gate`, which runs up to four windowed binaries at once.
It reproduces at the base without the landing lane's change, at 1-min load
38-43, and passes alone at 1-min load 10.6. Registry row:
`windowed-clickblock-bounds-under-load`.

## Root cause
Not found. `DriverInputComponent::require_bounds` checks the bounds once. It
assumes the harness pumped the window to a fixed point that registered the
target's bounds. Under load the frame it reads does not hold them yet. Two
hypotheses are open:
- The harness fixed point is reached before GPUI paints the frame that
  registers the target, so the pump's "settled" is too early.
- The target's element is rendered in a later frame for a reason only load
  exposes, for example a deferred layout or a pre-warm timeout.

## Missing piece
Litmus answers:
- ENVIRONMENT: the failing path is the windowed driver's bounds check, which
  runs only in the windowed wiring, and only at a load the windowed gate makes.
- COVERAGE: the keystone can generate the click; the gap is the timing, not the
  interaction.

No headless rung models paint timing, and no windowed run judges the settle
point under load.

## Remedy
Open. Registered as a pass-with-note known red so the windowed gate does not
re-triage it. To close it, make the windowed fixed point wait for the
target's bounds, or show that the frame is final and the target is missing,
which would be a product defect.
