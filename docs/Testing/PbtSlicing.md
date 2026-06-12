# PBT Slicing — Capability-Composed Property Tests

> **PARTIALLY SUPERSEDED (2026-06-14) by [`PbtCompositionDesign.md`](PbtCompositionDesign.md).**
> The capability/selection *concepts* here remain valid, but several concrete
> details have drifted from code (§4/§12/§13 reference `MemBlockStore` /
> `MemEditorMirror` that do not exist; `EditorPureSut` just wraps `EditorPureRef`).
> For the trivial-arbitrary-slice mechanism (capability typemap, omnipotent Ref +
> negative selection, async object-safety, migration plan), follow the new design.
> Trust code over this doc where they disagree.

**Audience**: a Claude session asked to add or refactor a property-based test in this repo. Read this *before* writing a new PBT, and prefer reusing the abstractions described here over adding a monolithic per-test ref/SUT struct.

**One-line model**: there is **one** set of transitions, **one** registry of invariants, and **one** runner. A *slice* is a small product struct that composes capability impls; the slice picks which transitions and invariants it can run purely by which capability traits it satisfies. The wide E2E PBT and the fast narrow slices share all of it.

**Canonical code references**:
- `crates/holon-pbt-core/src/capabilities.rs` — the capability trait surface (`Ref*` / `Sut*` clusters).
- `crates/holon-pbt-core/src/invariant.rs` — `Invariant<R,S>` trait + `InvariantResult` (`Ok`/`Fail`/`Skipped`).
- `crates/holon-pbt-core/src/caching_proxy.rs` — `CachingProxy<'a,S>`: per-tick SUT memoizer.
- `crates/holon-integration-tests/src/pbt/invariants/registry.rs` — metadata registry (`InvariantSpec`, `Subsystem`, `RunMode`) + parity self-tests.
- `crates/holon-integration-tests/src/pbt/invariants/bodies/*.rs` — the executable `Invariant<R,S>` bodies (one per id).
- `crates/holon-integration-tests/src/pbt/invariant_runner.rs` — `run_invariant_registry`: the **sole** invariant path for every suite.
- `crates/holon-integration-tests/src/pbt/slice.rs` — the `declare_pbt_slice!` macro.
- `crates/holon-integration-tests/src/pbt/transition_budgets.rs` — the NFR (non-functional-requirement) budget spine.

---

## 1. The problem this solves

A naive E2E PBT is a monolith: one `ReferenceState`, one `Sut`, and a transition set that mixes pure-logic ops (`TypeChars`, `MoveCursor`) with full-stack ops (`ClickBlock`, `BulkExternalAdd`). Every new PBT then either duplicates that scaffolding for its narrower scope, or joins the monolith and pays seconds-per-case.

We want to **take a slice through Holon's components** and get a fast PBT for exactly that slice — pure in-memory editor (µs/case), Turso+Loro+Org without a UI (matview consistency), in-memory blocks + real GPUI (layout), or any future combination — while sharing **transitions**, **invariants**, and **generators** across all of them.

## 2. The core idea — capabilities, not monoliths

Replace the monolithic `ReferenceState`/`Sut` with small, composable **capability traits**. Transitions, invariants, and generators declare which capabilities they need via trait bounds. A concrete PBT picks a *slice*: a struct that implements the capabilities that slice supplies by composing impls from a menu. The compiler then determines, for free, which transitions and invariants apply.

Capability traits use native `async fn` (no `async_trait`; `#[allow(async_fn_in_trait)]`). **Caps correspond to abstracted system COMPONENTS, never to individual invariants** — this is the load-bearing design rule. If you find yourself adding a cap named after an invariant, find the real component it reads from instead.

### 2.1 Reference-side capability traits

Split into small read/write pairs; lean toward more rather than fewer (collapsing is cheap, splitting later is expensive). Read side first, `Mut`/`Write` suffix for the write side. Representative:

```rust
trait RefBlockTree     { /* read structure */ }   trait RefBlockTreeMut    { /* create/move/delete */ }
trait RefEditorMirror  { /* read text+cursor */ } trait RefEditorMirrorMut { /* type/delete/move cursor */ }
trait RefFocus         { /* current focus */ }    trait RefFocusMut        { /* focus a block */ }
trait RefBackend       { /* non-seed blocks */ }  trait RefWatches         { /* expected watch rows */ }
trait RefRender        { /* expected view/render */ } trait RefLayout       { /* expected layout/geometry */ }
trait RefLifecycle     { /* app_started */ }      trait RefTaskState / RefPeers / …
```

Write capabilities are what make a transition "destructive" against the ref state; many invariants only need the read side.

### 2.2 SUT-side capability traits (component-shaped)

Each cap maps to one real Holon component. The current homes:

| Component | Cap(s) | Representative methods |
|---|---|---|
| Query engine (Turso matviews/base tables) | `SutSqlProjection` | `block_row`, `all_block_ids`, `block_raw_row`, `focus_roots_rows`, `current_focus` |
| Live CDC mirror | `SutBackend` | `live_block_snapshot() -> Vec<Block>`, `live_focus_root_rows` |
| Loro store | `SutLoro`, `SutLoroLog`, `SutLoroTaskState` | `loro_block_snapshot`, `loro_children_of`, `loro_had_errors` |
| UI / ViewModel | `SutViewModel`, `SutRenderer` | `widget_tree_snapshot` (`WidgetSnapshot` IR), `current_view`, `provider_stability_report`, `frontend_root_vm`, `live_vs_fresh_tree_diff`, `drain_vm_emission_toggles` |
| Geometry (real window) | `SutLayout` | `rendered_elements() -> Vec<RenderedElement>`, `visual_content_fraction` |
| Org files | `SutOrgRead`, `SutOrgRender` | `org_block_snapshot`, `snapshot_org_render_pairs` |
| Watches | `SutWatchRows` | watch matview rows |
| Driver | `SutDriver` | synthesises keystrokes/clicks (non-proxiable — `&mut self`) |
| Errors / telemetry | `SutErrorLog`, `SutSpanMetrics` | error-source-per-component; OTel span/budget snapshot |

The point of the framework is **same cap, multiple impls**: `SutBackend` is a `Vec`-mutating `MemBlockStore` in a pure slice and a Turso-projection reader in an E2E slice; the invariant body bound on `SutBackend` runs unchanged against both.

## 3. Transitions, invariants, generators — generic over capabilities

A transition declares what it needs; a slice missing any cap simply can't include it (won't compile):

```rust
impl<R> TransitionRef<R> for SplitBlock where R: RefBlockTreeMut + RefEditorMirror + RefFocusMut { … }
impl<S> TransitionImpl<R,S> for SplitBlock where S: SutEditorMirrorWrite { … }
```

An invariant body is an `Invariant<R,S>` over its minimum caps. It returns `Ok` / `Fail(msg)` / `Skipped(reason)` and **never panics** (the runner decides what a `Fail` means via `RunMode`). CDC-lag and not-ready conditions return `Skipped`, orthogonal to `RunMode`:

```rust
impl<R, S: SutLoroLog> Invariant<R,S> for InvLoroNoErrors {
    fn mode(&self) -> RunMode { RunMode::Strict }
    async fn check(&self, _ref: &R, sut: &S) -> InvariantResult { … }
}
```

Generators take a reference state by trait bound. The single aggregation path is `holon_pbt_core::weighted_arm<R,F,T>` (call factory → apply weight multiplier → drop zero-weight → `prop_map` into the slice's enum); all slices route through it.

## 4. A slice = an assembly, not an abstraction

**A slice's `Sut` (and `Ref`) type is a plain product struct that holds capability impls and forwards trait methods to whichever field owns them. Nothing more.**

```rust
struct EditorPureSut { blocks: MemBlockStore, editor: MemEditorMirror, focus: MemFocusState }
impl SutBlockTree    for EditorPureSut { /* delegate to self.blocks */ }
impl SutEditorMirror for EditorPureSut { /* delegate to self.editor */ }
/* … pure forwarding … */
```

**Smell**: if you write logic *inside* a slice struct (beyond forwarding), a capability is missing — push it into a new cap trait. **Anti-pattern**: don't invent named "composite" types like `GpuiWithMemoryBacking`; that's just a slice's `Sut`, named after the slice and local to the test file.

## 5. Declaring a slice — `declare_pbt_slice!`

New slices cost ~15–25 LOC via the macro (`pbt/slice.rs`). It emits the `ReferenceStateMachine`, the SUT wrapper + `StateMachineTest`, the `prop_state_machine!` `#[test]`, and a per-binary shared tokio runtime.

```rust
declare_pbt_slice! {
    test_fn: cdc_delivery_pbt,
    machine: CdcDeliveryMachine,
    sut_wrapper: CdcDeliverySut,
    variant_ref: …::VariantRef<…::SqlOnly>,
    inner_sut:   …::E2ESut<…::SqlOnly>,
    transitions: [
        StartApp,
        (WriteOrgFile, "skip index.org (CDC quiescence race)", |t: &WriteOrgFile| t.filename != "index.org"),
        BulkExternalAdd, SplitBlock, JoinBlock, TypeChars, SetupWatch, RemoveWatch,
    ],
    invariants: [ InvLoroNoErrors, InvBlockTagsReferencesExist(PhantomData::<…::ReferenceState>) ],
    cases: 16, max_shrink_iters: 20, steps: 1..10,
}
```

- `transitions:` entries are a bare type, or `(Type, "reason", filter)` for a per-transition `prop_filter`.
- `invariants:` entries are arbitrary expressions, so unit structs and `PhantomData<R>`-carrying ones both work.
- Invariants are skipped pre-startup via `RefLifecycle::app_started`.

The macro assumes an `E2ESut<V>`-shaped SUT (`new(runtime)` + `apply_transition_async`). A pure, runtime-free SUT (e.g. `EditorPureSut`) hand-writes its `StateMachineTest` impl — see `tests/editor_pure_pbt.rs`.

### Representative slice consumers

| Slice | SUT | Renderer | Storage | Targeted bug class |
|---|---|---|---|---|
| `editor_pure_pbt` | `EditorPureSut` | none | in-memory | pure editor state-machine |
| `cdc_delivery_pbt` | `E2ESut<SqlOnly>` | none | Turso+Loro | matview→CDC→watch delivery |
| `general_e2e_pbt` | `E2ESut<Full>` | ReactiveEngine (headless) | Turso+Loro | full stack, no window |
| `gpui_ui_pbt` | `E2ESut<Full>` | real GPUI window | Turso+Loro | full stack + geometry |
| `org_roundtrip_pbt` | `Vec<Block>` | none | none | org parser↔renderer fidelity (stateless `proptest!`, shares `assert_normalized_docs_equal`) |

(Other E2E slices exist: `task_state_coherence_pbt`, `org_create_ordering_pbt`, `split_block_content_pbt`, `org_render_fixed_point_pbt`.)

## 6. The invariant registry + runner

This is the heart of the current architecture. **There is no inline invariant code** — every suite runs invariants only through `run_invariant_registry`.

**Registry (metadata only).** Each invariant is one `InvariantSpec { id, description, min_sut: BTreeSet<Subsystem>, mode: RunMode }`. `min_sut` is the *minimum* set of subsystems the body touches. The registry is kept in lockstep with `docs/TESTING_INVARIANT_AUDIT.md` (the invariant↔subsystem matrix).

`Subsystem` = `BlockTree | Loro | TursoProjection | Cdc | ViewModel | Renderer | EditorState | FrontendBounds | Driver`. `RunMode` = `Strict` (a `Fail` terminates) | `Warn` (a `Fail` is logged; a separate `block_raw` truth-check decides flake vs regression).

**Bodies (executable).** Each id has one `Invariant<R,S>` body in `invariants/bodies/`. Per-store block-identity invariants share a single body: `inv-blocks-match-ref/{matview,loro,block_raw,org}` all dispatch the one `compare_blocks` over their respective `SutBackend`/`SutLoroLog`/`SutSqlProjection`/`SutOrgRead` snapshot — which keeps subsystem selection declarative (the `/loro` id auto-drops in a Loro-less slice).

**The runner (`run_invariant_registry`).**
1. **Suite selection is detect-from-caps**: `Subsystem::all()` when `frontend_geometry.is_some()` (the `gpui_ui_pbt` window), else `Subsystem::headless_wide()` (all but `FrontendBounds`). A spec is selected iff its `min_sut ⊆` the suite's subsystems. No suite descriptor is plumbed through harnesses.
2. **Doc-URI remap (keystone)**: the wide ref model uses synthetic doc URIs (`block:ref-doc-N`); the SUT assigns real UUIDs asynchronously. `ReferenceState::with_resolved_doc_uris(map)` remaps the ref into the SUT ID-space once per pass (block ids, parent_ids, `block_documents` keys), so bodies compare same-space with **zero** per-body resolution logic.
3. **Settle**: `settle_before_invariants` polls `block_raw` to convergence before any body reads storage (the Loro→SQL projection is convergent but lags briefly). Its wall time is the `pbt.settle` span → the `settle_ms` NFR metric (§7).
4. **CachingProxy**: the SUT is wrapped in a per-tick memoizer so repeated cap reads in one pass are cheap. Non-proxiable caps (e.g. `SutDriver` with `&mut self`, `SutSpanMetrics` as an integration-tests-local trait) are driven by passing `self`, not `&proxy`, to `run_one`.
5. **Dispatch**: `run_one<S,B>(selected, ref, sut, body)` runs each selected body; the runner owns `Warn`-vs-`Strict` and the `nav_only` gate (structural invariants skipped for pure-navigation transitions). Distinct `[inv-*]` log labels disambiguate output.

## 7. Non-functional requirements — the budget spine

`transition_budgets.rs` budgets per-transition NFRs (SQL counts, wall time, RSS, sync latency) through one data-driven path:

- **`Metric`** (`SqlReads`/`SqlWrites`/`SqlDdl`/`MaxQueryMs`/`WallMs`/`SettleMs`/`RssDeltaBytes`/`RssCumulativeBytes`) + **`MetricSample { metric, actual, limit, severity, message }`**. `build_samples` turns raw `TransitionMetrics`/timing/memory into samples (each carrying its verbatim violation message); `evaluate` is the single comparator (`actual > limit`).
- **Baseline-relative regression**: a committed `nfr_baseline.json` (`transition_key → metric → value`); a sample regresses when `actual > baseline × (1 + tol)` (`HOLON_NFR_REGRESSION_TOL`, default 25%), emitted as a `Warning`. **Ships dormant** — no file means no regression checking (no fabricated numbers). Generate one deliberately: `HOLON_NFR_BASELINE_UPDATE=1 cargo nextest run -p holon-integration-tests --features pbt <test> --no-capture -j1`.

Adding a per-transition timing NFR is a 4-line change: wrap the work in a `pbt.<name>` span, `sum_span` it into `TransitionMetrics`, add a `Metric` variant, push one sample in `build_samples`. The only caller is the `inv-sql-budget` cap path, so it stays additive and behavior-preserving.

## 8. The hard parts — read before you code

- **ID identity across layers.** Pure-tree IDs are local strings; SQL/Loro IDs are URIs/peer_ids. Generators that pick an *existing* block go through `RefBlockTree::blocks()`; generators that *create* one mint the ID SUT-side and the transition trusts the returned ID. Don't bake "new block at position N" into a transition — bake "the block we just told the SUT to create."
- **Quiescence is the harness's job, not the transition's.** Per-call CDC drain (transition executor) + per-tick VM-emission drain (`CachingProxy`) + the runner's `settle_before_invariants` cover it. Transition bodies don't await consistency themselves.
- **Async-write determinism.** Higher slices return a created block's ID only *after* quiescence; chained transitions rely on the harness ordering, not on sleeps.
- **The frontend backing seam.** A slice mixing in-memory blocks with a real GPUI window needs the frontend to accept a non-Turso `BuilderServices`. The framework *exposes* this seam; it doesn't *grant* it — budget that refactor when proposing such a slice (see §11, "Target state").
- **Generics ergonomics.** Where-clauses on bodies top out at 3–4 caps. If one exceeds ~5, introduce a bundled supertrait with a blanket impl rather than repeating the list.

## 9. Naming conventions

- Test files: `<slice>_pbt.rs` in the crate's `tests/`. No phase numbers in names — they go stale.
- Slice types: `<SliceName>Ref` / `<SliceName>Sut` (assemblies), local to the test file.
- Cap impls: `<Backing><Capability>`, e.g. `MemBlockStore`.
- Cap traits: `Ref<Thing>` / `Sut<Thing>` for read; `Mut`/`Write` suffix for write.
- Invariant ids: `inv-<area>-<predicate>` (`/`-suffixed per-store where one body serves several). **Stable** — log greps and the registry depend on them.

## 10. Adding a new slice

1. Skim `tests/cdc_delivery_pbt.rs` — the smallest `declare_pbt_slice!` consumer; crib from it.
2. Pick the axes (storage / renderer / driver). Reuse an existing SUT variant where possible: `E2ESut<Full>`, `E2ESut<SqlOnly>`, `EditorPureSut`.
3. Pick invariants from `pbt/invariants/bodies/` whose trait bounds your SUT satisfies — trait bounds gate compile-time membership.
4. Add `tests/<slice>_pbt.rs` using `declare_pbt_slice!` (§5) or `component_pbt!` (§13).
5. Run `cargo nextest run --features pbt --lib invariants` — the registry parity self-tests must still pass.

A native-mode slice (`proptest_config:`) gets its `ProptestConfig` from the shared `pbt::standard_pbt_config(slice_name)` — it activates the atomic editor, installs the rejection-histogram panic hook, and pins `tests/<slice_name>.proptest-regressions` (pass the same `slice_name` from sibling slices that share one state machine to share the regressions file). The cases-based macro arm builds its config from `cases:` / `max_shrink_iters:` directly, so it needs none of this.

No registration step: a slice is discovered from its `test_fn:` in `tests/`. `just pbt-list` enumerates every slice with the `Wiring`/`ComponentSet` it composes; `just pbt-slice <name> [cases]` runs one by exact name (the file stem may differ — one file can declare several slices); `just pbt-slices` runs them all. (`just pbt <general|petri|orgmode|loro>` remains for the cross-crate PBTs that aren't `holon-integration-tests` slices.)

**One file per slice (don't merge them all).** Each `tests/*.rs` is a separate integration-test *binary* — independent incremental compile/link, `cargo test --test <stem>` and `just pbt-slice` granularity, and a file-scoped `.proptest-regressions` / `fixtures_dir`. Two slices that share a state machine + regressions file may live in one file (e.g. `general_e2e_pbt` + its `_sql_only` peer), but collapsing all slices into one file would make one giant binary and lose that isolation for no gain.

If a body you want is gated on a cap your SUT can't supply, that's the signal to either supply the cap (component-shaped!) or leave that invariant out — never copy its logic into the slice.

## 11. Guardrails

**Archlint** (`archlint/smells/pbt_transitions.toml`):
- `pbt-transition-helper-concrete-ref` — forbids new transition helpers from naming `ReferenceState` in their signature.
- `pbt-slice-invariant-foreign-module` — forbids slice test files from importing `Inv*` structs outside `…::pbt::invariants::bodies::` (no slice-local rubber-stamp invariants).

**Registry self-tests** (`invariants/registry.rs`):
`registry_size_matches_audit`, `gpui_wide_pbt_selects_all`, `headless_wide_pbt_drops_frontend_bounds_invariants`, `under_scoped_spec_rejects_multi_subsystem`, `warn_mode_invariants_preserved`, `body_ids_match_registry_ids`, `storage_slice_invariants_are_subset_of_wide_registry`, `every_registry_id_has_a_body_file`, `every_invariant_has_a_non_empty_min_sut`.

## 12. Target state (intentionally not built yet)

The current system above is complete for the headless and GPUI suites. The remaining gaps are deliberate, not debt:

- **In-memory blocks + real GPUI slice** (a "layout-only" slice without Turso). Needs a faithful non-Turso `BuilderServices` (the frontend backing seam, §8). Deferred on a LOC-budget basis (3 render-path matviews to reproduce in-memory); revive if a cross-frontend (Flutter/web) consumer materialises.
- **`every_registry_id_has_a_body_file` is weaker than ideal** — it checks a body *file* exists, not that it impls a runnable `Invariant`. In practice every id now has a real body, but a stronger compile-time guard ("every registered id has a runnable impl") would prevent regression.
- **Layout/a11y NFR bodies** over the existing `SutLayout::rendered_elements()` cap (`inv-no-zero-size-interactive`, `inv-text-not-clipped`, `inv-no-sibling-overlap`) are a natural next addition — gpui-only, pure functions over the geometry snapshot.
- **Bespoke PBTs not yet folded in**: `sync_controller_mutation_pbt` (could become a SUT over `SutOrgFileWrite + SutSqlProjection`) and the backend-vs-reference family (`loro_backend_pbt`, `turso_pbt_tests`, Todoist) — evaluate value before migrating. Genuinely out of scope: `petri_e2e_pbt`, `holon-engine` PBT, `inline_marks_proptest`, `identity_operations`, `turso_ivm_bug_proptest`.
- **Narrow state-machine PBTs — evaluated, kept as-is (do NOT fold into `declare_pbt_slice!`).** `editor_pure_pbt` and `loro_sync_controller_pbt` use their own minimal `ReferenceStateMachine` (`EditorPureRef`/`PureTransition`; `LoroSyncReference` over `GroupState`/`GroupTransition`) with `prop_state_machine!` directly, not the macro. This is correct, not debt:
  - `declare_pbt_slice!`/`component_pbt!` is hardwired to `ReferenceState`/`E2ETransition`/`E2ESut`. `editor_pure` would gain a real storage backend it doesn't want (it's a storage-free, ~5200× faster fuzz of the editor `_apply_to_ref` cap fns — the speed *is* the value), and `loro_sync` would need its CRDT group model rewritten into the block model (not even semantically possible).
  - They already adopt the architecture where it helps: `editor_pure` reuses the shared transition structs, `_cap` fns, and `TransitionFactory`/`weighted_arm` generation; both check areas the wide PBT *also* covers (structural integrity ≈ `inv-no-ln-blocks`; multi-peer sync via `AddPeer`/`PeerEdit`/`SyncWithPeer` in the convergence harness `subsystem_convergence_pbt`) but faster and in isolation, with extra checks the wide PBT lacks (cursor-within-text-len; CRDT convergence S1–S3/C1–C3).
  - They are a distinct *tier* (narrow, fast, sub-component) in the speed pyramid, not non-adopters of the slice schema. Folding would trade their reason for existing (speed + isolation) for nothing.
  - *Minor follow-up (not done):* `editor_pure`'s two invariants are inline asserts; `inv-tree-structural-integrity` has a registry sibling (`inv-no-ln-blocks`) but `inv-tree-cursor-within-text-len` appears to have none — so its "also fires in the wide PBT" header claim is partly stale. Either add a cursor-bounds body to the registry or correct the comment.

> Note: a clean "wide PBT green via the registry" run is currently gated on **pre-existing, unrelated** product bugs (JoinBlock dispatch, org `assert_blocks_equivalent`, the watch root-layout CDC bug), not on the test architecture. The architecture's own proof is *runner ≡ former monolith* — no new failures.

## 13. `ComponentSet` + component bisection (ADR 0009)

A slice's `wiring:` (ADR 0007) names which storage/sync/actor adapters it assembles. ADR 0009 lifts that into a first-class, *bisectable* value and adds a shared sequential engine both runners share. See `docs/adr/0009-component-subset-pbts-and-bisection.md` for the full rationale; this section is the operational summary.

**`ComponentSet` = `Wiring` + observable `Projection`s** (`crates/holon-pbt-core/src/component_set.rs`). `Projection::{ViewModel, EditorState}` are the rendered/edited surfaces the invariants observe. Blessed presets: `full_gpui` (UI window, all 9 subsystems), `full_headless` (CI default, all but `FrontendBounds`), `loro_vm_fast` (fast inner loop). `Subsystem` selection is **derived** from a set — `invariants/registry.rs::subsystems(&ComponentSet)` is total — so the invariant set is a projection of the components, not an independently-tuned knob. `validate()` enforces `UI ⟹ ≥1 storage + ViewModel`; `needs_real_window()` (≡ `has_actor(UI)`) picks the runner.

**The lattice.** `valid_children()` drops one component (validity-pruned: it never offers dropping `ViewModel` under `UI`); `valid_parents_within(ceiling)` adds one back. This is the search space for bisection.

**One-line slices — `component_pbt!`** (`slice.rs`). Sugar over `declare_pbt_slice!` that names a slice by *what it composes*: `component_pbt! { test_fn: x, set: ComponentSet::loro_vm_fast(), … }` lowers the set to its `.wiring` and delegates (native and explicit-slice forms). `loro_backend_pbt` is written this way. The projection axis isn't threaded into a standalone slice's selection (the runner always observes both projections); it only changes which lattice node a *bisection* oracle builds — so `set → set.wiring` loses nothing a single slice could express.

**One engine, a `Stepper` seam** (`crates/holon-integration-tests/src/pbt/stepper.rs`). `proptest-state-machine`'s `test_sequential` is a plain synchronous loop with **no thread affinity** (the main-thread constraint is GPUI's window alone). `run_sequence(stepper, ref0, transitions, seen, mode)` factors that loop over a `Stepper` (init / apply / check_invariants). Implementors:
- `SmtStepper<T>` — the headless proptest-macro path; both `declare_pbt_slice!` macros override `StateMachineTest::test_sequential` to route through it (generation/shrinking/`.proptest-regressions` are unchanged, above the loop).
- `GpuiReplayStepper<'a>` — replays a fixed `Vec<E2ETransition>` through an already-launched window (borrows the live SUT + driver). The live GPUI *generator* stays hand-rolled by design (mid-sequence window launch + seed-reproducible incremental generation + per-step gestures/screenshots).
- `BisectionStepper` — builds an `E2ESut` **per node** via `new_with_backend(storage_selector_for_wiring(node.wiring))`. (`SmtStepper<E2ESut>` can't serve a node: `E2ESut::init_test` hard-wires Turso.)
- `NullStepper` — no SUT; records the applied transitions, for the §3a spike.

**Cross-set replay portability (the load-bearing invariant, ADR §4).** `ReplayMode::SkipGated` makes a captured sequence portable *down the lattice*: a transition the node's wiring gates out (`E2ETransition::required_wiring().satisfied_by(node.wiring)` is false) becomes a deterministic `StepOutcome::SkippedByGating` no-op that **never reaches `apply`**, instead of `Strict` mode's hard panic. Because applicability is a pure function of `(transition, fixed wiring)` and reference `apply` is pure, the `SkipGated` applied sequence equals exactly the node's applicable subsequence — proven by `tests/bisection_pbt.rs` (`skip_gated_replay_is_portable_for_committed_capture`, `editor_transition_skips_purely_under_storeless_node`), always-on and SUT-free. **Gotcha:** assert over the *applied-transition sequence* (serde-canonical), never a whole-`ReferenceState` `Debug` — the latter embeds the interpreter's hash-ordered builder list and is unstable across instances; `E2ETransition` is deliberately not `PartialEq`.

**Captures, not seeds.** Bisection replays the JSON capture (`tests/.captures/*.captured.json`, a serde-tagged `Vec<E2ETransition>` written by the slice wrapper's `Drop`-on-panic), **never** a `.proptest-regressions` RNG seed — a seed regenerates a *different* sequence against a changed alphabet. The capture omits wiring on purpose: the bisector supplies it per node. **The GPUI runner writes captures too:** `run_pbt_with_driver_sync_callback` arms the same thread-local capture and writes `tests/.captures/<name>.captured.json` (default `gpui_ui_pbt`) on a panicking unwind — so a UI-observed failure feeds the fast headless lattice. (`HOLON_PBT_FORCE_FAIL_AT_STEP=N` forces a panic after step N — a deterministic on-demand capture generator for testing the pipeline.)

**Bisecting a failure** (`crates/holon-integration-tests/src/pbt/bisect_driver.rs`). `holon_pbt_core::bisect(ceiling, floor, reproduces)` walks the lattice and returns a `Localization`: `DownwardMinimal` (bug enters when a component is *added* — the classic case), `UpwardMinimal` (an absent-component / missing-handler bug, present only while a component is *removed*), or `NotReproduced`. The node oracle `reproduces_under(set, caps)` replays the capture through `run_sequence(.., SkipGated)` inside a `catch_unwind`. **Honest framing:** "reproduces" means the reference model and the node's enabled projections *disagree* — it localizes *where* the divergence enters, not *who is wrong* (in this codebase it has often been a reference-model fidelity gap, not a prod bug).

**CI triage — `scripts/pbt-bisect.sh`.** On a red wide PBT, the slice has already written its capture; one command localizes it (each node builds a real SUT, so it is manual/env-gated, not inline on every run):
```
scripts/pbt-bisect.sh <slice>            # full bisection → Localization
scripts/pbt-bisect.sh <slice> --probe    # cheap: does the ceiling even reproduce?
```
Under the hood it sets `HOLON_BISECT_SLICE=<slice>` (resolve the capture from the slice name) on the `bisect_capture_from_env` test; `HOLON_BISECT_CAPTURE=<path>` takes an explicit path instead, `HOLON_BISECT_CEILING` ∈ {`full_gpui`, `full_headless`, `loro_vm_fast`} sets the ceiling (default `full_headless`, a safe universal ceiling — unset transitions simply never gate), and `HOLON_BISECT_PROBE=1` does the single-node reproduce check. With none set, the test no-ops (CI-safe).

**Editor path is bisectable across storage (ADR asymmetry #1).** `TypeChars`/`PressKey` gate on `AnyStorageOf({Loro, Turso})` (not `HasStorage(Loro)`), so "edit content" is structurally available under Turso-only — exercising the on-blur `set_field` path as a transition. Headless Turso-only slices are unaffected: the transitions' `preconditions` still require `enable_loro() || real_editor_enabled()`, gating them dynamically without a real editor.

**Reproduction is a signature match, not "any panic".** A capture generated under one wiring does not always replay faithfully under another — a transition's SUT path can be storage-coupled (e.g. `SplitBlock`'s Turso-only `probe_block_sql_state` diagnostic crashing `test_ctx()` "App not started" on a no-Turso node) or settle-timing-sensitive. Counting *any* replay panic as a reproduction makes such infidelity aborts look like the bug, and the walk localizes spuriously (an early `general_e2e_pbt` run mis-localized to `{Loro}` this way). So `reproduces_under` counts a panic as a reproduction **iff its message contains the reproduction signature** — by default the cross-layer `trouble begins at:` marker (present iff an invariant actually diverged), overridable with `HOLON_BISECT_SIGNATURE` to pin an exact failure. Everything else is logged as an *inconclusive node* and not counted, so the search never descends into a node it cannot faithfully replay. (`HOLON_BISECT_VERBOSE=1` prints the panic; `HOLON_BISECT_REPEAT=N` measures Heisenbug flakiness.)

**Still open** (ADR §"Migration"): asymmetry #2 — assert the GPUI window's `ReactiveEngine` is the *same instance* the ViewModel invariants observe.
