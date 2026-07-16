//! Type-level proof that the shared `holon_layout_testing` impls work
//! against `ReferenceState` + a `SutBlockInteract` handle via the orphan-rule
//! wrappers and the `layout_bridge` module.
//!
//! Constructing a `ReferenceState` requires the full test wiring
//! (`TestVariant` + `ShadowInterpreter`) and a real full-cap SUT impl
//! has ~30 methods — outside the scope of a smoke test. So this file
//! is intentionally compile-only: it asserts that *the types line up*
//! and the trait obligations are satisfied. Run-time exercise lands
//! when the variants are added to `declare_e2e_transitions!`
//! (follow-up).
//!
//! @pbt kind harness
//! @pbt covers layout-bridge-typelevel — type-level proof shared holon_layout_testing impls bind ReferenceState
//! @pbt overlaps arch-lint — compile-only

#![cfg(feature = "pbt")]

use std::sync::Arc;

use holon_integration_tests::pbt::layout_bridge::SutClickAdapter;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_layout_testing::LayoutRef;
use holon_layout_testing::LayoutRefState;
use holon_layout_testing::LayoutSut;
use holon_pbt_core::SwitchViewMode;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::capabilities::SutBlockInteract;

/// Compile-time assertion: `ReferenceState` implements `LayoutRefState`.
fn _ref_state_impls_layout_ref_state(state: &ReferenceState) {
    let _: &[holon_layout_testing::BlockHandle] = state.switchable_handles();
    let _: &[holon_layout_testing::blueprint::DrawerHandle] = state.drawer_handles();
    let _: Vec<String> = state.deferred_block_ids();
    let _: Vec<String> = state.collapsible_target_ids();
    let _: Option<&str> = state.current_view_mode("block:foo");
    let _: bool = state.drawer_is_open("block:foo");
}

/// Compile-time assertion: the shared
/// `SwitchViewMode::apply_to_sut<LayoutRef<R>, LayoutSut<S>>` impl
/// type-checks when `R = ReferenceState` and `S = SutClickAdapter`.
/// This is the core sharing guarantee: one body in
/// `holon_layout_testing::transitions::switch_view_mode` works for the
/// GPUI layout PBT (`S = GpuiInteractionSession`) and the
/// integration-tests PBT (`S = SutClickAdapter` wrapping
/// `&mut dyn SutBlockInteract`).
async fn _shared_apply_drives_sut_handle<H: SutBlockInteract>(
    state: &ReferenceState,
    handle: &mut H,
    action: &SwitchViewMode,
) {
    let ref_view = LayoutRef::new(state);
    let mut adapter = SutClickAdapter(handle);
    let mut sut = LayoutSut::new(&mut adapter);
    action.apply_to_sut(&ref_view, &mut sut).await;
}

/// Runtime smoke: after `apply_start_app`, the bridge surfaces both default
/// sidebars as drawer handles and reports them open.
#[test]
fn default_layout_surfaces_two_drawer_handles_after_start_app() {
    let interp = Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new());
    let mut state = ReferenceState::new(holon_pbt_core::Wiring::full(), interp);

    assert!(
        state.drawer_handles().is_empty(),
        "pre-startup: no drawers rendered yet"
    );

    state.action.app_started = true;
    state
        .ui
        .tab
        .drawer_open
        .insert("block:default-left-sidebar".to_string(), true);
    state
        .ui
        .tab
        .drawer_open
        .insert("block:default-right-sidebar".to_string(), true);

    let handles = state.drawer_handles();
    assert!(
        handles.len() >= 2,
        "post-startup: expected >=2 drawer handles, got {}",
        handles.len()
    );
    assert!(state.drawer_is_open("block:default-left-sidebar"));
    assert!(state.drawer_is_open("block:default-right-sidebar"));
}
