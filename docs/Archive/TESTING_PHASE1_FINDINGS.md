# Phase 1 fold findings — `ToggleCollapse`

Phase 1 of the testing strategy plan: pick one wide-PBT transition that already has a shared counterpart in `holon-pbt-core` + `holon-layout-testing`, refactor the wide PBT to consume the shared struct + factory + impl machinery. Catalogue every breaking change needed in the trait surface.

Target picked: `CollapseToggle` → `holon_pbt_core::ToggleCollapse`. Inverse partner (`ExpandToggle`) has no shared counterpart yet and stays wide-PBT-local.

## What ended up shared

- **The variant struct** — `holon_pbt_core::ToggleCollapse { target_id: String }`. Wide PBT no longer has its own. The wide PBT's `E2ETransition` enum carries `ToggleCollapse(ToggleCollapse)`.
- **The generator** — `<ToggleCollapse as TransitionFactory<LayoutRef<'_, ReferenceState>>>::weighted_generator`. Wide PBT delegates by wrapping `&ReferenceState` in `LayoutRef::new(...)`, mapping `ToggleCollapseReason::NoCollapsibleTargets` → `Reason::NoCollapseToggleCandidates`.
- **The candidate source** — `LayoutRefState::collapsible_target_ids()` already bridged in `pbt/layout_bridge.rs` (returns the wide PBT's `expanded_toggles` set).

## What stayed local

- **`apply_to_ref`** — wide PBT removes `target_id` from `expanded_toggles`. Cannot be shared.
- **`apply_to_sut`** — wide PBT delegates to `SutHandle::apply_collapse_toggle(uri)`, which flips the Mutable<bool> gate directly. The shared impl clicks `expand_toggle_id_for(target_id)` via `Clickable`.
- **`preconditions`** — wide PBT enforces `app_started`, `render_expressions.contains_key(uri)`, `expanded_toggles.contains(uri)`. The shared `preconditions` is empty.
- **`expected_sql`** (otel-testing budget) — local concern of the wide PBT; not in `holon-pbt-core`'s trait surface.

## Findings

### F1. `LayoutRef` is read-only by design

`LayoutRef<'a, R>` wraps `&'a R`, not `&'a mut R`. Shared `apply_to_ref` impls in `holon-layout-testing` therefore cannot mutate the consumer's reference state — `ToggleCollapse::apply_to_ref` is `{}`, intentionally.

**Implication:** for any wide-PBT transition whose `apply_to_ref` mutates ref state (essentially all the rich ones — pin tracking, expanded set, focus history, …), the consumer must keep a local `TransitionImpl` whose `apply_to_ref` body lives in the consumer crate. Only `apply_to_sut`, `preconditions`, and the generator can flow through the shared crate.

**Decision for Phase 2:** *Do not* extend `LayoutRef` to a mutable variant. The current asymmetry has good structure — shared crates own *gestural* semantics (which element to click, what the variant means); consumers own *consequence* semantics (what state mutates as a result). Forcing both into a shared impl entangles them.

If a future variant genuinely needs cross-consumer ref mutation, the right answer is a *new* mutating capability method on a separate trait (e.g. `LayoutRefMut::record_collapsed(&mut self, target_id: &str)` with a default-noop), not a global `&mut` wrapper. This keeps the read/write split explicit and prevents accidental coupling.

### F2. SUT capability gaps are silent

`SutClickAdapter::click_at_element` calls `SutHandle::apply_click_at_element`, which has a default impl that panics with "the concrete SUT for this PBT must implement this." None of the three concrete SUTs in the wide PBT do, today.

Because `ToggleCollapse` is dormant (no fixture corpus produces `expand_toggle` blocks), the panic is never hit. But the wide PBT now imports the shared `apply_to_sut`-via-Clickable path and *would* hit the panic the moment a fixture activates the transition.

**Decision for the fold:** wide PBT keeps `apply_to_sut` local, delegating to `SutHandle::apply_collapse_toggle` (the existing direct-flip shortcut), not to the shared `Clickable` path. This preserves current behaviour exactly.

**Open follow-up:** when the corpus grows `expand_toggle` blocks, implement `apply_click_at_element` on `E2ESut`, `GpuiUserDriver`, `TuiUserDriver`. At that point switch this transition's `apply_to_sut` to the shared impl (delete the local override). The shared path exercises more of production (click → dispatch → handler → Mutable flip) than the direct flip; that's the point of having a shared `Clickable`-based variant.

### F3. `Validated` Reason types must be mapped at the boundary

The shared factory returns `Validated<_, ToggleCollapseReason>`; the wide PBT works in `Validated<_, Reason>`. The fold needs a per-variant `Reason::NoCollapseToggleCandidates` mapping. Wide PBT's `Reason` enum already has the exact variant.

**Implication:** consumer crates need a per-variant mapping function or inline match. Cost is small (≈3 lines per variant) and explicit. **No trait-surface change needed.**

### F4. Variant naming: `Collapse*` → `Toggle*`

Wide PBT's old struct was named `CollapseToggle { block_id: EntityUri }`. Shared struct is `ToggleCollapse { target_id: String }`. Three differences:
- Word order (`Collapse*Toggle` → `Toggle*Collapse`) — purely cosmetic; the latter matches the imperative-verb naming convention in `holon-pbt-core`.
- Field name (`block_id` → `target_id`) — semantic gain: not every collapsible widget is a block.
- Field type (`EntityUri` → `String`) — the shared crate must be agnostic of `holon_api::EntityUri`. The consumer parses at the boundary (helper `parse_target` in the fold file). **Cost: one `EntityUri::parse(&self.target_id)` call at three sites (preconditions, apply_to_ref, apply_to_sut).**

**Implication for Phase 2:** every fold has this parse-at-boundary tax. For 5–10 variants the tax is negligible. If it becomes a pattern, consider a typed-wrapper trait (`ParseTarget<T>`) — but not yet.

### F5. `holon-pbt-core`'s current variant zoo is not the bottleneck

Today only four variants are in `holon-pbt-core::interactions`: `SwitchViewMode`, `ToggleCollapse`, `ToggleDrawer`, `DeliverBlockContent`. The fold for one of them was straightforward. The other three are similarly UI-shaped and should fold the same way (likely also with the F1/F2 asymmetry). What's *not* shareable today: anything that mutates ref state, anything whose SUT capability isn't yet wired on the wide PBT side.

The wide PBT has ~50 transitions; only this small handful is fold-eligible without a parallel investment in capability traits. **This bounds the realistic scope of variant-sharing.** Phase 2's leaf-crate design should not assume "most transitions become shared" — the realistic ratio is closer to 5/50.

## Trait-surface changes required (zero)

The Phase 1 exit criterion: "Catalogue every breaking change needed in `holon-pbt-core`'s trait surface. If >1 breaking change, expand it here before Phases 2–7 build on it."

**Zero breaking changes to `holon-pbt-core`.** The trait surface (`TransitionFactory<Ref>`, `TransitionImpl<Ref, Sut>`) and the existing variant struct (`ToggleCollapse`) were sufficient as-is. The fold consumed them directly.

**Phase 2 unblocked.** Proceed to extracting reusable content into leaf crates with the understanding that:
1. Only `gestural` semantics share cleanly.
2. The ≥2-consumer gate (v4 plan) is the right discipline — `ToggleCollapse` has exactly 2 consumers (layout PBT + wide PBT) now that this fold lands.
3. Each future fold pays a small parse-at-boundary tax (F4).

## Capability-pattern verdict (v3 plan question)

**Yes — the capability+wrapper template from `holon-layout-testing` does generalise**, but with the F1 asymmetry: read-only for ref state, mutating for SUT. The Phase 1 second-target proposal in v3 ("introduce a `holon-pbt-loro` leaf crate to confirm the template generalises beyond UI") becomes redundant once we accept F1 as the template's *correct* shape. The Loro sync PBT can still consume `holon-pbt-core`'s trait machinery; it just won't share many variant structs because its domain (peer ops, merges) doesn't overlap with the UI variant set today.

## Recommendation for Phase 2 sequencing

- **Skip** introducing `holon-pbt-loro` as a leaf crate. The ≥2-consumer gate isn't met (only `loro_sync_controller_pbt` would consume it).
- **Defer** `holon-pbt-blocktree` as a *new* leaf crate. `holon-block-roundtrip-testing` already exists with overlapping scope. Extend it instead.
- **Promote** the layout_bridge.rs pattern into a documented template in `docs/TESTING_STRATEGY.md` (Phase 12). It's the load-bearing piece.
- **Add** sequel-fold candidates as their corpus pre-requisites land: `SwitchViewMode` and `ToggleDrawer` are next, but only after the wide PBT's `ReferenceState` populates `switchable_handles` and `drawer_handles` respectively. Until then, those variants stay dormant in the wide PBT.
