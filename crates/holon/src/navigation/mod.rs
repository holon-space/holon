//! Navigation module for backend-driven navigation state
//!
//! This module provides navigation operations that work through Turso's IVM (Incremental View Maintenance).
//! Navigation state is stored in database tables, and queries JOIN against `current_focus` view
//! to automatically update UI when navigation changes.
//!
//! ## Key concepts:
//! - `navigation_history`: Stores navigation history for back/forward
//! - `navigation_cursor`: Points to current position in history per region
//! - `current_focus`: View that JOINs cursor to history for easy querying
//!
//! ## Operations:
//! - `focus(region, block_id)`: Navigate to view a block and its children
//! - `go_back(region)`: Navigate to previous view in history
//! - `go_forward(region)`: Navigate to next view in history
//! - `go_home(region)`: Return to root view (clear focus)
//!
//! ## Schema Management
//!
//! Navigation schema is managed by `NavigationSchemaModule` via the `SchemaRegistry`.
//! See `storage/schema_modules.rs` for the schema definition.

mod in_memory_provider;
mod provider;

pub use in_memory_provider::InMemoryNavigationProvider;
pub use provider::{NavigationProvider, navigation_operation_descriptors};

#[cfg(test)]
mod loro_exclusion_test {
    /// Navigation tables (`navigation_history`, `navigation_cursor`) are
    /// intentionally local-only — pinned tabs and back/forward history are
    /// per-device user expectations; cross-device sync would surface
    /// phantom pins on other machines.
    ///
    /// This regression test asserts no source file in the Loro replication
    /// path references those table names, so a future contributor can't
    /// accidentally enable replication without the test failing first.
    #[test]
    fn loro_paths_do_not_reference_navigation_tables() {
        const FORBIDDEN: &[&str] = &["navigation_history", "navigation_cursor"];
        // include_str! is compile-time relative to this file. Each path
        // hits a distinct Loro touchpoint: the backend (writes) and the
        // sync controller (orchestrates inbound/outbound).
        let sources: &[(&str, &str)] = &[
            ("loro_backend.rs", include_str!("../../../holon-loro/src/loro_backend.rs")),
            (
                "loro_sync_controller.rs",
                include_str!("../../../holon-loro/src/loro_sync_controller.rs"),
            ),
        ];
        for (file, src) in sources {
            for needle in FORBIDDEN {
                assert!(
                    !src.contains(needle),
                    "Loro source `{file}` references local-only nav table `{needle}` — \
                     navigation state is per-device by design (pinned tabs, back/forward \
                     history). If you intended to replicate, add the table to the proper \
                     replicated-tables list and remove it from this test's FORBIDDEN list \
                     after a deliberate decision."
                );
            }
        }
    }
}
