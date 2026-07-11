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
use holon_api::HistoryEvent;
use holon_api::HistoryStore;
use holon_api::OpOrigin;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::PROVENANCE_PROPERTY;
use holon_api::ProvenanceStamp;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::clock::Clock;
use holon_api::clock::SystemClock;
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
    /// Wall-clock authority for provenance stamps (ADR 0024 P8 / C2a). Defaults
    /// to [`SystemClock`]; a test wiring overrides it via [`Self::with_clock`]
    /// so stamp timestamps are deterministic. Never a raw `SystemTime::now`.
    clock: Arc<dyn Clock>,
    /// Optional op/effect history relation (C2b). When wired, every successful
    /// op appends its field deltas to the stream. `None` on a wiring without a
    /// query substrate (the block `_provenance` stamp still lands regardless).
    history: Option<Arc<dyn HistoryStore>>,
    /// Read capability for `instantiate_template`
    /// (docs/Proposals/Templating-2026-07-12.md). `None` on a wiring without a
    /// queryable block projection — the operation then fails loud, disclosed.
    template_source: Option<Arc<dyn crate::api::template_source::TemplateSource>>,
}

/// The engine-level compound operation name: expands into `create` ops routed
/// through whatever provider owns `block` creation in this session's wiring.
const INSTANTIATE_TEMPLATE_OP: &str = "instantiate_template";

/// Op names whose params are a block field-map written to `block_raw`, so an
/// injected `_provenance` property lands in the row's `properties` JSON through
/// the existing "unknown fields pack into properties" provider path (zero
/// provider edits). These are the *authoring* ops the vision cares about
/// (rule/agent-created and updated blocks). Chord ops (split/join/move) and the
/// single-field `set_field` shape are covered by the history relation (C2b),
/// not by this property stamp.
const PROVENANCE_STAMPED_OPS: &[&str] = &["create", "update"];

/// Inject the `_provenance` property into an authoring op's params. Pure and
/// clock-free (the timestamp is passed in) so it is directly unit-testable.
fn stamp_params(
    op_name: &str,
    mut params: StorageEntity,
    origin: &OpOrigin,
    now_millis: i64,
) -> StorageEntity {
    if PROVENANCE_STAMPED_OPS.contains(&op_name) {
        let stamp = ProvenanceStamp::from_origin(origin, now_millis);
        params.insert(Arc::from(PROVENANCE_PROPERTY), stamp.to_value());
    }
    params
}

/// Build the history events for one completed op — one per field delta, with
/// provenance ids derived from `origin` via [`ProvenanceStamp`] so the stream
/// and the block stamp never disagree. Pure and clock-free (unit-testable).
fn history_events_for(
    op_name: &str,
    origin: &OpOrigin,
    changes: &[holon_core::FieldDelta],
    now_millis: i64,
) -> Vec<HistoryEvent> {
    let stamp = ProvenanceStamp::from_origin(origin, now_millis);
    changes
        .iter()
        .map(|delta| HistoryEvent {
            block_id: delta.entity_id.clone(),
            op_name: op_name.to_string(),
            origin: stamp.origin.clone(),
            transition_id: stamp.transition_id.clone(),
            session_id: stamp.session_id.clone(),
            tool_call_id: stamp.tool_call_id.clone(),
            field: Some(delta.field.clone()),
            new_value: Some(render_value(&delta.new_value)),
            at_millis: stamp.at_millis,
        })
        .collect()
}

/// Render a field value to the `new_value` string the history relation stores.
/// Scalars render naturally; structured values fall back to JSON so a query can
/// still match on them (disclosed, lossless-enough for state-transition
/// counts).
fn render_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::DateTime(s) | Value::Json(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
        }
    }
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
            clock: Arc::new(SystemClock),
            history: None,
            template_source: None,
        }
    }

    /// Override the provenance-stamp clock (test determinism). Production keeps
    /// the [`SystemClock`] default so stamps carry real wall-clock time.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Wire the op/effect history relation (C2b). Every successful op then
    /// appends its field deltas to the stream.
    pub fn with_history_store(mut self, history: Arc<dyn HistoryStore>) -> Self {
        self.history = Some(history);
        self
    }

    /// Wire the template read capability, enabling the engine-level
    /// `instantiate_template` operation on the `block` entity.
    pub fn with_template_source(
        mut self,
        source: Arc<dyn crate::api::template_source::TemplateSource>,
    ) -> Self {
        self.template_source = Some(source);
        self
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
            clock: Arc::new(SystemClock),
            history: None,
            template_source: None,
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

    /// Inject the `_provenance` stamp into an authoring op's params. For
    /// non-authoring ops (or a `set_field`/chord shape) the params pass through
    /// unchanged — those are covered by the C2b history relation, not the block
    /// property stamp. The timestamp is read from the injected clock seam.
    fn stamp_provenance(
        &self,
        op_name: &str,
        params: StorageEntity,
        origin: &OpOrigin,
    ) -> StorageEntity {
        stamp_params(op_name, params, origin, self.clock.now_millis())
    }

    /// Append one [`HistoryEvent`] per field delta of a completed op to the
    /// history relation. Provenance ids are derived from `origin` (reusing the
    /// same [`ProvenanceStamp`] extraction as the block stamp, so the two
    /// surfaces never disagree). The timestamp is the injected clock.
    async fn record_history(
        &self,
        history: &dyn HistoryStore,
        op_name: &str,
        origin: &OpOrigin,
        changes: &[holon_core::FieldDelta],
    ) -> Result<()> {
        for event in history_events_for(op_name, origin, changes, self.clock.now_millis()) {
            history.record(event).await?;
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

    /// Execute `instantiate_template`
    /// (docs/Proposals/Templating-2026-07-12.md): load the template
    /// subtree, build the deterministic instantiation plan, and dispatch
    /// one `create` per node through the NORMAL operation path —
    /// so C2a provenance stamping, the C2b history relation, and per-create
    /// undo classification all apply unchanged. Returns the instance root id.
    async fn run_instantiate_template(
        &self,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<Option<Value>> {
        use crate::core::template_instantiation::InstantiateRequest;
        use crate::core::template_instantiation::plan_instantiation;

        let source = self.template_source.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "instantiate_template requires a template source — not wired in this session (no \
                 queryable block projection)"
            )
        })?;
        let request = InstantiateRequest::from_params(params)?;
        let nodes = source.load_subtree(&request.template_id).await?;
        let plan = plan_instantiation(&nodes, &request)?;

        let block_entity = EntityName::new("block");
        for create_params in plan.creates {
            // Boxed for async recursion (this IS execute_operation calling
            // itself one level deep; a nested instantiate cannot occur —
            // the plan only emits `create`).
            Box::pin(OperationEngine::execute_operation(
                self,
                &block_entity,
                "create",
                create_params,
                origin.clone(),
            ))
            .await?;
        }
        Ok(Some(Value::String(plan.root_id)))
    }

    /// The synthetic descriptor advertising the engine-level
    /// `instantiate_template` op so MCP/UI discover it like any provider op.
    fn instantiate_template_descriptor() -> OperationDescriptor {
        use holon_api::render_types::TypeHint;
        let param = |name: &str, hint: TypeHint, description: &str| holon_api::OperationParam {
            name: name.to_string(),
            type_hint: hint,
            description: description.to_string(),
        };
        OperationDescriptor {
            entity_name: EntityName::new("block"),
            entity_short_name: "block".to_string(),
            id_column: "id".to_string(),
            name: INSTANTIATE_TEMPLATE_OP.to_string(),
            display_name: "Instantiate template".to_string(),
            description: "Deep-copy a template block subtree under target_parent, substituting \
                          {{var}} slots from bindings. Instance ids are deterministic per \
                          (template_id, context_key), so rule re-fires converge. Fails loud on \
                          missing bindings."
                .to_string(),
            required_params: vec![
                param(
                    "template_id",
                    TypeHint::String,
                    "Id of the template root block (must carry the 'template' property)",
                ),
                param(
                    "target_parent",
                    TypeHint::String,
                    "Block id the instance root is created under",
                ),
                param(
                    "context_key",
                    TypeHint::String,
                    "Idempotence key: rules pass their firing key; manual callers a fresh key",
                ),
            ],
            affected_fields: vec![],
            param_mappings: vec![],
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        }
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
        // Engine-level compound: expand a template instantiation into ordinary
        // `create` dispatches (each re-enters this method and gets stamping /
        // history / undo classification like any other op).
        if op_name == INSTANTIATE_TEMPLATE_OP && entity_name.as_str() == "block" {
            return self.run_instantiate_template(&params, &origin).await;
        }

        // Provenance stamping (ADR 0024 P8 / C2a): the dispatcher drops `origin`
        // before the write, so this is the last place holding it. For authoring
        // ops we inject a `_provenance` property into the params; it travels as
        // ordinary block-field data down the existing write path and lands in
        // `block_raw.properties`, with no provider edits.
        let params = self.stamp_provenance(op_name, params, &origin);

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

        // History relation (ADR 0024 P8 / C2b): append the op's field deltas to
        // the queryable op/effect stream. This is the append-only complement to
        // the block `_provenance` stamp — it captures set_field/delete/etc. that
        // the property stamp does not, and answers "postponed N times". Fails
        // loud (the relation is rebuildable but errors are never swallowed).
        if let Some(history) = &self.history {
            self.record_history(history.as_ref(), op_name, &origin, &result.changes)
                .await?;
        }

        Ok(result.response)
    }

    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        let mut ops: Vec<OperationDescriptor> = self
            .dispatcher
            .operations()
            .into_iter()
            .filter(|op| op.entity_name == entity_name)
            .collect();
        if entity_name == "block" && self.template_source.is_some() {
            ops.push(Self::instantiate_template_descriptor());
        }
        ops
    }

    async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        if entity_name == "block"
            && op_name == INSTANTIATE_TEMPLATE_OP
            && self.template_source.is_some()
        {
            return true;
        }
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

#[cfg(test)]
mod instantiate_template_tests {
    use std::collections::HashMap;

    use super::*;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::di::test_helpers::create_test_engine_with_providers;
    use crate::storage::BLOCK_WRITE_TABLE;

    /// A test engine with the `block` SQL operation provider registered,
    /// mirroring the `action_watcher` test harness. `BackendEngine::new` wires
    /// the Turso [`TemplateSource`] automatically.
    async fn block_engine() -> Arc<BackendEngine> {
        create_test_engine_with_providers(":memory:".into(), |module| {
            module.with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                ))
            })
        })
        .await
        .unwrap()
    }

    async fn create_block(engine: &BackendEngine, fields: &[(&str, Value)]) {
        let params: StorageEntity = fields
            .iter()
            .map(|(k, v)| (Arc::from(*k), v.clone()))
            .collect();
        engine
            .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::User)
            .await
            .unwrap();
    }

    /// Seed: target parent + a two-level template (root with marker props and
    /// a `{{date}}` slot, one child with a `{{mood}}` slot and a link mark).
    async fn seed_template(engine: &BackendEngine) {
        create_block(
            engine,
            &[
                ("id", Value::String("block:target".into())),
                ("content", Value::String("Target".into())),
            ],
        )
        .await;
        create_block(
            engine,
            &[
                ("id", Value::String("block:tpl".into())),
                ("content", Value::String("{{date}}".into())),
                ("template", Value::String("daily".into())),
                ("template_vars", Value::String("date, mood=neutral".into())),
            ],
        )
        .await;
        // Child content "see {{date}} now" with a bold mark on "see" (0..3).
        create_block(
            engine,
            &[
                ("id", Value::String("block:tpl-c1".into())),
                ("parent_id", Value::String("block:tpl".into())),
                ("content", Value::String("see {{date}} now".into())),
                (
                    "marks",
                    Value::String(r#"[{"start":0,"end":3,"kind":"Bold"}]"#.into()),
                ),
            ],
        )
        .await;
    }

    fn instantiate_params(context_key: &str, bindings: &[(&str, &str)]) -> StorageEntity {
        let mut params = StorageEntity::new();
        params.insert("template_id".into(), Value::String("block:tpl".into()));
        params.insert("target_parent".into(), Value::String("block:target".into()));
        params.insert("context_key".into(), Value::String(context_key.into()));
        if !bindings.is_empty() {
            params.insert(
                "bindings".into(),
                Value::Object(
                    bindings
                        .iter()
                        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                        .collect(),
                ),
            );
        }
        params
    }

    async fn instance_roots(engine: &BackendEngine) -> Vec<StorageEntity> {
        engine
            .db_handle()
            .query(
                "SELECT * FROM block_raw WHERE parent_id = 'block:target' AND id != 'block:tpl' \
                 ORDER BY id",
                HashMap::new(),
            )
            .await
            .unwrap()
    }

    fn str_field<'a>(row: &'a StorageEntity, key: &str) -> &'a str {
        match row.get(key) {
            Some(Value::String(s)) => s,
            other => panic!("field '{key}': expected string, got {other:?}"),
        }
    }

    /// Read a row's `properties` as a JSON object regardless of whether the
    /// query hands it back as a raw JSON string or an already-parsed
    /// `Value::Object` (`DbHandle::query` may do either).
    fn props_of(row: &StorageEntity) -> serde_json::Value {
        match row.get("properties") {
            Some(Value::String(s)) | Some(Value::Json(s)) => {
                serde_json::from_str(s).expect("properties is valid JSON")
            }
            Some(obj @ Value::Object(_)) => {
                serde_json::to_value(obj).expect("Value serialization is total")
            }
            other => panic!("properties: expected object or JSON string, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn instantiate_is_idempotent_per_context_key_and_substitutes() {
        let engine = block_engine().await;
        seed_template(&engine).await;
        let entity = EntityName::new("block");

        let root_id = engine
            .execute_operation(
                &entity,
                "instantiate_template",
                instantiate_params("2026-07-12", &[("date", "2026-07-12")]),
                OpOrigin::Rule {
                    transition_id: "rule:test-template".into(),
                },
            )
            .await
            .unwrap();
        let Some(Value::String(root_id)) = root_id else {
            panic!("instantiate_template must return the instance root id");
        };

        // Same context key again → converged, still exactly one instance.
        engine
            .execute_operation(
                &entity,
                "instantiate_template",
                instantiate_params("2026-07-12", &[("date", "2026-07-12")]),
                OpOrigin::Rule {
                    transition_id: "rule:test-template".into(),
                },
            )
            .await
            .unwrap();
        let roots = instance_roots(&engine).await;
        assert_eq!(
            roots.len(),
            1,
            "re-fire with same context_key must converge"
        );
        let root = &roots[0];
        assert_eq!(str_field(root, "id"), root_id);
        assert_eq!(str_field(root, "content"), "2026-07-12", "substituted");
        let props = props_of(root);
        assert_eq!(props["instance_of"], "block:tpl");
        assert!(props.get("template").is_none(), "marker stripped");
        assert_eq!(
            props["_provenance"]["origin"], "rule",
            "creates carry rule provenance (C2a)"
        );

        // The child: substituted content, marks survived as real marks.
        let children = engine
            .db_handle()
            .query(
                &format!(
                    "SELECT * FROM block_raw WHERE parent_id = '{}'",
                    root_id.replace('\'', "''")
                ),
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(str_field(&children[0], "content"), "see 2026-07-12 now");
        let marks_json = match children[0].get("marks") {
            Some(Value::String(s)) | Some(Value::Json(s)) => s.clone(),
            Some(arr @ Value::Array(_)) => serde_json::to_string(arr).unwrap(),
            other => panic!("marks: expected array or JSON string, got {other:?}"),
        };
        let marks = holon_api::marks_from_json(&marks_json).unwrap();
        assert_eq!((marks[0].start, marks[0].end), (0, 3), "bold mark intact");

        // A different context key mints a second, distinct instance.
        engine
            .execute_operation(
                &entity,
                "instantiate_template",
                instantiate_params("2026-07-13", &[("date", "2026-07-13")]),
                OpOrigin::Rule {
                    transition_id: "rule:test-template".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            instance_roots(&engine).await.len(),
            2,
            "a new context_key is a new instance"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_binding_fails_loud_and_creates_nothing() {
        let engine = block_engine().await;
        seed_template(&engine).await;

        let err = engine
            .execute_operation(
                &EntityName::new("block"),
                "instantiate_template",
                instantiate_params("k1", &[]),
                OpOrigin::User,
            )
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("missing bindings"), "got: {msg}");
        assert!(msg.contains("date"), "got: {msg}");
        assert_eq!(
            instance_roots(&engine).await.len(),
            0,
            "failed instantiation must create nothing"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn advertised_in_available_operations() {
        let engine = block_engine().await;
        let ops = OperationEngine::available_operations(engine.as_ref(), "block").await;
        assert!(
            ops.iter().any(|op| op.name == "instantiate_template"),
            "instantiate_template must be discoverable (MCP/UI)"
        );
        assert!(
            OperationEngine::has_operation(engine.as_ref(), "block", "instantiate_template").await
        );
    }
}

#[cfg(test)]
mod provenance_stamp_tests {
    use super::*;

    fn params_with(fields: &[(&str, Value)]) -> StorageEntity {
        fields
            .iter()
            .map(|(k, v)| (Arc::from(*k), v.clone()))
            .collect()
    }

    #[test]
    fn create_op_gets_rule_provenance_stamped() {
        let params = params_with(&[("content", Value::String("hi".into()))]);
        let origin = OpOrigin::Rule {
            transition_id: "rule:journal".into(),
        };
        let stamped = stamp_params("create", params, &origin, 1234);

        let prov = stamped
            .get(PROVENANCE_PROPERTY)
            .expect("create op must carry a _provenance property");
        let parsed = ProvenanceStamp::from_value(prov).unwrap();
        assert_eq!(parsed.origin, "rule");
        assert_eq!(parsed.transition_id.as_deref(), Some("rule:journal"));
        assert_eq!(parsed.at_millis, 1234);
        // Original field preserved.
        assert!(stamped.contains_key("content"));
    }

    #[test]
    fn update_op_gets_agent_provenance_stamped() {
        let origin = OpOrigin::Agent {
            session_id: "mcp-session:s".into(),
            tool_call_id: "tool-call:c".into(),
        };
        let stamped = stamp_params("update", StorageEntity::default(), &origin, 7);
        let parsed =
            ProvenanceStamp::from_value(stamped.get(PROVENANCE_PROPERTY).unwrap()).unwrap();
        assert_eq!(parsed.origin, "agent");
        assert_eq!(parsed.session_id.as_deref(), Some("mcp-session:s"));
        assert_eq!(parsed.tool_call_id.as_deref(), Some("tool-call:c"));
    }

    #[test]
    fn non_authoring_ops_are_not_stamped() {
        for op in ["set_field", "move_block", "split_block", "delete", "focus"] {
            let stamped = stamp_params(op, StorageEntity::default(), &OpOrigin::User, 1);
            assert!(
                !stamped.contains_key(PROVENANCE_PROPERTY),
                "op '{op}' must not be provenance-stamped (covered by the history relation)"
            );
        }
    }

    fn delta(entity: &str, field: &str, new_value: Value) -> holon_core::FieldDelta {
        holon_core::FieldDelta {
            entity_id: entity.to_string(),
            field: field.to_string(),
            old_value: Value::Null,
            new_value,
        }
    }

    #[test]
    fn history_events_carry_origin_and_deltas() {
        let changes = vec![
            delta("A", "status", Value::String("postponed".into())),
            delta("A", "count", Value::Integer(7)),
        ];
        let origin = OpOrigin::Rule {
            transition_id: "rule:postpone".into(),
        };
        let events = history_events_for("set_field", &origin, &changes, 999);

        assert_eq!(events.len(), 2, "one event per field delta");
        assert_eq!(events[0].block_id, "A");
        assert_eq!(events[0].op_name, "set_field");
        assert_eq!(events[0].origin, "rule");
        assert_eq!(events[0].transition_id.as_deref(), Some("rule:postpone"));
        assert_eq!(events[0].field.as_deref(), Some("status"));
        assert_eq!(events[0].new_value.as_deref(), Some("postponed"));
        assert_eq!(events[0].at_millis, 999);
        // Non-string values render for query matching.
        assert_eq!(events[1].new_value.as_deref(), Some("7"));
    }

    #[test]
    fn history_events_carry_agent_identity() {
        let changes = vec![delta("B", "content", Value::String("hi".into()))];
        let origin = OpOrigin::Agent {
            session_id: "mcp-session:s".into(),
            tool_call_id: "tool-call:c".into(),
        };
        let events = history_events_for("create", &origin, &changes, 1);
        assert_eq!(events[0].origin, "agent");
        assert_eq!(events[0].session_id.as_deref(), Some("mcp-session:s"));
        assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-call:c"));
    }
}
