//! Typed result row for link-autocomplete search (`[[` popup).
//!
//! Replaces the raw-SQL `popup_query` capability (storage de-leak Stage 2):
//! the search SQL lives behind the query capability; the frontend only sees
//! parsed candidates.

use crate::entity_uri::EntityUri;

/// One entity matching a link-search filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCandidate {
    /// Typed entity id (parsed fail-loud from the storage row).
    pub id: EntityUri,
    /// Human-readable label (first content line for pages, content for blocks).
    pub label: String,
}
