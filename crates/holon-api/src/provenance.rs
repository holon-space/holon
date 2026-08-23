//! Provenance stamping (ADR 0024 P8 / VisionGapAnalysis C2a).
//!
//! A [`ProvenanceStamp`] is the "who/when caused this block state" fact carried
//! *onto the data itself*: every engine-executed create/update op writes it
//! into the block's `_provenance` property so the substrate's own history (Loro
//! op history ≻ jj/git ≻ none) can be filtered down to the exact firing without
//! a separate log. It is derived from [`OpOrigin`] (the dispatch-time
//! provenance axis) plus a timestamp read from the injected
//! [`crate::clock::Clock`] seam — never from an ambient `SystemTime::now` in
//! domain code.
//!
//! This is the *authorship* stamp on the block (latest writer). The append-only
//! op/effect **stream** ("postponed 7 times", supervision view) is the separate
//! history relation (C2b, [`crate::history`]); the two are complementary.
//!
//! Parse-don't-validate: the stamp is a typed struct with a total conversion to
//! and a fallible parse from [`Value`]. [`ProvenanceStamp::from_value`] fails
//! loud on a malformed stamp rather than silently dropping provenance.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

use crate::OpOrigin;
use crate::Value;

/// The block property key under which the provenance stamp is stored. Leading
/// underscore marks it a system property (hidden from content rendering, same
/// convention as `_source_header_args`).
pub const PROVENANCE_PROPERTY: &str = "_provenance";

/// Property keys the ENGINE mints, so an authored one is refused rather than
/// overwritten (ruling D5.a).
///
/// An EXACT-spelling list, deliberately not the `_` prefix: `_drawer_order`
/// (`holon-org-format` `models.rs:43`) is an authored carrier the org ingest
/// path legitimately puts into create/update params
/// (`holon-orgmode/src/block_params.rs:167`), and banning the prefix would
/// refuse the vault's own write leg. `_proposal`/`_proposed_by` are NOT here
/// either: the engine re-dispatches `_proposed_by` through its own operation
/// boundary when promoting a proposal, so reserving it would refuse
/// `accept_proposal`.
pub const ENGINE_OWNED_PARAM_KEYS: &[&str] = &[PROVENANCE_PROPERTY];

// Field keys inside the nested `_provenance` object. Named constants so the
// SQL/JSON query surface (`json_extract(properties, '$._provenance.origin')`)
// and the Rust (de)serialization agree on one spelling.
const KEY_ORIGIN: &str = "origin";
const KEY_AT_MILLIS: &str = "at_millis";
const KEY_TRANSITION_ID: &str = "transition_id";
const KEY_SESSION_ID: &str = "session_id";
const KEY_TOOL_CALL_ID: &str = "tool_call_id";

/// The provenance of a single block-mutating operation, stamped onto the block.
///
/// `origin` is the [`OpOrigin::tag`] discriminator; the id fields are populated
/// per origin kind (`transition_id` for [`OpOrigin::Rule`]; `session_id` +
/// `tool_call_id` for [`OpOrigin::Agent`]) and `None` otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceStamp {
    /// Origin kind: `user` | `agent` | `rule` | `sync` | `ingest`.
    pub origin: String,
    /// Wall-clock time (ms since Unix epoch) the op was dispatched, read from
    /// the injected [`crate::clock::Clock`].
    pub at_millis: i64,
    /// The firing transition/rule id — set only for [`OpOrigin::Rule`].
    pub transition_id: Option<String>,
    /// The driving agent session id — set only for [`OpOrigin::Agent`].
    pub session_id: Option<String>,
    /// The driving agent tool-call id — set only for [`OpOrigin::Agent`].
    pub tool_call_id: Option<String>,
}

impl ProvenanceStamp {
    /// Derive the stamp from the dispatch-time origin and a clock timestamp.
    pub fn from_origin(origin: &OpOrigin, at_millis: i64) -> Self {
        let (transition_id, session_id, tool_call_id) = match origin {
            OpOrigin::Rule { transition_id } => (Some(transition_id.clone()), None, None),
            OpOrigin::Agent {
                session_id,
                tool_call_id,
            } => (None, Some(session_id.clone()), Some(tool_call_id.clone())),
            OpOrigin::User | OpOrigin::Sync | OpOrigin::Ingest => (None, None, None),
        };
        Self {
            origin: origin.tag().to_string(),
            at_millis,
            transition_id,
            session_id,
            tool_call_id,
        }
    }

    /// Total conversion into a nested [`Value::Object`] — the shape written
    /// into `block.properties["_provenance"]`. Absent id fields are omitted
    /// (not stored as null) so the JSON stays minimal.
    pub fn to_value(&self) -> Value {
        let mut map: HashMap<String, Value> = HashMap::new();
        map.insert(KEY_ORIGIN.to_string(), Value::String(self.origin.clone()));
        map.insert(KEY_AT_MILLIS.to_string(), Value::Integer(self.at_millis));
        if let Some(t) = &self.transition_id {
            map.insert(KEY_TRANSITION_ID.to_string(), Value::String(t.clone()));
        }
        if let Some(s) = &self.session_id {
            map.insert(KEY_SESSION_ID.to_string(), Value::String(s.clone()));
        }
        if let Some(c) = &self.tool_call_id {
            map.insert(KEY_TOOL_CALL_ID.to_string(), Value::String(c.clone()));
        }
        Value::Object(map)
    }

    /// Fallible parse from the stored [`Value`]. Fails loud on a non-object or
    /// a missing/mistyped required field (parse-don't-validate: a malformed
    /// stamp is a bug to surface, never provenance to silently drop).
    pub fn from_value(value: &Value) -> anyhow::Result<Self> {
        let map = match value {
            Value::Object(m) => m,
            other => anyhow::bail!("provenance stamp must be an object, got {other:?}"),
        };
        let origin = match map.get(KEY_ORIGIN) {
            Some(Value::String(s)) => s.clone(),
            other => {
                anyhow::bail!("provenance stamp '{KEY_ORIGIN}' must be a string, got {other:?}")
            }
        };
        let at_millis = match map.get(KEY_AT_MILLIS) {
            Some(Value::Integer(i)) => *i,
            other => anyhow::bail!(
                "provenance stamp '{KEY_AT_MILLIS}' must be an integer, got {other:?}"
            ),
        };
        let opt_str = |key: &str| -> anyhow::Result<Option<String>> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                other => anyhow::bail!(
                    "provenance stamp '{key}' must be a string when present, got {other:?}"
                ),
            }
        };
        Ok(Self {
            origin,
            at_millis,
            transition_id: opt_str(KEY_TRANSITION_ID)?,
            session_id: opt_str(KEY_SESSION_ID)?,
            tool_call_id: opt_str(KEY_TOOL_CALL_ID)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_origin_carries_transition_id() {
        let origin = OpOrigin::Rule {
            transition_id: "rule:delegate-work".to_string(),
        };
        let stamp = ProvenanceStamp::from_origin(&origin, 1_700_000_000_000);
        assert_eq!(stamp.origin, "rule");
        assert_eq!(stamp.transition_id.as_deref(), Some("rule:delegate-work"));
        assert_eq!(stamp.session_id, None);
        assert_eq!(stamp.at_millis, 1_700_000_000_000);
    }

    #[test]
    fn agent_origin_carries_session_and_tool_call() {
        let origin = OpOrigin::Agent {
            session_id: "sess-1".to_string(),
            tool_call_id: "call-42".to_string(),
        };
        let stamp = ProvenanceStamp::from_origin(&origin, 42);
        assert_eq!(stamp.origin, "agent");
        assert_eq!(stamp.session_id.as_deref(), Some("sess-1"));
        assert_eq!(stamp.tool_call_id.as_deref(), Some("call-42"));
        assert_eq!(stamp.transition_id, None);
    }

    #[test]
    fn value_round_trip_is_lossless() {
        for origin in [
            OpOrigin::User,
            OpOrigin::Sync,
            OpOrigin::Ingest,
            OpOrigin::Rule {
                transition_id: "t1".to_string(),
            },
            OpOrigin::Agent {
                session_id: "s".to_string(),
                tool_call_id: "c".to_string(),
            },
        ] {
            let stamp = ProvenanceStamp::from_origin(&origin, 99);
            let parsed = ProvenanceStamp::from_value(&stamp.to_value()).unwrap();
            assert_eq!(stamp, parsed, "round trip for origin {origin:?}");
        }
    }

    #[test]
    fn from_value_fails_loud_on_non_object() {
        let err = ProvenanceStamp::from_value(&Value::String("nope".to_string()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be an object"), "got: {err}");
    }

    #[test]
    fn from_value_fails_loud_on_missing_origin() {
        let mut map = HashMap::new();
        map.insert(KEY_AT_MILLIS.to_string(), Value::Integer(1));
        let err = ProvenanceStamp::from_value(&Value::Object(map))
            .unwrap_err()
            .to_string();
        assert!(err.contains(KEY_ORIGIN), "got: {err}");
    }
}
