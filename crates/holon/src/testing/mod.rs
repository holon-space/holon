//! Generic testing infrastructure for OperationProvider implementations
//!
//! This module provides property-based testing infrastructure that works
//! for any provider implementing the `OperationProvider` trait.
//!
//! Key components:
//! - `GenericProviderState`: Tracks entity state and generates valid operation
//!   sequences
//! - Integration with `proptest-state-machine` for automatic test generation
//! - `E2ETestContext`: End-to-end testing utilities for BackendEngine

pub mod e2e_test_helpers;
pub mod generic_provider_state;

pub use e2e_test_helpers::ChangeType;
pub use e2e_test_helpers::E2ETestContext;
pub use e2e_test_helpers::assert_change_sequence;
pub use e2e_test_helpers::assert_change_type;
pub use e2e_test_helpers::extract_entity_ids;
pub use e2e_test_helpers::filter_changes_by_entity;
pub use e2e_test_helpers::wait_for_change;
pub use generic_provider_state::GenericProviderState;
