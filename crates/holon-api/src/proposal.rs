//! Proposal records — the coerced-emission form of a sub-trust-threshold
//! operation (VisionGapAnalysis C5).
//!
//! A below-threshold origin never reaches canonical state directly: the trust
//! gate at the dispatch boundary re-emits its operation as a *proposal block*
//! under the proposal place (`block:proposals`). The wrapped operation —
//! entity, op name, params — is carried verbatim in the block's `_proposal`
//! property, and the block's ordinary `_provenance` stamp names the proposer.
//! Confirmation is an ordinary intent from a trusted origin
//! (`accept_proposal`) that re-dispatches the wrapped op into the canonical
//! place; rejection (`reject_proposal`) retracts the proposal from the pending
//! set. The safety property — nothing sub-threshold mutates canonical state —
//! is thereby derived from place topology, not asserted per code path.
//!
//! Parse-don't-validate: the record is a typed struct with a total conversion
//! to and a fallible parse from [`Value`]. A malformed record is a loud error,
//! never a silently dropped proposal.

use std::collections::HashMap;

use crate::EntityName;
use crate::Value;

/// The block property key carrying the wrapped operation. Leading underscore
/// marks it a system property (same convention as `_provenance`).
pub const PROPOSAL_PROPERTY: &str = "_proposal";

/// The block property key carrying the *proposer's* provenance stamp on a
/// promoted block. The promoted block's `_provenance` names the confirmer (the
/// latest writer); `_proposed_by` preserves the sub-threshold origin that
/// authored the content, so both provenances survive promotion.
pub const PROPOSED_BY_PROPERTY: &str = "_proposed_by";

/// The well-known id of the proposal place root block (`block:proposals`).
pub const PROPOSALS_ROOT_ID: &str = "proposals";

/// Engine-level compound op: promote a pending proposal by re-dispatching its
/// wrapped operation with the confirmer's origin.
pub const ACCEPT_PROPOSAL_OP: &str = "accept_proposal";

/// Engine-level compound op: retract a pending proposal without executing it.
pub const REJECT_PROPOSAL_OP: &str = "reject_proposal";

// Field keys inside the nested `_proposal` object. Named constants so the SQL
// query surface (`json_extract(properties, '$._proposal.status')`) and the
// Rust (de)serialization agree on one spelling.
const KEY_STATUS: &str = "status";
const KEY_ENTITY: &str = "entity";
const KEY_OP: &str = "op";
const KEY_PARAMS: &str = "params";
const KEY_RESOLVED_BY: &str = "resolved_by";

/// Lifecycle of a proposal. `Pending` is the only state the confirmation ops
/// accept; `Accepted`/`Rejected` are terminal and keep the record queryable
/// for the supervision view (acceptance stats per origin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
}

impl ProposalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            other => anyhow::bail!("unknown proposal status '{other}'"),
        }
    }
}

/// The wrapped operation a proposal carries, plus its lifecycle state.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalRecord {
    pub status: ProposalStatus,
    /// Entity the wrapped op targets (e.g. `block`).
    pub entity: EntityName,
    /// Wrapped op name (e.g. `create`, `set_field`).
    pub op_name: String,
    /// Wrapped op params, verbatim as dispatched by the proposer.
    pub params: HashMap<String, Value>,
    /// Provenance stamp value of the confirmer/rejecter — set when the
    /// proposal leaves `Pending`, `None` before.
    pub resolved_by: Option<Value>,
}

impl ProposalRecord {
    /// A fresh pending record wrapping one operation.
    pub fn pending(
        entity: EntityName,
        op_name: impl Into<String>,
        params: HashMap<String, Value>,
    ) -> Self {
        Self {
            status: ProposalStatus::Pending,
            entity,
            op_name: op_name.into(),
            params,
            resolved_by: None,
        }
    }

    /// Total conversion into the nested [`Value::Object`] stored under
    /// `block.properties["_proposal"]`.
    pub fn to_value(&self) -> Value {
        let mut map: HashMap<String, Value> = HashMap::new();
        map.insert(
            KEY_STATUS.to_string(),
            Value::String(self.status.as_str().to_string()),
        );
        map.insert(
            KEY_ENTITY.to_string(),
            Value::String(self.entity.as_str().to_string()),
        );
        map.insert(KEY_OP.to_string(), Value::String(self.op_name.clone()));
        map.insert(KEY_PARAMS.to_string(), Value::Object(self.params.clone()));
        if let Some(resolved_by) = &self.resolved_by {
            map.insert(KEY_RESOLVED_BY.to_string(), resolved_by.clone());
        }
        Value::Object(map)
    }

    /// Fallible parse from the stored [`Value`]. Fails loud on a non-object or
    /// a missing/mistyped field.
    pub fn from_value(value: &Value) -> anyhow::Result<Self> {
        let map = match value {
            Value::Object(m) => m,
            other => anyhow::bail!("proposal record must be an object, got {other:?}"),
        };
        let get_str = |key: &str| -> anyhow::Result<String> {
            match map.get(key) {
                Some(Value::String(s)) => Ok(s.clone()),
                other => anyhow::bail!("proposal record '{key}' must be a string, got {other:?}"),
            }
        };
        let status = ProposalStatus::parse(&get_str(KEY_STATUS)?)?;
        let entity = EntityName::new(get_str(KEY_ENTITY)?);
        let op_name = get_str(KEY_OP)?;
        let params = match map.get(KEY_PARAMS) {
            Some(Value::Object(m)) => m.clone(),
            other => {
                anyhow::bail!("proposal record '{KEY_PARAMS}' must be an object, got {other:?}")
            }
        };
        let resolved_by = map.get(KEY_RESOLVED_BY).cloned();
        Ok(Self {
            status,
            entity,
            op_name,
            params,
            resolved_by,
        })
    }

    /// The record after resolution: terminal `status`, stamped with the
    /// confirmer/rejecter provenance value.
    pub fn resolved(mut self, status: ProposalStatus, resolved_by: Value) -> Self {
        assert!(
            matches!(status, ProposalStatus::Accepted | ProposalStatus::Rejected),
            "resolution status must be terminal, got {status:?}"
        );
        self.status = status;
        self.resolved_by = Some(resolved_by);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ProposalRecord {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String("block:x".to_string()));
        params.insert("content".to_string(), Value::String("hello".to_string()));
        ProposalRecord::pending(EntityName::new("block"), "create", params)
    }

    #[test]
    fn value_round_trip_is_lossless() {
        let record = sample();
        let parsed = ProposalRecord::from_value(&record.to_value()).unwrap();
        assert_eq!(record, parsed);
    }

    #[test]
    fn resolved_round_trip_carries_resolver() {
        let record = sample().resolved(ProposalStatus::Accepted, Value::String("user".to_string()));
        let parsed = ProposalRecord::from_value(&record.to_value()).unwrap();
        assert_eq!(parsed.status, ProposalStatus::Accepted);
        assert_eq!(parsed.resolved_by, Some(Value::String("user".to_string())));
    }

    #[test]
    fn from_value_fails_loud_on_missing_op() {
        let mut map = HashMap::new();
        map.insert(KEY_STATUS.to_string(), Value::String("pending".to_string()));
        map.insert(KEY_ENTITY.to_string(), Value::String("block".to_string()));
        map.insert(KEY_PARAMS.to_string(), Value::Object(HashMap::new()));
        let err = ProposalRecord::from_value(&Value::Object(map))
            .unwrap_err()
            .to_string();
        assert!(err.contains(KEY_OP), "got: {err}");
    }

    #[test]
    fn unknown_status_fails_loud() {
        let err = ProposalStatus::parse("maybe").unwrap_err().to_string();
        assert!(err.contains("maybe"), "got: {err}");
    }
}
