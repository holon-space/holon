//! Shared event vocabulary.
//!
//! The EventBus itself has been decommissioned (blocks flow via the convergent
//! `LiveData<Block>` feed; dir/file and Todoist caches are fed directly from
//! their sync-provider broadcasts). What remains here is the small set of types
//! and param-key constants that are still shared across the write path:
//! [`EventOrigin`] (write provenance, carried on the `_change_origin` CDC
//! column), [`PublishErrorTracker`], and the operation-control param keys.

// The operation-control param keys now live in `holon-api` alongside the
// `StorageEntity` type they belong to (the write-path param contract is shared
// kernel, not Loro-specific). Re-exported here for back-compat with the
// `holon_loro::event_bus::*` consumers.
pub use holon_api::POSITION_AFTER_BLOCK_ID_PARAM;
pub use holon_api::ROUTING_DOC_URI_KEY;
pub use holon_core::EventOrigin;
pub use holon_core::PublishErrorTracker;
