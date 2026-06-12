//! Shared event vocabulary.
//!
//! The EventBus itself has been decommissioned (blocks flow via the convergent
//! `LiveData<Block>` feed; dir/file and Todoist caches are fed directly from
//! their sync-provider broadcasts). What remains here is the small set of types
//! and param-key constants that are still shared across the write path:
//! [`EventOrigin`] (write provenance, carried on the `_change_origin` CDC
//! column), [`PublishErrorTracker`], and the operation-control param keys.

pub use holon_core::EventOrigin;
pub use holon_core::PublishErrorTracker;

/// Param-side name for the document-routing hint — names the document URI that
/// owns the block this op applies to. Producers (`build_block_params` in
/// `holon-orgmode`, plus internal lookups in `SqlOperationProvider`) add this to
/// the params HashMap they hand to a create/update/delete op. The leading
/// underscore lets `SqlOperationProvider::partition_params` recognise it as
/// operation-control metadata (via the `_routing_` prefix) and keep it out of
/// the persisted row.
// ALLOW(routing_payload_key): producer-side param const, see doc-comment above.
pub const ROUTING_DOC_URI_KEY: &str = "_routing_doc_uri";

/// Param-side name for the typed positional intent — names the predecessor
/// sibling a freshly-created block should sit after. Producers
/// (`BlockOperations::split_block`, `FileSyncController::on_file_changed`) add
/// this key to the params HashMap they hand to a create op; `SqlOperationProvider`
/// reads it (to drive sibling ordering) and drops it from the persisted row.
pub const POSITION_AFTER_BLOCK_ID_PARAM: &str = "after_block_id";
