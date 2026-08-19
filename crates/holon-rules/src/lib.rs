//! @c4 component
//! @c4 layer Core
//! @c4 uses holon-pattern "guard AST & parser" "Rust"
//! Pattern: Shared Kernel
//!
//! `holon_rule` YAML front-end (ADR 0024 Phase-2/3, plan §7.2) — the
//! single-block rule surface: a guard (`when:` / arc `input:`) **and** its
//! effect (`emit:` / arc `output:`), parsed at the boundary into a closed typed
//! representation ([`HolonRule`]). Mirrors the ADR 0022 discipline
//! (`parse_advice_rule` + typed error enum + newtypes; `deny_unknown_fields` so
//! malformed rules fail loud).
//!
//! A `holon_rule` block is valid YAML in the `holon_advice_rule_yaml` family
//! (ADR 0024 Amendment: "Rule blocks get a `holon_rule` source language … Rule
//! bodies are valid YAML, with guard expressions as strings parsed by the
//! Pattern parser"). Two authoring forms desugar to the *same* [`HolonRule`]:
//!
//! - **sugar** — top-level `when:` guard string + `emit:` marking delta. The
//!   `place:` value carries the placement kind: a bare root (`journals`) is an
//!   inline child; `page(journals)` is a page-file child (the emitted block is
//!   `Page`-tagged so it materializes into its own `Journals/{today}.org` — ADR
//!   0024 §7.2 journal intent; grammar + watcher landed, default seed flip
//!   deferred to Fork B B1 companion de-inline): ```yaml name: daily_journal
//!   when: 'not block_exists("Journals/{today}")' emit: place: page(journals)
//!   name: "{today}" ```
//! - **canonical arc-array** — explicit read / inhibitor input arcs + output
//!   arcs (ADR 0024 "guards are on arcs"; `absent: true` = the inhibitor arc, a
//!   `type: clock` arc = the read arc that makes the rule clock-driven):
//!   ```yaml name: daily_journal input:
//!     - bind: c type: clock
//!     - bind: j type: journal when: 'block_exists("Journals/{today}")' absent:
//!       true
//!   output:
//!     - emit: place: journals name: "{today}"
//!   ```
//!
//! ## What is lowered vs deferred (plan §7.2 scope)
//!
//! - **Guard** — fully lowered to the dual-evaluated [`Guard`] (matching path).
//! - **Effect** — the *ratcheted create* emission (`emit: {place, name}`) is
//!   lowered to a typed [`Emit`]: a canonical placement + a
//!   `{today}`-interpolated name template. The placement is either an inline
//!   child (`place: journals`) or a page-file child (`place: page(journals)`,
//!   `Place::is_page` true). This is the journal-auto-create shape.
//! - **Deferred (documented, not silently dropped):** display placement
//!   (`place: display(...)`, the advice/maintained-view side — stays ADR 0022);
//!   `consume:` input arcs (delete/move effects); multi-arc (`OutputSlot > 0`)
//!   emission. A rule that requests any of these parses its guard but carries
//!   no [`Emit`]; the operate watcher surfaces a loud status rather than firing
//!   a half-understood effect.

use holon_pattern::pattern::BuiltinRef;
use holon_pattern::pattern::Guard;
use holon_pattern::pattern::GuardParseError;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::parse_builtin;
use holon_pattern::pattern::parse_guard_body;
use serde::Deserialize;
use thiserror::Error;

/// A parsed `holon_rule`: its name, the extracted [`Guard`] (matching path),
/// and the lowered [`Emit`] effect if this is a ratcheted-create ("operate")
/// rule. Advice / guard-only rules carry `emit: None`.
#[derive(Debug, Clone, PartialEq)]
pub struct HolonRule {
    pub name: RuleName,
    pub guard: Guard,
    pub emit: Option<Emit>,
}

/// A stable rule slug ([a-z0-9_]). Same discipline as [`crate::RuleSlug`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleName(String);

impl RuleName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(raw: &str) -> Result<Self, HolonRuleParseError> {
        if is_slug(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(HolonRuleParseError::Name {
                name: raw.to_string(),
            })
        }
    }
}

fn is_slug(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

// ─── Effect (emission) ─────────────────────────────────────────────────────

/// A ratcheted **create** emission (ADR 0024 "emission into a canonical place
/// is ratcheted — the block persists once fired"). The output arc `create` =
/// "the emitted token *is* the new block". Compiled by the operate watcher to a
/// `block.create` intent stamped with rule provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct Emit {
    /// Where the created block is placed (its parent).
    pub place: Place,
    /// The created block's leaf name/content, `{today}`-interpolated per
    /// firing.
    pub name: NameTemplate,
}

/// A canonical placement: a parent block id root plus a *file granularity* — is
/// the emitted block an inline child, or its own page-file?
///
/// Three authoring forms, all canonical (ratcheted) — the placement *kind*
/// lives in the `place` value, exactly as the ADR models it (`display(under:
/// x)` is the maintained sibling of these, not parsed here):
///
/// - `place: journals` — **inline child** of `block:journals`. The created
///   block renders in the parent's own org file (`* {today}` under the journals
///   page).
/// - `place: page(journals)` — **page-file child** of `block:journals`. The
///   created block is `Page`-tagged, so the fileless-page sweep materializes it
///   into its OWN `Journals/{today}.org` (ADR 0024 §7.2 journal intent). Same
///   `kind(arg)` shape as the ADR's `display(...)`, kept colon-free so it is a
///   plain (unquoted) YAML scalar; parent resolution reuses the bare-root logic
///   (`journals` → `block:journals`), so the page's name-chain is `[Journals,
///   {today}]` — the very chain the guard's `block_exists("Journals/{today}")`
///   matches.
/// - `place: display(under: x)` — the advice/maintained side, **not** parsed
///   here (ADR 0024 — stays ADR 0022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The scheme-free parent block id root (e.g. `journals` →
    /// `block:journals`).
    root: String,
    /// Whether the emitted block is `Page`-tagged (own page-file) vs an inline
    /// child. `place: page(<root>)` sets this; a bare `place: <root>` leaves it
    /// false.
    is_page: bool,
}

impl Place {
    /// Parse a canonical place. Rejects the `display(...)` form loudly
    /// (deferred) and any non-slug root (so it can never break out of the
    /// id it becomes). `page(<root>)` is the page-file form; a bare
    /// `<root>` is inline.
    pub fn parse(raw: &str) -> Result<Self, HolonRuleParseError> {
        let raw = raw.trim();
        if raw.starts_with("display(") {
            return Err(HolonRuleParseError::DisplayPlacementDeferred {
                place: raw.to_string(),
            });
        }
        if let Some(inner) = raw.strip_prefix("page(").and_then(|s| s.strip_suffix(')')) {
            // `page(<root>)` — the page-file placement kind: the emitted block is a
            // `Page`-tagged child of `block:<root>`. Colon-free (unlike the ADR's
            // prose `display(under: x)`) so it is a plain YAML scalar needing no
            // quoting; `<root>` names the parent block, resolved exactly like a
            // bare `place: <root>`.
            return Self::from_root(inner.trim(), true).ok_or_else(|| {
                HolonRuleParseError::PagePlacement {
                    place: raw.to_string(),
                }
            });
        }
        // A place may already carry the `block:` scheme; accept and strip it so
        // the stored root is scheme-free (ORG_SYNTAX: bare ids on disk).
        Self::from_root(raw, false).ok_or_else(|| HolonRuleParseError::Place {
            place: raw.to_string(),
        })
    }

    /// Build from a (possibly `block:`-scheme-prefixed) root, or `None` if the
    /// stripped root is not a valid slug.
    fn from_root(raw: &str, is_page: bool) -> Option<Self> {
        let root = raw.strip_prefix("block:").unwrap_or(raw);
        is_slug(root).then(|| Self {
            root: root.to_string(),
            is_page,
        })
    }

    /// The scheme-free placement root (e.g. `journals`).
    pub fn as_str(&self) -> &str {
        &self.root
    }

    /// Whether the emitted block is `Page`-tagged — placed in its own page-file
    /// rather than inline under its parent.
    pub fn is_page(&self) -> bool {
        self.is_page
    }

    /// The parent block id the emit creates under (`block:journals`).
    pub fn parent_id(&self) -> String {
        format!("block:{}", self.root)
    }
}

/// A `name:` template — literal text with `{today}` / `{clock.today}`
/// interpolation, resolved against the firing binding at fire time.
#[derive(Debug, Clone, PartialEq)]
pub struct NameTemplate {
    pub segments: Vec<TemplateSegment>,
}

/// One segment of a [`NameTemplate`].
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateSegment {
    Lit(String),
    Builtin(BuiltinRef),
}

impl NameTemplate {
    /// Parse a `"Journal {today}"`-style template. `{name}` is a builtin
    /// reference; unescaped braces are a loud error (never a silent literal).
    pub fn parse(raw: &str) -> Result<Self, HolonRuleParseError> {
        if raw.is_empty() {
            return Err(HolonRuleParseError::EmptyName);
        }
        let mut segments = Vec::new();
        let mut rest = raw;
        while let Some(open) = rest.find('{') {
            if open > 0 {
                segments.push(TemplateSegment::Lit(rest[..open].to_string()));
            }
            let after = &rest[open + 1..];
            let close = after
                .find('}')
                .ok_or_else(|| HolonRuleParseError::NameTemplate {
                    template: raw.to_string(),
                    reason: "unterminated `{` interpolation".to_string(),
                })?;
            let inner = &after[..close];
            let builtin = parse_builtin(inner).map_err(|_| HolonRuleParseError::NameTemplate {
                template: raw.to_string(),
                reason: format!("unknown builtin {{{inner}}} (expected `today`)"),
            })?;
            segments.push(TemplateSegment::Builtin(builtin));
            rest = &after[close + 1..];
        }
        if rest.contains('}') {
            return Err(HolonRuleParseError::NameTemplate {
                template: raw.to_string(),
                reason: "unmatched `}`".to_string(),
            });
        }
        if !rest.is_empty() {
            segments.push(TemplateSegment::Lit(rest.to_string()));
        }
        Ok(Self { segments })
    }

    /// Render the template against the firing's `today` value.
    pub fn render(&self, today: &str) -> String {
        self.segments
            .iter()
            .map(|s| match s {
                TemplateSegment::Lit(t) => t.as_str(),
                TemplateSegment::Builtin(BuiltinRef::Today) => today,
            })
            .collect()
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
        "a holon_rule needs exactly one guard source: `when:` (sugar) or `input:` arcs \
         (canonical) — not both, not neither"
    )]
    GuardSource,
    #[error(
        "canonical `input` arcs yielded no guard condition (need at least one non-clock arc \
         carrying a `when`)"
    )]
    EmptyGuard,
    #[error(
        "input arc (bind {bind:?}, type {arc_type:?}) has no `when` but is not a clock read arc"
    )]
    ArcMissingWhen {
        bind: Option<String>,
        arc_type: Option<String>,
    },
    #[error("a holon_rule declares its effect once: `emit:` (sugar) or `output:` arcs — not both")]
    EmitAndOutput,
    #[error(
        "emit place {place:?} is not a valid placement root ([a-z0-9_], optional `block:` scheme)"
    )]
    Place { place: String },
    #[error(
        "emit place {place:?} uses display placement — the maintained/advice side (ADR 0024) is \
         not lowered by the operate front-end; use an advice rule"
    )]
    DisplayPlacementDeferred { place: String },
    #[error(
        "emit page placement {place:?} is malformed: expected `page(<root>)` where          \
         <root> is a placement slug ([a-z0-9_], optional `block:` scheme)"
    )]
    PagePlacement { place: String },
    #[error("emit name template is empty")]
    EmptyName,
    #[error("emit name template {template:?} is malformed: {reason}")]
    NameTemplate { template: String, reason: String },
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
    emit: Option<EmitWire>,
    /// Effect-kind marker (ADR 0024 `advise` | `operate`). The operate
    /// front-end reads the guard only for advice rules; the value is
    /// accepted for shape.
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
    /// `absent: true` = an inhibitor arc (negated existence): the token must
    /// NOT be present for the transition to be enabled (ADR 0024
    /// Amendment).
    #[serde(default)]
    absent: bool,
    #[serde(default)]
    #[allow(dead_code)]
    consume: bool,
}

/// A canonical `output:` arc — carries the `emit:` marking delta.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputArcWire {
    emit: EmitWire,
}

/// The `emit:` marking delta (ratcheted create): a place + a name template.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitWire {
    place: String,
    name: String,
}

impl EmitWire {
    fn lower(&self) -> Result<Emit, HolonRuleParseError> {
        Ok(Emit {
            place: Place::parse(&self.place)?,
            name: NameTemplate::parse(&self.name)?,
        })
    }
}

/// Parse a `holon_rule` YAML block (either authoring form) into a
/// [`HolonRule`], or a typed error. Both forms desugar to the same [`Guard`]
/// and [`Emit`].
pub fn parse_holon_rule(yaml: &str) -> Result<HolonRule, HolonRuleParseError> {
    let wire: RuleWire =
        serde_yaml::from_str(yaml).map_err(|e| HolonRuleParseError::Yaml(e.to_string()))?;
    let name = RuleName::parse(&wire.name)?;

    let guard = match (&wire.when, &wire.input) {
        (Some(_), Some(_)) | (None, None) => return Err(HolonRuleParseError::GuardSource),
        (Some(when), None) => Guard::parse(when)?,
        (None, Some(arcs)) => guard_from_arcs(arcs)?,
    };

    let emit = match (&wire.emit, &wire.output) {
        (Some(_), Some(_)) => return Err(HolonRuleParseError::EmitAndOutput),
        (Some(e), None) => Some(e.lower()?),
        // Canonical output: lower the first arc's emit (single-output Phase-2
        // scope; multi-arc OutputSlot emission is deferred, see module docs).
        (None, Some(arcs)) => match arcs.first() {
            Some(arc) => Some(arc.emit.lower()?),
            None => None,
        },
        (None, None) => None,
    };

    Ok(HolonRule { name, guard, emit })
}

/// Build a guard body from the canonical input arcs: each non-clock arc's
/// `when` is a condition (negated when `absent: true`); a `type: clock` arc is
/// the read arc that makes the rule clock-driven. Conjoin the conditions.
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
    use holon_pattern::pattern::Subject;

    use super::*;

    const SUGAR: &str = r#"
name: daily_journal
when: 'not block_exists("Journals/{today}")'
emit:
  place: journals
  name: "{today}"
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
  - emit:
      place: journals
      name: "{today}"
"#;

    const ADVICE: &str = r#"
name: project_related_lessons
when: 'has_tag("project")'
advise:
  source:
    has_tag: lesson
"#;

    #[test]
    fn both_forms_desugar_to_the_same_rule() {
        let sugar = parse_holon_rule(SUGAR).expect("sugar parses");
        let canonical = parse_holon_rule(CANONICAL).expect("canonical parses");
        assert_eq!(sugar.name.as_str(), "daily_journal");
        assert_eq!(canonical.name.as_str(), "daily_journal");
        assert_eq!(sugar.guard, canonical.guard, "the two forms must agree");
        assert_eq!(sugar.guard.subject, Subject::Clock);
        assert_eq!(
            sugar.emit, canonical.emit,
            "the two effect forms must agree"
        );
        let emit = sugar.emit.expect("operate rule carries an emit");
        assert_eq!(emit.place.parent_id(), "block:journals");
        assert_eq!(emit.name.render("2026-07-10"), "2026-07-10");
    }

    const PAGE_SUGAR: &str = r#"
name: daily_journal
when: 'not block_exists("Journals/{today}")'
emit:
  place: page(journals)
  name: "{today}"
"#;

    #[test]
    fn page_placement_parses_to_page_file_child() {
        let rule = parse_holon_rule(PAGE_SUGAR).expect("page sugar parses");
        let emit = rule.emit.expect("operate rule carries an emit");
        assert!(
            emit.place.is_page(),
            "page(journals) must mark the emission a page-file child"
        );
        // Parent resolution is identical to a bare root: the page's name-chain is
        // `[Journals, {today}]` — the chain the guard's block_exists matches.
        assert_eq!(emit.place.parent_id(), "block:journals");
        assert_eq!(emit.place.as_str(), "journals");
        assert_eq!(emit.name.render("2026-07-10"), "2026-07-10");
    }

    #[test]
    fn bare_place_is_not_a_page() {
        let rule = parse_holon_rule(SUGAR).expect("bare sugar parses");
        assert!(
            !rule.emit.unwrap().place.is_page(),
            "a bare `place: journals` is an inline child, not a page-file"
        );
    }

    #[test]
    fn page_placement_accepts_block_scheme_in_root() {
        let p = Place::parse("page(block:journals)").expect("scheme in root parses");
        assert!(p.is_page());
        assert_eq!(p.as_str(), "journals");
    }

    #[test]
    fn malformed_page_placement_is_a_typed_error() {
        // Empty root.
        assert!(matches!(
            Place::parse("page()").unwrap_err(),
            HolonRuleParseError::PagePlacement { .. }
        ));
        // Non-slug root inside.
        assert!(matches!(
            Place::parse("page(Bad-Root)").unwrap_err(),
            HolonRuleParseError::PagePlacement { .. }
        ));
    }

    #[test]
    fn emit_place_accepts_block_scheme_and_strips_it() {
        let yaml = "name: r\nwhen: 'not block_exists(\"A/{today}\")'\nemit:\n  place: \
                    \"block:journals\"\n  name: \"{today}\"\n";
        let rule = parse_holon_rule(yaml).expect("scheme-prefixed place parses");
        assert_eq!(rule.emit.unwrap().place.as_str(), "journals");
    }

    #[test]
    fn name_template_interpolates_literal_and_builtin() {
        let t = NameTemplate::parse("Journal {today}").unwrap();
        assert_eq!(t.render("2026-07-10"), "Journal 2026-07-10");
    }

    #[test]
    fn advice_when_carries_no_emit() {
        let rule = parse_holon_rule(ADVICE).expect("advice sugar parses");
        assert_eq!(rule.guard.subject, Subject::Block);
        assert_eq!(rule.guard.body, Pattern::HasTag("project".to_string()));
        assert_eq!(rule.emit, None, "an advice rule has no ratcheted emit");
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
    fn emit_and_output_together_is_rejected() {
        let yaml = "name: r\nwhen: 'not block_exists(\"A/{today}\")'\nemit:\n  place: a\n  name: \
                    \"{today}\"\noutput:\n  - emit:\n      place: a\n      name: \"{today}\"\n";
        assert_eq!(
            parse_holon_rule(yaml).unwrap_err(),
            HolonRuleParseError::EmitAndOutput
        );
    }

    #[test]
    fn display_placement_is_a_loud_deferral() {
        let yaml = "name: r\nwhen: 'has_tag(\"x\")'\nemit:\n  place: \"display(under: x)\"\n  \
                    name: \"{today}\"\n";
        assert!(matches!(
            parse_holon_rule(yaml).unwrap_err(),
            HolonRuleParseError::DisplayPlacementDeferred { .. }
        ));
    }

    #[test]
    fn malformed_name_template_is_a_typed_error() {
        let yaml = "name: r\nwhen: 'not block_exists(\"A/{today}\")'\nemit:\n  place: a\n  name: \
                    \"{tomorrow}\"\n";
        assert!(matches!(
            parse_holon_rule(yaml).unwrap_err(),
            HolonRuleParseError::NameTemplate { .. }
        ));
    }

    #[test]
    fn unknown_field_is_rejected() {
        let yaml = "name: r\nwhen: 'has_tag(\"x\")'\nfrobnicate: true\n";
        assert!(matches!(
            parse_holon_rule(yaml).unwrap_err(),
            HolonRuleParseError::Yaml(_)
        ));
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
