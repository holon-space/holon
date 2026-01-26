# Next sequel-fold survey (Phase 2 deliverable)

Per Phase 1 finding F5: `holon-pbt-core` carries four shared UI-variant structs today (`DeliverBlockContent`, `SwitchViewMode`, `ToggleDrawer`, `ToggleCollapse`). `ToggleCollapse` is folded. This document costs the next two candidates so a contributor can pick them up in order.

## Candidate A: `ToggleDrawer` — recommended next

### What exists today

- Variant struct: `holon_pbt_core::ToggleDrawer { block_id: String }`.
- Shared impl: `holon_layout_testing::transitions::toggle_drawer` (assumed present alongside `toggle_collapse.rs`; verify before starting).
- Bridge stub: `layout_bridge.rs:42-44` — `drawer_handles()` returns empty; `drawer_is_open(_)` returns `true` (stub).
- Wide-PBT transition: **none.** No `drawer_*` transition exists in `pbt/transitions/`.

### What's needed

1. **ReferenceState data.** The wide PBT boots the default layout with two real drawers (left + right sidebar). The bridge comment at `layout_bridge.rs:38-44` already calls this out: "when their `block_id`s are added to `ReferenceState`, surface them here." So:
   - Add `pub drawer_handles: Vec<DrawerHandle>` (or a domain-shaped equivalent) to `ReferenceState`.
   - Populate it at startup once the default layout is materialised (likely in `apply_start_app` / `init_test`).
   - Add `pub drawer_open: BTreeMap<String, bool>` to track open/closed state — needed for `drawer_is_open`. Init both drawers to `true` (default-open).
2. **Bridge body.**
   - `drawer_handles()` returns `&self.drawer_handles`.
   - `drawer_is_open(block_id)` returns `*self.drawer_open.get(block_id).unwrap_or(&true)`.
3. **SUT capability** (`Clickable::click_at_element` on the wide PBT's adapter). Already wired (`SutClickAdapter`); but its `apply_click_at_element` SutHandle default panics. **Needs implementation on `E2ESut`, `GpuiUserDriver`, `TuiUserDriver`** before activation. This is the F2 landmine from `TESTING_PATTERNS.md`.
   - Alternative: keep the shared variant struct + factory, override `apply_to_sut` locally to a wide-PBT-specific direct path (e.g. `sut.apply_toggle_drawer(block_id)`). Matches the `ToggleCollapse` fold's compromise.
4. **Wide-PBT transition file** `transitions/toggle_drawer.rs` — the per-consumer `E2ETransitionFactory`/`E2ETransitionImpl` impls on the shared struct. Roughly identical shape to `transitions/toggle_collapse.rs`.
5. **Enum + arch test.** Add `ToggleDrawer(ToggleDrawer)` to the `E2ETransition` enum. Sibling file already exists from step 4.
6. **Reason variant.** Add `Reason::NoDrawerHandles` (or reuse `PreconditionFailed`).

### Cost estimate

- ReferenceState additions: ~10 LOC (two fields + Default).
- Startup-time population: ~15 LOC (fetch the default-layout sidebar block_ids).
- Bridge body: ~6 LOC.
- Wide-PBT transition file: ~70 LOC (mirrors `toggle_collapse.rs`).
- SutHandle impl (`apply_toggle_drawer` direct path, or `apply_click_at_element` if doing the full Clickable path): ~30 LOC × 3 implementors = 90 LOC. **Pick the direct path first — it preserves behaviour and matches the `ToggleCollapse` precedent.**
- Reason enum + arch tests: ~3 LOC.

**Total: ~200 LOC, mostly mechanical.** Risk: low. Behaviour change in wide PBT: drawer toggling becomes a live transition (previously absent). Could surface generator-balance issues if drawers are over-weighted; the shared impl uses weight 1, which is fine.

### Pre-requisites

- None blocking. Default layout reliably produces drawer block_ids.

### Why next

- **F2 risk is low** — direct-path `apply_to_sut` sidesteps the `apply_click_at_element` panic landmine. The fold itself is straightforward.
- **Activates a real wide-PBT transition** — `ToggleCollapse` is still dormant pending corpus growth; `ToggleDrawer` fires on every default-layout boot. Validates that the pattern carries a *live* transition end-to-end.
- **Smallest scope** of the remaining three variants.

## Candidate B: `SwitchViewMode` — needs prep first

### What exists today

- Variant struct: `holon_pbt_core::SwitchViewMode { block_id, target_mode }`.
- Shared impl: `holon_layout_testing::transitions::switch_view_mode` (assumed; verify).
- Bridge stub: `layout_bridge.rs:34-36, 68-70` — `switchable_handles()` returns empty, `current_view_mode(_)` returns `None`.
- Wide-PBT transition: **`SwitchView { view_name }`** exists but is a *different concept* — it switches the top-level UI view ("all"/"sidebar"/"main"), not a per-block render mode. Naming collision but no semantic overlap.

### What's needed

1. **Decide on the naming collision.** Either rename wide-PBT `SwitchView` → `SwitchAppView` (or similar) to free the `SwitchViewMode` namespace, or live with the dual concept. Recommendation: rename. The wide-PBT current `SwitchView` is misleadingly named; "app view mode" or "navigation view" is more accurate.
2. **ReferenceState data for VMS handles.** Today `BlockHandle { block_id, mode_names, mode_thunks, in_drawer, initial_mode }`. The wide PBT's `render_expressions` carry rhai expressions that mention `vms(...)`. Extracting mode names requires either:
   - Static rhai-mention analysis (parse `vms(...)` calls, extract mode-name string literals) — fragile, mirrors what `value_fn_invariants::rhai_mentions` does today.
   - Runtime introspection: after render, query the produced ViewModel tree for VMS nodes. More robust but ties Phase 2 to ViewModel availability.
   - Add a new `WideBlockHandle { block_id, mode_names, current_mode }` and a separate capability method `wide_switchable_handles()`, keeping `BlockHandle` GPUI-blueprint-only.
3. **Bridge body.** Populated from whichever data source step 2 picks.
4. **SUT capability**. The shared impl clicks `vms_button_id_for(block_id, target_mode)`. Same `apply_click_at_element` F2 landmine — bridge or override.
5. **Wide-PBT transition file** + enum + Reason variants. ~90 LOC.

### Cost estimate

- Rename `SwitchView`: ~30 LOC across `transitions/switch_view.rs`, dispatch enum, `apply_switch_view` SutHandle method, three SutHandle implementors, Reason references.
- ReferenceState VMS handle population: ~50-150 LOC depending on data-source choice. Rhai static analysis is cheaper but mirrors duplicated logic; runtime introspection is more lines but more correct.
- Bridge body: ~10 LOC.
- Wide-PBT transition file: ~90 LOC.
- SutHandle impl: ~40 LOC × 3 implementors = 120 LOC.

**Total: ~290-380 LOC.** Risk: medium. Naming-collision cleanup is the biggest variable.

### Pre-requisites

- Decide on data source for switchable handles.
- Rename existing `SwitchView` to free the namespace.

### Why not first

- Bigger scope, ambiguous data source, naming-collision cleanup is a separate concern. Better to land `ToggleDrawer` first and revisit `SwitchViewMode` with the F1/F2 patterns under one's belt.

## Candidate C: `DeliverBlockContent` — out of scope for now

The bridge stub at `layout_bridge.rs:48-52` says: "Deferred `live_block` placeholders are a fast-UI test concern (the GPUI layout PBT's `arb_deferred_live_block_scenario`). The integration-tests PBT runs a real backend that always returns real data — no deferred placeholders to deliver."

This is a deliberate architectural separation. `DeliverBlockContent` doesn't apply to the wide PBT and should stay dormant. **No fold.**

## Recommended Sequence

1. Land `ToggleDrawer` next (Phase 2.5 or as an early Phase 5 deliverable). ~200 LOC, low risk.
2. Rename `SwitchView` → `SwitchAppView` as a separate small PR.
3. Land `SwitchViewMode` once the rename is in. ~290-380 LOC, medium risk.
4. Leave `DeliverBlockContent` dormant indefinitely.

After all three folds, **3 of 4 shared `holon-pbt-core` variants would be live in both PBTs.** That's the realistic ceiling for the current shared-variant zoo. Growth beyond that requires net-new shared variants — driven by either Phase 3+ work (which may not produce new ones) or by deliberate "this variant has obvious second-consumer demand" decisions case-by-case.
