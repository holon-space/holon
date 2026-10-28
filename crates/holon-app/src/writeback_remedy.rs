//! The remedies for an emptied vault file (`ConditionKind::VaultFileEmptied`):
//! fire the write-back net's manual `AcceptRemoval` or `RewriteFromStore`
//! for the document. The operation answers `{"accepted": <occurrence>}`;
//! the outcome follows when the net settles that occurrence.

use std::collections::HashMap;

use fluxdi::Injector;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeHint;
use holon_api::Value;
use holon_conditions::condition_profile::OpId;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;
use holon_filesystem::writeback_net;
use holon_net::exec::OccurrenceId;

const OPS: [(OpId, &str, &str, &str); 2] = [
    (
        OpId::AcceptRemoval,
        writeback_net::ACCEPT_REMOVAL,
        "Accept the removal",
        "Delete the document's blocks, as the emptied file says. NOT UNDOABLE when the store \
         did not change after the file was emptied; otherwise the store content goes back to \
         the file.",
    ),
    (
        OpId::RewriteFromStore,
        writeback_net::REWRITE_FROM_STORE,
        "Rewrite the file from the store",
        "Write the document's blocks back to the emptied file.",
    ),
];

fn descriptor(op: OpId, display_name: &str, description: &str) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: op.entity().into(),
        entity_short_name: op.entity().to_string(),
        id_column: String::new(),
        name: op.op().to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        required_params: vec![OperationParam {
            name: "doc".to_string(),
            type_hint: TypeHint::String,
            description: "The document of the emptied file".to_string(),
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

/// Serves the [`OPS`]. Resolves the org idle signal lazily: it is a member of
/// the provider set the dispatcher is built from.
pub struct WritebackRemedyProvider {
    injector: Injector,
    issued: std::sync::atomic::AtomicU64,
}

impl WritebackRemedyProvider {
    pub fn new(injector: Injector) -> Self {
        Self {
            injector,
            issued: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl OperationProvider for WritebackRemedyProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        OPS.iter()
            .map(|(op, _, name, description)| descriptor(*op, name, description))
            .collect()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        let (op, transition, _, _) = OPS
            .iter()
            .find(|(op, ..)| *entity_name == EntityName::new(op.entity()) && op_name == op.op())
            .ok_or_else(|| {
                format!("WritebackRemedyProvider: does not serve '{entity_name}::{op_name}'")
            })?;
        let doc = params
            .get("doc")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("{}: missing required parameter 'doc'", op.op()))?
            .to_string();
        let idle = self
            .injector
            .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
            .map_err(|e| format!("{}: this session has no org write-back: {e}", op.op()))?;
        let n = self
            .issued
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let occurrence = format!("{}:{doc}:{n}", op.op());
        idle.writeback_net()
            .fire(transition, &doc, OccurrenceId::caller(&occurrence))
            .map_err(|e| format!("{} for {doc}: {e}", op.op()))?;
        Ok(OperationResult::declared_irreversible(
            Vec::new(),
            "the write-back net owns the file; its outcome is a file write or a block removal",
        )
        .with_response(Value::Object(HashMap::from([(
            "accepted".to_string(),
            Value::String(occurrence),
        )]))))
    }
}
