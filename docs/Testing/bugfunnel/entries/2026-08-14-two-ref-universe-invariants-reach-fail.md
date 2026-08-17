---
id: 2026-08-14-two-ref-universe-invariants-reach-fail
date: 2026-08-14
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Two ref-universe invariants reach a FAIL verdict against a
  deliberately-empty reference universe, so
  `gpui_window_slice::capmap_hosts_windowed_sutlayout_over_real_geometry`
  cannot pass however healthy the window is.
source_line: 702
---

## Bug

(task-#28 windowed-slice-revive lane; re-triage of a red logged 2026-08-09
as merely "pre-existing") **Two ref-universe invariants reach a FAIL verdict
against a deliberately-empty reference universe, so
`gpui_window_slice::capmap_hosts_windowed_sutlayout_over_real_geometry`
cannot pass however healthy the window is.** `gpui_window_slice.rs:282`
(current tree; logs read `:284`/`:281` pre-`#[ignore]`) asserts no failures
over a stage that intentionally uses `window_ref_caps()` — documented as an
"honestly-empty" oracle that knows none of the vault's blocks, on the
premise that `inv-displayed-text` SKIPS unknowns.
`inv-viewmodel-entity-ids-subset-of-data` and
`inv-matview-consistent-with-ref/root_layout` do not skip: with `Ref-known
block IDs (0)` they report the three layout panels plus `journals` as
phantoms and `block:root-layout` as an IVM ghost.

## Root cause

task-#28 windowed-slice-revive lane, re-triage of a red first noted on
2026-08-09 (row below) as merely "pre-existing": **two ref-universe
invariants reach a FAIL verdict against a deliberately-EMPTY reference
universe, so
`gpui_window_slice::capmap_hosts_windowed_sutlayout_over_real_geometry`
cannot pass however healthy the window is.** At
`frontends/gpui/tests/gpui_window_slice.rs:282` (current tree; the run logs
read `:284`/`:281`, taken before and between the `#[ignore]` attributes that
shifted the line) the test asserts `report.failures().is_empty()` over a
stage that intentionally uses `window_ref_caps()` — documented at
`crates/holon-integration-tests/src/pbt/window_slice/builders.rs:289-303` as
an "honestly-empty" oracle that "deliberately knows none of the booted
vault's blocks", the point being that `inv-displayed-text` SKIPS unknown
blocks. `inv-viewmodel-entity-ids-subset-of-data` and
`inv-matview-consistent-with-ref/root_layout` do not skip: they classify
everything outside the ref's block set as a phantom/ghost, so they report
`Missing: [default-left-sidebar, default-main-panel, default-right-sidebar,
e5f42b7b-…, journals]` with `Ref-known block IDs (0): {}`, and `IVM MATVIEW
GHOST ROW DETECTED … reference model: 0 known ids, extra in matview:
[block:root-layout]` (verbatim `lane-logs/wslice-reactor-fix.log:953`).
Neither is a real phantom or a real ghost — the phantom invariant's own
header
(`crates/holon-integration-tests/src/pbt/invariants/bodies/viewmodel_entity_ids_subset_of_data.rs:13-17`)
states that the layout containers "are real seeded blocks … so subtracting
the ref-known block set makes the check layout-agnostic", which presupposes
a ref that knows them. ORACLE: the applicability gate is one condition
short. Both invariants gate on a non-empty rendered-tree set and a non-empty
query-data set, but NOT on a non-empty ref-known set, so an empty ref
universe produces a verdict that is vacuously false in the FAILING direction
— the mirror image of the vacuous-pass the whole engagement-floor machinery
exists to prevent. NOT FIXED — left `#[ignore]`d pointing here. The remedy
is a ref whose block UNIVERSE is read from `block_raw` at boot (ids only,
never content), which keeps both invariants' teeth exactly — a fabricated id
with no backing block still fails, a stale matview row pointing at a deleted
block still fails — while removing the vacuous arm; the A1 lane's
`window_ref_caps_journal_feed` already seeds `default-main-panel` +
`journals` for precisely this reason, so the convention exists and
`window_ref_caps()` simply never followed it. Deliberately not written blind
in this lane: it changes a shared ref builder and needs its own red/green.)

## Missing piece

Both invariants gate on a non-empty rendered-tree set and a non-empty
query-data set, but NOT on a non-empty ref-known set — so an empty ref
universe yields a verdict that is vacuously false in the FAILING direction,
the mirror image of the vacuous pass the engagement-floor machinery guards
against.

## Remedy

NOT FIXED — left `#[ignore]`d pointing at this row. Remedy: a ref whose
block UNIVERSE is read from `block_raw` at boot (ids only, never content),
which keeps both invariants' teeth (a fabricated id, or a matview row for a
deleted block, still fails) while removing the vacuous arm;
`window_ref_caps_journal_feed` already seeds `default-main-panel` +
`journals` for this reason, so the convention exists and `window_ref_caps()`
never followed it. Changes a shared ref builder — needs its own red/green,
deliberately not written blind here.
