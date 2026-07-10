//! `holon_rule` YAML front-end (ADR 0024 Phase-2 spike) — guard extraction only.
//!
//! A `holon_rule` block is valid YAML in the `holon_advice_rule_yaml` family
//! (ADR 0024 Amendment: "Rule blocks get a `holon_rule` source language … Rule
//! bodies are valid YAML, with guard expressions as strings parsed by the Pattern
//! parser"). This module parses the daily-journal rule in **two** authoring forms
//! and desugars both to the *same* [`Guard`]:
//!
//! - **sugar** — a single `when:` guard string plus an `emit:` marking delta:
//!   ```yaml
//!   name: daily_journal
//!   when: 'not block_exists("Journals/{today}")'
//!   emit:
//!     - name: "Journals/{today}"
//!       type: journal
//!   ```
//! - **canonical arc-array** — explicit read / inhibitor input arcs + output arcs
//!   (ADR 0024 "guards are on arcs"; `absent: true` = the inhibitor arc, a
//!   `type: clock` arc = the read arc that makes the rule clock-driven):
//!   ```yaml
//!   name: daily_journal
//!   input:
//!     - bind: c
//!       type: clock
//!     - bind: j
//!       type: journal
//!       when: 'block_exists("Journals/{today}")'
//!       absent: true
//!   output:
//!     - name: "Journals/{today}"
//!       type: journal
//!   ```
//!
//! Scope: this extracts the **guard** (the matching path). Firing/effects
//! (`emit`/`output`/`consume`) are parsed for shape only and NOT wired — that is
//! plan §7.2, not the spike. Errors are typed and carried, mirroring
//! [`crate::AdviceRuleParseError`].

use holon_api::pattern::{Guard, GuardParseError, Pattern, parse_guard_body};
use serde::Deserialize;
use thiserror::Error;

/// A parsed `holon_rule` — the spike only surfaces its name + extracted guard.
#[derive(Debug, Clone, PartialEq)]
pub struct HolonRule {
    pub name: RuleName,
    pub guard: Guard,
}

/// A stable rule slug ([a-z0-9_]). Same discipline as [`crate::RuleSlug`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleName(String);

impl RuleName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(raw: &str) -> Result<Self, HolonRuleParseError> {
        let ok = !raw.is_empty()
            && raw
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if ok {
            Ok(Self(raw.to_string()))
        } else {
            Err(HolonRuleParseError::Name {
                name: raw.to_string(),
            })
        }
    }
}

/// A typed parse error, carried so a rule block can render its own status
/// (mirrors [`crate::AdviceRuleParseError`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HolonRuleParseError {
    #[error("holon_rule YAML error: {0}")]
    Yaml(String),
    #[error("rule name {name:?} is not a valid slug ([a-z0-9_])")]
    Name { name: String },
    #[error(
        "a holon_rule needs exactly one guard source: `when:` (sugar) or `input:` \
         arcs (canonical) — not both, not neither"
    )]
    GuardSource,
    #[error(
        "canonical `input` arcs yielded no guard condition (need at least one \
         non-clock arc carrying a `when`)"
    )]
    EmptyGuard,
    #[error(
        "input arc (bind {bind:?}, type {arc_type:?}) has no `when` but is not a clock read arc"
    )]
    ArcMissingWhen {
        bind: Option<String>,
        arc_type: Option<String>,
    },
    #[error(transparent)]
    Guard(#[from] GuardParseError),
}

// ─── YAML wire structs ────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleWire {
    name: String,
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    input: Option<Vec<InputArcWire>>,
    #[serde(default)]
    output: Option<Vec<OutputArcWire>>,
    #[serde(default)]
    emit: Option<Vec<OutputArcWire>>,
    /// Effect-kind marker (ADR 0024 `advise` | `operate`). The spike reads only
    /// the guard, so the value is accepted and ignored beyond shape.
    #[serde(default)]
    #[allow(dead_code)]
    advise: Option<serde_yaml::Value>,
}

/// A read / inhibitor input arc: `{bind, type, when, absent, consume}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputArcWire {
    #[serde(default)]
    bind: Option<String>,
    #[serde(rename = "type", default)]
    arc_type: Option<String>,
    #[serde(default)]
    when: Option<String>,
    /// `absent: true` = an inhibitor arc (negated existence): the token must NOT
    /// be present for the transition to be enabled (ADR 0024 Amendment).
    #[serde(default)]
    absent: bool,
    #[serde(default)]
    #[allow(dead_code)]
    consume: bool,
}

/// An output (emit) arc — parsed for shape only; effects are not wired (§7.2).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct OutputArcWire {
    name: String,
    #[serde(rename = "type", default)]
    arc_type: Option<String>,
    #[serde(default)]
    after: Option<String>,
}

/// Parse a `holon_rule` YAML block (either authoring form) into a [`HolonRule`],
/// or a typed error. Both forms desugar to the same [`Guard`].
pub fn parse_holon_rule(yaml: &str) -> Result<HolonRule, HolonRuleParseError> {
    let wire: RuleWire =
        serde_yaml::from_str(yaml).map_err(|e| HolonRuleParseError::Yaml(e.to_string()))?;
    let name = RuleName::parse(&wire.name)?;

    let guard = match (&wire.when, &wire.input) {
        (Some(_), Some(_)) | (None, None) => return Err(HolonRuleParseError::GuardSource),
        (Some(when), None) => Guard::parse(when)?,
        (None, Some(arcs)) => guard_from_arcs(arcs)?,
    };

    // Effect deltas are validated for shape (deny_unknown_fields already ran) but
    // not lowered — firing is plan §7.2, out of the spike's guard-only scope.
    let _ = (&wire.output, &wire.emit);

    Ok(HolonRule { name, guard })
}

/// Build a guard body from the canonical input arcs: each non-clock arc's `when`
/// is a condition (negated when `absent: true`); a `type: clock` arc is the read
/// arc that makes the rule clock-driven. Conjoin the conditions.
fn guard_from_arcs(arcs: &[InputArcWire]) -> Result<Guard, HolonRuleParseError> {
    let mut conditions = Vec::new();
    for arc in arcs {
        if arc.arc_type.as_deref() == Some("clock") {
            // Read arc on the clock relation: drives re-fire, contributes no body.
            continue;
        }
        let when = arc
            .when
            .as_deref()
            .ok_or_else(|| HolonRuleParseError::ArcMissingWhen {
                bind: arc.bind.clone(),
                arc_type: arc.arc_type.clone(),
            })?;
        let cond = parse_guard_body(when)?;
        conditions.push(if arc.absent {
            Pattern::Not(Box::new(cond))
        } else {
            cond
        });
    }
    let body = match conditions.len() {
        0 => return Err(HolonRuleParseError::EmptyGuard),
        1 => conditions.pop().unwrap(),
        _ => Pattern::And(conditions),
    };
    Ok(Guard::from_body(body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::pattern::Subject;

    const SUGAR: &str = r#"
name: daily_journal
when: 'not block_exists("Journals/{today}")'
emit:
  - name: "Journals/{today}"
    type: journal
"#;

    const CANONICAL: &str = r#"
name: daily_journal
input:
  - bind: c
    type: clock
  - bind: j
    type: journal
    when: 'block_exists("Journals/{today}")'
    absent: true
output:
  - name: "Journals/{today}"
    type: journal
"#;

    const ADVICE: &str = r#"
name: project_related_lessons
when: 'has_tag("project")'
advise:
  source:
    has_tag: lesson
"#;

    #[test]
    fn both_forms_desugar_to_the_same_guard() {
        let sugar = parse_holon_rule(SUGAR).expect("sugar parses");
        let canonical = parse_holon_rule(CANONICAL).expect("canonical parses");
        assert_eq!(sugar.name.as_str(), "daily_journal");
        assert_eq!(canonical.name.as_str(), "daily_journal");
        assert_eq!(sugar.guard, canonical.guard, "the two forms must agree");
        assert_eq!(sugar.guard.subject, Subject::Clock);
    }

    #[test]
    fn advice_when_extracts_block_guard() {
        let rule = parse_holon_rule(ADVICE).expect("advice sugar parses");
        assert_eq!(rule.guard.subject, Subject::Block);
        assert_eq!(rule.guard.body, Pattern::HasTag("project".to_string()));
    }

    #[test]
    fn both_or_neither_guard_source_is_rejected() {
        let both = "name: r\nwhen: 'has_tag(\"x\")'\ninput: []\n";
        assert_eq!(
            parse_holon_rule(both).unwrap_err(),
            HolonRuleParseError::GuardSource
        );
        let neither = "name: r\n";
        assert_eq!(
            parse_holon_rule(neither).unwrap_err(),
            HolonRuleParseError::GuardSource
        );
    }

    #[test]
    fn bad_name_is_typed_error() {
        let yaml = "name: Bad-Name\nwhen: 'has_tag(\"x\")'\n";
        assert!(matches!(
            parse_holon_rule(yaml).unwrap_err(),
            HolonRuleParseError::Name { .. }
        ));
    }

    #[test]
    fn guard_parse_error_propagates() {
        let yaml = "name: r\nwhen: 'frobnicate(\"x\")'\n";
        assert!(matches!(
            parse_holon_rule(yaml).unwrap_err(),
            HolonRuleParseError::Guard(GuardParseError::UnknownFunction { .. })
        ));
    }
}
