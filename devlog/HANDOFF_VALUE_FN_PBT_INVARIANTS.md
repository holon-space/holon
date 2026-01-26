# Handoff — PBT invariants for value-fn providers

**Status**: Step 10 of `~/.claude/plans/peaceful-strolling-muffin.md`
deferred. Steps 1–9 + Task #12 landed on this worktree
(`peaceful-strolling-muffin`). Unit tests for the per-provider
behaviours are in place; what is missing is the *integration-level*
guarantee that providers behave correctly inside a running
`ReactiveEngine` and that re-renders don't silently drop them.

This document is what the next session needs to pick up.

## Background — what already passes

Already covered (do not re-add):

- **F1 regression** — `crates/holon-api/src/render_eval.rs::tests::
  f1_unknown_fn_returns_null_not_first_arg`. Unknown
  `FunctionCall { name }` returns `Value::Null`, not its first arg.
- **Cache GC** — `crates/holon-frontend/src/provider_cache.rs::tests::
  cache_constructs_fresh_after_all_drops`. Drop both consumer Arcs;
  next `get_or_create` constructs a fresh provider (Weak failed to
  upgrade).
- **Cache identity stability** — `cache_reuses_arc_for_same_key` in
  the same file. Same `(name, args)` → same Arc while alive.
- **`focus_chain` snapshot** — `crates/holon-frontend/src/value_fns/
  focus_chain.rs::tests` (3 cases): empty when nothing focused, single
  row at level 0 when focused, snapshot reacts to
  `focused_block.set(Some(...))`.
- **`concat` core fn** — `core_concat_still_works` in `render_eval.rs`.

What's left is the integration story.

## What needs to be added to `general_e2e_pbt.rs`

There is no separate `general_e2e_pbt.rs` to edit — the integration
test entry point is `crates/holon-integration-tests/tests/
general_e2e_pbt.rs:1` (111 lines), which only wires
`prop_state_machine!` macros. The actual state machine, transitions,
reference state, and invariant body live in
`crates/holon-integration-tests/src/pbt/`. All work below happens in
`src/pbt/`, NOT in the entry-point file. Do not add a new PBT (per
`CLAUDE.md`).

The verification section of the v3 plan listed six invariants. Three
are already covered (above). The remaining three are integration-level
and need to land here:

### Invariant V1 — provider arg variance

> "for every render that uses `chain_ops(col("level"))` or similar
> per-row arg, assert the produced row-sets differ across rows in the
> predicted way."

**Why it matters**: today the only thing exercising `chain_ops` /
`ops_of` end-to-end is the manual mobile-bar profile YAML. If the
macro `Collection` extraction silently regresses to ignore
`get_rows("collection")`, every focus-chain row would receive the
same inherited `ctx.data_source` and produce identical inner ops —
visible only when running the live app. The PBT should catch that.

**Where to wire**:
- `crates/holon-integration-tests/src/pbt/sut.rs:2067`
  (`check_invariants_async`) — add a new `inv11` block alongside the
  existing `inv10*` (entity ID set, decompiled rows, etc.) inside the
  `if !nav_only` arm at line ~3167.
- The display tree is already available as `display_tree` (line
  2817). Walk it to find any node produced by `ops_of` /
  `chain_ops`.

**What to assert**:
- Find every `Streaming` collection node in `display_tree` whose
  `RenderExpr` matches `FunctionCall { name: "ops_of"|"chain_ops",
  args: [..ColumnRef..] }` — i.e. arg depends on the row context.
- For each pair of *outer* rows (focus-chain entries) feeding that
  inner collection, snapshot the inner row IDs. Assert: when outer
  rows differ in the column the inner reads (`uri` for `ops_of`,
  `level` for `chain_ops`), the inner row sets differ in the way the
  reference model predicts.
- The reference for "ops registered for URI X" comes from
  `services.resolve_profile(&{id: X}).operations`. The reference for
  `focus_chain` is just "0 or 1 row depending on focus state" — see
  `ReferenceState.focused_block` (currently absent, see V3 below).

**Generator changes**:
- Add a fixture render-source block that uses `chain_ops(col("level"))`
  to `crates/holon-integration-tests/src/pbt/generators.rs`. Track its
  presence in `ReferenceState.layout_blocks.render_source_ids`.
- Drive at least one `NavigateFocus` transition (`transitions.rs:93`)
  per case so focus_chain has a non-empty chain.

### Invariant V2 — provider identity stability

> "the same `(name, args)` pair evaluated twice in one render pass
> returns Arcs with equal `cache_identity()`."

**Why it matters**: regression on `ProviderCache.get_or_create`
silently doubles every value-fn allocation per render and unsubscribes
half of the focused block's signal listeners. Cache hit ratio is
invisible from outside.

**Where to wire**:
- `inv12` in `check_invariants_async`, same site as V1.
- Today the cache is owned by `ReactiveEngine.provider_cache`
  (`crates/holon-frontend/src/reactive.rs:846`) but is **not yet
  wired into the `ValueFn::invoke` path** — `ops_of` and `focus_chain`
  construct providers fresh on every call. **This invariant cannot
  pass until that wiring lands** (see "Prerequisite work" below).

**What to assert**:
- After interpretation, walk `display_tree` and collect every
  `Streaming.data_source.cache_identity()` for known-cacheable
  invocations (`focus_chain()`, `ops_of(literal_uri)` — anything
  with row-context-independent args). Group by `(name,
  args_fingerprint)`. Assert each group has exactly one distinct
  cache_identity.

### Invariant V3 — no flicker on re-render

> "re-interpreting the outer tree doesn't change the `Arc` identity
> of the inner provider for unchanged outer rows."

**Why it matters**: the layout proptest's `stable_cache_key` already
catches `ReactiveQueryResults`-backed views. Synthetic providers
(focus_chain, ops_of) need the same guarantee, otherwise mobile-bar
items will visibly flicker on every CDC tick.

**Where to wire**:
- `inv13` in `check_invariants_async`. Snapshot every value-fn
  provider's `cache_identity()` before a non-`nav_only` mutation;
  re-interpret; assert identity unchanged for any provider whose
  args weren't affected by the mutation.

**Reference-model changes**:
- Currently `ReferenceState` does not track focus
  (`reference_state.rs:543` only has `focused_block_content` which
  reads through navigation history, not `UiState.focused_block`).
- Add `focused_block: HashMap<Region, Option<EntityUri>>` to
  `ReferenceState` and update on `NavigateFocus`. Mirror what
  `UiState::set_focus` does.
- Without this, V1's "predicted in the predicted way" cannot be
  written.

## Prerequisite work (must land before V2/V3)

The plan said "Step 6: ProviderCache shipped. ReactiveEngine owns
one." That is true at the type level — `ReactiveEngine.provider_cache`
exists and has its own unit tests — but **no `ValueFn::invoke` site
calls `cache.get_or_create()`**. `ops_of`, `focus_chain`,
`chain_ops` all construct fresh `Arc::new(SyntheticRows::from_rows(...))`
or `Arc::new(FocusChainProvider::new(...))` per invocation. V2 and
V3 cannot pass until this is fixed.

Concretely:

1. Expose the cache through `BuilderServices`. Suggested signature
   (additive, default returns `None` for headless):
   ```rust
   fn provider_cache(&self) -> Option<Arc<crate::provider_cache::ProviderCache>> {
       None
   }
   ```
   `ReactiveEngine` returns `Some(self.provider_cache.clone())`.
2. In each `ValueFn::invoke`, route construction through
   `services.provider_cache()`:
   ```rust
   let provider = match services.provider_cache() {
       Some(cache) => cache.get_or_create("focus_chain", args, || {
           Arc::new(FocusChainProvider::new(focused))
       }),
       None => Arc::new(FocusChainProvider::new(focused)),
   };
   InterpValue::Rows(provider)
   ```
3. Verify the existing `ProviderCache::tests::cache_reuses_arc_for_same_key`
   still passes; nothing else should change.

Estimated effort: ~20 lines per value fn, all three identical
shape.

## Triggering the mobile bar in PBT

The new mobile action bar lives in
`assets/default/types/block_profile.yaml:18` under the `if_space(600,
...)` narrow branch. The PBT today never sets a viewport, so the
narrow branch never fires and the bar is never instantiated. Two
options:

- **Option A** (preferred — minimal change): add an `inv11_mobile`
  variant that explicitly sets a viewport via
  `BuilderServices::ui_state().set_viewport(ViewportInfo { width_px:
  500.0, height_px: 800.0, scale_factor: 1.0 })` before snapshotting
  `display_tree`. Run the full inv11/12/13 block twice — once with
  the wide (default) viewport, once with the narrow one.
- **Option B**: add a `SetViewport { width_px, height_px }`
  transition (`transitions.rs`) so generators randomise viewport
  size. Larger blast radius — viewport is global state, every other
  invariant currently assumes wide-screen layout.

Start with Option A. Reach for B only if reviewers ask for randomised
viewport coverage.

## Filenames touched

- Modify: `crates/holon-integration-tests/src/pbt/sut.rs` —
  `check_invariants_async`, add `inv11`/`inv12`/`inv13` blocks (~150
  LOC total).
- Modify: `crates/holon-integration-tests/src/pbt/reference_state.rs`
  — add `focused_block: HashMap<Region, Option<EntityUri>>`, update
  on `NavigateFocus`. Mirror in `apply_transition`.
- Modify: `crates/holon-integration-tests/src/pbt/generators.rs` —
  fixture render-source using `chain_ops(col("level"))`.
- Modify: `crates/holon-frontend/src/reactive.rs` — add
  `BuilderServices::provider_cache()` trait method.
- Modify: `crates/holon-frontend/src/value_fns/{focus_chain,ops_of,
  chain_ops}.rs` — route construction through the cache.

## Don'ts (from CLAUDE.md)

- Do **not** add a new PBT file — extend `general_e2e_pbt.rs` only via
  the shared state machine.
- Do **not** swallow failures in invariant blocks (`.ok()`, `_ =>
  default`). Use `assert!` / `panic!` — invariants are the contract.
- Do **not** mock the cache or services in the integration test;
  use the real `ReactiveEngine` already constructed at
  `sut.rs:2696`.
- Do **not** introduce a `cfg(test)`-only API surface to make
  invariants observable. If walking `display_tree` for value-fn nodes
  needs a richer hook, add it as a real method on `ViewModel` (used
  by tests *and* MCP introspection).

## Acceptance

- `cargo test -p holon-integration-tests --test general_e2e_pbt`
  passes 8/8 cases for both the `Full` and `SqlOnly` SUTs.
- `inv11` reports a non-zero count of value-fn-driven collection
  nodes in at least the `Full` SUT (proves the mobile-bar branch is
  reached).
- `inv12` reports cache reuse > 0 (proves cache is wired).
- `inv13` reports zero identity changes across 8/8 cases (proves
  no flicker).

## Pointers

- The plan: `~/.claude/plans/peaceful-strolling-muffin.md` —
  Verification section (lines 710–748) is the source of truth for
  what these invariants must guarantee.
- Project memory: `~/.claude/projects/-Users-martin-Workspaces-pkm-
  holon/memory/MEMORY.md` — entries `pbt_org_sync_investigation`,
  `decentralized_entity_ownership`, and the inv10* prose history are
  worth scanning before touching `check_invariants_async`.
- Existing inv10* implementation
  (`sut.rs:2700-3165`) is the closest pattern. Match its shape:
  early-return on transient skips, `eprintln!("[inv11] ...")` for
  diagnostic context, hard `assert!` for the actual contract.
