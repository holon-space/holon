//! Property-Based Testing state machine for Holon E2E tests.
//!
//! Extracted from `tests/general_e2e_pbt.rs` so the state machine can be
//! reused by other test harnesses (e.g. Flutter FFI PBT).

pub mod action_actor_state;
pub mod bisect_driver;
pub mod composed;
pub mod driver_input;
pub mod file_adapter_state;
pub mod fixtures;
pub mod frontend_slice;
pub mod generators;
pub mod invariant_mode_override;
pub mod invariants;
pub mod layout_bridge;
pub mod local_caps;
pub mod loro_slice;
pub mod loro_sync;
pub mod mcp_server_actor_state;
pub mod memory_slice;
pub mod op_write_cap;
pub mod panic_diag;
pub mod peer_ops;
pub mod query;
pub mod query_ast;
pub mod reference_capabilities;
pub mod reference_domain_state;
pub mod reference_state;
pub mod retry;
pub mod slice;
pub mod sql_loro_slice;
pub mod sql_slice;
pub mod staleness;
pub mod state_machine;
pub mod stepper;
pub mod sut;
pub mod sut_capabilities;
mod sut_check_invariants;
mod sut_handle;
mod sut_keybindings;
pub mod sut_loro;
mod sut_metrics;
mod sut_render;
mod sut_row_parsing;
#[cfg(feature = "otel-testing")]
pub mod transition_budgets;
pub mod transition_dispatch;
pub mod transitions;
pub mod types;
pub mod ui_actor_state;
pub mod ui_harness;
pub mod ui_interaction;
pub mod validation;
pub mod value_fn_invariants;
pub mod window_slice;

/// Whether `id` is a ref-side SYNTHETIC placeholder the SUT replaces with a
/// real UUID. Only split suffixes are: `reference_state.rs::split_block`
/// mints `block::split-N` (note the double colon) and the SUT later binds it
/// to a freshly-minted UUID. Bulk ids (`block:bulk-N-i`) are NOT synthetic —
/// `bulk_external_add` writes them deterministically on BOTH sides, so they
/// appear verbatim in `block_raw`/CDC. Single source of truth — substring
/// sniffs like `contains(":split-")` false-positived on legitimately named
/// blocks (e.g. the gherkin fixture's `block:split-target-block`).
pub fn is_synthetic_ref_id(id: &holon_api::EntityUri) -> bool {
    id.as_str().starts_with("block::split-")
}

pub use action_actor_state::ActionActorState;
pub use file_adapter_state::FileAdapterState;
pub use mcp_server_actor_state::MCPServerActorState;
// The phased entry points (the ready-context struct + the driver-sync run/replay
// functions) are deliberately NOT re-exported
// (increment 4c): the windowed harnesses ride the composed path
// (`with_windowed_wide_sut` + `replay_steps` over `ComposedSut<WideE2E>`), and
// `phased.rs` itself is deletion-scheduled (Phase 2). Nothing outside `phased.rs`
// may depend on them.
pub use query::{TestQuery, WatchSpec};
pub use reference_domain_state::ReferenceDomainState;
pub use reference_state::ReferenceState;
pub use state_machine::{ReferenceMachine, fresh_reference_state, storage_selector_for_wiring};
pub use sut::E2ESut;
pub use transitions::E2ETransition;
pub use types::*;
pub use ui_actor_state::{UIActorState, UITabState, UIUserState};
pub use ui_harness::{
    DEFAULT_FRONTEND_MEMORY_MULTIPLIER, install_rejection_histogram_panic_hook, screenshot_dir,
    set_loro_peer_id_if_unset, set_memory_multiplier_if_unset, spawn_quit_on_pbt_finish,
    standard_pbt_config, try_start_embedded_mcp, wait_for_geometry_ready,
};
pub use ui_interaction::UiInteraction;
