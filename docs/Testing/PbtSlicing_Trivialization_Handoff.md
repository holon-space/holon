<!-- HANDOFF (2026-06-14). Implement in a fresh session. Companion to docs/Testing/PbtSlicing.md. -->

> **SUPERSEDED (2026-06-14) by [`PbtCompositionDesign.md`](PbtCompositionDesign.md).**
> The Move-A runner extraction below is retained in the new design; the Move-B
> `sut_caps_absent!` / `unimplemented!()` macro is **rejected** there (it relocates
> the union tax to a runtime panic). The new design uses a capability typemap
> (`composition::CapMap`) — composition by component list, no faked caps. Read the
> new doc first; this file is kept only for the diagnosis in "Why it's not trivial today".

# Handoff: make adding a PBT slice trivial (decouple the wide invariant registry from `E2ESut`)

## Goal

Adding an arbitrary PBT slice should be a ~15-25 LOC addition (the promise in `docs/Testing/PbtSlicing.md` §5/§10). Today that holds for **narrow** slices that hand-write their invariants (`tests/editor_pure_pbt.rs`, `crates/holon/tests/loro_backend_pbt.rs`) but NOT for a slice that wants the **wide invariant registry** (the 35 cross-subsystem bodies the big E2E slices share). This handoff is the plan to close that gap — and the same change makes the **MemoryBackend wide-registry slice** (Phase 5 Rank 2) a trivial addition.

## Why it's not trivial today (diagnosis — verified)

All in `crates/holon-integration-tests/src/pbt/invariant_runner.rs`.

The good news — **already generic over the SUT `S`** (no work needed):
- `run_one<S>(selected, ref_, sut: &S, body: &dyn DynInvariant<ReferenceState, S>, …)` (:594).
- `DynInvariant<R, S>` + blanket `impl<R,S,T: Invariant<R,S>> DynInvariant<R,S> for T` (:429-449).
- `CachingProxy<'a, S>` / `cached<S>(&S)` (holon-pbt-core/src/caching_proxy.rs:47/72), which forwards each cap conditionally: `impl<'a, S: SutViewModel> SutViewModel for CachingProxy<'a, S>`, same for `SutBackend`, `SutSqlProjection`, `SutLoroLog`, `SutLayout`, `SutCdc`, … So the proxy auto-provides whatever caps `S` has.
- The invariant **bodies** are already `Invariant<R,S>` generic over their minimum caps.

The two things still bolted to the concrete `E2ESut`:

1. **The runner is an `impl E2ESut` method.** `run_invariant_registry[_gated]` (:276/:289) and its prep live in `impl E2ESut` (:71). Prep that touches E2ESut fields:
   - id-remap: `build_doc_uri_map` (:75, reads `self.doc_uri_map` + `ref_state.files.documents`) → `ref_state.with_resolved_doc_uris(&map)` (:295).
   - settle barrier: `settle_before_invariants` (:164) → `settle_on_snapshot` (:207) via `self.ctx.session().block_query()`.
   - window detection: `self.render.frontend_geometry.is_some()` (:318) → picks suite name + toggles `Actor::UI`.
   - otel metrics: `self.metrics.freeze_at_check_start()` (:305, `#[cfg(feature="otel-testing")]`).
   - nav gate + report: `self.last_transition.variant_name()` (:256, :387).
   Call sites (the E2E `StateMachineTest` check/teardown path): `pbt/stepper.rs`, `pbt/slice.rs` (`sut.inner.run_invariant_registry(ref_state)` / `…_end_of_case`), `pbt/phased.rs`.

2. **The body list is monomorphized to `S = CachingProxy<E2ESut>` / `E2ESut`.** `native_proxy_invariants()` (:466) returns `Vec<Box<dyn DynInvariant<ReferenceState, CachingProxy<'a, E2ESut>>>>` with all 35 bodies hardcoded (type aliases `ProxyInvariant`/`SelfInvariant` at :452/:455). For that `vec![]` to compile, the SUT must satisfy the **union of ~25 cap traits** — which is why `pbt/sut_capabilities.rs` (~2152 lines) implements all of them for `E2ESut`, most returning "absent" sentinels (`None` / `EngineFocus::NoEngine` / empty / `unimplemented!()`) for caps a given wiring lacks. A new SUT must do the same → the ~2000-line tax.

**So the two blockers are:** (1) the runner can't be called for a non-`E2ESut` SUT, and (2) any SUT dispatched through the wide list must implement all ~25 caps.

## Target design

### Move A — extract the runner to a generic free function behind a `RegistryHost` trait

Introduce (in `invariant_runner.rs` or a new `registry_runner.rs`):

```rust
/// Everything the registry runner needs from a SUT that ISN'T a read-cap
/// (the prep the runner does before dispatching bodies). E2ESut implements it;
/// a memory/pure SUT gives near-trivial impls.
pub trait RegistryHost {
    /// Map the reference model into this SUT's id-space. Memory/pure SUTs that
    /// never reassign ids return the ref unchanged (identity).
    async fn resolve_ref(&self, ref_state: &ReferenceState) -> ReferenceState;
    /// Block until the SUT's convergent projection matches `resolved`. Memory/pure
    /// SUTs (synchronous, no CDC lag) are a no-op.
    async fn settle(&self, resolved: &ReferenceState);
    /// True when a real window/geometry exists (selects gpui suite + Actor::UI).
    fn has_window(&self) -> bool { false }
    /// Last transition name (nav-gate + report label).
    fn last_transition_name(&self) -> &'static str;
    /// otel budget freeze; default no-op.
    fn freeze_budget(&self) {}
}

pub async fn run_registry<S>(
    sut: &S,
    host: &dyn RegistryHostFor<S>,  // or fold host into S; see note
    ref_state: &ReferenceState,
    nav_only: bool,
    proxy_bodies: Vec<Box<dyn DynInvariant<ReferenceState, CachingProxy<'_, S>>>>,
    self_bodies:  Vec<Box<dyn DynInvariant<ReferenceState, S>>>,
) { /* body = the current :289-388 logic, with self.* → host.* and the two
       body lists passed in instead of hardcoded */ }
```

Then `impl E2ESut { pub async fn run_invariant_registry(&self, r) { run_registry(self, self, r, self.nav_only(), native_proxy_invariants(), native_self_invariants()).await } }` keeps the existing call sites unchanged.

Note: simplest is `S: RegistryHost` (the SUT *is* its own host) so `run_registry<S: RegistryHost>(sut: &S, …)` takes one value. E2ESut adds an `impl RegistryHost for E2ESut` wrapping its current prep methods. Memory SUT's impl is ~10 lines (identity resolve, no-op settle).

### Move B — kill the cap-completeness tax with an `unimplemented!()` macro

The runtime selector already drops any body whose `min_sut ⊄ subsystems` (`PbtSuiteSpec::select`), so unsupported caps are **never called** for a scoped slice. They only need to *compile*. Provide:

```rust
// generates `impl SutLoroLog for $T { ... unimplemented!("cap not in this slice") }`
// for every cap NOT in `supported`, so the wide body list type-checks for $T.
sut_caps_absent!(MemoryBackendSut, supported = [
    SutBackend, SutBlockTreeWrite, SutEditorMirrorRead, SutEditorMirrorWrite,
    SutFocusWrite, SutQuiesce, SutCdc, SutLifecycle,
]);
```

The macro lives in `holon-integration-tests` (or `holon-pbt-core`), enumerates the full cap set once, and emits panicking impls for the complement of `supported`. This replaces the ~2000-line hand-written absent-sentinel tax for new SUTs. (E2ESut can keep its real impls; it isn't required to adopt the macro.)

### Result — what a new slice costs after A+B

`crates/holon-integration-tests/tests/memory_backend_wide_pbt.rs` (sketch, target ≈ 60-80 LOC):
```rust
struct MemoryBackendSut { backend: MemoryBackend, editor: MemEditorMirror, focus: MemFocus, last: &'static str }
impl SutBackend for MemoryBackendSut { /* read all blocks → Vec<Block> */ }
impl SutBlockTreeWrite for MemoryBackendSut { /* CoreOperations create/move/delete */ }
impl SutEditorMirrorRead/Write, SutFocusWrite, SutQuiesce, SutCdc, SutLifecycle { /* small */ }
sut_caps_absent!(MemoryBackendSut, supported = [ ...the 8 above... ]);
impl RegistryHost for MemoryBackendSut { fn resolve_ref(r)=r.clone(); fn settle(_){} fn last_transition_name(&self){self.last} }
impl StateMachineTest for MemoryBackendSut {           // hand-written, crib editor_pure_pbt.rs
    type Reference = /* wide */ WideMachine;            // reuse ReferenceState + its transitions
    fn apply(...) { /* drive the wide transitions via *_apply_to_sut against MemoryBackend */ }
    fn check_invariants(sut, ref) { runtime.block_on(run_registry(sut, sut, ref, false, mem_proxy_bodies(), vec![])) }
}
// ComponentSet restricted to {BlockTree, Cdc, EditorState} → only those invariants select.
```
`mem_proxy_bodies()` = the subset of `native_proxy_invariants()` whose caps the memory SUT supports — OR keep the full list (the absent ones compile via the macro and are dropped at runtime by selection). Prefer the full list so the slice auto-gains future BlockTree/Cdc invariants.

## Step-by-step plan (incremental; keep the suite GREEN after each step)

1. **Extract, no behavior change.** Move `run_invariant_registry_gated` body into `run_registry<S: RegistryHost>(…, proxy_bodies, self_bodies)`. Add `impl RegistryHost for E2ESut` delegating to the existing prep methods. Make `E2ESut::run_invariant_registry` call `run_registry(self, self, …, native_proxy_invariants(), native_self_invariants())`. Generalize the `ProxyInvariant`/`SelfInvariant` aliases + `native_*_invariants()` to be generic `<S>` (return `Vec<Box<dyn DynInvariant<ReferenceState, CachingProxy<'_, S>>>>`), OR pass them in. **Validate:** `cargo nextest run -p holon-integration-tests --features pbt -E 'test(storage_consistency) or test(cdc_delivery) or test(general_e2e)'` (sql_only first ~50s). Must be green and behavior-identical.
2. **Add `sut_caps_absent!`.** Write the macro + a unit test that a dummy struct supporting only `SutBackend` compiles into the wide body list. No production change.
3. **MemoryBackendSut + caps.** Implement the ~8 real caps over `MemoryBackend` (crib `editor_pure_pbt.rs` `PureEditor` for the mirror; `MemoryBackend` already gives `CoreOperations`/`children_of`/`ChangeNotifications`). Apply the macro. `impl RegistryHost` (identity resolve, no-op settle).
4. **Wire the slice.** Hand-write `StateMachineTest` (template: `editor_pure_pbt.rs`) reusing the wide transitions (`*_apply_to_sut`/`*_apply_to_ref` cap fns) and the wide `ReferenceState`; restrict `ComponentSet` to `{BlockTree, Cdc, EditorState}`. **Validate:** new slice green + sub-second-to-seconds; then reproduce a known structural bug (ghost-row / unseeded-split family) on it to prove fast localization.
5. **Backfill the doc.** Update `docs/Testing/PbtSlicing.md` §4/§12/§13: correct the `MemBlockStore`/`MemEditorMirror` idealization (they don't exist), and document `run_registry`/`RegistryHost`/`sut_caps_absent!` as the trivial-slice path.

## Risks & gotchas (learned this session)

- **`PbtSlicing.md` has drifted from code.** §4 shows `EditorPureSut { blocks: MemBlockStore, … }` and §2.2 lists caps idealistically — `MemBlockStore`/`MemEditorMirror` DON'T exist; `EditorPureSut` just wraps `EditorPureRef`. Trust code over doc.
- **Compile-time vs runtime cap handling are different.** Runtime selection (`min_sut ⊆ subsystems`) drops *running* a body; it does NOT relieve the *compile-time* requirement that `S` implement the cap. That's exactly what `sut_caps_absent!` solves. Don't return "empty" from a real cap to fake absence — empty looks like real data and fails the comparison; `unimplemented!()` (never reached because deselected) is correct.
- **`SutSqlProjection`/matview bodies must stay deselected for memory**, not faked. MemoryBackend has no SQL projection; `{BlockTree,Cdc,EditorState}` excludes `TursoProjection`, so those bodies are dropped — good.
- **id-remap is identity for memory** (no async doc creation), **settle is a no-op** (synchronous writes, no CDC lag). Don't copy E2ESut's 5s doc-materialization wait.
- **Keep E2E green at every step.** The runner is the heart of every E2E PBT. Step 1 must be behavior-identical (pure extraction). Run the sql_only slices (~50s) as the fast gate before the full ~60s ones.
- **otel feature gating:** `freeze_budget` + `InvSqlBudget` are `#[cfg(feature="otel-testing")]`. Keep the cfg in the `RegistryHost` default + self-bodies list.

## Already done this session (context)
- `loro_backend_pbt` (the narrow MemoryBackend↔LoroBackend structural+watcher PBT) was **chronically RED**; root-caused a harness false-positive (duplicate-content watcher-event pairing ambiguity), fixed surgically, and **promoted to `crates/holon/tests/loro_backend_pbt.rs`** (first-class integration test, GREEN ~62s). This is the *narrow-slice* reference; the *wide-registry* slice is what this handoff enables.

## Key files
- `crates/holon-integration-tests/src/pbt/invariant_runner.rs` — the runner to extract (Move A).
- `crates/holon-pbt-core/src/capabilities.rs` (1512 lines) — the full cap-trait set (enumerate for the macro).
- `crates/holon-pbt-core/src/caching_proxy.rs` — `CachingProxy<S>`/`cached<S>` (already generic).
- `crates/holon-integration-tests/src/pbt/sut_capabilities.rs` (~2152) — E2ESut's cap impls (the tax to eliminate for new SUTs).
- `crates/holon-integration-tests/src/pbt/reference_state.rs` (2170) — wide `ReferenceState` + `with_resolved_doc_uris`.
- `crates/holon-integration-tests/tests/editor_pure_pbt.rs` — hand-written `StateMachineTest` template.
- `crates/holon/src/api/memory_backend.rs` — the SUT backing.
- `crates/holon-integration-tests/src/pbt/{slice.rs,stepper.rs,phased.rs}` — `declare_pbt_slice!` + E2E call sites of the runner.
- `docs/Testing/PbtSlicing.md` — design doc to update (Step 5).
