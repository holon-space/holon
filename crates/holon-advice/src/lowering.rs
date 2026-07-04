//! Lower a typed `AnchorSelector` to injection-safe SQL fragments.
//!
//! The fragments are **JOINs + WHERE predicates**, never correlated subqueries:
//! Turso's IVM rejects `EXISTS`/`IN (SELECT …)` at CREATE. Tag membership is an
//! inner JOIN onto `block_tags` (proven IVM-correct — Spike-2 probes);
//! `block_type` / `properties` predicates need a 1:1 JOIN onto `block_raw` used
//! ONLY in `WHERE` (a `block_raw` column in `GROUP BY`/`SELECT` corrupts the
//! IVM aggregate — see the probe tests). The selector is lowered relative to an
//! *id expression* (e.g. `atg.block_id`) rather than a fixed alias, because the
//! synthesized matview identifies anchors/candidates by the `block_tags`
//! overlap columns, not a `block_raw` alias.
//!
//! Injection safety is by REFUSAL, not escaping (parse-don't-validate): string
//! values are validated against a conservative charset and refused if they
//! carry anything exotic, then wrapped in single quotes.

use thiserror::Error;

use crate::rule::AnchorSelector;
use crate::rule::PropEqSpec;

/// SQL fragments a selector contributes to the synthesized matview.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SqlFragments {
    /// Full `JOIN …` clauses (each a self-contained IVM-safe inner join onto
    /// `block_tags`).
    pub joins: Vec<String>,
    /// `WHERE` conjuncts (plain column/scalar predicates — no subqueries).
    pub predicates: Vec<String>,
    /// True if any predicate reads `block_raw` columns → the synthesizer must
    /// add a 1:1 `JOIN block_raw {raw_alias} ON {raw_alias}.id = {id_expr}`
    /// (WHERE-only use).
    pub needs_block_raw: bool,
}

/// A typed lowering error, carried so a broken rule fails loud on its own
/// block.
#[derive(Debug, Error, PartialEq)]
pub enum LoweringError {
    #[error(
        "advice-rule string value {value:?} for {field} contains characters that are refused at \
         the SQL boundary (allowed: letters, digits, space, and `_ - . / :`); refusing rather \
         than escaping"
    )]
    UnsafeValue { field: &'static str, value: String },

    #[error(
        "advice-rule property key {key:?} is not a safe JSON key (allowed: letters, digits, `_`); \
         refusing rather than escaping"
    )]
    UnsafeKey { key: String },

    #[error("advice-rule `and:` selector is empty — a vacuous always-true anchor is refused")]
    EmptyAnd,
}

/// Value charset: letters, digits, space, and a few safe punctuation. Anything
/// else (quotes, backslash, semicolon, `%`, control chars, …) is refused.
fn is_safe_value(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.' | '/' | ':'))
}

/// JSON key charset: strictly `[A-Za-z0-9_]` (no dots — v1 is single-level).
fn is_safe_key(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn sql_string_literal(value: &str, field: &'static str) -> Result<String, LoweringError> {
    if is_safe_value(value) {
        Ok(format!("'{value}'"))
    } else {
        Err(LoweringError::UnsafeValue {
            field,
            value: value.to_string(),
        })
    }
}

impl AnchorSelector {
    /// Lower to IVM-safe SQL fragments.
    ///
    /// - `id_expr`: the block-id expression this selector filters (e.g.
    ///   `atg.block_id`).
    /// - `raw_alias`: the `block_raw` alias to use if a predicate needs
    ///   `block_raw` columns; the synthesizer joins it 1:1 iff
    ///   `needs_block_raw` is set.
    /// - `join_seq`: mints unique `block_tags` join aliases across the whole
    ///   query.
    pub fn lower(
        &self,
        id_expr: &str,
        raw_alias: &str,
        join_seq: &mut usize,
    ) -> Result<SqlFragments, LoweringError> {
        let mut out = SqlFragments::default();
        self.lower_into(id_expr, raw_alias, join_seq, &mut out)?;
        Ok(out)
    }

    fn lower_into(
        &self,
        id_expr: &str,
        raw_alias: &str,
        join_seq: &mut usize,
        out: &mut SqlFragments,
    ) -> Result<(), LoweringError> {
        match self {
            AnchorSelector::Entity(name) => {
                let lit = sql_string_literal(name.as_str(), "entity")?;
                out.needs_block_raw = true;
                out.predicates
                    .push(format!("{raw_alias}.block_type = {lit}"));
            }
            AnchorSelector::HasTag(tag) => {
                let lit = sql_string_literal(tag, "has_tag")?;
                let j = format!("ht{join_seq}");
                *join_seq += 1;
                out.joins.push(format!(
                    "JOIN block_tags {j} ON {j}.block_id = {id_expr} AND {j}.tag = {lit}"
                ));
            }
            AnchorSelector::PropEq(PropEqSpec { key, value }) => {
                if !is_safe_key(key) {
                    return Err(LoweringError::UnsafeKey { key: key.clone() });
                }
                let lit = sql_string_literal(value, "prop_eq.value")?;
                out.needs_block_raw = true;
                out.predicates.push(format!(
                    "json_extract({raw_alias}.properties, '$.{key}') = {lit}"
                ));
            }
            AnchorSelector::And(parts) => {
                if parts.is_empty() {
                    return Err(LoweringError::EmptyAnd);
                }
                for part in parts {
                    part.lower_into(id_expr, raw_alias, join_seq, out)?;
                }
            }
        }
        Ok(())
    }
}
