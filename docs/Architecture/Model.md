# The Holon Model (read this first)

*Part of [Architecture](../Architecture.md). This is the one-page mental model of
the whole system — the map agents and humans should load before touching sync,
storage, or UI code. Details live in [Replication](Replication.md),
[Sync](Sync.md), [Storage](Storage.md), [Operations](Operations.md),
[UI](UI.md); if this page and a detail doc disagree, this page states the
intent and the detail doc may be stale — flag it, don't silently pick one.*

## One sentence

Holon is one logical block tree, replicated across heterogeneous partial
replicas (org files, the Loro store, external APIs, the UI editor),
kept convergent by a single per-vault **consolidator** doing 3-way merges
against per-replica **bases**, and projected one-way into an incremental query
pipeline.

The read path is **one incremental computation**:
`Loro → (base diff) → Turso → (IVM matviews) → CDC → cells/LiveData → UI`.

## Five layers, one rule each

| # | Layer | Members | The rule |
|---|-------|---------|----------|
| 1 | **Replicas** | org files, Loro store, external APIs, UI editor | Every replica has a base; inbound intent = `diff(base, current)`. No replica writes another replica. |
| 2 | **Consolidator** | one per vault, epoch-pinned (Loro when enabled; Turso-LWW in SqlOnly) | The only merger. Monopolist of order: it mints every fractional index. Text merges via the CRDT merge *function*; structure via the tree CRDT (store present) or AST 3-way (absent). |
| 3 | **Projection** | Turso | Exactly one writer per mode; verbatim and total; never re-merges; ephemeral by contract. |
| 4 | **Reactive pipeline** | matviews → CDC → `LiveData<Block>` / cells | Convergent state, not an event log; recovery is resync (`Replace`), not acks. |
| 5 | **UI** | ViewModel `Mutable`s + `Cell`s | Displays fields and captures intent; owns no entity values. Structural ops are commit points. |

## Four orthogonal mode axes

"SqlOnly" is a *point in this grid*, not a different architecture. The
`HOLON_CRDT_ENABLED` switch currently flips several axes at once — keep the
concepts separate even where the config isn't:

| Axis | Values |
|------|--------|
| Storage backend | Loro store on / off |
| File adapter | org / none (a `crates/holon-markdown` was implemented then removed 2026-07-06 as unwired dead code — org is the sole selectable adapter; re-addable from git history) |
| Merge fidelity (per field) | op-CRDT ≻ base-3-way ≻ LWW |
| Transport | Iroh P2P on / off |

## Loro is three capabilities, not one

1. **Merge algebra** — RGA text merge, tree move semantics; a pure function
   library, always linked.
2. **Store** — durable replica with op history and frontiers.
3. **Transport substrate** — delta export/import for P2P.

The value of using the CRDT *without* the store equals the quality of your
state→ops inference, because without history you must reconstruct ops from
states:

- **Text**: character diff against a base is faithful → a *transient*
  `LoroText` gives real op-level merge from states alone
  (`TransientTextMergeProvider`). Worth it.
- **Trees**: reconstructing move-ops from two tree states *is* the hard problem
  itself — a transient tree CRDT would just consume an AST 3-way diff's output.
  Pointless. Tree-CRDT semantics are only real when the store holds actual
  history.

This asymmetry *derives* the rule "structural merge exists only when Loro is
the store" (Replication §2) rather than asserting it. The degradation ladder
for any field is: op-fidelity (store) → base-limited 3-way (transient) → LWW.

## Invariants

(1)–(7) are [Replication §9](Replication.md); (8)–(12) extend them.

1. One base per replica, diffed against — never against the cache.
2. One consolidator per sibling-set owns order; sinks store its fi verbatim.
3. Intent carries `after_sibling`, never an order key — enforced at the
   intent boundary by the closed `BlockWriteField` vocabulary
   (`holon-api::block_write_field`): a block `set_field` over
   `sort_key`/`after_block_id` is a loud `Err` in `OperationDispatcher`
   and in `LoroBlockOperations::execute_operation`, in both modes; the
   frontend intent constructor (`OperationIntent::set_field`) asserts the
   same. Reorders dispatch `move_block { id, parent_id, after_block_id }`.
4. Exactly one writer per store; the projection is total.
5. Sinks never re-merge.
6. Causality is inherited (scalar base now; Loro/git DAG if P2P topology ever
   demands it), never hand-rolled.
7. Loro-the-store and text-merge are decoupled capabilities.
8. **Structural ops are commit points** — pending editor state flushes through
   the merge path before the op executes, in one ordered dispatch
   ([UI](UI.md)).
9. **Tombstones outlive every base** — a tombstone may not be GC'd until every
   registered replica's base has advanced past it; otherwise a stale replica
   resurrects the deleted block on its next diff.
10. **Consolidator handover is an epoch, not a runtime lookup** — bases are
    only meaningful against one consolidator's linear history. Toggling
    Loro on/off without re-seeding every base from the new consolidator's
    state produces phantom diffs (spurious rewrites, fake conflicts LWW'd on
    incomparable timestamps) and mixes fi keyspaces (`gen_key_between` vs
    Loro-fi in one `sort_key` column). Handover = explicit migration: seed the
    new consolidator from the old consolidated state, rewrite all bases.
    **Today that migration is unbuilt** (spec 0008 Phase 4.1): the startup
    guard (`guard_consolidator_epoch` in
    `crates/holon-app/src/consolidator_epoch.rs`) refuses a mode flip, and
    acknowledging it with `HOLON_CONSOLIDATOR_MIGRATE=1` is an INTERIM
    wipe-and-reseed — it deletes every component's durable state, destroying
    anything not re-derivable from surviving replicas (Loro op history,
    SQL-only fields).
11. **One consolidator per file replica** — cross-device convergence travels
    through Loro/P2P, never through a byte-level file syncer
    (Syncthing/iCloud/Dropbox on the vault is out of contract). A foreign
    file write is indistinguishable from a user edit to the base diff, so each
    device re-ingests the other's projections as fresh intent: duplicated ops
    with wrong attribution, order oscillation, and `.sync-conflict` files
    ingested as duplicate-ID documents.

    **Disclosed exception — shared/mounted subtrees (share write-back):** a
    shared subtree is exactly the inverse setup of the failure above, and turns
    it into the design. The subtree lives in its OWN shared `LoroDoc` that
    converges across devices over iroh (invariant 11's sanctioned P2P channel).
    For that subtree, **Loro is truth and the on-disk org file is a one-way
    projection SINK** — rendered FROM the shared doc, never re-ingested as
    fresh global intent (Inc 3 marks the file so the file-sync controller
    suppresses its ingest). This is the deliberate carve-out from "org files
    are truth": for a mounted subtree the org file is a materialized view, and
    convergence still travels only through Loro/P2P, never through the file.

    The mount node is projected as a **Page** so the subtree owns a dedicated
    file, and there is a deliberate **Loro↔SQL shape difference at the mount
    boundary** the keystone oracles are taught to MAP (never skip): the mount
    id ≡ the shared page's id — when a PAGE `P` is shared the mount adopts `P`'s
    identity and `P`'s node FOLDS onto the mount in the SQL/org projection
    (P's children reparent to the mount, P's own row is dropped) while `P`
    stays uncollapsed in the shared Loro doc. When a plain BLOCK is shared the
    mount is a synthetic container page. A mount page never sits under a
    non-page (Amendment A: it bubbles to the nearest page ancestor).
12. **Every field write resolves a cell backing** — content and scalars do so
    in both modes: `LoroTextCellBacking`/`LoroMetaCellBacking<T>` in Full,
    `LwwTextCellBacking`/`LwwScalarBacking<T>` in SqlOnly (via
    `BlockCellRegistry::sql_only_wired`). The disclosed exceptions: the
    tree-position fields (`parent_id`/`sort_key`, pending Cells plan
    Phase 2.3 — `set_field("sort_key")` is a hard error, order is minted by
    the consolidator only), the derived/control fields
    (`id`/`depth`/`content_type`/`source_name`, `_expected_*` watermarks —
    routed to SQL), and the unseeded-vault content case. Per-backing status
    lives in [Storage §Cells](Storage.md).

## Cell vs Mutable (the UI state cut)

- `Cell<T>`, keyed `(uri, field, type)` (the registry cache key is
  `(EntityUri, String, TypeId)`, `crates/holon-core/src/cell_registry.rs`):
  entity field state — has identity, an authority behind it, cross-consumer
  coherence. Coherence is guaranteed by the shared backing, not by
  cell-object identity — two consumers at different `T` get distinct `Cell`
  instances. `current()` reflects the
  *local authority replica* including uncommitted local ops; cross-device
  confirmation is invisible by design (CRDT).
- `Mutable<T>` on the ViewModel node: per-render-slot widget state (expanded,
  scroll, hover). Two same-id rows in different panes need independent state —
  never collapse these into a `(uri, field)` registry (FU-1 lesson).

## Offline (future) — where it plugs in

Command sourcing is *intentional early design*, not dead code. When offline
lands, the upstream intent channel (Replication §7) **becomes** the durable
command log; `id_mappings` is the `OwnForeign(map)` ID capability of
Replication §2. What we keep warm today are the hard-to-retrofit invariants:
client-minted operation IDs, serializable ops with provenance, inverse ops,
anchored (not offset) positions.
