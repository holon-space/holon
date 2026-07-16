# ADR 0027: ReferenceState private-fragment extension registry — DEFER (Option 0), reject CapMap-as-registry

**Status:** Accepted (2026-07-16). Ruled by Martin from the Inc 6 options
document (three independent lens analyses — ergonomics, performance,
type-safety — each critique-verified against the repo).
**Deciders:** Martin
**Relates to:** ADR 0012 (reference-model capability contract), ADR 0013 (test
support boundary), ADR 0019 (CapMap dependency injection),
`docs/Testing/PbtCompositionDesign.md` §5.3/§5.5/§8.9,
`docs/Plans/RefStateSplit-2026-07-12.md` §3 Increment 6.

## Problem

`ReferenceState` (`crates/holon-integration-tests/src/pbt/reference_state.rs:325`)
holds the subsystem-private fragments `loro: LoroRefExt`, `files:
FileAdapterState`, `mcp: MCPServerActorState` as plain named fields.
PbtCompositionDesign §5.5 sketches, as backlog item (b), an *open registry* so a
subsystem crate could contribute ref-state with **zero edits to
integration-tests**. Increment 6 is the decision gate: build that registry now,
in what shape — or not at all.

Ground facts every option had to respect (all three lenses converged;
code-verified):

- **Value semantics with bespoke Clone seams.** proptest clones `ReferenceState`
  per step and per case (shrinking keeps old clones alive). `LoroRefExt::clone`
  deliberately Clone-SHARES its `clock_feed` `Arc<Mutex<..>>` (harness seam) while
  Clone-DEEP-FORKING `shadow_mesh` (model state). Any registry must run each
  extension's **own** `Clone`; Arc-sharing extensions across clones is
  semantically wrong (cross-clone corruption during shrinking), not merely slow.
- **Perf is a non-axis.** Per-clone cost is dominated by the ShadowMesh
  deep-fork and the domain `BTreeMap<EntityUri, Block>` clone. A registry adds
  ~one Box alloc + vtable call per ext per clone — noise. The registry can only
  lose a little, never win.
- **`Resolved<T>` witness.** `with_resolved_doc_uris` clones and remaps ids; a
  registry needs a resolution hook, and how that hook defaults determines
  whether the witness silently weakens.
- **Base rate is n=1.** After five increments, exactly **one** genuinely
  private, out-of-tree-shaped extension exists (`LoroRefExt`). `files`/`mcp`
  serve caps consumed by core transitions and will plausibly never live
  out-of-tree. §5.5's payoff customer (a Loro+Iroh module) is hypothetical.

## Decision

**Rule Option 0 (null): keep the plain named fields; build no registry in
Increment 6.** A future subsystem ships its ext type + generators + transitions
in its own crate (the landed Inc 5 shape) and makes a small PR against
integration-tests: one field, one `Default` line, and its thin cap-impl file in
`ref_caps/`.

This ruling is **explicitly on the path to the other options** — deferral, not
rejection of §5.5. The following riders bind the decision.

### Rider 1 — named revisit trigger

The moment a **second genuinely private `*RefExt`** (iroh/P2P) is *in hand* — not
hypothesized — build **Option A**: a value-semantic `dyn` typemap
(`RefExtMap`) in `holon-pbt-core`, with the type-safety hardening that buys back
most of the lost compile-time guarantees:

- an `ExtRegistered<T>` zero-sized token, mintable only via `register()`, that a
  module's transition constructor requires — so "transition generated but ext
  absent" is unrepresentable at the API surface;
- **no default** `remap_doc_uris` on the `RefExt` trait — every author must
  explicitly claim "no synthetic `EntityUri`s here" or remap, so the `Resolved`
  witness cannot silently weaken;
- a **boot-time `ExtSet` check** ("required exts ⊆ registered exts") at
  `Config::build` (the one salvageable idea from Option B, §5.3's fail-loud rule);
- a `HasRefExts` access seam so module crates blanket-impl their own cap traits
  without naming `ReferenceState` — removing the *second* chokepoint (the thin
  cap impls forced into `ref_caps/` by the orphan rule), which the field-only
  registry does not touch;
- **loro-only scope**: only `loro` moves into the registry; `files` and `mcp`
  stay plain fields (in-tree-forever, core-adjacent; `files.documents` is
  EntityUri-keyed, straddling §5.5's private/additive boundary).

Do not build a registry for one tenant. Before Option A's exact API is ratified,
resolve its one known open defect: the sketched Mut-cap blanket shape does not
borrow-check for the Inc-5 use case (`ref_ext_mut(&mut self)` borrows all of
`self`, so the body cannot hold `&mut LoroRefExt` while reading core data through
the same `self`). A ~1–2 h throwaway experiment — a combined split-accessor
`fn ref_ext_mut_with_core<T>(&mut self) -> (&mut T, &RefCoreView)`, porting one
landed Mut cap until it compiles — is the cheap proof that the deferred design
survives contact with real signatures.

### Rider 2 — CapMap-as-registry (Option B) rejected, permanently

Reusing `CapMap` as the ref-state registry is a **category error**, recorded here
so it does not resurface:

- `CapMap` is `#[derive(Default)]` **only** — it has no `Clone` impl (`Box<dyn
  Any>` isn't `Clone`), so a `ReferenceState` holding one could not even derive
  `Clone`. B fails at compile time before it fails semantically.
- Any `Clone` one *added* would necessarily **Arc-share** providers — the exact
  opposite of what per-step-cloned model state needs. Step N's mutations would
  bleed into the pristine clones proptest keeps for shrinking, corrupting the
  reference model nondeterministically. `Arc<Mutex<E>>` doesn't help (same shared
  state); `with_resolved_doc_uris`'s clone wouldn't deep-copy the ext, so the
  "resolved clone" would mutate the unresolved original — disqualifying for the
  witness.
- There is no `&mut` path (`expect` returns `Arc<C>`), downgrading compile-time
  exclusivity to runtime lock discipline.

CapMap's contract (shared, immutable-after-build, one Arc backs many caps) is
right for *serving capabilities* and wrong for *owned model state*. §5.5's
"symmetric to how the SUT is an open CapMap" is symmetry of shape, not of
instance. Regardless of this ruling, subsystem **capabilities keep being served
via `ref_caps/`** — nothing about caps changes here. Only the salvaged
`CapSet`-style boot-time presence check folds into Option A (Rider 1).

### Rider 3 — confronting §5.5's "production form of the Ref"

§5.5 calls the open registry "the production form of the Ref." That sentence is
addressed head-on, not ignored: **§5.5 stays the target shape**, **Option A is
the ratified *design* for it**, and this deferral is a scope call the plan's own
gate invites — the plan frames Inc 6 as a *decision gate*, and §5.5 is backlog,
not a ratified obligation. Deferring with a named trigger (Rider 1) is a
legitimate ruling, not a violation. C1 (`inventory` auto-registration) remains a
possible later upgrade *after* §8.9 proves inventory in the transition registry.

## Consequences

- Every illegal state (absent ext, duplicate ext, wrong-type ext, un-remapped
  ext in `with_resolved_doc_uris`) stays unrepresentable or a compile error;
  Clone seams stay visible at the struct; read ergonomics, `Debug`, and rustdoc
  stay strictly best; zero perf risk.
- `reference_state.rs` remains a central chokepoint (plan risk R5:
  links/marks, RowIdentity, undo streams collide there), and §5.5's zero-central-
  edit submodule story stays a small-PR away rather than zero.
- Nothing here is irreversible: Increments 1–5 already made the eventual flip
  cheap.

## Key files

`crates/holon-integration-tests/src/pbt/reference_state.rs` (:325 struct, :383
`Resolved`, :436 `with_resolved_doc_uris`),
`crates/holon-integration-tests/src/pbt/ref_caps/peers.rs` (the 123-line cap
chokepoint), `crates/holon-loro-testing/src/ref_ext.rs` (Clone seams :22–40,
`clock_feed` :108, `shadow_catch_up_primary(&self)` :442),
`crates/holon-pbt-core/src/composition.rs` (CapMap :124 — `#[derive(Default)]`,
not Clone), `docs/Testing/PbtCompositionDesign.md` §5.3/§5.5/§8.9,
`docs/Plans/RefStateSplit-2026-07-12.md` §3 Increment 6.

## Appendix: reversibility experiment (2026-07-16, throwaway worktree)

The rider-1 de-risking experiment was run same-day: `RefExtMap` + `HasRefExts`
sketched in holon-pbt-core, `RefPeersMut::add_peer` ported through the combined
accessor, compiled against Inc 5 real signatures.

**Verdict: compiles cleanly, zero borrow contortions.** The split-borrow defect
flagged in the options doc is cured exactly as proposed: the concrete impl
performs the field-disjoint split, so
`fn ref_ext_mut_with_core<T: RefExt>(&mut self) -> (&mut T, RefCoreView<_>)`
is fine as a trait method. Option 0 is confirmed cheap-to-reverse.

Two findings amend the deferred Option A design:

1. **`RefExt: Any + Debug + Send + Sync`** (not just `Send`). `ReferenceState`
   is `Sync`-constrained (BuilderServices impl; a spawn-drop in
   `subsystem_seed.rs`), so every registered extension must be `Sync` — a
   load-bearing constraint on future extension authors, enforced at their
   `impl RefExt`.
2. **`RefCoreView` = holon-api block maps + abstract predicate traits.** The
   ported cap needs a layout-membership check whose type lives in
   integration-tests; the working shape is a pbt-core
   `LayoutMembership { fn is_layout_member(&self, &EntityUri) -> bool }`
   carried as `&dyn LayoutMembership`. Future caps reading other core slices
   widen the view or add sibling accessors — a small, real addition to the
   options doc line-count budget.

Unmeasured (skipped for budget): per-step cap-read counts (the hoisting
question). Measure before building Option A.
