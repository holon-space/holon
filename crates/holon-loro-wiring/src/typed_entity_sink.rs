//! The Turso leg of the Loro typed-entity store: the projector lands each
//! changed entity's row in its type's raw table through that type's SQL
//! provider, tagged `EventOrigin::Loro` so nothing echoes it back.

use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use fluxdi::Shared;
use holon::core::SqlOperationProvider;
use holon::storage::turso::DbHandle;
use holon_api::EntityName;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::EventOrigin;
use holon_core::OriginTaggedWrites;
use holon_loro::loro_entity_operations::TypedEntitySink;
use holon_profiles::TypeRegistry;
use holon_turso::turso_adapter::TursoAdapter;

pub struct SqlTypedEntitySink {
    db: DbHandle,
    registry: Shared<TypeRegistry>,
}

impl SqlTypedEntitySink {
    pub fn new(db: DbHandle, registry: Shared<TypeRegistry>) -> Self {
        Self { db, registry }
    }
}

#[async_trait]
impl TypedEntitySink for SqlTypedEntitySink {
    async fn project(
        &self,
        entity: &EntityName,
        rows: Vec<(String, Option<StorageEntity>)>,
    ) -> Result<()> {
        let type_def = self
            .registry
            .all()
            .into_iter()
            .find(|t| &EntityName::new(t.name.as_str()) == entity)
            .with_context(|| format!("no type is declared for the typed entity '{entity}'"))?;
        let provider = SqlOperationProvider::for_type(self.db.clone(), &type_def)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let exists_sql = format!(
            "SELECT id FROM {} WHERE id = $id",
            TursoAdapter::raw_table_name(&type_def)
        );
        for (id, row) in rows {
            let held = !self
                .db
                .query(
                    &exists_sql,
                    HashMap::from([("id".to_string(), Value::String(id.clone()))]),
                )
                .await
                .with_context(|| format!("reading the Turso row of '{id}'"))?
                .is_empty();
            let (op, params) = match row {
                Some(fields) if held => ("update", fields),
                Some(fields) => ("create", fields),
                None if held => (
                    "purge",
                    StorageEntity::from([("id".into(), Value::String(id.clone()))]),
                ),
                None => continue,
            };
            provider
                .execute_operation_with_origin(entity, op, params, EventOrigin::Loro)
                .await
                .map_err(|e| anyhow::anyhow!("{op} of '{id}' in its Turso table: {e}"))?;
        }
        Ok(())
    }
}
