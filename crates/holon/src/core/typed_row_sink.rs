//! Writing the declared-type rows a vault file owns.
//!
//! The file-sync controller sits below the operation dispatcher and holds only
//! block-shaped write seams, so an adapter that derives typed rows
//! (`holon_kitchen`'s `.cook` recipes) reaches the generic entity path through
//! this sink. Every write goes through the ONE shared dispatcher — the same
//! `create` / `delete` an MCP client or a keybinding would run — so vault
//! ingest never becomes a second writer of these tables.
//!
//! The dispatcher is resolved per call rather than held: it aggregates every
//! `OperationProvider`, one of which builds the file-sync controller this sink
//! is wired into, so eager resolution would close a DI cycle.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use async_trait::async_trait;
use fluxdi::Injector;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::TypedRowSet;
use holon_core::file_format::TypedRowSink;
use holon_profiles::TypeRegistry;

use crate::api::operation_dispatcher::AuthoredInput;
use crate::api::operation_dispatcher::OperationDispatcher;
use crate::storage::turso::DbHandle;

pub struct DispatchingTypedRowSink {
    injector: Injector,
}

impl DispatchingTypedRowSink {
    pub fn new(injector: Injector) -> Self {
        Self { injector }
    }

    async fn dispatcher(&self) -> Arc<OperationDispatcher> {
        self.injector.resolve_async::<OperationDispatcher>().await
    }

    async fn types(&self) -> Arc<TypeRegistry> {
        self.injector.resolve_async::<TypeRegistry>().await
    }

    fn db_handle(&self) -> DbHandle {
        self.injector
            .resolve::<dyn crate::di::DbHandleProvider>()
            .handle()
    }

    /// Refuse a row set whose type or owner column the registry does not
    /// declare: both are spliced into SQL below, and an unknown one is a wiring
    /// bug rather than input to interpret.
    async fn checked_entity(&self, owned: &TypedRowSet) -> Result<EntityName> {
        let type_def = self.types().await.get(&owned.type_name).ok_or_else(|| {
            anyhow::anyhow!(
                "no type named {:?} is registered, so its rows have nowhere to land",
                owned.type_name
            )
        })?;
        if !type_def.fields.iter().any(|f| f.name == owned.owner_column) {
            bail!(
                "type {:?} declares no field {:?} — an adapter can only scope its rows to a file \
                 through a column the type actually has. Declared: {:?}",
                owned.type_name,
                owned.owner_column,
                type_def.fields.iter().map(|f| &f.name).collect::<Vec<_>>(),
            );
        }
        Ok(EntityName::new(owned.type_name.clone()))
    }

    /// Ids of the rows `owner_column` currently scopes to this file.
    async fn ids_in_scope(&self, owned: &TypedRowSet, entity: &EntityName) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT id FROM {}_raw WHERE {} = '{}'",
            entity.table_name(),
            owned.owner_column,
            owned.owner_value.replace('\'', "''"),
        );
        let rows = self
            .db_handle()
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("reading the rows already scoped to this file: {e}"))?;
        rows.iter()
            .map(|row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "a {} row has no id — its keyspace is broken",
                            owned.type_name
                        )
                    })
            })
            .collect()
    }

    async fn run(&self, entity: &EntityName, op: &str, params: StorageEntity) -> Result<()> {
        // `Ingest`, not `User`: the file stays authoritative and re-derives
        // these rows, so they carry no undo entry.
        self.dispatcher()
            .await
            .execute_operation_with_provenance(
                entity,
                op,
                params,
                AuthoredInput::Verbatim,
                OpOrigin::Ingest,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{} {op}: {e}", entity.as_str()))?;
        Ok(())
    }
}

#[async_trait]
impl TypedRowSink for DispatchingTypedRowSink {
    async fn replace_typed_rows(&self, sets: &[TypedRowSet]) -> Result<()> {
        for owned in sets {
            let entity = self.checked_entity(owned).await?;

            // Every row must carry the id the replacement below keys on.
            for row in &owned.rows {
                if row.get("id").and_then(|v| v.as_string()).is_none() {
                    bail!(
                        "a {} row carries no id — a file's rows must be keyed by the file itself",
                        owned.type_name
                    );
                }
            }

            // Retire the whole scope, then write the parse. The file is
            // authoritative for it, so "what the file no longer says" and "what
            // the file changed" are one operation — and a create whose id names
            // a LIVE row is refused by the write path's recognition check, so
            // there is no in-place upsert to prefer over this.
            for stale in self.ids_in_scope(owned, &entity).await? {
                let mut params = StorageEntity::new();
                params.insert("id".into(), Value::String(stale.clone()));
                self.run(&entity, "delete", params)
                    .await
                    .with_context(|| format!("retiring the {} row {stale}", owned.type_name))?;
            }

            for row in &owned.rows {
                self.run(&entity, "create", row.clone())
                    .await
                    .with_context(|| format!("writing a {} row", owned.type_name))?;
            }
        }
        Ok(())
    }
}
