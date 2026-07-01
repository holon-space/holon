//! Builders for the windowed slice — the per-slice cost is just the component
//! list, as everywhere else (§1).

use std::sync::Arc;

use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_pbt_core::composition::{CapMap, CapProvider, Config};
use holon_pbt_core::{Actor, ComponentSet};

use super::components::{GpuiFrontendEngineComponent, GpuiWindowComponent};
use crate::pbt::composed::seed_primitives::{Plant, apply_plant, seed_ref_tree};
use crate::pbt::driver_input::DriverInputComponent;
use crate::pbt::reference_capabilities::reference_state_ref_caps;
use crate::pbt::reference_state::Resolved;
use crate::pbt::state_machine::fresh_reference_state;

/// Build a composed SUT hosting only windowed [`SutLayout`] geometry over
/// `geometry` (a live window's `BoundsRegistry` clone). Use [`window_wide`] when
/// the geometry **and** the ViewModel of the same window are both needed (the
/// `inv-frontend-bounds-rendered` / `inv-displayed-text` family).
///
/// [`SutLayout`]: holon_pbt_core::capabilities::SutLayout
pub fn window_layout(geometry: Box<dyn GeometryProvider>) -> CapMap {
    Config::new()
        .with(GpuiWindowComponent::new(geometry))
        .build()
}

/// The full windowed slice: real geometry ([`SutLayout`]) **and** the ViewModel /
/// renderer ([`SutViewModel`] + [`SutRenderer`]) of the **same** live window —
/// `geometry` is its `BoundsRegistry` clone, `engine` is the frontend
/// [`ReactiveEngine`] it renders from. This is the composition that lets the
/// windowed registry invariants (`inv-frontend-bounds-rendered`,
/// `inv-displayed-text/widget` + `/viewmodel`) run via `run_selected` — the
/// geometry and the VM it is compared against come from one render pipeline.
///
/// [`SutLayout`]: holon_pbt_core::capabilities::SutLayout
/// [`SutViewModel`]: holon_pbt_core::capabilities::SutViewModel
/// [`SutRenderer`]: holon_pbt_core::capabilities::SutRenderer
pub fn window_wide(geometry: Box<dyn GeometryProvider>, engine: Arc<ReactiveEngine>) -> CapMap {
    Config::new()
        .with(GpuiWindowComponent::new(geometry))
        .with(GpuiFrontendEngineComponent::new(engine))
        .build()
}

/// [`window_wide`] plus the windowed [`SutDriver`] over the **same** engine — the
/// full windowed SUT that selects `inv-window-focus-matches-engine-focus` (E4
/// increment 4). `SutDriver::engine_focused_block` reads `engine`'s focus; the
/// invariant compares it against the window focus `SutLayout` reports from
/// `geometry`. Both come from one window, so a settled engine/window focus
/// divergence (steal-back / zombie editor, ADR 0010) is what makes it fail.
///
/// [`SutDriver`]: holon_pbt_core::capabilities::SutDriver
pub fn window_focus_wide(
    geometry: Box<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
) -> CapMap {
    Config::new()
        .with(GpuiWindowComponent::new(geometry))
        .with(GpuiFrontendEngineComponent::new(engine.clone()))
        .with(DriverInputComponent::new(engine))
        .build()
}

/// E4 — the full windowed **input** SUT: [`window_focus_wide`]'s read caps
/// (`SutLayout` + `SutViewModel` + `SutRenderer` + `SutDriver`) **plus** the
/// windowed input caps `SutBlockInteract` + `SutArrowNavigate`, driven through the
/// live window's production `UserDriver` (`driver`). Because its `cap_set()` now
/// carries the input caps, the value-level cap gate in
/// `transition_dispatch::aggregate_transitions` stops narrowing `PressKey` /
/// `ArrowNavigate` / `DragDropBlock` / `ClickBlock` out of the alphabet on the
/// composed windowed path — the input transitions become cap-admitted.
///
/// `DriverInputComponent::with_input` wraps `driver` (it never re-implements input)
/// and reads `geometry` (a `clone_box` of the same `BoundsRegistry` the
/// `GpuiWindowComponent` reads) for the single-shot bounds precheck.
pub fn window_input_wide(
    geometry: Box<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    driver: Arc<dyn UserDriver>,
) -> CapMap {
    let driver_geometry = geometry.clone_box();
    Config::new()
        .with(GpuiWindowComponent::new(geometry))
        .with(GpuiFrontendEngineComponent::new(engine.clone()))
        .with(DriverInputComponent::with_input(
            engine,
            driver,
            driver_geometry,
        ))
        .build()
}

/// The `Actor::UI` sibling of [`compose_sut`](crate::pbt::composed::compose_sut):
/// build the windowed composed `CapMap` from a **live window's** handles,
/// validated against a UI-bearing [`ComponentSet`] (the canonical one is
/// [`ComponentSet::full_gpui`]).
///
/// `compose_sut` deliberately **cannot** build this: it boots its components on
/// the tokio runtime, but a GPUI window has thread affinity — it must be
/// launched on the gpui thread by the windowed harness, which then hands its
/// `geometry` / `engine` / production `UserDriver` here. So the two construction
/// paths are siblings, not one entry (`compose_sut` asserts `!has_actor(UI)` and
/// points here). Both yield a `ComponentSet`-described `CapMap` that runs the one
/// shared catalog via `run_selected` — one SUT *shape*, two harnesses.
///
/// Fail-loud: panics if `set` isn't a real windowed set (no `Actor::UI`, or
/// invalid — `Actor::UI` requires the `ViewModel` projection).
pub fn compose_windowed_sut(
    set: &ComponentSet,
    geometry: Box<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    driver: Arc<dyn UserDriver>,
) -> CapMap {
    assert!(
        set.has_actor(Actor::UI),
        "compose_windowed_sut needs a windowed set (Actor::UI present); got {set:?}. \
         Headless sets go through `compose_sut`."
    );
    set.validate().unwrap_or_else(|e| {
        panic!("compose_windowed_sut: invalid windowed set {set:?}: {e}");
    });
    window_input_wide(geometry, engine, driver)
}

/// ★ Round-5 windowed repoint: overlay the windowed SHELL caps onto an already-composed
/// HEADLESS CapMap (from `compose_sut(ComponentSet::full_headless())`). The window is a
/// PURE RENDERER over the same `engine` the headless caps read, so this:
/// - ADDS windowed [`SutLayout`] geometry ([`GpuiWindowComponent`] over the live window's
///   `BoundsRegistry` clone) — the headless CapMap has none; and
/// - REPLACES (by cap `TypeId`, via `CapMap::insert`) the headless driver-input caps
///   (`SutDriver` / `SutBlockInteract` / `SutArrowNavigate`) with the live window's
///   `GpuiUserDriver`-backed ones ([`DriverInputComponent::with_input`]), so gestures drive
///   the HIGHEST-available rung (§8.11 faithfulness), not the headless `ReactiveEngineDriver`.
///
/// The result runs the FULL composed catalog: the block/storage/editor families — with the
/// `compose_sut` `IdResolver` reconcile intact, so id-resolution comes FREE — PLUS the
/// windowed families (bounds-rendered / displayed-text / window-focus). This is why the
/// windowed repoint needs no separate booter or new reconcile: the headless boot IS the
/// booter and the window merely renders it.
///
/// `geometry` is a `BoundsRegistry` clone from the launched window; `engine` is the SAME
/// frontend [`ReactiveEngine`] the window paints and the headless caps were built over
/// (surface it via `ComposedSut::reactive`); `driver` is the window's `GpuiUserDriver`.
///
/// [`SutLayout`]: holon_pbt_core::capabilities::SutLayout
pub fn overlay_windowed_caps(
    mut caps: CapMap,
    geometry: Box<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    driver: Arc<dyn UserDriver>,
) -> CapMap {
    let driver_geometry = geometry.clone_box();
    Arc::new(GpuiWindowComponent::new(geometry)).register(&mut caps);
    Arc::new(DriverInputComponent::with_input(
        engine,
        driver,
        driver_geometry,
    ))
    .register(&mut caps);
    caps
}

/// Like [`window_focus_wide`] but with the windowed `SutDriver`'s
/// `engine_focused_block` **forced** to `forced_focus` instead of reading the live
/// engine — the planted negative control. Pair a window that genuinely focuses
/// block X with `forced_focus = Focused(Y)` (Y ≠ X) and
/// `inv-window-focus-matches-engine-focus` must **fail**: the engine claims focus on
/// Y while the window shows X focused (steal-back). This is the focus-axis analogue
/// of [`window_ref_caps_planted`]'s content plant — it proves the windowed focus
/// oracle genuinely compares the two authorities rather than passing vacuously.
pub fn window_focus_wide_planted(
    geometry: Box<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    forced_focus: holon_pbt_core::capabilities::EngineFocus,
) -> CapMap {
    Config::new()
        .with(GpuiWindowComponent::new(geometry))
        .with(GpuiFrontendEngineComponent::new(engine.clone()))
        .with(DriverInputComponent::with_forced_engine_focus(
            engine,
            forced_focus,
        ))
        .build()
}

/// The reference `CapMap` for the windowed slice: a minimal, honestly-empty
/// [`ReferenceState`] oracle carrying `RefLayout` / `RefBlockTree` /
/// `RefEditorMirror`. It deliberately knows none of the booted vault's
/// (random-UUID) blocks, so the `inv-displayed-text` family skips them (an
/// unknown block is skipped, not failed) and `inv-frontend-bounds-rendered`'s
/// document-gated content checks are disclosed-off — leaving its **strict
/// geometry checks** (expected-size, no-error-widgets, VM y-order/contiguity) to
/// run over the window's real geometry. A vault-matching oracle is the next
/// increment's `StateMachineTest` concern, not this one's.
///
/// [`ReferenceState`]: crate::pbt::ReferenceState
pub fn window_ref_caps() -> CapMap {
    let ref_state = fresh_reference_state(holon_pbt_core::Wiring::full());
    reference_state_ref_caps(Resolved::identity(ref_state).map(Arc::new))
}

/// Increment 3a reference `CapMap`: the same oracle as [`window_ref_caps`] but
/// **seeded with the fixed shared `parent`/`c1`/`c2` tree** ([`seed_ref_tree`]),
/// so the ref *knows* the content of exactly the blocks the test grafts into the
/// window's backend ([`super::seed::graft_displayed_text_tree`]). With the ids
/// shared (the [`fixed_ids`](crate::pbt::composed::subsystem_seed::fixed_ids)
/// keystone), `inv-displayed-text` matches the rendered widgets for those blocks
/// by id and **bites** — the vault's random-UUID blocks remain unknown and are
/// skipped. This is what makes the windowed text oracle non-vacuous.
pub fn window_ref_caps_seeded() -> CapMap {
    let mut ref_state = fresh_reference_state(holon_pbt_core::Wiring::full());
    seed_ref_tree(&mut ref_state);
    reference_state_ref_caps(Resolved::identity(ref_state).map(Arc::new))
}

/// Like [`window_ref_caps_seeded`] but with a planted **content** divergence on
/// `c1` ([`Plant::Content`] ⇒ the ref says `c1-WRONG`). Paired with an unplanted
/// grafted SUT tree, `inv-displayed-text` must **fail** on `c1` — the negative
/// control proving the windowed oracle genuinely compares rendered text, not
/// merely that it ran.
pub fn window_ref_caps_planted() -> CapMap {
    let mut ref_state = fresh_reference_state(holon_pbt_core::Wiring::full());
    seed_ref_tree(&mut ref_state);
    apply_plant(&mut ref_state, Plant::Content);
    reference_state_ref_caps(Resolved::identity(ref_state).map(Arc::new))
}
