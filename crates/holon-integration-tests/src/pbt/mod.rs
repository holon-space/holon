//! Property-Based Testing state machine for Holon E2E tests.
//!
//! Extracted from `tests/general_e2e_pbt.rs` so the state machine can be
//! reused by other test harnesses (e.g. Flutter FFI PBT).

pub mod generators;
pub mod phased;
pub mod query;
pub mod reference_state;
pub mod state_machine;
pub mod sut;
pub mod transitions;
pub mod types;

pub use phased::{
    PbtPhaseState, PbtStepResult, PbtUiOperation, pbt_execute_operation, pbt_setup, pbt_step,
    pbt_step_confirm, pbt_teardown, run_pbt_with_driver_sync_callback, run_phased_pbt,
};
pub use query::{TestQuery, WatchSpec};
pub use reference_state::ReferenceState;
pub use state_machine::VariantRef;
pub use sut::E2ESut;
pub use transitions::E2ETransition;
pub use types::*;
