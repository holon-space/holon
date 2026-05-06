//! Property-Based Testing state machine for Holon E2E tests.
//!
//! Extracted from `tests/general_e2e_pbt.rs` so the state machine can be
//! reused by other test harnesses (e.g. Flutter FFI PBT).

pub mod generators;
pub mod loro_sut;
pub mod loro_sync;
pub mod panic_diag;
pub mod peer_ops;
pub mod phased;
pub mod query;
pub mod query_ast;
pub mod reference_state;
pub mod state_machine;
pub mod sut;
#[cfg(feature = "otel-testing")]
pub mod transition_budgets;
pub mod transition_dispatch;
pub mod transitions;
pub mod types;
pub mod ui_harness;
pub mod ui_interaction;
pub mod validation;
pub mod value_fn_invariants;

pub use phased::{
    PbtPhaseState, PbtReadyContext, PbtReadyResult, PbtStepResult, PbtUiOperation,
    pbt_execute_operation, pbt_setup, pbt_step, pbt_step_confirm, pbt_teardown,
    run_pbt_with_driver_sync_callback, run_phased_pbt,
};
pub use query::{TestQuery, WatchSpec};
pub use reference_state::ReferenceState;
pub use state_machine::VariantRef;
pub use sut::E2ESut;
pub use transitions::E2ETransition;
pub use types::*;
pub use ui_harness::{
    DEFAULT_FRONTEND_MEMORY_MULTIPLIER, enable_atomic_editor_if_unset, screenshot_dir,
    set_loro_peer_id_if_unset, set_memory_multiplier_if_unset, spawn_quit_on_pbt_finish,
    try_start_embedded_mcp, wait_for_geometry_ready,
};
pub use ui_interaction::UiInteraction;
