---
id: 2026-09-18-inline-row-mount-reads-a-frame-the-harness-never-settled
date: 2026-09-18
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  `inv-inline-row-mount-present` judges the frame the window painted after a
  transition, at a point the harness has not settled, so a navigation-heavy
  alphabet reds it on a frame that is still 4 chrome elements.
---

## Bug

`frontends/tui` `tui_ui_pbt` panics with the composed catalog's
`inv-inline-row-mount-present`:

```
reconciled composed sequence diverged from the oracle:
[("inv-inline-row-mount-present", "[inv-inline-row-mount-present] rendered 4
element(s) but NONE is a `render_entity`-tagged `block:*` row — the window
painted no block through the production inline-row mount path …")]
  — crates/holon-integration-tests/src/pbt/composed/harness.rs:1390
```

Listed as a load-correlated residual by the `tui-novel-reds-triage` lane
(2 of its 36 unbiased runs). Re-measured by the `fix-navigate-noop` lane: it is
**navigation-correlated, not load-correlated** — the same binary at load 9.4-11.4
reds **21 of 24** runs under `HOLON_PBT_WEIGHTS=NavigateFocus:2000`
(`lane-logs/f1navA-summary.txt`, `lane-logs/f1navB-summary.txt`) while the
unbiased alphabet at the same load reds **0 of 12**
(`lane-logs/f3unb-summary.txt`). Every reding run has `skipped=0`: the guard
added by the D144.a fix did not fire, so those runs drove a real navigation and
took the unchanged click + barrier path.

## Root cause

Two reads of the same observable at different points of a transition. The
per-transition settle hook (`frontends/tui/tests/common/pbt_main.rs`) decides
"settled" from element-count stability alone and runs at the START of the next
transition's apply; `inv-inline-row-mount-present` is then checked immediately
after the current apply, on a frame the harness never established had been
repainted. After a page navigation the window's committed frame is 4 chrome
elements with no `render_entity` block row, and it stays that way while the
harness is idle: forcing the settle hook to demand the boot gate's row predicate
(`wait_for_geometry_ready` → `is_inline_row_tag`) did not merely wait — it
turned 12 of 24 runs at load 124-250 into `settle hook never reached a fixed
point` panics with a 5s cap and 14 of 20 with a 30s cap, i.e. no row-carrying
frame arrived for 30s (`lane-logs/navpostA|B-summary.txt`,
`lane-logs/navfinA|B-summary.txt`). That experiment is reverted; the refuted
predicate is documented at the hook.

## Missing piece

The harness has no settle between a transition's apply and
`ComposedSut::check_invariants`, and the readiness predicate it does have
(`is_inline_row_tag`) is applied only at boot and by the invariant — never at
the point where the frame is adjudicated. Whether prod's own window repaints its
rows after a page navigation without further CDC stimulus is NOT established by
these runs: `all_elements()` reflects the last committed frame, and no run
logged the renderer's own paint cadence.

## Remedy

OPEN. The naive fix (strengthen the settle hook's predicate) is measured to be
worse than the red it removes. The remaining candidates, in order: settle
between apply and `check_invariants` for the windowed slices; or instrument the
TUI renderer to log its frame cadence after a navigation and decide whether the
row-less frame is a harness read-point artefact or a real repaint gap.
