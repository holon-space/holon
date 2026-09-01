//! The storage form that keeps a [`Value`]'s kind when JSON alone cannot.
//!
//! [`Value`] is `#[serde(untagged)]`, so `DateTime` and `Json` serialize as
//! bare JSON strings and read back as `String`. A leg that persists a property
//! as JSON must therefore carry the kind somewhere.
//!
//! The SQL leg records it BESIDE the bag ([`crate::PropertyKinds`]), which is
//! sound there because one UPDATE writes bag and kinds together. A CRDT leg
//! cannot do that: a value map and a kind map are two registers that merge
//! independently, so concurrent peers can land one peer's value under the
//! other's kind. Here the kind travels INSIDE the value instead — one fact,
//! one register.
//!
//! Only the ambiguous kinds are enveloped. Every other value keeps the exact
//! bytes it has always had, so a pre-existing document needs no migration.

use crate::AmbiguousKind;
use crate::Value;

/// The key that marks a stored object as a kind envelope.
pub const KIND_ENVELOPE_KEY: &str = "__holon_kind";

/// The key carrying the enveloped payload.
pub const KIND_ENVELOPE_PAYLOAD_KEY: &str = "v";

/// A stored envelope that is not the exact shape a write produces.
#[derive(Debug, thiserror::Error)]
pub enum KindEnvelopeError {
    #[error(
        "a stored kind envelope must be exactly {{{KIND_ENVELOPE_KEY:?}: <kind>, \
         {KIND_ENVELOPE_PAYLOAD_KEY:?}: <payload>}}, got {found}"
    )]
    NotTheShape { found: String },
    #[error("{KIND_ENVELOPE_KEY:?} must name a known kind, got {found}")]
    UnknownKind { found: String },
    #[error("an enveloped {kind} must carry a string payload, got {found}")]
    PayloadNotAString { kind: &'static str, found: String },
    #[error("an enveloped date_time must be an RFC3339 timestamp, got {found:?}")]
    NotADateTime { found: String },
    #[error("an enveloped json payload is not valid JSON ({source}): {found:?}")]
    PayloadNotJson {
        found: String,
        source: serde_json::Error,
    },
}

/// The envelope for `value`, or `None` when JSON already names its kind.
///
/// Mirrors [`AmbiguousKind::of`] — the two must agree on which kinds need
/// carrying, and the debug assertion below is what keeps them agreeing.
pub fn encode(value: &Value) -> Option<serde_json::Value> {
    let enveloped = match value {
        Value::DateTime(text) => Some(envelope(AmbiguousKind::DateTime, text)),
        Value::Json(text) => Some(envelope(AmbiguousKind::Json, text)),
        _ => None,
    };
    debug_assert_eq!(
        enveloped.is_some(),
        AmbiguousKind::of(value).is_some(),
        "the envelope and the kind map must agree on which kinds are ambiguous"
    );
    enveloped
}

fn envelope(kind: AmbiguousKind, payload: &str) -> serde_json::Value {
    serde_json::json!({
        KIND_ENVELOPE_KEY: kind.as_str(),
        KIND_ENVELOPE_PAYLOAD_KEY: payload,
    })
}

/// Does this stored JSON claim to be an envelope?
///
/// Only the marker key is inspected. Deciding on the marker alone — rather
/// than on the whole shape — is what makes [`decode`] able to be LOUD: a
/// half-written envelope is claimed here and rejected there, instead of
/// quietly falling through to a plain object.
pub fn claims_envelope(json: &serde_json::Value) -> bool {
    json.get(KIND_ENVELOPE_KEY).is_some()
}

/// Decode a stored envelope, strictly.
///
/// Anything that is not the exact shape a write produces is corruption: the
/// write leg refuses to store an authored look-alike
/// ([`Value::reject_kind_envelope_shape`]), so no legitimate path can produce
/// one. Erring rather than falling back to a plain object is the difference
/// between reporting corruption and silently serving it.
pub fn decode(json: &serde_json::Value) -> Result<Value, KindEnvelopeError> {
    let object = json
        .as_object()
        .ok_or_else(|| KindEnvelopeError::NotTheShape {
            found: json.to_string(),
        })?;
    if object.len() != 2 || !object.contains_key(KIND_ENVELOPE_PAYLOAD_KEY) {
        return Err(KindEnvelopeError::NotTheShape {
            found: json.to_string(),
        });
    }
    let kind: AmbiguousKind =
        serde_json::from_value(object[KIND_ENVELOPE_KEY].clone()).map_err(|_| {
            KindEnvelopeError::UnknownKind {
                found: object[KIND_ENVELOPE_KEY].to_string(),
            }
        })?;
    let payload = &object[KIND_ENVELOPE_PAYLOAD_KEY];
    let text = payload
        .as_str()
        .ok_or_else(|| KindEnvelopeError::PayloadNotAString {
            kind: kind.as_str(),
            found: payload.to_string(),
        })?;
    match kind {
        AmbiguousKind::DateTime if !is_rfc3339(text) => Err(KindEnvelopeError::NotADateTime {
            found: text.to_string(),
        }),
        AmbiguousKind::DateTime => Ok(Value::DateTime(text.to_string())),
        // The Json variant carries a DOCUMENT, not its byte formatting, so the
        // payload is re-serialized to its canonical spelling — the same
        // contract `PropertyKinds::retype` gives the SQL leg.
        AmbiguousKind::Json => {
            let document: serde_json::Value =
                serde_json::from_str(text).map_err(|source| KindEnvelopeError::PayloadNotJson {
                    found: text.to_string(),
                    source,
                })?;
            Ok(Value::Json(document.to_string()))
        }
    }
}

fn is_rfc3339(text: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(text).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: &Value) -> Value {
        let encoded = encode(value).expect("an ambiguous kind must envelope");
        decode(&encoded).expect("a freshly written envelope must decode")
    }

    #[test]
    fn the_ambiguous_kinds_survive_the_envelope() {
        let when = Value::DateTime("2026-08-22T10:00:00Z".to_string());
        assert_eq!(round_trip(&when), when);
        let doc = Value::Json(r#"{"a":1}"#.to_string());
        assert_eq!(round_trip(&doc), doc);
    }

    #[test]
    fn an_unambiguous_kind_is_never_enveloped() {
        // Including a String that LOOKS like a timestamp: the envelope records
        // what the author meant, and a plain string meant a plain string.
        for value in [
            Value::String("2026-08-22T10:00:00Z".to_string()),
            Value::Integer(1),
            Value::Boolean(true),
            Value::Null,
            Value::Array(vec![Value::Integer(1)]),
        ] {
            assert!(encode(&value).is_none(), "{value:?} must not be enveloped");
        }
    }

    #[test]
    fn a_json_payload_reads_back_canonical() {
        let encoded = encode(&Value::Json(r#"{ "a":   1 }"#.to_string())).expect("ambiguous");
        assert_eq!(
            decode(&encoded).expect("decodes"),
            Value::Json(r#"{"a":1}"#.to_string())
        );
    }

    /// A malformed envelope is CORRUPTION, never a plain object: falling
    /// through would silently serve the wrong value for a key whose kind the
    /// author declared.
    #[test]
    fn a_malformed_envelope_is_loud() {
        let cases = [
            serde_json::json!({ KIND_ENVELOPE_KEY: "date_time" }),
            serde_json::json!({ KIND_ENVELOPE_KEY: "date_time", "v": "x", "extra": 1 }),
            serde_json::json!({ KIND_ENVELOPE_KEY: "no_such_kind", "v": "x" }),
            serde_json::json!({ KIND_ENVELOPE_KEY: "date_time", "v": 7 }),
            serde_json::json!({ KIND_ENVELOPE_KEY: "date_time", "v": "not a timestamp" }),
            serde_json::json!({ KIND_ENVELOPE_KEY: "json", "v": "{not json" }),
        ];
        for case in cases {
            assert!(
                claims_envelope(&case),
                "{case} must be CLAIMED so decode can refuse it"
            );
            assert!(decode(&case).is_err(), "{case} must be refused, not served");
        }
    }
}
