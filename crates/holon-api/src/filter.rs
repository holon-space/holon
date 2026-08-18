//! A bookmarkable filter as data (FLT-1.b).
//!
//! A filter is a `holon_filter` SOURCE block whose body is a predicate in the
//! render-DSL predicate grammar and whose containing headline names it. Its
//! `:polarity` / `:subtree` header args say what the predicate does to the
//! rows it matches. [`FilterSpec::parse`] resolves the headline block plus its
//! filter-source child into a typed spec, failing loud on a malformed body or
//! an unrecognized header value — never a default that hides the mismatch.

use std::str::FromStr;

use crate::Value;
use crate::block::Block;
use crate::entity_uri::EntityUri;
use crate::predicate::Predicate;
use crate::render_dsl::parse_predicate;
use crate::types::SourceLanguage;

/// What the filter does to the rows its predicate matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterPolarity {
    /// Matched rows are hidden.
    Hide,
    /// Only matched rows are kept; everything else is hidden.
    Keep,
}

impl FromStr for FilterPolarity {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "hide" => Ok(FilterPolarity::Hide),
            "keep" => Ok(FilterPolarity::Keep),
            other => {
                anyhow::bail!("unknown filter polarity {other:?} (expected \"hide\" or \"keep\")")
            }
        }
    }
}

/// How far a match reaches into the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtreeRule {
    /// The match applies to the matched block alone.
    JustBlock,
    /// The match carries to the block and its whole subtree.
    WithDescendants,
}

impl FromStr for SubtreeRule {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "just-block" => Ok(SubtreeRule::JustBlock),
            "with-descendants" => Ok(SubtreeRule::WithDescendants),
            other => anyhow::bail!(
                "unknown filter subtree rule {other:?} (expected \"just-block\" or \
                 \"with-descendants\")"
            ),
        }
    }
}

/// A filter resolved from a headline block and its `holon_filter` source child.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterSpec {
    /// The containing headline block's id.
    pub id: EntityUri,
    /// Human-readable name — the headline block's content (FLT-1.b).
    pub name: String,
    /// The predicate that selects rows.
    pub predicate: Predicate,
    /// What matched rows get (default [`FilterPolarity::Hide`]).
    pub polarity: FilterPolarity,
    /// How far a match reaches (default [`SubtreeRule::JustBlock`]).
    pub subtree: SubtreeRule,
}

impl FilterSpec {
    /// Resolve the filter named by the headline `head_id` out of `blocks`.
    ///
    /// `blocks` is any collection holding the headline block and its
    /// `holon_filter` source child (e.g. a whole document's block set). Fails
    /// loud when the headline is missing, when it has no (or several)
    /// `holon_filter` source children, when the predicate body is malformed, or
    /// when a header value is unrecognized.
    pub fn parse(head_id: &EntityUri, blocks: &[Block]) -> anyhow::Result<Self> {
        let head = blocks
            .iter()
            .find(|b| &b.id == head_id)
            .ok_or_else(|| anyhow::anyhow!("filter headline {head_id} not found in block set"))?;

        let mut sources = blocks.iter().filter(|b| {
            &b.parent_id == head_id && b.source_language == Some(SourceLanguage::HolonFilter)
        });
        let src = sources.next().ok_or_else(|| {
            anyhow::anyhow!(
                "filter {head_id} ({:?}): no holon_filter source child — a filter's predicate \
                 lives in a `holon_filter` source block under its headline",
                head.content
            )
        })?;
        if let Some(extra) = sources.next() {
            anyhow::bail!(
                "filter {head_id} ({:?}): several holon_filter source children ({} and {}); a \
                 filter has exactly one predicate body",
                head.content,
                src.id,
                extra.id
            );
        }

        // A filter's NAME is its headline title (FLT-1.b); an unnamed filter is
        // malformed — fail loud rather than mint a nameless spec.
        let name = head.content.clone();
        if name.trim().is_empty() {
            anyhow::bail!(
                "filter {head_id}: headline title is empty — a filter's name IS its title"
            );
        }

        let predicate = parse_predicate(&src.content).map_err(|e| {
            anyhow::anyhow!(
                "filter {head_id}: malformed predicate body {:?}: {e:#}",
                src.content
            )
        })?;

        // `polarity`/`subtree` are string header args. A non-string Value is a
        // malformed filter — name it and fail loud, never silently default.
        let polarity = match src.properties.get("polarity") {
            Some(Value::String(raw)) => FilterPolarity::from_str(raw)
                .map_err(|e| anyhow::anyhow!("filter {head_id}: {e}"))?,
            Some(other) => {
                anyhow::bail!("filter {head_id}: polarity must be a string, got {other:?}")
            }
            None => FilterPolarity::Hide,
        };
        let subtree = match src.properties.get("subtree") {
            Some(Value::String(raw)) => {
                SubtreeRule::from_str(raw).map_err(|e| anyhow::anyhow!("filter {head_id}: {e}"))?
            }
            Some(other) => {
                anyhow::bail!("filter {head_id}: subtree must be a string, got {other:?}")
            }
            None => SubtreeRule::JustBlock,
        };

        Ok(FilterSpec {
            id: head_id.clone(),
            name,
            predicate,
            polarity,
            subtree,
        })
    }
}
