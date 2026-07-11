//! Property-Based Testing state machine for Holon E2E tests.
//!
//! Extracted from `tests/general_e2e_pbt.rs` so the state machine can be
//! reused by other test harnesses (e.g. Flutter FFI PBT).

pub mod action_actor_state;
pub mod advice_expectation;
pub mod bisect_driver;
pub mod block_state;
pub mod clock_state;
pub mod composed;
pub mod convergence;
pub mod driver_input;
pub mod file_adapter_state;
pub mod fixtures;
pub mod frontend_slice;
pub mod generators;
pub mod invariant_mode_override;
pub mod invariants;
pub mod layout_bridge;
pub mod loro_slice;
pub mod loro_sync;
pub mod mcp_server_actor_state;
pub mod memory_slice;
pub mod op_write_cap;
pub mod panic_diag;
pub mod peer_ref_state;
// `peer_ops` co-located to `holon-loro-testing` (Phase-1a Step 4); re-exported
// here so central `crate::pbt::peer_ops::*` / `super::peer_ops::*` call sites
// (reference_state, state_machine, shadow_mesh, …) keep resolving unchanged.
pub use holon_loro_testing::peer_ops;
pub mod query;
pub mod query_ast;
pub mod ref_caps;
// `reference_capabilities` split into `ref_caps/` (RefStateSplit Increment 2,
// docs/Plans/RefStateSplit-2026-07-12.md §3); this alias keeps the existing
// `crate::pbt::reference_capabilities::reference_state_ref_caps` call sites
// resolving unchanged.
pub use ref_caps as reference_capabilities;
pub mod reference_domain_state;
pub mod reference_state;
pub mod shadow_mesh;
pub mod sql_loro_slice;
pub mod sql_slice;
pub mod state_machine;
pub mod stepper;
// `sut_loro` (the `LoroSut`) co-located to `holon-loro-testing`; re-exported so
// central `crate::pbt::sut_loro::LoroSut` (composed::builder) resolves
// unchanged.
pub use holon_loro_testing::sut_loro;
mod sut_metrics;
mod sut_row_parsing;
#[cfg(feature = "otel-testing")]
pub mod transition_budgets;
pub mod transition_dispatch;
pub mod transitions;
pub mod types;
pub mod ui_actor_state;
pub mod ui_harness;
pub mod ui_interaction;
pub mod ui_types;
pub mod value_fn_invariants;
pub mod vm_snapshot;
pub mod window_slice;

pub use action_actor_state::ActionActorState;
pub use file_adapter_state::FileAdapterState;
/// Whether `id` is a ref-side SYNTHETIC placeholder the SUT replaces with a
/// real UUID. Single source of truth lifted to the pbt-core floor (co-location
/// Phase 2) so co-located store arms (`holon-turso-testing`) can filter
/// synthetic ids without reaching into this crate; re-exported here for the
/// many central call sites (`test_environment`, `frontend_slice`, `harness`,
/// `builder`).
pub use holon_pbt_core::observables::is_synthetic_ref_id;
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
pub use state_machine::ReferenceMachine;
pub use state_machine::fresh_reference_state;
pub use state_machine::storage_selector_for_wiring;
pub use transitions::E2ETransition;
pub use types::*;
pub use ui_actor_state::UIActorState;
pub use ui_actor_state::UITabState;
pub use ui_actor_state::UIUserState;
pub use ui_harness::DEFAULT_FRONTEND_MEMORY_MULTIPLIER;
pub use ui_harness::install_rejection_histogram_panic_hook;
pub use ui_harness::screenshot_dir;
pub use ui_harness::set_loro_peer_id_if_unset;
pub use ui_harness::set_memory_multiplier_if_unset;
pub use ui_harness::spawn_quit_on_pbt_finish;
pub use ui_harness::standard_pbt_config;
pub use ui_harness::try_start_embedded_mcp;
pub use ui_harness::wait_for_geometry_ready;
pub use ui_interaction::UiInteraction;
