# PBT Slicing — Capability-Composed Property Tests

**Status**: shipped. Stages A + B + Phase 10 cleanup landed 2026-05-18 (21 commits on the `pbt-slicing-doc` branch). Three slice consumers running today. Phase 9 (in-memory + real GPUI) deferred per H7 audit — see [PHASE_9_H7_AUDIT.md](../PHASE_9_H7_AUDIT.md). The design below is the canonical reference; the [§ "What actually shipped" callout](#-what-actually-shipped) summarises divergences.

**Audience**: a Claude session asked to add or refactor a property-based test in this repo. Read this *before* writing a new PBT, and prefer reusing the abstractions described here over adding monolithic per-test ref/SUT structs.

**Sister docs**:
- [`docs/TESTING_PHASE10_HANDOFF.md`](../TESTING_PHASE10_HANDOFF.md) — final delivery state, what's done, what's left
- [`docs/PHASE_9_H7_AUDIT.md`](../PHASE_9_H7_AUDIT.md) — why Phase 9 was deferred
- `docs/TESTING_INVARIANT_AUDIT.md` — invariant ↔ subsystem matrix
- `docs/TESTING_PATTERNS.md` — fold patterns and pitfalls
- `crates/holon-integration-tests/src/pbt/invariants/registry.rs` — runtime registry (28 entries) + parity self-tests
- `crates/holon-pbt-core/src/capabilities.rs` — canonical trait surface (all clusters)
- `crates/holon-pbt-core/src/invariant.rs` — `Invariant<R,S>` trait + `RunMode` + `InvariantResult`
- `crates/holon-pbt-core/src/caching_proxy.rs` — per-tick SUT cache (drain-once VM emissions)

## 🚢 What actually shipped

Five slice consumers run today; framework's structural claim — *same transitions + same invariants execute across different SUT compositions* — is empirically validated.

| Slice | SUT variant | Renderer | Storage | Cases × Steps | Wall | Bug class targeted |
|---|---|---|---|---|---|---|
| `tests/editor_pure_pbt.rs` (Stage A) | `EditorPureSut` | none | in-memory | 1024 × 30 | ~2 s | pure editor state-machine bugs |
| `tests/storage_consistency_pbt.rs` (Phase 8) | `E2ESut<SqlOnly>` | none | real Turso+Loro | 16 × 1-10 | ~124 s | end-to-end storage consistency |
| `tests/cdc_delivery_pbt.rs` (Phase B+) | `E2ESut<SqlOnly>` | none | real Turso+Loro | 16 × 1-10 | ~54 s | matview→CDC→watch delivery (MEMORY's Turso IVM bug class) |
| `tests/general_e2e_pbt.rs` (wide) | `E2ESut<Full>` | ReactiveEngine headless | real Turso+Loro | varies | minutes | full stack |
| `tests/org_roundtrip_pbt.rs` (Phase B+) | `Vec<Block>` | none | none | 512 × 1 | <1 s | org parser↔renderer fidelity (tag drop, drawer-property drop, TODO keyword loss) |

**Divergences from the original design** (preserved here so future readers know what the doc claimed vs. what we built):

- **`CanonicalEvent` never built.** Section 2.3's bridging enum never materialised because no slice needed cross-medium notification assertions. Per the §9 escape hatch, the variant was killed before it ossified — slice-local event types remain the right granularity for now.
- **`SutQuiesce` split into `SutCdc` + `CachingProxy` drain.** The original "uniform `quiesce()` future" became two surfaces: per-call CDC drain on the SUT (called by the transition executor) and per-tick eager drain of VM emissions inside the `CachingProxy` (called by invariant evaluation). The split is documented at [`caching_proxy.rs`](../../crates/holon-pbt-core/src/caching_proxy.rs).
- **`WidgetSnapshot` IR emerged** (not in original doc) as the cross-renderer abstraction for invariants that need a renderer's output. Lives in `holon_pbt_core::capabilities::WidgetSnapshot`. Renderer-required invariants bind on `SutRenderer::widget_tree_snapshot()` returning this IR, so they can run against any UI-bearing slice — not just real GPUI.
- **Capability clusters expanded to 8** beyond the original 4-6: Loro, Turso/CDC, ViewModel/Renderer, Layout, Driver, OrgRender, QueryCompile, Lifecycle. Trait surface canonicalised at [`capabilities.rs`](../../crates/holon-pbt-core/src/capabilities.rs).
- **Invariant migration is partial.** 4 of 7 functional invariants migrated to capability-bound `Invariant<R,S>` calls in the wide PBT runner; 3 retained inline because the wide PBT asserts strictly more than the slim migrated impls (richer state-toggle checks, fresh-interp viewmodel path). Documented in `TESTING_PHASE10_HANDOFF.md`.
- **`E2ETransitionFactory` / `E2ETransitionImpl` NOT retired.** The Stage A/B migration moved transition LOGIC into capability-bound free helpers, but the `declare_e2e_transitions!` macro still generates these traits as the dispatch surface. Audit verdict: not retirable while the macro shape stays.
- **Phase 9 deferred** per H7 audit: matview-required count is 3 (>2 gate), a faithful in-memory `BuilderServices` impl estimates at 1600-2950 LOC (>1500 gate). Framework still ships without it.

## 🛡️ Two archlint guardrails enforce the discipline

Both in `archlint/smells/pbt_transitions.toml`:

- `pbt-transition-helper-concrete-ref` — forbids new `pub fn <name>_(apply_to_ref|weighted_generator|preconditions)` helpers from naming `ReferenceState` in their signature. Forward-looking: doesn't punish the ~46 not-yet-migrated transitions because they don't have free-function helpers matching the pattern.
- `pbt-slice-invariant-foreign-module` — forbids slice test files from importing `Inv*` structs outside `holon_integration_tests::pbt::invariants::bodies::`. Static counterpart to the runtime H11 anti-rubber-stamp test in the registry self-tests.

Plus the registry self-tests in `crates/holon-integration-tests/src/pbt/invariants/registry.rs`:
- `body_ids_match_registry_ids` — id parity between bodies/ dir and `register_default()` registry
- `every_registry_id_has_a_body_file` — file-system parity
- `storage_slice_invariants_are_subset_of_wide_registry` — runtime H11 guard

---

## 1. The problem this exists to solve

Today the wide PBT (`general_e2e_pbt.rs`, `gpui_ui_pbt`) is one big monolith: a single `ReferenceState`, a single `Sut`, and a transition set that mixes pure-logic ops (TypeChars, MoveCursor) with full-stack ops (ClickBlock, BulkExternalAdd). Every new PBT today either (a) duplicates that scaffolding for its narrower scope, or (b) joins the monolith and pays the seconds-per-case cost.

We want to be able to **take a slice of our choice through Holon's components** and get a fast PBT for exactly that slice:

- pure in-memory editor + block tree (microsecond-per-case)
- Turso + Loro + Org without a UI (matview consistency)
- in-memory blocks + full GPUI (layout-only)
- the event bus as a notification surface
- whatever future combination a bug demands

These slices must share **transitions**, **invariants**, and **generators** — otherwise we're back to copy-paste.

## 2. The core idea — capabilities, not monoliths

Replace the monolithic `ReferenceState` and `Sut` with small, composable **capability traits**. Transitions, invariants, and generators declare which capabilities they need via trait bounds. A concrete PBT picks a *slice*: a struct that implements the capabilities that slice supplies, by composing impls from a menu.

The compiler then determines, for free, which transitions and invariants apply to the slice — no runtime filtering needed.

### 2.1 Reference-side capability traits

Split into small read/write pairs. Lean toward more rather than fewer — collapsing a capability is cheap, splitting one is expensive.

```rust
trait RefBlockTree         { /* read structure */ }
trait RefBlockTreeMut      { /* create / move / delete */ }
trait RefEditorMirror      { /* read text+cursor */ }
trait RefEditorMirrorMut   { /* type / delete / move cursor */ }
trait RefFocus             { /* current focus */ }
trait RefFocusMut          { /* focus a block */ }
trait RefEventLog          { /* observed canonical events */ }
trait RefRenderedBounds    { /* synthetic predicted bounds */ }
```

**Why read/write split**: write capabilities are what makes a transition "destructive" against the ref state; many invariants only need the read side. Splitting now avoids re-decomposing once we add a slice where one side is supplied but not the other (e.g. a read-only consistency PBT).

### 2.2 SUT-side capability traits

Symmetric, plus async/quiescence concerns and a notification surface:

```rust
trait SutBlockTree           { /* read */ }
trait SutBlockTreeWrite      { /* may dispatch async ops */ }
trait SutEditorMirror        { /* read live editor state */ }
trait SutEditorMirrorWrite   { /* keystrokes / cursor */ }
trait SutFocus               { /* current focus */ }
trait SutFocusWrite          { /* focus a block */ }
trait SutNotifications       { /* await canonical events */ }
trait SutLayout              { /* widget bounds + kinds */ }
trait SutOrgRender           { /* render to org file */ }
trait SutLoroLog             { /* read Loro sync error log */ }
trait SutSqlProjection       { /* read matviews / base tables */ }
trait SutQuiesce             { /* await consistency */ }
```

Same capability, multiple impls — the point of the framework:

| Capability | Impl A | Impl B | Impl C |
|---|---|---|---|
| `SutBlockTreeWrite` | `MemBlockStore` (mutate Vec) | `TursoBackedSut` (emit `OperationIntent`, await projection) | `GpuiSut` (synth key chord, await rendered row) |
| `SutNotifications` | `TursoEventBusObserver` (queue events) | `PopupObserver` (poll for popup matching predicate) | `WatchObserver` (subscribe to a watch) |
| `SutQuiesce` | `NoQuiesce` (no-op for pure slices) | `CdcDrain` (await CDC settle) | `GpuiFramePump` (drive frames until idle) |

### 2.3 The canonical event vocabulary

For `SutNotifications` to be cross-slice, observers translate from their native representation into a shared enum:

```rust
enum CanonicalEvent {
    BlockCreated  { id: BlockId, parent: Option<BlockId> },
    BlockDeleted  { id: BlockId },
    ContentChanged { id: BlockId, text: String },
    FocusMoved    { from: Option<BlockId>, to: Option<BlockId> },
    /* … grow as bugs demand … */
}
```

This is the **riskiest** part of the design — it bridges UI popups, event-bus messages, and CDC deltas under one schema. Keep it lean. Add variants only when a slice needs to assert on the new event class; don't pre-build a maximal vocabulary.

## 3. Transitions, invariants, generators — generic over capabilities

A transition declares what it needs:

```rust
struct SplitBlock { target: BlockId, at: usize }

impl<R> RefApply<R> for SplitBlock
where R: RefBlockTreeMut + RefEditorMirror + RefFocusMut { ... }

impl<S> SutApply<S> for SplitBlock
where S: SutEditorMirrorWrite + SutQuiesce { ... }
```

A slice missing any of those caps simply can't include this transition in its `TransitionSet` (won't compile). That replaces the runtime `min_sut` filtering in today's registry — though the registry stays as a human-readable catalogue.

Invariants identical pattern:

```rust
impl<S: SutLoroLog> Invariant<S> for InvLoroNoErrors { ... }

impl<R, S> Invariant2<R, S> for InvOrgRenderFixedPoint
where R: RefBlockTree, S: SutOrgRender + SutQuiesce { ... }
```

Generators take a reference state by trait bound and produce a transition:

```rust
fn split_block_gen<R: RefBlockTree + RefEditorMirror + RefFocus>(state: &R)
    -> BoxedStrategy<SplitBlock>
```

## 4. A slice = an assembly, not an abstraction

Critical convention: **a slice's `Sut` (and `Ref`) type is a plain product struct that holds capability impls and forwards trait methods to whichever field owns them.** Nothing more.

```rust
struct EditorPureSut {
    blocks: MemBlockStore,
    editor: MemEditorMirror,
    focus:  MemFocusState,
}
// trait forwarding — mechanical, candidate for a derive macro once we have >2 slices
impl SutBlockTree      for EditorPureSut { /* delegate to self.blocks */ }
impl SutBlockTreeWrite for EditorPureSut { /* delegate to self.blocks */ }
impl SutEditorMirror   for EditorPureSut { /* delegate to self.editor */ }
/* … */
impl SutQuiesce        for EditorPureSut { fn quiesce(&self) -> ... { ready(()) } }
```

**Smell**: if you find yourself writing logic *inside* the slice struct (beyond forwarding), a capability is missing — push it into a new capability trait instead. Slice structs should be boring.

**Anti-pattern**: do not invent named "composite" types like `GpuiWithMemoryBacking`. That's just a slice's `Sut` and should be local to the test file, named after the slice (`MatviewDriftSlice`, `EditorPureSlice`, etc.), and contain only forwarding.

## 5. The slice declaration

Every PBT becomes:

```rust
struct EditorPureSlice;
impl PbtSlice for EditorPureSlice {
    type Ref = EditorPureRef;
    type Sut = EditorPureSut;
    type TransitionSet = (TypeChars, DeleteBackward, MoveCursorLeft, MoveCursorRight,
                          SplitBlock, JoinBlock, Indent, Outdent);
    type InvariantSet  = (InvTreeStructuralIntegrity,
                          InvTreeCursorWithinTextLen,
                          InvTreeCursorTextTrimStable);
    fn name() -> &'static str { "editor-pure" }
}
```

That's the entire spec. The framework runs it.

## 6. The hard parts — read before you code

### 6.1 ID identity across layers
Pure-tree generates string IDs locally; SQL/Loro IDs come from URIs and peer_ids. Convention:
- Generators that need to pick *an existing* block go through `RefBlockTree::blocks()` (layer-agnostic).
- Generators that need to *create* a fresh ID call a trait method `RefBlockTreeMut::fresh_id()` — the impl decides whether that's a string UUID, a Loro tree-node ID, or a URI. The transition body trusts the returned ID.
- Don't bake "this is a new block at position N" into the transition; bake "this is the block we just told the SUT to create."

### 6.2 The quiescence model
Has to be uniform across slices. `SutQuiesce::quiesce()` returns a future:
- Pure slice: `ready(())`
- SQL slice: drains the CDC queue, awaits matview catch-up
- GPUI slice: pumps frames until the bounds registry stops changing

Most existing PBT timing code (`wait_for_widget_kind`, CDC-drain helpers) folds cleanly into this.

### 6.3 ID-from-async-write determinism
At higher slices, `SutBlockTreeWrite::create_block(...)` returns the ID *after* quiescence. Transitions that chain on a just-created block must `quiesce().await` first. Wire this into the harness, not into each transition body.

### 6.4 The frontend backing seam
Some slices (in-memory blocks + real GPUI) require the frontend to accept a non-Turso `BuilderServices` impl. The capability framework *exposes* this seam, it doesn't *grant* it. Budget the frontend refactor when proposing such a slice. Other slices (matview-drift without UI, pure editor) cost almost nothing once the traits exist.

**Status**: Phase 9 H7 audit ran the LOC budget — `BuilderServices` has 3 render-path matviews (`block`, `block_requirement_edges`, `focus_roots`) to faithfully reproduce in-memory, exceeding the ≤2 matview gate; total impl estimated 1600-2950 LOC vs ≤1500 budget. **Deferred** to a separate plan; framework still ships without this slice. See [PHASE_9_H7_AUDIT.md](../PHASE_9_H7_AUDIT.md) for the full audit and viable alternatives.

### 6.5 Generics ergonomics
Long `where` clauses get painful. Mitigations:
- Bundled "umbrella" traits with blanket impls:
  ```rust
  trait EditorOps: SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce {}
  impl<T> EditorOps for T where T: SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce {}
  ```
- Macros for the `impl Transition for X requires caps Y` boilerplate, once we have ≥3 transitions sharing a pattern.

### 6.6 The registry doesn't go away
`pbt/invariants/registry.rs` stays as the human-readable catalogue (id, description, min_sut, run mode). Trait bounds replace runtime filtering, but the registry is still where humans read "what invariants does this slice cover" and where docs link to.

## 7. Roll-out strategy (HISTORICAL — Stages A + B complete)

**Stage A (Phases 1-5) — shipped:**
1. ✅ Capability traits at `holon-pbt-core/src/capabilities.rs` — `RefBlockTree(+Mut)`, `RefEditorMirror(+Mut)`, `RefFocus(+Mut)` + symmetric `Sut*` variants. Blanket impls on `ReferenceState` and `E2ESut<V>`. No wide-PBT behavior change.
2. ✅ Seven (eventually nine) T0 transitions migrated: `TypeChars`, `DeleteBackward`, `MoveCursor*`, `SplitBlock`, `JoinBlock`, `Indent`, `Outdent`. Each exposes a `pub fn <name>_(apply_to_ref|weighted_generator|preconditions)<R: ...>` free helper; the macro-generated trait impl is a thin adapter.
3. ✅ First slice consumer at `tests/editor_pure_pbt.rs` — `EditorPureRef` + `EditorPureSut` implementing exactly the 6 trait pairs.
4. ✅ Two-consumer gate satisfied: wide PBT and editor_pure_pbt consume the same trait set.

**Stage B (Phases 6-10) — shipped:**
- **Phase 6**: all 8 remaining cluster traits scaffolded — Loro, Turso/CDC, ViewModel/Renderer, Layout, Driver, OrgRender, QueryCompile, Lifecycle. 26/32 SUT methods wired; 6 stubs with documented blockers.
- **Phase 7**: `Invariant<R,S>` trait + `CachingProxy<'a, S>` (zero-unsafe eager-drain) + `WidgetSnapshot` IR. 28 `Invariant<R,S>` impls under `pbt/invariants/bodies/` (9 functional, 19 deferred with documented blockers).
- **Phase 8**: second slice consumer at `tests/storage_consistency_pbt.rs` using `E2ESut<SqlOnly>` + 2 storage invariants.
- **Phase 9**: **deferred** per H7 audit (matview-required count 3 > 2 gate; LOC budget 1600-2950 > 1500 gate). See [PHASE_9_H7_AUDIT.md](../PHASE_9_H7_AUDIT.md).
- **Phase 10**: cleanup + hardening — 28 invariants registered with parity self-tests; 2 archlint smell rules forbidding regression; 4 wide-PBT inline invariant assertions migrated to `Invariant<R,S>` calls; 4 dead-code helpers retired from `sut.rs`.

**What remains (not blocking):**
- 19 deferred `Invariant<R,S>` bodies under `pbt/invariants/bodies/` returning `InvariantResult::Skipped(...)` with documented unblockers — promote as the corresponding SUT plumbing lands. (5 still Skipped as of 2026-05-19 — see "Phase A progress" below.)
- 3 wide-PBT inline invariants kept inline (state-toggle, root-matches-render-expr, entity-ids-subset-of-data) because the wide version asserts strictly more than the slim migrated impls.
- Phase 9 deferred to a separate plan if cross-frontend (Flutter, web) consumers materialise.

## 🧹 Cleanup work in progress (2026-05-19)

`sut.rs` was 7322 LOC the morning of 2026-05-19 (incomplete Phase 10 + 46 unmigrated SutHandle transitions). Active refactor splits:

**Phase A — promote `Skipped` bodies to live invariants** (deletes the inline equivalents in `sut.rs::check_invariants_async`):
- ✅ A1: `InvOrgRenderFixedPoint` promoted. Capability `SutOrgRender::render_documents_to_org` widened to `snapshot_org_render_pairs() -> Vec<(path, disk, rendered)>`. sut.rs inline section (33 LOC at the old L4273-4305) replaced with a `assert_invariants!` macro call. storage_consistency_pbt PASS (186 s).
- ✅ A2: `InvViewmodelNoErrorWidgets` promoted. Capability `SutViewModel::headless_error_node_count() -> Option<usize>` added. Impl snapshots `reactive_engine` + `reactive_root_id` (now `pub(super)` after D4), drives `HeadlessBuilderServices::new(backend_engine)` + `interpret_pure` + `count_error_nodes`. Wide-PBT inline kept for diagnostic richness (per §9 retrospective). Body now runs on any `SutViewModel` slice; SqlOnly slices Skip cleanly (no engine).
- ✅ A3 (clarification, not real work): `focus_matches_ref` and `editable_text_has_draggable` were already migrated to live in the morning audit — the "Skipped" word in their doc comments described conditional skip paths inside live bodies, not blockers.
- ⏳ Remaining 2 true Skipped bodies:
  - **`matview_consistent_with_ref`**: needs `SutViewModel::root_layout_data_row_ids()` (and ref-side caps for `block_state.blocks` + `layout_blocks` + `profile_block_ids` + `is_descendant_of_any` + `expected_focus_root_ids`). Plus `RunMode::Warn` + a `Skipped`-path classifier for the soft-check semantics.
  - **`viewmodel_tree_virtual_slots`**: needs `display_tree` wired into `WidgetSnapshot` + virtual-slot entity IDs propagated. Larger architectural lift.

**Phase D — mechanical file split** (no behavior change; pure relocation):
- ✅ D1: `leader_key_for` + `KeybindingsFile` + `leader_key_tests` → `pbt/sut_keybindings.rs` (86 LOC).
- ✅ D2: `parse_block_row` + `mutation_expected_properties` + `row_properties_to_map` + `BLOCK_SQL_COLUMNS` → `pbt/sut_row_parsing.rs` (160 LOC).
- ✅ D3: `impl SutHandle for E2ESut<V>` (the 2400-LOC trait impl block) → `pbt/sut_handle.rs` (2426 LOC). Required bumping visibility on 19 `E2ESut` methods (`pub(super)`) and 2 fields (`loro_sut`, `pre_ref_state`).
- ✅ D4: `apply_transition_async` + `check_invariants_async` + `live_blocks` + `live_focus_roots` + `wait_for_live_data_mirrors` (the 3rd `impl<V> E2ESut<V>` block, ~2960 LOC) → `pbt/sut_check_invariants.rs` (2991 LOC). `assert_invariants!` macro moved to `pbt/sut_macros.rs` (24 LOC). Bumped visibility on 6 more fields (`reactive_engine`, `vm_emissions`, `reactive_root_id`, `live_tree`, `live_blocks_cell`, `live_focus_roots_cell`) and `FocusRoot` struct + fields.
- ✅ D5: Extracted 6 of 14 sections from `check_invariants_async` into named `pub(super) (async )?fn check_inv_<name>` methods:
  - 3 trivial `assert_invariants!` delegations: `check_inv_loro_no_errors`, `check_inv_org_render_fixed_point`, `check_inv_focus_matches_ref`.
  - `check_inv_no_startup_errors` (Flutter DDL/sync race guard).
  - `check_inv_view_and_watches` (sections 4 + 5, sync).
  - `check_inv_no_orphan_blocks` (section 6, takes `backend_blocks` + `live_blocks_stale` as args).
  - Plus deleted the unused `live_focus_roots_arc` method.
- ✅ D6 (cleanup): pruned ~9 unused-import warnings post-D4. `live_focus_roots_arc` (dead since D4) removed.
- **Result: sut.rs 7322 → 1710 LOC (−5612, −77 %).** All 3 fast slices + storage_consistency_pbt pass post-split.

**Phase C — migrate fat SutHandle transitions to capability-bound per-transition helpers** (not started yet, recalibrated from earlier estimate):
- 8 fat transitions carry inline business logic: `apply_bulk_external_add` (288 LOC), `apply_start_app` (211), `apply_split_block` (195), `apply_toggle_state` (179), `apply_trigger_doc_link` (161), `apply_click_block` (121), `apply_edit_via_display_tree` (116), `apply_edit_via_view_model` (115), `apply_trigger_slash_command` (113).
- Per-migration: 1-3 hr each, moves body into `pbt/transitions/<name>.rs` as `pub fn <name>_apply_to_sut<S: ...>(sut: &mut S, ...)` bound on capability traits.
- Coupling cost: most reach into private E2ESut state (`driver`, `ctx`, `reactive_engine`, `wait_for_entity_bounds`, `resolve_uri`, …) — each migration extends one or two capability traits along the way.
- Total expected reduction in `sut_handle.rs`: −1200 to −1500 LOC over 3-5 days. Increases modularity (one transition per file) and slice reusability (capability-bound helpers).
- 40 thin transitions are 4-20 LOC pure dispatches; not worth migrating until a slice asks for them.

### Where to pick up tomorrow

After overnight session of 2026-05-18→19, Phase D is fully landed and Phase A is 2-of-3-true-Skipped done. Real remaining `bodies/` Skipped count: **2** (true blockers):

1. **`matview_consistent_with_ref`** — needs `SutViewModel::root_layout_data_row_ids()` (and ref-side caps for `block_state.blocks` + `layout_blocks` + `profile_block_ids` + `is_descendant_of_any` + `expected_focus_root_ids`). Plus `RunMode::Warn` + a `Skipped`-path classifier for the soft-check semantics. Larger lift (~3-4 hr including the ref-side caps).
2. **`viewmodel_tree_virtual_slots`** — needs `display_tree` wired into `WidgetSnapshot` + virtual-slot entity IDs propagated through the IR. Largest architectural lift (~1 day).

Recommended next steps, ROI-ordered:

1. **Continue splitting `check_invariants_async` into named per-section methods** (~2-3 hr) — sections `1`/`1b`/`2`/`2b`/`3`/`7`/`8`/`9/10` still inline. Each extraction requires moving the local `resolve` closure setup into a `self` method or recomputing the `lazy_doc_uri_map` inside the new method (cheap, ~10 LOC each). After this, `check_invariants_async` becomes a narrative sequence of 14 `self.check_inv_X(ref_state).await` calls and individual sections become individually testable / replaceable. Pure modularity win, no behavior change.
2. **Phase A3: promote `matview_consistent_with_ref`** (~3-4 hr) — extend `SutViewModel::root_layout_data_row_ids()` mirroring the A2 pattern (snapshot reactive_engine + extract data rows), add the missing ref-side caps (`RefBlockTreeReadAll`, `RefLayoutBlocks`, `RefFocusRoots`). Promote the body, delete the inline equivalent in `sut_check_invariants.rs`.
3. **Phase C first migration: `apply_focus_editable_text`** (~1-2 hr) — extend `SutFocusWrite` with `wait_for_entity_bounds(id, timeout)` + `click_entity(id, region)`. Move the body into `pbt/transitions/focus_editable_text.rs` as `pub async fn apply_to_sut_<S: SutFocusWrite>(sut: &mut S, id: &EntityUri)`. Thin adapter remains in `sut_handle.rs`. Sets the template for the 8 fat transitions (`apply_bulk_external_add` 288 LOC, `apply_start_app` 211, `apply_toggle_state` 179, `apply_trigger_doc_link` 161, `apply_click_block` 121, `apply_edit_via_*` 115 each, `apply_trigger_slash_command` 113).
4. **Phase A4: promote `viewmodel_tree_virtual_slots`** (~1 day) — needs `display_tree` wired into `WidgetSnapshot` + virtual-slot entity IDs threaded through the IR. Bigger structural change.

### Final LOC distribution (post-overnight)

| File | LOC | Purpose |
|---|---|---|
| `sut.rs` | 1700 | `E2ESut<V>` struct + `FocusRoot`, Deref/DerefMut/Debug/Drop, `new`, `with_driver`, chord dispatch (`send_key_chord`, `dispatch_block_op_via_chord`, `send_leader_chord`), URI/keybinding helpers, the 4th `impl<V> E2ESut<V>` block (state-machine setup + helpers), `impl StateMachineTest`. |
| `sut_handle.rs` | 2426 | `impl SutHandle for E2ESut<V>` — 52 transition dispatch methods. |
| `sut_check_invariants.rs` | 3017 | `apply_transition_async` + `check_invariants_async` (still ~2740 LOC, 14 numbered sections — 6 extracted into named `check_inv_*` methods so far) + live-data mirror accessors. |
| `sut_capabilities.rs` | 751 | Capability trait blanket impls (`SutBlockTree`, `SutLoroLog`, `SutOrgRender`, `SutViewModel::headless_error_node_count` from A2, etc.). |
| `sut_keybindings.rs` | 86 | Leader-chord YAML lookup. |
| `sut_row_parsing.rs` | 160 | Row→Block conversion + property helpers. |
| `sut_macros.rs` | 24 | `assert_invariants!` declarative macro. |

**`sut.rs` shrinkage: 7322 → 1700 LOC (−5622, −77 %)** while 4 of 5 slices continue to pass post-refactor:
- ✅ `editor_pure_pbt` (fast)
- ✅ `org_roundtrip_pbt` (fast)
- ✅ `storage_consistency_pbt` (~106-135 s, gates the SqlOnly E2ESut variant)
- ✅ `cdc_delivery_pbt` (~86 s)
- ⚠ `loro_sync_controller_pbt` fails on the **checked-in regression seed** `6e33948a…` which shrinks to `(peers:2, peer_counter:3, transitions:[])` — failure happens at init with zero transitions applied. Pre-existing issue: the seeds in `tests/loro_sync_controller_pbt.pbt-regressions` predate this work, the failure is at `crates/holon-integration-tests/src/pbt/loro_sync/mod.rs:123` (invariant I1: downstream store doesn't mirror SUT primary Loro doc), and the slice uses `StubSut` not `E2ESut` — so the D-phase relocation can't have caused it. Worth investigating separately as an unrelated bug in the Loro sync controller.

### Final session summary (2026-05-18→19, autonomous)

In one overnight session:

- **2 deferred invariants promoted to live** (`InvOrgRenderFixedPoint`, `InvViewmodelNoErrorWidgets`) by extending `SutOrgRender` and `SutViewModel` capability traits. 2 of 3 true-Skipped bodies now functional; 2 still blocked on bigger architectural lifts.
- **6 of 14 `check_invariants_async` sections** extracted into named `check_inv_*` methods (`no_startup_errors`, `loro_no_errors`, `org_render_fixed_point`, `focus_matches_ref`, `view_and_watches`, `no_orphan_blocks`). Wide PBT runner is now half-narrative.
- **`sut.rs` decomposed** into 6 sibling files: `sut.rs` (struct + lifecycle + StateMachineTest), `sut_handle.rs` (52 SutHandle dispatch methods), `sut_check_invariants.rs` (apply + invariant runner), `sut_keybindings.rs`, `sut_row_parsing.rs`, `sut_macros.rs`. `sut.rs` 7322 → 1700 LOC.
- **Each split preserved by visibility bumps** to `pub(super)` on the methods/fields that newly cross module boundaries — no `pub` leak past the `pbt` module.
- **All builds clean**, 4 of 5 slices pass post-refactor (the 5th was already failing on a checked-in seed before the work started).

Sibling files now in `crates/holon-integration-tests/src/pbt/`:
- `sut.rs` (1710 LOC) — `E2ESut<V>` struct + `FocusRoot`, lifecycle (`new`, `with_driver`, `Drop`, `Deref`/`DerefMut`/`Debug`), chord dispatch (`send_key_chord`, `dispatch_block_op_via_chord`, `send_leader_chord`), URI/keybinding helpers (`resolve_uri`, `resolve_stable_id`, `find_keybinding_for_op`), the 4th `impl<V> E2ESut<V>` block (lots of state-machine setup + helpers), `impl StateMachineTest`.
- `sut_handle.rs` (2426 LOC) — `impl SutHandle for E2ESut<V>` — 52 transition dispatch methods.
- `sut_check_invariants.rs` (2991 LOC) — `apply_transition_async` + `check_invariants_async` (still one ~2780-LOC method with 14 numbered sections) + live-data mirror accessors. Next architectural target: split `check_invariants_async` into ~14 named `check_inv_<name>` methods, OR continue promoting `bodies/` skeletons so the inline sections delete themselves.
- `sut_capabilities.rs` (711 LOC) — capability trait blanket impls (`SutBlockTree`, `SutLoroLog`, `SutOrgRender`, etc.).
- `sut_keybindings.rs` (86 LOC) — leader-chord YAML lookup.
- `sut_row_parsing.rs` (160 LOC) — row→Block conversion + property helpers.
- `sut_macros.rs` (24 LOC) — `assert_invariants!` declarative macro.

**Stage B+ — future slices:**
The pattern: each new slice forces extraction of one or two more capability methods. Don't pre-extract.
- Notification-bus PBT → would extract `SutNotifications` + revisit the killed `CanonicalEvent`.
- Real GPUI + in-memory blocks → Phase 9 follow-up plan; opens the frontend backing seam.

## 8. Naming conventions

- Test files: `<slice-name>_pbt.rs` in the relevant crate's `tests/`. Examples: `editor_pure_pbt.rs`, `editor_loro_pbt.rs`, `matview_drift_pbt.rs`, `block_cell_registry_pbt.rs`. **Do not** put phase numbers (`t0`, `t1`) in filenames — they go stale.
- Slice types: `<SliceName>Slice` (declaration), `<SliceName>Ref`, `<SliceName>Sut` (assemblies).
- Capability impls: `<Backing><Capability>`, e.g. `MemBlockStore`, `TursoBackedSut`, `GpuiLayoutImpl`.
- Capability traits: `Ref<Thing>` / `Sut<Thing>` for read; suffix `Mut` / `Write` for write. Reads first, writes split.
- Invariant ids: keep the `inv-<area>-<predicate>` shape already in the registry. Stable identifiers — log greps depend on them.

## 9. Where this design was wrong (post-implementation retrospective)

The §9 risks were stated before the work landed. Verdict:

- **`CanonicalEvent` stayed empty.** Never materialised — no slice needed cross-medium notification assertions. Killed before the schema ossified; slice-local event types remain right.
- **Forwarding boilerplate manageable.** Three slices ship without a derive macro. The `E2ESut<V>` blanket impls in `sut_capabilities.rs` are ~700 LOC mechanical forwarding; `EditorPureSut` is ~150 LOC. Below the "write the macro" threshold.
- **Quiescence model split, not leaked.** Original `SutQuiesce::quiesce()` became (a) per-call `SutCdc::drain_cdc` invoked by the transition executor before invariant evaluation, and (b) per-tick eager drain inside `CachingProxy` for VM emissions. Transition bodies don't touch either — the harness owns both.
- **Trait bounds stayed manageable.** Where-clauses on individual invariant impls top out at 3-4 caps. No umbrella traits introduced. If a future invariant exceeds 5 caps, that's the trigger.

Additional learnings from the implementation:

- **`Invariant<R,S>` vs the registry's `InvariantSpec`** — kept both. `InvariantSpec` is the human-readable catalogue (id, description, min_sut, mode); `Invariant<R,S>` is the executable. Parity guarded by `body_ids_match_registry_ids` self-test. The "merge them" temptation was resisted because the catalogue describes *every* invariant including the still-inline ones, while the trait impls only exist for migrated bodies.
- **Wide PBT keeps inline bodies that assert strictly more than the migrated impls.** State-toggle, root-matches-render-expr, entity-ids-subset-of-data — the wide inline carries diagnostic richness (display_tree pretty-print, label-vs-state_display, fresh `interpret_pure` path) the slim impls don't model. Migration loses coverage; we accept the dual maintenance.
- **The macro-generated thin adapters stay.** `declare_e2e_transitions!` still emits `apply_to_ref(&self, state: &mut ReferenceState)` because that's the proptest-state-machine shape; the trait body delegates to the capability-bound free helper. This is the right boundary — generic transitions ship behind a concrete-typed adapter.

## 10. Adding a new slice

If you're standing up a new slice today:

1. Read this doc + [TESTING_PHASE10_HANDOFF.md](../TESTING_PHASE10_HANDOFF.md).
2. Pick the slice axes (storage / renderer / driver). Don't think of slices as "wide vs narrow" — think of them as compositions on those three axes.
3. Reuse existing SUT variants where possible: `E2ESut<Full>` (wide), `E2ESut<SqlOnly>` (no UI), `EditorPureSut` (pure data).
4. Pick the invariants from `pbt/invariants/bodies/` whose trait bounds your slice satisfies. Trait bounds gate compile-time membership — if your SUT lacks `SutLoroLog`, `InvLoroNoErrors` won't compile into your slice.
5. Add a new test file in `crates/holon-integration-tests/tests/` named `<slice>_pbt.rs`. The archlint rule `pbt-slice-invariant-foreign-module` will keep you honest about not introducing slice-local-only invariants.
6. Run `cargo nextest run --features pbt --lib invariants` — the registry parity tests should still pass.

When in doubt: read this doc + the handoff, write the slice, then send the diff back so we can refine the doc.
