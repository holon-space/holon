//! SUT-side realizations of the datatype axis's write and Loro-read caps.
//!
//! `TypedEntityWriter` dispatches through the session's operation engine, so
//! the mode's registration decides which authority serves the write.
//! `LoroTypedEntityReader` reads the Loro store's on-disk shape directly — an
//! oracle of the shape, independent of the store's own reader.

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::OperationEngine;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;
use holon_loro::LoroDocument;
use holon_pbt_core::capabilities::SutTypedEntityLoro;
use holon_pbt_core::capabilities::SutTypedEntityWrite;
use loro::LoroValue;

pub struct TypedEntityWriter {
    engine: Arc<dyn OperationEngine>,
}

impl TypedEntityWriter {
    pub fn new(engine: Arc<dyn OperationEngine>) -> Self {
        Self { engine }
    }

    async fn dispatch(&self, type_name: &str, op: &str, params: StorageEntity) {
        self.engine
            .execute_operation(
                &holon_api::EntityName::new(type_name),
                op,
                params.clone(),
                holon_api::OpOrigin::User,
            )
            .await
            .unwrap_or_else(|e| panic!("{type_name}/{op} {params:?} failed: {e:#}"));
    }
}

fn id_param(id: &str) -> StorageEntity {
    HashMap::from([("id".into(), Value::String(id.to_string()))])
}

#[async_trait::async_trait(?Send)]
impl SutTypedEntityWrite for TypedEntityWriter {
    async fn create_typed_entity(&self, type_name: &str, id: &str, fields: Vec<(String, String)>) {
        let mut params = id_param(id);
        for (col, val) in fields {
            params.insert(col.into(), Value::String(val));
        }
        self.dispatch(type_name, "create", params).await;
    }

    async fn set_typed_entity_field(&self, type_name: &str, id: &str, field: &str, value: &str) {
        let mut params = id_param(id);
        params.insert("field".into(), Value::String(field.to_string()));
        params.insert("value".into(), Value::String(value.to_string()));
        self.dispatch(type_name, "set_field", params).await;
    }

    async fn delete_typed_entity(&self, type_name: &str, id: &str) {
        self.dispatch(type_name, "delete", id_param(id)).await;
    }
}

/// The store's own tombstone key on a type that declares no `soft_delete`.
const TOMBSTONE: &str = "_deleted";

pub struct LoroTypedEntityReader {
    doc: Arc<LoroDocument>,
}

impl LoroTypedEntityReader {
    pub fn new(doc: Arc<LoroDocument>) -> Self {
        Self { doc }
    }
}

fn cell(value: &LoroValue) -> String {
    match value {
        LoroValue::String(s) => s.to_string(),
        other => panic!("a typed-entity TEXT field holds {other:?}"),
    }
}

#[async_trait::async_trait(?Send)]
impl SutTypedEntityLoro for LoroTypedEntityReader {
    async fn loro_typed_entity_rows(
        &self,
        type_name: &str,
        columns: Vec<String>,
    ) -> Vec<Vec<String>> {
        let scheme = holon_api::EntityName::new(type_name);
        let doc = self.doc.doc();
        let root = doc.get_map(format!("entity:{}", scheme.as_str()));
        let entities = match root.get_deep_value() {
            LoroValue::Map(m) => m,
            other => panic!("the type root map read as {other:?}"),
        };
        let mut rows: Vec<Vec<String>> = entities
            .iter()
            .filter_map(|(key, entity)| {
                let fields = match entity {
                    LoroValue::Map(m) => m,
                    other => panic!("entity {key:?} of '{type_name}' holds {other:?}, not a map"),
                };
                if fields.get(TOMBSTONE).is_some_and(|v| !v.is_null()) {
                    return None;
                }
                Some(
                    columns
                        .iter()
                        .map(|c| match c.as_str() {
                            "id" => format!("{}:{key}", scheme.as_str()),
                            c => fields.get(c).map(cell).unwrap_or_default(),
                        })
                        .collect(),
                )
            })
            .collect();
        rows.sort();
        rows
    }
}
