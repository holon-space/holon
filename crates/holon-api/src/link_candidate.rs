//! Typed result row for link-autocomplete search (`[[` popup).
//!
//! Replaces the raw-SQL `popup_query` capability (storage de-leak Stage 2):
//! the search SQL lives behind the query capability; the frontend only sees
//! parsed candidates.

use crate::entity_search::SearchHit;
use crate::entity_uri::EntityUri;

/// One entity matching a link-search filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCandidate {
    /// Typed entity id (parsed fail-loud from the storage row).
    pub id: EntityUri,
    /// The first line of the entity's title field.
    pub label: String,
}

/// Two-section result of a quick-open search (`cmd-K` modal): the modal
/// renders `pages` first (jump-to-page targets), then `content`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickOpenResults {
    /// Hits in the `Page` search group.
    pub pages: Vec<SearchHit>,
    /// Every other hit, of any searchable type.
    pub content: Vec<SearchHit>,
}
