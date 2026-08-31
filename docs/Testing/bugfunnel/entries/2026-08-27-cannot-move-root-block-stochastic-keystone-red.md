---
id: 2026-08-27-cannot-move-root-block-stochastic-keystone-red
date: 2026-08-27
gap: ORACLE
secondary: null
status: PARTIAL
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

**Caught again and made deterministic (2026-08-31).** The linkedrefs-hide
lane's keystone-smoke drew the same red and shrank it to three transitions —
`BulkExternalAdd` under the journals page `block:61133fe7-…`, `RehomeEntity`
on the leaf `block:bulk-0-1`, `UndoLastMutation`
(`.claude/worktrees/linkedrefs-hide/lane-logs/gates4-2.log:933`). That
sequence is now the hand-authored pin
`undo-of-a-rehome-moves-the-leaf-off-the-root`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`),
which reproduces the failure on the first replay with the byte-identical
message: `undo failed: … undo/redo replay of 'move_block' failed: Cannot move
root block` (RED `lane-logs/undo-root-red-1.log:248`, GREEN after the fix
`lane-logs/undo-root-green-1.log`).

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
transition's precondition wrong). Without that invariant the sequence surfaced
only as a raw stochastic error red — twice, five days apart, at a draw rate low
enough that a 24-draw A/B saw it zero times.

The pin closes that: the reachable case no longer depends on the draw.

## Remedy

PARTIAL — the undo half is fixed, the reorder half is not.

**Fixed.** Remedy (a) for `move_block`, the shared reparenting chokepoint.
`BlockEntity` gained `stored_parent()` — the parent as stored, sentinel
included — and `move_block_prefetched`'s no-prefetch arm reads the old parent
through it instead of through `parent_id()`'s block-only view
(`crates/holon-core/src/traits.rs`). A block at the root now moves off it, so
`rehome_entity`'s recorded inverse (`move_block` back to the old parent,
`crates/holon-app/src/rehome_entity.rs:288-311`) is executable and a re-home is
no longer a one-way door. A parent that is neither a block nor the sentinel is
still refused, now by a message naming the block and the parent it hangs off.

The verdict this settles: root-as-source is LEGAL. It follows from the standing
`root-sentinel-not-null` ruling (root is a real, addressable parent) plus the
undo contract — an operation whose inverse cannot run is not undoable, and
`rehome_entity` is in the undoable catalog.

**Also fixed — the toothless guard the removed bail was hiding.** Adversarial
verification found that lifting the bail let a PAGE at the root move under a
NON-PAGE, landing the topology the no-pages-under-non-pages guard
(`traits.rs:2459-2482`, interim ruling 2026-07-13) exists to prevent. The guard
was not bypassed; it was fed a lie. Both of its inputs came from `get_by_id`,
which decodes through the derived `TryFromEntity` and defaults every
`#[edge_field]` to empty — so a stored `Page` reads back as a non-page and
`moved_is_page` was ALWAYS false. Measured, not inferred: on the base tree a
NON-root page was already movable under a non-page, so the hole pre-existed and
removing the bail only widened it to the root-level population (exactly the
org-file pages).

Fixed at the input for both populations, with no root special-case: both
`moved_is_page` (`traits.rs:2423`) and `parent_is_page` (`traits.rs:2455`) now
read `is_page_authoritative`, which the SQL store overrides to read the
`block_tags` write authority and whose own doc already required it for
page-boundary guards. The destination's EXISTENCE still comes from `get_by_id`
— an absent parent has no tags either, so the authoritative read alone would
answer "not a page" instead of refusing the move.

Pinned by two rungs in `crates/holon-app/tests/move_block_to_root.rs`, each
asserting its fixture's page-ness against the stored `tags` first so a green
cannot come from a vacuous fixture:
`a_root_level_page_is_refused_under_a_non_page` (the placement this change
unmasked) and `a_nested_page_is_refused_under_a_non_page` (the pre-existing
hole). Both RED for the right reason first — "moving a page under a NON-page
was ACCEPTED — the prohibited topology landed in the store"
(`lane-logs/undo-root-guard-red2.log`) — GREEN after
(`lane-logs/undo-root-guard-green.log`).

Latent sharp edge, noted not fixed: `EntityUri::is_no_parent()` tests the
SCHEME, so ANY `sentinel:` uri would pass the origin check. Harmless while
`no_parent` is the only sentinel minted; carried as a one-line comment at the
check.

**Still open.** `move_up` (`traits.rs:2119`) and `move_down` (`traits.rs:2227`)
capture the old parent the same way and still refuse a root block, so a
re-homed leaf cannot be re-ordered among its root siblings. Left alone
deliberately: no invariant states the sibling-order contract at root, and
un-refusing them is a behaviour change that owes a red-for-the-right-reason
PBT first (`holon-feature`). The remaining oracle work is that invariant — a
root block round-trips out and back, and re-orders among its root siblings.

Not added to `docs/Testing/KeystoneKnownReds.md`: the signature is fixed for
`move_block`, not accepted. Worth knowing for the next one, though —
`scripts/keystone-known-reds.sh` could not have classified this red even if it
were registered: every registry pattern anchors on `diverged from the oracle`,
and this failure arrives as an `undo failed:` panic, so undo-replay panics are
invisible to the classifier and always report as novel.
