# ADR 0019: CapMap — insert-only capability dependency injection

**Status:** Accepted (retroactive — documenting shipped architecture)
**Deciders:** Martin
**Context:** The PBT composition spine (γ design, `crates/holon-pbt-core/src/composition.rs`)
assembles a system-under-test (and its reference model) from independent
capability-providing components rather than from one god-type. The mechanism is
`CapMap`: a typemap keyed by capability trait-object `TypeId`. This ADR records
why `CapMap` is an *insert-only, fail-loud-on-duplicate* dependency-injection
container, why that shape (over a service locator or pervasive constructor
injection) is what enables the single-SUT-shape PBT architecture, and where its
boundary with the production application's DI (`fluxdi`) lies.
**Relates to:** ADR 0007 (wiring manifest for PBT subsets), ADR 0009
(component-subset PBTs and bisection), ADR 0012 (reference-model capability
contract — the trait surface `CapMap` hosts). How-to companion:
`docs/Testing/PbtSlicing.md`.

## Problem

ADR 0012 established the *contract surface*: ~39 `Sut*`/`Ref*` capability traits
that transitions and invariants declare against. ADR 0007 established *which
adapters a run wires*; ADR 0009 established *how subsets form a bisection
lattice*. None of them answered the mechanical question underneath all three:
**how is a concrete run assembled from components, and how are capabilities
resolved at invariant-check time?**

Three forces constrain the answer:

1. **One model, many wirings, assembled the same way.** The same reference model
   runs against the wide headless E2E SUT (Turso + Loro + Org + reactive
   ViewModel), against narrow slices (pure in-memory editor, Loro-backend-only,
   org-ordering-only), and against a live GPUI window
   (`crates/holon-integration-tests/src/pbt/window_slice/components.rs` hosts a
   real GPUI window and a real frontend engine as components). If each shape had
   its own bespoke assembly, "GPUI + memory" and "Turso + Loro" would diverge in
   structure and the bisection lattice (ADR 0009) would not be mechanical.

2. **The pre-capability design was a god-object.** One `Sut` type implementing
   the union of every capability (ADR 0012's diagnosis). Every narrow slice
   either duplicated the scaffolding or joined the monolith and paid seconds per
   case. Gating by trait bounds on one giant SUT type "scales poorly past a
   handful of capabilities" (ADR 0007).

3. **Silent capability shadowing is a correctness hole, not a convenience.** If a
   run can register two providers for the same capability, whichever invariants
   read the *loser* are silently invalidated — they pass or fail against a
   provider that the composition never intended to be authoritative. In a
   differential PBT this is indistinguishable from a real product bug until hours
   of debugging bottom out in the wiring. Fail-loud on duplicate turns that class
   of bug into a compile-adjacent panic at composition time.

## Decision

### 1. `CapMap` is a `TypeId`-keyed typemap of `Arc<dyn Cap>` providers

`CapMap` (`crates/holon-pbt-core/src/composition.rs:107`) stores
`HashMap<TypeId, Box<dyn Any>>` where each value erases an `Arc<dyn Cap>`, plus a
parallel `names: HashMap<TypeId, &'static str>` used *only* for fail-loud
diagnostics. The key is `TypeId::of::<dyn SutBackend>()` — the capability's
trait-object type, not a string, not a concrete implementor. One
`Arc<Concrete>` can back several caps: a single component's `Arc` is cast to
`Arc<dyn C>` once per capability it provides (`CapProvider::register`,
`composition.rs:289`).

Capability identity is minted by the `capability!` / `#[capmap_adapter]` macros
(`composition.rs:91`, `crates/holon-macros/src/capmap.rs`), which emit
`impl CapName for dyn Foo` (`CapName`, `composition.rs:63`) and — critically —
`impl Foo for CapMap`, forwarding every `&self` method to
`self.expect::<dyn Foo>().method(..)`. **A composed `CapMap` that holds an
`Arc<dyn Foo>` is itself a `Foo`.** That is what lets one invariant body read its
declared caps straight off the map with no per-shape plumbing.

### 2. Registration is insert-only and fail-loud on duplicate

`CapMap::insert::<C>` (`composition.rs:130`) panics if a provider for `C` is
already present, naming the capability and listing the three sanctioned remedies.
The invariant: **exactly one provider per capability `TypeId` per composed map.**
The panic message is not decoration — it is the enforcement mechanism for force
(3): a second registration cannot silently shadow the first.

Two escape hatches exist, each deliberately narrower than a re-`insert`:

- `CapMap::replace::<C>` (`composition.rs:164`) overwrites a provider that is
  **already present** — and `assert!`s presence, so replacing a cap that was
  never inserted panics. This is "second wins, on purpose": a builder registers a
  component's default provider, then intentionally swaps in a specialised one
  under the same cap `TypeId` (e.g. the composed Turso builder replaces
  `SqlProjectionComponent`'s fresh-resolver block-tree writer with the
  shared-resolver dispatch-floor writer). `insert` first, `replace` second — the
  ordering is load-bearing and asserted.
- `CapMap::merge_missing` (`composition.rs:244`) folds two overlapping cap sets
  with **first-registered wins**, and *returns the names of the shadowed caps* so
  the caller can disclose the precedence collision (log/assert) rather than hide
  it. An empty return means the maps were disjoint.

Resolution mirrors registration's fail-loud stance: `get::<C>`
(`composition.rs:180`) returns `Option` for genuine "is this wired?" queries;
`expect::<C>` (`composition.rs:192`) and `expect_ref::<C>` (`composition.rs:209`)
panic on absence, because a miss there means *selection said the cap was present
and the map disagrees* — an internal harness bug, never a user-facing missing
feature. Missing-cap access is a selection-guarded assertion, never an
`unimplemented!()` stand-in (ADR 0012 §5, restated in `composition.rs:17`).

### 3. A config is *just a list of components*

`Config` (`composition.rs:296`) wraps a `CapMap` and exposes only `.with(component)`
/ `.with_arc(component)` / `.build()`. The entire per-slice surface is a sequence
of `.with(...)` calls; `Config::with` calls `Arc::new(component).register(&mut caps)`
(`composition.rs:304`). "GPUI + memory" is two `.with` calls; the wide E2E SUT is a
longer list of the *same shape*. Because assembly is a flat, order-sensitive
component list feeding one insert-only map, the bisection lattice (ADR 0009) is
mechanical: dropping a component drops exactly its caps, and selection
(`Needs::selected_against`, `composition.rs:334`) recomputes which invariants
still run.

### 4. Why not the alternatives

- **A service locator (global/ambient registry).** Rejected: a global resolved
  by string or singleton would make "which providers does *this* run wire?"
  unanswerable at a glance, defeat parallel slices sharing a process, and — fatally
  — reintroduce silent shadowing (last writer to the global wins, invisibly).
  `CapMap` is a *value*: each run owns its map, and the insert-only invariant is a
  per-map local property, not a global-state race.

- **Constructor injection everywhere (thread every dependency as a typed
  parameter).** This is the type-safe ideal, but it collapses under ADR 0012's
  many-to-many reality: an invariant body reads an open, run-dependent *subset* of
  ~39 caps, and a hub cap (`RefBlockTree`, `RefBackend`) answers several questions
  for several consumers. Encoding each run's exact cap subset as a distinct
  constructor signature yields a combinatorial explosion of SUT types — the
  god-object problem inverted. `CapMap` recovers the safety at the *use* site
  instead: the `#[capmap_adapter]`-generated `impl Foo for CapMap` means a body
  that reads `dyn Foo` names the trait it needs, and the `Needs` declaration that
  drives selection is single-sourced with those reads (`cap_invariant!`), so a
  read of an unwired cap is impossible-by-selection, and a wired-but-absent cap is
  a fail-loud panic rather than a wrong answer.

- **A permissive typemap (last-insert-wins, like a plain `HashMap`).** This is the
  ergonomic default and exactly the one force (3) forbids. Insert-only is the
  whole point: the container refuses to let a composition express an ambiguity it
  cannot mean.

### 5. Boundary with production DI (`fluxdi`)

`CapMap` is the DI container for the **PBT composition spine only**
(`holon-pbt-core` and its consumers in `holon-integration-tests`, the frontend
slice tests, and the windowed runners). The **production application** boots
through a different container: `fluxdi` (`Injector`/`Module`/`Provider`) in
`crates/holon-app/src/wiring.rs`. The two are not merged and should not be
confused.

What *is* shared — and what "PBT and prod share one wiring shape" means — is that
`CapMap` hosts **real production components**, not mocks: the window slice
registers a live GPUI window and the real frontend engine
(`crates/holon-integration-tests/src/pbt/window_slice/components.rs`); the wide
E2E SUT registers real Turso, Loro, and org projections. So the *same assembly
mechanism and the same SUT shape* carry a slice from a pure in-memory editor all
the way up to a real window, differing only in which components are `.with`'d.
The reference model and invariants never learn which shape they are running
against — they read caps off a `CapMap` that is structurally identical in every
run.

## Consequences

### Payoff

- **One assembly mechanism, all shapes.** Adding a slice is a component list, not
  a new SUT type. The bisection lattice (ADR 0009) and the wiring manifest (ADR
  0007) both stand on this flat, insert-only assembly.
- **Silent shadowing is unrepresentable.** The single-provider-per-cap invariant
  is enforced at composition time with a message that names the cap and the three
  remedies. A whole class of "the invariant was reading the wrong provider" bugs
  cannot occur.
- **Reads are cap-typed, not shape-typed.** `impl Cap for CapMap` plus
  single-sourced `Needs` means an invariant body reads exactly the caps it
  declares; an unwired read is impossible-by-selection and a wired-but-absent read
  is a fail-loud panic — never a wrong answer.
- **Real components at every altitude.** Because `CapMap` stores `Arc<dyn Cap>`,
  the same map hosts a toy `MemoryBackend` in a microsecond slice and a live GPUI
  window in an E2E run, prod-faithfully.

### Cost

- **`Any`/`TypeId` erasure is a genuine unsafety-adjacent seam.** The typemap
  stores `Box<dyn Any>` erasing `Arc<dyn C>`; the `downcast_ref::<Arc<C>>` in
  `get` (`composition.rs:183`) is correct only because the key is minted from the
  same `TypeId`. The `.expect("CapMap type key invariant")` documents that the map
  trusts its own key discipline. This is the "mechanically-risky part" the module
  doc calls out (`composition.rs:11`) — de-risked by the round-trip tests, but it
  is machinery, not language-level safety.
- **`&mut self` caps can't be forwarded through a shared `Arc`.** The
  `#[capmap_adapter]` macro emits a fail-loud `unimplemented!` for `&mut self`
  methods (`crates/holon-macros/src/capmap.rs`), so drains/apply-phase side
  effects route around the map. Write caps compose in only via interior
  mutability (`composition.rs:35`). A slice that wrongly routes a drain through the
  map panics clearly — but that it *can* be mis-wired is a sharp edge.
- **Two DI systems in the tree.** `CapMap` (PBT) and `fluxdi` (prod app) coexist.
  A reader must know which layer they are in. This ADR and the module docs are the
  mitigation; the risk is that a future agent tries to unify them or wire prod
  through `CapMap` (it isn't, by §5).
- **Order-sensitivity is real, if disclosed.** `replace`/`merge_missing`
  precedence depends on registration order. It is asserted (`replace` panics on
  absence) and disclosed (`merge_missing` returns shadowed names), but a builder
  that lists components in the wrong order can still express the wrong precedence.

## Known weaknesses / open questions

- The `names` table duplicates the `TypeId` keyspace purely for diagnostics
  (`composition.rs:111`); it must be kept in lock-step with `providers` on every
  mutation (see the `replace` note at `composition.rs:172` on the "asymmetry
  trap"). A future rename under an existing `TypeId` is the scenario the lock-step
  guards.
- There is no compile-time proof that a component's `register` inserts exactly the
  caps its type claims — a component could under- or over-register. Selection
  (`Needs`) catches under-registration at run time (a selected invariant panics on
  the missing cap); over-registration is caught only if a *second* provider
  collides.

## References

- `crates/holon-pbt-core/src/composition.rs` — `CapMap` (`:107`), `insert`
  (`:130`), `replace` (`:164`), `get` (`:180`), `expect` (`:192`), `expect_ref`
  (`:209`), `merge_missing` (`:244`), `CapProvider` (`:289`), `Config` (`:296`),
  `Needs::selected_against` (`:334`).
- `crates/holon-macros/src/capmap.rs` — `#[capmap_adapter]` (typemap key +
  `impl Cap for CapMap` forwarding).
- `crates/holon-app/src/wiring.rs` — production DI via `fluxdi` (the *other*
  container; §5).
- `crates/holon-integration-tests/src/pbt/window_slice/components.rs` — real GPUI
  window + frontend engine as `CapProvider`s.
- ADR 0007 (wiring manifest), ADR 0009 (bisection lattice), ADR 0012
  (reference-model capability contract).
