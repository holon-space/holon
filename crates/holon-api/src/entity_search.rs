//! Generic search over every type that declares `services.searchable`.

use anyhow::Result;
use async_trait::async_trait;

use crate::entity_uri::EntityUri;

/// One entity whose searchable fields contain the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: EntityUri,
    /// The search group the entity belongs to, `None` outside every group.
    pub group: Option<String>,
    /// The first line of the type's title field.
    pub label: String,
    /// Each line of the matched field that contains the query, the label's
    /// own line excepted.
    pub snippet: Vec<String>,
}

/// Search text, matched case-insensitively as a literal substring. Never
/// empty: the empty string is a substring of every entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchText<'a>(&'a str);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the search text is empty, and the empty text matches every entity")]
pub struct EmptySearchText;

impl<'a> SearchText<'a> {
    pub fn new(text: &'a str) -> Result<Self, EmptySearchText> {
        if text.is_empty() {
            return Err(EmptySearchText);
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SearchQuery<'a> {
    pub text: SearchText<'a>,
    /// Only types declaring `services.linkable`.
    pub linkable_only: bool,
    /// Hits per type outside every group.
    pub limit: usize,
    /// Hits per type inside each named group. Every group a searched type
    /// declares must be listed.
    pub group_limits: &'a [(&'a str, usize)],
}

#[async_trait]
pub trait EntitySearch: Send + Sync {
    /// Ranked hits: per type and group, prefix matches on the title first,
    /// then shorter titles.
    async fn search(&self, query: SearchQuery<'_>) -> Result<Vec<SearchHit>>;
}
