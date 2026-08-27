---
id: 2026-08-27-cannot-move-root-block-stochastic-keystone-red
date: 2026-08-27
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A stochastic keystone red fails with "Cannot move root block" in the
  RehomeEntity / move-undo area — a block already AT the root sentinel has
  `parent_id() == None`, so every source-side move refuses it, while the same
  sentinel is a legal move DESTINATION.
---

## Bug

During the 2026-08-26/27 overnight session the nv2 lane's keystone draw went
red with the refusal string `Cannot move root block`, in the
`RehomeEntity` / move-undo area of the catalog (session evidence — the draw
log is the session's, not re-derived here).

A 24-draw population A/B across both trees showed **0 recurrence on either
tree**, so the red is pre-existing and stochastic, NOT the nv2 lane's
regression (session evidence).

It is not a documented known-red: `docs/Testing/KeystoneKnownReds.md` contains
neither "Cannot move" nor "root block" (verified by grep), and no other entry
under `docs/Testing/bugfunnel/entries/` records this signature — the only
prior mention is `2026-07-16-move-block-happily-reparents-page-under`, which
cites the same refusal only as the reason its own guard is unreachable for a
root page.

## Root cause

The root parent is an explicit STORED sentinel, `EntityUri::no_parent()` =
`sentinel:no_parent` (`crates/holon-api/src/entity_uri.rs:120`), never NULL —
per the standing ruling (`root-sentinel-not-null`, 2026-08-23), and
`move_block` accepts that sentinel as a DESTINATION (pinned by
`crates/holon-app/tests/move_block_to_root.rs:147`).

But the source side reads the parent through `BlockEntity::parent_id`, whose
`holon_api::block::Block` impl deliberately returns `None` for a non-block
parent — `self.parent_id.is_block().then_some(&self.parent_id)`
(`crates/holon-core/src/traits.rs:2735`). The sentinel is not a `block:` URI,
so a block sitting at root reports "no parent", and the three
`BlockOperations` chokepoints that need the old parent all bail:

- `move_up` — `crates/holon-core/src/traits.rs:2119`
- `move_down` — `crates/holon-core/src/traits.rs:2227`
- `move_block` (old-parent capture, no-prefetch arm) —
  `crates/holon-core/src/traits.rs:2403`

So the operation set is asymmetric: root is reachable as a destination, and
once reached the block can no longer be moved, re-ordered, or moved back — and
a move-undo that has to restore a block whose recorded previous placement was
root hits the same refusal.

Reachable transitions: `RehomeEntity`
(`crates/holon-integration-tests/src/pbt/transitions/rehome_entity.rs`, wired
into the composed catalog at
`crates/holon-integration-tests/src/pbt/composed/builder.rs:484,632`) performs
exactly this move-to-root — `move_block` to
`EntityUri::no_parent()`, `crates/holon-app/src/rehome_entity.rs:294` — after its own explicit at-root
refusal (`crates/holon-app/src/rehome_entity.rs:268-279`). Any later
move/reorder/undo drawn against that now-root block reaches the chokepoint
refusal.

## Missing piece

The catalog CAN generate the sequence — `RehomeEntity` moves a leaf to root and
later transitions move it again — so this is not a coverage gap. What is
missing is an oracle that names the intended contract for root-as-source: no
invariant in `crates/holon-integration-tests/src/pbt/composed/invariants/`
states whether a block at the root sentinel must be movable (making the
refusal a prod bug) or must be refused (making the refusal correct and the
transition's precondition wrong). Without that invariant the sequence surfaces
only as a raw stochastic error red, with no dedicated pin, no known-red row,
and no verdict on which side is wrong.

## Remedy

OPEN — nothing fixed. Next steps, in order:

1. Get the ruling: is a root-sentinel block movable? The stored-sentinel ruling
   says root is a real, addressable parent, which argues the three
   `parent_id()`-based bails are the defect, not the draw.
2. Given the ruling, either (a) route the three chokepoints through the stored
   sentinel instead of `parent_id()`'s block-only view, and add the invariant
   that a root block round-trips out and back; or (b) tighten the transition's
   precondition and add the invariant that a root block is refused loudly with
   a message naming the block.
3. Only then can this be classified as a keystone regression rather than a
   stochastic red; until then it must NOT be added to
   `docs/Testing/KeystoneKnownReds.md` as an accepted red.
