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
use anyhow::bail;
use async_trait::async_trait;
use holon_api::ACCEPT_PROPOSAL_OP;
use holon_api::EntityName;
use holon_api::HistoryEvent;
use holon_api::HistoryStore;
use holon_api::OpOrigin;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::PROPOSAL_PROPERTY;
use holon_api::PROPOSALS_ROOT_ID;
use holon_api::PROPOSED_BY_PROPERTY;
use holon_api::PROVENANCE_PROPERTY;
use holon_api::ProposalRecord;
use holon_api::ProposalStatus;
use holon_api::ProvenanceStamp;
use holon_api::REJECT_PROPOSAL_OP;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::clock::Clock;
use holon_api::clock::SystemClock;
use holon_api::effect_id::FiringKey;
use holon_api::effect_id::deterministic_proposal_id;
use holon_api::entity_uri::EntityUri;
pub use holon_api::operation_engine::OperationEngine;
use holon_core::FieldDelta;
use holon_core::OperationProvider;
use holon_core::Precondition;
use holon_core::UndoAction;
use holon_core::UndoEntry;
use holon_core::UndoStack;
use holon_core::UndoStateReader;
use holon_core::UndoStore;
use holon_core::storage::types::StorageEntity;
use holon_core::verify_precondition;
use holon_profiles::trust::TrustDecision;
use holon_profiles::trust::TrustPolicy;
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
    /// Trust policy (VisionGapAnalysis C5): decides per (origin, entity, op)
    /// whether a dispatch executes against canonical state or is coerced into
    /// a proposal emission. Defaults to [`TrustPolicy::trust_all`] — the gate
    /// is a no-op until a policy is configured.
    trust_policy: Arc<TrustPolicy>,
}

/// The engine-level compound operation name: expands into `create` ops routed
/// through whatever provider owns `block` creation in this session's wiring.
/// Single spelling shared with the frontend picker (`holon_api::template`).
use holon_api::INSTANTIATE_TEMPLATE_OP;

/// The engine-level compound that turns a block into a page (Option B,
/// `docs/Plans/BlockToPageTransform-Options-2026-07-17.md`): mint a new page,
/// move the origin's content + children onto it, leave a `[[page]]` link
/// behind, and re-point inbound backlinks. Composed from ordinary invertible
/// ops so the whole thing is ONE reversible [`UndoEntry`].
const CONVERT_BLOCK_TO_PAGE_OP: &str = "convert_block_to_page";

/// Re-parse a plan's `page_id` string into an `EntityUri` for the `[[P]]` link
/// mark. The id crosses the dispatch boundary as a plan `Value` string (the
/// planner minted it via `PageId::for_path`), so this is a genuine boundary.
fn convert_page_uri(page_id: &str) -> EntityUri {
    // ALLOW(entity_uri_from_raw): page_id arrives as a plan Value string across the dispatch boundary
    EntityUri::from_raw(page_id)
}

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
    entity_name: &str,
    op_name: &str,
    origin: &OpOrigin,
    changes: &[holon_core::FieldDelta],
    now_millis: i64,
) -> Vec<HistoryEvent> {
    let stamp = ProvenanceStamp::from_origin(origin, now_millis);
    changes
        .iter()
        .map(|delta| HistoryEvent {
            entity_name: entity_name.to_string(),
            block_id: delta.entity_id.clone(),
            op_name: op_name.to_string(),
            origin: stamp.origin.clone(),
            transition_id: stamp.transition_id.clone(),
            session_id: stamp.session_id.clone(),
            tool_call_id: stamp.tool_call_id.clone(),
            // Reserved for ADR 0024 effect firings; the dispatch chokepoint
            // has no effect id yet, so the column stays NULL for now.
            effect_id: None,
            field: Some(delta.field.clone()),
            old_value: Some(render_value(&delta.old_value)),
            new_value: Some(render_value(&delta.new_value)),
            at_millis: stamp.at_millis,
            // Assigned by the store per record_batch call (one op = one group).
            op_group: None,
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

/// The origin's identity rendered for the deterministic proposal id: the
/// origin class plus its per-instance ids, so distinct sessions/tool calls/
/// rules mint distinct proposals while a re-fire of the SAME dispatch
/// converges (unit separators, same discipline as `deterministic_block_id`).
fn origin_identity_key(origin: &OpOrigin) -> String {
    match origin {
        OpOrigin::User | OpOrigin::Sync | OpOrigin::Ingest => origin.tag().to_string(),
        OpOrigin::Rule { transition_id } => format!("rule\x1f{transition_id}"),
        OpOrigin::Agent {
            session_id,
            tool_call_id,
        } => format!("agent\x1f{session_id}\x1f{tool_call_id}"),
    }
}

/// Normalize a block `properties` column value to its object form. The reader
/// may hand back raw JSON TEXT (`String`/`Json`) or an already-structured
/// `Object`; anything else is a loud error.
fn properties_object(value: &Value) -> Result<std::collections::HashMap<String, Value>> {
    match value {
        Value::Object(map) => Ok(map.clone()),
        Value::String(s) | Value::Json(s) => {
            let json: serde_json::Value = serde_json::from_str(s)
                .map_err(|e| anyhow::anyhow!("properties JSON parse failed: {e}"))?;
            match Value::from_json_value(json) {
                Value::Object(map) => Ok(map),
                other => anyhow::bail!("properties JSON is not an object, got {other:?}"),
            }
        }
        other => anyhow::bail!("properties column is neither JSON text nor an object: {other:?}"),
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
            trust_policy: Arc::new(TrustPolicy::trust_all()),
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

    /// Wire a live-state reader without undo persistence. The trust gate uses
    /// it for proposal idempotence checks and `accept_/reject_proposal` reads;
    /// undo preconditions benefit too.
    pub fn with_state_reader(mut self, reader: Arc<dyn UndoStateReader>) -> Self {
        self.reader = Some(reader);
        self
    }

    /// Configure the trust policy (VisionGapAnalysis C5). Without this the
    /// default [`TrustPolicy::trust_all`] keeps the gate a no-op.
    pub fn with_trust_policy(mut self, policy: Arc<TrustPolicy>) -> Self {
        self.trust_policy = policy;
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
            trust_policy: Arc::new(TrustPolicy::trust_all()),
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
    /// history relation, as ONE batch in one transaction (the store assigns
    /// the batch a shared `op_group`). Provenance ids are derived from
    /// `origin` (reusing the same [`ProvenanceStamp`] extraction as the block
    /// stamp, so the two surfaces never disagree). The timestamp is the
    /// injected clock.
    async fn record_history(
        &self,
        history: &dyn HistoryStore,
        entity_name: &str,
        op_name: &str,
        origin: &OpOrigin,
        changes: &[holon_core::FieldDelta],
    ) -> Result<()> {
        let events = history_events_for(
            entity_name,
            op_name,
            origin,
            changes,
            self.clock.now_millis(),
        );
        history.record_batch(events).await
    }

    /// Dispatch a stored op verbatim (used for inverse/forward replay). Never
    /// pushes an undo entry — replays bypass the push path by construction.
    /// Replay one undo/redo op through the dispatcher, returning the field
    /// deltas it produced. The caller aggregates these to decide whether the
    /// whole entry made an observable change (see
    /// [`Self::changes_are_vacuous`]): a provably-vacuous replay
    /// (identical-content set_field) must be reported
    /// as [`UndoOutcome::NoChange`], never as `Applied`.
    async fn replay(&self, op: &Operation) -> Result<Vec<FieldDelta>> {
        let result = self
            .dispatcher
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
        Ok(result.changes)
    }

    /// Whether a set of replayed field deltas PROVES the operation changed
    /// nothing: at least one delta was reported AND every reported delta is
    /// vacuous (`old_value == new_value`). An empty delta set is NOT provably
    /// vacuous — property/edge writes report no column deltas yet are real
    /// changes — so it conservatively reads as "changed" (single-writer safe).
    /// Column set_field always reports a delta, so an identical-content write
    /// (the poison entry this classifier exists to catch) is provably vacuous.
    fn changes_are_vacuous(changes: &[FieldDelta]) -> bool {
        !changes.is_empty() && changes.iter().all(|d| d.old_value == d.new_value)
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
        // Fail loud on a bogus target_parent — silently creating an orphaned
        // subtree violates the C2a invariant (every block has a reachable
        // parent chain to a page root).
        if !source.exists(&request.target_parent).await? {
            bail!(
                "instantiate_template: target_parent '{}' does not exist",
                request.target_parent
            );
        }
        // Verify the block-to-replace exists BEFORE any create, so an empty→
        // in-place instantiation against a stale id fails loud without leaving
        // a half-instantiated orphan subtree behind.
        if let Some(replace_id) = &request.replace_block {
            if !source.exists(replace_id).await? {
                bail!(
                    "instantiate_template: replace_block '{replace_id}' does not exist"
                );
            }
        }
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
        // Empty→in-place placement (frontend picker): the instance is created,
        // now delete the empty block it supersedes. Ordered AFTER the creates so
        // a failed instantiation never destroys the target (the block is empty,
        // so this never touches existing content). Routed through the normal
        // `delete` op → provenance/history/undo classification all apply.
        if let Some(replace_id) = &request.replace_block {
            let mut del_params: StorageEntity = StorageEntity::default();
            del_params.insert(Arc::from("id"), Value::String(replace_id.clone()));
            Box::pin(OperationEngine::execute_operation(
                self,
                &block_entity,
                "delete",
                del_params,
                origin.clone(),
            ))
            .await?;
        }
        Ok(Some(Value::String(plan.root_id)))
    }

    /// Dispatch ONE constituent write of the block→page compound through the
    /// normal dispatcher path (exactly as the UI would), returning the stored
    /// FORWARD op (for redo), its exact op-level INVERSE (for undo), and the
    /// field deltas it produced. Fails loud if the constituent cannot describe
    /// an inverse — sub-ruling 5: a partial-undo transform is never shipped.
    async fn dispatch_constituent(
        &self,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<(Operation, Operation, Vec<FieldDelta>)> {
        let block = EntityName::new("block");
        let forward = Operation::new(
            block.clone(),
            op_name,
            op_name,
            params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        );
        let result = self
            .dispatcher
            .execute_operation(&block, op_name, params)
            .await
            .map_err(|e| {
                anyhow::anyhow!("convert_block_to_page: constituent '{op_name}' failed: {e}")
            })?;
        let inverse = match result.undo {
            UndoAction::Undo(inv) => inv,
            UndoAction::DeclaredIrreversible(reason) => bail!(
                "convert_block_to_page: constituent '{op_name}' is irreversible ({reason}) — \
                 refusing to ship a partial-undo transform (sub-ruling 5)"
            ),
            UndoAction::Undeclared => bail!(
                "convert_block_to_page: constituent '{op_name}' returned an Undeclared undo \
                 classification"
            ),
        };
        Ok((forward, inverse, result.changes))
    }

    /// Execute the block → page transform (Option B). See
    /// [`CONVERT_BLOCK_TO_PAGE_OP`]. Params: `target` (origin block id) and
    /// optional `destination_path` (`/`-joined page path; empty = vault root,
    /// missing segments are created). Returns the new page id.
    async fn run_convert_block_to_page(
        &self,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<Option<Value>> {
        use holon_api::PAGE_TAG;
        use holon_api::inline_mark::EntityRef;
        use holon_api::inline_mark::InlineMark;
        use holon_api::inline_mark::MarkSpan;
        use holon_api::inline_mark::marks_to_json;

        use crate::core::block_to_page_plan::BlockToPagePlan;

        let block = EntityName::new("block");

        // 1. Plan (read-only): origin content+marks, ordered children, resolved
        //    destination chain. Provider-side because it needs DB reads.
        let plan_result = self
            .dispatcher
            .execute_operation(&block, "block_to_page_plan", params.clone())
            .await
            .map_err(|e| anyhow::anyhow!("convert_block_to_page: planning failed: {e}"))?;
        let plan_value = plan_result.response.ok_or_else(|| {
            anyhow::anyhow!("convert_block_to_page: planner returned no plan payload")
        })?;
        let plan = BlockToPagePlan::from_value(&plan_value)
            .map_err(|e| anyhow::anyhow!("convert_block_to_page: {e}"))?;

        // Inverses are bucketed per step, NOT blanket-reversed: the undo order
        // must reverse the STEPS while keeping the child re-homes in FORWARD
        // order (each child's move-back anchors on its original predecessor, so
        // C1 must land before C2), and delete the hierarchy leaf→root.
        let page_tag = || Value::Array(vec![Value::String(PAGE_TAG.to_string())]);
        let mut forwards: Vec<Operation> = Vec::new();
        let mut seg_invs: Vec<Operation> = Vec::new();
        let mut child_invs: Vec<Operation> = Vec::new();
        let mut all_changes: Vec<FieldDelta> = Vec::new();

        // 2. Create any missing destination-hierarchy pages, root→leaf. Each is an
        //    invertible `create` (inverse: delete).
        for seg in &plan.missing_segments {
            let mut p = StorageEntity::new();
            p.insert("id".into(), Value::String(seg.id.clone()));
            p.insert("content".into(), Value::String(seg.name.clone()));
            p.insert("parent_id".into(), Value::String(seg.parent_id.clone()));
            p.insert("depth".into(), Value::Integer(seg.depth));
            p.insert("tags".into(), page_tag());
            let p = self.stamp_provenance("create", p, origin);
            let (fwd, inv, ch) = self.dispatch_constituent("create", p).await?;
            forwards.push(fwd);
            seg_invs.push(inv);
            all_changes.extend(ch);
        }

        // 3. Create the new page P, moving the origin's content (+ marks) onto it.
        //    `create` inverse = delete(P) (leaf-exact: its children are re-homed BACK
        //    before P is deleted on undo).
        let mut pc = StorageEntity::new();
        pc.insert("id".into(), Value::String(plan.page_id.clone()));
        pc.insert("content".into(), Value::String(plan.origin_content.clone()));
        pc.insert(
            "parent_id".into(),
            Value::String(plan.destination_parent_id.clone()),
        );
        pc.insert("depth".into(), Value::Integer(plan.page_depth));
        pc.insert("tags".into(), page_tag());
        if let Value::String(marks) = &plan.origin_marks {
            if !marks.is_empty() && marks != "[]" {
                pc.insert("marks".into(), Value::String(marks.clone()));
            }
        }
        let pc = self.stamp_provenance("create", pc, origin);
        let (fwd, p_inv, ch) = self.dispatch_constituent("create", pc).await?;
        forwards.push(fwd);
        all_changes.extend(ch);

        // 4. Re-home each child under P, preserving sibling order (after_block_id = the
        //    previous child). move_block inverse restores the child's original parent +
        //    predecessor exactly.
        let mut prev: Option<String> = None;
        for child in &plan.child_ids {
            let mut mp = StorageEntity::new();
            mp.insert("id".into(), Value::String(child.clone()));
            mp.insert("parent_id".into(), Value::String(plan.page_id.clone()));
            match &prev {
                Some(pid) => {
                    mp.insert("after_block_id".into(), Value::String(pid.clone()));
                }
                None => {
                    mp.insert("after_block_id".into(), Value::Null);
                }
            }
            let (fwd, inv, ch) = self.dispatch_constituent("move_block", mp).await?;
            forwards.push(fwd);
            child_invs.push(inv);
            all_changes.extend(ch);
            prev = Some(child.clone());
        }

        // 5. Leave a `[[P]]` link behind. The origin's TEXT is unchanged; only its
        //    marks gain a full-span Link to P. A DIRECT `set_field(marks=…)` (not a
        //    `content=Object` write, whose dispatcher-split marks follow-up drops the
        //    marks inverse) yields the exact `set_field(marks=old)` inverse — so undo
        //    restores the origin's original marks faithfully.
        let label = plan.origin_content.clone();
        let link_mark = MarkSpan::new(
            0,
            label.chars().count(),
            InlineMark::Link {
                target: EntityRef::Internal {
                    id: convert_page_uri(&plan.page_id),
                },
                label: label.clone(),
            },
        );
        let mut sf = StorageEntity::new();
        sf.insert("id".into(), Value::String(plan.origin_id.clone()));
        sf.insert("field".into(), Value::String("marks".into()));
        sf.insert("value".into(), Value::String(marks_to_json(&[link_mark])));
        let (fwd, marks_inv, ch) = self.dispatch_constituent("set_field", sf).await?;
        forwards.push(fwd);
        all_changes.extend(ch);

        // 6. Re-point inbound backlinks origin → P (exact capture-based inverse
        //    `restore_link_resolution`).
        let mut rw = StorageEntity::new();
        rw.insert("from".into(), Value::String(plan.origin_id.clone()));
        rw.insert("to".into(), Value::String(plan.page_id.clone()));
        let (fwd, rewrite_inv, ch) = self
            .dispatch_constituent("rewrite_link_resolution", rw)
            .await?;
        forwards.push(fwd);
        all_changes.extend(ch);

        // Compose ONE undo entry. Redo re-executes the forward constituents in
        // order. Undo replays the STEPS in reverse: restore inbound links →
        // restore origin marks (drops the link) → re-home children to origin (in
        // FORWARD order, so predecessors land first) → delete P → delete the
        // minted hierarchy leaf→root.
        if origin.is_user() {
            let mut inverse_ops: Vec<Operation> = Vec::new();
            inverse_ops.push(rewrite_inv);
            inverse_ops.push(marks_inv);
            inverse_ops.extend(child_invs);
            inverse_ops.push(p_inv);
            seg_invs.reverse();
            inverse_ops.extend(seg_invs);
            // Staleness fingerprint: guard the LITERALLY-restored fields
            // (parent_id, content, marks, resolved link state) and EXCLUDE the
            // derived positional columns `depth`/`sort_key`. Structural ops
            // RECOMPUTE those from the live tree rather than restoring the
            // captured value, so their post-undo value is a function of the
            // current parent chain, not the pre-op value — fingerprinting them
            // makes a legitimate undo→redo trip spuriously "stale". (The moved
            // blocks' parent_id — the field that actually defines the re-home —
            // is still fingerprinted, so real external edits are caught.)
            let fp_changes: Vec<FieldDelta> = all_changes
                .iter()
                .filter(|d| d.field != "depth" && d.field != "sort_key")
                .cloned()
                .collect();
            let entry = UndoEntry {
                ops: forwards,
                inverse_ops,
                origin: OpOrigin::User,
                group_id: 0,
                precondition: Precondition::forward(&fp_changes),
                redo_precondition: Precondition::inverse(&fp_changes),
            };
            self.undo_stack.write().await.push(entry);
            self.persist().await?;
        }

        if let Some(history) = &self.history {
            self.record_history(
                history.as_ref(),
                "block",
                CONVERT_BLOCK_TO_PAGE_OP,
                origin,
                &all_changes,
            )
            .await?;
        }

        Ok(Some(Value::String(plan.page_id)))
    }

    /// The synthetic descriptor advertising the engine-level
    /// `convert_block_to_page` op so MCP / the slash menu discover it like any
    /// provider op.
    fn convert_block_to_page_descriptor() -> OperationDescriptor {
        use holon_api::render_types::TypeHint;
        OperationDescriptor {
            entity_name: EntityName::new("block"),
            entity_short_name: "block".to_string(),
            id_column: "id".to_string(),
            name: CONVERT_BLOCK_TO_PAGE_OP.to_string(),
            display_name: "Turn into page".to_string(),
            description: "Turn this block into a page: move its content and children onto a new \
                          page and leave a link behind."
                .to_string(),
            required_params: vec![holon_api::OperationParam {
                name: "target".to_string(),
                type_hint: TypeHint::String,
                description: "Origin block id to convert".to_string(),
            }],
            // Resolve `target` from the focused block's `id` (the only key live
            // context_params reliably carries), so a plain slash-menu click on a
            // block turns THAT block into a page. `destination_path` is optional
            // and defaults, backend-side, to the nearest page ancestor.
            param_mappings: vec![holon_api::render_types::ParamMapping {
                from: "id".to_string(),
                provides: vec!["target".to_string()],
                defaults: Default::default(),
            }],
            trigger: None,
            bound_params: Default::default(),
            affected_fields: vec![],
            precondition: None,
        }
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
                // `replace_block` (optional) is passed by the frontend picker's
                // empty→in-place placement; not advertised as required.
            ],
            affected_fields: vec![],
            param_mappings: vec![],
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        }
    }

    /// Coerce a sub-trust-threshold dispatch into a proposal emission
    /// (VisionGapAnalysis C5). The wrapped op never reaches canonical state:
    /// it is recorded verbatim in a proposal block under `block:proposals`,
    /// stamped with the proposer's provenance. The proposal id is
    /// deterministic per (origin identity, entity, op, params) — a re-fire
    /// converges on the same proposal instead of stacking duplicates
    /// (ADR 0024 P4). Anything that cannot be recorded as a proposal is a
    /// loud error, never a silent drop.
    async fn coerce_to_proposal(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        origin: &OpOrigin,
    ) -> Result<Option<Value>> {
        use anyhow::Context;

        let proposal_id = deterministic_proposal_id(
            &origin_identity_key(origin),
            entity_name.as_str(),
            op_name,
            &FiringKey::from_row(&params),
        );

        let disclosed = |status: &str| {
            let mut response = std::collections::HashMap::new();
            response.insert(
                "proposal_id".to_string(),
                Value::String(proposal_id.as_str().to_string()),
            );
            response.insert("status".to_string(), Value::String(status.to_string()));
            Ok(Some(Value::Object(response)))
        };

        // Idempotent re-fire: the deterministic id already exists → nothing to
        // create. Without a reader the create itself must be id-convergent
        // (Loro upsert semantics), so the check is skipped, disclosed by type.
        if let Some(reader) = &self.reader
            && reader
                .field_value(proposal_id.as_str(), "id")
                .await
                .context("trust gate: proposal existence check")?
                .is_some()
        {
            return disclosed("already_proposed");
        }

        self.ensure_proposals_root(origin).await?;

        let record = ProposalRecord::pending(
            entity_name.clone(),
            op_name,
            params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        );
        let mut create: StorageEntity = StorageEntity::new();
        create.insert("id".into(), Value::String(proposal_id.as_str().to_string()));
        create.insert(
            "parent_id".into(),
            Value::String(EntityUri::block(PROPOSALS_ROOT_ID).as_str().to_string()),
        );
        create.insert(
            "content".into(),
            Value::String(format!(
                "Proposal: {op_name} on {entity_name} (by {})",
                origin.tag()
            )),
        );
        create.insert(Arc::from(PROPOSAL_PROPERTY), record.to_value());
        // The proposal block's `_provenance` names the PROPOSER — that is the
        // fact the supervision view groups by.
        let create = self.stamp_provenance("create", create, origin);

        let result = self
            .dispatcher
            .execute_operation(&EntityName::new("block"), "create", create)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "trust gate: coercing '{op_name}' on '{entity_name}' from origin '{}' into a \
                     proposal failed: {e}",
                    origin.tag()
                )
            })?;

        if let Some(history) = &self.history {
            self.record_history(history.as_ref(), "block", "create", origin, &result.changes)
                .await?;
        }

        disclosed("proposed")
    }

    /// Ensure the proposal place root (`block:proposals`) exists. With a
    /// reader the check is a direct read; without one the create relies on
    /// deterministic-id upsert semantics (same id, same content — convergent).
    async fn ensure_proposals_root(&self, origin: &OpOrigin) -> Result<()> {
        use anyhow::Context;

        let root_uri = EntityUri::block(PROPOSALS_ROOT_ID);
        if let Some(reader) = &self.reader
            && reader
                .field_value(root_uri.as_str(), "id")
                .await
                .context("trust gate: proposals root existence check")?
                .is_some()
        {
            return Ok(());
        }
        let mut create: StorageEntity = StorageEntity::new();
        create.insert("id".into(), Value::String(root_uri.as_str().to_string()));
        create.insert(
            "parent_id".into(),
            Value::String(EntityUri::no_parent().as_str().to_string()),
        );
        create.insert("content".into(), Value::String("Proposals".to_string()));
        let create = self.stamp_provenance("create", create, origin);
        self.dispatcher
            .execute_operation(&EntityName::new("block"), "create", create)
            .await
            .map_err(|e| anyhow::anyhow!("trust gate: creating proposals root failed: {e}"))?;
        Ok(())
    }

    /// Resolve a pending proposal: `accept` re-dispatches the wrapped op with
    /// the CONFIRMER's origin through the normal path (gate, stamping, undo,
    /// history), preserving the proposer's stamp as `_proposed_by`; `reject`
    /// retracts without executing. Both flip the proposal to a terminal
    /// status carrying the resolver's provenance, so the supervision view
    /// keeps acceptance stats per origin.
    async fn run_resolve_proposal(
        &self,
        params: &StorageEntity,
        origin: &OpOrigin,
        accept: bool,
    ) -> Result<Option<Value>> {
        use anyhow::Context;

        let verb = if accept { "accept" } else { "reject" };
        let proposal_id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("{verb}_proposal requires an 'id' param"))?
            .to_string();
        let reader = self.reader.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "{verb}_proposal requires a live-state reader — not wired in this session"
            )
        })?;

        let properties = reader
            .field_value(&proposal_id, "properties")
            .await
            .with_context(|| format!("{verb}_proposal: reading proposal '{proposal_id}'"))?
            .ok_or_else(|| {
                anyhow::anyhow!("{verb}_proposal: proposal '{proposal_id}' not found")
            })?;
        let properties = properties_object(&properties).with_context(|| {
            format!("{verb}_proposal: proposal '{proposal_id}' properties malformed")
        })?;
        let record =
            ProposalRecord::from_value(properties.get(PROPOSAL_PROPERTY).ok_or_else(|| {
                anyhow::anyhow!(
                    "{verb}_proposal: block '{proposal_id}' carries no '{PROPOSAL_PROPERTY}' \
                     property — not a proposal"
                )
            })?)?;
        if record.status != ProposalStatus::Pending {
            anyhow::bail!(
                "{verb}_proposal: proposal '{proposal_id}' is already {}",
                record.status.as_str()
            );
        }

        let resolver_stamp =
            ProvenanceStamp::from_origin(origin, self.clock.now_millis()).to_value();

        let response = if accept {
            let proposer_stamp = properties.get(PROVENANCE_PROPERTY).ok_or_else(|| {
                anyhow::anyhow!(
                    "accept_proposal: proposal '{proposal_id}' carries no proposer \
                     '{PROVENANCE_PROPERTY}' stamp — refusing to promote without provenance"
                )
            })?;
            let mut wrapped: StorageEntity = record
                .params
                .iter()
                .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
                .collect();
            if PROVENANCE_STAMPED_OPS.contains(&record.op_name.as_str()) {
                wrapped.insert(Arc::from(PROPOSED_BY_PROPERTY), proposer_stamp.clone());
            }
            // Boxed for async recursion: promotion IS an ordinary dispatch
            // with the confirmer's origin — gate, `_provenance` stamp, undo
            // classification, and history all apply unchanged.
            Box::pin(OperationEngine::execute_operation(
                self,
                &record.entity.clone(),
                &record.op_name.clone(),
                wrapped,
                origin.clone(),
            ))
            .await
            .with_context(|| {
                format!(
                    "accept_proposal: promoting proposal '{proposal_id}' ('{}' on '{}') failed",
                    record.op_name, record.entity
                )
            })?
        } else {
            let mut response = std::collections::HashMap::new();
            response.insert(
                "proposal_id".to_string(),
                Value::String(proposal_id.clone()),
            );
            response.insert(
                "status".to_string(),
                Value::String(ProposalStatus::Rejected.as_str().to_string()),
            );
            Some(Value::Object(response))
        };

        let status = if accept {
            ProposalStatus::Accepted
        } else {
            ProposalStatus::Rejected
        };
        let resolved = record.resolved(status, resolver_stamp);
        let mut update: StorageEntity = StorageEntity::new();
        update.insert("id".into(), Value::String(proposal_id.clone()));
        update.insert(Arc::from(PROPOSAL_PROPERTY), resolved.to_value());
        self.dispatcher
            .execute_operation(&EntityName::new("block"), "update", update)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{verb}_proposal: marking proposal '{proposal_id}' {} failed: {e}",
                    status.as_str()
                )
            })?;

        Ok(response)
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
        // Trust gate (VisionGapAnalysis C5): a sub-threshold (origin, entity,
        // op) never reaches canonical state — it is coerced into a proposal
        // emission under `block:proposals`. This runs FIRST so every shape
        // (plain ops, compounds, even accept/reject themselves) is governed by
        // the same place-topology rule; a trusted origin falls through with
        // zero behavior change.
        if self.trust_policy.decide(&origin, entity_name, op_name) == TrustDecision::Propose {
            return self
                .coerce_to_proposal(entity_name, op_name, params, &origin)
                .await;
        }

        // Engine-level compounds: proposal confirmation (C5). Acceptance
        // re-dispatches the wrapped op with the CONFIRMER's origin; rejection
        // retracts without executing.
        if entity_name.as_str() == "block" {
            if op_name == ACCEPT_PROPOSAL_OP {
                return self.run_resolve_proposal(&params, &origin, true).await;
            }
            if op_name == REJECT_PROPOSAL_OP {
                return self.run_resolve_proposal(&params, &origin, false).await;
            }
        }

        // Engine-level compound: expand a template instantiation into ordinary
        // `create` dispatches (each re-enters this method and gets stamping /
        // history / undo classification like any other op).
        if op_name == INSTANTIATE_TEMPLATE_OP && entity_name.as_str() == "block" {
            return self.run_instantiate_template(&params, &origin).await;
        }

        // Engine-level compound: block → page (Option B). Composed from ordinary
        // invertible ops (create / move_block / set_field(marks) /
        // rewrite_link_resolution) whose op-level inverses assemble into ONE
        // composite `UndoEntry`. Intercepted here (like `instantiate_template`)
        // so undo/redo replay the CONSTITUENTS, never the compound name.
        if op_name == CONVERT_BLOCK_TO_PAGE_OP && entity_name.as_str() == "block" {
            return self.run_convert_block_to_page(&params, &origin).await;
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
        //
        // No-op writes never enter the log (BugFunnel 2026-07-13 undo row):
        // a provably-vacuous forward op (every reported field delta has
        // `old_value == new_value`, e.g. an identical-content `set_field`) is a
        // reversible-but-inert write. Journaling it poisons the stack with an
        // entry whose undo is itself a no-op, so consecutive undo presses get
        // eaten while a real target underneath stays unreachable. Bug A stops
        // the frontend from dispatching these; this is the defense-in-depth
        // complement — any provider that reports a vacuous change is not
        // journaled. (An empty delta set is NOT vacuous here — property/edge
        // writes report no column deltas but are real; they still journal.)
        if origin.is_user() && !Self::changes_are_vacuous(&result.changes) {
            if let UndoAction::Undo(inverse_op) = &result.undo {
                // Redo identity-stability: a `create` whose caller omitted `id`
                // has one MINTED by the provider (interactive block creation,
                // Rhai `block.create`). The stored forward (redo) op is built
                // from the ORIGINAL params, which lack that id — so a redo would
                // re-mint a fresh uuid, dangling every ref/link/junction that
                // targeted the original (BugFunnel dogfood #4). The create's
                // inverse is `delete{id: <minted>}`, so the minted id is
                // authoritative there; graft it onto the redo op so redo
                // recreates the SAME block.
                let mut forward_op = forward_op;
                if op_name == "create" && !forward_op.params.contains_key("id") {
                    if let Some(minted) = inverse_op.params.get("id") {
                        forward_op.params.insert("id".to_string(), minted.clone());
                    }
                }
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
            self.record_history(
                history.as_ref(),
                entity_name.as_str(),
                op_name,
                &origin,
                &result.changes,
            )
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
        if entity_name == "block" {
            ops.push(Self::convert_block_to_page_descriptor());
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
        if entity_name == "block" && op_name == CONVERT_BLOCK_TO_PAGE_OP {
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

        let mut changes = Vec::new();
        for op in &entry.inverse_ops {
            changes.extend(self.replay(op).await?);
        }
        // Fail-loud (CLAUDE.md): the entry is consumed either way — a stale-top
        // poison entry must not be re-attempted — but if the inverse replay
        // proved no observable change, report `NoChange` so the caller never
        // claims "undone" for a no-op press (BugFunnel 2026-07-13 undo row).
        self.undo_stack.write().await.commit_undo();
        self.persist().await?;
        if Self::changes_are_vacuous(&changes) {
            tracing::warn!("undo: inverse replay produced no observable change (no-op entry)");
            return Ok(UndoOutcome::NoChange);
        }
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

        let mut changes = Vec::new();
        for op in &entry.ops {
            changes.extend(self.replay(op).await?);
        }
        self.undo_stack.write().await.commit_redo();
        self.persist().await?;
        if Self::changes_are_vacuous(&changes) {
            tracing::warn!("redo: forward replay produced no observable change (no-op entry)");
            return Ok(UndoOutcome::NoChange);
        }
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
    async fn nonexistent_target_parent_fails_loud_and_creates_nothing() {
        let engine = block_engine().await;
        // Seed the template subtree (block:tpl + child) but NOT the target
        // parent — instantiate into a bogus parent must fail loud.
        seed_template_without_target(&engine).await;

        let err = engine
            .execute_operation(
                &EntityName::new("block"),
                "instantiate_template",
                instantiate_params("k1", &[("date", "2026-07-12")]),
                OpOrigin::User,
            )
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("block:target"),
            "error must name the missing parent; got: {msg}"
        );
        assert!(
            msg.contains("target_parent"),
            "error must mention target_parent; got: {msg}"
        );
        assert_eq!(
            instance_roots(&engine).await.len(),
            0,
            "bogus-parent instantiation must create nothing"
        );
    }

    /// SEVERE-data-loss guard: an org-parsed template block carries its `:ID:`
    /// as an "ID" property. If that copies into the instance, the instance
    /// claims the template's id — on org writeback+reload the duplicate `:ID:`
    /// collides and empties the template file. The instance must carry NO "ID".
    #[tokio::test(flavor = "multi_thread")]
    async fn instance_never_carries_template_org_id() {
        let engine = block_engine().await;
        create_block(
            &engine,
            &[
                ("id", Value::String("block:target".into())),
                ("content", Value::String("Target".into())),
            ],
        )
        .await;
        // Template block WITH an org "ID" property, exactly as the org parser
        // lifts `:ID:` (block_params.rs).
        create_block(
            &engine,
            &[
                ("id", Value::String("block:tpl".into())),
                ("content", Value::String("body".into())),
                (
                    "properties",
                    Value::String(
                        r#"{"template":"daily","template_vars":"","ID":"block:tpl","keep":"y"}"#
                            .into(),
                    ),
                ),
            ],
        )
        .await;

        engine
            .execute_operation(
                &EntityName::new("block"),
                "instantiate_template",
                instantiate_params("k1", &[]),
                OpOrigin::User,
            )
            .await
            .unwrap();

        let roots = instance_roots(&engine).await;
        assert_eq!(roots.len(), 1);
        let props = props_of(&roots[0]);
        assert!(
            props.get("ID").is_none(),
            "instance must NOT carry the template's org ID (org-roundtrip destruction); got {props:?}"
        );
        assert_eq!(props["keep"], "y", "non-identity properties still copy");
        assert_eq!(props["instance_of"], "block:tpl");
    }

    async fn block_exists(engine: &BackendEngine, id: &str) -> bool {
        !engine
            .db_handle()
            .query(
                &format!(
                    "SELECT id FROM block_raw WHERE id = '{}'",
                    id.replace('\'', "''")
                ),
                HashMap::new(),
            )
            .await
            .unwrap()
            .is_empty()
    }

    /// Empty→in-place placement: `replace_block` deletes the empty block the
    /// instance supersedes, AFTER the instance is created.
    #[tokio::test(flavor = "multi_thread")]
    async fn replace_block_deletes_empty_target_after_instantiation() {
        let engine = block_engine().await;
        seed_template(&engine).await;
        // The empty bullet the user triggered the picker on.
        create_block(
            &engine,
            &[
                ("id", Value::String("block:empty".into())),
                ("parent_id", Value::String("block:target".into())),
                ("content", Value::String("".into())),
            ],
        )
        .await;

        let mut params = instantiate_params("k1", &[("date", "2026-07-12")]);
        params.insert("replace_block".into(), Value::String("block:empty".into()));
        engine
            .execute_operation(
                &EntityName::new("block"),
                "instantiate_template",
                params,
                OpOrigin::User,
            )
            .await
            .unwrap();

        assert!(
            !block_exists(&engine, "block:empty").await,
            "the empty block must be deleted (replaced in place)"
        );
        assert_eq!(
            instance_roots(&engine).await.len(),
            1,
            "exactly one instance root created under the parent"
        );
    }

    /// A `replace_block` pointing at a nonexistent block fails loud and creates
    /// nothing — the pre-create existence check.
    #[tokio::test(flavor = "multi_thread")]
    async fn replace_block_nonexistent_fails_loud_and_creates_nothing() {
        let engine = block_engine().await;
        seed_template(&engine).await;

        let mut params = instantiate_params("k1", &[("date", "2026-07-12")]);
        params.insert("replace_block".into(), Value::String("block:ghost".into()));
        let err = engine
            .execute_operation(
                &EntityName::new("block"),
                "instantiate_template",
                params,
                OpOrigin::User,
            )
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("replace_block"), "got: {msg}");
        assert!(msg.contains("block:ghost"), "got: {msg}");
        assert_eq!(
            instance_roots(&engine).await.len(),
            0,
            "must create nothing when replace_block is bogus"
        );
    }

    /// Like `seed_template` but does NOT create `block:target` — used for
    /// bogus-parent tests.
    async fn seed_template_without_target(engine: &BackendEngine) {
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
        holon_core::FieldDelta::new(entity, field, Value::Null, new_value)
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
        let events = history_events_for("block", "set_field", &origin, &changes, 999);

        assert_eq!(events.len(), 2, "one event per field delta");
        assert_eq!(events[0].entity_name, "block");
        assert_eq!(events[0].block_id, "A");
        assert_eq!(events[0].op_name, "set_field");
        assert_eq!(events[0].origin, "rule");
        assert_eq!(events[0].transition_id.as_deref(), Some("rule:postpone"));
        assert_eq!(events[0].field.as_deref(), Some("status"));
        assert_eq!(events[0].old_value.as_deref(), Some("null"));
        assert_eq!(events[0].new_value.as_deref(), Some("postponed"));
        assert_eq!(events[0].at_millis, 999);
        assert_eq!(events[0].op_group, None, "group is store-assigned");
        assert_eq!(events[0].effect_id, None, "reserved until effects dispatch");
        // Non-string values render for query matching.
        assert_eq!(events[1].new_value.as_deref(), Some("7"));
    }

    /// An `add_tag`'s `history_only` `tags` delta is still recorded as a
    /// history event (the fingerprint only excludes it from undo preconditions,
    /// never from the history relation) — so a real add_tag produces a
    /// `block_history` op_group just like a scalar `set_field`.
    #[test]
    fn history_only_tag_delta_still_produces_a_history_event() {
        let changes = vec![holon_core::FieldDelta::history_only(
            "block:x",
            "tags",
            Value::Null,
            Value::String("todo".into()),
        )];
        let events = history_events_for("block", "add_tag", &OpOrigin::User, &changes, 42);
        assert_eq!(events.len(), 1, "the tag delta must record one event");
        assert_eq!(events[0].op_name, "add_tag");
        assert_eq!(events[0].field.as_deref(), Some("tags"));
        assert_eq!(events[0].new_value.as_deref(), Some("todo"));
    }

    #[test]
    fn history_events_carry_agent_identity() {
        let changes = vec![delta("B", "content", Value::String("hi".into()))];
        let origin = OpOrigin::Agent {
            session_id: "mcp-session:s".into(),
            tool_call_id: "tool-call:c".into(),
        };
        let events = history_events_for("block", "create", &origin, &changes, 1);
        assert_eq!(events[0].origin, "agent");
        assert_eq!(events[0].session_id.as_deref(), Some("mcp-session:s"));
        assert_eq!(events[0].tool_call_id.as_deref(), Some("tool-call:c"));
    }
}
