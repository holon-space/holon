//! Trust-policy configuration (VisionGapAnalysis C5 — autonomy/trust
//! enforcement at the intent boundary).
//!
//! Policy/fact separation: the *facts* the gate reads are provenance —
//! [`OpOrigin`] at dispatch time, the `_provenance` stamp on blocks (C2a). The
//! *policy* is this profile-level configuration: which origin classes are
//! trusted for which entities/operations. A sub-threshold `(origin, entity,
//! op)` is never executed against canonical state; the dispatch-boundary gate
//! coerces it into a proposal emission (see `holon_api::proposal`).
//!
//! Parse-don't-validate: the YAML wire form is deserialized with
//! `deny_unknown_fields` and lowered to typed enums at the boundary; an
//! unknown origin class or decision is a loud [`TrustPolicyParseError`], never
//! a silently-trusted default.

use holon_api::EntityName;
use holon_api::OpOrigin;
use serde::Deserialize;
use thiserror::Error;

/// The origin-class axis a trust rule matches on — [`OpOrigin`] with the
/// per-dispatch identity fields (session, tool call, transition) erased.
/// Thresholds are per *class*; provenance keeps the per-instance identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginClass {
    User,
    Agent,
    Rule,
    Sync,
    Ingest,
}

impl OriginClass {
    /// Classify a concrete dispatch-time origin.
    pub fn of(origin: &OpOrigin) -> Self {
        match origin {
            OpOrigin::User => Self::User,
            OpOrigin::Agent { .. } => Self::Agent,
            OpOrigin::Rule { .. } => Self::Rule,
            OpOrigin::Sync => Self::Sync,
            OpOrigin::Ingest => Self::Ingest,
        }
    }

    pub fn parse(raw: &str) -> Result<Self, TrustPolicyParseError> {
        match raw {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            "rule" => Ok(Self::Rule),
            "sync" => Ok(Self::Sync),
            "ingest" => Ok(Self::Ingest),
            other => Err(TrustPolicyParseError::UnknownOrigin {
                origin: other.to_string(),
            }),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Rule => "rule",
            Self::Sync => "sync",
            Self::Ingest => "ingest",
        }
    }
}

/// What the gate does with a matching dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    /// Above threshold: the op executes against canonical state unchanged.
    Trusted,
    /// Below threshold: the op is coerced into a proposal emission — it may
    /// only reach the proposal place, never canonical state directly.
    Propose,
}

impl TrustDecision {
    pub fn parse(raw: &str) -> Result<Self, TrustPolicyParseError> {
        match raw {
            "trusted" => Ok(Self::Trusted),
            "propose" => Ok(Self::Propose),
            other => Err(TrustPolicyParseError::UnknownDecision {
                decision: other.to_string(),
            }),
        }
    }
}

/// One policy clause: origin class, optional entity/operation narrowing, and
/// the decision. `entity`/`operation` of `None` match everything.
#[derive(Debug, Clone, PartialEq)]
pub struct TrustRule {
    pub origin: OriginClass,
    pub entity: Option<EntityName>,
    pub operation: Option<String>,
    pub decision: TrustDecision,
}

impl TrustRule {
    fn matches(&self, origin: OriginClass, entity: &EntityName, op_name: &str) -> bool {
        self.origin == origin
            && self.entity.as_ref().is_none_or(|e| e == entity)
            && self.operation.as_deref().is_none_or(|o| o == op_name)
    }
}

/// The profile-level trust policy: an ordered rule list, first match wins,
/// no match ⇒ [`TrustDecision::Trusted`].
///
/// The permissive default is deliberate: trust enforcement is opt-in
/// configuration, so an unconfigured session behaves exactly as before the
/// gate existed (every existing flow is trusted-origin).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrustPolicy {
    rules: Vec<TrustRule>,
}

impl TrustPolicy {
    /// The empty policy: every origin is trusted (the gate is a no-op).
    pub fn trust_all() -> Self {
        Self::default()
    }

    pub fn new(rules: Vec<TrustRule>) -> Self {
        Self { rules }
    }

    /// Decide one dispatch. First matching rule wins; no match ⇒ `Trusted`.
    pub fn decide(&self, origin: &OpOrigin, entity: &EntityName, op_name: &str) -> TrustDecision {
        let class = OriginClass::of(origin);
        self.rules
            .iter()
            .find(|r| r.matches(class, entity, op_name))
            .map(|r| r.decision)
            .unwrap_or(TrustDecision::Trusted)
    }

    /// Parse a policy from its YAML wire form:
    ///
    /// ```yaml
    /// rules:
    ///   - origin: agent
    ///     decision: propose
    ///   - origin: rule
    ///     entity: block
    ///     operation: delete
    ///     decision: propose
    /// ```
    pub fn parse_yaml(yaml: &str) -> Result<Self, TrustPolicyParseError> {
        let wire: PolicyWire = serde_yaml::from_str(yaml)?;
        let rules = wire
            .rules
            .into_iter()
            .map(|r| {
                Ok(TrustRule {
                    origin: OriginClass::parse(&r.origin)?,
                    entity: r.entity.map(EntityName::new),
                    operation: r.operation,
                    decision: TrustDecision::parse(&r.decision)?,
                })
            })
            .collect::<Result<Vec<_>, TrustPolicyParseError>>()?;
        Ok(Self { rules })
    }
}

#[derive(Debug, Error)]
pub enum TrustPolicyParseError {
    #[error("trust policy YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("unknown origin class '{origin}' (expected user|agent|rule|sync|ingest)")]
    UnknownOrigin { origin: String },
    #[error("unknown trust decision '{decision}' (expected trusted|propose)")]
    UnknownDecision { decision: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyWire {
    rules: Vec<RuleWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleWire {
    origin: String,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    operation: Option<String>,
    decision: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_origin() -> OpOrigin {
        OpOrigin::Agent {
            session_id: "s".to_string(),
            tool_call_id: "c".to_string(),
        }
    }

    #[test]
    fn empty_policy_trusts_everything() {
        let policy = TrustPolicy::trust_all();
        assert_eq!(
            policy.decide(&agent_origin(), &EntityName::new("block"), "create"),
            TrustDecision::Trusted
        );
    }

    #[test]
    fn yaml_round_trip_and_first_match_wins() {
        let policy = TrustPolicy::parse_yaml(
            "rules:\n\x20 - origin: agent\n\x20   entity: block\n\x20   operation: delete\n\x20   \
             decision: trusted\n\x20 - origin: agent\n\x20   decision: propose\n",
        )
        .unwrap();
        let block = EntityName::new("block");
        assert_eq!(
            policy.decide(&agent_origin(), &block, "delete"),
            TrustDecision::Trusted,
            "narrower first rule wins"
        );
        assert_eq!(
            policy.decide(&agent_origin(), &block, "create"),
            TrustDecision::Propose
        );
        assert_eq!(
            policy.decide(&OpOrigin::User, &block, "create"),
            TrustDecision::Trusted
        );
    }

    #[test]
    fn unknown_origin_is_a_loud_error() {
        let err = TrustPolicy::parse_yaml("rules:\n  - origin: gremlin\n    decision: propose\n")
            .unwrap_err();
        assert!(
            matches!(err, TrustPolicyParseError::UnknownOrigin { .. }),
            "got {err}"
        );
    }

    #[test]
    fn unknown_decision_is_a_loud_error() {
        let err = TrustPolicy::parse_yaml("rules:\n  - origin: agent\n    decision: shrug\n")
            .unwrap_err();
        assert!(
            matches!(err, TrustPolicyParseError::UnknownDecision { .. }),
            "got {err}"
        );
    }

    #[test]
    fn unknown_yaml_field_is_a_loud_error() {
        let err = TrustPolicy::parse_yaml(
            "rules:\n  - origin: agent\n    decision: propose\n    frobnicate: 1\n",
        )
        .unwrap_err();
        assert!(matches!(err, TrustPolicyParseError::Yaml(_)), "got {err}");
    }
}
