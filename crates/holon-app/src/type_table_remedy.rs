//! The remedy for a refused type table (`ConditionKind::TypeTableRefused`):
//! serve the type from a table created from its declaration, then drop the
//! tables Holon recorded as its quarantines, with their rows, in this session.

use std::sync::Arc;

use fluxdi::Injector;
use holon::di::schema_providers::UnservedTypes;
use holon_api::ConditionBus;
use holon_api::ConditionKey;
use holon_api::ConditionKind;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeHint;
use holon_api::condition_profile::OpId;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;
use holon_profiles::TypeRegistry;
use holon_turso::turso_adapter::TursoAdapter;

const OP: OpId = OpId::DropRefusedTypeTable;

pub fn drop_refused_table_descriptor() -> OperationDescriptor {
    OperationDescriptor {
        entity_name: OP.entity().into(),
        entity_short_name: OP.entity().to_string(),
        id_column: String::new(),
        name: OP.op().to_string(),
        display_name: "Drop the refused table".to_string(),
        description: "Drop the quarantined tables of a type this session does not serve, with \
                      all their rows, and serve the type from a new empty table. NOT UNDOABLE: \
                      the rows are deleted."
            .to_string(),
        required_params: vec![OperationParam {
            name: "type".to_string(),
            type_hint: TypeHint::String,
            description: "The refused type's name".to_string(),
        }],
        optional_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Global,
        // Reached from the condition's remedy slot, never from a block menu.
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::External,
        },
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        trigger: None,
        bound_params: Default::default(),
        marking_delta: holon_api::marking::MarkingDelta::Static { kinds: vec![] },
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Declared {
            reads: vec![],
            emits: vec![],
        },
    }
}

/// Serves [`drop_refused_table_descriptor`]. Resolves its collaborators
/// lazily: it is a member of the provider set the dispatcher is built from.
pub struct TypeTableRemedyProvider {
    injector: Injector,
}

impl TypeTableRemedyProvider {
    pub fn new(injector: Injector) -> Self {
        Self { injector }
    }
}

#[async_trait::async_trait]
impl OperationProvider for TypeTableRemedyProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![drop_refused_table_descriptor()]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if *entity_name != EntityName::new(OP.entity()) || op_name != OP.op() {
            return Err(format!(
                "TypeTableRemedyProvider: advertises only '{}::{}', got '{entity_name}::{op_name}'",
                OP.entity(),
                OP.op()
            )
            .into());
        }
        let type_name = params
            .get("type")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("{}: missing required parameter 'type'", OP.op()))?
            .to_string();

        let unserved = self.injector.resolve_async::<UnservedTypes>().await;
        match unserved.condition(&type_name) {
            Some(ConditionKind::TYPE_TABLE_REFUSED) => {}
            Some(_) => {
                return Err(format!(
                    "{}: {}; no quarantined table holds its rows",
                    OP.op(),
                    unserved
                        .explanation(&type_name)
                        .expect("an unserved type has an explanation")
                )
                .into());
            }
            None => {
                return Err(format!(
                    "{}: type '{type_name}' is served or not declared; no quarantined table \
                     holds its rows",
                    OP.op()
                )
                .into());
            }
        }
        let type_def = self
            .injector
            .resolve_async::<TypeRegistry>()
            .await
            .get(&type_name)
            .ok_or_else(|| format!("{}: no type '{type_name}' is declared", OP.op()))?;
        let db = self
            .injector
            .resolve_async::<dyn holon::di::DbHandleProvider>()
            .await
            .handle();
        let dispatcher = self
            .injector
            .resolve_async::<holon::api::operation_dispatcher::OperationDispatcher>()
            .await;

        TursoAdapter::register(&type_def, &db).await?;
        holon_turso::table_shape::drop_quarantined(&db, &TursoAdapter::raw_table_name(&type_def))
            .await?;
        holon::core::type_declaration::derive_write_authority(&type_def, &db, &dispatcher)?;
        holon::core::type_declaration::register_companion_operations(&type_name, &db, &dispatcher)?;
        unserved.remove(&type_name);
        self.injector
            .resolve_async::<Arc<ConditionBus>>()
            .await
            .clear(&ConditionKey {
                subject: type_name,
                kind: ConditionKind::TYPE_TABLE_REFUSED,
            });

        Ok(OperationResult::declared_irreversible(
            Vec::new(),
            "dropping a table deletes its rows; nothing restores them",
        ))
    }
}
