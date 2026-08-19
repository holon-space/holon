//! Synchronization infrastructure that belongs to the engine.
//!
//! The Loro CRDT backend and P2P sync live in `holon-loro`; the wiring that
//! binds them to this crate lives in `holon-loro-wiring`. Consumers name
//! `holon_loro::` directly — this module deliberately re-exports neither, so
//! `holon` never depends on `holon-loro`.

pub mod advice_reconciler;
pub mod clock_scheduler;
pub mod turso_block_query_source;

// Re-export wiring modules that stayed in holon
pub use advice_reconciler::AdviceReconcilerHandle;
pub use advice_reconciler::spawn_advice_reconciler;
// Re-export live_data module and the LiveData type (moved to holon-api earlier)
pub use holon_api::live_data;
pub use holon_api::live_data::LiveData;
pub use holon_turso::matview_manager;
pub use holon_turso::matview_manager::MatviewManager;
pub use holon_turso::matview_manager::WatchResult;
pub use holon_turso::matview_manager::reconcile_named_view;
pub use holon_turso::util::order_by_sort_spec;
pub use holon_turso::util::trailing_order_by;
