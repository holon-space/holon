//! Writing the declared-type rows a vault file owns.
//!
//! The file-sync controller sits below the operation dispatcher and holds only
//! block-shaped write seams, so an adapter that derives typed rows
//! (`holon_kitchen`'s `.cook` recipes) reaches the generic entity path through
//! this sink. Every write goes through the ONE shared dispatcher, one
//! `replace_scope` per file-owned row set, so vault ingest never becomes a
//! second writer of these tables and a reader sees a scope's whole old row set
//! or its whole new one.
//!
//! The dispatcher is resolved per call rather than held: it aggregates every
//! `OperationProvider`, one of which builds the file-sync controller this sink
//! is wired into, so eager resolution would close a DI cycle.

use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use async_trait::async_trait;
use fluxdi::Injector;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::StorageEntity;
use holon_api::TypeDefinition;
use holon_core::file_format::TypedRowSet;
use holon_core::file_format::TypedRowSink;
use holon_core::type_write_rule::ScopeReplacement;
use holon_profiles::TypeRegistry;

use crate::api::operation_dispatcher::AuthoredInput;
use crate::api::operation_dispatcher::OperationDispatcher;

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

    /// Refuse a row set whose type or owner column the registry does not
    /// declare: an unknown one is a wiring bug rather than input to interpret.
    async fn checked_type(&self, owned: &TypedRowSet) -> Result<TypeDefinition> {
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
        Ok(type_def)
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
            let type_def = self.checked_type(owned).await?;
            let entity = EntityName::new(type_def.name.clone());

            // Every row must carry the id the replacement below keys on, and
            // it must already NAME its entity: the file's keyspace is bare and
            // the scheme is added by the parse that produced these rows (the
            // plugin adapter renders each typed local id with
            // `EntityUri::from_segments`).
            // An unschemed id here means a producer skipped that parse, which
            // the dispatcher would refuse further down with less context.
            for row in &owned.rows {
                match row.get("id").and_then(|v| v.as_string()) {
                    Some(id) if EntityUri::schemed(id).is_some() => {}
                    Some(bare) => bail!(
                        "a {} row carries the unschemed id {bare:?}; a row producer must parse \
                         its file-local ids into entity references before they reach the write \
                         path",
                        owned.type_name
                    ),
                    None => bail!(
                        "a {} row carries no id — a file's rows must be keyed by the file itself",
                        owned.type_name
                    ),
                }
            }

            // The dispatcher parses a param's references by its descriptor, and
            // `rows` nests them, so each row is parsed as the create it amounts to.
            let dispatcher = self.dispatcher().await;
            for row in &owned.rows {
                dispatcher
                    .parse_entity_references_of(&entity, "create", row, &[])
                    .map_err(|e| anyhow::anyhow!("a {} row: {e}", owned.type_name))?;
            }
            let scope = ScopeReplacement::new(
                owned.owner_column.clone(),
                owned.owner_value.clone(),
                owned.rows.clone(),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            self.run(&entity, ScopeReplacement::OP, scope.params())
                .await
                .with_context(|| {
                    format!(
                        "replacing the {} rows {} = {:?}",
                        owned.type_name, owned.owner_column, owned.owner_value
                    )
                })?;
        }
        Ok(())
    }
}
