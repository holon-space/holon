# PBT Sharing Patterns — `layout_bridge.rs` Template

How to add a new shared transition variant in `holon-pbt-core` and consume it from multiple PBTs without re-implementing logic. This is the load-bearing pattern Phase 1 of the testing strategy plan validated.

## The Three-Crate Setup

```
holon-pbt-core
  ├── interactions/<variant>.rs      ← the variant struct (POD; no logic)
  └── lib.rs                          ← `TransitionFactory<Ref>`, `TransitionImpl<Ref, Sut>` traits

holon-layout-testing            ← "shared content" leaf crate
  ├── sut.rs                          ← capability traits + LayoutSut/LayoutRef newtypes
  └── transitions/<variant>.rs        ← the *shared* impls keyed off LayoutSut/LayoutRef

<consumer crate>                ← e.g. frontends/gpui/tests, holon-integration-tests
  ├── (frontend session) implements the SUT capability traits
  └── pbt/layout_bridge.rs            ← bridge: impl LayoutRefState for ReferenceState
                                         + SutClickAdapter wraps the consumer's SutHandle
                                         + per-consumer `impl E2ETransitionFactory/Impl for <Variant>`
                                           when the consumer's traits differ from `TransitionFactory/Impl`
```

## Why three crates

The orphan rule: `impl<R, S> TransitionImpl<R, S> for SwitchViewMode` is rejected when both `TransitionImpl` and `SwitchViewMode` are foreign. Inserting a local newtype at the head of `(Ref, Sut)` — `LayoutSut<'a, S>`, `LayoutRef<'a, R>` — makes the impl legal because *at least one* type parameter is local to the impl's crate.

This is the same trick a workspace would use to put extension methods on `std::Vec<T>`: introduce a local wrapper. Here the wrappers are **zero-cost transparent**: `LayoutSut::new(&mut session)` and `LayoutRef::new(&state)` at the dispatch site, no runtime overhead.

## What goes where

| Lives in `holon-pbt-core` | Lives in `holon-layout-testing` | Lives in the consumer |
|---|---|---|
| The variant struct (POD) | The shared `weighted_generator` body | The `LayoutRefState` impl on the consumer's `ReferenceState` |
| The two traits (`TransitionFactory`, `TransitionImpl`) | The shared `preconditions` body | The capability-trait impls (`Clickable`, `LiveBlockSink`, …) on the consumer's session type — typically via an adapter like `SutClickAdapter` |
| | The shared `apply_to_sut` body (calls capability methods, never the SUT directly) | Per-consumer `apply_to_ref` mutation (cannot be shared — see F1) |
| | Capability traits describing what the SUT/Ref must offer | The `E2ETransition*` enum dispatch wiring |

## Bedrock Constraints

These came out of Phase 1's `ToggleCollapse` fold. They are not bugs — they are the pattern's correct shape.

### F1. `LayoutRef` is read-only by design

`LayoutRef<'a, R>` holds `&'a R`, not `&'a mut R`. Shared `apply_to_ref` impls in `holon-layout-testing` therefore cannot mutate consumer ref state — the shared body is empty (`{}`) and the consumer overrides locally.

**Rule:** shared crates own *gestural* semantics (what the variant means, what element to click); consumers own *consequence* semantics (what state mutates as a result). Forcing both into one shared impl entangles concerns that legitimately differ across consumers.

**If you think you need `LayoutRefMut`:** you don't. Add a separate mutating capability method on a small new trait (e.g. `LayoutRefSink::record_collapsed(&mut self, target_id: &str)` with a default-noop). This keeps the read/write split explicit. The wide PBT's `ReferenceState` and the layout PBT's blueprint-state have legitimately different mutation needs; sharing the mutation in the trait body forces them to converge artificially.

### F2. SUT capability gaps are silent landmines

`SutClickAdapter::click_at_element` forwards to `SutHandle::apply_click_at_element`, which has a default impl that *panics* with "the concrete SUT for this PBT must implement this." If a transition is fold-eligible at the type level but the consumer's SUT doesn't yet implement the required capability, you only find out at runtime — and only if the transition's generator produces candidates.

**Rule:** before activating a shared `apply_to_sut` in a new consumer, verify the capability method is *actually implemented* on every concrete SUT type the consumer reaches (e.g. `E2ESut`, `GpuiUserDriver`, `TuiUserDriver`). If not, keep the consumer's `apply_to_sut` local (delegating to the existing direct-call path) and file a follow-up to implement the capability.

The `ToggleCollapse` fold did exactly this: `apply_to_sut` stayed local (calls `sut.apply_collapse_toggle(uri)` directly), waiting on a future PR to implement `apply_click_at_element` on the three SUT types.

### F3. Reason types map at the boundary

The shared factory returns `Validated<_, <SharedVariant as TransitionFactory>::Reason>`. The consumer works in its own `Reason` enum. Map at the boundary:

```rust
match <ToggleCollapse as TransitionFactory<LayoutRef<'_, ReferenceState>>>::weighted_generator(&layout_ref) {
    Validated::Good(pair) => Validated::Good(pair),
    Validated::Fail(_) => Validated::fail(Reason::NoCollapseToggleCandidates),
}
```

≈3 LOC per fold. No trait-surface change needed.

### F4. Parse at the boundary

Shared variant structs are crate-agnostic: `target_id: String`, not `target_id: EntityUri`. Consumers parse at the three sites that use it (preconditions, apply_to_ref, apply_to_sut). Encapsulate in a helper:

```rust
fn parse_target(target_id: &str) -> EntityUri {
    EntityUri::parse(target_id)
        .unwrap_or_else(|e| panic!("[ToggleCollapse] invalid target_id {target_id:?}: {e}"))
}
```

## Step-by-step: adding a new shared variant

1. **Land the struct in `holon-pbt-core::interactions`** as a POD type. Re-export from `holon-layout-testing` for ergonomic access.
2. **Add the capability trait method** on `LayoutRefState` (for ref-state queries the generator needs — e.g. `collapsible_target_ids`) and/or on a SUT capability trait (for the gestural primitive the `apply_to_sut` needs).
3. **Land the shared impl** in `holon-layout-testing::transitions::<variant>.rs`:
   - `impl<R> TransitionFactory<LayoutRef<'_, R>> for <Variant> where R: <CapabilityTrait>`
   - `impl<R, S> TransitionImpl<LayoutRef<'_, R>, LayoutSut<'_, S>> for <Variant> where S: <SutCapability>`
4. **Bridge into one consumer first** (the simpler one). Add a `bridge.rs` file (or extend an existing one) with:
   - `impl <CapabilityTrait> for <ConsumerRefState>` — surface the data the generator needs.
   - `impl <SutCapability> for <SutAdapter>` — wire the gestural primitive to the consumer's actual SUT method.
5. **Either** add a per-consumer `impl <ConsumerTransitionFactory/Impl> for <SharedVariant>` that delegates via `LayoutRef::new` / `LayoutSut::new` (the wide-PBT pattern) **or** consume the shared trait directly (the layout-PBT pattern). Wide-PBT pattern is needed when the consumer has its own transition trait surface (`E2ETransitionFactory`, `E2ETransitionImpl`).
6. **Verify with arch-tests**: every variant in the consumer's `E2ETransition` enum has a sibling `transitions/<snake_case>.rs` file.
7. **Build, test, document the fold**.

## When to use which fold style

- **Layout-PBT style** (consume `TransitionFactory<LayoutRef<…>>` / `TransitionImpl<…>` directly): when the consumer's PBT runner uses `proptest_state_machine` against `holon-pbt-core`'s traits with no consumer-specific trait surface.
- **Wide-PBT style** (consume via a per-consumer trait that delegates to the shared impl): when the consumer has its own `E2ETransition*` trait surface that carries extra responsibilities the shared trait doesn't model (e.g. `expected_sql` for otel budget tracking). Implement the consumer's trait on the shared variant struct; the body forwards.

Both coexist. They are not migration stages.

## Anti-patterns

- ❌ Putting the variant struct in the leaf crate (e.g. `holon-layout-testing`). It belongs in `holon-pbt-core` so consumers can refer to it without depending on the leaf crate. (`holon-layout-testing` re-exports for convenience.)
- ❌ Mutable `LayoutRefMut`. See F1.
- ❌ Single-consumer leaf crates. The ≥2-consumer gate is strict.
- ❌ Lifting "almost-shared" generators by parameterising them with closures. The right answer is more capability methods, not generator polymorphism.
- ❌ A capability trait with five methods. Keep capability traits *small* (one method, sometimes two). New abilities → new trait.

## Glossary

- **Variant struct.** Plain-old-data describing one user-visible interaction (`SwitchViewMode { block_id, target_mode }`). Lives in `holon-pbt-core::interactions::*`.
- **Capability trait.** A small trait the consumer's SUT or Ref must implement to participate in a shared variant impl (`Clickable`, `LiveBlockSink`, `LayoutRefState`).
- **Local newtype.** A zero-cost wrapper (`LayoutSut<'a, S>`, `LayoutRef<'a, R>`) that satisfies the orphan rule.
- **Bridge.** The consumer-side file implementing capability traits on the consumer's ref-state and SUT (e.g. `pbt/layout_bridge.rs`).
- **Adapter.** A consumer-side wrapper (`SutClickAdapter`) implementing a capability trait on top of the consumer's `SutHandle`.

## Reference: the `ToggleCollapse` fold (Phase 1)

The first end-to-end fold. Files touched:

- `crates/holon-pbt-core/src/interactions/toggle_collapse.rs` — pre-existing variant struct.
- `crates/holon-layout-testing/src/transitions/toggle_collapse.rs` — pre-existing shared impl.
- `crates/holon-integration-tests/src/pbt/layout_bridge.rs` — pre-existing bridge.
- `crates/holon-integration-tests/src/pbt/transitions/toggle_collapse.rs` — *new* per-consumer `E2ETransitionFactory`/`E2ETransitionImpl` impls delegating to the shared factory.
- `crates/holon-integration-tests/src/pbt/transitions/mod.rs` — re-export, enum variant rename.
- `crates/holon-integration-tests/src/pbt/transitions/collapse_toggle.rs` — *deleted*.

Total delta: ~110 lines added (mostly new file + docs), ~83 lines removed. Zero `holon-pbt-core` trait-surface changes.
