//! The operation-execution capability (ADR 0004 — "Turso is one of four").
//!
//! `OperationEngine` is the seam the frontend's mutation/operation path depends
//! on instead of the concrete Turso
//! [`BackendEngine`](crate::api::BackendEngine). It covers dispatching
//! operations, discovering which operations an entity supports, and undo/redo.
//! Operations are *not* fundamentally Turso-bound — they flow through the
//! [`OperationDispatcher`](crate::api::OperationDispatcher) and a per-session
//! undo stack — so a future no-Turso wiring can provide this capability over
//! the Loro consolidator. Today only `BackendEngine` implements
//! it; the frontend holds it as `Option<Arc<dyn OperationEngine>>` so a
//! no-Turso session reports the capability's absence as a typed fact rather
//! than panicking behind `engine()`.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::UndoOutcome;
use holon_api::Value;
pub use holon_api::operation_engine::OperationEngine;
use holon_core::OperationProvider;
use holon_core::Precondition;
use holon_core::UndoAction;
use holon_core::UndoEntry;
use holon_core::UndoStack;
use holon_core::UndoStateReader;
use holon_core::UndoStore;
use holon_core::storage::types::StorageEntity;
use holon_core::verify_precondition;
use tokio::sync::RwLock;

use crate::api::BackendEngine;
use crate::api::operation_dispatcher::OperationDispatcher;

#[async_trait]
impl OperationEngine for BackendEngine {
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        origin: OpOrigin,
    ) -> Result<Option<Value>> {
        BackendEngine::execute_operation(self, entity_name, op_name, params, origin).await
    }

    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        BackendEngine::available_operations(self, entity_name).await
    }

    async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        BackendEngine::has_operation(self, entity_name, op_name).await
    }

    async fn undo(&self) -> Result<UndoOutcome> {
        BackendEngine::undo(self).await
    }

    async fn redo(&self) -> Result<UndoOutcome> {
        BackendEngine::redo(self).await
    }

    async fn can_undo(&self) -> bool {
        BackendEngine::can_undo(self).await
    }

    async fn can_redo(&self) -> bool {
        BackendEngine::can_redo(self).await
    }
}

/// A backend-agnostic [`OperationEngine`] over a bare
/// [`OperationDispatcher`](crate::api::OperationDispatcher) plus a per-session
/// [`UndoStack`]. This is the operation capability for a no-Turso (Loro-only)
/// session: it carries the same dispatch + undo/redo logic as the Turso
/// [`BackendEngine`] but without any of Turso's query/CDC machinery, so a
/// session that registers Loro-native operation providers (e.g.
/// `LoroBlockOperations`) gets full mutation + undo support.
pub struct DispatchingOperationEngine {
    dispatcher: Arc<OperationDispatcher>,
    undo_stack: Arc<RwLock<UndoStack>>,
    /// Live-state reader for precondition (staleness) verification. `None` on a
    /// non-persistent (Loro-only) wiring — entries there carry empty
    /// preconditions, so nothing needs reading.
    reader: Option<Arc<dyn UndoStateReader>>,
    /// Snapshot persistence. `None` for an in-memory-only stack.
    store: Option<Arc<dyn UndoStore>>,
    seq: AtomicI64,
}

impl DispatchingOperationEngine {
    /// Build an in-memory engine over the given dispatcher (no persistence, no
    /// staleness reader). Used by Loro-only sessions whose reversible ops carry
    /// no field-level preconditions.
    pub fn new(dispatcher: Arc<OperationDispatcher>) -> Self {
        Self {
            dispatcher,
            undo_stack: Arc::new(RwLock::new(UndoStack::default())),
            reader: None,
            store: None,
            seq: AtomicI64::new(0),
        }
    }

    /// Build a persistent engine. Loads any prior stack snapshot from `store`
    /// and re-verifies preconditions against live state via `reader` at replay.
    pub async fn new_persistent(
        dispatcher: Arc<OperationDispatcher>,
        reader: Arc<dyn UndoStateReader>,
        store: Arc<dyn UndoStore>,
    ) -> Result<Self> {
        let (stack, seq) = match store.load().await? {
            Some(json) => {
                let stack: UndoStack = serde_json::from_str(&json)
                    .map_err(|e| anyhow::anyhow!("undo snapshot deserialize: {e}"))?;
                (stack, 1)
            }
            None => (UndoStack::default(), 0),
        };
        Ok(Self {
            dispatcher,
            undo_stack: Arc::new(RwLock::new(stack)),
            reader: Some(reader),
            store: Some(store),
            seq: AtomicI64::new(seq),
        })
    }

    /// Persist the current stack snapshot (no-op without a store). Fails loud
    /// on a write error — a dropped persist would silently lose history.
    async fn persist(&self) -> Result<()> {
        if let Some(store) = &self.store {
            let json = {
                let stack = self.undo_stack.read().await;
                serde_json::to_string(&*stack)
                    .map_err(|e| anyhow::anyhow!("undo snapshot serialize: {e}"))?
            };
            let seq = self.seq.fetch_add(1, Ordering::SeqCst);
            store.save(&json, seq).await?;
        }
        Ok(())
    }

    /// Dispatch a stored op verbatim (used for inverse/forward replay). Never
    /// pushes an undo entry — replays bypass the push path by construction.
    async fn replay(&self, op: &Operation) -> Result<()> {
        self.dispatcher
            .execute_operation(
                &op.entity_name,
                &op.op_name,
                op.params
                    .iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
                    .collect(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("undo/redo replay of '{}' failed: {e}", op.op_name))?;
        Ok(())
    }

    /// Verify a precondition against live state. With no reader wired (an
    /// in-memory, single-writer session that has no external-mutation surface)
    /// verification is skipped — disclosed, not a silent fake. When a reader is
    /// present a divergence is reported so the caller drops the entry loudly.
    async fn check_stale(&self, precondition: &Precondition) -> Result<Option<String>> {
        if precondition.is_empty() {
            return Ok(None);
        }
        match &self.reader {
            Some(reader) => verify_precondition(reader.as_ref(), precondition).await,
            None => {
                tracing::debug!(
                    "undo: no state reader wired — skipping precondition check (in-memory session)"
                );
                Ok(None)
            }
        }
    }
}

#[async_trait]
impl OperationEngine for DispatchingOperationEngine {
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        origin: OpOrigin,
    ) -> Result<Option<Value>> {
        let forward_op = Operation::new(
            entity_name.clone(),
            op_name,
            op_name,
            params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        );

        let result = self
            .dispatcher
            .execute_operation(entity_name, op_name, params)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Operation '{op_name}' on entity '{entity_name}' failed: {e}")
            })?;

        // Ruling #2: an unclassified result is a loud error, never a silent
        // no-entry.
        if result.undo.is_undeclared() {
            anyhow::bail!(
                "operation '{op_name}' on '{entity_name}' returned an Undeclared undo \
                 classification — provider must return Undo(..) or DeclaredIrreversible(..)"
            );
        }

        // Ruling #1: only User-origin operations push undo entries. Rule/Sync/
        // Ingest ops mutate state but never enter the user history.
        if origin.is_user() {
            if let UndoAction::Undo(inverse_op) = &result.undo {
                let entry = UndoEntry {
                    ops: vec![forward_op],
                    inverse_ops: vec![inverse_op.clone()],
                    origin: OpOrigin::User,
                    group_id: 0,
                    precondition: Precondition::forward(&result.changes),
                    redo_precondition: Precondition::inverse(&result.changes),
                };
                self.undo_stack.write().await.push(entry);
                self.persist().await?;
            }
        }

        Ok(result.response)
    }

    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.dispatcher
            .operations()
            .into_iter()
            .filter(|op| op.entity_name == entity_name)
            .collect()
    }

    async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        self.dispatcher
            .operations()
            .into_iter()
            .any(|op| op.entity_name == entity_name && op.name == op_name)
    }

    async fn undo(&self) -> Result<UndoOutcome> {
        let entry = match self.undo_stack.read().await.peek_undo().cloned() {
            Some(e) => e,
            None => return Ok(UndoOutcome::Empty),
        };

        // Ruling #4: verify BEFORE replaying; a stale entry is dropped loudly,
        // never silently skipped to the next entry.
        if let Some(reason) = self.check_stale(&entry.precondition).await? {
            self.undo_stack.write().await.drop_undo();
            self.persist().await?;
            tracing::error!("undo: dropped stale entry ({reason})");
            return Ok(UndoOutcome::StaleDropped { reason });
        }

        for op in &entry.inverse_ops {
            self.replay(op).await?;
        }
        self.undo_stack.write().await.commit_undo();
        self.persist().await?;
        Ok(UndoOutcome::Applied)
    }

    async fn redo(&self) -> Result<UndoOutcome> {
        let entry = match self.undo_stack.read().await.peek_redo().cloned() {
            Some(e) => e,
            None => return Ok(UndoOutcome::Empty),
        };

        if let Some(reason) = self.check_stale(&entry.redo_precondition).await? {
            self.undo_stack.write().await.drop_redo();
            self.persist().await?;
            tracing::error!("redo: dropped stale entry ({reason})");
            return Ok(UndoOutcome::StaleDropped { reason });
        }

        for op in &entry.ops {
            self.replay(op).await?;
        }
        self.undo_stack.write().await.commit_redo();
        self.persist().await?;
        Ok(UndoOutcome::Applied)
    }

    async fn can_undo(&self) -> bool {
        self.undo_stack.read().await.can_undo()
    }

    async fn can_redo(&self) -> bool {
        self.undo_stack.read().await.can_redo()
    }
}
