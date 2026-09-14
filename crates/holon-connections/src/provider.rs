//! The one generic operation a configured list connection registers:
//! `remote_list_sync`.
//!
//! It performs ONE round — pull, reconcile, push — and hands the local writes
//! back as follow-up operations, so every mirrored row is written by the
//! declared type's own generic authority.
//!
//! No cadence lives here. The operation runs when something calls it.

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_core::storage::types::StorageEntity;

use crate::intent::local_intent_operation;
use crate::reconcile::RemoteListReconciler;
use crate::snapshot::LocalRowReader;
use crate::spec::CompiledListSync;
use crate::sync::RemoteListPeer;
use crate::sync::sync_once;

/// The operation name every list connection registers, under its own entity.
pub const REMOTE_LIST_SYNC: &str = "remote_list_sync";

/// A clock, so a round is reproducible from its inputs in a test and reads the
/// wall clock in production.
pub trait RoundClock: Send + Sync {
    /// Milliseconds since the epoch, scoping one round's command ids.
    fn now_ms(&self) -> i64;
}

pub struct SystemRoundClock;

impl RoundClock for SystemRoundClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

pub struct RemoteListOperations {
    compiled: Arc<CompiledListSync>,
    peer: Arc<dyn RemoteListPeer>,
    rows: Arc<dyn LocalRowReader>,
    clock: Arc<dyn RoundClock>,
    /// Stable per install, echoed on every commit. Not a secret.
    device_id: String,
    /// The connection this operation belongs to, for its error messages.
    connection: String,
}

impl RemoteListOperations {
    /// The connection this operation belongs to, for a wiring that wants to
    /// assert what it registered.
    pub fn connection(&self) -> &str {
        &self.connection
    }

    pub fn new(
        connection: impl Into<String>,
        compiled: Arc<CompiledListSync>,
        peer: Arc<dyn RemoteListPeer>,
        rows: Arc<dyn LocalRowReader>,
        clock: Arc<dyn RoundClock>,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            compiled,
            peer,
            rows,
            clock,
            device_id: device_id.into(),
            connection: connection.into(),
        }
    }
}

#[async_trait]
impl OperationProvider for RemoteListOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        let entity = &self.compiled.spec().entity;
        vec![OperationDescriptor {
            entity_name: EntityName::new(entity),
            entity_short_name: entity.clone(),
            name: REMOTE_LIST_SYNC.to_string(),
            display_name: format!("Sync {}", self.connection),
            description: "Exchange one round of changes with a remote list".to_string(),
            // No list parameter: the sidecar's tool `url` names the one list
            // this connection syncs.
            required_params: vec![],
            id_column: "id".to_string(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Internal,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        _: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        let entity = &self.compiled.spec().entity;
        // Compare through `EntityName`, which normalizes an underscored name to
        // its canonical hyphenated form: the dispatcher routes by the
        // normalized name, so a raw `&str` compare here rejects every real
        // dispatch.
        if *entity_name != EntityName::new(entity) || op_name != REMOTE_LIST_SYNC {
            return Err(format!(
                "the '{}' connection serves only {entity}/{REMOTE_LIST_SYNC}, not \
                 {entity_name}/{op_name}",
                self.connection
            )
            .into());
        }

        let outcome = sync_once(
            self.peer.as_ref(),
            self.rows.as_ref(),
            &RemoteListReconciler::new(self.compiled.clone()),
            &self.device_id,
            self.clock.now_ms(),
        )
        .await
        .map_err(|e| format!("{REMOTE_LIST_SYNC} on '{}': {e:#}", self.connection))?;

        let spec = self.compiled.spec();
        let follow_ups = outcome
            .local
            .iter()
            .map(|intent| local_intent_operation(spec, intent))
            .collect();
        // A completed exchange with another peer has no inverse: undoing it
        // would push the reverse commands at a list that has already moved on.
        Ok(
            OperationResult::declared_irreversible(Vec::new(), "a peer sync cannot be un-sent")
                .with_follow_ups(follow_ups),
        )
    }
}
