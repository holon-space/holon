---
id: 2026-08-16-page-switch-rendered-accordion-must-direct
date: 2026-08-16
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Every page switch rendered "accordion must be a direct child of a main-panel
  column".
source_line: 689
---

## Bug

(task #18 lane-accordion; found by Martin dogfooding a fresh vault with
GPUI) **Every page switch rendered "accordion must be a direct child of a
main-panel column".** GPUI's split predicate `has_accordion_child` requires
the panel tree's ROOT to be literally a `column`; since `dae2cd2c` the main
panel lost its `collection_view()` marker, so the backend auto-wraps it in
the query-source `view_mode_switcher` and the accordion falls through to the
fail-loud error widget.

## Root cause

task #18 lane-accordion, found by Martin dogfooding a fresh vault with GPUI
on `/Users/martin/Workspaces/pkm/holon-pkm/` — every page switch showed the
error widget **"accordion must be a direct child of a main-panel column"**:
GPUI's flow-panel split predicate `has_accordion_child`
(`frontends/gpui/src/render/builders/column.rs:39`, called from
`views/reactive_shell.rs:794`) requires the panel tree's ROOT to be
literally a `column`. Since `dae2cd2c` (2026-08-15) the main panel's render
source lost its `collection_view()` marker, so `block_domain.rs:177-200` now
auto-wraps it in the query-source `view_mode_switcher` — the shape BOTH
sidebars already had. Root is a `view_mode_switcher`, predicate false, split
never fires, accordion falls through to `accordion.rs:126`'s fail-loud error
widget. PERCEPTION, not coverage: the covering windowed test
`seeded_accordion_panel_smoke` DID go red at exactly the right commit and
sat unread for ~24h because no gate executes GPUI windowed tests
(`precommit`/`prepush`/`landing-gate` only typecheck them) — the signal was
produced and never read. Gate omission itself is task #22, separate.
ENVIRONMENT secondary: even un-redded, that test fed `parse_render_dsl`
output straight to the builders and never applied the backend wrap, so its
root was a bare `column` and it could not have reproduced the break. FIXED
(D28 arm a): `vms_slot_accordion_column` resolves one slot through a
`view_mode_switcher` root and
`view_mode_switcher::render_accordion_split_slot` runs the same split on the
slot column with the mode bar overlaid; the smoke test now calls the REAL
`BlockDomain::wrap_in_query_source_switcher` so its topology equals
production's, and went red for the right reason first (no `live_query`
inside the accordion = the childless error widget). 5 predicate unit tests
pin the negatives, incl. the sidebar firewall.)

## Missing piece

The covering windowed test went red at the right commit and nothing read it
— no gate EXECUTES GPUI windowed tests (task #22). Secondarily, that test
never applied the backend wrap, so its root topology was not production's.

## Remedy

FIXED — `vms_slot_accordion_column` +
`view_mode_switcher::render_accordion_split_slot` teach the split to see
through a switcher root (D28 arm a); `seeded_accordion_panel_smoke` now
calls the real `wrap_in_query_source_switcher` and asserts the accordion
wraps its `live_query` (the error widget is childless); 5 predicate unit
tests in `column.rs` pin the negatives.
