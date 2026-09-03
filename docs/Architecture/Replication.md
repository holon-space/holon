# Replication & Consolidation Model

*Part of [Architecture](../Architecture.md). Reconciled with code on 2026-09-03.*

> **Status: target architecture (2026-05).** This document captures the model
> we converged on for how blocks (and other entities) are kept in sync across
> heterogeneous components — Loro, org/markdown files, Todoist, Turso, the UI.
> Parts of it already exist (`last_projection` diffing, `LiveData<Block>`
> projection, Loro-fi authority); parts are aspirational. Where the two differ
> it is called out explicitly. This refines, and in places corrects, the
> "Authority + Projection" framing in [Sync.md](Sync.md).

---

## 1. The problem this solves

Holon stores one logical structure — a tree of **blocks** — but no single
component holds all of it, and the components are wildly heterogeneous:

- **Org / Markdown files** (when enabled) hold one document's blocks; optional; durable; edited *out of
  band* (the user or an AI agent edits the file directly — we cannot funnel
  those writes).
- **Loro** (when enabled) holds a CRDT replica of the tree; optional.
- **Todoist** holds tasks in *its own* id-space; external; partial. *(The
  `holon-todoist` crate was deleted; a Todoist MCP sidecar is bundled
  (`crates/holon-mcp-client/src/bundled_sidecars.rs`) but off unless
  explicitly enabled per-vault. Nothing exercises its sync, write-back, or
  undo — see `FeatureMap.md` §Unpinned. Todoist stands in here for any
  external, partial, server-authoritative component.)*
- **Turso** holds a queryable SQL cache of everything; ephemeral (deleted on
  most app starts).
- **The UI** holds the rendered subset and originates user edits.

The bugs that motivated this model (sibling-order scramble, "sort_key stays
`A0`", resurrected-after-delete content, ack headaches on the event bus) all
trace to the same root: **we never had a precise notion of *what each component
was last known to agree on*, so every divergence was treated as a fresh edit,
and more than one writer mutated the same store.**

The model below is, in one sentence: **multi-master replication of one logical
tree across partial, heterogeneous replicas, reconciled by 3-way merge against a
per-component base, with order owned by a single consolidator and projected
verbatim to read-only sinks.**

Equivalently, from the user's point of view: the 3-way merge is how we
**recover intent** from an out-of-band edit. We cannot funnel an org-file edit
through an intent API, so we reconstruct the intent after the fact —
`diff(base, theirs)` is precisely the delta between what a component last
agreed on and what it holds now, i.e. the user's intent expressed in that
component's language. §4 turns that delta into explicit intent ops.

---

## 2. Components are capability profiles, not roles

Stop asking "is X the authority." Ask "what can X do." Every block-handling
component declares a capability profile:

| Axis | Values | org file | Loro (full) | Todoist | Turso | UI |
|---|---|---|---|---|---|---|
| **ID policy** | Mint / AcceptForeign / OwnForeign(map) | AcceptForeign (+mint on a brand-new on-disk block) | Mint | **OwnForeign** (needs id-map) | AcceptForeign | — |
| **Merge caps** | FullCRDT / TextCRDT-on-demand / LWW | LWW (or borrow TextCRDT) | FullCRDT | LWW per field | LWW (or borrow TextCRDT) | — |
| **Order rep** | Sequence / SortKey / FractionalIndex | Sequence (line order) | FractionalIndex | Sequence | SortKey | — |
| **Domain** | all / one-doc / tasks-only / rendered-subset | one doc | all | tasks only | **union (all)** | rendered subset |
| **Durability** | durable / ephemeral | **durable** | durable-if-present | external | **ephemeral** | ephemeral |

**Roles are assigned dynamically from capabilities — never hardcoded:**

- **Consolidator** = the most capable *merger* currently present. It performs the
  one authoritative merge per change. Capability order:
  `FullCRDT (Loro store) > 3-way-with-base (git/jj over files) > LWW` — this
  ranks **structural** merge capability; text merge never degrades (see the
  store-vs-merge-function distinction below).
- **Durable base** = whichever durable component holds truth. *Today we default
  to the org files*, but that is a stability choice, not an architectural one —
  org is exactly as optional as every other component. We default to org while
  Holon matures because a mangled org file is human-readable and trivially
  repaired, while a mangled Loro doc is not; once Holon is stable enough, Loro
  becomes the default durable base. The abstraction must allow org **or** Loro
  to fill the role.
- **Sink** = a derived, read-only consumer that **never re-merges**; it applies
  the consolidated result verbatim. Turso and the UI are sinks.

This is "make the best of what we've got": with the Loro store present, Loro is
the consolidator and Turso is a downstream sink; with only Turso (the "don't
delete the `.db`" mode), Turso is consolidator *and* store *and* sink.

**Loro-the-store and Loro-the-merge-function are two different capabilities**
(§4, invariant 7). The Loro *library* is always linked, so CRDT **text** merge
is available in every mode: a consolidator not backed by the Loro store can
still merge concurrent edits to one text field (say, org file vs. UI) through a
*transient* `LoroText`. What actually varies by mode is **structural** merge
capability — the tree CRDT (parent / order / move semantics) exists only when
Loro is the store; without it, structure falls back to 3-way AST merge or LWW.
That is what the consolidator capability order above ranks.

**Removability is orthogonal to peer-ness.** Dropping Turso loses cross-system
queries but replication still runs; dropping Loro downgrades merge to
LWW/3-way. No component is mandatory.

---

## 3. The central primitive: per-component base + 3-way merge

The one abstraction everything else rests on. Each component persists its
**last-synced base** — the common ancestor for a 3-way merge. Given:

```
base   = last-synced state of this component        (the ancestor)
theirs = what the component holds now (disk / API / tree)
mine   = current consolidated logical state

diff(base, theirs) ≠ ∅  →  genuine inbound edit   →  ingest
diff(base, mine)   ≠ ∅  →  component is stale      →  emit (write it out)
both changed same field  →  conflict              →  capability-merge, disclosed
after a successful sync:  base := merged result
```

Without `base` you **cannot** distinguish "the file was edited" from "the file
is stale and another component moved ahead" — and that ambiguity is the
resurrection/scramble bug class. This answers, in one move:

- **"What does *synced* mean?"** → the component's base is current with respect
  to the consolidator's logical clock.
- **"How do we tell old vs. new edits in an org/markdown file?"** → the 3-way
  diff above. A field that differs disk-vs-base is a real on-disk edit; a field
  that differs mine-vs-base but disk==base means the file is merely behind.
- **"What is `last_synced_hash` a special case of?"** → a content-addressed
  identity of the base snapshot.
- **"Where does *intent* come from?"** → this same diff. For components whose
  writes we cannot funnel (files edited out of band), `diff(base, theirs)` *is*
  the intent extraction; §4 converts it into explicit intent ops.

> **Current state (2026-07):** (a) and (b) are done — the base is formalized
> behind the `BaseStore` trait seam (`crates/holon-filesystem/src/sync_base_store.rs`)
> and both the org diff and `LoroProjection::project` read their "before" through
> it. (c) is partial: `FileSyncController` no longer diffs against the cache, but
> still reads it (`block_reader.get_blocks`) for cold-boot base re-seeding and
> count/render checks. **Diff against the base, never against Turso** — Turso is
> the consolidated *current* state, the wrong ancestor.

### The base store is a KV interface, Loro is one impl

Target shape — per `(component, entity)`: get/put the base snapshot. The landed
version (`crates/holon-filesystem/src/sync_base_store.rs`) is narrower: the trait
is `BaseStore` (sync, not async), keyed by `BaseKey { peer, file }` rather than a
`ComponentId` type (which doesn't exist yet), and it stores whole-document
snapshots (`HashMap<block-id, SnapshotBlock>`) rather than per-entity bases —
`SyncBaseStore` is the concrete in-memory impl, which today ignores the key
("one global doc"):

```rust
pub trait BaseStore: Send + Sync {
    fn get_base(&self, key: &BaseKey) -> Arc<HashMap<String, SnapshotBlock>>;
    fn put_base(&self, key: &BaseKey, snapshot: HashMap<String, SnapshotBlock>);
    fn is_base_seeded(&self, key: &BaseKey) -> bool;
}
```

Implementations, "best available for the job":

- **Shadow copy** in `.holon/` — simplest; start here.
- **git / jj** — adds history, content-addressing (dedup), and a *line-based*
  text 3-way merge for the no-Loro fallback. **jj fits better than git**: its
  working copy is always an auto-snapshot, so "current disk" is diffable with no
  staging dance, and its op-log is the causal DAG. Keep a `last-synced`
  bookmark, advance it on sync completion (see §6 on commit discipline).
- **Loro version frontier** — for Loro-backed data the base is just a Loro
  version; merge is built in.

---

## 4. Diff → intent → merge

Three separable steps. Keep them separate; conflating them is how order keys
leak and how stale data gets re-ingested.

1. **Extract intent** from `diff(base, theirs)`:
   - **Structure / order** (block created / re-parented / reordered) comes from
     diffing the *parsed ASTs* (base-AST vs disk-AST) — **not** a text/line diff
     and **not** Loro. The org parser produces structural intent.
   - **Text fields**: seed a `LoroText` at the base and `update(disk)` so Loro
     emits the minimal ops *as this component* — the intent is then already in
     the merge target's language. This does **not** require Loro to be the
     store: a *shared* `LoroText` when it is, a *transient* one when it isn't
     (see below). There is no separate text-diff path: when a field's merge
     capability is LWW the whole value wins (no ops extracted), and a git/jj
     3-way merge consumes the raw texts directly — ops-as-intent always means
     LoroText.
2. **Merge** the intent at the **consolidator** (the most capable merger). Only
   the consolidator decides the outcome. Every other component applies the
   result.

3. **Project** the consolidated result to sinks (§7).

**Intent is expressed in domain terms and carries no storage encoding:**

```
create(canonical_id, content)
set_field(field, value)          // field is an enum; NOT sort_key, NOT parent_id
relocate(uri, parent, after_sibling)   // positional intent only
delete(uri)
```

`relocate` carries `after_sibling`, **never** a sort_key / fractional index /
sequence number (see §5). This is "parse, don't validate" applied to the wire:
the bus type literally cannot carry an order key, so the disjoint-keyspace bug
(gen_n_keys vs Loro-fi) becomes unrepresentable.

> **Landed:** `ChangeOp::Relocate { id, parent, after_sibling }` in
> `crates/holon-api/src/change_set.rs` has exactly this shape — no order-key
> field. Caveat: the decode path still accepts `sort_key` params as an
> explicitly-marked fallback until the Phase 5 flip (spec 0007), so the
> "cannot carry an order key" guarantee is the wire *type*, not yet the whole
> pipeline.

### Loro: store vs. merge-function are two things

`LoroStore` (a durable replica: the tree, persistence, P2P) and `TextMerge`
(a merge *function* for one text field) must be decoupled — this is what makes
"Loro optional" tractable and lets Todoist borrow CRDT text merge **without**
storing in Loro.

- The **merge target** for `(uri, field)` is one *shared* `LoroText` when Loro
  is the store (this is what gives real-time collaboration); a *transient*
  `LoroText` (or git/LWW) when it is not.
- The **per-component bases** do **not** live in the `LoroText` — they live in
  `SyncBaseStore`. The `LoroText` is the convergent *result*; the bases are the
  *inputs* used to derive each non-CRDT component's intent. Different layers, no
  conflict.
- So a `TextMergeProvider` returns the *same* target per `(uri, field)` (shared,
  when backed by a store), and components remain individual through their
  *bases*, not through separate mergers. *(Landed:
  `crates/holon-loro/src/text_merge_provider.rs` with `LoroTextMergeProvider`
  and `TransientTextMergeProvider`.)*

---

## 5. Ordering: one owner, fractional index, projected verbatim

Order is the field-type where merge capability matters most, and the historical
source of the worst bugs. The rule:

> **Order is owned by exactly one consolidator per sibling-set. The owner
> generates the fractional index; every sink stores it verbatim.**

Consequences (all desirable, all free once the rule holds):

- **Do not build a `Vec<EntityUri> → Vec<FractionalIndex>` trait.** It asks the
  caller to *assert* an order that may disagree with the owner — the dual-writer
  bug reborn for ordering. Because intent only ever carries `after_sibling`
  (incremental), the owner is never handed a full order to "bless," so the
  "input Vec differs from Loro's" inconsistency *cannot arise*.
- **Same fi in Turso and Loro**, automatically — Turso stores Loro's value
  verbatim (`read_block_from_tree` already reads `tree.fractional_index(node)`).
  Great for debugging.
- **O(1) reorder.** Inserting between neighbors A and B mints a key strictly
  between their fis; A and B are untouched. One row UPDATE in Turso, one CDC
  delta for IVM. No integer-index renumber, no O(N) sibling churn.
- **One keyspace.** Only the owner generates keys, so gen_n_keys-space and
  Loro-fi-space can never mix. The original sin is structurally impossible.
- **Loro absent?** The owner role moves (org-3-way, or Turso-LWW in `.db`-only
  mode) and *that* component generates fi locally (existing `gen_key_between`).
  Still one owner → still one keyspace. `new_child_anchor`-returns-`String` is
  reachable **only** in the mode where that component owns order, enforced at the
  type level: it lives on a separate `OrderKeyMinting` trait implemented solely
  by the SqlOnly `Store` order owner (`SqlBlockOperations`). The Loro ordering
  seam (`LoroBlockOrdering`) implements only `BlockOrdering` and has no minting
  method at all, so the Loro path *cannot* mint a key — it is unrepresentable,
  not merely unused. `split_block` reaches minting through
  `BlockOperations::order_key_minter()`, which returns `Some` only for the store
  owner and `None` on the Loro path (where `apply_create` derives the fi from
  `position_after_block_id`). *(Done, spec 0008 §3.2: the former Loro-mode
  `loro_seams.rs` placeholder that returned a `default_sort_key()` has been
  deleted.)*

**Partial-replica rows** Loro doesn't have (e.g. Todoist tasks in Turso's union
domain) get their order from *their* owner (Todoist's sequence), converted to fi
by the Todoist→Turso projector. No conflict because each sibling-set has exactly
one owner. **Mixed-origin sibling sets (decided 2026-07):** when a parent's
children come from two different source components (an external task embedded
under a note), order ownership follows the **parent's home component** — that
owner mints the fi for every child, foreign ones included; the foreign
component's native sequence is input to the owner, never a second keyspace.
Implementation waits for the first integration that embeds under a note.

The live "sort_key stays `A0`" bug **is** a violation of this section: the
projection failed to carry every Loro block's fi (non-total projection) while a
second writer inserted NULL rows. "Single owner + verbatim *total* projection"
makes the column always equal Loro's fi — the fix and the architecture are the
same change.

> **Status (2026-07):** the projection is verbatim-from-Loro-fi with a
> totality guard (R-1 `debug_assert` in `loro_sync_controller.rs` + a PBT
> invariant counting `sort_key IS NULL` rows), but not yet strictly total: the
> snapshot loop falls back to `default_sort_key()` (`"A0"`) when Loro returns
> no fi for a node. Spec 0007 Phase 1 still lists "total, verbatim fi
> projection" as open.

---

## 6. Causality without version vectors

We need *some* representation of the causal partial order to know "is this
component merely behind, or did we both edit concurrently." There are two
encodings:

- **Version vector** — `{component → counter}`; incomparable vectors ⇒
  concurrent. A compressed summary of history.
- **Commit DAG** (git) — content-addressed commits naming their parents; the
  3-way base is the lowest common ancestor. The *full* history.

**Why Git is P2P yet needs no version vectors:** it keeps causality in full (the
DAG) instead of summarizing it (a vector). Same questions, different encoding.

**Our choice: star-sync ⇒ scalar bases, no version vectors.** Because every
component syncs against the single logical consolidator, causal history is
*linear* (the consolidator's sequence of states), so "the base" collapses to one
content-addressed pointer per component — exactly `SyncBaseStore`. Version
vectors are the price of *arbitrary-topology* P2P, which we do not need yet
(YAGNI until two devices edit the same document offline through *different*
backends).

**We never hand-roll causality machinery.** If arbitrary P2P is ever required,
Loro's internal op-DAG/frontier provides it for Loro-backed data and git/jj
provides it for files. Steal, don't build.

**git/jj base-store commit discipline:** the base *is* a commit, so commit
exactly when a sync round completes (merge-commit semantics; the merged result
becomes the next base). Commit too late → re-detect already-synced changes as
new (resurrection). Commit too early (in-progress edits as base) → treat real
edits as already-based and lose them. With jj, advance a `last-synced` bookmark
on completion; the auto-snapshotting working copy stays separate, and mid-sync
user edits are just more uncommitted change picked up next round.

---

## 7. Two transports, not one

The data-transfer layer (today: the event bus; its acking is the pain point) has
**two distinct needs** that should use two channels:

| Direction | Needs | Transport |
|---|---|---|
| source-component → consolidator | intent + provenance + base ref, **lossless** | a thin **intent/op channel** |
| consolidator → sinks (Turso, UI) | convergent **current state**, ack-free | **`LiveData<Block>` / `SignalVec<VecDiff>`** |

`SignalVec` / `LiveData<Block>` is **convergent state, not an event log**: a new
subscriber gets `Replace{values}` (full current state) then diffs; drop a diff
and you resync from the next `Replace`. That kills the acking problem — and it is
exactly right for the *downstream* side, which has nearly all the consumers
(hence "simplifies consumers a lot"). But it is **insufficient as the inter-peer
merge bus**:

> *(The per-parent `SignalVec` shape is aspirational — today's block downstream
> is `BlockFeed`/`LiveData<Block>`; `SignalVec` is only used in the
> command-palette popup.)*

1. **It ships whole values.** `UpdateAt{value}` carries the entire `Block`; if
   `Block` includes `sort_key`, order keys cross the wire via the value —
   violating §5. Avoid by omitting `sort_key` from the on-wire `Block` and
   letting **vec index encode order** (cleanest with **one `SignalVec` per
   parent**: `InsertAt{index}` *is* "insert as the index-th child").
2. **It is lossy on intermediate edits.** Two edits to one field between
   observations collapse to the final value — perfect for a cache, destructive
   for merge inputs.
3. **No provenance/base** for 3-way merge.

So: `LiveData<Block>` downstream (the existing `BlockFeed` wiring in
`event_infra_module.rs` is this; the older `run_block_mirror` was deleted); a
thin, lossless, provenance-carrying intent channel upstream of the consolidator.
Turso writes its `sort_key` column locally from the vec index/owner fi and never
echoes a key back upstream.

### The durable form of this channel (offline, future)

When offline support lands, the upstream intent channel **becomes a durable
command log** — the same channel, persisted. This absorbs the earlier
"command sourcing" design (started deliberately early so the architecture would
account for offline; previously a standalone sketch in
`command_sourcing_todo.md`, now folded here). What carries over:

- **Log shape**: an append-only `commands` table — client-generated UUID as
  the idempotency key, `entity_id` for per-entity ordering, the serialized
  intent op as payload, a `pending → syncing → synced | failed` status, and
  `error_details` for user-facing rejection feedback. The payload vocabulary
  is §4's intent ops (`ChangeOp`) — **not** a separate command enum; the
  earlier `CommandType` sketch is superseded by the intent vocabulary.
- **ID mapping**: an `id_mappings` table (internal id → external id, filled
  after sync) — this is precisely the `OwnForeign(map)` ID capability of §2
  made concrete, for optimistic creates against systems that mint their own
  ids.
- **Partial-batch failure policy** (decided): stop on first failure per
  entity, mark it, refetch the entity's canonical state, leave the remaining
  commands pending. Rollback is **refetch, not inverse computation**.
- **Principles kept warm today** (the hard-to-retrofit invariants, all already
  present): client-minted operation IDs, ops serializable with provenance,
  inverse ops (`OperationResult::undo`), anchored — never offset — positions.
- Deferred mechanics for when it's built: background sync worker with
  per-entity serial replay, command compaction for long offline periods,
  batch sync under one idempotency key.

The `OperationLog`'s `PendingSync`/`Synced`/`Cancelled` statuses
([Sync](Sync.md)) are the implemented today-side of this: undo bookkeeping now,
sync bookkeeping later. There is deliberately **one** future offline design —
this section — not three.

---

## 8. End-to-end picture

```
            ┌──────────── source components (originate truth) ───────────┐
            │  org file   markdown   Loro store   Todoist   UI            │
            └────┬───────────┬──────────┬───────────┬────────┬───────────┘
                 │ diff(base,theirs) → intent (after_sibling, no keys)    │  per-component
                 ▼           ▼          ▼           ▼        ▼            │  base in
            ╔═══════════════════════════════════════════════════╗        │  SyncBaseStore
            ║  CONSOLIDATOR  (most capable merger present)        ║◄───────┘  (shadow/git/Loro)
            ║  - one authoritative 3-way merge per change         ║
            ║  - owns order per sibling-set → mints fractional idx║
            ╚═══════════════════════╤═══════════════════════════╝
                                     │ convergent current state
                                     │ LiveData<Block> (ack-free, fi verbatim)
                     ┌───────────────┼───────────────┐
                     ▼               ▼                ▼
                  Turso            UI            (other sinks)
              (union query)    (rendered)        never re-merge
```

---

## 9. Invariants (the things that must not be violated)

1. **One base per component**, diffed against — never against the cache.
2. **One consolidator per sibling-set owns order**; it emits the fractional
   index; sinks store it verbatim. No `Vec → Vec<fi>` trait.
3. **Intent carries `after_sibling`, never an order key**; `set_field` cannot
   carry `sort_key`/`parent_id`.
4. **Exactly one writer per store.** The consolidated feed is the sole writer of
   Turso; the projection is *total* (every owned block's fi reaches the column).
5. **Sinks never re-merge.** They apply the consolidated result as a fait
   accompli.
6. **Causality is inherited, never hand-rolled** (scalar base now; Loro/git DAG
   if P2P is ever needed).
7. **`LoroStore` and `TextMerge` are decoupled** — borrowing CRDT text merge
   never implies storing in Loro.

---

## 10. Open questions (refinements, not blockers)

- Exact shape of the upstream `ChangeSet`/intent type and its provenance fields.
- Per-parent vs. global `SignalVec` for the downstream projection.
- First `SyncBaseStore` impl: shadow-copy vs. jj (lean shadow-copy first, behind
  the trait, upgrade where history/merge earns its keep).
- ~~A parent whose children originate in two different source components (who
  owns that sibling-set's order).~~ **Decided (2026-07):** the parent's home
  component owns the sibling-set's order — see §5.

---

## 11. Relationship to the live bug & first step

The "sort_key stays `A0` / sibling-order scramble" bug is, precisely, violations
of invariants **2** and **4** (a second writer inserts NULL `sort_key` rows; the
fi projection is non-total). The fix is topology-independent — it holds under
every variant of this model — so it doubles as the foundation's first step:

1. Formalize `last_projection` → `SyncBaseStore`; diff against the base, not the
   cache.
2. Make order single-owner with a **total, verbatim** fi projection.
3. Make the consolidated feed the **sole** writer of Turso.

See [Sync.md](Sync.md) for the current implementation surface this evolves.

> **Implementation status (verified 2026-07-01).** Steps 1–2 have **landed**:
> the `BaseStore` trait + `SyncBaseStore` impl exist
> (`crates/holon-filesystem/src/sync_base_store.rs`, see §3 for how the landed
> shape differs from the sketch) and `LoroProjection::project` diffs against the
> base; children read the Loro tree for single-owner order
> (`loro_block_operations.rs::children_ordered`). Step 3 is partial: the
> `sole_block_writer` archlint gate exists, but it sanctions two mode-dependent
> writers (Loro projection in Full mode, `SqlBlockOperations` in SqlOnly), and
> `codev/specs/0007-architecture-improvement-plan.md` — the tracking spec for
> the sole-writer / authority-first-delete / intent-vocabulary work — still has
> those Phase 1 items open (spec header: Phase 0 in progress).
> Cell backing types `LoroMetaCellBacking<T>`
> (`holon-loro/src/loro_meta_cell_backing.rs:95`) and `LwwScalarBacking<T>`
> (`holon-core/src/cell.rs:301`) **are implemented and wired** via
> `block_cell_registry.rs` (scalar resolution at :547, meta at :609), alongside
> `LoroTextCellBacking` **and** `LwwTextCellBacking`. The tree-position backings
> `LoroTreeParentCellBacking` / `LoroTreePositionCellBacking` remain
> unimplemented; the `write_field` carve-outs in `block_cell_registry.rs` route
> parent/position writes correctly without them.
> See `devlog/2026-05-22-blocksync-p3-basediff-handoff.md`.
