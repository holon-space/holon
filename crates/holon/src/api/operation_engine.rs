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
use holon_api::ENGINE_OWNED_PARAM_KEYS;
use holon_api::EntityName;
use holon_api::HistoryEvent;
use holon_api::HistoryStore;
use holon_api::OpOrigin;
use holon_api::OpOutcome;
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
use crate::api::operation_dispatcher::AuthoredInput;
use crate::api::operation_dispatcher::OperationDispatcher;
use crate::core::sql_operation_provider::WriteSchema;

#[async_trait]
impl OperationEngine for BackendEngine {
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        origin: OpOrigin,
    ) -> Result<OpOutcome> {
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
    /// Resolver for the owning document's `#+TODO:` vocabulary, consulted by
    /// every path that parses or cycles a task keyword. `None` on a wiring
    /// without a queryable block projection — those paths then fall back to the
    /// defaults and say so.
    vocabulary_source: Option<Arc<dyn crate::core::task_keyword_promotion::TaskVocabularySource>>,
    /// Trust policy (VisionGapAnalysis C5): decides per (origin, entity, op)
    /// whether a dispatch executes against canonical state or is coerced into
    /// a proposal emission. Defaults to [`TrustPolicy::trust_all`] — the gate
    /// is a no-op until a policy is configured.
    trust_policy: Arc<TrustPolicy>,
    /// Per-entity serialization of the write-and-journal step (see
    /// [`EntityWriteLocks`]).
    entity_write_locks: EntityWriteLocks,
}

/// Serializes the write-and-journal step per entity.
///
/// Capturing an op's prior state, writing the new state, and pushing the undo
/// entry are three steps that must be ONE step for a given entity: the editor
/// spawns one un-awaited task per keystroke
/// (`holon_frontend::operations::dispatch_operation`), so N writes to the same
/// block are in flight at once. Interleaved, a later write reads a prior value
/// the earlier write has already superseded (its stored inverse then skips
/// characters) and entries land in completion rather than write order — both
/// make every following undo fail its own precondition and be dropped.
///
/// Striped over a fixed table: two different entities serialize only on a hash
/// collision, and the table never grows with the vault. The stripes are
/// [`tokio::sync::Mutex`]es, whose FIFO fairness is what makes the write order
/// equal the acquisition order (an unfair lock would preserve atomicity but not
/// order, and the undo stack is an ordered structure).
struct EntityWriteLocks {
    stripes: Vec<tokio::sync::Mutex<()>>,
}

impl Default for EntityWriteLocks {
    fn default() -> Self {
        Self {
            stripes: (0..64).map(|_| tokio::sync::Mutex::new(())).collect(),
        }
    }
}

impl EntityWriteLocks {
    /// The stripe an entity hashes to. The stripe index — not the
    /// `(entity_name, id)` key — is the lock's identity, so any code that takes
    /// more than one guard must order and dedupe on THIS value.
    fn stripe_of(&self, entity_name: &str, id: &str) -> usize {
        use std::hash::Hash;
        use std::hash::Hasher;

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        entity_name.hash(&mut hasher);
        id.hash(&mut hasher);
        (hasher.finish() % self.stripes.len() as u64) as usize
    }

    async fn lock_stripe(&self, stripe: usize) -> tokio::sync::MutexGuard<'_, ()> {
        self.stripes[stripe].lock().await
    }

    async fn lock(&self, entity_name: &str, id: &str) -> tokio::sync::MutexGuard<'_, ()> {
        self.lock_stripe(self.stripe_of(entity_name, id)).await
    }

    /// Lock the entity an op targets, named by the `id_key` param it uses
    /// (`id` for ordinary ops, `target` for the block→page compound,
    /// `canonical` for the merge). An op that names none — a `create` that
    /// mints its own id — has no prior state to race for.
    ///
    /// This takes exactly one guard and must not nest inside another. The only
    /// sanctioned multi-guard hold is [`Self::lock_all`], which is safe because
    /// it acquires in ascending stripe order; a `lock_target` nested inside one
    /// of those holds would acquire out of order and could deadlock.
    async fn lock_target(
        &self,
        entity_name: &str,
        params: &StorageEntity,
        id_key: &str,
    ) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        let id = params.get(id_key).and_then(Value::as_string)?;
        Some(self.lock(entity_name, id).await)
    }

    /// Take every stripe the given `(entity_name, id)` pairs hash to, as ONE
    /// hold. Deadlock-freedom rests on two properties, both of which are about
    /// the stripe index rather than the key: acquiring in ascending stripe
    /// order gives all callers a single total order over the actual mutexes,
    /// and deduping stripes stops a hash collision between two distinct ids
    /// from re-locking a stripe this task already holds (a `tokio` mutex is not
    /// reentrant, so that would hang forever).
    async fn lock_all<'a>(
        &self,
        targets: impl Iterator<Item = (&'a str, &'a str)>,
    ) -> Vec<tokio::sync::MutexGuard<'_, ()>> {
        let mut stripes: Vec<usize> = targets
            .map(|(entity_name, id)| self.stripe_of(entity_name, id))
            .collect();
        stripes.sort_unstable();
        stripes.dedup();

        let mut guards = Vec::with_capacity(stripes.len());
        for stripe in stripes {
            guards.push(self.lock_stripe(stripe).await);
        }
        guards
    }
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

/// The engine-level compound that folds a duplicate identity into a canonical
/// one (`docs/Plans/MergeBlocksInc1-2026-07-30.md`): move the duplicate's
/// children over, collapse normalization-equal siblings, union tags/props, and
/// leave a REPLICATED redirect so the duplicate's id keeps resolving. Composed
/// from ordinary invertible ops so the whole merge is ONE reversible
/// [`UndoEntry`].
const MERGE_BLOCKS_OP: &str = "merge_blocks";

/// Advance a block one step around its owning document's task-state ring
/// (Cmd+Enter). Intercepted at the engine because the ring is a function of the
/// DOCUMENT's declared `#+TODO:` vocabulary, and only the engine holds the
/// handle that resolves it — a storage provider sees one row and can do no
/// better than a hardcoded list, which is exactly the keyword the org parser
/// then refuses to read back.
pub const CYCLE_TASK_STATE_OP: &str = "cycle_task_state";

/// Names the writes that enforce the keyword-convergence rule in failure
/// messages and WARNs. Not a dispatchable op: convergence is a property of
/// every block write, not something a caller asks for.
const CONVERGE_TASK_KEYWORD_OP: &str = "converge_task_keyword";

pub use holon_api::SOURCE_TEXT_FIELD;

/// The id of the child that parks a merged-away block's body when BOTH sides
/// carried content. Derived from the duplicate's id so a merge is idempotent
/// in the id it mints and the block is greppable back to its origin.
fn merged_body_child_id(duplicate_id: &str) -> String {
    format!("{duplicate_id}-merged-body")
}

/// Re-parse a plan's `page_id` string into an `EntityUri` for the `[[P]]` link
/// mark. The id crosses the dispatch boundary as a plan `Value` string (the
/// planner minted it via `PageId::for_path`), so this is a genuine boundary.
fn convert_page_uri(page_id: &str) -> EntityUri {
    // ALLOW(entity_uri_from_raw): plan Value string across dispatch boundary
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

/// Param PAIRS where one param NAMES a field and the other carries its value.
///
/// `set_field`'s `(field, value)` is the whole vocabulary today, verified by
/// enumerating every declared `OperationParam` name — an op that lets the
/// caller choose which field to write declares that choice as a param, and this
/// is the spelling the descriptors use. Adding a second spelling means adding
/// it HERE, which is the one place [`reject_engine_owned_keys`] reads.
const FIELD_NAMING_PARAMS: &[(&str, &str)] = &[("field", "value")];

/// The property keys a whole-BAG value carries.
///
/// Two encodings reach this boundary and both must be read: a decoded `Object`,
/// and the JSON STRING live callers pass (`sql_operation_provider.rs` accepts
/// both when it merges the overflow column). A bag whose shape we cannot read
/// is REFUSED rather than waved through — "I could not tell whether this
/// carries a reserved key" is not evidence that it does not.
fn bag_keys(op_name: &str, value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Object(map) => Ok(map.keys().map(|k| k.to_string()).collect()),
        Value::String(json) => {
            let parsed: serde_json::Value = serde_json::from_str(json).map_err(|e| {
                anyhow::anyhow!(
                    "'{op_name}' carries a '{bag}' bag that is not readable as JSON ({e}), so it \
                     cannot be shown free of the engine-owned keys {ENGINE_OWNED_PARAM_KEYS:?} — \
                     the write is REFUSED rather than trusted unread.",
                    bag = WriteSchema::OVERFLOW_COLUMN,
                )
            })?;
            match parsed {
                serde_json::Value::Object(map) => Ok(map.keys().cloned().collect()),
                other => anyhow::bail!(
                    "'{op_name}' carries a '{bag}' bag that is JSON but not an object (got \
                     {kind}), so it has no property keys to check — the write is REFUSED rather \
                     than trusted unread.",
                    bag = WriteSchema::OVERFLOW_COLUMN,
                    kind = match other {
                        serde_json::Value::Null => "null",
                        serde_json::Value::Bool(_) => "a boolean",
                        serde_json::Value::Number(_) => "a number",
                        serde_json::Value::String(_) => "a string",
                        serde_json::Value::Array(_) => "an array",
                        serde_json::Value::Object(_) => unreachable!("matched above"),
                    },
                ),
            }
        }
        other => anyhow::bail!(
            "'{op_name}' carries a '{bag}' bag of an unreadable shape ({other:?}), so it cannot \
             be shown free of the engine-owned keys {ENGINE_OWNED_PARAM_KEYS:?} — the write is \
             REFUSED rather than trusted unread.",
            bag = WriteSchema::OVERFLOW_COLUMN,
        ),
    }
}

/// Every property key `params` would write, whichever route names it.
///
/// THREE routes, differing only in how deep the key sits: `create`/`update` put
/// it in the param KEYS; `set_field` puts it in the VALUE of a field-naming
/// param; and either shape can instead hand over the whole property BAG, one
/// level deeper again. Reading all three, for every op, is what makes the
/// refusal route-agnostic — an op allowlist has to be extended per route and
/// silently admits the ones nobody remembered (measured twice: `set_field`,
/// then the bag).
///
/// Which param carries a bag comes from the SCHEMA
/// ([`WriteSchema::OVERFLOW_COLUMN`]), not from a list of operations, so a
/// fourth route cannot be opened by inventing another op name.
fn authored_property_keys(op_name: &str, params: &StorageEntity) -> Result<Vec<String>> {
    let bag = WriteSchema::OVERFLOW_COLUMN;
    let mut keys: Vec<String> = params.keys().map(|k| k.to_string()).collect();

    for (name_param, value_param) in FIELD_NAMING_PARAMS {
        let Some(named) = params.get(*name_param).and_then(|v| v.as_string()) else {
            continue;
        };
        keys.push(named.to_string());
        // The named field IS the overflow column, so `value` is the whole bag.
        if named == bag
            && let Some(v) = params.get(*value_param)
        {
            keys.extend(bag_keys(op_name, v)?);
        }
    }

    // The bag handed over directly as a param (`create`/`update`).
    if let Some(v) = params.get(bag) {
        keys.extend(bag_keys(op_name, v)?);
    }
    Ok(keys)
}

/// Refuse ANY op that would write a key the ENGINE mints (ruling D5.a).
///
/// Two distinct silent failures, one refusal: on `create`/`update` the stamp
/// below is an `insert`, which REPLACES the authored value; through `set_field`
/// the authored value would instead be taken as authoritative by the
/// substrate-rebuild read (`history_store.rs`) and the trust supervision view.
/// Reserved by EXACT spelling, never by the `_` prefix: see
/// [`ENGINE_OWNED_PARAM_KEYS`].
fn reject_engine_owned_keys(op_name: &str, params: &StorageEntity) -> Result<()> {
    for key in authored_property_keys(op_name, params)? {
        if let Some(owned) = ENGINE_OWNED_PARAM_KEYS.iter().find(|k| **k == key) {
            anyhow::bail!(
                "'{op_name}' would write the engine-owned property key '{owned}', which the \
                 engine mints itself — the write is REFUSED rather than silently overwriting or \
                 forging the stamp. Remove '{owned}' from the operation; the stamp is derived \
                 from the operation's origin."
            );
        }
    }
    Ok(())
}

/// Inject the `_provenance` property into an authoring op's params. Pure and
/// clock-free (the timestamp is passed in) so it is directly unit-testable.
fn stamp_params(
    op_name: &str,
    mut params: StorageEntity,
    origin: &OpOrigin,
    now_millis: i64,
) -> Result<StorageEntity> {
    reject_engine_owned_keys(op_name, &params)?;
    if PROVENANCE_STAMPED_OPS.contains(&op_name) {
        let stamp = ProvenanceStamp::from_origin(origin, now_millis);
        params.insert(Arc::from(PROVENANCE_PROPERTY), stamp.to_value());
    }
    Ok(params)
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
pub(crate) fn properties_object(value: &Value) -> Result<std::collections::HashMap<String, Value>> {
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
            vocabulary_source: None,
            trust_policy: Arc::new(TrustPolicy::trust_all()),
            entity_write_locks: EntityWriteLocks::default(),
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

    /// Wire the owning-document `#+TODO:` vocabulary resolver. Without it every
    /// keyword parse judges blocks against the DEFAULT keywords, which
    /// disagrees with the parser in any document that declares its own — so an
    /// unwired source is announced at WARN, never degraded quietly.
    pub fn with_task_vocabulary_source(
        mut self,
        source: Arc<dyn crate::core::task_keyword_promotion::TaskVocabularySource>,
    ) -> Self {
        self.vocabulary_source = Some(source);
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
            vocabulary_source: None,
            trust_policy: Arc::new(TrustPolicy::trust_all()),
            entity_write_locks: EntityWriteLocks::default(),
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

    /// Open a composite-undo group (Inc1). While open, every User-origin op
    /// dispatched through [`execute_operation`](Self::execute_operation) is
    /// buffered into ONE composite [`UndoEntry`] instead of pushing its own —
    /// so a multi-op compound (template instantiation) is ONE undo gesture.
    /// Nestable (flatten): only the outermost
    /// [`end_undo_group`](Self::end_undo_group) materializes the composite.
    /// Per-op provenance stamping and the history relation are UNCHANGED
    /// (they run per sub-op); only the undo bookkeeping is grouped. The
    /// lock is acquired and released here — the fan-out's own per-op pushes
    /// re-acquire it, so there is no re-entrant hold.
    pub async fn begin_undo_group(&self) {
        self.undo_stack.write().await.begin_group();
    }

    /// Close the innermost composite-undo group opened by
    /// [`begin_undo_group`](Self::begin_undo_group). At the outermost close the
    /// buffered sub-ops materialize as one composite entry (forward ops in
    /// order, inverses reversed leaf-first) and the snapshot is persisted. Loud
    /// on imbalance.
    pub async fn end_undo_group(&self) -> Result<()> {
        self.undo_stack.write().await.end_group();
        self.persist().await
    }

    /// Test-only: push a hand-crafted [`UndoEntry`] directly onto the stack.
    /// Used to exercise the composite-inverse REPLAY paths (e.g.
    /// partial-failure index naming) that natural provider ops cannot force
    /// — the SQL provider's `delete` cascades and its `create` is an
    /// idempotent upsert, so neither fails on a well-formed tree.
    #[cfg(test)]
    pub(crate) async fn push_undo_entry_for_test(&self, entry: UndoEntry) {
        self.undo_stack.write().await.push(entry);
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
    ) -> Result<StorageEntity> {
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

    /// Journal a completed step and persist the snapshot.
    ///
    /// Taking the entity's write guard by REFERENCE is the point of this
    /// signature: the undo stack's order is the write order only while the step
    /// that produced the entry still holds the entity, so releasing the stripe
    /// before journaling stops compiling rather than silently reintroducing the
    /// reorder. `None` is for ops that name no entity — a `create` that mints
    /// its own id has no prior state to order against.
    async fn journal_step(
        &self,
        // ALLOW(unused_param): the held guard IS the compile-time evidence the
        // stripe lock is held during journaling; naming it for use would let the
        // step release the stripe early without a compile error (task #29).
        _held: Option<&tokio::sync::MutexGuard<'_, ()>>,
        entry: UndoEntry,
    ) -> Result<()> {
        self.undo_stack.write().await.push(entry);
        self.persist().await
    }

    /// Take every stripe one undo/redo gesture will write, for the whole
    /// gesture. One entry replays N ops; holding their stripes across the
    /// staleness check AND every replay is what stops an external write landing
    /// between the check and the replay, or between two inverses of one
    /// composite entry (task #47).
    ///
    /// Ops that name no `id` contribute no stripe — a `create` that mints its
    /// own id has no prior state to race for, exactly as in [`Self::replay`].
    async fn lock_entry(&self, ops: &[Operation]) -> Vec<tokio::sync::MutexGuard<'_, ()>> {
        self.entity_write_locks
            .lock_all(ops.iter().filter_map(|op| {
                op.params
                    .get("id")
                    .and_then(Value::as_string)
                    .map(|id| (op.entity_name.as_str(), id))
            }))
            .await
    }

    /// Dispatch a stored op verbatim (used for inverse/forward replay). Never
    /// pushes an undo entry — replays bypass the push path by construction.
    /// Replay one undo/redo op through the dispatcher, returning the field
    /// deltas it produced. The caller aggregates these to decide whether the
    /// whole entry made an observable change (see
    /// [`Self::changes_are_vacuous`]): a provably-vacuous replay
    /// (identical-content set_field) must be reported
    /// as [`UndoOutcome::NoChange`], never as `Applied`.
    async fn replay(
        &self,
        op: &Operation,
        // ALLOW(unused_param): the held guards ARE the compile-time evidence
        // the gesture's stripes are held across this replay; naming them for
        // use would let a caller replay outside the hold without a compile
        // error, reopening the check-to-replay window (task #47).
        _held: &[tokio::sync::MutexGuard<'_, ()>],
    ) -> Result<Vec<FieldDelta>> {
        let params: StorageEntity = op
            .params
            .iter()
            .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
            .collect();
        // A replay is a write like any other, so the convergence rule applies:
        // undoing a promotion restores keyword-headed text, which IS the task
        // again. The gesture is semantically void by construction — the escape
        // is further undos through the typing ops that built the keyword.
        let (params, converged) = self
            .converge_block_write(&op.entity_name, &op.op_name, params)
            .await?;
        let mut result = self
            .dispatcher
            .execute_operation(&op.entity_name, &op.op_name, params)
            .await
            .map_err(|e| anyhow::anyhow!("undo/redo replay of '{}' failed: {e}", op.op_name))?;
        if let Some((id, promotion)) = &converged {
            let (_, _, ch) = self.write_converged_task_state(id, promotion).await?;
            result.changes.extend(ch);
        }
        let (_, _, post_ch) = self
            .converge_after_write(&op.entity_name, &result.changes)
            .await?;
        result.changes.extend(post_ch);
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
        use holon_api::template_instantiation::InstantiateRequest;
        use holon_api::template_instantiation::plan_instantiation;

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
        if let Some(replace_id) = &request.replace_block
            && !source.exists(replace_id).await?
        {
            bail!("instantiate_template: replace_block '{replace_id}' does not exist");
        }
        let nodes = source.load_subtree(&request.template_id).await?;
        let plan = plan_instantiation(&nodes, &request)?;

        let block_entity = EntityName::new("block");
        let creates = plan.creates;
        let replace_block = request.replace_block.clone();
        let root_id = plan.root_id;

        // Composite undo (Inc3): the whole instantiation is ONE user gesture, so
        // wrap the per-create fan-out (+ any empty→in-place delete) in ONE undo
        // group. Each sub-op still RE-ENTERS `execute_operation` UNCHANGED — C2a
        // provenance stamping, the C2b history relation, and per-op undo
        // classification all apply verbatim; only the undo PUSH is buffered, so
        // the N per-create entries collapse into ONE composite entry (inverse =
        // leaf-first deletes, so one undo removes every instance block). A
        // Rule/Sync-origin instantiation buffers nothing (the push is
        // User-gated), so its group materializes NOTHING.
        self.begin_undo_group().await;
        let fanout: Result<()> = async {
            for create_params in creates {
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
            // now delete the empty block it supersedes. Ordered AFTER the creates
            // so a failed instantiation never destroys the target (the block is
            // empty, so this never touches existing content). Routed through the
            // normal `delete` op → provenance/history/undo classification apply.
            if let Some(replace_id) = &replace_block {
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
            Ok(())
        }
        .await;
        // ALWAYS close the group — even on a mid-fan-out failure — so a partial
        // instantiation is ONE undoable composite (of the sub-ops that landed)
        // and never leaks an open group into the next operation.
        self.end_undo_group().await?;
        fanout?;
        Ok(Some(Value::String(root_id)))
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

        // The origin block is read by the planner, rewritten by the
        // constituents, and fingerprinted by the composite entry — a
        // read-modify-write-journal over one entity, exactly the step
        // [`EntityWriteLocks`] exists to make atomic. Held for the whole
        // compound (the constituents dispatch straight to the dispatcher, so
        // this hold never nests). DISCLOSED SCOPE: the other blocks a convert
        // touches — the minted page, the re-homed children, the re-pointed
        // linkers — get no stripe, so a concurrent write to one of THOSE can
        // still race; the origin is the block a user is typing in when they
        // reach for this chord, and one caret cannot be in two blocks.
        let origin_guard = self
            .entity_write_locks
            .lock_target(block.as_str(), params, "target")
            .await;

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

        // The window a test must be able to force: everything this compound read
        // above is about to be rewritten below. Holding the origin's stripe, a
        // writer queued behind it cannot run here; drop the hold and this pause
        // hands the block over, so the plan the constituents write from — and
        // the inverse they journal — describes content the user has already
        // moved past. Wide enough for a whole competing write, because that is
        // the event being made forceable.
        #[cfg(feature = "test-yield")]
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // RECOGNITION (resolve-before-mint, ADR 0029): before materializing the
        // destination page P, recognize whether `plan.page_id` is ALREADY held by
        // a DIFFERENT-titled entity — the state a `RenamePage` leaves (title
        // changed, id preserved). Minting P there would clobber the rename. Read
        // the id's current holder from the live-state reader (the projected
        // `block_raw` base table — mode-correct in BOTH Turso and Loro authority
        // modes, the SAME source the journal rule's inhibitor reads) and REFUSE
        // BEFORE dispatching any constituent, so no partial state is left. Free or
        // same-title ids proceed (a fresh mint, or an idempotent upsert of
        // unchanged content). The reference mirrors this refusal with the SAME
        // `recognize_derived_id`; the SUT driver tolerates the `IdentityCollision`.
        if let Some(reader) = &self.reader {
            let holder_title = reader
                .field_value(&plan.page_id, "content")
                .await?
                .and_then(|v| v.as_string().map(str::to_string));
            // ALLOW(entity_uri_from_raw): plan.page_id is a derived PageId::for_path id.
            let page_uri = holon_api::EntityUri::from_raw(&plan.page_id);
            if let holon_api::Recognition::Collision(collision) = holon_api::recognize_derived_id(
                &page_uri,
                holder_title.as_deref(),
                &plan.origin_content,
            ) {
                return Err(anyhow::Error::new(collision));
            }
        }

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
            p.insert("tags".into(), page_tag());
            let p = self.stamp_provenance("create", p, origin)?;
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
        pc.insert("tags".into(), page_tag());
        if let Value::String(marks) = &plan.origin_marks
            && !marks.is_empty()
            && marks != "[]"
        {
            pc.insert("marks".into(), Value::String(marks.clone()));
        }
        let pc = self.stamp_provenance("create", pc, origin)?;
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
            // EVERY child travels, so rule machinery among them keeps the
            // siblings it is read with and the net gate's separation refusal
            // does not apply. One move's delta cannot show that, so this loop
            // states it.
            mp.insert(
                crate::api::net_guard::CONFIRM_BREAK_PARAM.into(),
                Value::Boolean(true),
            );
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
        //
        // The span must match the origin's PERSISTED content. Storage trims
        // trailing whitespace on every write (SqlOperationProvider::
        // trimmed_content), so deriving the span from a raw/untrimmed
        // `origin_content` would mint a Link longer than the text it decorates
        // — an out-of-bounds mark that aborts EVERY render in
        // `scalar_range_to_bytes`. Derive both label and span from the trimmed
        // content (a no-op for the already-trimmed planner read, robust against
        // any untrimmed source). The span is then in bounds by construction —
        // it is derived from the very string it decorates.
        let label = plan.origin_content.trim_end().to_string();
        // Non-empty by `sanitize_page_title`'s contract, which the planner
        // enforces by refusing to build a plan without a title
        // (`sql_operation_provider.rs:3765`). Asserted because that guarantee
        // lives in another crate, and an empty label would mint a ZERO-WIDTH
        // Link mark that every read boundary silently DROPS rather than reports.
        assert!(
            !label.is_empty(),
            "convert_block_to_page: origin {} produced an empty page title — \
             sanitize_page_title must reject it before a plan exists",
            plan.origin_id
        );
        let link_marks = vec![MarkSpan::new(
            0,
            label.chars().count(),
            InlineMark::Link {
                target: EntityRef::from_uri(&convert_page_uri(&plan.page_id)),
                label: label.clone(),
            },
        )];
        let mut sf = StorageEntity::new();
        sf.insert("id".into(), Value::String(plan.origin_id.clone()));
        sf.insert("field".into(), Value::String("marks".into()));
        sf.insert("value".into(), Value::String(marks_to_json(&link_marks)));
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
            // derived order key `sort_key`. Structural ops RECOMPUTE it from the
            // live tree rather than restoring the captured value, so its
            // post-undo value is a function of the current sibling set, not the
            // pre-op value — fingerprinting it makes a legitimate undo→redo trip
            // spuriously "stale". (The moved blocks' parent_id — the field that
            // actually defines the re-home — is still fingerprinted, so real
            // external edits are caught.)
            let fp_changes: Vec<FieldDelta> = all_changes
                .iter()
                .filter(|d| d.field != "sort_key")
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
            // The second forced window: this entry describes state the
            // constituents just wrote. Under the hold no other writer can reach
            // the origin here; without it, a competing write overtakes and the
            // entry is born stale — dropped by the very next undo.
            #[cfg(feature = "test-yield")]
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.journal_step(origin_guard.as_ref(), entry).await?;
        }
        // The origin's read-modify-write-journal step is complete.
        drop(origin_guard);

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

    /// One `set_field` constituent of a task-keyword compound, dispatched
    /// straight to the dispatcher (so the compound's stripe is never
    /// re-entered) and returning its forward op, its inverse and its field
    /// deltas. `op` names the compound in the failure messages.
    async fn dispatch_task_keyword_constituent(
        &self,
        op: &str,
        params: StorageEntity,
    ) -> Result<(Operation, Operation, Vec<FieldDelta>)> {
        let block = EntityName::new("block");
        let forward = Operation::new(
            block.clone(),
            "set_field",
            "set_field",
            params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        );
        let result = self
            .dispatcher
            .execute_operation(&block, "set_field", params)
            .await
            .map_err(|e| anyhow::anyhow!("{op}: constituent set_field failed: {e}"))?;
        let inverse = match result.undo {
            UndoAction::Undo(inv) => inv,
            UndoAction::DeclaredIrreversible(reason) => bail!(
                "{op}: constituent set_field is irreversible ({reason}) — refusing to ship a \
                 partial-undo gesture"
            ),
            UndoAction::Undeclared => {
                bail!("{op}: constituent set_field returned an Undeclared undo classification")
            }
        };
        Ok((forward, inverse, result.changes))
    }

    /// The guard's view of live state: the block's persisted content and the
    /// keyword of its `task_state`, if any. Fails loud on a missing block —
    /// writing a task keyword onto a row that is not there would write a ghost.
    async fn read_task_keyword_prior_state(
        &self,
        op: &str,
        id: &str,
    ) -> Result<(String, Option<String>)> {
        let reader = self.reader.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "{op} needs a live-state reader to resolve the block's current task state; this \
                 engine was built without one"
            )
        })?;
        if reader.field_value(id, "id").await?.is_none() {
            bail!("{op}: block {id} does not exist");
        }
        let content = reader
            .field_value(id, "content")
            .await?
            .and_then(|v| v.as_string().map(str::to_string))
            .unwrap_or_default();
        let keyword = match reader.field_value(id, "properties").await? {
            None | Some(Value::Null) => None,
            Some(Value::Object(map)) => map
                .get("task_state")
                .and_then(|v| v.as_string().map(str::to_string)),
            Some(Value::String(json)) | Some(Value::Json(json)) if !json.trim().is_empty() => {
                let parsed: serde_json::Value = serde_json::from_str(&json)
                    .map_err(|e| anyhow::anyhow!("{op}: corrupt properties JSON on {id}: {e}"))?;
                parsed
                    .get("task_state")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }
            Some(Value::String(_)) | Some(Value::Json(_)) => None,
            Some(other) => {
                bail!("{op}: block {id} has a non-object `properties` value {other:?}")
            }
        };
        Ok((content, keyword))
    }

    /// The owning document's task-keyword vocabulary. Read at use and never
    /// cached: a `#+TODO:` line is ordinary editable content, so a vocabulary
    /// resolved once goes stale the moment the user edits it.
    async fn document_vocabulary(
        &self,
        op: &str,
        id: &str,
    ) -> Result<holon_org_format::TaskKeywordVocabulary> {
        match &self.vocabulary_source {
            Some(source) => source.vocabulary_for_block(id).await,
            None => {
                tracing::warn!(
                    block = %id,
                    op,
                    "no task-vocabulary source wired; judging against the DEFAULT keywords, which \
                     disagrees with the parser in any document declaring #+TODO:"
                );
                Ok(holon_org_format::TaskKeywordVocabulary::default())
            }
        }
    }

    /// The block's stored `task_state` keyword, or `None` when it carries none
    /// — or when the row is not there yet (a `create` converges on its own
    /// params). Unlike [`Self::read_task_keyword_prior_state`] this tolerates a
    /// missing row, because it runs on writes that mint one.
    async fn stored_task_keyword(&self, id: &str) -> Result<Option<String>> {
        let Some(reader) = self.reader.as_ref() else {
            return Ok(None);
        };
        let keyword = match reader.field_value(id, "properties").await? {
            None | Some(Value::Null) => None,
            Some(Value::Object(map)) => map
                .get("task_state")
                .and_then(|v| v.as_string())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            Some(Value::String(json)) | Some(Value::Json(json)) if !json.trim().is_empty() => {
                let parsed: serde_json::Value = serde_json::from_str(&json).map_err(|e| {
                    anyhow::anyhow!(
                        "task-keyword convergence: corrupt properties JSON on {id}: {e}"
                    )
                })?;
                parsed
                    .get("task_state")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            }
            Some(Value::String(_)) | Some(Value::Json(_)) => None,
            Some(other) => bail!(
                "task-keyword convergence: block {id} has a non-object `properties` value {other:?}"
            ),
        };
        Ok(keyword)
    }

    /// The vault format's illegal-state rule: a block that carries no task
    /// state and whose content is keyword-headed in its OWN document's
    /// vocabulary is not representable — org reads exactly those bytes back as
    /// a task, so the store would hold a reading the file disagrees with.
    /// Returns the task the content already IS.
    ///
    /// The vocabulary-free shape gate runs first, so an ordinary content write
    /// costs no document read at all; a format provider that declares no
    /// keywords converges nothing.
    async fn keyword_convergence(
        &self,
        id: &str,
        content: &str,
        write_sets_task_state: bool,
    ) -> Result<Option<holon_org_format::Promotion>> {
        if write_sets_task_state
            || !holon_org_format::could_converge(content)
            || self.stored_task_keyword(id).await?.is_some()
        {
            return Ok(None);
        }
        let vocabulary = self
            .document_vocabulary(CONVERGE_TASK_KEYWORD_OP, id)
            .await?;
        if self.reader.is_none() && holon_org_format::keyword_headed(content, &vocabulary).is_some()
        {
            tracing::warn!(
                block = %id,
                "content is keyword-headed but this engine has no live-state reader, so the block \
                 cannot be proven untasked; leaving the unconverged state"
            );
            return Ok(None);
        }
        Ok(holon_org_format::converge_keyword_headed(
            content,
            &vocabulary,
        ))
    }

    /// The content value a block write would land, and whether that same write
    /// supplies a task state. `None` for writes that set no content.
    fn intended_content<'a>(op_name: &str, params: &'a StorageEntity) -> Option<(&'a str, bool)> {
        match op_name {
            "set_field" => (params.get("field").and_then(|v| v.as_string()) == Some("content"))
                .then(|| {
                    (
                        params
                            .get("value")
                            .and_then(|v| v.as_string())
                            .unwrap_or(""),
                        false,
                    )
                }),
            "create" | "update" => params
                .get("content")
                .and_then(|v| v.as_string())
                .map(|c| (c, params.contains_key("task_state"))),
            _ => None,
        }
    }

    /// Rewrite a block write that would land the illegal state into the task it
    /// already is, BEFORE it reaches the store. Pre-rewriting rather than
    /// repairing afterwards is what keeps the unconverged content from ever
    /// being observable — and keeps the editor's `write_seq` matched to exactly
    /// one content write, so its echo discriminator still recognises its own
    /// keystroke.
    async fn converge_block_write(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        mut params: StorageEntity,
    ) -> Result<(StorageEntity, Option<(String, holon_org_format::Promotion)>)> {
        if entity_name.as_str() != "block" {
            return Ok((params, None));
        }
        let Some(id) = params
            .get("id")
            .and_then(|v| v.as_string())
            .map(str::to_string)
        else {
            return Ok((params, None));
        };
        let Some((content, sets_task_state)) = Self::intended_content(op_name, &params) else {
            return Ok((params, None));
        };
        let content = content.to_string();
        let Some(promotion) = self
            .keyword_convergence(&id, &content, sets_task_state)
            .await?
        else {
            return Ok((params, None));
        };
        let field = if op_name == "set_field" {
            "value"
        } else {
            "content"
        };
        params.insert(Arc::from(field), Value::String(promotion.stripped.clone()));
        Ok((params, Some((id, promotion))))
    }

    /// Converge every block whose content a write LEFT keyword-headed. The
    /// pre-rewrite above covers writes that name their content; this covers the
    /// ones that compute it inside the provider (`split_block`, the merge
    /// family), which surface only as field deltas.
    async fn converge_after_write(
        &self,
        entity_name: &EntityName,
        changes: &[FieldDelta],
    ) -> Result<(Vec<Operation>, Vec<Operation>, Vec<FieldDelta>)> {
        let mut forwards = Vec::new();
        let mut inverses = Vec::new();
        let mut extra_changes = Vec::new();
        if entity_name.as_str() != "block" {
            return Ok((forwards, inverses, extra_changes));
        }
        for delta in changes {
            if delta.field != "content" {
                continue;
            }
            let Some(content) = delta.new_value.as_string() else {
                continue;
            };
            let Some(promotion) = self
                .keyword_convergence(&delta.entity_id, content, false)
                .await?
            else {
                continue;
            };
            for (field, value) in [
                ("content", promotion.stripped.clone()),
                ("task_state", promotion.keyword.keyword.clone()),
            ] {
                let mut p = StorageEntity::new();
                p.insert("id".into(), Value::String(delta.entity_id.clone()));
                p.insert("field".into(), Value::String(field.to_string()));
                p.insert("value".into(), Value::String(value));
                let (fwd, inv, ch) = self
                    .dispatch_task_keyword_constituent(CONVERGE_TASK_KEYWORD_OP, p)
                    .await?;
                forwards.push(fwd);
                // Leaf-first: undo drops the task state before restoring the
                // text it was derived from.
                inverses.insert(0, inv);
                extra_changes.extend(ch);
            }
        }
        Ok((forwards, inverses, extra_changes))
    }

    /// The `task_state` write that pairs a pre-rewritten content write.
    async fn write_converged_task_state(
        &self,
        id: &str,
        promotion: &holon_org_format::Promotion,
    ) -> Result<(Operation, Operation, Vec<FieldDelta>)> {
        let mut p = StorageEntity::new();
        p.insert("id".into(), Value::String(id.to_string()));
        p.insert("field".into(), Value::String("task_state".into()));
        p.insert(
            "value".into(),
            Value::String(promotion.keyword.keyword.clone()),
        );
        self.dispatch_task_keyword_constituent(CONVERGE_TASK_KEYWORD_OP, p)
            .await
    }

    /// Write a block's full vault source: parse [`SOURCE_TEXT_FIELD`] under the
    /// owning document's vocabulary and land `content` + `task_state` as ONE
    /// reversible gesture. Params: `id`, `value` (the raw source) and an
    /// optional `write_seq` forwarded to the content write so the editor still
    /// recognises its own echo.
    ///
    /// The parse is total — there is no refusal. Source that is keyword-headed
    /// yields the task it spells; source that is not yields plain content AND
    /// clears any `task_state` the block carried, which is how a user deletes
    /// the keyword out of the editable surface and demotes the block.
    async fn run_set_source_text(
        &self,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<Option<Value>> {
        use holon_org_format::converge_keyword_headed;

        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("set_field({SOURCE_TEXT_FIELD}): missing 'id' param"))?;
        let source = params
            .get("value")
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("set_field({SOURCE_TEXT_FIELD}): missing 'value' param")
            })?;
        let write_seq = params.get("write_seq").cloned();

        // Read-modify-write-journal over ONE entity, the same step the other
        // task-keyword compounds take; the constituents go straight to the
        // dispatcher so the hold never nests.
        let write_guard = self
            .entity_write_locks
            .lock_target("block", params, "id")
            .await;

        let vocabulary = self.document_vocabulary(SOURCE_TEXT_FIELD, &id).await?;
        let parsed = converge_keyword_headed(&source, &vocabulary);
        let (content, keyword) = match &parsed {
            Some(p) => (p.stripped.clone(), p.keyword.keyword.clone()),
            // Empty string is how `set_field("task_state")` clears the property
            // — the same value `cycle_task_state` writes for the blank ring
            // slot, so demotion reuses the existing clearing path rather than
            // inventing one.
            None => (source.clone(), String::new()),
        };

        let constituent = |field: &str, value: &str| {
            let mut p = StorageEntity::new();
            p.insert("id".into(), Value::String(id.clone()));
            p.insert("field".into(), Value::String(field.to_string()));
            p.insert("value".into(), Value::String(value.to_string()));
            if field == "content"
                && let Some(seq) = &write_seq
            {
                p.insert("write_seq".into(), seq.clone());
            }
            p
        };

        // Source that carries no keyword on a block that carries no task state
        // has nothing to clear. Writing the empty keyword anyway would ADD a
        // blank `task_state` property to every plain block whose text merely
        // has the SHAPE of a keyword — a state no other write path produces.
        let prior_keyword = self.stored_task_keyword(&id).await?;
        let clears_nothing = parsed.is_none() && prior_keyword.is_none();
        // A task-state change is a change even when the content constituent is a
        // no-op (`TODO ` on an empty block re-writes the same empty content).
        // The provider reports property writes without a field delta, so the
        // delta-only vacuity test would judge that gesture unundoable.
        let keyword_changed = prior_keyword.unwrap_or_default() != keyword;

        let (c_fwd, c_inv, mut changes) = self
            .dispatch_task_keyword_constituent(SOURCE_TEXT_FIELD, constituent("content", &content))
            .await?;
        let mut forwards = vec![c_fwd];
        // Leaf-first: undo drops the task state before restoring the text it
        // was derived from.
        let mut inverses = vec![c_inv];
        if !clears_nothing {
            let (t_fwd, t_inv, t_changes) = self
                .dispatch_task_keyword_constituent(
                    SOURCE_TEXT_FIELD,
                    constituent("task_state", &keyword),
                )
                .await?;
            changes.extend(t_changes);
            forwards.push(t_fwd);
            inverses.insert(0, t_inv);
        }

        if origin.is_user() && (keyword_changed || !Self::changes_are_vacuous(&changes)) {
            let entry = UndoEntry {
                ops: forwards,
                inverse_ops: inverses,
                origin: OpOrigin::User,
                group_id: 0,
                precondition: Precondition::forward(&changes),
                redo_precondition: Precondition::inverse(&changes),
            };
            self.journal_step(write_guard.as_ref(), entry).await?;
        }
        drop(write_guard);

        if let Some(history) = &self.history {
            self.record_history(history.as_ref(), "block", "set_field", origin, &changes)
                .await?;
        }

        let mut payload = std::collections::HashMap::new();
        payload.insert("content".to_string(), Value::String(content));
        payload.insert("task_state".to_string(), Value::String(keyword));
        Ok(Some(Value::Object(payload)))
    }

    /// Advance a block one step around its document's task-state ring. See
    /// [`CYCLE_TASK_STATE_OP`]. Params: `id`. Returns the keyword written.
    ///
    /// The vocabulary is read at use and never cached: a document's `#+TODO:`
    /// line is ordinary editable content, so a ring resolved once would go
    /// stale the moment the user edits it.
    async fn run_cycle_task_state(
        &self,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<Option<Value>> {
        use crate::core::task_keyword_cycle::cycle_ring;

        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("cycle_task_state: missing 'id' param"))?;

        // Read-modify-write-journal over ONE entity, the same step the
        // promotion compound takes; the constituent goes straight to the
        // dispatcher so the hold never nests.
        let write_guard = self
            .entity_write_locks
            .lock_target("block", params, "id")
            .await;

        let vocabulary = self.document_vocabulary(CYCLE_TASK_STATE_OP, &id).await?;
        let (_content, prior_keyword) = self
            .read_task_keyword_prior_state(CYCLE_TASK_STATE_OP, &id)
            .await?;
        let ring = cycle_ring(&vocabulary);
        let next = holon_api::render_eval::cycle_state(
            prior_keyword.as_deref().unwrap_or_default(),
            &ring,
        );

        let mut set_field_params = StorageEntity::new();
        set_field_params.insert("id".into(), Value::String(id.clone()));
        set_field_params.insert("field".into(), Value::String("task_state".into()));
        set_field_params.insert("value".into(), Value::String(next.clone()));
        // `set_field("task_state")` pairs the `task_state_category` sidecar in
        // the same write, so the pair invariant holds without a second op.
        let (forward, inverse, changes) = self
            .dispatch_task_keyword_constituent(CYCLE_TASK_STATE_OP, set_field_params)
            .await?;

        if origin.is_user() {
            let entry = UndoEntry {
                ops: vec![forward],
                inverse_ops: vec![inverse],
                origin: OpOrigin::User,
                group_id: 0,
                precondition: Precondition::forward(&changes),
                redo_precondition: Precondition::inverse(&changes),
            };
            self.journal_step(write_guard.as_ref(), entry).await?;
        }
        drop(write_guard);

        if let Some(history) = &self.history {
            self.record_history(
                history.as_ref(),
                "block",
                CYCLE_TASK_STATE_OP,
                origin,
                &changes,
            )
            .await?;
        }

        Ok(Some(Value::String(next)))
    }

    /// Execute the duplicate-identity merge. See [`MERGE_BLOCKS_OP`]. Params:
    /// `canonical` and `duplicate` (block ids). Returns the canonical id.
    async fn run_merge_blocks(
        &self,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<Option<Value>> {
        use crate::core::merge_blocks_plan::MergeBlocksPlan;
        use crate::core::merge_blocks_plan::normalize_content;

        let block = EntityName::new("block");

        // Same read-modify-write-journal step as the block→page compound, over
        // the CANONICAL block: the planner reads its content, the content step
        // rewrites it, and the composite entry fingerprints it. Held for the
        // whole merge; the constituents dispatch straight to the dispatcher, so
        // this hold never nests. DISCLOSED SCOPE: the duplicate and the moved
        // children get no stripe — one hold at a time is what keeps the striping
        // deadlock-free without a lock order, and the canonical is the block the
        // merge rewrites in place.
        let canonical_guard = self
            .entity_write_locks
            .lock_target(block.as_str(), params, "canonical")
            .await;

        // 1. Plan (read-only). Every precondition is enforced here, so a refusal
        //    happens before the first write.
        let plan_result = self
            .dispatcher
            .execute_operation(&block, "merge_blocks_plan", params.clone())
            .await
            .map_err(|e| anyhow::anyhow!("merge_blocks: {e}"))?;
        let plan_value = plan_result
            .response
            .ok_or_else(|| anyhow::anyhow!("merge_blocks: planner returned no plan payload"))?;
        let plan = MergeBlocksPlan::from_value(&plan_value)
            .map_err(|e| anyhow::anyhow!("merge_blocks: {e}"))?;

        // Same forced window as the block→page compound: see there.
        #[cfg(feature = "test-yield")]
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Inverses are bucketed per STEP so undo can replay the steps in reverse,
        // each bucket in strict LIFO of its own forward ops (see the assembly).
        let mut forwards: Vec<Operation> = Vec::new();
        let mut move_invs: Vec<Operation> = Vec::new();
        let mut dedupe_move_invs: Vec<Operation> = Vec::new();
        let mut dedupe_field_invs: Vec<Operation> = Vec::new();
        let mut dedupe_delete_invs: Vec<Operation> = Vec::new();
        let mut field_invs: Vec<Operation> = Vec::new();
        let mut all_changes: Vec<FieldDelta> = Vec::new();
        // Rows this merge REMOVES. Their deltas must stay out of the staleness
        // fingerprint: the guard reads the field back from `block_raw`, and a
        // deleted row answers nothing, so fingerprinting it would make every
        // merge's undo read as stale.
        let mut removed: Vec<String> = vec![plan.duplicate_id.clone()];

        // 2. Content: the canonical wins. An empty canonical adopts the duplicate's
        //    body; two differing bodies park the duplicate's as the canonical's FIRST
        //    CHILD rather than dropping it.
        //
        // Ordered BEFORE the moves and the dedupe so the parked body is an
        // ordinary child by the time collapsing runs — otherwise a parked body
        // equal to an existing child would leave a normalized-equal pair behind.
        // When its content ALREADY appears among the merged children, parking it
        // would BE that duplicate, so it is not created: the content survives in
        // the child that already carries it.
        let canonical_norm = normalize_content(&plan.canonical_content);
        let duplicate_norm = normalize_content(&plan.duplicate_content);
        let body_already_present = plan
            .merged_children
            .iter()
            .any(|c| normalize_content(&c.content) == duplicate_norm);
        if canonical_norm.is_empty() && !duplicate_norm.is_empty() {
            let mut sf = StorageEntity::new();
            sf.insert("id".into(), Value::String(plan.canonical_id.clone()));
            sf.insert("field".into(), Value::String("content".into()));
            sf.insert(
                "value".into(),
                Value::String(plan.duplicate_content.clone()),
            );
            let (fwd, inv, ch) = self.dispatch_merge_constituent("set_field", sf).await?;
            forwards.push(fwd);
            field_invs.push(inv);
            all_changes.extend(ch);
        } else if !duplicate_norm.is_empty()
            && duplicate_norm != canonical_norm
            && !body_already_present
        {
            let mut cp = StorageEntity::new();
            cp.insert(
                "id".into(),
                Value::String(merged_body_child_id(&plan.duplicate_id)),
            );
            cp.insert(
                "content".into(),
                Value::String(plan.duplicate_content.clone()),
            );
            cp.insert("parent_id".into(), Value::String(plan.canonical_id.clone()));
            cp.insert("after_block_id".into(), Value::Null);
            let cp = self.stamp_provenance("create", cp, origin)?;
            let (fwd, inv, ch) = self.dispatch_merge_constituent("create", cp).await?;
            forwards.push(fwd);
            field_invs.push(inv);
            all_changes.extend(ch);
        }

        // 3. Move the duplicate's children under the canonical, appended after its last
        //    existing child, order preserved.
        //
        // The children are moved in REVERSE document order, each anchored on the
        // SAME `tail` — which lands them in their original order all the same,
        // because each one is inserted just after the tail and thus ahead of the
        // ones already moved. The reason to do it this way is the INVERSE:
        // `move_block` captures the block's predecessor as it runs, so moving
        // front-to-back strips each child's predecessor out of the source parent
        // before the next one is captured, and every inverse degrades to "become
        // the first child" — replaying them then reverses the siblings. Going
        // back-to-front leaves each child's predecessor in place at capture time.
        let own = plan.canonical_child_count as usize;
        let tail: Option<String> = plan
            .merged_children
            .get(own.wrapping_sub(1))
            .map(|c| c.id.clone());
        for child in plan.merged_children.iter().skip(own).rev() {
            let mut mp = StorageEntity::new();
            mp.insert("id".into(), Value::String(child.id.clone()));
            mp.insert("parent_id".into(), Value::String(plan.canonical_id.clone()));
            mp.insert(
                "after_block_id".into(),
                match &tail {
                    Some(t) => Value::String(t.clone()),
                    None => Value::Null,
                },
            );
            // The duplicate's whole child set lands under the canonical, so
            // rule machinery among them keeps the siblings it is read with and
            // the net gate's separation refusal does not apply. One move's
            // delta cannot show that, so this loop states it.
            mp.insert(
                crate::api::net_guard::CONFIRM_BREAK_PARAM.into(),
                Value::Boolean(true),
            );
            let (fwd, inv, ch) = self.dispatch_merge_constituent("move_block", mp).await?;
            forwards.push(fwd);
            move_invs.push(inv);
            all_changes.extend(ch);
        }

        // 4. One-level dedupe: each loser's children are re-homed under the keeper
        //    BEFORE the loser is deleted behind its own redirect, so no subtree is ever
        //    orphaned.
        for group in &plan.dedupe_groups {
            let mut anchor: Option<String> = group.keeper_last_child.clone();
            let mut absorbed = group.keeper_merged_from.clone();
            for loser in &group.losers {
                // Back-to-front against a fixed anchor, for the same reason as
                // the child moves above: it preserves each orphan's predecessor
                // in the loser until `move_block` has captured it.
                for orphan in loser.children.iter().rev() {
                    let mut mp = StorageEntity::new();
                    mp.insert("id".into(), Value::String(orphan.clone()));
                    mp.insert("parent_id".into(), Value::String(group.keeper.clone()));
                    mp.insert(
                        "after_block_id".into(),
                        match &anchor {
                            Some(a) => Value::String(a.clone()),
                            None => Value::Null,
                        },
                    );
                    // Every orphan of the loser lands under the keeper, so this
                    // relocation carries a rule whole exactly as the child move
                    // above does.
                    mp.insert(
                        crate::api::net_guard::CONFIRM_BREAK_PARAM.into(),
                        Value::Boolean(true),
                    );
                    let (fwd, inv, ch) = self.dispatch_merge_constituent("move_block", mp).await?;
                    forwards.push(fwd);
                    dedupe_move_invs.push(inv);
                    all_changes.extend(ch);
                }
                // The next loser's orphans append after this loser's, which now
                // sit at the keeper's tail.
                if let Some(last) = loser.children.last() {
                    anchor = Some(last.clone());
                }
                // The loser's id keeps resolving, to the keeper.
                absorbed.push((loser.id.clone(), plan.merged_at));
                let (fwd, inv, ch) = self.write_merged_from(&group.keeper, &absorbed).await?;
                forwards.push(fwd);
                dedupe_field_invs.push(inv);
                all_changes.extend(ch);

                let mut dp = StorageEntity::new();
                dp.insert("id".into(), Value::String(loser.id.clone()));
                let (fwd, inv, ch) = self.dispatch_merge_constituent("delete", dp).await?;
                forwards.push(fwd);
                dedupe_delete_invs.push(inv);
                all_changes.extend(ch);
                removed.push(loser.id.clone());
            }
        }

        // 5. Tags union (a Page tag on either side survives) and the properties the
        //    canonical lacks.
        let mut tp = StorageEntity::new();
        tp.insert("id".into(), Value::String(plan.canonical_id.clone()));
        tp.insert("field".into(), Value::String("tags".into()));
        tp.insert(
            "value".into(),
            Value::Array(
                plan.union_tags
                    .iter()
                    .map(|t| Value::String(t.clone()))
                    .collect(),
            ),
        );
        let (fwd, inv, ch) = self.dispatch_merge_constituent("set_field", tp).await?;
        forwards.push(fwd);
        field_invs.push(inv);
        all_changes.extend(ch);

        for (key, value) in &plan.adopted_properties {
            let mut pp = StorageEntity::new();
            pp.insert("id".into(), Value::String(plan.canonical_id.clone()));
            pp.insert("field".into(), Value::String(key.clone()));
            pp.insert("value".into(), value.clone());
            let (fwd, inv, ch) = self.dispatch_merge_constituent("set_field", pp).await?;
            forwards.push(fwd);
            field_invs.push(inv);
            all_changes.extend(ch);
        }

        // 6. Provenance + redirect in ONE write: `merged_from` is the replicated fact,
        //    and `block_redirects` is re-derived from it at the SQL write boundary (so
        //    undo's property removal retracts the redirect too).
        let mut absorbed = plan.existing_merged_from.clone();
        absorbed.push((plan.duplicate_id.clone(), plan.merged_at));
        let (fwd, redirect_inv, ch) = self
            .write_merged_from(&plan.canonical_id, &absorbed)
            .await?;
        forwards.push(fwd);
        all_changes.extend(ch);

        // 7. Re-point inbound links duplicate → canonical (exact capture-based
        //    inverse).
        let mut rw = StorageEntity::new();
        rw.insert("from".into(), Value::String(plan.duplicate_id.clone()));
        rw.insert("to".into(), Value::String(plan.canonical_id.clone()));
        let (fwd, rewrite_inv, ch) = self
            .dispatch_merge_constituent("rewrite_link_resolution", rw)
            .await?;
        forwards.push(fwd);
        all_changes.extend(ch);

        // 8. The duplicate is now childless and its id redirects; delete it.
        let mut dp = StorageEntity::new();
        dp.insert("id".into(), Value::String(plan.duplicate_id.clone()));
        let (fwd, delete_inv, ch) = self.dispatch_merge_constituent("delete", dp).await?;
        forwards.push(fwd);
        all_changes.extend(ch);

        // Undo replays the steps in reverse: re-create the duplicate → restore
        // inbound links → drop the redirect → restore fields → undo the dedupe
        // → move the children back (each bucket in FORWARD order so
        // predecessors land before their followers).
        if origin.is_user() {
            let mut inverse_ops: Vec<Operation> = vec![delete_inv, rewrite_inv, redirect_inv];
            field_invs.reverse();
            inverse_ops.extend(field_invs);
            // Every bucket replays in strict LIFO of its forward ops: an inverse
            // anchors on the tree as it stood just before its own op, so the ops
            // after it must already have been undone. For the move buckets that
            // means each move-back finds its predecessor restored, since the
            // forward moves ran back-to-front.
            dedupe_delete_invs.reverse();
            inverse_ops.extend(dedupe_delete_invs);
            dedupe_field_invs.reverse();
            inverse_ops.extend(dedupe_field_invs);
            dedupe_move_invs.reverse();
            inverse_ops.extend(dedupe_move_invs);
            move_invs.reverse();
            inverse_ops.extend(move_invs);
            // Same rule as the block→page transform: fingerprint the literally
            // restored fields, never the order key structural ops recompute from
            // the live tree — and never a row this merge removes.
            let fp_changes: Vec<FieldDelta> = all_changes
                .iter()
                .filter(|d| d.field != "sort_key")
                .filter(|d| !removed.contains(&d.entity_id))
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
            // Same second forced window as the block→page compound: see there.
            #[cfg(feature = "test-yield")]
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.journal_step(canonical_guard.as_ref(), entry).await?;
        }
        // The canonical's read-modify-write-journal step is complete.
        drop(canonical_guard);

        if let Some(history) = &self.history {
            self.record_history(
                history.as_ref(),
                "block",
                MERGE_BLOCKS_OP,
                origin,
                &all_changes,
            )
            .await?;
        }

        Ok(Some(Value::String(plan.canonical_id)))
    }

    /// Dispatch ONE constituent of the merge, returning the forward op (for
    /// redo), its exact inverse (for undo) and its deltas. A constituent that
    /// cannot describe an inverse aborts the merge — a half-undoable merge is
    /// worse than a refused one.
    async fn dispatch_merge_constituent(
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
            .map_err(|e| anyhow::anyhow!("merge_blocks: constituent '{op_name}' failed: {e}"))?;
        let inverse = match result.undo {
            UndoAction::Undo(inv) => inv,
            UndoAction::DeclaredIrreversible(reason) => bail!(
                "merge_blocks: constituent '{op_name}' is irreversible ({reason}) — refusing to \
                 ship a partial-undo merge"
            ),
            UndoAction::Undeclared => bail!(
                "merge_blocks: constituent '{op_name}' returned an Undeclared undo classification"
            ),
        };
        Ok((forward, inverse, result.changes))
    }

    /// Write `absorbed` as `to_id`'s merge provenance. This ONE property write
    /// is both the replicated redirect record and the `:merged-from:` the org
    /// round-trip carries; the `block_redirects` index is re-derived from it at
    /// the SQL write boundary, so the `set_field` inverse retracts both.
    async fn write_merged_from(
        &self,
        to_id: &str,
        absorbed: &[(String, i64)],
    ) -> Result<(Operation, Operation, Vec<FieldDelta>)> {
        use crate::core::merge_blocks_plan::MERGED_FROM_FIELD;
        use crate::core::merge_blocks_plan::render_merged_from;

        let mut p = StorageEntity::new();
        p.insert("id".into(), Value::String(to_id.to_string()));
        p.insert("field".into(), Value::String(MERGED_FROM_FIELD.into()));
        p.insert("value".into(), Value::String(render_merged_from(absorbed)));
        self.dispatch_merge_constituent("set_field", p).await
    }

    /// The synthetic descriptor advertising the engine-level `merge_blocks` op
    /// so MCP discovers it like any provider op.
    pub(crate) fn merge_blocks_descriptor() -> OperationDescriptor {
        use holon_api::render_types::TypeHint;
        let param = |name: &str, description: &str| holon_api::OperationParam {
            name: name.to_string(),
            type_hint: TypeHint::String,
            description: description.to_string(),
        };
        OperationDescriptor {
            entity_name: EntityName::new("block"),
            entity_short_name: "block".to_string(),
            id_column: "id".to_string(),
            name: MERGE_BLOCKS_OP.to_string(),
            display_name: "Merge duplicate".to_string(),
            description: "Fold a duplicate block into the canonical one: move its children and \
                          content over, and keep its id resolving via a redirect."
                .to_string(),
            required_params: vec![
                param("canonical", "The surviving block id"),
                param("duplicate", "The block id folded away"),
            ],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            // Repair surface, not an authoring gesture: reached through MCP and
            // the identity tooling, never the slash menu.
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Internal,
            },
            // Both blocks already share an identity and thus an audience; the
            // merge moves nothing across a replication boundary.
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            trigger: None,
            bound_params: Default::default(),
            affected_fields: vec![],
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }
    }

    /// The synthetic descriptor advertising the engine-level
    /// `convert_block_to_page` op so MCP / the slash menu discover it like any
    /// provider op.
    pub(crate) fn convert_block_to_page_descriptor() -> OperationDescriptor {
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
            target_scope: holon_api::TargetScope::Block,
            menu_exposure: holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet {
                    slash_menu: true,
                    action_bar: false,
                },
            },
            // Restructures a block into a new page at the same placement, within
            // the SAME replication container (C1 containers are replication
            // units, not every page) — content stays in its current audience.
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            trigger: None,
            bound_params: Default::default(),
            affected_fields: vec![],
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
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
            target_scope: holon_api::TargetScope::Block,
            menu_exposure: holon_api::MenuExposure::PickerBacked {
                picker: holon_api::PickerKind::Template,
            },
            // Deep-copies a template subtree under target_parent within the same
            // container — never crosses an audience boundary.
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }
    }

    /// The engine-synthetic `block` operations that are NOT
    /// dispatcher-registered providers — the SINGLE source for both
    /// injection sites (the profile resolver's `entity_operations` map in
    /// `di::registration` and the MCP `available_operations` discovery
    /// list), so the two can never drift. `convert_block_to_page` is always
    /// present; `instantiate_template` only when a template source is wired
    /// (it is `PickerBacked`, surfaced via the template picker, never as a
    /// bare menu op).
    pub fn block_synthetic_descriptors(include_template_picker: bool) -> Vec<OperationDescriptor> {
        let mut ops = Vec::with_capacity(2);
        if include_template_picker {
            ops.push(Self::instantiate_template_descriptor());
        }
        ops.push(Self::convert_block_to_page_descriptor());
        ops.push(Self::merge_blocks_descriptor());
        ops
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
        let create = self.stamp_provenance("create", create, origin)?;

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
        let create = self.stamp_provenance("create", create, origin)?;
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
            .response
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

    /// Verify a precondition against live state. An EMPTY precondition asserts
    /// nothing and needs no reader. A non-empty one cannot be verified without
    /// a reader: every reversible op journals one, so refusing outright would
    /// take undo away entirely on a reader-less wiring. The replay proceeds
    /// UNVERIFIED and says so at WARN — degraded, disclosed, never silent
    /// (task #47).
    async fn check_stale(&self, precondition: &Precondition) -> Result<Option<String>> {
        if precondition.is_empty() {
            return Ok(None);
        }
        match &self.reader {
            Some(reader) => verify_precondition(reader.as_ref(), precondition).await,
            None => {
                tracing::warn!(
                    "undo/redo entry carries a {}-field precondition but this engine has no \
                     live-state reader wired, so staleness cannot be verified; replaying \
                     UNVERIFIED — an external write to these fields will be overwritten. \
                     Build the engine with `new_persistent` or `with_state_reader`.",
                    precondition.fields.len()
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
    ) -> Result<OpOutcome> {
        // Ruling D5.a, and it runs before the trust gate for that reason: a
        // sub-threshold op is captured into a proposal record VERBATIM, so a
        // refusal further down would store the reserved key and only reject it
        // at accept time, an operation the author never performed.
        reject_engine_owned_keys(op_name, &params)?;

        // Trust gate (VisionGapAnalysis C5): a sub-threshold (origin, entity,
        // op) never reaches canonical state — it is coerced into a proposal
        // emission under `block:proposals`. This runs FIRST so every shape
        // (plain ops, compounds, even accept/reject themselves) is governed by
        // the same place-topology rule; a trusted origin falls through with
        // zero behavior change.
        if self.trust_policy.decide(&origin, entity_name, op_name) == TrustDecision::Propose {
            return self
                .coerce_to_proposal(entity_name, op_name, params, &origin)
                .await
                .map(OpOutcome::proven);
        }

        // Engine-level compounds: proposal confirmation (C5). Acceptance
        // re-dispatches the wrapped op with the CONFIRMER's origin; rejection
        // retracts without executing.
        if entity_name.as_str() == "block" {
            if op_name == ACCEPT_PROPOSAL_OP {
                return self
                    .run_resolve_proposal(&params, &origin, true)
                    .await
                    .map(OpOutcome::proven);
            }
            if op_name == REJECT_PROPOSAL_OP {
                return self
                    .run_resolve_proposal(&params, &origin, false)
                    .await
                    .map(OpOutcome::proven);
            }
        }

        // Engine-level compound: expand a template instantiation into ordinary
        // `create` dispatches (each re-enters this method and gets stamping /
        // history / undo classification like any other op).
        if op_name == INSTANTIATE_TEMPLATE_OP && entity_name.as_str() == "block" {
            return self
                .run_instantiate_template(&params, &origin)
                .await
                .map(OpOutcome::proven);
        }

        // Engine-level compound: block → page (Option B). Composed from ordinary
        // invertible ops (create / move_block / set_field(marks) /
        // rewrite_link_resolution) whose op-level inverses assemble into ONE
        // composite `UndoEntry`. Intercepted here (like `instantiate_template`)
        // so undo/redo replay the CONSTITUENTS, never the compound name.
        if op_name == CONVERT_BLOCK_TO_PAGE_OP && entity_name.as_str() == "block" {
            return self
                .run_convert_block_to_page(&params, &origin)
                .await
                .map(OpOutcome::proven);
        }

        // Engine-level compound: a write of the block's full vault SOURCE.
        // Intercepted here because only the engine can resolve the owning
        // document's vocabulary, which is what turns the source into
        // (content, task_state). See [`SOURCE_TEXT_FIELD`].
        if op_name == "set_field"
            && entity_name.as_str() == "block"
            && params.get("field").and_then(|v| v.as_string()) == Some(SOURCE_TEXT_FIELD)
        {
            return self
                .run_set_source_text(&params, &origin)
                .await
                .map(OpOutcome::proven);
        }

        // Engine-level op: the task-state ring. Intercepted rather than left to
        // the providers because the ring is the owning DOCUMENT's `#+TODO:`
        // vocabulary and only the engine can resolve it — the providers' own
        // hardcoded rings are unreachable for this op by construction.
        if op_name == CYCLE_TASK_STATE_OP && entity_name.as_str() == "block" {
            return self
                .run_cycle_task_state(&params, &origin)
                .await
                .map(OpOutcome::proven);
        }

        // Engine-level compound: duplicate-identity merge. Same shape as the
        // block→page transform — invertible constituents assembled into ONE
        // composite `UndoEntry`.
        if op_name == MERGE_BLOCKS_OP && entity_name.as_str() == "block" {
            return self
                .run_merge_blocks(&params, &origin)
                .await
                .map(OpOutcome::proven);
        }

        // Everything below — capture the prior state, write the new state,
        // journal the inverse — is ONE step per entity (see
        // [`EntityWriteLocks`]); the journal push must stay INSIDE the hold, or
        // the stack's order is only whatever order the tasks happen to resume
        // in. The compound interceptors above have already returned by here:
        // each compound takes the stripe of the block it rewrites for its whole
        // span, and its constituents go straight to the dispatcher, so no hold
        // is ever nested.
        let write_guard = self
            .entity_write_locks
            .lock_target(entity_name.as_str(), &params, "id")
            .await;

        // Provenance stamping (ADR 0024 P8 / C2a): the dispatcher drops `origin`
        // before the write, so this is the last place holding it. For authoring
        // ops we inject a `_provenance` property into the params; it travels as
        // ordinary block-field data down the existing write path and lands in
        // `block_raw.properties`, with no provider edits.
        let params = self.stamp_provenance(op_name, params, &origin)?;

        // Keyword convergence (ruling 2026-08-10): a write that would leave the
        // block as keyword-headed plain text is rewritten to the task it
        // already is, here, before it reaches the store — so `forward_op` (the
        // redo record) and the write itself carry the SAME converged content.
        let (params, converged) = self
            .converge_block_write(entity_name, op_name, params)
            .await?;

        // Same reason, second consumer of the origin this method is the last to
        // hold: whether `content` may carry raw org markup the author just typed
        // (`[[Page]]`, `*bold*`) is a fact about PROVENANCE, so only here can it
        // be stated. A human or an agent is authoring; a rule, a peer merge, and
        // ingest are not. Undo/redo replay never passes through this method at
        // all (`replay` dispatches straight to the dispatcher), so a stored
        // inverse is byte-identity-preserving by construction.
        let input = match origin {
            OpOrigin::User | OpOrigin::Agent { .. } => AuthoredInput::Live,
            OpOrigin::Rule { .. } | OpOrigin::Sync | OpOrigin::Ingest => AuthoredInput::Verbatim,
        };

        let forward_op = Operation::new(
            entity_name.clone(),
            op_name,
            op_name,
            params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        );

        let mut result = self
            .dispatcher
            .execute_operation_with_input(entity_name, op_name, params, input)
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

        // The two halves of convergence: the `task_state` that pairs a
        // pre-rewritten content write, and the repair for writes that compute
        // their content inside the provider (`split_block`, the merge family),
        // which the pre-rewrite cannot see. Both run under this entity's write
        // hold, so no reader observes the block between the two writes.
        let mut converge_forwards = Vec::new();
        let mut converge_inverses = Vec::new();
        if let Some((id, promotion)) = &converged {
            let (fwd, inv, ch) = self.write_converged_task_state(id, promotion).await?;
            result.changes.extend(ch);
            converge_forwards.push(fwd);
            converge_inverses.push(inv);
        }
        let (post_fwd, post_inv, post_ch) = self
            .converge_after_write(entity_name, &result.changes)
            .await?;
        result.changes.extend(post_ch);
        converge_forwards.extend(post_fwd);
        for inv in post_inv.into_iter().rev() {
            converge_inverses.insert(0, inv);
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
        if origin.is_user()
            && !Self::changes_are_vacuous(&result.changes)
            && let UndoAction::Undo(inverse_op) = &result.undo
        {
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
            if op_name == "create"
                && !forward_op.params.contains_key("id")
                && let Some(minted) = inverse_op.params.get("id")
            {
                forward_op.params.insert("id".to_string(), minted.clone());
            }
            // A converged write is ONE undoable gesture: the convergence
            // writes are appended to the forwards and PREPENDED to the
            // inverses, so undo drops the task state before restoring the text
            // it was derived from. Undo lands the pre-write content, which is
            // itself converged, so the block stays representable.
            let mut ops = vec![forward_op];
            ops.extend(converge_forwards);
            let mut inverse_ops = converge_inverses;
            inverse_ops.push(inverse_op.clone());
            let entry = UndoEntry {
                ops,
                inverse_ops,
                origin: OpOrigin::User,
                group_id: 0,
                precondition: Precondition::forward(&result.changes),
                redo_precondition: Precondition::inverse(&result.changes),
            };
            self.journal_step(write_guard.as_ref(), entry).await?;
        }
        // The entity's write-and-journal step is complete; the history relation
        // below is an append-only side record that orders itself.
        drop(write_guard);

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

        Ok(OpOutcome {
            response: result.response,
            delivery: result.delivery,
        })
    }

    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        let mut ops: Vec<OperationDescriptor> = self
            .dispatcher
            .operations()
            .into_iter()
            .filter(|op| op.entity_name == entity_name)
            .collect();
        if entity_name == "block" {
            ops.extend(Self::block_synthetic_descriptors(
                self.template_source.is_some(),
            ));
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
        if entity_name == "block"
            && (op_name == CONVERT_BLOCK_TO_PAGE_OP || op_name == MERGE_BLOCKS_OP)
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

        // One gesture, one hold: the stripes are taken BEFORE the staleness
        // check and released only after the entry is committed, so no external
        // write can land between the check and the replay (task #47).
        let held = self.lock_entry(&entry.inverse_ops).await;

        // Ruling #4: verify BEFORE replaying; a stale entry is dropped loudly,
        // never silently skipped to the next entry.
        if let Some(reason) = self.check_stale(&entry.precondition).await? {
            self.undo_stack.write().await.drop_undo();
            self.persist().await?;
            tracing::error!("undo: dropped stale entry ({reason})");
            return Ok(UndoOutcome::StaleDropped { reason });
        }

        let mut changes = Vec::new();
        // Partial-failure discipline (Inc1): a composite entry replays N
        // inverses in order. On the FIRST failure, stop and fail loud naming the
        // failing inverse index — never silently swallow a half-applied undo.
        // (The already-replayed inverses are NOT rolled back; the entry stays on
        // the undo stack, un-committed, so the loud error is the single source of
        // truth about the partial state.)
        let inverse_count = entry.inverse_ops.len();
        for (idx, op) in entry.inverse_ops.iter().enumerate() {
            let replayed = self.replay(op, &held).await.map_err(|e| {
                anyhow::anyhow!(
                    "undo: composite inverse op {idx} of {inverse_count} ('{}' on '{}') failed — \
                     stopping (partial undo, earlier inverses already applied): {e}",
                    op.op_name,
                    op.entity_name
                )
            })?;
            changes.extend(replayed);
        }
        // Fail-loud (CLAUDE.md): the entry is consumed either way — a stale-top
        // poison entry must not be re-attempted — but if the inverse replay
        // proved no observable change, report `NoChange` so the caller never
        // claims "undone" for a no-op press (BugFunnel 2026-07-13 undo row).
        self.undo_stack.write().await.commit_undo();
        self.persist().await?;
        drop(held);
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

        // Symmetric one-gesture-one-hold (task #47); a redo replays the FORWARD
        // ops, so those name the stripes.
        let held = self.lock_entry(&entry.ops).await;

        if let Some(reason) = self.check_stale(&entry.redo_precondition).await? {
            self.undo_stack.write().await.drop_redo();
            self.persist().await?;
            tracing::error!("redo: dropped stale entry ({reason})");
            return Ok(UndoOutcome::StaleDropped { reason });
        }

        let mut changes = Vec::new();
        // Symmetric partial-failure discipline (Inc1): a composite redo replays
        // N forwards in order; the first failure stops and names its index.
        let forward_count = entry.ops.len();
        for (idx, op) in entry.ops.iter().enumerate() {
            let replayed = self.replay(op, &held).await.map_err(|e| {
                anyhow::anyhow!(
                    "redo: composite forward op {idx} of {forward_count} ('{}' on '{}') failed — \
                     stopping (partial redo, earlier ops already applied): {e}",
                    op.op_name,
                    op.entity_name
                )
            })?;
            changes.extend(replayed);
        }
        self.undo_stack.write().await.commit_redo();
        self.persist().await?;
        drop(held);
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
        let Some(Value::String(root_id)) = root_id.response else {
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

    /// Instance blocks minted by a template instantiation: everything in
    /// `block_raw` that is neither the seed target nor a template-definition
    /// block (`seed_template` seeds `block:target`, `block:tpl`,
    /// `block:tpl-c1`).
    async fn remaining_instance_blocks(engine: &BackendEngine) -> Vec<String> {
        engine
            .db_handle()
            .query(
                "SELECT id FROM block_raw WHERE id LIKE 'block:%' AND id NOT IN ('block:target', \
                 'block:tpl', 'block:tpl-c1') ORDER BY id",
                HashMap::new(),
            )
            .await
            .unwrap()
            .iter()
            .map(|row| str_field(row, "id").to_string())
            .collect()
    }

    /// composite-undo Inc0 (red-for-the-right-reason): `instantiate_template`
    /// is ONE user gesture, so ONE `undo()` must remove ALL instance
    /// blocks. Today the engine re-enters `execute_operation` per create
    /// and pushes N separate `UndoEntry`s (`run_instantiate_template`), so
    /// a single undo pops only the LAST create — the instance CHILD — and
    /// leaves the instance ROOT behind. Inc3's composite grouping makes
    /// instantiate push ONE `UndoEntry` (all creates grouped, inverse =
    /// leaf-first deletes) so one undo removes every instance block. RED
    /// until Inc3: the assertion below fails because the instance root
    /// survives the single undo.
    #[tokio::test(flavor = "multi_thread")]
    async fn instantiate_template_is_one_undo_gesture() {
        let engine = block_engine().await;
        seed_template(&engine).await;
        let entity = EntityName::new("block");

        // User origin so the constituent creates journal undo entries
        // (Rule/Sync/Ingest never push to the user history — ruling #1).
        engine
            .execute_operation(
                &entity,
                "instantiate_template",
                instantiate_params("undo-key", &[("date", "2026-07-12")]),
                OpOrigin::User,
            )
            .await
            .unwrap();

        // A two-node template ⇒ two instance blocks (root + child).
        let before = remaining_instance_blocks(&engine).await;
        assert_eq!(
            before.len(),
            2,
            "instantiation minted root + child; got {before:?}"
        );

        // ONE undo. It IS an observable change (a delete), so `Applied`.
        let outcome = engine.undo().await.unwrap();
        assert!(
            matches!(outcome, UndoOutcome::Applied),
            "the single undo of an instantiation must apply, got {outcome:?}"
        );

        // The whole gesture is gone after that one undo (Inc3 composite group).
        // Before Inc3 this was RED: instantiate pushed N per-create entries, so
        // one undo removed only the last-pushed CHILD and the instance ROOT
        // survived. The Inc3 begin/end_undo_group collapse makes one undo remove
        // ALL instance blocks (inverse = leaf-first deletes).
        let after = remaining_instance_blocks(&engine).await;
        assert!(
            after.is_empty(),
            "one undo must remove ALL instance blocks (instantiation is one gesture); \
             {} still present after a single undo: {after:?}",
            after.len()
        );
    }

    /// Inc3 (ruling #1): a Rule-origin instantiation mutates state (blocks are
    /// created) but journals NO undo entry — only User-origin ops enter the
    /// user history. The composite group opens and closes around the
    /// fan-out, but every per-create push is User-gated, so the group
    /// materializes NOTHING.
    #[tokio::test(flavor = "multi_thread")]
    async fn rule_origin_instantiation_journals_no_undo_entry() {
        let engine = block_engine().await;
        let block = EntityName::new("block");
        let rule = || OpOrigin::Rule {
            transition_id: "rule:test-template".into(),
        };
        let params = |fields: &[(&str, Value)]| -> StorageEntity {
            fields
                .iter()
                .map(|(k, v)| (Arc::from(*k), v.clone()))
                .collect()
        };
        // Seed the target + template under RULE origin so the SEED itself
        // journals nothing (ruling #1) — this isolates the instantiate's own
        // journaling. `can_undo` is then a clean signal for the op under test.
        engine
            .execute_operation(
                &block,
                "create",
                params(&[
                    ("id", Value::String("block:target".into())),
                    ("content", Value::String("Target".into())),
                ]),
                rule(),
            )
            .await
            .unwrap();
        engine
            .execute_operation(
                &block,
                "create",
                params(&[
                    ("id", Value::String("block:tpl".into())),
                    ("content", Value::String("{{date}}".into())),
                    ("template", Value::String("daily".into())),
                    ("template_vars", Value::String("date, mood=neutral".into())),
                ]),
                rule(),
            )
            .await
            .unwrap();
        engine
            .execute_operation(
                &block,
                "create",
                params(&[
                    ("id", Value::String("block:tpl-c1".into())),
                    ("parent_id", Value::String("block:tpl".into())),
                    ("content", Value::String("see {{date}} now".into())),
                ]),
                rule(),
            )
            .await
            .unwrap();
        assert!(
            !engine.can_undo().await,
            "sanity: the rule-origin seed journals nothing"
        );

        engine
            .execute_operation(
                &block,
                "instantiate_template",
                instantiate_params("rule-key", &[("date", "2026-07-12")]),
                rule(),
            )
            .await
            .unwrap();

        // The instance blocks (root + child) were really created …
        assert_eq!(
            remaining_instance_blocks(&engine).await.len(),
            2,
            "rule-origin instantiation still mutates state"
        );
        // … but nothing is undoable: the group materialized no composite entry.
        assert!(
            !engine.can_undo().await,
            "a Rule-origin instantiation must journal NO undo entry (ruling #1)"
        );
    }

    /// How many of `ids` currently exist in `block_raw`.
    async fn ids_present(engine: &BackendEngine, ids: &[&str]) -> usize {
        let quoted = ids
            .iter()
            .map(|i| format!("'{}'", i.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        engine
            .db_handle()
            .query(
                &format!("SELECT id FROM block_raw WHERE id IN ({quoted})"),
                HashMap::new(),
            )
            .await
            .unwrap()
            .len()
    }

    /// A block-op with a single `id` param.
    fn id_op(entity: &str, op: &str, id: &str) -> Operation {
        let mut p = HashMap::new();
        p.insert("id".to_string(), Value::String(id.to_string()));
        Operation::new(entity, op, op, p)
    }

    /// Inc1 engine seam: `begin_undo_group` … `end_undo_group` collapses N
    /// User-origin dispatches into ONE composite undo entry, so a single undo
    /// reverses the whole group. This is the seam Inc3's instantiate migration
    /// rides on.
    #[tokio::test(flavor = "multi_thread")]
    async fn begin_end_undo_group_is_one_undo_gesture() {
        let engine = block_engine().await;
        let entity = EntityName::new("block");
        let create = |id: &str, content: &str| {
            let mut p = StorageEntity::new();
            p.insert("id".into(), Value::String(id.to_string()));
            p.insert("content".into(), Value::String(content.to_string()));
            p
        };

        engine.begin_undo_group().await;
        engine
            .execute_operation(&entity, "create", create("block:g-a", "a"), OpOrigin::User)
            .await
            .unwrap();
        engine
            .execute_operation(&entity, "create", create("block:g-b", "b"), OpOrigin::User)
            .await
            .unwrap();
        engine.end_undo_group().await.unwrap();

        assert_eq!(
            ids_present(&engine, &["block:g-a", "block:g-b"]).await,
            2,
            "both grouped creates landed"
        );
        assert!(engine.can_undo().await);

        // ONE undo reverses the WHOLE group.
        let outcome = engine.undo().await.unwrap();
        assert!(matches!(outcome, UndoOutcome::Applied));
        assert_eq!(
            ids_present(&engine, &["block:g-a", "block:g-b"]).await,
            0,
            "one undo removed BOTH grouped blocks"
        );
        assert!(
            !engine.can_undo().await,
            "the group was ONE entry — nothing left to undo"
        );
    }

    /// Amendment 2: a composite undo that fails PART-WAY through its inverses
    /// stops loud and names the failing inverse index (never a silent
    /// half-undo). Forced with a hand-crafted entry because the SQL
    /// provider's cascade-delete and idempotent-create cannot naturally
    /// fail on a well-formed tree.
    #[tokio::test(flavor = "multi_thread")]
    async fn partial_failure_mid_composite_undo_names_the_failing_inverse_index() {
        let engine = block_engine().await;
        // A real block so the FIRST inverse succeeds (is deleted).
        create_block(
            &engine,
            &[
                ("id", Value::String("block:keep".into())),
                ("content", Value::String("k".into())),
            ],
        )
        .await;

        // inverse[0] deletes a real block (ok); inverse[1] is a `block.delete`
        // with NO `id` param ⇒ the provider rejects it loud ("Missing 'id'
        // parameter"). undo() must stop at index 1.
        let entry = UndoEntry {
            ops: vec![id_op("block", "create", "block:keep")],
            inverse_ops: vec![
                id_op("block", "delete", "block:keep"),
                Operation::new("block", "delete", "Delete", HashMap::new()),
            ],
            origin: OpOrigin::User,
            group_id: 0,
            precondition: Precondition::default(),
            redo_precondition: Precondition::default(),
        };
        engine.push_undo_entry_for_test(entry).await;

        let err = engine
            .undo()
            .await
            .expect_err("a mid-inverse failure must surface, not be swallowed");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("composite inverse op 1"),
            "error must name the failing inverse index (1): {msg}"
        );
        assert!(
            msg.contains("Missing 'id'"),
            "error must carry the underlying cause: {msg}"
        );
        // Disclosed partial state: inverse[0] applied before the failure.
        assert_eq!(
            ids_present(&engine, &["block:keep"]).await,
            0,
            "the earlier inverse applied before the failure (partial undo, disclosed)"
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
        let stamped =
            stamp_params("create", params, &origin, 1234).expect("a plain create is stamped");

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
        let stamped = stamp_params("update", StorageEntity::default(), &origin, 7)
            .expect("a plain update is stamped");
        let parsed =
            ProvenanceStamp::from_value(stamped.get(PROVENANCE_PROPERTY).unwrap()).unwrap();
        assert_eq!(parsed.origin, "agent");
        assert_eq!(parsed.session_id.as_deref(), Some("mcp-session:s"));
        assert_eq!(parsed.tool_call_id.as_deref(), Some("tool-call:c"));
    }

    /// Ruling D5.a: the engine mints `_provenance`, so an authored one is a
    /// NAMED refusal at the write boundary — never a silent replacement, which
    /// would tell the author their attribution landed when it did not.
    #[test]
    fn an_authored_provenance_is_refused_by_name() {
        for op in PROVENANCE_STAMPED_OPS {
            let params = params_with(&[
                ("content", Value::String("hi".into())),
                (
                    PROVENANCE_PROPERTY,
                    Value::String("authored-by-hand".into()),
                ),
            ]);
            let err = stamp_params(op, params, &OpOrigin::User, 1)
                .expect_err("an authored _provenance must be REFUSED, not replaced");
            let msg = format!("{err:#}");
            assert!(
                msg.contains(PROVENANCE_PROPERTY) && msg.contains(op),
                "the refusal must name the offending key and the operation, got: {msg}"
            );
        }
    }

    /// The SECOND route: `set_field` names the key in a param VALUE, so a
    /// refusal reading only param KEYS lets a FORGED stamp through — and a
    /// forged stamp is worse than a replaced one, because
    /// `history_store.rs` and the trust supervision view read it as
    /// authoritative.
    #[test]
    fn an_authored_provenance_via_set_field_is_refused_by_name() {
        let params = params_with(&[
            ("id", Value::String("block:x".into())),
            ("field", Value::String(PROVENANCE_PROPERTY.into())),
            ("value", Value::String("forged".into())),
        ]);
        let err = reject_engine_owned_keys("set_field", &params)
            .expect_err("set_field naming an engine-owned key must be REFUSED");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(PROVENANCE_PROPERTY) && msg.contains("set_field"),
            "the refusal must name the offending key and the operation, got: {msg}"
        );
    }

    /// THIRD route, sub-shape (a): the key sits one level deeper, inside the
    /// property BAG named by `set_field("properties")`. `properties` IS a
    /// `block_raw` column, so this takes the direct-column branch and replaces
    /// the WHOLE blob — forged stamp included.
    #[test]
    fn a_provenance_inside_a_set_field_properties_bag_is_refused_by_name() {
        let params = params_with(&[
            ("id", Value::String("block:x".into())),
            ("field", Value::String("properties".into())),
            (
                "value",
                Value::String(r#"{"_provenance":{"origin":"forged"},"keep":"me"}"#.into()),
            ),
        ]);
        let err = reject_engine_owned_keys("set_field", &params)
            .expect_err("a reserved key inside the properties bag must be REFUSED");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(PROVENANCE_PROPERTY) && msg.contains("set_field"),
            "the refusal must name the offending key and the operation, got: {msg}"
        );
    }

    /// THIRD route, sub-shape (b): a nested `properties` bag on `create`. The
    /// engine stamp wins the `or_insert_with` merge, so the authored key is
    /// SILENTLY DISCARDED — told-success-while-discarding, the exact shape
    /// D5.a refuses.
    #[test]
    fn a_provenance_inside_a_create_properties_bag_is_refused_by_name() {
        let mut bag = std::collections::HashMap::new();
        bag.insert(
            PROVENANCE_PROPERTY.to_string(),
            Value::String("forged".into()),
        );
        bag.insert("keep".to_string(), Value::String("me".into()));
        let params = params_with(&[
            ("content", Value::String("hi".into())),
            ("properties", Value::Object(bag)),
        ]);
        let err = reject_engine_owned_keys("create", &params)
            .expect_err("a reserved key inside a nested properties bag must be REFUSED");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(PROVENANCE_PROPERTY) && msg.contains("create"),
            "the refusal must name the offending key and the operation, got: {msg}"
        );
    }

    /// An ordinary bag still passes — the refusal reads the bag's keys, it does
    /// not reject bags.
    #[test]
    fn an_ordinary_properties_bag_still_passes_the_boundary() {
        let params = params_with(&[
            ("content", Value::String("hi".into())),
            (
                "properties",
                Value::String(r#"{"COLLAPSED":"true","_drawer_order":"ID"}"#.into()),
            ),
        ]);
        reject_engine_owned_keys("create", &params)
            .expect("a bag naming no engine-owned key is an ordinary write");
    }

    /// A bag we cannot READ is refused, not waved through: "I could not tell"
    /// is not evidence of absence.
    #[test]
    fn an_unreadable_properties_bag_is_refused_rather_than_trusted() {
        let params = params_with(&[
            ("content", Value::String("hi".into())),
            ("properties", Value::String("not json at all {{{".into())),
        ]);
        let err = reject_engine_owned_keys("create", &params)
            .expect_err("an unreadable bag must be REFUSED");
        assert!(
            format!("{err:#}").contains("REFUSED"),
            "the refusal must say so plainly: {err:#}"
        );
    }

    /// The refusal is keyed on the EXACT spelling, not the `_` prefix: the org
    /// ingest path puts `_drawer_order` into create params
    /// (`holon-orgmode/src/block_params.rs:167`), so a prefix ban would refuse
    /// the vault's own write leg. Both routes, since both now read keys.
    #[test]
    fn other_underscored_keys_still_pass_the_boundary_on_both_routes() {
        let via_set_field = params_with(&[
            ("id", Value::String("block:x".into())),
            ("field", Value::String("_drawer_order".into())),
            ("value", Value::String("ID,COLLAPSED".into())),
        ]);
        reject_engine_owned_keys("set_field", &via_set_field)
            .expect("only engine-minted keys are reserved, whichever route names them");
    }

    #[test]
    fn other_underscored_keys_still_pass_the_boundary() {
        let params = params_with(&[
            ("_drawer_order", Value::String("ID,COLLAPSED".into())),
            ("_proposed_by", Value::String("agent".into())),
        ]);
        let stamped = stamp_params("create", params, &OpOrigin::User, 1)
            .expect("only engine-minted keys are reserved");
        assert!(stamped.contains_key("_drawer_order"));
        assert!(stamped.contains_key("_proposed_by"));
    }

    #[test]
    fn non_authoring_ops_are_not_stamped() {
        for op in ["set_field", "move_block", "split_block", "delete", "focus"] {
            let stamped = stamp_params(op, StorageEntity::default(), &OpOrigin::User, 1)
                .expect("a non-authoring op is passed through");
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
