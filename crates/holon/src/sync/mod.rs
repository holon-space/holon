//! Synchronization infrastructure — re-exported from `holon-loro`.
//!
//! The Loro CRDT backend, P2P sync, snapshot helpers, and block operation
//! providers now live in `holon-loro`. This module re-exports them for
//! backward compatibility (`holon::sync::*` paths keep resolving).
//!
//! Three wiring modules stay in `holon` because they bridge the `holon::core`
//! SQL types into the Loro pipeline (see Phase D partition).

pub mod event_infra_module;
pub mod loro_block_query_source;
pub mod loro_module;
pub mod turso_block_query_source;

pub use holon_loro::*;
pub use holon_turso::matview_manager;
pub use holon_turso::matview_manager::{MatviewHook, MatviewManager, WatchResult, reconcile_named_view};

// Re-export live_data module and the LiveData type (moved to holon-api earlier)
pub use holon_api::live_data;
pub use holon_api::live_data::LiveData;

// Re-export wiring modules that stayed in holon
pub use loro_module::{LoroConfig, LoroModule};
pub use event_infra_module::{EventInfraModule, LinkEventSubscriberHandle};
