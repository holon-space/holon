//! Advice-rule discovery — the entity-profile pattern (ADR 0022 Context).
//!
//! Rules are vault blocks with `source_language = 'holon_advice_rule_yaml'`,
//! discovered by the exact `content_type = 'source'` scan that `get_profiles.sql`
//! uses for entity profiles. This module owns the SQL asset and the parse of a
//! discovered block into a typed rule (or its typed error — carried, not dropped).

use crate::rule::{AdviceRule, AdviceRuleParseError, parse_advice_rule};

/// `source_language` marker that identifies an advice-rule block.
pub const ADVICE_RULE_SOURCE_LANGUAGE: &str = "holon_advice_rule_yaml";

/// The discovery scan (clone of `crates/holon/sql/profiles/get_profiles.sql`).
/// Selects `(id, content)` for every advice-rule source block.
pub const GET_ADVICE_RULES_SQL: &str = include_str!("../sql/get_advice_rules.sql");

/// True if a block's `source_language` marks it an advice rule.
pub fn is_advice_rule_block(source_language: Option<&str>) -> bool {
    source_language == Some(ADVICE_RULE_SOURCE_LANGUAGE)
}

/// A discovered rule keyed by its block id, carrying either the parsed rule or the
/// typed parse error. The error is returned (not logged-and-dropped) so the caller
/// can render it on the rule block (ADR 0022 "rule blocks render their own status").
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredRule {
    pub block_id: String,
    pub result: Result<AdviceRule, AdviceRuleParseError>,
}

/// Parse a discovered `(block_id, yaml_content)` pair into a `DiscoveredRule`.
pub fn parse_discovered_rule(block_id: impl Into<String>, yaml_content: &str) -> DiscoveredRule {
    DiscoveredRule {
        block_id: block_id.into(),
        result: parse_advice_rule(yaml_content),
    }
}
