# Option C — `doc_blocks` as a declared derived holder

**Date:** 2026-08-04
**Status:** Inc 0 LANDED; **Inc 1 implemented, uncommitted** (§9); **Inc 2 UNBLOCKED** (§9.7 resolved).
**Tree state:** GREEN — see the run-counted gate matrix in §9.8. Shadow default-ON in debug; budget-asserting suites opt out at the suite level; the armed suite gates on shadow liveness.

Inc 0 (`crates/holon-api/src/live_data/home_by.rs`, verifier-CONFIRMED after one refute/fix round). Implementation deviations from this design, all property-driven:

- **§1.1** — the combinator takes a `HomeAuthority` trait, not a per-value `home_fn` closure: the sibling-group and subtree reads Laws 1–2 require cannot be expressed over one value.
- **§1.2** — accumulator entries are `{home, parent, tree_prev, value}`, not bare `Home<K>`: the retained `Arc<T>` lets neighbours/descendants that emit no feed event be re-emitted with a value (keeps the emitted stream self-determining, which the fold-equality property needs); `tree_prev` is a pure movement detector.
- **§2.2/§1.3** — `prev` is **document-relative** (skips siblings owning their own document; a cross-document pointer is unresolvable by a per-document consumer). Emitted order comes from `children_of` with a per-document cursor; `prev_sibling` only detects movement. Verified right-by-design against prod: `get_blocks` drops Page children in both CTE arms, `loro_seams` skips them pre-recursion, the renderer adds no link/stub for them.
- **§3.1** — the subtree fan-out gates on the **doc change itself** (`old_doc != new_doc`), NOT on why it changed. Gating on self-page-toggle was refuted: a cross-document reparent re-homes the subtree identically (deterministic regressions `reparent_into/out_of_a_page_subtree_rehomes_descendants`, red→green). The property alone catches this class only ~20%/run — the hand-rolled regressions are the reliable gate.
- **Boot** — `locate_batch` is the amortization seam (default loops `locate`).

**Inc 1 IMPLEMENTED — see §9 for the as-built record.** All four Inc-1 open items closed:

- **(a) error latch** → RULED "let it die": the terminal latch is CORRECT and stays; no error handling inside the combinator. Recovery is DI-level supervision at **Inc 2** (respawn combinator+accumulator+stream from the container, keep authority/controller, re-seed via `MapDiff::Replace` so recovery path == boot path; bounded restarts then degraded mode). Inc-2 prerequisite: `DegradedSignalBus` is not yet wired. Supervisor gets its own red-first PBT. §9.1.
- **(b) O(subtree) reparent cost** → MEASURED and it bites: N+1 `locate` calls, ≈840 ms extrapolated at N=1000. FIXED via `locate_batch`; calls now constant in N. §9.3.
- **(c) orphan-prefix convergence** → PROVEN, and it *retro-justifies* the per-step closure constraint rather than removing it. Both properties now exist and test different contracts. §9.4.
- **(d) root membership** → RULED children-only, applied at materialization AND in the reference model (one convention everywhere, not normalized at the comparison site). §9.2.

**Inc 2 is UNBLOCKED.** The order-authority question the shadow raised was adjudicated as a defect in the SHADOW'S OWN ORACLE, not in either prod authority: `get_blocks`' `Vec` carries within-parent relative order ONLY, so comparing it as a document sequence was invalid by construction. Prod output is byte-identical either way; canonical authority is `BlockOrdering::children` per ADR-0005. Oracle fixed to compare per-parent. §9.7.
**Ruling this refines:** Martin ruled Option C from
`~/.claude/plans/stale-delta-redesign-options-2026-08-04.md` (§3), and ruled the
ordering representation toward **previous-sibling-id, not `sort_key`-on-the-feed**.
This document resolves that ordering question, specifies the combinator, and lays
out a de-riskable migration.

Absolute paths are given where a file is named so review can read along.

---

## 0. TL;DR

Today `doc_blocks`
(`crates/holon-filesystem/src/file_sync_controller.rs:321`) is a
`HashMap<EntityUri, IndexMap<EntityUri, Block>>` hand-mutated by branches inside
`render_with_cache` (`:3847`). Correctness of one write-back is the conjunction of
~10 guards (options doc §2.4). Option C replaces the hand-rolled maintenance with
**one declared combinator** built in `crates/holon-api/src/live_data/` beside the
existing `group_by` (`group_by.rs`). Its accumulator is the routing+ordering state

```
block id → (owning document, previous-sibling id)
```

derived — like `group_by`'s `key_fn` today — by **authoritative reads**
(`resolve_doc_for_block` for the document, `BlockOrdering::prev_sibling` /
`children` for the order), never from a feed value and never from a `sort_key`
carried on the domain `Block`. The controller then only ever renders what the
holder says; it takes no routing or reseed decisions.

**The decisive resolution:** because both the document and the previous sibling
are read from the **authority inside the combinator**, Option C needs **no feed
schema change and no matview change**, and **ADR-0005 stays intact** (`sort_key`
never touches the domain `Block`). The options-doc assumption that C requires
`sort_key`-on-the-feed (Q2) is dissolved: the feed already lags, so a value it
carried would be stale anyway — the same reason `resolve_doc_for_block` already
reads the authority for the document. Order is read from the authority the same
way.

**The load-bearing precondition, verified (§2.5):** a pure same-parent reorder
**does** deliver a feed event for the moved block. The `block` matview projects
`b.sort_key` verbatim, so `tree.mov_after(X)` changes X's matview row and CDC
emits an `Update` — even though the deserialized domain `Block` shows no changed
field. Evidence in §2.5. Without this the whole order machinery would never run;
with it, no schema change is needed and Q2 stays dissolved.

The safety net is a **zero-runtime-cost keystone**: a per-event fold-equality
property test written **strawman-first** (§6), plus a **`#[cfg(debug_assertions)]`
reconciliation** that asserts holder-output == the current hand-rolled
`doc_blocks` during the shadow increment and is compiled out of release.

---

## 1. Combinator interface

### 1.1 Shape, by analogy to `group_by`

`group_by` (`crates/holon-api/src/live_data/group_by.rs:81`) already re-groups a
`LiveData<Block>` changelog by an async fallible derived **key** (the owning
document), keeping a `BTreeMap<String, K>` accumulator so a key change emits
`Remove(old)` before `Upsert(new)` (Law 1), and re-seeds atomically on
`MapDiff::Replace`/`Clear` (Law 3). It emits `GroupedDiff<K, T>`
(`Upsert{group,key,value}` / `Remove{group,key}`).

Option C generalises the derived state from a bare key `K` to a **pair**
(document, previous-sibling) and emits position with the upsert. The new
combinator — call it `home_by` (it *homes* each block to a document **and** a
sibling slot):

```rust
// crates/holon-api/src/live_data/home_by.rs

/// The derived home of a block: which document owns it, and which sibling it
/// follows under its parent (None = first child). Both fields are authority
/// reads, never feed values — the feed lags, so a carried position would be
/// stale for exactly the reason resolve_doc_for_block already reads authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home<K> {
    pub doc: K,                       // owning document (or an Unresolved sentinel)
    pub prev: Option<String>,         // previous-sibling block id under same parent
}

/// One retraction/addition emitted by `LiveData::home_by`. Mirrors
/// `GroupedDiff` but the Upsert carries the sibling slot so the consumer can
/// place the block without a second read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomedDiff<K, T> {
    Upsert { doc: K, key: String, prev: Option<String>, value: Arc<T> },
    Remove { doc: K, key: String },
}

impl<T> LiveData<T> {
    pub fn home_by<K, F, Fut>(&self, home_fn: F) -> impl Stream<Item = Result<HomedDiff<K, T>>>
    where
        K: Clone + Ord + Send + 'static,
        F: Fn(Arc<T>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Home<K>>> + Send;
}
```

The `home_fn` is the union of today's two authority reads:
`resolve_doc_for_block` (`crates/holon-orgmode/src/di.rs:763`, the ancestor walk
to the owning `Page`) **and** `BlockOrdering::prev_sibling`
(`crates/holon-core/src/block_ordering.rs:89`, the sibling immediately preceding
`id`). Both already exist; `home_by` composes them into one derivation.

### 1.2 Accumulator — precise type

```rust
struct HomeState<S, K, T, F> {
    source: S,
    /// block id → its last-emitted home. The whole point: survives Remove/Clear
    /// (value gone, home retained) so a departure retracts from the right doc,
    /// and survives a same-block content edit so a *position* change is detected.
    acc: BTreeMap<String, Home<K>>,
    pending: VecDeque<Result<HomedDiff<K, T>>>,
    home_fn: F,
    errored: bool,
}
```

This is `group_by`'s `GroupState` with `K` widened to `Home<K>`. The stored
`prev` is what makes a same-parent reorder observable **without `sort_key` on the
value** — the exact wart guard 3 exists to paper over (options doc §2.4, and the
`render_with_cache` comment at `:3830`). Under C, a reorder is a `prev` change in
the accumulator, emitted as an `Upsert` with the new slot; no `ordering.children`
compare in the controller.

### 1.3 Output holder — what the consumer materialises

The `di.rs` resolver task folds the `HomedDiff` stream into the same
`OrgRerender` messages it produces today (`di.rs:611-660`), but the holder — the
thing that replaces `doc_blocks` — is materialised **in the controller** from
those diffs:

```rust
// replaces doc_blocks: HashMap<EntityUri, IndexMap<EntityUri, Block>>
holder: HashMap<EntityUri /*doc*/, DocOrder>,

struct DocOrder {
    /// block id → (its Block value, its previous-sibling id).
    /// parent_id lives on Block (ADR-0005-safe domain field), prev comes from
    /// the accumulator. Render order = DFS from the doc root, each sibling
    /// group linearised by following `prev` (a linked list, None-rooted).
    blocks: HashMap<EntityUri, (Block, Option<EntityUri>)>,
}
```

Render order is reconstructed by traversal (§2.2), replacing the `IndexMap`
insertion-order trick that `render_cached_doc` (`:3918`) relies on today.

---

## 2. Where order comes from — the ruling

### 2.1 Ruling

> **The accumulator's order component is the block's previous-sibling id, read
> from the `BlockOrdering` authority (`prev_sibling` / `children`) inside the
> combinator. `sort_key` is NOT put on the feed, NOT put on the domain `Block`.
> No feed schema or matview change. ADR-0005 stands unchanged.**

### 2.2 Why prev-sibling-from-authority works, and is *cheaper* than today

**Reconstructing positional order.** A document is a tree; render order is a
pre-order DFS in which each sibling group is linearised. Per document, group the
holder's blocks by `Block::parent_id` (domain field), order each group by
following `prev` from the `None`-rooted head, then DFS from the doc root. This
reproduces exactly what `get_blocks` returns today (it is `ORDER BY sort_key,
id`), without the value ever carrying `sort_key`.

**A move updates neighbour pointers — handled by re-reading, not by incremental
pointer surgery.** When `X` moves from after `A` to after `B`, three prev-pointers
change: `X.prev = B`; `X`'s old successor `C` gets `C.prev = A`; `B`'s old
successor `D` gets `D.prev = X`. In the CRDT authority a `sort_key` move
(`tree.mov_after`) rewrites **only the moved block's row**, so **only `X` emits a
feed event**. `C` and `D` change position without their rows changing. If the
holder tried to keep the linked list by writing only `X.prev` on `X`'s event, `C`
and `D` would desync — the fork/cycle hazard the task flags.

The resolution is to **never maintain the linked list incrementally from
per-block deltas**: on any *structural* event for `X` (parent changed, first
appearance, or `prev` changed), the combinator re-reads
`BlockOrdering::children(parent)` **once** (O(siblings)) and refreshes the
prev-pointers of the **entire affected sibling group(s)** from that single
consistent read — both the old parent's group (X departed) and the new parent's
group (X arrived). Because `children()` returns a **total order** derived from
the authority's `sort_key`, a fork or cycle **cannot appear within one read**;
cross-read torn states converge at quiescence, which is contract-legal (Law:
convergence at quiescence, not instantaneous equality). This is the same
O(siblings) cost guard 3 pays today — but it now updates all affected neighbours
from one read instead of comparing one block, so it is *correct by construction*
rather than by conjunction.

**The cheap path gets cheaper.** Detecting "did `X` move?" needs exactly **one**
`prev_sibling(X)` authority read: if it equals the accumulator's stored `X.prev`
and the parent/tags are unchanged, position is unchanged → content-only refresh,
no `children()` scan. Today's cheap path pays `get_block_authoritative` **plus a
full `ordering.children()` read and compare** on every cheap candidate
(`:3872-3886`). One `prev_sibling` read is strictly less than one `children` read
+ Vec compare. So prev-sibling is not a latency cost — it is a latency *win* over
guard 3.

### 2.3 Why `sort_key`-on-the-feed is *not* needed (the Q2 dissolution)

The options doc treated C as gated on Q2 ("does `sort_key` join the feed?").
That gate was premised on the holder carrying order *on the value*. It does not:
order is an **authority read in `home_fn`**, identical in kind to the document
resolution `home_fn` already performs. The feed carries the domain `Block`
(`id`, `parent_id`, `tags`, `content`) unchanged. There is therefore **no ADR-0005
revision, no `block_raw` column, no matview edit**. The only structural authority
consulted is `BlockOrdering`, which already exposes `prev_sibling`/`children`
without leaking `sort_key`.

### 2.4 When would prev-sibling fail, and the costed fallback

The one place prev-sibling could fail is if the authority itself presented a
non-total sibling order (fork/cycle). It cannot: `BlockOrdering::children`
returns a totally-ordered `Vec<EntityUri>` from a valid `sort_key` assignment;
Loro's tree guarantees totality. Concurrent sibling edits are serialised by the
CRDT before the authority read sees them. So the concurrent-edit fork/cycle
concern is answered by *always deriving from the authority's total order and
never from incremental pointer math*.

**Fallback (not taken), costed for completeness.** If a future authority could
not answer `children(parent)` cheaply (e.g. a backend without an ordering index),
the alternative is to put a monotone `sort_key` on the feed and store it in the
accumulator instead of `prev`. Cost: an ADR-0005 revision, a `block_raw` →
matview → CDC schema change across `holon-turso` + `holon-api`, and it
re-introduces "the value carries a lagging order" — a class C exists to remove.
Since every current backend (Turso, Loro, test doubles) implements
`BlockOrdering` with O(siblings) `children`, the fallback buys nothing today and
costs a cross-crate schema change. **Rejected.**

### 2.5 Verified precondition — a pure reorder DOES emit a feed event

This design's order machinery only runs when an event **arrives**. Since ADR-0005
keeps `sort_key` off the domain `Block`, a pure `tree.mov_after(X)` could in
principle change no *visible* domain field and be suppressed by IVM as a
no-visible-change row — which would make reorders silently invisible, strictly
worse than guard 3. **Verified false.** The event fires. Evidence:

1. **The matview projects `sort_key`.** `BLOCK_RAW_COLUMNS`
   (`crates/holon-turso/src/schema_modules.rs:400-419`) lists `"sort_key"` at
   `:404` among "all columns of `block_raw`, projected **verbatim** into the
   `block` matview". The pinned matview SELECT
   (`schema_modules.rs:1329`, asserted exactly by
   `block_matview_select_exact_shape`) reads:
   `SELECT b.id, b.parent_id, b.depth, b.sort_key, b.content, … FROM block_raw b …`.
   So a `sort_key` write to `block_raw` **changes the matview row**, and IVM/CDC
   emits an `Update` for X on row-level change detection — before any domain
   deserialization happens.
2. **The domain `Block` then drops it.** `sort_key` survives only on the separate
   `SnapshotBlock` type (`crates/holon-api/src/block.rs:1189-1199`), whose own
   doc-comment states "The domain `Block` no longer carries `sort_key`
   (ADR 0005)". So the event arrives carrying a `Block` whose fields are all
   unchanged.
3. **Guard 3's existence is the corroborating trace.** `render_with_cache`'s
   comment (`file_sync_controller.rs:3830-3839`) says a same-parent reorder "is
   invisible to a `parent_id`/`tags` comparison alone", so it does a live
   `ordering.children()` compare. A guard that *detects* a reorder can only run
   because the write-back was *triggered* — i.e. the event was delivered. Guard 3
   is empirical proof of delivery.

**Consequence:** change detection happens at the **matview row** level (which
includes `sort_key`), not at the domain-`Block` level. `home_by` therefore
receives an event on every reorder and can compare `prev_sibling(X)` against its
remembered `prev`. **No schema change, no matview change, no partial revival of
Q2.** The design's §2.2 claim stands as written.

---

## 3. Laws 1–3 enforcement, and closing the §2.3 hole

`group_by` already enforces all three for the *document* key; C extends the same
machine to (document, prev) **and** closes the ancestor-repartition hole that
neither `group_by` nor `render_with_cache` handles today.

### 3.1 Law 1 — re-home emits a retraction (remembered previous state)

`process_diff` on `MapDiff::Update{X}` computes `new_home = home_fn(X)`; if the
accumulator's `old_home.doc != new_home.doc`, it queues `Remove{old_home.doc, X}`
**before** `Upsert{new_home.doc, X, new_home.prev}` (exactly `group_by.rs:189-204`,
widened). The controller drops `X` from the old doc's holder and re-renders it
without `X`, and adds `X` to the new doc.

**Closing the §2.3 hole (ancestor gains `Page`) — the part that is genuinely new.**
The derived document is a function of the block's **ancestor chain**
(`resolve_doc_for_block` walks parents to the first `Page`), not of the block's
own row. So when an intermediate node `A` gains the `Page` tag
(`convert_block_to_page`), `A`'s row changes — `A` emits — but its descendants'
rows do **not**, so they emit nothing, yet their owning document has changed from
`A`'s old page to `A` itself. Today both `group_by`'s per-block keying and
`render_with_cache`'s per-block cache miss this until each descendant later
happens to emit; the options doc §2.3 documents it as the residual, self-healing
hole (and it skips `veto_ungrounded_removals` on the cheap path meanwhile).

C closes it **by construction**, and — importantly — **without the accumulator
needing to remember `tags`**. The signal is the toggled node's *own doc change*.
`resolve_doc_for_block` (`di.rs:763-778`) begins its walk at **the block itself**
(`let mut id = block.id`) and returns `current.id` on the first `is_page()` hit,
so:

- when `A` **gains** `Page`: `resolve_doc_for_block(A)` now returns **`A` itself**,
  where it previously returned `A`'s ancestor page ⇒ `remembered.doc != new.doc`
  **and** `new.doc == A.id`;
- when `A` **loses** `Page`: it now returns `A`'s ancestor page, where it
  previously returned `A` ⇒ `remembered.doc != new.doc` **and**
  `remembered.doc == A.id`.

Either predicate is computable from `Home{doc, prev}` alone plus the element key
— no `tags` field is added to the accumulator, and `HomeState.acc:
BTreeMap<String, Home<K>>` (§1.2) stands as typed. On detecting it, the
combinator expands the event — it reads `X`'s current descendants
from the authority (a bounded `get_blocks`-style subtree read, paid only on a
Page-toggle, which is rare) and, for every descendant whose recomputed `doc`
differs from its remembered `doc` in the accumulator, queues `Remove{old} →
Upsert{new}`. Because the accumulator **remembers** each descendant's last doc
(Law 1's raison d'être), this is a precise diff, not a blanket reseed. The
ancestor-repartition case now re-homes the whole subtree in the *same* event that
tags `A`, so no interleaving of two deltas can leave `X` double-owned, and the
removal from `A`'s old page runs through `veto_ungrounded_removals` because every
doc that lost blocks is dirtied by a real `Remove` diff (§3.2).

### 3.2 Law 2 — Remove strictly before Upsert

Enforced by queue order in `process_diff`: the retraction `push_back`s before the
addition for the same element (`group_by.rs:193` then `:200`). The controller
consumes the `pending` queue in order, so no observer sees `X` in two files.
C adds an explicit **vector assertion** in the property test (§6): for every
element whose home changed, the emitted `Remove{old}` index is strictly less than
the `Upsert{new}` index. The controller processes a document's dirty set only
after the full `pending` batch for one input event has drained (the batching
boundary is one `source.next()`), so a doc that both loses and gains blocks in one
event renders once, post-swap.

### 3.3 Law 3 — atomic snapshot swap on reseed

On the boot snapshot (`MapDiff::Replace`) and on `Clear`, `process_diff` retracts
every retained entry then re-seeds from the snapshot in one `pending` batch
(`group_by.rs:164-177, 215-219`). The controller applies the whole batch to the
holder before rendering, so no render observes the state between clear and refill.
This subsumes today's `reseed_doc_blocks` (`:3909`) — a reseed is no longer a
controller decision; it is a `Replace` batch the holder swaps atomically.

**Boot amortization — the `Replace` batch must NOT be per-block point reads.**
Naively, `home_fn` per element costs two authority point reads (an ancestor walk
+ a `prev_sibling`), so a 16k-block cold boot would pay ~32k point reads plus
repeated ancestor walks. That is precisely the cost that forced guard 9
(`snapshot_pending` boot fold, `di.rs:585`) into existence, and re-introducing it
would destabilize cold boot again. `process_diff`'s `Replace` arm therefore takes
a **batch path**, not `home_fn` per element:

- **Order:** one `children(parent)` read **per distinct parent** in the snapshot,
  linearising that whole sibling group at once — O(total blocks) work over
  O(distinct parents) reads, instead of one `prev_sibling` per block.
- **Documents:** resolve **top-down in a single DFS** over the snapshot's parent
  map, carrying the nearest enclosing page down the walk, so each block's doc is
  O(1) from its parent's already-resolved doc instead of an independent
  O(depth) ancestor walk. Ancestor walks are never repeated.

The per-element `home_fn` is used only on the incremental arms
(`Insert`/`Update`/`Remove`), where the delta is one block. Guard 9's boot fold in
`di.rs` is retained unchanged — this design does not change what boot *renders*,
only how the holder is seeded.

**Guards retired by construction:** guard 1 (delta pre-filter `:3853`), guard 3
(`children` compare `:3881`), guard 4 (the `reseeded` bool `:3774` — a dirty
marker; under C every doc the holder emits a `Remove` for is dirtied, so
`veto_ungrounded_removals` runs on exactly those and unconditionally, no boolean).

**Guards explicitly accounted for, not retired:**

- **Guard 2 (authority re-check of `parent_id`/`tags`, `:3872`) — SUBSUMED, not
  merely kept.** It exists only because guard 1 gated on a *feed value* and
  needed a corrective authority read behind it. Under C there is no feed-value
  gate left to correct: `home_fn` **is** the authority read, and its result is
  what the accumulator stores and the holder renders. There is no second, staler
  source to re-check against, so the guard has no proposition left to defend and
  disappears with guard 1 at Inc 2.
- **Guard 8 (page identity-file pre-flight, `:3634`) — RETAINED.** Its stated
  justification ("that router reads the block-feed, whose `is_page` can lag") is
  already stale in the current tree, and C makes it *more* stale — routing now
  provably comes from an authority read. But the guard's real, still-live job is
  **file existence and page renames**, not routing:
  `materialize_page_identity_file` (`:4425`) creates the identity file for a page
  and moves it on a title change (`prior_path` cleanup, guarding
  `inv-every-page-has-its-own-file` against double-homing). A **childless** page
  routes no blocks, so the holder emits nothing for its document and its file
  would never be written. C does not address that, so guard 8 stays. Its
  *comment* must be rewritten at Inc 3 to state the real reason (childless-page
  materialization + rename cleanup) instead of the obsolete router-lag one — the
  exact comment-rot failure mode the options doc §2.4 flags.

---

## 4. What the feed must carry

**Nothing new.** The feed remains `LiveData<Block>` carrying the domain `Block`.
Both derived components are authority reads inside `home_by`:

| Derived component | Source | Already exists? |
|---|---|---|
| owning document | `resolve_doc_for_block` ancestor walk over `BlockReader` | yes — `di.rs:763` |
| previous sibling | `BlockOrdering::prev_sibling` / `children` | yes — `block_ordering.rs:89` |
| parent (for DFS) | `Block::parent_id` domain field | yes |
| page-ness (for subtree re-home) | `Block::tags` domain field + `Block::is_page()` | yes |

The holder reads order **from the structural authority on structural events
only** — never from the feed value — which is precisely the discipline that keeps
ADR-0005 (no `sort_key` on `Block`) and keeps the schema untouched. Contrast the
options-doc §3 Option C bullet ("It also needs the feed to carry `sort_key`"):
that requirement is **withdrawn** by this design.

One thing the feed must keep doing, stated as an explicit precondition rather
than an assumption: **deliver an event when only `sort_key` changes.** It does
today, because the matview projects `sort_key` and change detection is
row-level (§2.5). This is a property of the *existing* schema that C **depends
on but does not introduce** — no change is requested, and the pinned
`block_matview_select_exact_shape` test already fails loudly if it regresses.

---

## 5. Migration increments

Each increment is independently landable and leaves the tree strictly better.
De-risk the combinator as an experiment differentially tested against the current
hand-rolled maintenance **before** deleting anything (project refactoring rule).

**Inc 0 — build `home_by` + red-first property (no production wiring).**
Add `crates/holon-api/src/live_data/home_by.rs` beside `group_by.rs`. Write the
fold-equality property (§6) strawman-first, capture the red, make it green.
Landable: a new, tested, still-unused combinator. Tree strictly better.

**Inc 1 — shadow the hand-rolled cache in debug builds (differential, no
behaviour change). LANDED as built; see §9 for the as-built design and findings.**
Wire `home_by` in `di.rs` **in parallel** to the existing `group_by` resolver,
`#[cfg(debug_assertions)]` only, and compare the two holders **at quiescence**
(NOT per `on_block_changed` — see §9.1 for why the per-event formulation was
withdrawn). Production still writes from `doc_blocks`.

**Inc 2 — cutover: controller renders from the holder; delete the hand-rolled
maintenance. UNBLOCKED (§9.7); adds the renderer re-nesting assertion.**
Switch `render_*` to read the holder. Delete `doc_blocks` (`:321`),
`render_with_cache` (`:3847`), `reseed_doc_blocks` (`:3909`),
`render_cached_doc` (`:3918`), guard 1, guard 3, and the `reseeded` bool (guard
4); make `veto_ungrounded_removals` run on every doc the holder emits a `Remove`
for. Replace `di.rs`'s `feed.group_by(resolve_doc)` with
`feed.home_by(resolve_doc_and_prev)` and forward `HomedDiff` → `OrgRerender`.
Keep the debug-only reconciliation from Inc 1 pointed at a naive recompute (no
`doc_blocks` left to compare against). Landable; deletes code.

**Inc 3 — docs/ADR.**
Add `docs/Architecture/WriteBack.md` stating the holder contract (options doc §1.1
+ the two-holder table) and record the prev-sibling-from-authority ruling
(this §2) as an ADR. Landable.

**Explicitly out of scope:**
- Option B (snapshot seam on `BlockReader`, coalescing drain) — not built.
- The 15k-block `Projects/Holon.org` outlier (store-side incremental per-document
  block sets / chunked render) — its own lane; no option here fixes it.
- `resolve_doc_for_block`'s O(depth) ancestor-walk cost — unchanged.
- The orthogonal guards 5 (TOCTOU `:3751`), 6 (pending-external-ingest `:3717`),
  7 (virtual-seed `:3743`), 10 (quarantine `:3763`) — routing-independent, kept.
- `last_projection` / `last_projection_hash` echo-suppression — unchanged.
- `on_file_changed` ingest direction — unchanged.

---

## 6. The fold-equality property test

Reuse `group_by.rs`'s `run_convergence` structure (`group_by.rs:411-445`)
verbatim in spirit: after **every** input diff, the folded emitted output must
equal a naive recompute from the reference model. This one equality is strictly
stronger than checking Laws 1–3 separately (a missing re-home leaves a stale doc;
a mis-ordered Remove/Upsert double-owns; a torn reseed blanks — all three show up
as `fold != naive`).

**Reference model.** A flat `BTreeMap<block_id, Block>` plus a reference authority
(a deterministic in-test `BlockOrdering` + parent map). `naive_recompute` builds
`doc → ordered [Block]` directly: resolve each block's doc by the reference
ancestor walk, group by `parent_id`, order each sibling group by the reference
`sort_key`, DFS from each doc root.

**fold(emitted).** Apply the `HomedDiff` stream into `doc → {block → (value,
prev)}`, then reconstruct order by the same DFS-over-prev the holder uses.

**Property.** For a generated op sequence (insert / content-update / reparent /
same-parent reorder / **ancestor-Page-toggle** / remove / clear), translate each
op to a valid `MapDiff` + reference-authority mutation, feed the diff to
`home_by`, and after each step assert `fold(emitted-so-far) == naive_recompute`.
Add the Law-2 vector assertion: every re-home's `Remove` precedes its `Upsert`.

**Strawman-first (the proof the property has teeth).** First implement a
**stateless** `home_fn` folded without the accumulator's memory: key each block by
`(resolve_doc(block), prev_sibling(block))` and write only that block on its own
event — **no subtree re-home on Page-toggle, no sibling-group refresh on move**.
Run the property. Red-for-the-right-reason, two independent signatures:
1. the **ancestor-Page-toggle** op leaves descendants homed to the old page
   (`fold` keeps them under the old doc; `naive` moves them) — the §2.3 hole,
   reproduced as a test failure;
2. the **same-parent reorder** op leaves a neighbour's `prev` stale
   (`fold` renders the pre-move order; `naive` renders the new order) — the guard-3
   wart, reproduced.
Capture both red logs in the PR. Then add the accumulator memory + subtree
re-home + sibling-group refresh; both go green. That sequence is the evidence the
property detects the defect class, not merely exercises code.

**End-to-end.** The existing keystone `inv-blocks-match-ref/org` remains the
outer lock; the `home_by` property is the inner, per-event lock at zero runtime
cost. No release-build reconciliation — the debug-only assert (Inc 1) is a
migration aid, not the correctness mechanism.

---

## 7. Blast radius + risk register

**Touched files**
- `crates/holon-api/src/live_data/home_by.rs` — **new** combinator (+ `mod`
  export in `live_data/mod.rs`).
- `crates/holon-orgmode/src/di.rs` — resolver task swaps `group_by` →
  `home_by`; `HomedDiff` → `OrgRerender` mapping (`:611-660`);
  `resolve_doc_for_block` stays, gains a `prev_sibling` companion in `home_fn`.
- `crates/holon-filesystem/src/file_sync_controller.rs` — `doc_blocks` field and
  `render_with_cache` / `reseed_doc_blocks` / `render_cached_doc` deleted; holder
  + render-from-holder added; `veto_ungrounded_removals` gating simplified. Its
  only importer is `holon-filesystem/src/lib.rs` (reverse-deps: 1), so the crate
  boundary is stable.
- Test call sites constructing `BlockDelta` /
  driving the resolver: `crates/holon-orgmode/tests/{incremental_org_writeback_smoke,
  sync_controller_mutation_pbt, name_chain_error_propagation, writeback_readonly_skip,
  vault_path_escape}.rs`. Mechanical; a `mech-executor` job at Inc 2.

**Added by Inc 1 (as built, see §9):**
- `crates/holon-orgmode/src/home_authority.rs` — **new**. `BlockHomeAuthority`
  (production `HomeAuthority` over `BlockReader` + `BlockOrdering`) and `DocHome`.
  Carries the amortized `locate_batch` (§9.3).
- `crates/holon-orgmode/src/writeback_shadow.rs` — **new**. `ShadowInputs`,
  `WritebackShadow`, the quiescence state machine (§9.1).
- `crates/holon-filesystem/src/file_sync_controller.rs` — `cached_doc_orders()`
  accessor (+13 lines), the shadow's comparison target. Deliberately NOT
  `#[cfg(debug_assertions)]`: gating it forces `cfg` onto the `select!` arm bodies
  in `di.rs`, and the accessor is inert when unused.
- `crates/holon-orgmode/src/di.rs` — `run_file_sync_controller` gains a
  `shadow_inputs: Option<ShadowInputs>` parameter (single caller) plus two
  `select!` arms. Inc 1 does NOT swap `group_by` → `home_by`; that is Inc 2.

**Cross-lane dependency — the §2.3 reproducer (task #14).** A separate lane is
building a reproduction of the ancestor-gains-`Page` hole (options doc §2.3 /
Q4). That reproducer is a **dependency of Inc 0's red-log evidence**: its red
scenario becomes the **second, end-to-end (keystone-level) lock** on this design,
complementing the strawman red from §6, which is unit-level on the combinator.
The two are deliberately different altitudes — the strawman proves `home_by`'s
property detects the defect class in isolation; the reproducer proves the same
defect is observable through the real write-back path and that C's subtree
re-home (§3.1) closes it end-to-end. **Inc 0 should not claim its red-log
evidence complete until the reproducer lane lands its scenario**; if that lane
concludes the hole does not reproduce, §3.1's subtree-re-home expansion needs
re-justification on its own terms (it would then be hardening, not a fix) before
Inc 2 spends complexity on it.

**Unchanged public API:** `BlockDelta` stays as-is (unlike Option A, C does not
alter it — the holder is fed the same `Block` values; only the *maintenance* of
the per-doc mirror moves into the combinator). `BlockOrdering` and `BlockReader`
traits unchanged. No schema, no matview, no ADR-0005 change.

**Risk register**
| Risk | Likelihood | Mitigation |
|---|---|---|
| ~~Subtree re-home on Page-toggle reads a large subtree~~ **MEASURED, and the framing was wrong** | **was: high** | Not Page-toggle-only — ANY cross-document reparent fans out (Inc 0 verifier finding). Measured at N+1 `locate` calls, ≈840 ms extrapolated at N=1000. **Fixed** via `locate_batch` (§9.3); calls now constant in N. Residual `subtree_of` BFS is an Inc-2 measurement item (§9.6). |
| `prev_sibling` read cost per structural-candidate event | low | strictly cheaper than today's `children`+compare (guard 3); content-only edits pay one point read |
| "matview gap ⇒ write-back gap, no backstop" (options doc §3 C con) | medium | the debug-only reconciliation (Inc 1) + the keystone catch a feed/holder divergence in test; in prod the holder is a faithful fold of the feed by construction, and any `home_fn` error surfaces loudly (never `Unresolved`-swallowed silently — `di.rs:558` pattern) |
| torn cross-read sibling order during concurrent edits | low | converges at quiescence (contract-legal); each `children()` read is internally total |
| Reconciliation assert masks a real prod-only bug because it is debug-only | low | the keystone property runs release-representative logic; the assert is an *additional* net during migration, removed after Inc 2 stabilises |
| **The shadow never actually runs, so green means nothing** | **REALISED in Inc 1** | The default keystone wiring has no Turso ⇒ no `BlockFeed` ⇒ shadow AND incumbent resolver both inert (§9.5). Mitigation: prove execution by counter/log, never by absence-of-panic; use `bidirectional_sync` as the armed suite. Standing ENVIRONMENT gap raised for its own BugFunnel row. |
| ~~The two order authorities disagree~~ **RESOLVED — it was the shadow's ORACLE** | **REALISED then CLOSED in Inc 1** | `get_blocks`' `Vec` carries within-parent order ONLY (global sort over per-group fractional indices); comparing it as a document sequence was invalid by construction. H2 (`mov_after` lag) REFUTED; prod byte-identical. Oracle now compares per-parent; `get_blocks` contract documented; repro pinned. §9.7. |
| **Cold-boot cost: naive `Replace` = 2 point reads/block (~32k at 16k blocks)** | **medium if unmitigated — this is guard 9's original failure mode** | the batch `Replace` path (§3.3): one `children()` per distinct parent + one top-down DFS for docs, so O(distinct parents) reads not O(blocks); guard 9's boot fold retained; Inc 1's shadow runs against a real cold boot before cutover, and cold-boot commit count is already a pinned keystone metric |
| A reorder stops emitting a feed event (matview drops `sort_key`) | low | §2.5 precondition; `block_matview_select_exact_shape` pins the SELECT string verbatim, so removing `sort_key` breaks that test loudly — the precondition is already test-enforced |

---

## 8. Staleness guard — greps to re-run at each increment start

The options doc is a moving target (guard line numbers drift as the tree lands).
Before each increment, re-establish ground truth:

```bash
# Guard sites and cache surface (expect the render_with_cache family until Inc 2):
grep -n "doc_blocks\|render_with_cache\|reseed_doc_blocks\|render_cached_doc\|reseeded" \
  crates/holon-filesystem/src/file_sync_controller.rs

# The two authority reads home_fn composes (confirm signatures unchanged):
grep -n "fn resolve_doc_for_block" crates/holon-orgmode/src/di.rs
grep -n "fn prev_sibling\|fn children\|fn next_sibling" crates/holon-core/src/block_ordering.rs

# The combinator we build beside (confirm group_by contract unchanged):
grep -n "fn group_by\|enum GroupedDiff\|fn process_diff\|fn run_convergence" \
  crates/holon-api/src/live_data/group_by.rs

# The resolver wiring we swap (group_by → home_by) and its OrgRerender mapping:
grep -n "\.group_by(\|OrgRerender::\|DocGroup::" crates/holon-orgmode/src/di.rs

# BlockDelta consumers (test call sites to migrate at Inc 2):
grep -rln "BlockDelta" crates/*/src crates/*/tests

# §2.5 PRECONDITION — the matview must still project sort_key, or reorders stop
# emitting events and the order machinery silently never runs. Expect a hit in
# BLOCK_RAW_COLUMNS and in the pinned matview SELECT:
grep -n "sort_key" crates/holon-turso/src/schema_modules.rs

# §9.5 — the shadow only arms where a BlockFeed exists (needs Turso). Confirm
# the provider still gates on the block matview watch:
grep -n "BlockFeed(" crates/holon/src/sync/event_infra_module.rs
```

---

## 9. Inc 1 — as built (2026-08-04)

Inc 1 landed with two Martin rulings that changed the design as written above.
This section is the record; where it disagrees with §5's original Inc 1 text,
**this section is canonical**.

### 9.1 Ruling A — the shadow compares at QUIESCENCE, not per event

§5's original Inc 1 said "after each `on_block_changed`, materialise the holder
and `assert_eq!`". **That formulation was wrong and is withdrawn.** `home_by` and
the write-back resolver are *independent subscriptions to the same feed*,
delivered on independent schedules. Comparing after each `on_block_changed`
compares two different feed prefixes: draining the shadow first makes it *at
least* as advanced as the controller, and if any event landed in between, *more*
advanced — reporting lag as divergence. A flaky `panic!` in debug builds would
fire inside the keystone for everyone, which is worse than no shadow at all.

The derived-data contract says the two must agree **once inputs quiesce**, so
that is what is asserted. As built (`crates/holon-orgmode/src/writeback_shadow.rs`):

**State machine.** On each 250 ms tick:

| condition | action |
|---|---|
| activity since last check | clear flag, **reset streak to 0**, skip (not quiescent) |
| quiescent, lists match | reset streak to 0 |
| quiescent, lists differ | streak += 1, `warn!` with the diff |
| streak reaches **`MISMATCH_STREAK_LIMIT`** | `panic!` — genuine non-convergence |

**Why the streak limit is 8, not 2 (§9.8).** A *real* divergence never converges,
so it survives ANY finite threshold — the threshold trades detection **latency**
only, never sensitivity. Two checks (~500 ms of stillness) sits inside this
system's actual convergence window under test load and produced intermittent
false positives; measured, the mismatch streak never exceeds 1 once the window is
generous. Raising the limit is calibration, not desensitisation.

Two non-skipped checks imply no intervening activity *by construction*, which is
what makes "twice in a row" mean "not lag".

**Activity reset — what counts. BOTH sides must be still.** Two independent
sources of movement, and quiescence requires neither has moved:
1. *Feed side* — any `MapDiff` arrival on **either** subscription:
   `note_activity()` on the `rerender_rx` arm (the resolver / `group_by` side) and
   inside `apply()` on every `HomedDiff` (the `home_by` side).
2. *Controller side* — `doc_blocks` is ALSO mutated by paths that emit no feed
   event at all: file ingest, the boot scan, `poll_tracked_files`,
   `re_render_all_tracked`. `check()` therefore snapshots the controller's tracked
   state and treats any change since the previous check as activity. Without this,
   a disk-driven reseed reads as divergence (§9.7).

**Comparison target — ids only, deliberately.** The shadow's values come from the
lagging feed; `doc_blocks`' come from authoritative reads. Equal membership and
order can legitimately pair with different *content*, so comparing `Block` values
would report lag as divergence. Ordered id lists are compared; values are not.

**Panic message.** Names the doc, both ordered lists with counts, and the first
divergent index — diagnosable from a CI log alone, which matters because it fires
inside keystone/integration runs.

**Zero release cost — the mechanism.** `shadow`, `shadow_rx` and `shadow_tick`
are all `Option`, constructed only under `cfg!(debug_assertions)`, and both
`select!` arms await `std::future::pending()` when the option is `None` — a future
that never wakes. There is therefore **no `cfg` attribute on any `select!` arm**
(which tokio does not reliably support) and **no timer armed in release**.
`cargo check --release -p holon-orgmode` is clean.

**The combinator keeps its terminal error latch** ("let it die"). The shadow's
forwarding task logs at ERROR and stops when the stream errors; supervision is an
Inc-2 concern (respawn combinator+accumulator+stream fresh from the container,
keep authority/controller, re-seed via `MapDiff::Replace` so recovery path ==
boot path; bounded restarts then degraded mode). **Inc-2 prerequisite: the
`DegradedSignalBus` is not yet wired.** The supervisor gets its own red-first PBT
at Inc 2 (inject authority failure mid-sequence → death → respawn → fold-equality
holds post-restart).

### 9.2 Ruling B — children-only membership, one convention everywhere

The per-doc member list the holder exposes **excludes the document root**, making
it a drop-in for `get_blocks`/`doc_blocks` consumers. The root stays in the
accumulator internals, where page-toggle detection needs it (§3.1).

Applied in **both** places rather than normalized at the comparison site:
- materialization — `WritebackShadow::materialize` nests using the root, then
  drops it from the exposed list;
- reference model — `naive_recompute` skips `doc == id`; `reconstruct` retains-out
  the root after nesting.

Follow-on this surfaced: `scenario()` was not pruning empty docs the way
`run_convergence` does, so a document whose only member was its own root compared
`{"p0": []}` against an absent entry. Fixed to the same pruning — an absent entry
and an empty one mean the same thing.

### 9.3 The `locate_batch` fan-out fix

§7's risk register listed the O(subtree) reparent cost as "measure it". Measured
(`reparent_fanout_cost`), it **bites**, and the fan-out now routes through
`locate_batch` instead of a per-descendant `locate`.

Authority **calls** for a cross-document reparent of a subtree with N descendants:

| N | before | after |
|---:|---|---|
| 10 | locate=11, total=16 | locate=1, batch=1, **total=7** |
| 100 | locate=101, total=106 | locate=1, batch=1, **total=7** |
| 1000 | locate=1001, total=1006 | locate=1, batch=1, **total=7** |

Exactly **N+1 `locate` calls**, and the synthetic authority *understated* it:
production's `locate` is itself an O(depth) ancestor walk and its `subtree_of`
issues one `children` per node, so the real cost was ≈ N×(1+depth) point reads
+ N `children` reads. At N=1000, depth≈5, and the Q3-measured
`get_block_authoritative` p95 of 0.13–0.15 ms, that extrapolates to **~840 ms on a
single Turso actor that head-of-line-blocks the SLO chain** — far past any
defensible budget, and the number that justified the fix.

**Threshold used: any per-descendant authority call is unacceptable**, because the
fan-out is unbounded in subtree size while the SLO is fixed.

`BlockHomeAuthority::locate_batch`
(`crates/holon-orgmode/src/home_authority.rs`) does one row read per block plus a
single memoized top-down document pass, so no ancestor walk is ever repeated:
production reads drop N×(1+depth) → N, and authority calls become constant in N.

### 9.4 Orphan-prefix convergence, and why the per-step closure constraint stands

Inc 0 constrained the generator to keep the feed **ancestor-closed**, which was
flagged as possibly hiding a real transient. It does not.
`prop_orphan_prefix_converges_at_quiescence` (256 cases × 3 runs, green) builds
the authority tree first, then delivers every live block in an
**ancestry-ignoring order** — children routinely arrive before parents — and
asserts equality once the feed catches up.

This **retro-justifies** the per-step constraint rather than removing it. Mid-flight
the holder is *legitimately* ambiguous: a block whose parent has not arrived has
no in-document ancestor to nest under, so two blocks can both be "first in their
group" with nothing to order them against. The two properties test two different
contracts and both are needed:
- `prop_convergence` — equality after **every** step, on an ancestor-closed feed
  (which is what the real matview-mirror feed is);
- `prop_orphan_prefix_converges_at_quiescence` — equality **at quiescence**, on an
  arbitrarily-ordered feed.

### 9.5 Keystone wiring correction — an ENVIRONMENT gap

An earlier Inc-1 claim that "every keystone run is a differential test for free"
was **half wrong** and is corrected here.

Right: the keystone runs the `test` profile (plain `cargo test`, no `--release`,
and workspace `[profile.dev]` does not override `debug-assertions`), so
`debug_assertions` is on and the shadow **compiles in**.

Wrong: `keystone-smoke` passed with **zero shadow executions**. `BlockFeed` is
registered only by `EventInfraModule`, which requires a Turso
`watch("SELECT * FROM block")` matview. The drawn wiring was
`storage={Loro, Org}` with no Turso, so `block_feed` is `None` and **neither the
shadow nor the incumbent `group_by` resolver runs**. Zero `[OrgMode]` /
`FileSyncController` lines in that log confirm the whole feed-driven write-back
path is inert in that wiring.

> **ENVIRONMENT gap, worth its own BugFunnel row.** The composed keystone's
> default wiring draw never exercises the feed-driven org write-back path at all.
> Every guard in `render_with_cache`, the whole `group_by` resolver, and
> `OrgRerender` routing are unexercised unless the draw happens to include Turso.
> This is not a shadow problem — it is a standing hole in what the keystone
> covers, and it means "the keystone is green" has never been evidence about this
> layer.

**Armed suite:** `crates/holon-integration-tests/tests/bidirectional_sync.rs`
(12 tests, ~6 s) is the cheapest test that actually arms the shadow.
`stale_rewrite_sibling_order.rs` also arms it. Proof of execution is the
`[writeback-shadow]` counter/log line, never absence-of-panic.

### 9.6 `subtree_of` BFS — Inc-2 measurement item

`BlockHomeAuthority::subtree_of` walks descendants breadth-first, issuing one
`ordering.children()` per node, because `children` is the only ordered-descendant
read the seams expose. After §9.3 this is the remaining per-descendant cost on a
cross-document reparent. Replacing it with a single doc-scoped read trades
O(subtree) reads for one O(document) read — better for large subtrees, worse for
small ones in large documents. **Not fixed now; measure against real Turso at
Inc 2 before choosing.**

### 9.7 RESOLVED — the order-authority question was an ORACLE defect

The shadow fired on its first armed run. Adjudicated verdict: **the defect was in
the shadow's own oracle, not in either prod authority.** Inc 2 is **unblocked**.

**What was wrong.** `WritebackShadow::compare` normalized the holder to a
document pre-order and compared it as a **flat sequence** against `doc_blocks`,
which is seeded from `get_blocks`' `ORDER BY sort_key, id`. That is a **global**
sort over fractional indices minted **per sibling group**: every group restarts at
the same low key (`"80"`), 1861/1889 live blocks share a `sort_key` with another
row, and the exact prod CTE returns **14 consecutive `sort_key="80"` rows from 14
different parents at depths 1–4**. Cross-parent ties fall to the `id` tiebreak and
interleave the sequence. So `get_blocks`' `Vec` is a **set with within-parent
relative order only — never a document sequence**, and comparing it against a
pre-order walk was invalid by construction.

The reported `doc=block:journals` divergence was exactly this: `journals::action::0`
is a **depth-2 grandchild**, the sole child of `auto-create`, so its own group
restarts at `"80"`, ties with the depth-1 keys, and UUID-sorts to position 0. **No
move was involved** — H2 (`mov_after` projection staleness) is **REFUTED**: the
Loro projector tie-suffixes within groups, dirties both scopes on `Move`, and
diffs `sort_key` first.

**Prod output is unaffected.** `render_entitys` re-nests by `parent_id` and reads
only within-parent order, which the global sort preserves. Rendering is
byte-identical from either sequence — proven by
`crates/holon-org-format/tests/get_blocks_flat_order_is_not_document_order.rs`,
adopted into this increment (two cases: the flat comparator reproduces the
interleaving; the render is byte-identical across the two orders).

**Canonical order authority = `BlockOrdering::children`**, per ADR-0005:
`sort_key` is a storage encoding chosen by one adapter, not a cross-parent
ordering key.

**Fixes landed in this increment:**
1. `WritebackShadow::compare` normalizes **both** sides to `parent -> [child ids]`
   and compares per-parent — the only ordering either side actually claims.
2. `BlockReader::get_blocks` (`crates/holon-filesystem/src/sync_ports.rs`)
   documents the within-parent-only guarantee and **names both impls**, which
   legitimately differ: `LoroBlockReader::collect_subtree` is a pre-order DFS (its
   sequence *is* document order, incidentally); `CacheBlockReader` is the flat
   global sort. No future consumer should read the result as a sequence.
3. The Turso CTE is **deliberately NOT changed** to pre-order. That is an **Inc-2
   consideration**: making `get_blocks` return a genuine document order would let
   consumers rely on the sequence, which is a contract widening, not a bug fix.

**Load-bearing note for Inc 2.** After cutover the holder hands the renderer a
**genuine document order**, where `doc_blocks` handed it an interleaved set. That
is safe *only because* `render_entitys` re-nests by `parent_id` and ignores
cross-parent sequence. **Inc 2 must add an explicit assertion pinning that
re-nesting property**, so a future renderer change that starts trusting input
sequence cannot silently break write-back.

**A second, independent oracle gap** surfaced while fixing this: quiescence
accounted only for *feed* activity, but `doc_blocks` is also mutated by paths that
emit no feed event — file ingest, the boot scan, `poll_tracked_files`,
`re_render_all_tracked`. A disk-driven reseed therefore looked like divergence
(membership mismatches, e.g. holder 0 children vs cache 3). `check()` now requires
**both sides unchanged** between consecutive checks: feed silence alone is not
quiescence. §9.1's state machine is updated accordingly.

With both oracle defects fixed, `stale_rewrite_sibling_order` is **GREEN with the
shadow still armed** (53 invariants 7/7), and the shadow continues to execute and
compare on `bidirectional_sync`.

### 9.8 Shadow interference, the arming policy, and the flakiness discipline

**The finding.** The shadow is not observationally neutral. It issues authority
reads of its own — `locate` (1 + depth `get_block_authoritative` calls),
`children_of`, `prev_sibling` per feed event, plus `locate_batch`/`subtree_of` on
structural events — through the same Turso actor the `inv-sql-budget` oracle
measures. Establishing the baseline settled it:

| `stale_rewrite_sibling_order` | result |
|---|---|
| shadow **OFF** (`HOLON_SHADOW_OFF=1`), 6 runs | **6/6 green** |
| shadow **ON**, 6 runs | **3/6 fail** |

The failing runs carried **0 shadow markers and 0 panics** — the failure was
`inv-sql-budget`: `PinBlock.sql_reads: 29 exceeds expected 17 + tolerance 1 = 18`,
against a baseline of exactly `PinBlock: reads=17/17`. Not a divergence: read
inflation by the measurement apparatus.

**The ruling.** The read-budget oracle exists to measure PROD read behaviour. The
shadow is transient measurement apparatus — debug-only, and deleted at Inc 2
cutover — so its reads are legitimately excluded, but by **arming policy**, never
by touching budget numbers or instrumentation:

- **Default-ON in debug builds**, keeping the free differential surface.
- **Budget-asserting suites opt out at the suite level, in code** —
  `writeback_shadow::disable_for_budget_suite()`. Not a per-developer env var.
  (`HOLON_SHADOW_OFF` remains as an ad-hoc lever for re-measuring interference.)

  **Placement is load-bearing: arming is read ONCE at feed construction**
  (`di.rs`), so the disable must precede the caps being booted. It therefore
  lives at cap construction — `boot_and_seed_wide_with_peer_id` (headless) and
  `boot_and_seed_wide_windowed_base` (windowed) in `pbt/composed/wide_e2e.rs` —
  NOT in `harness.rs::from_parts`/`init_test`, which run AFTER the boot and are
  a no-op for that session. `SHADOW_ARMED` is process-global and sticky, so a
  late disable would also expose only the FIRST windowed case per binary — an
  order-dependent flake generator. Windowed budget consumers this protects:
  `gpui_compose_sut_windowed.rs` and the tui `pbt_main.rs`, both reaching
  `inv-sql-budget` through `windowed_composed_sut`.
- **`single_large_document_scale` also opts out.** Its oracle is a wall-clock
  ingest budget, and the shadow issues authority reads on the same Turso actor —
  measurement apparatus competing with the thing being measured. Its budgets are
  generous (40 ms/block, 60 s floor), but "generous" is not a reason to admit
  apparatus into a latency measurement at 25 000-block scale; the read-budget
  principle applied consistently.
- The `holon-orgmode` mutation PBT needs no opt-out: it asserts no read budget
  and drives `FileSyncController` directly, so the shadow never arms there.
- **The armed differential suite gates on shadow liveness — and on the right
  observable.** `bidirectional_sync::shadow_completes_at_least_one_quiescent_comparison`
  asserts three things:
  1. `completed_comparisons() >= 1` — the tick loop reached quiescence;
  2. `docs_compared() >= 1` — **the load-bearing one**: `completed_comparisons`
     counts tick-loop iterations and rises even on an empty tracked slice, so it
     alone never proves any holder content was checked against anything;
  3. `!divergence_detected()` — the non-convergence panic fires on a *spawned
     task*, which does **not** fail a plain `#[test]`. The composed harness
     catches that via `inv-no-observed-errors`; this suite has no such invariant,
     so without an explicit flag a real divergence here would be swallowed and
     the gate would be decorative.

  Its fixture is **deliberately nested** (document → child → two grandchildren,
  i.e. ≥2 sibling groups). A single-group fixture makes a flat sequence and a
  per-parent grouping identical *by construction*, so it cannot witness the §9.7
  oracle defect at all.

  **Mutation-proven.** Re-applying the exact flat-compare mutation (collapse both
  sides to one constant group) makes this suite FAIL — "the write-back
  differential shadow reported NON-CONVERGENCE", off a real document
  (`doc=block:journals`, 4 children, first divergent index 1). Reverting restores
  green (3/3). Before the nesting and the `docs_compared`/divergence gates, that
  same mutation left every suite green.

**Release neutrality is by dead-code elimination, not `#[cfg]`.** The guard is
`cfg!(debug_assertions)` — a const-false branch in release, so the shadow costs
zero runtime work and both `select!` arms await `pending()`. But the modules
(`writeback_shadow`, `home_authority`) and `FileSyncController::cached_doc_orders`
**remain compiled and public in release**: inert, not absent. An accepted Inc-1
cost, not a claim of an unchanged artifact; Inc 2 deletes them outright.

**Keystone: OFF for now.** Its default wiring draws never arm the feed path
anyway (§9.5), and its click-transition budget pins (17/22) would break under
Turso draws. **Inc-2 disposition:** at cutover the holder becomes production, its
reads ARE prod reads and count normally, and the shadow is deleted along with
these exclusions.

**Convergence-window calibration.** `MISMATCH_STREAK_LIMIT = 8`, not 2. A real
divergence never converges, so it survives ANY finite limit — the limit trades
detection **latency** only, never sensitivity. Measured, a lagging feed's
mismatch streak never exceeds 1; two checks (~500 ms) sat inside this system's
actual convergence window under test load and produced intermittent false
positives.

**What the membership scare turned out to be.** A block reported missing was
`ABSENT FROM HOLDER ENTIRELY` — not mis-homed, not `Unresolved`, and the terminal
latch never fired. So `BlockHomeAuthority` never failed to route it; the holder
simply had not received it yet. **There is no no-Page-ancestor drop risk:**
`BlockHomeAuthority::resolve_doc` (`home_authority.rs`) mirrors prod's
`resolve_doc_for_block` (`di.rs:763-778`) walk-for-walk, and the membership rules
are exact inverses — `get_blocks` walks DOWN from the doc's children excluding
`Page`-tagged blocks (`crates/holon-app/src/turso_seams.rs:274-289`), while
`resolve_doc_for_block` walks UP to the nearest `Page`. The panic message now
names, for each cache-only block, whether it is homed elsewhere or absent
entirely, so this question is answerable from a CI log alone.

**Flakiness discipline (binding for this workstream).** Two lucky-seed incidents
happened here: Inc 0's property passed on a lucky seed and was refuted by the
verifier, and Inc 1's `stale_rewrite_sibling_order` was reported green off a
single 2/2 run that later ran 3/6 red. Rules now: **every gate claim states its
run count**; anything the shadow touches needs **≥3 consecutive green runs**, and
`stale_rewrite_sibling_order` needs **6** (it burned us twice). No single-run
claims.

---

## 10. Inc 2 — cutover plan, prerequisites, and gates (consolidated)

Everything gating the cutover, in one place. §5's Inc-2 paragraph states the
*scope*; this section states what must be true BEFORE that scope is safe, drawn
from the Inc-0/Inc-1 findings (§9) and the session rulings.

### 10.1 Scope (unchanged from §5)

Controller renders from the holder. Delete `doc_blocks`, `render_with_cache`,
`reseed_doc_blocks`, `render_cached_doc`, guard 1 (delta pre-filter), guard 3
(`children` compare), guard 4 (the `reseeded` bool); `veto_ungrounded_removals`
runs unconditionally on every doc the holder emits a `Remove` for. `di.rs` swaps
`feed.group_by(resolve_doc)` → `feed.home_by(...)`. The shadow and
`cached_doc_orders()` are DELETED (their job ends when there is no second holder
to differ from).

### 10.2 Prerequisites — each independently checkable

1. **DI supervisor for stream death, red-first** (let-it-die ruling 2026-08-04).
   The terminal error latch stays; recovery = DI respawns combinator +
   accumulator + stream fresh, re-seeded via one atomic `Replace` (recovery
   path == boot path). Bounded restarts, then escalate to degraded mode. The
   supervisor lands with its own PBT: inject an authority failure
   mid-sequence → stream dies loudly → respawn → fold-equality holds
   post-restart.
2. **Degraded-mode escalation must be audible end-to-end.** The bus +
   unconditional bridge landed (`2a3db504`), but `OrgIngestFailed` and five
   other reason kinds are NON-STICKY and can race window launch (task #22).
   The supervisor's "kept dying" signal must be a sticky reason (or emitted
   post-bridge) before cutover — otherwise let-it-die degrades to
   die-quietly on exactly the persistent-failure path it exists to disclose.
3. **Renderer re-nesting assertion.** `home_by` hands the renderer a genuine
   document order; today's `doc_blocks` hands it a flat global sort. Byte-
   identity across the two was PROVEN (§9.7) *because the renderer re-nests by
   parent* — Inc 2 adds an explicit assertion/test pinning that re-nesting
   contract so a future renderer "optimization" that starts trusting input
   order fails loudly.
4. **Root-membership mapping.** Holder emission is children-only (§9.2), same
   convention as `get_blocks` — but the reference model's root-in-own-doc
   off-by-one (§9 header, item d) must be reconciled at the exact seam where
   `HomedDiff` replaces `doc_blocks` as the render input.
5. **`subtree_of` measurement** (§9.6): one `children` read per node today;
   measure against real Turso before cutover; single doc-scoped read is the
   fallback if it dominates.
6. **Keystone budget re-pin.** Under Inc 2 the holder's authority reads ARE
   prod reads: `inv-sql-budget` pins (PinBlock 17/OpenTab 22 etc.) will move
   and must be re-derived from the new steady state, not tolerated upward
   (fix-cap-not-withhold). Also resolves the shadow arming exclusions (§9.8)
   by deletion.
7. **Keystone ENVIRONMENT gap** (§9.5): the composed keystone's default draw
   never exercises the feed-driven write-back path. Inc 2 makes the holder
   the ONLY write-back path — a keystone that never draws Turso would then
   test write-back nowhere. The wiring draw must include the feed-driven path
   before the hand-rolled one is deleted.
   **ADDRESSED 2026-08-04.** The gate is TURSO ALONE, not Turso+Org: the
   composed builder (`pbt/composed/builder.rs`) branches only on
   `has_turso` / `has_loro` / `has_frontend` — there is no `has_org`, so a drawn
   `StorageAdapter::Org` changes nothing the SUT boots (it only flips the
   `org-writeback=on` text in `authority_routing_disclosure`). Measured share of
   accepted draws carrying Turso was **0.312**; `QUERY_ADAPTER_INCLUSION_PROB`
   0.15 → 0.20 lifts it to **0.388**, above the new
   `MIN_QUERY_ADAPTER_DRAW_SHARE` = 1/3 floor that
   `query_adapter_draw_share_meets_writeback_floor` now enforces. Reachability
   is proven, not assumed: `turso_draw_reaches_the_feed_driven_writeback_path`
   asserts `org.on_block_feed` spans fire on a Turso draw (observed 54), and a
   Turso-drawing keystone case engages `inv-blocks-match-ref/org` on every tick
   (19/19, 13/13, 28/28, 35/35 across the 2026-08-04 runs) while Turso-free
   cases never select it at all.
8. **BlockDelta test-callsite sweep** (§7): mechanical `mech-executor` job,
   listed test files.

### 10.3 Gates at cutover

- Fold-equality properties (both) + hand-rolled regressions, run-count
  discipline per §9.8.
- `inv-blocks-match-ref/org` end-to-end on the composed keystone WITH a
  Turso-inclusive draw.
- Byte-stability: render byte-identity harness (adopted §9.7 test) extended to
  assert org output unchanged across the cutover commit on a real-vault copy.
- Supervisor PBT (10.2.1) green; kill-component chaos transition follows as
  the crash-resilience keystone extension (vault: Crash Resilience).
