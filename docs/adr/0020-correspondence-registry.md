# ADR 0020: The correspondence registry

**Status:** Accepted (retroactive — documenting shipped architecture)
**Deciders:** Martin
**Context:** Holon's composed PBT is *differential*: it runs one reference model
against N system-under-test *stores* — the write-side `block_raw` table, the
CDC-driven `block` matview, the Loro CRDT log, the org render, the editor mirror
— and asserts they all agree with the reference. Historically each such
"observable = agree across stores" check was a hand-written `CapInvariant`, and a
first sweep of the *clean 1:1* cases was single-sourced through the
`capability_pair!` macro. The `capability_pair!` convertible set is now provably
exhausted; everything richer than a two-value 1:1 edge lives in the
**correspondence registry** (`crates/holon-integration-tests/src/pbt/correspondence.rs`),
landed through Phase 2. This ADR records what "correspondence" means, why a
declarative registry replaced/complements the macro track, and the type-safety
it buys.
**Relates to:** ADR 0012 (reference-model capability contract — the `Ref*`/`Sut*`
traits the registry reads), ADR 0009 (component-subset PBTs — the `Needs`
selection the registry reuses per store), ADR 0007 (wiring manifest).

## Problem

The composed keystone PBT compares one reference model against several **storage
representations** of the same logical fact. "The set of non-seed blocks" must be
identical whether read from the write-side `block_raw` table, the CDC matview, or
the Loro log; "the active editor's caret" must match between the reference mirror
and the SUT mirror. Each such check is one *logical observable* projected into
several *stores*.

Two approaches were tried and both hit a ceiling:

1. **Hand-written `CapInvariant` per store.** Every `inv-blocks-match-ref/matview`,
   `inv-block-content/sql`, etc. was its own struct with its own `needs()`,
   extraction, comparison, and convergence handling inline. Adding a store to an
   observable, or adding an observable, meant copying a whole invariant body — and
   the extraction/comparison logic drifted per copy. Worse, comparison strategies
   were sometimes closures buried *inside* SUT impls, so the thing under test
   influenced how it was judged.

2. **The `capability_pair!` macro** (`crates/holon-macros/src/capability_pair.rs`).
   One declaration emits both the `Sut*` and `Ref*` read traits and — for
   `#[compare]` methods — auto-derives an equality invariant. This is the right
   tool for a **clean 1:1 two-value edge** (one arg-less method per side returning
   the whole comparable value). But a 2026-07-03 body-level audit
   (`capability_pair.rs:55`) found the convertible set is *structurally capped* at
   exactly four invariants and now **exhausted**:
   - The SUT↔Ref comparison graph is **many-to-many, not 1:1**. Hub reference caps
     (`RefBlockTree` ×3, `RefBackend` ×3, `RefLayout` ×2) answer several questions
     for several SUT caps; a trait can live in at most one pair, so pairing a hub
     would orphan its other edges.
   - `#[compare]` is **strictly two-value**. Most invariant bodies read 2+ methods
     per side, loop over per-id parameterized reads, or need 3-valued
     `Result`-Skip / borrow returns (editor caret/text).

So the majority of differential checks — hub-fed, multi-cap, 3-valued, or with
per-store convergence policy — had no single-source home. That is the gap the
registry fills.

## Decision

### 1. A correspondence = one logical observable × N store projections

The registry (`crates/holon-integration-tests/src/pbt/correspondence.rs`) models
each differential check as data:

- `Observable` (`correspondence.rs:66`) — a trait with an associated
  `type Value: Debug` and a `const NAME: &'static str`. `NAME` is the invariant-id
  family stem: each store emits `inv-<NAME>/<store>`.
- `Correspondence<O>` (`correspondence.rs:126`) — exactly one **reference
  projection** (`ref_project: fn(&CapMap) -> Extraction<O::Value>`) plus a
  `Vec<StoreProjection<O>>`.
- `StoreProjection<O>` (`correspondence.rs:111`) — one SUT store's view: a written-
  out `id`, a `store` name, a `Needs` cap-selection triad (the *same* `Needs` data
  hand-written wires declare — ADR 0009), an `extract` fn, a `compare`
  (`NamedCompare`), and a `converge` policy.
- `Correspondence::wire` (`correspondence.rs:136`) emits **one `CapInvariant` per
  store** for the shared catalog, panicking at catalog-build time if a store's
  written-out `id` violates the `inv-<NAME>/<store>` convention.

The tables themselves are declarative entries in
`crates/holon-integration-tests/src/pbt/composed/correspondences.rs`: one
`pub fn <observable>()` per observable (7 today: `non_seed_blocks`,
`block_content`, `block_parent`, `active_editor_text`, `active_editor_caret`,
`org_blocks`, `matview_ghost_rows`). The catalog splices them with
`catalog.extend(correspondences::<observable>().wire())`
(`composed/catalog.rs:124`). **Adding a store to an observable — or a whole new
observable — is one table entry, nothing else.** One generic invariant body,
`StoreInvariant<O>` (`correspondence.rs:156`), sits behind every entry: select on
`needs`; project the reference; extract the store (honouring `converge`);
`Unobservable` on either side → `Skipped` (disclosed); compare → `Ok`/`Fail`.

### 2. The reference is the sole source of the expected value; the SUT never judges itself

Integrity rule (`correspondence.rs:14`): extraction and comparison strategies are
**named `fn`s referenced from the table**, never inline closures buried in SUT
impls. Two consequences:

- The wiring stays *greppable*: a failure reports `inv-block-content/sql` and the
  written-out `id` leads straight to the table entry.
- The SUT **cannot influence how it is judged**. `extract` takes both maps, but
  the ref map is passed **only as seed-filter context** — the expected value comes
  *solely* from `ref_project` (`correspondence.rs:128`, and the `non_seed_blocks`
  doc at `composed/correspondences.rs:30`). The comparator is a pure
  `fn(&T, &T) -> Result<(), String>` (`NamedCompare`, `correspondence.rs:82`) with
  the same fail-loud contract as `capability_pair!`'s `#[compare(with)]`.

### 3. 3-valued extraction — disclosed Skip, never a faked value

`Extraction<T>` (`correspondence.rs:74`) is `Value(T) | Unobservable(reason)`. A
side that *cannot observe the value right now* says so with a disclosed reason and
the store's invariant returns `Skipped` — it never fabricates an empty value to
force a pass. The editor-mirror observables are the exemplar: both "ref has no
active editor" and "SUT returned Err / no keystroke yet" map to
`Unobservable`. This is Holon's fail-loud philosophy encoded in the extraction
type (CLAUDE.md "Fail Loud, Never Fake"): degraded observability is *visible*, not
silent.

### 4. Settle-first convergence policy

`Converge` (`correspondence.rs:89`) is per-store, defaulting to `Converge::None`
(diverged = `Fail`). The harness quiesces projections *before* the invariant-check
pass via the deterministic 3-projection convergence settle
(`WideE2E::settle_after_apply` → `converge_projections` → `converge_signals`,
capped at `wide_e2e::SETTLE` = 150 ms), so `Converge::None` is soundly backed: a
store that still diverges after a real-convergence settle is a genuine bug, not a
race (user decision 2026-07-03, `correspondence.rs:18`). `Converge::Retry` is a
*disclosed exception* for a store the settle provably cannot cover; each use
states its reason in the table. A lag tolerance firing is treated first as a
settle **gap to fix**, not something to tolerate.

### 5. Coexistence, not a flag day; and a hard scope boundary

Registry output coexists with hand-written `wire()`s and macro-derived pair
invariants in the same catalog — no big-bang migration (`correspondence.rs:12`).
The registry has a **documented refusal to grow** beyond value correspondences
over the storage pipeline + editor mirror (`correspondence.rs:43`): it is
explicitly *not* for the viewmodel/renderer/geometry cluster, self-checks
(`no_parent_cycles`, `no_errors`), budget/metrics, or fixed-point checks. A
2026-07-04 design review further ruled that speculative extension points
(`Converge::UpstreamOf`, `StorePairCorrespondence`, `NamedPredicate`) should
**not** be built ahead of a real consumer — each had ≤2 — and that settle-first
likely obsoletes the staleness classifiers entirely (`staleness.rs` to be deleted,
not promoted).

### 6. Why a registry over more macro

`capability_pair!` is kept for exactly what it is good at (the exhausted 1:1
two-value set) and the registry takes everything else, because the two failures of
"just add more macro" are structural, not effort:

- A macro emits **traits and a fixed comparison shape**; it cannot express *one
  hub reference projection fanned out to N heterogeneous stores*, each with its own
  cap-selection, extraction, and convergence. That is a *data* relationship (a
  table), not a *type* relationship (a trait pair).
- Multi-cap / per-id-parameterized / 3-valued bodies don't fit the two-value
  `#[compare]` mold at all. Forcing them through a macro would mean a macro DSL
  approaching a general programming language. A declarative table of named `fn`s is
  simpler, greppable, and reviewable.

## Consequences

### Payoff

- **One entry per store/observable.** Adding coverage is a table row; the generic
  `StoreInvariant<O>` body and `wire()` do the rest. Phase 1 net **−203 LOC**
  while adding observables; the lib test count *dropped* (161→157) as per-store
  selection-triad tests consolidated into shared registry catch tests.
- **The SUT cannot grade its own homework.** Expected value comes only from the
  reference projection; comparators are pure named `fn`s in the table.
- **Fail-loud is typed.** `Extraction::Unobservable(reason)` makes a disclosed Skip
  the only way to "not observe" — no faked empties. `Converge::None` default means
  divergence is a `Fail` by default.
- **Greppable diagnostics.** Written-out `inv-<NAME>/<store>` ids (convention
  asserted in `wire()`) lead from a failure report straight to the table entry.
- **No flag day.** Registry, macro-pairs, and hand-written invariants share one
  catalog; migration is incremental.

### Cost

- **Ids are written out, not derived.** `InvariantId` needs `&'static str` and the
  registry refuses to leak, so each store repeats its full id — the `wire()`-time
  convention assert is the guard against drift.
- **A second single-sourcing mechanism.** `capability_pair!` *and* the registry
  both exist. The boundary is documented (macro = clean 1:1 two-value; registry =
  everything else) but a contributor must learn which tool a new check needs. The
  macro module doc and this ADR are the mitigation.
- **The generic body constrains expressiveness.** Anything the registry
  deliberately excludes (structural predicates over widget trees, self-checks,
  arg-bearing projections, `&mut` drains — `correspondence.rs:43`) stays a
  hand-written invariant. The registry is intentionally narrow, so it does not
  absorb the whole catalog.
- **Convergence soundness rides on the settle.** `Converge::None` is only correct
  because the 3-projection settle genuinely waits for convergence. If the settle
  regresses, strict stores turn flaky; the discipline is "a lag Skip is a settle
  gap to fix," which requires vigilance rather than being structurally enforced.

## Known weaknesses / open questions

- The infra lives in `holon-integration-tests`, not `holon-pbt-core`, because
  `pbt-core` has no tokio and `retry_until_ok` is integration-side
  (correspondence-registry-phase0 note). If a second crate ever needs the registry
  it must move — deferred until that need is real.
- Whether settle-first fully obsoletes the staleness classifiers
  (`no_orphan`/`focus_roots`/`matview_consistent`) — deleting `staleness.rs`
  rather than promoting it to `Converge::UpstreamOf` — was the open Phase 4
  question the 2026-07-04 review leaned toward "delete," but the sweep is not
  recorded as complete here.
- Over-narrow selection is possible: a `StoreProjection` whose `needs` under-states
  the caps its `extract` reads would panic at run time on a wired-but-missing cap
  (fail-loud) rather than being caught statically.

## References

- `crates/holon-integration-tests/src/pbt/correspondence.rs` — `Observable`
  (`:66`), `Extraction` (`:74`), `NamedCompare` (`:82`), `Converge` (`:89`),
  `StoreProjection` (`:111`), `Correspondence` (`:126`), `wire` (`:136`),
  `StoreInvariant` (`:156`).
- `crates/holon-integration-tests/src/pbt/composed/correspondences.rs` — the 7
  observable tables and their named extract/compare `fn`s.
- `crates/holon-integration-tests/src/pbt/composed/catalog.rs:124` — registry
  splice into the shared catalog.
- `crates/holon-macros/src/capability_pair.rs` — the complementary macro track and
  its exhausted-convertible-set scope note (`:55`).
- ADR 0012 (reference-model capability contract), ADR 0009 (component-subset PBTs),
  ADR 0007 (wiring manifest).
