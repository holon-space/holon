//! A whole block as one opaque column value.

use std::fmt;
use std::sync::Arc;

use holon_api::Value;
use holon_api::block::SnapshotBlock;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Number;

use crate::error::EngineError;

/// The canonical encoding of a [`SnapshotBlock`]: equal blocks give equal
/// bytes, so a block fed again unchanged cancels out instead of churning.
/// Only [`Payload::encode`] makes one.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Payload(Arc<[u8]>);

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
        let mut json = serde_json::to_value(block).expect("a SnapshotBlock serializes");
        canonical(&mut json);
        Ok(Payload(
            serde_json::to_vec(&json)
                .expect("a JSON value serializes")
                .into(),
        ))
    }

    pub fn decode(&self) -> SnapshotBlock {
        serde_json::from_slice(&self.0).expect("a Payload holds an encoded SnapshotBlock")
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

/// Sorts the keys of every object and writes -0.0 as 0.0. The map keeps
/// insertion order (the workspace enables `preserve_order`), so it is rebuilt
/// in key order.
fn canonical(json: &mut serde_json::Value) {
    match json {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = std::mem::take(map).into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            for (_, value) in &mut entries {
                canonical(value);
            }
            *map = entries.into_iter().collect();
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(canonical),
        serde_json::Value::Number(n)
            if n.as_f64().is_some_and(|f| f == 0.0 && f.is_sign_negative()) =>
        {
            *n = Number::from_f64(0.0).expect("0.0 is finite")
        }
        _ => {}
    }
}
