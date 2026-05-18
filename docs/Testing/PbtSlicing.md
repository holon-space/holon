# PBT Slicing — Capability-Composed Property Tests

**Status**: starting-point design doc, written 2026-05-18. Expect refinement after the first slices land.

**Audience**: a Claude session asked to add or refactor a property-based test in this repo. Read this *before* writing a new PBT, and prefer reusing the abstractions described here over adding monolithic per-test ref/SUT structs.

**Sister docs**:
- `docs/TESTING_HANDOFF.md` — current phase status, what's done, what's next
- `docs/TESTING_INVARIANT_AUDIT.md` — invariant ↔ subsystem matrix
- `docs/TESTING_PATTERNS.md` — fold patterns and pitfalls
- `crates/holon-integration-tests/src/pbt/invariants/registry.rs` — runtime registry

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

## 7. Roll-out strategy

**Stage A — minimum viable decomposition (ships with the first new slice):**
1. Extract `RefBlockTree(+Mut)`, `RefEditorMirror(+Mut)`, `RefFocus(+Mut)` from `reference_state.rs` as traits. Today's `ReferenceState` gets blanket impls for these. No behavior change in the wide PBT.
2. Symmetric on the SUT side: `SutBlockTreeWrite`, `SutEditorMirrorWrite`, `SutFocusWrite`, `SutQuiesce`.
3. Make the seven editor-state-machine transitions generic over those bounds:
   `TypeChars`, `DeleteBackward`, `MoveCursorLeft`, `MoveCursorRight`, `SplitBlock`, `JoinBlock`, `Indent`, `Outdent`.
   Their `apply_to_ref` / `apply_to_sut` become trait-bound. Other transitions stay as-is.
4. Write the first new slice (e.g. `tests/editor_pure_pbt.rs`): minimal `Ref` + `Sut` implementing exactly those six trait pairs.
5. Register the new invariant family in the registry (e.g. `inv-tree-cursor-within-text-len`, `inv-tree-cursor-text-trim-stable`, `inv-tree-structural-integrity`).

**Two-consumer gate.** Stage A only lands when both the wide PBT and the new slice consume the same trait set. This mirrors the `holon-layout-testing` leaf-crate rule: no new abstraction without two real users.

**Stage B — extend opportunistically (per new slice):**
Each new slice you want forces extraction of one or two more capabilities. Don't pre-extract.
- Phase 6's BlockCellRegistry routing PBT → extracts `SutLoroLog` + `SutSqlProjection`.
- Notification-bus PBT → extracts `SutNotifications` + grows `CanonicalEvent`.
- In-memory + GPUI slice → extracts `SutLayout` + opens the frontend backing seam.

## 8. Naming conventions

- Test files: `<slice-name>_pbt.rs` in the relevant crate's `tests/`. Examples: `editor_pure_pbt.rs`, `editor_loro_pbt.rs`, `matview_drift_pbt.rs`, `block_cell_registry_pbt.rs`. **Do not** put phase numbers (`t0`, `t1`) in filenames — they go stale.
- Slice types: `<SliceName>Slice` (declaration), `<SliceName>Ref`, `<SliceName>Sut` (assemblies).
- Capability impls: `<Backing><Capability>`, e.g. `MemBlockStore`, `TursoBackedSut`, `GpuiLayoutImpl`.
- Capability traits: `Ref<Thing>` / `Sut<Thing>` for read; suffix `Mut` / `Write` for write. Reads first, writes split.
- Invariant ids: keep the `inv-<area>-<predicate>` shape already in the registry. Stable identifiers — log greps depend on them.

## 9. When this design will be wrong

Likely. Things to watch for as the first slices land:

- **`CanonicalEvent` either explodes or stays empty.** If a slice needs an event variant nothing else uses, it's pulling its weight; the schema is real. If after three slices `CanonicalEvent` still has the four bootstrap variants and no one's asserting on them, kill it and use slice-local event types.
- **Forwarding boilerplate dwarfs the test.** If slice structs are 200 lines of `impl X for Slice { fn f(&self) { self.field.f() } }`, write the derive macro early.
- **Quiescence model leaks.** If transitions start poking at `quiesce()` directly, the harness isn't owning it correctly — fix the harness, don't normalise the leak.
- **Trait bounds metastasize.** If `where` clauses on every transition exceed ~4 capabilities, introduce umbrella traits — but not before, because umbrellas obscure which caps a transition actually touches.

When in doubt: read this doc, write the slice, then send the diff back so we can refine the doc.
