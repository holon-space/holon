# merge_blocks — Inc 1 (ratified 2026-07-30)

Ratified context: split-doc-root repair = general duplicate-identity merge.
Canonical default = disk-declared anchor id. Ingest stays strict. Strict-husk
auto-adopt is Inc 2; this doc is Inc 1: the operation itself.

## Operation contract

Engine-level compound (ADR 0024), MCP-exposed through the existing dispatch:

    merge_blocks { canonical: EntityUri, duplicate: EntityUri }

It is the structural twin of `convert_block_to_page`: a read-only provider
planner computes the whole change, then the engine executes each constituent as
an ordinary dispatched, invertible op and assembles ONE composite `UndoEntry`.

- Compound + descriptor + undo assembly: `crates/holon/src/api/operation_engine.rs`
  (`MERGE_BLOCKS_OP`, `run_merge_blocks`, `merge_blocks_descriptor`,
  `dispatch_merge_constituent`, `write_merged_from`).
- Planner op `merge_blocks_plan` and the redirect derivation:
  `crates/holon/src/core/sql_operation_provider.rs`.
- Typed plan, normalization, keeper choice: `crates/holon/src/core/merge_blocks_plan.rs`.
- Under Loro authority the block CRUD provider is `LoroBlockOperations`, which
  advertises neither the planner nor `rewrite_link_resolution`, so
  `merge_blocks_plan` joins the narrow SQL-provider allowlist in
  `crates/holon-app/src/turso_seams.rs` — the same seam `block_to_page_plan`
  already needed.

Preconditions (all fail loud, all in the planner so a refusal writes nothing):
both ids resolve; canonical != duplicate; the duplicate is not already merged
away; the duplicate is not an ancestor of the canonical; the duplicate is NOT a
document root with a live `file.document_id` binding (out of Inc 1 scope —
refuse rather than guess at file semantics).

## Semantics — one undo entry

1. CHILDREN: the duplicate's children move under the canonical, appended after
   its last existing child, order preserved.

   They are moved BACK-TO-FRONT against a fixed anchor (the canonical's
   original last child). The forward result is identical either way, but
   `move_block` captures its inverse anchor as it runs: moving front-to-back
   pulls each child out of the shared parent before the next one's inverse is
   captured, so every inverse degrades to "become the first child" and undo
   reverses the siblings. The dedupe's orphan re-homing follows the same rule,
   and both move buckets replay in strict LIFO.
2. DEDUPE (one level): over the combined direct children, group by normalized
   content (trim + collapse each whitespace run to one space). Keeper =
   authored `:ID:` over minted, then oldest `created_at`, then id (total and
   reproducible). Each loser's children are re-homed under the keeper FIRST,
   then the loser is deleted behind its own redirect. Husks (empty after
   normalization) are never duplicates of one another.
3. CONTENT: the canonical wins. An empty canonical adopts the duplicate's body;
   two differing bodies park the duplicate's as the canonical's FIRST CHILD
   (visible, reversible, outdentable), never silently dropped. The parking runs
   BEFORE the child moves and the dedupe, so the parked body is an ordinary
   child by the time collapsing happens; when its normalized content already
   appears among the merged children it is not created at all, since parking it
   would BE that duplicate.
4. TAGS/PROPS: union with the canonical winning conflicts — so a `Page` tag on
   either side survives, and only keys the canonical lacks are adopted. The
   duplicate's `ID` is NEVER adopted: copying the merged-away authored `:ID:`
   onto the survivor would make write-back render `:ID: <merged-away id>`,
   re-creating the split-root shape this operation repairs. Internal
   underscore-prefixed keys (`_provenance`) are excluded for the same reason —
   they are the writer's bookkeeping, not the donor's to give.
5. REDIRECT + PROVENANCE — ONE fact, not two: the canonical gains a
   `merged_from` property holding space-separated `<merged-away-id> <millis>`
   pairs. This is the REPLICATED record (an ordinary block property in the main
   Loro doc, so it replicates and org-round-trips as `:merged-from:`), and the
   `block_redirects` table is its queryable index, re-derived at the SQL write
   boundary exactly as `block_links` is re-derived from `marks`. Inbound links
   are re-pointed eagerly via the existing `rewrite_link_resolution` op, whose
   capture-based inverse restores them exactly.
6. RESOLUTION HOOK: `BackendEngine::resolve_block_id`
   (`crates/holon/src/api/backend_engine.rs`) is the ONE seam — it follows
   redirect chains and returns the id unchanged when nobody merged it away, so
   callers can route through it unconditionally. It fails loud on a cycle
   rather than spinning. Foreign org files referencing the old id heal at
   resolution time; their text is rewritten only on their own next write-back.

## Deviations from the ratified draft (with reasons)

- **The redirect is NOT a new doc-level Loro map.** The draft called for a new
  replicated `block_redirects` map in the main doc, projected to SQL. Storing
  the same fact as the canonical's `merged_from` property achieves every
  ratified constraint (replicated, not owner-private like `AliasLedger`,
  append-only, acyclic, chain-following) with zero new Loro containers, zero
  sync-controller changes, and the merge's provenance and its redirect kept as
  ONE fact that cannot disagree. It also makes the redirect's retraction fall
  out of the ordinary `set_field` inverse.
- **The draft's premise that deletion is a soft-delete is false.** `block_raw`
  has no deleted flag and deletion removes the row, so the redirect could not
  live on the merged-away block; it must live on the survivor. Inc 1's recovery
  story is therefore the one-gesture undo plus the `merged_from` provenance.
- **`sort_key` is not restored verbatim by undo, by design.** Structural ops
  recompute the positional columns from the live tree (the same rule the
  block→page transform's undo fingerprint follows), so "exact pre-merge state"
  means parent, content and sibling ORDER — which the PBT asserts directly.
- **Deltas for rows the merge deletes are excluded from the staleness
  fingerprint.** The guard reads fields back from `block_raw`; a deleted row
  answers nothing, so fingerprinting it would make every merge's undo read as
  stale.

## Chain semantics, the delete hole, and what Inc 2 owes

Resolution follows redirect chains (A→B→C resolves A to C) and refuses a cycle
with the chain in the message. `merge_blocks` never creates a cycle: the
planner refuses when the duplicate already redirects anywhere.

**The delete hole (Inc 2).** Nothing stops a later `delete` of a merge
survivor, which strands every id merged into it — the redirect then points at a
row that no longer exists. Inc 1 makes this LOUD rather than silently wrong:
both resolution paths fail with the full chain and an explanation that the
survivor was deleted. It does not PREVENT it. Inc 2 owes a real answer —
either refusing to delete a block that redirects resolve to, or re-pointing its
redirects at its own successor.

**Seam coverage.** The redirect consult is wired on the block-lookup MISS path
of `CacheBlockReader::get_block_authoritative`, so an unmerged lookup pays
nothing extra. `CacheBlockReader` IS the `dyn BlockReader` the app and the test
environment resolve: `turso_seams.rs` registers it and no other registration
exists, Loro-enabled or not. Property (g) asserts this through the DI-resolved
reader rather than through the concrete type.

`LoroBlockReader` is registered ONLY by `build_no_turso_container`
(`StorageSelector::LoroMemory`, test_environment.rs). That container registers
no `DbHandleProvider` and has no `block_redirects` table, and `merge_blocks_plan`
is on the SQL-provider allowlist, so `merge_blocks` cannot run there at all.
There is consequently no `DbHandle` to plumb into `LoroBlockReader` and nothing
for it to redirect to — wiring it is not an Inc 2 debt, it is a non-operation
under that storage selector.

The remaining unwired seam is `DocumentManager::get_by_id`
(`LiveDocumentManager`, which does hold a `DbHandle`). Documents are the
narrower case since a doc root with a live file binding is refused as a merge
duplicate outright, so this is left for Inc 2 with the delete hole.

## Trash

No Trash container exists. Inc 1's recovery = the one-gesture undo +
`merged_from` provenance. A user-visible Trash container is a later increment.

## Testing

`crates/holon-integration-tests/tests/merge_blocks_pbt.rs` — a dedicated
op-level PBT over generated husk / both-non-empty shapes whose children are
drawn from a small alphabet with whitespace decoration, so
normalization-equal duplicates arise across BOTH sides. It drives the real
`execute_operation("block", "merge_blocks", …)` dispatch, which is also the
MCP path. Properties:

(a) resolving the duplicate's id yields the canonical;
(b) the normalized non-husk content multiset survives up to dedupe collapse,
    every collapsed group keeping at least one member, and nothing is invented;
(c) child order is deterministic — the canonical's own children keep their
    relative order and precede the duplicate's survivors;
(d) one undo gesture restores the pre-merge blocks, sibling order, link
    resolutions, and retracts the redirect;
(e) every inbound link that resolved to the duplicate resolves to the canonical;
(f) merging an already-merged pair fails loud.

Two further properties:

(g) the DI-resolved PRODUCTION `BlockReader` resolves the merged-away id to the
    canonical block — asserted through the container, so a future re-wiring of
    `dyn BlockReader` to a reader without the redirect consult goes red;
(h) tags union with the canonical winning conflicts, properties adopted only
    for keys the canonical lacks, and the duplicate's authored `ID` never
    adopted.

The generator draws children WITH grandchildren (so a dedupe loser carries a
subtree and the orphan re-homing loop actually runs) and tags/properties on both
sides, the duplicate always carrying an authored `ID`. The underscore-prefixed
half of the ID rule is NOT independently observable: every dispatched write
stamps `_provenance`, so the canonical always already holds it.

That coverage immediately caught a defect in Inc 1 as landed: the planner read
the `properties` column as JSON TEXT (`Value::as_string`), but the SQL read
boundary (`normalize_known_json_columns`) parses that column into a
`Value::Object` before any provider sees it. `properties_absent_from`,
`property_from_blob` and `properties_carry_authored_id` therefore all returned
"empty" unconditionally — property adoption never happened, chained merges lost
their prior `merged_from`, and the dedupe's authored-`:ID:`-wins keeper rule
never fired. They now read the object shape and fail loud on anything else.

`merge_blocks_undo_restores_order_after_identical_child_collapse` pins the
shrunk shape that caught the move-inverse anchor defect (two children with
identical normalized content, so a collapse deletes one and undo must both
re-create it and restore the pair's order) as a deterministic regression.

## Latent bug this surfaced elsewhere

`convert_block_to_page` re-homes children with the same front-to-back loop
(`run_convert_block_to_page` step 4) and its comment claims the `move_block`
inverse "restores the child's original parent + predecessor exactly". It does
not, for the same reason: after the first child moves, its siblings' captured
predecessors are gone. Undoing a block→page conversion of a block with two or
more children should reverse them. NOT fixed here (out of Inc 1 scope) —
needs its own red-first test.

Keystone generator rung (Dir.org + Dir/Child.org) + a `MergeBlocks` transition
remain Inc 4, as ratified.
