---
id: 2026-11-24-quick-open-window-focus-red-under-load
date: 2026-11-24
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The windowed quick-open focus test reports that the seated row's editor has no window focus when the host is heavily loaded, on a tree whose change cannot reach the GUI.
---

## Bug
- Found by the landing gate (windowed-gate step), not by a test of the changed code.
- `frontends/gpui/tests/quick_open_returns_focus_windowed.rs:500` (`enter_navigating_out_of_quick_open_seats_a_caret`): "the seated row's editor must hold WINDOW focus too". It was red on the first run and on the gate's own rerun.
  - The rerun also failed `typing_into_a_slot_creates_the_node_before_the_write` (`:705`).
- The 1-min host load was up to 70 on 16 cores during the run.
- The tree under test changed only one gate-script test in Python. The same test passed one hour earlier on the same main at a lower load.

## Root cause
- Not found. The same test passed when the gate re-ran it at a lower load (1-min load about 20). The leading hypothesis is that, under heavy load, window focus arrives later than the test waits for, or another window takes it.

## Missing piece
- The windowed gate does not tell a focus or timing failure under load from a product defect, and the test has no load-independent wait for window focus.

## Remedy
- Open. Reproduce at high load (A/B: idle host against a host loaded to above 50) before you change the test or the product.
