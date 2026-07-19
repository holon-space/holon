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

/// Two-section result of a quick-open / content search (`cmd-K` modal).
///
/// The user-facing search modal renders `pages` first (jump-to-page targets)
/// and `content` second (full-text block matches), so the split is carried in
/// the type rather than re-derived by the frontend — the search SQL that knows
/// which rows are `Page`-tagged stays behind the query capability.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickOpenResults {
    /// Page-tagged blocks whose content matched, label = first content line.
    pub pages: Vec<LinkCandidate>,
    /// Non-page blocks whose content matched, label = matched content line.
    pub content: Vec<LinkCandidate>,
}
