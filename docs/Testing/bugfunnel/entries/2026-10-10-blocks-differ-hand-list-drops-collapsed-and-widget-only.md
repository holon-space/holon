---
id: 2026-10-10-blocks-differ-hand-list-drops-collapsed-and-widget-only
date: 2026-10-10
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A block whose only change was `collapsed` or `widget_only` never reached
  `block_raw` on the incremental Loro projection, because the projection's
  change gate `blocks_differ` compared a hand-picked field list that named
  neither.
---

## Bug

Folding or unfolding a block (or toggling `widget_only`) wrote the Loro
authority, but the incremental projection judged the block unchanged and
emitted no UPDATE, so `block_raw.collapsed` / `block_raw.widget_only` kept
their old values until some other field of the same block changed or a full
reseed ran.

Found by agent code-reading in lane `block-l1` (typed `block_type` slot,
2026-10-09): making `set_field(block_type)` reach SQL required adding
`block_type` to the gate, and the same read showed the two neighbouring
typed fields were missing too.

## Root cause

The incremental path emits an UPDATE only when
`blocks_differ(old, &nb)` is true
(`crates/holon-loro/src/loro_sync_controller.rs:1178`). `blocks_differ`
compared `sort_key`, `content`, `parent_id`, `content_type`, the source
fields, the edge fields, `properties` and `marks` — a list written by hand.
`collapsed` and `widget_only` are typed `Block` slots lifted out of the
properties bag by `read_block_from_tree`, so the `properties` comparison
did not cover them either. A change to only one of them was compare-and-
skipped. `block_diff_params` did emit both, so the loss was purely the gate.

The same class happened before: `BlockEventStorm.md` H12 (`requires`
omitted from the same gate, 2026-06-28). That fix closed it for edge fields
only, by iterating `EdgeField::ALL`.

`TursoSinkReader` (`crates/holon/src/storage/turso_sink_reader.rs:48-49`)
also did not select `collapsed`/`widget_only`/`block_type`, so the reseed
diff base read them as defaults.

## Missing piece

ORACLE. The keystone generates the interaction (`ToggleCollapse`,
`crates/holon-integration-tests/src/pbt/transitions/toggle_collapse.rs`),
but no invariant compares the projected `block_raw` row column by column
with the authority's `Block`, so a column the projection never updates
agrees with nothing and nothing goes red.

## Remedy

Fixed in lane `block-l1`:

- `blocks_differ` destructures `Block` without `..`
  (`crates/holon-loro/src/loro_sync_controller.rs:2998-3032`), so a new
  `Block` field does not compile until the gate names it; it now compares
  `collapsed`, `widget_only` and `block_type`.
- `TursoSinkReader` selects the three columns.
- `projection_totality_tests` in the same file lock that
  `block_to_params` / `block_diff_params` cover every declared column.

Open: the oracle gap itself — a Loro-vs-`block_raw` per-column
differential invariant in the keystone — is not built.
