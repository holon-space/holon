# open-registry-poc

Explores replacing the PBT's central `E2ETransition` enum + `declare_e2e_transitions!`
macro with an **open registry**: every transition / generator / invariant is contributed
by a self-contained module via `inventory::submit!`, and the central code names none of them.

```
cargo run     # narrated end-to-end demo
cargo test    # typetag roundtrip + cap-gate unit tests
```

## What it shows

| Property the enum buys today | How the open design recovers it |
| --- | --- |
| proptest generation (`aggregate_transitions`) | `core::build_alphabet` folds a weighted `Union` over `inventory::iter::<TransitionGen>` |
| shrink `Clone` on the transition value | `dyn-clone` (`clone_trait_object!`) on `Box<dyn Transition>` |
| replay serialization (ADR 0009 bisect) | `typetag` tagged ser/de of `Box<dyn Transition>` |
| `required_caps()` alphabet gating | a `Transition` trait method + a `CapSet` filter in `build_alphabet` |
| value-level mirrors (`variant_name`, `required_caps`) | ordinary trait methods — no macro `match` |

`src/core.rs` is the entire "central" surface and it mentions **no concrete variant**.
`split.rs`, `toggle.rs`, and `invariants.rs` each self-register. Adding `toggle.rs` did not
edit `core.rs` — the property the production enum cannot offer.

## Unified-architecture revision (folded onto γ's `CapMap`)

The transitions now sit on the γ design's own SUT type and follow three corrections:

1. **The SUT is a `CapMap`** (`TypeId → Arc<dyn Cap>` typemap), not a parallel `dyn Sut`
   bundle — the same shape as `holon-pbt-core/src/composition.rs::CapMap`. A component
   (`InMemBackend`) registers one `Arc` under several caps; write caps take `&self` and
   mutate through interior mutability (`Mutex`), so the SUT is driven via `&CapMap`
   (`&mut` would only be needed to *restructure* the map — AddPeer/lifecycle).
2. **`cap_transition!` injects the cap extraction.** The `caps: { tree: dyn SutBlockTreeWrite }`
   clause single-sources *both* `required_caps()` *and* the
   `let tree = sut.expect::<dyn SutBlockTreeWrite>()` binding in `apply_to_sut`, so bodies
   are narrowly typed with no hand-written `expect` and declared-vs-used caps cannot drift.
   (The transition analog of γ's `cap_invariant!`.) The uniform `apply_to_sut(&self, sut: &CapMap)`
   signature is unavoidable — `Box<dyn Transition>` needs one signature across heterogeneous
   transitions — but the macro hides it.
3. **`apply_to_sut` takes no ref.** Transitions are self-contained (all data baked in at
   generation time); ref is read only by the *generator* and mutated only by `apply_to_ref`.
   This matches the production "move required info into the transition" refactor.

Invariants are now cap-gated by the *same* predicate as transitions (each declares the caps
its body reads), so `expect` is always a proven-present lookup, never a panic or a faked `None`.

## Subsystem-config shrinking (Design §8.7) — `shrink.rs`

The active *optional-subsystem set* is **shrinkable proptest input**: a failing case
auto-minimizes to the minimal `(subsystem set, sequence)` that still reproduces. Three real
components — an always-on `BlockStore` plus two optional axes, `Toggle` and `Editor` — give a
genuine subset to minimize (`proptest::sample::subsequence` shrinks a present subsystem toward
absent, so "fewer subsystems = the minimal causal set" for free). A transition whose subsystem
isn't wired is a deterministic no-op (the cap gate = §8.7 precondition replay).

Bugs are planted as **wrong *reference* data** (the components stay correct), so each
differential invariant fires only under its causal subsystem. The demo and the
`shrinking_isolates_the_causal_subset` test show proptest minimizing to:

| Plant | Minimal config | Minimal sequence | Why |
|---|---|---|---|
| `BlockTreeBug` | `[]` | `[]` | always-on substrate; no optional needed |
| `ToggleBug` | `[Toggle]` | `[]` | seed-time ref divergence; fires when Toggle selected |
| `EditorBug` | `[Editor]` | `[Type('a')]` | behavioral; needs Editor wired **and** one keystroke — joint minimization |

`causal_structure_over_the_powerset` proves the same causal table *deterministically* (no
shrinker) over the whole `{Toggle, Editor}` powerset — the robust §8.7 evidence, with teeth in
both directions (fails iff the causal subsystem is wired). The config is keyed on the stable,
serializable `Subsystem` enum, **not** the non-stable `TypeId` used to gate the alphabet.

> Greedy ≠ ddmin (§8.7/§8.8): proptest's shrink is a greedy hill-climb, so a *local* minimal
> config — here it lands on the true causal subset, but it isn't a provable global minimum.

## Caps are trait types, not a `Cap` enum

A capability **is** a fine-grained SUT trait (`SutBlockTreeWrite`, `SutMutate`, `SutRead`),
and its runtime identity is `cap::<dyn ThatTrait>()` = `TypeId::of::<dyn ThatTrait>()` — the
production `CapId`. The token a transition declares in `required_caps()` is the *same* trait
whose method its `apply_to_sut` drives, so:

- there is no `Cap` enum and no enum↔trait mapping to maintain;
- declaration and use **cannot drift**, so the production
  `required_caps_match_transition_impl_bounds` guard test is unnecessary here;
- `cap::<T>()` derives the token (and a display name) straight from the type, so adding a new
  cap trait needs no central edit — exactly the open property we want for transitions too.

Caps are *stored*/advertised by `TypeId` (DI under the hood: `CapSet` is a `HashSet<TypeId>`),
but *accessed* through one `&mut dyn Sut` bundle that supertrait-implies every cap. We
deliberately do **not** resolve each cap trait separately (`container.resolve::<&mut dyn A>()`
+ `…::<&mut dyn B>()` would be two aliasing mutable borrows of one container). The bundle is
the production `CapMap: SutHandle`-implements-everything facade that keeps multi-cap
transitions borrow-legal.

## Honest residual costs (the real trade vs. the enum)

1. **Erased SUT.** `apply_to_sut` takes `&mut dyn Sut`, not a generic `S: SutHandle`. Cap
   *presence* is enforced at the alphabet (CapSet) level, not in each variant's `S` bound.
   Little real safety is lost because production narrowing is already a runtime
   `required_caps()` gate — but the compile-time *documentation* of which cap a variant uses
   moves from the type signature to the `required_caps()` body. (With the trait-TypeId caps,
   that body literally names the trait, so it is still self-evident.)

2. **Async needs `#[async_trait]`.** `async fn` in a trait is not yet object-safe, so the real
   `async fn apply_to_sut` behind `dyn` would wear `#[async_trait]` (or return `BoxFuture`).
   Kept sync here so the PoC builds clean. This is a genuine erasure cost, not a blocker.

3. **Lost exhaustiveness on the *operation* axis.** Adding a new cross-cutting operation
   (e.g. `SqlBudget`) becomes a new trait method → every transition impl must be updated, and
   the compiler enforces it at each impl site rather than via one total `match`. This is the
   mirror image of the win: easy to add variants, harder to add operations. Good trade for a
   codebase with ~stable operations and a growing variant set.

4. **One residual centralization.** `transitions/mod.rs` still lists the modules so their
   `inventory::submit!` constructors get linked. A build script that globbed the directory
   would remove even that. (`core.rs` already lists nothing.)

## Not a cost: wasm

`inventory` supports all wasm targets. The only caveat (`__wasm_call_ctors` for
instantiated-once-called-many modules) does not apply: this registry is native-test-only.

## Relationship to the production γ design

This crate is an **exploration**, not a plan of record. The production PBT (see
`docs/Testing/PbtCompositionDesign.md`) deliberately keeps a **closed** `E2ETransition` enum +
`declare_e2e_transitions!` macro and dispatches `apply_to_sut` generically as
`impl<S: SutHandle> TransitionImpl`. That is correct *today* for one concrete reason: during the
F2 convergence two SUT types coexist — `E2ESut<V>` (the live `general_e2e_pbt`) and a composed
`CapMap` (the `general_e2e_composed_pbt` swap) — and the same generic dispatch monomorphises to
both. A `Box<dyn Transition>` (what this PoC uses) **cannot** do that: it must erase the SUT to a
single type. So the open encoding here is **blocked until E5** (E2ESut deleted, `CapMap` sole SUT).

What production has already adopted that this PoC also has: the `CapMap` SUT, `TypeId`/`CapId`
caps, cap-gated alphabet **and** invariants, and subsystem-config shrinking (§8.7). What landed
from this exploration into production is the **`cap_transition!` authoring seam** (Design §8.9):
it single-sources each transition's cap and — crucially — makes the open-vs-closed decision a
property of *one macro's expansion*, not of the 52 transition files. So nothing is locked in.

## Migration path to the open trait-based encoding (staged, reversible)

If we ever decide the open registry is worth its costs, the path is staged so each step is
independently valuable and the decision stays reversible:

- **Tier 1 — LANDED (no preconditions).** `cap_transition!` is the authoring surface; the cap is
  single-sourced (drift-guard drops out). Closed dispatch unchanged.
- **Tier 1.5 — optional, no E5 needed.** A `build.rs` glob of `transitions/*.rs` generates the
  `declare_e2e_transitions!` variant list → *drop-a-file = a new variant*, while keeping the
  closed enum, generic dispatch, native async, serde, and exhaustiveness. This is the
  "open authoring + closed dispatch" corner neither the enum nor this PoC occupies — likely the
  best stopping point for an in-tree-only transition set.
- **Tier 2 — the open encoding (needs E5: `CapMap` sole SUT).** Flip `cap_transition!`'s
  expansion (behind a cargo feature so both backends build and CI can prove equivalence):
  1. emit `#[typetag::serde] #[async_trait(?Send)] impl Transition for X { apply_to_sut(&self, &Ref, &mut CapMap) }` + `inventory::submit!` of a generator, instead of `impl<S: Cap> TransitionImpl`;
  2. replace `declare_e2e_transitions!`/`aggregate_transitions` with the inventory-iterating
     aggregator (this crate's `core::build_alphabet`) — same wiring+cap gate, `Union` of
     `Box<dyn Transition>`;
  3. swap the replay/bisect format to typetag string tags (de-risk first by keying the recorded
     sequence on `variant_name()` so the saved corpus survives — ADR 0009);
  4. `dyn_clone::clone_trait_object!(Transition)` for the shrinker; inventory self-audit replaces
     the compile-time exhaustiveness (+ optional checked-in manifest for a CI closed-set gate);
  5. delete the enum once parity is proven against the feature-gated closed path.
  - **Costs paid only here:** `async_trait` boxing, `typetag`+`inventory` deps, exhaustiveness →
    runtime audit, loss of the (already non-load-bearing) static per-variant cap bound.
  - **The payoff that only Tier 2 buys:** per-subsystem **crate** ownership — `holon-loro` etc.
    `inventory::submit!` their own transitions, alphabet auto-assembles from linked crates. (The
    Tier-1.5 glob is in-crate only.)

This PoC is the worked reference for Tier-2 steps 1–4 (`core.rs::build_alphabet`, typetag replay,
`dyn-clone`, cap-gating, the `Transition` trait shape). See Design §8.9 for the seam in production.
