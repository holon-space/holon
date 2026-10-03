//! A whole block as one opaque column value.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use holon_api::RemovedTag;
use holon_api::Value;
use holon_api::block::SnapshotBlock;
use serde::Deserialize;
use serde::Serialize;

use crate::error::EngineError;

/// The canonical encoding of a [`SnapshotBlock`]: equal blocks give equal
/// bytes, so a block fed again unchanged cancels out instead of churning.
/// [`Payload::encode`] makes one; deserialized bytes are one only if they
/// decode.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "Arc<[u8]>")]
pub struct Payload(Arc<[u8]>);

/// The block travels with its properties taken out, because `Value`'s own
/// serde is untagged and reads `DateTime` and `Json` back as `String`.
#[derive(Serialize, Deserialize)]
struct Wire {
    block: SnapshotBlock,
    properties: BTreeMap<String, Tagged>,
}

/// [`Value`] with its variant written out. Maps are ordered, and -0.0 is
/// stored as 0.0, which it equals.
#[derive(Serialize, Deserialize)]
enum Tagged {
    Removed,
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(String),
    Json(String),
    Array(Vec<Tagged>),
    Object(BTreeMap<String, Tagged>),
    Null,
}

impl Payload {
    /// JSON has no NaN or infinity, and `serde_json` writes them as `null`,
    /// so a block holding one is refused rather than changed.
    pub fn encode(block: &SnapshotBlock) -> Result<Payload, EngineError> {
        for (key, value) in &block.block.properties {
            if !finite(value) {
                return Err(EngineError::NonFiniteFloat {
                    id: block.block.id.clone(),
                    key: key.clone(),
                });
            }
        }
        let mut block = block.clone();
        let properties = std::mem::take(&mut block.block.properties)
            .into_iter()
            .map(|(key, value)| (key, Tagged::from(&value)))
            .collect();
        let wire = Wire { block, properties };
        let bytes = serde_json::to_vec(&wire).map_err(|e| EngineError::UnencodableBlock {
            id: wire.block.block.id.clone(),
            reason: e.to_string(),
        })?;
        Ok(Payload(bytes.into()))
    }

    pub fn decode(&self) -> Result<SnapshotBlock, EngineError> {
        decode(&self.0)
    }
}

impl TryFrom<Arc<[u8]>> for Payload {
    type Error = EngineError;

    fn try_from(bytes: Arc<[u8]>) -> Result<Payload, EngineError> {
        decode(&bytes)?;
        Ok(Payload(bytes))
    }
}

fn decode(bytes: &[u8]) -> Result<SnapshotBlock, EngineError> {
    let Wire {
        mut block,
        properties,
    } = serde_json::from_slice(bytes).map_err(|e| EngineError::UndecodablePayload {
        reason: e.to_string(),
    })?;
    block.block.properties = properties
        .into_iter()
        .map(|(key, value)| (key, Value::from(value)))
        .collect();
    Ok(block)
}

impl From<&Value> for Tagged {
    fn from(value: &Value) -> Tagged {
        match value {
            Value::Removed(RemovedTag) => Tagged::Removed,
            Value::String(s) => Tagged::String(s.clone()),
            Value::Integer(i) => Tagged::Integer(*i),
            Value::Float(f) => Tagged::Float(if *f == 0.0 { 0.0 } else { *f }),
            Value::Boolean(b) => Tagged::Boolean(*b),
            Value::DateTime(s) => Tagged::DateTime(s.clone()),
            Value::Json(s) => Tagged::Json(s.clone()),
            Value::Array(items) => Tagged::Array(items.iter().map(Tagged::from).collect()),
            Value::Object(map) => Tagged::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), Tagged::from(value)))
                    .collect(),
            ),
            Value::Null => Tagged::Null,
        }
    }
}

impl From<Tagged> for Value {
    fn from(tagged: Tagged) -> Value {
        match tagged {
            Tagged::Removed => Value::Removed(RemovedTag),
            Tagged::String(s) => Value::String(s),
            Tagged::Integer(i) => Value::Integer(i),
            Tagged::Float(f) => Value::Float(f),
            Tagged::Boolean(b) => Value::Boolean(b),
            Tagged::DateTime(s) => Value::DateTime(s),
            Tagged::Json(s) => Value::Json(s),
            Tagged::Array(items) => Value::Array(items.into_iter().map(Value::from).collect()),
            Tagged::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, value)| (key, Value::from(value)))
                    .collect(),
            ),
            Tagged::Null => Value::Null,
        }
    }
}

impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Payload({})", String::from_utf8_lossy(&self.0))
    }
}

fn finite(value: &Value) -> bool {
    match value {
        Value::Float(f) => f.is_finite(),
        Value::Array(items) => items.iter().all(finite),
        Value::Object(map) => map.values().all(finite),
        Value::Removed(_)
        | Value::String(_)
        | Value::Integer(_)
        | Value::Boolean(_)
        | Value::DateTime(_)
        | Value::Json(_)
        | Value::Null => true,
    }
}
