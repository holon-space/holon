# Reactivity: the Derived-Data Contract

*Part of [Architecture](../Architecture.md). Detail page for
[Model.md](Model.md) layer 4 (matviews → CDC → `LiveData` / cells). Ratified
2026-07-24 after the cross-doc-move convergence arc proved the contract
generative: it predicted where the bug lived (a stateless transformer), what
the fix was (a stateful combinator), and what to enforce (differential
invariants). See also [Sync.md](Sync.md) §Sync Wiring for the pipeline this
governs.*

## The contract (MatView = View, at quiescence)

> Every component that **holds derived data** must, once its inputs quiesce,
> equal a live recomputation of that data from the most current upstream
> state — within a bounded convergence window.

Three refinements that make this precise:

1. **Convergence, not instantaneous equality.** Mid-flight frames may lag
   (IVM maintenance, debounced renders). The standard is equality at
   quiescence within a bounded window; oracles therefore use
   bounded-wait-for-stable, never one-shot strict snapshots
   (`inv-org-render-fixed-point`, `inv-matview-consistent-with-recompute`).
2. **Applies to every derived holder, not just SQL matviews.** Turso matviews,
   `LiveData` mirrors, stream combinators, per-doc render caches
   (`render_with_cache`'s `doc_blocks`), UI row snapshots
   (`ReactiveRenderedRows`) — anything that caches a function of upstream
   state owes the same equation.
3. **A violation is an architecture bug, not a call-site bug.** When a holder
   diverges permanently, the fix belongs in the reactive layer's abstractions
   (a missing combinator, a wrong delta, a broken atomicity promise), not in a
   bespoke cache-invalidation patch at the consumer. The 2026-07 echo was
   fixed by adding `LiveData::group_by`, not by teaching the org renderer to
   second-guess its cache.

## Corollaries (each earned by a real defect)

### 1. Re-grouping a keyed changelog by a derived key requires state

A stream of `MapDiff`s keyed by element id, grouped by a *derived* key
(block → owning doc), cannot be transformed statelessly: when an element's
derived key changes, the retraction for the **old** group can only be emitted
by something that remembers the old key. A stateless map is structurally
incapable of it — the old group silently retains the element forever
(the permanent-echo class: the source doc's cache wrote a departed child back
to disk in perpetuity).

The abstraction is `LiveData::group_by`
(`crates/holon-api/src/live_data/group_by.rs`): an internal accumulator
(element key → last group key), seeded from the initial `Replace` snapshot,
emitting `GroupedDiff::Remove { group: old }` **strictly before**
`Upsert { group: new }` on a key change, and routing feed-level `Remove`s via
the accumulator (the value is gone; retained state is the only truth).
Consumed by the org-sync doc router (`crates/holon-orgmode/src/di.rs`).

### 2. Retraction before assertion

On a group change, `Remove{old}` precedes `Upsert{new}` — pinned by vector
assertion in the combinator's tests, not just fold-equality. Consumers that
process sequentially then see a move as departure-then-arrival, never as a
transient duplicate.

### 3. Intermediate states must be non-destructive (atomic re-snapshot)

A resync/reseed that is logically "replace everything" must be **delivered**
atomically: a new-generation snapshot swap, never a same-generation
delete-all followed by re-inserts. An observer between the delete and the
re-insert sees an empty projection that recomputation would not produce — a
contract violation even though the end state converges. (Known violation as
of 2026-07-24: the block-projection reseed emits a phantom all-delete at the
same UI generation, blanking the Main panel >3s; fix lane running. The
creation-slot panic is that violation's loud witness — do not silence the
witness, fix the delivery.)

The mechanism on the UI read model is
`LiveData::replace_all` (`crates/holon-api/src/live_data.rs`), which goes
through `MutableBTreeMap::replace_cloned` and so emits ONE `MapDiff::Replace`.
The Loro projection's reseed calls it immediately before it swaps its own diff
base (`crates/holon-loro/src/loro_sync_controller.rs`, the `may_seed_live`
arm). `insert`/`remove` in a loop is the violation this corollary names; a
producer that re-derives its whole set must use `replace_all`.

### 3b. The read model is emitted at the commit, before the index write

D172.a. In CRDT mode Loro is the authority and SQL is a derived index, so the
UI's read model is published when the Loro commit is projected — **before**
the SQL sink write — and the projection's `live` map stays a private diff base
that advances only after that write commits. Three rules make this a second
*consumer* of one diff rather than a second *source of truth*:

- **Deduplicate at the source.** Publish the projection's `staging` (already
  stripped of no-ops by `blocks_differ`), never the raw `changed` set.
  Re-publishing an unchanged row is the churn this model exists to remove.
- **No lock across the publish.** Emit owned values after the `live` guard has
  dropped. A `MutableBTreeMap` write while holding the projection's
  `StdMutex` is a deadlock/jank hazard.
- **Ordering is the projection's, not the publisher's.** `project()` is
  serialized on `project_lock`, so consecutive commits publish in commit
  order. Publishing outside that lock would let an older keystroke land after
  a newer one.

**A pass can publish twice.** The delta goes out before the pass branches,
and two of the three branches (an unarmed delete, an FK-ungrounded batch)
route to the full walk instead of writing the sink incrementally — which then
publishes an atomic snapshot over it. So the contract is "the read model never
lags the authority", not "exactly one publish per commit"; a consumer must
tolerate a snapshot arriving after a delta it already applied. `replace_all`
makes that second publish one generation, so the overlap is invisible
downstream.

Because the read model leads the index, a single read-model/SQL difference is
the ordinary in-flight window, not a defect; only a *persistent* one is. And a
failed sink write means the **index** is wrong, not the read model: the
correction is to disclose (a condition on the bus) and repair by reseed —
never to roll the UI back to a stale index. **Every leg that can fail after a
publish discloses**, through one definition
(`LoroProjection::disclose_read_model_ahead_of_index`) that both the
incremental apply and the full walk call — a leg that failed quietly would
leave the UI ahead of the index with nothing said. Pinned by
`inv-view-model-matches-store-at-quiescence`, which compares read model ⇄
`live` ⇄ SQL at quiescence with a bounded wait, and whose capability is
present exactly where a Loro projection is: a slice without one deselects the
invariant, rather than reporting agreement against an empty comparison.

### 4. Error policy in combinators: fatal vs encoded fallback

A combinator's `Err` item ends the stream permanently — correct for corrupt
internal state (continuing would fold on garbage), far too fatal for
transient upstream faults. The pattern: **encode recoverable fallbacks in the
key/value domain** (`DocGroup::{Resolved, Unresolved}` — `Unresolved` routes
to a disclosed full re-render) and `tracing::error!` loudly, reserving
stream-`Err` for genuinely unrecoverable states. Never a silent skip: that is
the swallowed-error anti-pattern with extra steps.

### 5. Enforcement is differential, not anecdotal

Two mechanical patterns keep the contract honest:

- **Differential invariants**: derived holder vs direct recomputation,
  sorted-multiset diff, bounded-wait — `inv-matview-consistent-with-recompute`
  (every matview vs its defining SELECT), `inv-org-render-fixed-point`
  (rendered file vs re-render from SQL). Each has a fault-injection seam
  (`HOLON_PBT_MATVIEW_STALE`) proving the red end-to-end.
- **Per-event fold-equality PBTs** for combinators: after every input diff,
  the fold of emitted output diffs must equal the naive recomputation over
  the reference model (`prop_convergence` in `group_by.rs`), with a
  **stateless strawman red** proving the property catches the real defect
  class before the implementation exists.

When an oracle cannot go red deterministically (a race needs an injection
seam that doesn't exist), that missing seam is itself a reportable gap — park
the test with the reason, build the seam, then un-park.

## What this contract is *not*

- Not an event log: layer 4 is convergent state; recovery is resync
  (`Replace`), not replayed acks ([Model.md](Model.md) layer table).
- Not instantaneous: SLO-bounded lag is legal; *permanent* divergence and
  *destructive* intermediate states are not.
- Not a license for defensive re-derivation at consumers: a consumer that
  re-queries upstream "just in case" hides the layer's bug and pays the cost
  forever. Trust the contract; enforce it with invariants.
