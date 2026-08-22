---
id: 2026-08-22-loro-create-projection-drops-fold-state
date: 2026-08-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The Loro→SQL outbound CREATE projection never emitted `collapsed` /
  `widget_only`, so any block born folded reached `block_raw` with the fold
  state cleared — UI toggles hid it, because those take the UPDATE path.
---

## Bug

`loro_sync_controller::block_to_params` — the outbound projection that writes a
newly created Loro node into `block_raw` — emitted neither `collapsed` nor
`widget_only`. A block that is born folded therefore lands in SQL with
`collapsed = 0`.

The function DOES flatten `block.properties` onto the params, and the Loro
authority DOES keep both fields in the node's property map, so this looks like
it should work. It cannot: `read_block_from_tree`
(`holon-loro/src/loro_backend.rs:519-530`) has already REMOVED both keys from
`properties` and lifted them into the typed `Block` slots before
`block_to_params` ever sees the block. The flatten is structurally incapable of
carrying them, and nothing emitted them explicitly.

Found in lane `collapsed-bug` while fixing
[2026-08-22-org-ingest-drops-collapsed-into-property-bag](2026-08-22-org-ingest-drops-collapsed-into-property-bag.md).
That bug's fix made the ingest hand the fold state to the Loro authority
correctly — and the composed test STAYED RED at `Integer(0)`, which is what
exposed this second, independent drop underneath it. Recorded separately
because it is a distinct production surface: it loses fold state for ANY
Loro-authored create, not only an org import.

## Root cause

`block_to_params` (`holon-loro/src/loro_sync_controller.rs:1632`) omitted the
two typed scalar fields from the params it builds.

**Why it hid for so long:** `block_diff_params`, forty lines below in the same
file, DOES emit the pair (`if old.collapsed != new.collapsed { … }`). That is
the UPDATE path, which is what a user folding a block in the UI takes. So the
observable behaviour was: toggling a fold persisted correctly, and only a block
that was already folded at the moment of creation lost it. The one everyday
gesture anyone would reach for to check this field exercised the working half.

## Missing piece

COVERAGE. Applying the litmus — "is there a transition sequence in the current
catalog+wiring that reaches this state?" — the answer is no:

* `CreateBlock` / `SplitBlock` mint blocks born UNFOLDED, so their projection
  goes through `block_to_params` with `collapsed = false`, which the missing
  emit rendered indistinguishable from correct.
* `ToggleCollapse` mutates an EXISTING block, so it routes to
  `block_diff_params` — the half that worked.
* `SimulateRestart` is a file-touch org re-ingest, NOT a Loro re-projection
  (`transitions/simulate_restart.rs` says so explicitly), so it does not
  re-enter `block_to_params` either.

No sequence in the catalog creates a block that is already folded. The only
production path that does is an org ingest of a `:COLLAPSED:` file — the very
scenario the companion entry records as missing.

NOT classified as a secondary ORACLE gap, deliberately: `inv-blocks-match-ref`
already compares `collapsed` field-by-field, so an invariant WOULD have gone red
the moment any case reached this state. The oracle was adequate; only the
generator could not get there. Marking ORACLE too would overstate the gap and
mis-steer the investment this ledger exists to steer.

## Remedy

FIXED in lane `collapsed-bug`.

`block_to_params` now emits both fields explicitly, and ALWAYS rather than
only-when-set, so unfolding clears the column instead of leaving the last `true`
pinned. The comment at the call site names why the `block.properties` flatten
below cannot cover them, so the next reader does not delete the emit as
redundant.

Red → green evidence:

* `structural_pbt.rs::org_ingest_collapsed_marker_reaches_block_raw` — the
  composed red that stayed red after the ingest-side fix landed:
  `an authored ':COLLAPSED: t' must reach block_raw.collapsed — got Integer(0)`
  (lane-logs/green-composed.log, the run where drop 1 was already fixed).
* `loro_sync_controller.rs::block_to_params_emits_the_typed_fold_fields_on_create`
  — a pure-function regression that pins this drop INDEPENDENTLY of org ingest:
  a `SnapshotBlock` with `collapsed = true` must project `Boolean(true)`, and an
  unfolded one must project `Boolean(false)` rather than omitting the key. It
  sits beside the existing `block_to_params_emits_marks_when_present` /
  `_omits_marks_when_none` pair, which is the contract-test shape this field
  family was missing.
