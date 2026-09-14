//! Local intents as operations against the mirrored type's own authority.
//!
//! Every local write goes through the declared type's generic
//! `create`/`set_field`/`purge`, so a connection adds no second writer of its
//! own (ADR 0034 §7, invariant 4).

use std::collections::HashMap;

use holon_api::Operation;
use holon_api::Value;

use crate::reconcile::LocalIntent;
use crate::spec::ListSyncSpec;

pub fn local_intent_operation(spec: &ListSyncSpec, intent: &LocalIntent) -> Operation {
    let entity = spec.entity.as_str();
    match intent {
        LocalIntent::Insert { id, columns } => {
            let mut params: HashMap<String, Value> = columns
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            params.insert("id".to_string(), Value::String(id.clone()));
            Operation::new(entity, "create", "Add synced row", params)
        }
        LocalIntent::SetColumn { id, column, value } => {
            set_field(entity, id, column, value.clone(), "Update synced row")
        }
        LocalIntent::TouchWatermark { id, at } => set_field(
            entity,
            id,
            &spec.watermark_column,
            Value::String(at.clone()),
            "Mark synced row seen",
        ),
        // `purge`, not `delete`: the type declares soft deletion, so `delete`
        // WRITES a tombstone for the peer to be told about. Both of these
        // intents are the other direction — the peer has already spoken, and
        // the row must actually go.
        LocalIntent::Delete { id } | LocalIntent::ReapTombstone { id } => {
            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::String(id.clone()));
            Operation::new(entity, "purge", "Remove synced row", params)
        }
    }
}

fn set_field(entity: &str, id: &str, field: &str, value: Value, display: &str) -> Operation {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(id.to_string()));
    params.insert("field".to_string(), Value::String(field.to_string()));
    params.insert("value".to_string(), value);
    Operation::new(entity, "set_field", display, params)
}
