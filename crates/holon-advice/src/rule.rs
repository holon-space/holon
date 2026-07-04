//! Typed advice-rule model + parse boundary (ADR 0022).
//!
//! An advice rule is a vault block (`source_language = 'holon_advice_rule_yaml'`)
//! parsed at the boundary into a closed, typed representation. Illegal states are
//! refused at parse — never carried forward as strings to be re-validated. The
//! typed `AnchorSelector` lowers to a SQL `WHERE` fragment (injection-safe by
//! refusal, not escaping), which keeps the anchor set IVM-computable.

use holon_api::EntityName;
use serde::Deserialize;
use thiserror::Error;

/// A parsed, validated advice rule. Every field is a type that encodes its
/// invariant; constructing an `AdviceRule` is proof the YAML was legal.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdviceRule {
    /// Stable slug → matview name `advice_rule_{slug}`.
    pub name: RuleSlug,
    /// SQL-lowerable typed anchor predicate (which blocks get advice woven under them).
    pub anchor: AnchorSelector,
    /// Closed scoring template (how candidate rows are selected + ranked).
    pub candidates: ScoringTemplate,
    /// Top-K applied at read time — the final display cap (the matview is
    /// anchor-denormalized, un-capped). Parse refuses `k > n`.
    pub k: BoundedK,
    /// Retrieval-stage `LIMIT N` (ADR 0023 two-stage relevance): the
    /// recall-oriented candidate width the matview read pulls *before* the
    /// app-layer reranker narrows to the final top-K. Absent → `N = K`
    /// (retrieval degenerates to final selection — the correct pre-reranker
    /// shape). Read via [`AdviceRule::n`], never the raw field.
    #[serde(default)]
    n: Option<BoundedN>,
    /// Explicit activation flag — gates matview DDL reconcile (ADR 0022 sub-decision 1).
    pub active: bool,
    /// RESERVED versioned raw-query escape hatch (ADR 0022 sub-decision 3). The
    /// schema reserves it day one; v1 parse REFUSES any rule that sets it. Kept as
    /// a field (never read) so `deny_unknown_fields` routes a set value into
    /// `Reserved`'s always-failing deserializer rather than an "unknown field".
    #[serde(default)]
    #[allow(dead_code)]
    raw_query: Option<Reserved>,
    /// RESERVED reranker-profile selector (ADR 0023 stage 2). Names a reranker
    /// model *profile* only — the endpoint, API key, and consent that back a
    /// profile are device-local prefs that are NEVER synced (they must not ride
    /// in a vault block). Unimplemented until Increment H; v1 parse REFUSES any
    /// rule that sets it. Kept as a field (never read) so `deny_unknown_fields`
    /// routes a set value into `RerankReserved`'s always-failing deserializer
    /// rather than an "unknown field". `rerank: null` (or absent) is allowed.
    #[serde(default)]
    #[allow(dead_code)]
    rerank: Option<RerankReserved>,
}

impl AdviceRule {
    /// The resolved retrieval width. `N = K` when `n` was absent — retrieval
    /// degenerates to the final selection (the correct pre-reranker shape,
    /// ADR 0023). Parse has already guaranteed `k <= n`.
    pub fn n(&self) -> BoundedN {
        self.n.unwrap_or(BoundedN(self.k.get()))
    }
}

/// A stable rule slug. Constrained to a SQL-identifier-safe charset because it is
/// interpolated directly into the matview name (`advice_rule_{slug}`) — an
/// identifier position that cannot be parameterized, so it is validated, not escaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSlug(String);

impl RuleSlug {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The synthesized matview name for this rule.
    pub fn matview_name(&self) -> String {
        format!("advice_rule_{}", self.0)
    }

    fn parse(raw: &str) -> Result<Self, AdviceRuleParseError> {
        if raw.is_empty() {
            return Err(AdviceRuleParseError::Slug {
                slug: raw.to_string(),
            });
        }
        let ok = raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !ok {
            return Err(AdviceRuleParseError::Slug {
                slug: raw.to_string(),
            });
        }
        Ok(Self(raw.to_string()))
    }
}

impl<'de> Deserialize<'de> for RuleSlug {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Top-K bound: `1..=10`. A larger K is refused at parse — not clamped, not a bare `usize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundedK(u8);

impl BoundedK {
    pub const MAX: u8 = 10;

    pub fn get(self) -> u8 {
        self.0
    }

    fn parse(raw: u8) -> Result<Self, AdviceRuleParseError> {
        if (1..=Self::MAX).contains(&raw) {
            Ok(Self(raw))
        } else {
            Err(AdviceRuleParseError::K { got: raw })
        }
    }
}

impl<'de> Deserialize<'de> for BoundedK {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = u8::deserialize(d)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// Retrieval-width bound: `1..=50` (ADR 0023). The recall-oriented `LIMIT N` the
/// matview read pulls before the app-layer reranker selects the final top-K. A
/// larger N is refused at parse — not clamped, not a bare `usize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundedN(u8);

impl BoundedN {
    pub const MAX: u8 = 50;

    pub fn get(self) -> u8 {
        self.0
    }

    fn parse(raw: u8) -> Result<Self, AdviceRuleParseError> {
        if (1..=Self::MAX).contains(&raw) {
            Ok(Self(raw))
        } else {
            Err(AdviceRuleParseError::N { got: raw })
        }
    }
}

impl<'de> Deserialize<'de> for BoundedN {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = u8::deserialize(d)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// A typed anchor predicate that lowers to a SQL `WHERE` fragment. Deliberately
/// NOT a scripting expression — that is what keeps the anchor set IVM-computable
/// rather than a per-commit full scan (ADR 0022).
///
/// Authored as a single-key YAML map (`has_tag: task`, `entity: note`,
/// `prop_eq: {key, value}`, `and: [...]`). serde_yaml renders *derived* externally
/// tagged enums as `!tag` YAML tags, which is hostile to hand-authored config, so
/// this is deserialized via a single-key wire struct with an exactly-one-key check.
#[derive(Debug, Clone, PartialEq)]
pub enum AnchorSelector {
    /// Match on the block's `block_type` (e.g. the entity a block represents).
    Entity(EntityName),
    /// Match blocks carrying a given tag.
    HasTag(String),
    /// Match blocks whose `properties` JSON has `key == value`.
    PropEq(PropEqSpec),
    /// Conjunction — all sub-selectors must hold. Empty is refused at lowering.
    And(Vec<AnchorSelector>),
}

/// Single-key wire form of `AnchorSelector`. Exactly one field must be set.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnchorWire {
    #[serde(default)]
    entity: Option<EntityName>,
    #[serde(default)]
    has_tag: Option<String>,
    #[serde(default)]
    prop_eq: Option<PropEqSpec>,
    #[serde(default)]
    and: Option<Vec<AnchorSelector>>,
}

impl<'de> Deserialize<'de> for AnchorSelector {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = AnchorWire::deserialize(d)?;
        let set = [
            wire.entity.is_some(),
            wire.has_tag.is_some(),
            wire.prop_eq.is_some(),
            wire.and.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if set != 1 {
            return Err(serde::de::Error::custom(format!(
                "an anchor selector needs exactly one of `entity` / `has_tag` / \
                 `prop_eq` / `and` (found {set})"
            )));
        }
        if let Some(e) = wire.entity {
            Ok(AnchorSelector::Entity(e))
        } else if let Some(t) = wire.has_tag {
            Ok(AnchorSelector::HasTag(t))
        } else if let Some(p) = wire.prop_eq {
            Ok(AnchorSelector::PropEq(p))
        } else {
            Ok(AnchorSelector::And(
                wire.and.expect("exactly-one checked above"),
            ))
        }
    }
}

/// `prop_eq: { key, value }` — a single JSON-property equality.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropEqSpec {
    pub key: String,
    pub value: String,
}

/// Closed scoring template. v1 has a single variant; new relevance ideas ship as
/// new variants (data-compatible additions), never as raw user query text.
///
/// Authored as a single-key YAML map (`tag_overlap_recency: {source}`); see
/// `AnchorSelector` for why this is a wire struct rather than a derived enum.
#[derive(Debug, Clone, PartialEq)]
pub enum ScoringTemplate {
    /// Candidates = blocks matching `source` that share ≥1 tag with the anchor,
    /// ranked by shared-tag count desc, then recency desc.
    TagOverlapRecency(TagOverlapRecencySpec),
}

/// Single-key wire form of `ScoringTemplate`. Exactly one field must be set.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoringWire {
    #[serde(default)]
    tag_overlap_recency: Option<TagOverlapRecencySpec>,
}

impl<'de> Deserialize<'de> for ScoringTemplate {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = ScoringWire::deserialize(d)?;
        match wire.tag_overlap_recency {
            Some(spec) => Ok(ScoringTemplate::TagOverlapRecency(spec)),
            None => Err(serde::de::Error::custom(
                "a scoring template needs exactly one of `tag_overlap_recency`",
            )),
        }
    }
}

/// `tag_overlap_recency: { source }`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagOverlapRecencySpec {
    /// Which blocks are eligible candidates (typed, SQL-lowerable like the anchor).
    pub source: AnchorSelector,
}

/// The RESERVED raw-query escape hatch (ADR 0022 sub-decision 3). Any attempt to
/// deserialize a value into this type fails loudly. `raw_query: null` (or absent)
/// is treated as unset and allowed.
#[derive(Debug, Clone, PartialEq)]
struct Reserved;

impl<'de> Deserialize<'de> for Reserved {
    fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "the `raw_query` field is reserved and not yet supported (ADR 0022 \
             sub-decision 3): raw PRQL/SQL scoring will open up via this field in a \
             later version; v1 refuses it rather than silently ignoring it",
        ))
    }
}

/// The RESERVED reranker-profile selector (ADR 0023 stage 2). Any attempt to
/// deserialize a value into this type fails loudly. `rerank: null` (or absent) is
/// treated as unset and allowed.
#[derive(Debug, Clone, PartialEq)]
struct RerankReserved;

impl<'de> Deserialize<'de> for RerankReserved {
    fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "the `rerank` field is reserved and not yet supported (ADR 0023 stage 2): \
             it will name a reranker model PROFILE (endpoint/key/consent are \
             device-local prefs, never synced); reranking lands in Increment H; v1 \
             refuses it rather than silently ignoring it",
        ))
    }
}

/// A typed parse error, carried (returned) so the rule block can render its own
/// status — never logged-and-dropped (ADR 0022 "rule blocks render their own status").
#[derive(Debug, Clone, Error, PartialEq)]
pub enum AdviceRuleParseError {
    #[error(
        "invalid advice-rule slug {slug:?}: must be non-empty and only \
         [a-z0-9_] (it is interpolated into a matview identifier)"
    )]
    Slug { slug: String },

    #[error("advice-rule k={got} out of range: must be 1..={max}", max = BoundedK::MAX)]
    K { got: u8 },

    #[error("advice-rule n={got} out of range: must be 1..={max}", max = BoundedN::MAX)]
    N { got: u8 },

    #[error(
        "advice-rule k={k} exceeds retrieval width n={n}: cannot display more rows \
         than are retrieved (ADR 0023 — k is the final top-K, n the recall-oriented \
         LIMIT N feeding the reranker)"
    )]
    KGreaterThanN { k: u8, n: u8 },

    #[error("advice-rule YAML did not parse: {message}")]
    Yaml { message: String },
}

/// Parse a rule block's YAML content into a typed `AdviceRule`, or a typed error.
///
/// Refused at this boundary: unknown fields (`deny_unknown_fields`), a slug outside
/// `[a-z0-9_]`, `k`/`n` over their caps, `k > n`, and any use of the reserved
/// `raw_query` / `rerank` fields.
pub fn parse_advice_rule(yaml_content: &str) -> Result<AdviceRule, AdviceRuleParseError> {
    let rule =
        serde_yaml::from_str::<AdviceRule>(yaml_content).map_err(|e| AdviceRuleParseError::Yaml {
            message: e.to_string(),
        })?;
    // Cross-field invariant (ADR 0023): the final display cap cannot exceed the
    // retrieval width. Checked here, after per-field parse, because it spans two
    // fields. When `n` is absent it defaults to `k`, so the invariant holds trivially.
    if let Some(n) = rule.n {
        if rule.k.get() > n.get() {
            return Err(AdviceRuleParseError::KGreaterThanN {
                k: rule.k.get(),
                n: n.get(),
            });
        }
    }
    Ok(rule)
}
