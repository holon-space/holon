//! Core datasource traits
//!
//! This module provides traits for datasource operations.
//! These traits are designed to work with external datasources that provide
//! both read and write capabilities.

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use holon_api::Change;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::StreamPosition;
use holon_api::Tags;
use holon_api::Value;
use serde::Deserialize;
use serde::Serialize;

use crate::cell_registry::EntityCellRegistryExt;

// Define Result type using Send + Sync for error
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Write provenance — which system originated a write.
///
/// Threaded through the `OperationProvider::*_with_origin` write API and
/// persisted on the `_change_origin` CDC column so the inbound direction can
/// echo-suppress its own writes (e.g. the Loro→SQL projection tags its writes
/// `EventOrigin::Loro`).
///
/// @c4 code
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventOrigin {
    Loro,
    Org,
    Ui,
    Other(String),
}

impl EventOrigin {
    pub fn as_str(&self) -> &str {
        match self {
            EventOrigin::Loro => "loro",
            EventOrigin::Org => "org",
            EventOrigin::Ui => "ui",
            EventOrigin::Other(s) => s.as_str(),
        }
    }

    pub fn parse_str(s: &str) -> Self {
        match s {
            "loro" => EventOrigin::Loro,
            "org" => EventOrigin::Org,
            "ui" => EventOrigin::Ui,
            other => EventOrigin::Other(other.to_string()),
        }
    }
}

/// Common operation provider interface.
///
/// Providers that support entity operations implement this trait.
/// The `*_with_origin` variants allow callers to tag writes with their
/// provenance so the inverse sync direction can skip echoes.
///
/// @c4 code
/// @c4 uses OperationResult "operation outcome" "returns"
#[async_trait]
pub trait OperationProvider: Send + Sync {
    /// Get all operations this provider supports
    fn operations(&self) -> Vec<OperationDescriptor>;

    /// Find operations that can be executed with given arguments
    fn find_operations(
        &self,
        entity_name: &EntityName,
        available_args: &[String],
    ) -> Vec<OperationDescriptor> {
        self.operations()
            .into_iter()
            .filter(|op| {
                if op.entity_name != *entity_name {
                    return false;
                }
                op.required_params.iter().all(|p| {
                    if available_args.contains(&p.name) {
                        return true;
                    }
                    op.param_mappings
                        .iter()
                        .any(|mapping| mapping.provides.contains(&p.name))
                })
            })
            .collect()
    }

    /// Execute an operation
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: holon_api::StorageEntity,
    ) -> Result<OperationResult>;

    /// Get the last created entity ID (if any). Default returns `None`.
    fn get_last_created_id(&self) -> Option<String> {
        None
    }

    /// Read the currently-stored `(content, marks)` of a block row, for the
    /// CRUD authority that owns block state.
    ///
    /// The dispatcher's live-edit mark-extraction follow-up (links increment 3)
    /// uses this to decide whether a `set_field("content")` actually CHANGED
    /// the block's mark set — comparing the newly extracted marks against
    /// ground truth instead of nulling marks blindly on every content
    /// commit.
    ///
    /// - `Ok(Some((content, marks)))` — the stored stripped-label `content` and
    ///   the `marks` column value (`Value::Null` when the block has no marks).
    /// - `Ok(None)` — this provider does NOT own readable block state (the
    ///   structural-ops providers, test stubs, and the Loro CRUD provider
    ///   today). Callers MUST treat `None` as UNKNOWN and fail safe: never null
    ///   a block's marks on the strength of an unreadable prior state.
    async fn read_block_content_marks(&self, _: &str) -> Result<Option<(String, Value)>> {
        Ok(None)
    }
}

/// The single backend that owns block CRUD (set_field / create / update /
/// delete) for a given session — the *authority* whose store is the source of
/// truth, distinct from the structural-ops providers that also live in the
/// `dyn OperationProvider` set.
///
/// The composition root registers exactly one of these per mode (the Loro
/// provider when a CRDT backend is enabled, the SQL provider in SqlOnly mode)
/// so consumers like the org file-sync wiring pick the CRUD authority by
/// resolving this marker — never by naming a concrete backend type. Absent in
/// SqlOnly when the consumer already holds the SQL provider locally.
pub struct CrudAuthority(pub std::sync::Arc<dyn OperationProvider>);

/// Origin-tagged write capability — execute operations while preserving the
/// originating [`EventOrigin`] for CDC echo-suppression.
///
/// Split out of [`OperationProvider`] because preserving write provenance is a
/// genuine precondition of the projection write paths (Loro→SQL, Org→SQL), not
/// a property every provider has. While these lived on `OperationProvider` with
/// a default that delegated to the non-origin variant, any caller handed a
/// provider that didn't override them would *silently drop the origin*.
/// Requiring this trait at those call sites turns "I preserve write origin"
/// into a checked obligation: a provider that can't tag writes won't typecheck
/// where one is needed. The two methods are deliberately required (no default)
/// so opting in is explicit.
#[async_trait]
pub trait OriginTaggedWrites: OperationProvider {
    /// Execute a single operation and tag the resulting events with `origin`.
    async fn execute_operation_with_origin(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: holon_api::StorageEntity,
        origin: EventOrigin,
    ) -> Result<OperationResult>;

    /// Execute a batch of operations and tag the resulting events with
    /// `origin`.
    async fn execute_batch_with_origin(
        &self,
        entity_name: &EntityName,
        operations: Vec<(String, holon_api::StorageEntity)>,
        origin: EventOrigin,
    ) -> Result<Vec<OperationResult>>;
}

/// Build the response payload for a structural focus-mover (`split_block` /
/// `join_block`): the block focus should move to, plus the initial caret
/// offset. The frontend reads this off the op result and moves the in-memory
/// focus authority in process (ADR 0010) — keys must match
/// `holon-frontend::reactive::structural_focus_target` (`block_id`,
/// `cursor_offset`).
fn focus_response(block_id: &str, cursor_offset: i64) -> Value {
    Value::Object(HashMap::from([
        ("block_id".to_string(), Value::String(block_id.to_string())),
        ("cursor_offset".to_string(), Value::Integer(cursor_offset)),
    ]))
}

/// Information about a completion state including progress percentage
///
/// This struct provides metadata about task completion states to enable
/// progress visualization in the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletionStateInfo {
    /// The state name (e.g., "TODO", "DOING", "DONE")
    pub state: String,
    /// Progress percentage from 0.0 to 100.0
    pub progress: f64,
    /// Whether this is a "done" state (completed)
    pub is_done: bool,
    /// Whether this is an "active" state (in progress)
    pub is_active: bool,
}

/// Represents the undo classification of an operation.
///
/// Every provider result MUST carry a deliberate classification — either a
/// concrete inverse ([`UndoAction::Undo`]) or an explicit
/// [`UndoAction::DeclaredIrreversible`] naming *why* it cannot be undone. The
/// third variant, [`UndoAction::Undeclared`], is the loud-failure default: a
/// result that reaches the engine still `Undeclared` is a programming error
/// (an arm that forgot to classify), not a silent no-op.
#[derive(Debug, Clone)]
pub enum UndoAction {
    /// The operation can be undone by executing the contained inverse
    /// operation.
    Undo(Operation),
    /// The operation is deliberately not undoable; the reason is greppable and
    /// user-surfaceable (e.g. "split_block: inverse not yet implemented").
    DeclaredIrreversible(&'static str),
    /// No classification was made. Reaching the engine in this state is a loud
    /// error — providers must choose `Undo` or `DeclaredIrreversible`.
    Undeclared,
}

impl UndoAction {
    /// Convert to Option<Operation>.
    // ALLOW(compatibility): legitimate Option<Operation> bridge consumed by
    // the macro-generated dispatcher; not a removable shim.
    pub fn into_option(self) -> Option<Operation> {
        match self {
            UndoAction::Undo(op) => Some(op),
            UndoAction::DeclaredIrreversible(_) | UndoAction::Undeclared => None,
        }
    }

    /// Check if this action is reversible
    pub fn is_reversible(&self) -> bool {
        matches!(self, UndoAction::Undo(_))
    }

    /// Whether the provider forgot to classify (loud-error condition).
    pub fn is_undeclared(&self) -> bool {
        matches!(self, UndoAction::Undeclared)
    }
}

impl From<Operation> for UndoAction {
    fn from(op: Operation) -> Self {
        UndoAction::Undo(op)
    }
}

impl From<Option<Operation>> for UndoAction {
    fn from(opt: Option<Operation>) -> Self {
        match opt {
            Some(op) => UndoAction::Undo(op),
            None => UndoAction::DeclaredIrreversible("inverse not provided"),
        }
    }
}

/// Whether a [`FieldDelta`] names a scalar (entity, field) the undo staleness
/// reader can `SELECT` back from the projection table (`Readable`), or an
/// edge/junction write that has NO readable column so it must be excluded from
/// [`Precondition`](crate::undo::Precondition) fingerprints while still flowing
/// into the history relation (`HistoryOnly`).
///
/// Parse-don't-validate: the delta itself carries the proof of which
/// fingerprinting is legal, so `Precondition::forward`/`inverse` never emit a
/// `SELECT <edge-field> FROM block_raw` the row table cannot answer (the edge
/// lives in a junction table). `#[serde(default)]` on the field keeps this
/// backward-compatible — a persisted delta with no fingerprint reads back as
/// `Readable`, the historical behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeltaFingerprint {
    /// The (entity, field) is a real projected scalar column: fingerprint it.
    #[default]
    Readable,
    /// The change is an edge/junction write with no readable column: record it
    /// in history, but never put it in a staleness precondition.
    HistoryOnly,
}

/// Represents a single field change with old and new values.
/// Used for change propagation (cache/sync), NOT for undo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDelta {
    pub entity_id: String,
    pub field: String,
    pub old_value: Value,
    pub new_value: Value,
    /// Whether this delta may be fingerprinted for undo staleness. Defaults to
    /// [`DeltaFingerprint::Readable`] (both for `FieldDelta::new` and for
    /// deserialization of pre-fingerprint persisted entries).
    #[serde(default)]
    pub fingerprint: DeltaFingerprint,
}

impl FieldDelta {
    pub fn new(
        entity_id: impl Into<String>,
        field: impl Into<String>,
        old_value: Value,
        new_value: Value,
    ) -> Self {
        Self {
            entity_id: entity_id.into(),
            field: field.into(),
            old_value,
            new_value,
            fingerprint: DeltaFingerprint::Readable,
        }
    }

    /// A delta over an edge/junction field with no readable projection column
    /// (e.g. `tags`): recorded in history, but excluded from undo staleness
    /// preconditions so no invalid `SELECT <field> FROM block_raw` is
    /// generated.
    pub fn history_only(
        entity_id: impl Into<String>,
        field: impl Into<String>,
        old_value: Value,
        new_value: Value,
    ) -> Self {
        Self {
            entity_id: entity_id.into(),
            field: field.into(),
            old_value,
            new_value,
            fingerprint: DeltaFingerprint::HistoryOnly,
        }
    }
}

/// Result of an operation, containing changes for propagation and undo action.
///
/// - `changes`: Field-level changes for propagation to cache/sync systems
/// - `undo`: Semantic undo operation (same code path as forward)
/// - `follow_ups`: Operations to execute after this one completes (e.g., cursor
///   update after split)
///
/// @c4 code
#[derive(Debug, Clone)]
pub struct OperationResult {
    pub changes: Vec<FieldDelta>,
    pub undo: UndoAction,
    /// Optional response payload from the operation (e.g. MCP tool call
    /// results). Non-MCP providers return `None`.
    pub response: Option<Value>,
    /// Operations to execute after this one completes successfully.
    /// Used for side-effects like updating editor cursor after split_block.
    /// The dispatcher executes these in order after the main operation.
    #[doc(hidden)]
    pub follow_ups: Vec<Operation>,
}

impl OperationResult {
    /// Create a reversible operation result
    pub fn new(changes: Vec<FieldDelta>, undo_operation: Operation) -> Self {
        Self {
            changes,
            undo: UndoAction::Undo(undo_operation),
            response: None,
            follow_ups: vec![],
        }
    }

    /// Create a deliberately-irreversible operation result with a default
    /// reason. Behaviour-preserving replacement for the former silent
    /// "no undo entry" path; the classification is now visible and greppable
    /// (`UndoAction::DeclaredIrreversible`). Use
    /// [`Self::declared_irreversible`] to name the specific reason.
    pub fn irreversible(changes: Vec<FieldDelta>) -> Self {
        Self::declared_irreversible(changes, "inverse not yet implemented")
    }

    /// Create an irreversible result naming *why* it cannot be undone.
    pub fn declared_irreversible(changes: Vec<FieldDelta>, reason: &'static str) -> Self {
        Self {
            changes,
            undo: UndoAction::DeclaredIrreversible(reason),
            response: None,
            follow_ups: vec![],
        }
    }

    // ALLOW(compatibility): bridge from a legacy UndoAction-shaped path
    // still consumed by macro-generated code; deletion needs upstream
    // macro work, not a one-line refactor.
    pub fn from_undo(undo: UndoAction) -> Self {
        Self {
            changes: Vec::new(),
            undo,
            response: None,
            follow_ups: vec![],
        }
    }

    /// Attach a response payload to this result
    pub fn with_response(mut self, response: Value) -> Self {
        self.response = Some(response);
        self
    }

    /// Add follow-up operations to execute after the main operation.
    pub fn with_follow_ups(mut self, follow_ups: Vec<Operation>) -> Self {
        self.follow_ups = follow_ups;
        self
    }
}

impl From<UndoAction> for OperationResult {
    fn from(undo: UndoAction) -> Self {
        OperationResult::from_undo(undo)
    }
}

pub type CreateResult = (String, OperationResult);

/// Error raised when a trait's dispatch helper does not recognize an operation
/// name.
#[derive(Debug)]
pub struct UnknownOperationError {
    trait_name: String,
    operation: String,
}

impl UnknownOperationError {
    pub fn new(trait_name: &str, operation: &str) -> Self {
        Self {
            trait_name: trait_name.to_string(),
            operation: operation.to_string(),
        }
    }

    /// Helper for callers that need to keep matching logic in one place.
    pub fn is_unknown(err: &(dyn std::error::Error + 'static)) -> bool {
        err.downcast_ref::<UnknownOperationError>().is_some()
    }
}

impl fmt::Display for UnknownOperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Unknown operation: {} for trait {}",
            self.operation, self.trait_name
        )
    }
}

impl std::error::Error for UnknownOperationError {}

// MaybeSendSync: Send + Sync on all targets. Historically this was relaxed
// to {} on wasm, but the wasm32 browser demo uses Arc/Mutex-backed types and
// keeping Send+Sync unifies async_trait across targets.
pub trait MaybeSendSync: Send + Sync {}
impl<T: Send + Sync + ?Sized> MaybeSendSync for T {}

/// Entities that support hierarchical tree structure
pub trait BlockEntity: MaybeSendSync {
    /// Get the entity's unique identifier
    fn id(&self) -> &EntityUri;

    fn parent_id(&self) -> Option<&EntityUri>;
    fn depth(&self) -> i64;

    /// Get the block content (text content of the block)
    fn content(&self) -> &str;

    /// Tags attached to this block. The literal `"Page"` tag marks the
    /// block as a page (org file root).
    fn tags(&self) -> Tags;

    /// Whether this block is a page (its `tags` contains `"Page"`).
    fn is_page(&self) -> bool {
        self.tags().contains(holon_api::PAGE_TAG)
    }
}

/// Entities that support task management (completion, priority, etc.)
pub trait TaskEntity: MaybeSendSync {
    fn completed(&self) -> bool;
    fn priority(&self) -> Option<i64>;
    fn due_date(&self) -> Option<DateTime<Utc>>;
}

/// CRUD operations provider (fire-and-forget to external system)
///
/// Provides create, update, and delete operations. Changes are confirmed
/// via ChangeNotifications streams, not return values.
// ALLOW(compatibility): the trait name is fixed by the macro-generated
// dispatcher in holon-macros; renaming requires a coordinated macro +
// trait change, not a one-line edit.
#[holon_macros::operations_trait]
#[async_trait]
pub trait CrudOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Set single field (returns changes and inverse operation for undo)
    /// Note: affected_fields is determined dynamically based on the field
    /// parameter
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult>;

    /// Create new entity (returns new ID, changes, and inverse operation for
    /// undo)
    async fn create(
        &self,
        fields: crate::storage::types::StorageEntity,
    ) -> Result<(String, OperationResult)>;

    /// Delete entity (returns changes and inverse operation for undo)
    async fn delete(&self, id: &str) -> Result<OperationResult>;

    /// Get operations metadata (automatically delegates to entity type)
    fn operations(&self) -> Vec<OperationDescriptor>
    where
        T: OperationRegistry,
    {
        T::all_operations()
    }
}

/// Trait for aggregating operation metadata from multiple trait sources
///
/// Entity types implement this trait to declare which operations they support.
/// The implementation aggregates operations from all applicable traits:
/// - `CrudOperations` operations (set_field, create, delete)
/// - `BlockOperations` operations (if entity implements `BlockEntity`)
/// - `TaskOperations` operations (if entity implements `TaskEntity`)
pub trait OperationRegistry: MaybeSendSync {
    /// Returns all operations supported by this entity type
    fn all_operations() -> Vec<OperationDescriptor>;

    /// Returns the entity name for this registry (e.g., "todoist_task",
    /// "block")
    fn entity_name() -> &'static str;

    /// Returns the short name for this entity type (e.g., "task", "project")
    /// Used for generating entity-typed parameters like "task_id", "project_id"
    /// Returns None if not specified in the entity attribute
    fn short_name() -> Option<&'static str> {
        None
    }
}

/// Read-only data access (from cache)
#[async_trait]
pub trait DataSource<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    async fn get_all(&self) -> Result<Vec<T>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<T>>;

    // Helper queries (default implementations)
    async fn get_children(&self, parent_id: &EntityUri) -> Result<Vec<T>>
    where
        T: BlockEntity,
    {
        let all_items: Vec<T> = self.get_all().await?;
        Ok(all_items
            .into_iter()
            .filter(|t: &T| t.parent_id() == Some(parent_id))
            .collect())
    }

    /// Get all descendants of a parent (recursive). Default uses iterative BFS
    /// over `get_children()`. Implementations may override with a recursive
    /// CTE.
    async fn get_descendants(&self, parent_id: &EntityUri) -> Result<Vec<T>>
    where
        T: BlockEntity,
    {
        let mut result = Vec::new();
        let mut queue = vec![parent_id.clone()];
        while let Some(pid) = queue.pop() {
            let children = self.get_children(&pid).await?;
            for child in children {
                queue.push(child.id().clone());
                result.push(child);
            }
        }
        Ok(result)
    }
}

/// Read-only query helpers for navigating block hierarchies
#[async_trait]
pub trait BlockQueryHelpers<T>: DataSource<T>
where
    T: BlockEntity + MaybeSendSync + 'static,
{
    /// Return the children of `parent_id` in authoritative sibling order.
    ///
    /// This is the single ordering primitive of the block domain: sibling
    /// order is a property of the parent→children relation, not of any
    /// per-block encoding (see ADR 0005). Each backend implements it from
    /// its internal ordering (Loro fractional index, SQL `ORDER BY
    /// sort_key`, in-memory child list); the encoding never leaks onto the
    /// domain entity. All sibling navigation below is defined in terms of
    /// list position within this result.
    async fn children_ordered(&self, parent_id: &EntityUri) -> Result<Vec<T>>;

    /// Get all siblings of a block (excluding itself), in sibling order.
    async fn get_siblings(&self, block_id: &EntityUri) -> Result<Vec<T>> {
        let block: T = self
            .get_by_id(block_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        let siblings: Vec<T> = if let Some(pid) = block.parent_id() {
            self.children_ordered(pid).await?
        } else {
            return Ok(vec![]);
        };

        Ok(siblings
            .into_iter()
            .filter(|s: &T| s.id() != block_id)
            .collect())
    }

    /// Get the previous sibling (the child immediately before `block_id` in
    /// its parent's ordered child list).
    async fn get_prev_sibling(&self, block_id: &EntityUri) -> Result<Option<T>> {
        let block: T = self
            .get_by_id(block_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        let siblings: Vec<T> = if let Some(pid) = block.parent_id() {
            self.children_ordered(pid).await?
        } else {
            return Ok(None);
        };

        let pos = siblings.iter().position(|s: &T| s.id() == block_id);
        Ok(pos
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| siblings.into_iter().nth(i)))
    }

    /// Get the next sibling (the child immediately after `block_id` in its
    /// parent's ordered child list).
    async fn get_next_sibling(&self, block_id: &EntityUri) -> Result<Option<T>> {
        let block: T = self
            .get_by_id(block_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        let siblings: Vec<T> = if let Some(pid) = block.parent_id() {
            self.children_ordered(pid).await?
        } else {
            return Ok(None);
        };

        let pos = siblings.iter().position(|s: &T| s.id() == block_id);
        Ok(pos.and_then(|i| siblings.into_iter().nth(i + 1)))
    }

    /// Get the first child of a parent (first in ordered child list).
    async fn get_first_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<T>> {
        let children: Vec<T> = if let Some(pid) = parent_id {
            self.children_ordered(pid).await?
        } else {
            return Ok(None);
        };
        Ok(children.into_iter().next())
    }

    /// Get the last child of a parent (last in ordered child list).
    async fn get_last_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<T>> {
        let children: Vec<T> = if let Some(pid) = parent_id {
            self.children_ordered(pid).await?
        } else {
            return Ok(None);
        };
        Ok(children.into_iter().last())
    }
}

/// Mutating maintenance operations on block hierarchies (depth updates,
/// rebalancing)
#[async_trait]
pub trait BlockMaintenanceHelpers<T>: CrudOperations<T> + DataSource<T>
where
    T: BlockEntity + MaybeSendSync + 'static,
{
    /// Recursively update depths of all descendants when a parent's depth
    /// changes
    async fn update_descendant_depths(
        &self,
        parent_id: &EntityUri,
        depth_delta: i64,
    ) -> Result<()> {
        if depth_delta == 0 {
            return Ok(());
        }

        let mut queue = vec![parent_id.clone()];

        while let Some(current_parent_id) = queue.pop() {
            let children: Vec<T> = self.get_children(&current_parent_id).await?;

            for child in children {
                let current_depth = child.depth();
                let new_depth = current_depth + depth_delta;
                self.set_field(child.id().as_str(), "depth", Value::Integer(new_depth))
                    .await?;
                queue.push(child.id().clone());
            }
        }

        Ok(())
    }
}

/// Backward-compatible alias combining both query and maintenance helpers
pub trait BlockDataSourceHelpers<T>: BlockQueryHelpers<T> + BlockMaintenanceHelpers<T>
where
    T: BlockEntity + MaybeSendSync + 'static,
{
}

/// Read a block's content through the cell registry. Returns `None`
/// when the registry is absent (synthetic test stores) or can't
/// resolve the field (block not yet in the Loro tree, SqlOnly mode).
/// Callers fall back to `T::content()` from the persistent store on
/// `None`. The Loro-backed cell, when present, returns the
/// post-keystroke text that the editor has been writing per-character —
/// so chord ops like `split_block` operate on what the user sees, not
/// the lagging SQL projection.
fn read_content_via_cells(
    registry: Option<&dyn crate::cell_registry::EntityCellRegistry>,
    uri: &EntityUri,
) -> Option<String> {
    let reg = registry?;
    let cell = reg.live_field::<String>(uri, "content").ok()?; // ALLOW(ok): expected fall-through when block not in Loro tree / SqlOnly mode
    Some(cell.current())
}

/// Authoritative block create through the cell registry.
///
/// Routes `split_block`'s new-block create into Loro (tree.create +
/// LoroText init + positional move). The outbound projector then emits
/// the SQL INSERT tagged `EventOrigin::Loro`, which the inbound gate
/// `EchoSuppress`es. The SQL-direct `BlockOperations::create` path tags
/// events `EventOrigin::Other("sql")`, which the post-3.3-flip gate
/// drops as an unmigrated chord-op write.
///
/// Returns `Ok(false)` when no cell route is available (synthetic
/// stores, SqlOnly mode); caller invokes `BlockOperations::create`. //
/// ALLOW(fallback): doc describes default path
async fn create_block_via_cells(
    registry: Option<&dyn crate::cell_registry::EntityCellRegistry>,
    parent_id: &EntityUri,
    after_id: Option<&EntityUri>,
    new_id: &EntityUri,
    content: holon_api::BlockContent,
) -> Result<bool> {
    let Some(reg) = registry else {
        return Ok(false);
    };
    reg.create_entity(
        parent_id,
        after_id,
        new_id,
        content,
        &std::collections::HashMap::<String, holon_api::Value>::new(),
        &Tags::default(),
        &[],
        &[],
    )
    .await
    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })
}

/// Authoritative block delete through the cell registry.
///
/// Mirrors [`create_block_via_cells`]: routes `join_block`'s merged-away
/// block delete into Loro (`tree.delete`), so the outbound projector emits
/// the SQL DELETE tagged `EventOrigin::Loro`. The SQL-direct
/// `BlockOperations::delete` path removes only the SQL row — the inbound
/// gate drops its event for Loro, leaving the dead block alive in the Loro
/// tree (the Full-slice SplitBlock→DeleteBackward divergence).
///
/// Returns `Ok(false)` when no cell route is available (synthetic stores,
/// SqlOnly mode); caller invokes `BlockOperations::delete`. // ALLOW(fallback):
/// doc describes default path
async fn delete_block_via_cells(
    registry: Option<&dyn crate::cell_registry::EntityCellRegistry>,
    id: &EntityUri,
) -> Result<bool> {
    let Some(reg) = registry else {
        return Ok(false);
    };
    reg.delete_entity(id)
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })
}

/// Hierarchical structure operations (for any block-like entity)
///
/// This trait provides operations for manipulating block hierarchies.
/// It requires that the entity type implements `BlockEntity` and that
/// the datasource implements `BlockDataSourceHelpers`.
#[holon_macros::operations_trait]
#[async_trait]
pub trait BlockOperations<T>: BlockDataSourceHelpers<T>
where
    T: BlockEntity + MaybeSendSync + 'static,
{
    /// Per-`(EntityUri, field)` reactive cell registry for this block
    /// store. Default `None` keeps the synthetic in-memory test substrate
    /// (`block_operations_tests.rs::MemStore`) working without rewriting:
    /// chord ops fall through to `T::content()` whenever `cells()` returns
    /// `None`. Production impls (`SqlBlockOperations` in Full mode)
    /// override to return their DI-resolved registry so chord ops read
    /// the live CRDT view of `block.content` instead of the lagging SQL
    /// projection.
    fn cells(&self) -> Option<&dyn crate::cell_registry::EntityCellRegistry> {
        None
    }

    /// Block positional-intent provider — encapsulates the (Loro tree.mov
    /// vs SqlOnly gen_key_between) split behind a typed API. Default
    /// `None` keeps the synthetic in-memory test substrate working;
    /// production impls override to return their wired ordering.
    /// Chord ops (`move_to_position`, `split_block`, ...) require
    /// `Some(_)` and panic if it's missing in a context that demands it.
    fn ordering(&self) -> Option<&dyn crate::block_ordering::BlockOrdering> {
        None
    }

    /// The order-key minter for this store, present **only** when this store is
    /// the `Store` consolidator that owns sibling order (SqlOnly mode).
    /// Loro-mode stores return `None` here by construction: in Loro mode
    /// the tree owns the fractional index and `apply_create` derives it
    /// from `position_after_block_id`, so no key is ever minted on that
    /// path. This is the type-level successor to the former Loro-mode
    /// `new_child_anchor` placeholder (Replication.md §5): `split_block`'s
    /// SqlOnly create branch reaches minting through this seam, and the
    /// Loro path can't reach it at all — the method doesn't exist on the
    /// Loro ordering seam.
    fn order_key_minter(&self) -> Option<&dyn crate::block_ordering::OrderKeyMinting> {
        None
    }

    /// Move block under its previous sibling (increase indentation).
    ///
    /// Delegates to [`move_block`] for the actual reparenting. The hand-rolled
    /// version of `indent` previously called `self.set_field("parent_id", …)`
    /// directly, which mutated SQL but did not fire the matview CDC events the
    /// UI watcher subscribes to — pressing Tab would land in the DB but the
    /// tree never re-rendered. `outdent` already routes through `move_block`
    /// and works correctly; mirroring that path here yields the same CDC
    /// propagation, plus the recursive-depth-update for descendants that the
    /// inline implementation was missing.
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    #[holon_macros::menu_exposure(listed)]
    async fn indent(&self, id: &EntityUri) -> Result<OperationResult> {
        let id_str = id.as_str();
        let block = self
            .get_by_id(id_str)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        // `move_block` enforces the "must have a parent" invariant, but we
        // check up-front to keep the indent-specific error message.
        block
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot indent root block"))?;

        let prev_sibling = self.get_prev_sibling(id).await?.ok_or_else(|| {
            anyhow::anyhow!("Cannot indent: no previous sibling to become parent")
        })?;
        let new_parent_uri = prev_sibling.id().clone();

        // Indent semantics: the indented block becomes the LAST child of the
        // previous sibling. `move_block` interprets `after_block_id = None`
        // as "insert at the beginning", so we look up the new parent's
        // current last child and pass its id as the anchor. The anchor must
        // come from the positional authority (`BlockOrdering`: Loro live tree
        // in Loro mode, sort_key order in SqlOnly) — `get_children` is
        // UNORDERED (a `get_all` filter), so `.last()` on it picks an
        // arbitrary sibling and the indented block lands mid-group.
        let after_uri = match self.ordering() {
            Some(ordering) => ordering.children(&new_parent_uri).await?.pop(),
            // Synthetic in-memory test substrate (no wired ordering): keep the
            // unordered read; those substrates have no positional authority.
            None => self
                .get_children(&new_parent_uri)
                .await?
                .last()
                .map(|c| c.id().clone()),
        };
        self.move_block(id, &new_parent_uri, after_uri.as_ref())
            .await
    }

    /// Position `id` under `parent_id`, immediately after `after_block_id`
    /// (or first when `None`). Delegates to the impl's `BlockOrdering` —
    /// see `crates/holon-core/src/block_ordering.rs`.
    async fn move_to_position(
        &self,
        id: &EntityUri,
        parent_id: &EntityUri,
        after_block_id: Option<&EntityUri>,
    ) -> Result<Vec<FieldDelta>> {
        let ordering = self.ordering().ok_or_else(|| {
            anyhow::anyhow!(
                "move_to_position requires a BlockOrdering — this BlockOperations impl returned \
                 None from ordering()"
            )
        })?;
        ordering.place(id, parent_id, after_block_id).await?;
        Ok(Vec::new())
    }

    /// Move block to different position (reorder within same parent or
    /// different parent)
    ///
    /// # Parameters
    /// * `id` - Block ID to move
    /// * `parent_id` - Target parent ID (must always have a parent)
    /// * `after_block_id` - Optional anchor block (move after this block, or
    ///   beginning if None)
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    #[holon_macros::triggered_by(availability_of = "tree_position", providing = ["parent_id", "after_block_id"])]
    #[holon_macros::triggered_by(availability_of = "selected_id", providing = ["parent_id"])]
    #[holon_macros::menu_exposure(pointer_gesture)]
    async fn move_block(
        &self,
        id: &EntityUri,
        parent_id: &EntityUri,
        after_block_id: Option<&EntityUri>,
    ) -> Result<OperationResult> {
        let id_str = id.as_str();
        // Capture old state before mutation
        let maybe_block: Option<T> = self.get_by_id(id_str).await?;
        let block: T = maybe_block.ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let old_parent_uri = block
            .parent_id()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Cannot move root block"))?;
        let old_predecessor = self.get_prev_sibling(id).await?;
        let old_depth = block.depth();

        // Compute new depth for descendants' delta-update.
        let maybe_parent: Option<T> = self.get_by_id(parent_id.as_str()).await?;
        let parent: T = maybe_parent.ok_or_else(|| anyhow::anyhow!("Parent not found"))?;

        // No-pages-under-non-pages (interim ruling 2026-07-13, Fork B B1): a page
        // block may only be reparented under another page (an org file nests only
        // under an org file). Enforced HERE, at the single shared write chokepoint
        // for every reparenting op — `move_block` itself plus `indent`/`outdent`/
        // `move_up`/`move_down`, which all route through it — so both the SQL and
        // Loro providers (each using this default `BlockOperations` impl) reject it
        // identically. This is the WRITE-side guard; `name_chain` (writeback) is the
        // downstream READ-side tripwire. Fail loud rather than let the prohibited
        // topology land and surface deep in writeback.
        if crate::block_op_catalog::page_under_non_page_prohibited(
            block.is_page(),
            Some(parent.is_page()),
        ) {
            return Err(anyhow::anyhow!(
                "move_block: refusing to reparent page block '{}' under non-page parent '{}' — \
                 pages under non-pages are prohibited (interim ruling 2026-07-13); a page may only \
                 nest under another page",
                id_str,
                parent_id.as_str(),
            )
            .into());
        }

        let new_depth = parent.depth() + 1;
        let depth_delta = new_depth - old_depth;

        // Position + depth update.
        let mut changes = self.move_to_position(id, parent_id, after_block_id).await?;
        // Disclose the reparent itself: `move_to_position` reports no deltas
        // (ordering-internal), but parent_id DID change — propagation consumers
        // and the undo precondition both need the true field-level change.
        changes.push(FieldDelta::new(
            id_str,
            "parent_id",
            Value::String(old_parent_uri.as_str().to_string()),
            Value::String(parent_id.as_str().to_string()),
        ));
        let depth_result = self
            .set_field(id_str, "depth", Value::Integer(new_depth))
            .await?;
        changes.extend(depth_result.changes);

        // Recursively update all descendants' depths by the same delta
        // Note: update_descendant_depths calls set_field internally, so it will also
        // return FieldDeltas For now, we'll skip collecting those to avoid
        // complexity
        if depth_delta != 0 {
            self.update_descendant_depths(id, depth_delta).await?;
        }

        // Return inverse operation using macro-generated helper
        use crate::__operations_block_operations;

        // Entity name will be set by OperationProvider when operation is executed
        let old_pred_uri = old_predecessor.as_ref().map(|p| p.id().clone());
        Ok(OperationResult::new(
            changes,
            __operations_block_operations::move_block_op(
                "placeholder", /* OperationDispatcher overwrites this with the resolved
                                * entity_name (see operation_dispatcher.rs:504). EntityName::new
                                * debug-asserts on empty/invalid scheme, so we use a valid
                                * placeholder. */
                id,
                &old_parent_uri,
                old_pred_uri.as_ref(),
            ),
        ))
    }

    /// Move block out to parent's level (decrease indentation)
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    #[holon_macros::menu_exposure(listed)]
    async fn outdent(&self, id: &EntityUri) -> Result<OperationResult> {
        let id_str = id.as_str();
        let maybe_block: Option<T> = self.get_by_id(id_str).await?;
        let block: T = maybe_block.ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_id = block
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot outdent root block"))?;

        let maybe_parent: Option<T> = self.get_by_id(parent_id.as_str()).await?;
        let parent: T = maybe_parent.ok_or_else(|| anyhow::anyhow!("Parent not found"))?;
        let grandparent_id = parent
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot outdent: parent is already at root level"))?;

        // Capture old predecessor before move (for inverse operation)
        let old_parent_uri = parent_id.clone();
        let old_predecessor = self.get_prev_sibling(id).await?;

        // Move to grandparent's children, after parent
        let grandparent_uri = grandparent_id.clone();
        let parent_uri = old_parent_uri.clone();
        let move_result = self
            .move_block(id, &grandparent_uri, Some(&parent_uri))
            .await?;

        // Return inverse: move_block back to old parent after old predecessor.
        // We can't use indent_op here because indent now resolves the previous sibling
        // dynamically, which wouldn't restore the exact original position.
        use crate::__operations_block_operations;

        // Entity name will be set by OperationProvider when operation is executed
        let old_pred_uri = old_predecessor.as_ref().map(|p| p.id().clone());
        Ok(OperationResult::new(
            move_result.changes,
            __operations_block_operations::move_block_op(
                "placeholder", /* OperationDispatcher overwrites this with the resolved
                                * entity_name (see operation_dispatcher.rs:504). EntityName::new
                                * debug-asserts on empty/invalid scheme, so we use a valid
                                * placeholder. */
                id,
                &old_parent_uri,
                old_pred_uri.as_ref(),
            ),
        ))
    }

    /// Split a block at a given position
    ///
    /// Creates a new block with content after the cursor and truncates
    /// the original block to content before the cursor. The new block
    /// appears directly below the original block using fractional indexing.
    ///
    /// # Parameters
    /// * `id` - Block ID to split
    /// * `position` - Character position to split at (as i64, will be converted
    ///   to usize)
    #[holon_macros::affects("content")]
    #[holon_macros::menu_exposure(keyboard_gesture)]
    async fn split_block(&self, id: &EntityUri, position: i64) -> Result<OperationResult> {
        use uuid::Uuid;

        let id_str = id.as_str();
        let maybe_block: Option<T> = self.get_by_id(id_str).await?;
        let block: T = maybe_block.ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        // Page blocks have null `parent_id` (the visible `__document_root__`
        // parent is added by hydration, not stored in SQL). Splitting them
        // would orphan the new block under `sentinel:no_parent` and is
        // semantically meaningless — Enter on a Page should never split
        // the Page itself.
        if block.is_page() {
            return Err(anyhow::anyhow!("Refusing to split Page block {id_str}").into());
        }

        // Prefer the live (Loro) view of the block's text when available
        // through the cell registry; the per-keystroke writes that produced
        // the cursor position the user sees may not have projected into
        // `block.content()` (the SQL copy) yet. Falls through to the
        // stored content when `cells()` is `None` (synthetic test stores)
        // or when the cell registry can't resolve the field (SqlOnly mode,
        // block not yet in Loro tree).
        let split_uri = id.clone();
        let content_owned = read_content_via_cells(self.cells(), &split_uri)
            .unwrap_or_else(|| block.content().to_string());
        let content: &str = &content_owned;

        // Convert i64 to usize (validate it's non-negative and fits in usize)
        if position < 0 {
            return Err(anyhow::anyhow!("Position must be non-negative").into());
        }
        let position = position as usize;

        // Validate offset is within bounds
        if position > content.len() {
            return Err(anyhow::anyhow!(
                "Split position {} exceeds content length {}",
                position,
                content.len()
            )
            .into());
        }

        // Split content at cursor
        let mut content_before = content[..position].to_string();
        let mut content_after = content[position..].to_string();

        // Strip trailing whitespace from the old block
        content_before = content_before.trim_end().to_string();

        // Strip leading whitespace from the new block
        content_after = content_after.trim_start().to_string();

        // Generate new block ID. Mirror the rest of the system's URI
        // convention: SQL `block.id` stores the prefixed form (`block:UUID`)
        // because `EntityUri::block(uuid)` serializes as `Value::String("block:UUID")`
        // when blocks land via the parser / CDC. Storing a bare UUID here
        // would create an id-format mismatch — every later
        // `get_by_id`, `parent_id` lookup, and `EntityUri::try_from(Value)`
        // round-trip would silently miss this block.
        let new_block_uuid = Uuid::new_v4().to_string();
        let new_block_id = format!("block:{new_block_uuid}");

        // Get current timestamp
        let now = holon_api::clock::now_millis();

        // Route the new-block create through the cell registry when a Loro
        // backing is available. The SQL-direct `self.create(...)` path
        // publishes a `Created` event tagged `EventOrigin::Other("sql")`,
        // which the post-Phase-3.3 inbound runtime gate drops as an
        // unmigrated chord-op write — leaving Loro without the new block
        // and the old block's SQL `content` column stuck on the
        // pre-split value (the prefix-trim UPDATE then has nothing to
        // race against, but the cell write also can't propagate because
        // the projector sees inconsistent state). Routing through Loro
        // first makes the outbound `LoroSyncController.on_loro_changed`
        // the only SQL writer, with `EventOrigin::Loro` events that the
        // gate correctly `EchoSuppress`es.
        let parent_for_split = block
            .parent_id()
            .cloned()
            .unwrap_or_else(EntityUri::no_parent);
        let new_block_uri = EntityUri::block(&new_block_uuid);
        let after_uri = id.clone();
        let wrote_create_via_cell = create_block_via_cells(
            self.cells(),
            &parent_for_split,
            Some(&after_uri),
            &new_block_uri,
            // The new half of a split is always plain text — the reference
            // model's `split_block` creates `Block::new_text`. (Splitting a
            // source block is not a user-reachable op.)
            holon_api::BlockContent::text(content_after.clone()),
        )
        .await?;

        tracing::trace!(
            "[split_block] new_block_id={} parent={} after={} wrote_create_via_cell={}",
            new_block_id,
            parent_for_split.as_str(),
            after_uri.as_str(),
            wrote_create_via_cell,
        );

        let mut changes = Vec::new();
        if !wrote_create_via_cell {
            // ALLOW(fallback): synthetic-store / SqlOnly mode has no Loro
            // authority — the SQL `create` path is the only way to persist
            // the new block. Disclosed and intentional.
            //
            // sort_key for the new block: mint it here, on the SqlOnly branch
            // ONLY — this is the sole path that persists a `sort_key` directly.
            // The key comes through the `OrderKeyMinting` seam, which exists
            // exclusively on the `Store` consolidator's order owner. The Loro
            // path (`wrote_create_via_cell == true`) never reaches this branch
            // and, by construction, has no minter to reach: the fractional index
            // is authoritative in the tree and `apply_create` derives it from
            // `position_after_block_id` (Replication.md §5).
            let parent_for_anchor = block
                .parent_id()
                .cloned()
                .unwrap_or_else(EntityUri::no_parent);
            let minter = self.order_key_minter().ok_or_else(|| {
                anyhow::anyhow!(
                    "split_block's SqlOnly create path requires an OrderKeyMinting seam (the \
                     Store consolidator's order owner) — this BlockOperations impl returned None \
                     from order_key_minter()"
                )
            })?;
            let new_sort_key = minter
                .new_child_anchor(&parent_for_anchor, Some(id)) // ALLOW(order_minting): routed through the sibling-set owner's OrderKeyMinting seam
                // via order_key_minter() above, not minted locally
                .await?;

            let mut new_block_fields = crate::storage::types::StorageEntity::new();
            new_block_fields.insert("id".into(), Value::String(new_block_id.clone()));
            new_block_fields.insert("content".into(), Value::String(content_after));
            new_block_fields.insert("parent_id".into(), {
                if let Some(ref pid) = block.parent_id() {
                    Value::String(pid.to_string())
                } else {
                    Value::Null
                }
            });
            new_block_fields.insert("depth".into(), Value::Integer(block.depth()));
            new_block_fields.insert("sort_key".into(), Value::String(new_sort_key));
            // Positional intent for Full (Loro) mode. The literal key here
            // must match `event_bus::POSITION_AFTER_BLOCK_ID_PARAM` over in the
            // `holon` crate — we can't depend on it from `holon-core`, so the
            // contract is duplicated as a string. `SqlOperationProvider::
            // prepare_create` strips it from SQL fields and from the event
            // payload, and lifts the value onto the typed
            // `Event::position_after_block_id` field that `apply_create`
            // reads.
            new_block_fields.insert("after_block_id".into(), Value::String(id_str.to_string()));
            new_block_fields.insert("created_at".into(), Value::Integer(now));
            new_block_fields.insert("updated_at".into(), Value::Integer(now));
            new_block_fields.insert("collapsed".into(), Value::Boolean(false));
            new_block_fields.insert("completed".into(), Value::Boolean(false));
            new_block_fields.insert("block_type".into(), Value::String("text".to_string()));

            let (_new_block_id, create_result) = self.create(new_block_fields).await?;
            changes.extend(create_result.changes);
        } else {
            // Loro (cell) create path emits no FieldDelta of its own. Fingerprint
            // the new block's post-split content so the split's inverse
            // (`restore_join`, which DELETES this block) is dropped LOUDLY if the
            // block was deleted or edited under a later undo — closing the
            // stale-guard gap that let an undo-after-delete destroy unrelated
            // blocks (BugFunnel dogfood #4). `content` is a projected column the
            // `SqlUndoStateReader` can read.
            changes.push(FieldDelta::new(
                new_block_id.clone(),
                "content",
                Value::Null,
                Value::String(content_after.clone()),
            ));
        }

        // Update current block with truncated content. Prefer writing
        // through the Loro CRDT layer (via the cell registry) when
        // available so the `LoroSyncController.on_loro_changed` outbound
        // projector becomes the single SQL writer for content. Drops
        // through to a direct `set_field` write when no cell route exists
        // (SqlOnly mode, synthetic in-memory test store, block not yet in
        // Loro tree).
        // `set_field` is the single content-write seam: it routes `content`
        // through the cell registry (Loro in Full mode) and falls back to a
        // direct SQL write when no cell route exists (SqlOnly, synthetic test
        // store, block not yet in the Loro tree).
        let content_result = self
            .set_field(id_str, "content", Value::String(content_before))
            .await?;
        changes.extend(content_result.changes);

        // Focus moves to the new block at caret offset 0. This is returned in
        // the op response (not dispatched as a backend `editor_focus`
        // follow-up): the frontend reads it off the result and moves the
        // in-memory focus authority in-process, so focus never round-trips
        // through the Turso `editor_cursor` cache (ADR 0010).
        //
        // Inverse: collapse the split. `restore_join` deletes the new block and
        // resets the original block's content to its EXACT pre-split value
        // (`content_owned`, captured before the prefix/suffix whitespace trim) —
        // so the untrimmed original is restored byte-for-byte, not the trimmed
        // `content_before`. Its own returned inverse re-splits deterministically
        // (same new-block id) so redo re-applies.
        use crate::__operations_block_operations;
        Ok(OperationResult::new(
            changes,
            __operations_block_operations::restore_join_op(
                // OperationDispatcher overwrites this with the resolved entity_name
                // (see operation_dispatcher.rs:588). Valid placeholder scheme so
                // EntityName::new's debug-assert passes.
                "placeholder",
                id,
                content_owned.clone(),
                &new_block_uri,
            ),
        )
        .with_response(focus_response(&new_block_id, 0)))
    }

    /// Join a block into its merge target.
    ///
    /// Two cases, both triggered by Backspace at position 0:
    ///   1. **Previous sibling exists** — symmetric inverse of `split_block`:
    ///        - appends `id`'s content to the end of the previous sibling
    ///        - re-parents `id`'s children under the previous sibling, placed
    ///          after any existing children of the previous sibling
    ///        - deletes `id`
    ///   2. **No previous sibling** (block is the first child) — child→parent
    ///      join, the natural extension when there's no prev to merge into:
    ///        - appends `id`'s content to the end of the **parent**
    ///        - re-parents `id`'s children under the parent, placed at `id`'s
    ///          old slot (i.e. before any of `id`'s former siblings)
    ///        - deletes `id`
    ///
    /// In either case the editor cursor moves onto the merge target at the
    /// join boundary (= old target content length).
    ///
    /// # Parameters
    /// * `id` - Block to join
    /// * `position` - Cursor position; non-zero positions are no-ops (returns
    ///   `Ok` with no changes). Real frontends only dispatch this op when the
    ///   cursor is at byte 0, but the SQL caller path may pass through stale
    ///   positions, so we re-check here.
    #[holon_macros::affects("content", "parent_id", "sort_key")]
    async fn join_block(&self, id: &EntityUri, position: i64) -> Result<OperationResult> {
        if position != 0 {
            return Ok(OperationResult::irreversible(vec![]));
        }

        let id_str = id.as_str();
        let block: T = self
            .get_by_id(id_str)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        // Prefer the live (Loro) view via the cell registry; same reasoning
        // as `split_block` — SQL `block.content` lags per-keystroke writes.
        let join_block_uri = id.clone();
        let block_content = read_content_via_cells(self.cells(), &join_block_uri)
            .unwrap_or_else(|| block.content().to_string());
        let block_id_str = block.id().to_string();

        // Pick merge target: prev sibling if any, else the parent.
        let prev_opt: Option<T> = self.get_prev_sibling(id).await?;
        let into_parent = prev_opt.is_none();
        let target: T = if let Some(prev) = prev_opt {
            prev
        } else {
            let parent_id = block.parent_id().ok_or_else(|| {
                anyhow::anyhow!("Cannot join: block has no previous sibling and no parent")
            })?;
            self.get_by_id(parent_id.as_str())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Cannot join: parent {parent_id} not found"))?
        };

        let target_uri = target.id().clone();
        let target_content = read_content_via_cells(self.cells(), &target_uri)
            .unwrap_or_else(|| target.content().to_string());
        let join_offset = target_content.len();
        let new_content = format!("{}{}", target_content, block_content);
        let target_id = target.id().to_string();

        // Children must come from the positional authority when one is wired
        // (`BlockOrdering`) — `get_children` is UNORDERED (a `get_all`
        // filter), so iterating it re-parents the subtree in arbitrary order.
        let block_children: Vec<EntityUri> = match self.ordering() {
            Some(ordering) => ordering.children(&join_block_uri).await?,
            // Synthetic in-memory test substrate (no wired ordering): keep the
            // unordered read; those substrates have no positional authority.
            None => self
                .get_children(&join_block_uri)
                .await?
                .iter()
                .map(|c| c.id().clone())
                .collect(),
        };
        // Captured before `block_children` is consumed by the re-parent loop:
        // the undo inverse is only exact for the leaf case (see the inverse
        // construction at the tail of this method).
        let block_had_children = !block_children.is_empty();

        // Case-B refusal (Phase 3.5): joining a first-child block into its
        // parent when it has children of its own would orphan or reposition
        // the grandchildren. LogSeq refuses this action in the same shape;
        // we mirror that here — Backspace at start of the first child with
        // its own subtree is a no-op rather than a partial mutation.
        if into_parent && !block_children.is_empty() {
            return Ok(OperationResult::irreversible(vec![]));
        }

        let mut changes = Vec::new();

        // Re-parent `id`'s children under `target_id`, appended after
        // `target`'s existing children. Only the case-A branch (prev
        // sibling exists) can reach this — the case-B-with-children
        // branch is refused above.
        //
        // We thread `last_after_id` (a stable block id) LOCALLY across
        // iterations instead of re-querying `target.children().last()` to
        // sidestep the SQL projection race: in Full (Loro) mode each
        // `move_to_position` commits to Loro synchronously, but the SQL
        // projection lands asynchronously via the outbound projector, so
        // a fresh `get_children(target_id)` mid-loop could miss the
        // child that was just re-parented. Item 4 phase 3: each iteration
        // calls `move_to_position(child, target, last_after_id)`, which
        // in Loro mode routes to `write_position` → `update_block_position`
        // (typed predecessor; no gen_key_between in the hot path) and in
        // SqlOnly mode falls back to the legacy compute + paired
        // `set_field` shape.
        if !block_children.is_empty() {
            let move_target_uri = target_uri.clone();
            // Same authority rule as above: the append anchor must be the
            // last child IN ORDER, not `.last()` of an unordered read.
            let mut last_after_uri: Option<EntityUri> = match self.ordering() {
                Some(ordering) => ordering.children(&move_target_uri).await?.pop(),
                None => self
                    .get_children(&move_target_uri)
                    .await?
                    .last()
                    .map(|c| c.id().clone()),
            };
            for child_uri in block_children {
                let move_changes = self
                    .move_to_position(&child_uri, &move_target_uri, last_after_uri.as_ref())
                    .await?;
                changes.extend(move_changes);
                last_after_uri = Some(child_uri);
            }
        }

        // Append `id`'s content to the merge target. `set_field` is the
        // single content-write seam — it routes through the cell registry
        // (Loro in Full mode) and falls back to a direct SQL write when no
        // cell route exists.
        let content_result = self
            .set_field(&target_id, "content", Value::String(new_content))
            .await?;
        changes.extend(content_result.changes);

        // Delete `id` (its children have already been re-parented). Route
        // through the cell registry (Loro authority) when available —
        // mirroring the split's `create_block_via_cells` — so the block
        // leaves the Loro tree and the outbound projector emits the SQL
        // DELETE. The SQL-direct `self.delete` deletes only the SQL row,
        // leaving the dead block alive in Loro.
        let wrote_delete_via_cell = delete_block_via_cells(self.cells(), &join_block_uri).await?;
        if !wrote_delete_via_cell {
            // ALLOW(fallback): synthetic-store / SqlOnly mode has no Loro
            // authority — the SQL delete is the only persistence path.
            let delete_result = self.delete(&block_id_str).await?;
            changes.extend(delete_result.changes);
        }

        // Prune any cached cells for the now-deleted block so a same-id
        // re-create within the same session can't observe a stale Cell
        // wrapping an orphaned `LoroText` container.
        if let Some(reg) = self.cells() {
            reg.on_entity_deleted(&join_block_uri);
        }

        // Inverse: re-split the merge. `restore_split` recreates the merged-away
        // block at its recorded slot (after the merge target in the prev-sibling
        // case, or as first child in the child→parent case) and resets the merge
        // target's content to its EXACT pre-join value (`target_content`).
        //
        // Only the leaf case is reversible: when the merged-away block had its
        // own children they were re-parented under the target, and this single
        // inverse cannot restore that subtree placement exactly. Declare it
        // irreversible (fail loud rather than ship a lossy inverse) — the
        // caret-position no-ops (position != 0), the refused
        // case-B-with-children, and this case-A-with-children all stay
        // irreversible by construction.
        let inverse: UndoAction = if !block_had_children {
            let block_parent = block
                .parent_id()
                .cloned()
                .unwrap_or_else(EntityUri::no_parent);
            // Slot anchor: the merged-away block sat directly after the merge
            // target in the prev-sibling case; in the child→parent case it was
            // the parent's first child (anchor `None`).
            let after: Option<EntityUri> = if into_parent {
                None
            } else {
                Some(target_uri.clone())
            };
            use crate::__operations_block_operations;
            UndoAction::Undo(__operations_block_operations::restore_split_op(
                // OperationDispatcher overwrites this placeholder (see split_block).
                "placeholder",
                &target_uri,
                target_content.clone(),
                id,
                block_content.clone(),
                &block_parent,
                block.depth(),
                after.as_ref(),
            ))
        } else {
            UndoAction::DeclaredIrreversible(
                "join_block: merged-away block had children re-parented under the target; a flat \
                 inverse cannot restore that subtree placement",
            )
        };

        // Focus moves to the merge target at the join boundary. Returned in
        // the op response (see `split_block`) rather than dispatched as a
        // backend `editor_focus` follow-up — the frontend applies it in
        // process, no Turso `editor_cursor` round-trip (ADR 0010).
        Ok(OperationResult {
            changes,
            undo: inverse,
            response: Some(focus_response(&target_id, join_offset as i64)),
            follow_ups: vec![],
        })
    }

    /// Inverse primitive — recreate a block and reset a sibling's content.
    ///
    /// The exact inverse of [`restore_join`]. Recreates `block_id` under
    /// `block_parent` (positioned directly after `after_id`, or as the first
    /// child when `after_id` is `None`) with `block_content`, then resets
    /// `target_id`'s content to `target_content`.
    ///
    /// This is the machine-generated inverse behind undo/redo of
    /// `split_block` / `join_block`; it is not a user-facing editor action.
    /// Positioning goes through the SAME create seam those ops use: the Loro
    /// cell registry when present (preserving sibling ORDER — the projection
    /// oracle's contract — while the fractional index is re-derived), and the
    /// `OrderKeyMinting` order owner otherwise (SqlOnly / synthetic stores),
    /// which for a deterministic minter reproduces the original `sort_key`
    /// byte-for-byte between the same neighbours.
    #[holon_macros::affects("content", "parent_id", "sort_key")]
    async fn restore_split(
        &self,
        target_id: &EntityUri,
        target_content: String,
        block_id: &EntityUri,
        block_content: String,
        block_parent: &EntityUri,
        block_depth: i64,
        after_id: Option<&EntityUri>,
    ) -> Result<OperationResult> {
        // Capture the target's current content so the returned inverse can put
        // it back on redo.
        let target_prior = match read_content_via_cells(self.cells(), target_id) {
            Some(c) => c,
            None => self
                .get_by_id(target_id.as_str())
                .await?
                .ok_or_else(|| anyhow::anyhow!("restore_split: target {target_id} not found"))?
                .content()
                .to_string(),
        };

        // Recreate the block. Prefer the Loro cell seam (positions after
        // `after_id`); fall back to a SQL create with an explicitly-minted
        // positional key for byte-identical restoration in SqlOnly / synthetic
        // stores. Mirrors `split_block`'s two create branches.
        let wrote_via_cell = create_block_via_cells(
            self.cells(),
            block_parent,
            after_id,
            block_id,
            holon_api::BlockContent::text(block_content.clone()),
        )
        .await?;

        let mut changes = Vec::new();
        if !wrote_via_cell {
            // ALLOW(fallback): SqlOnly / synthetic store — no Loro authority.
            let minter = self.order_key_minter().ok_or_else(|| {
                anyhow::anyhow!(
                    "restore_split's SqlOnly create path requires an OrderKeyMinting seam (the \
                     Store consolidator's order owner) — this BlockOperations impl returned None \
                     from order_key_minter()"
                )
            })?;
            let new_sort_key = minter // ALLOW(order_minting): routed through the sibling-set owner's OrderKeyMinting seam
                .new_child_anchor(block_parent, after_id)
                .await?;
            let mut fields = crate::storage::types::StorageEntity::new();
            fields.insert("id".into(), Value::String(block_id.as_str().to_string()));
            fields.insert("content".into(), Value::String(block_content.clone()));
            fields.insert(
                "parent_id".into(),
                Value::String(block_parent.as_str().to_string()),
            );
            fields.insert("sort_key".into(), Value::String(new_sort_key));
            fields.insert("depth".into(), Value::Integer(block_depth));
            let (_new_id, create_result) = self.create(fields).await?;
            changes.extend(create_result.changes);
        }

        // Reset the target's content.
        let content_result = self
            .set_field(target_id.as_str(), "content", Value::String(target_content))
            .await?;
        changes.extend(content_result.changes);

        // Inverse: collapse again (delete `block_id`, restore the target's
        // pre-restore content).
        use crate::__operations_block_operations;
        Ok(OperationResult::new(
            changes,
            __operations_block_operations::restore_join_op(
                "placeholder",
                target_id,
                target_prior,
                block_id,
            ),
        ))
    }

    /// Inverse primitive — delete a leaf block and reset a sibling's content.
    ///
    /// The exact inverse of [`restore_split`]. Deletes `deleted_id` and resets
    /// `target_id`'s content to `target_content`. `deleted_id` MUST be a leaf:
    /// a block with children cannot be deleted and later restored by this
    /// single primitive (its subtree would be orphaned), so that is a loud
    /// error rather than a lossy inverse. `split_block`'s new block is always a
    /// leaf, and `join_block` only chooses this inverse for the leaf case, so
    /// the guard never trips on the sanctioned paths.
    #[holon_macros::affects("content", "parent_id", "sort_key")]
    async fn restore_join(
        &self,
        target_id: &EntityUri,
        target_content: String,
        deleted_id: &EntityUri,
    ) -> Result<OperationResult> {
        let block = self
            .get_by_id(deleted_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("restore_join: block {deleted_id} not found"))?;

        // Leaf-only guard.
        let children: Vec<EntityUri> = match self.ordering() {
            Some(ordering) => ordering.children(deleted_id).await?,
            None => self
                .get_children(deleted_id)
                .await?
                .iter()
                .map(|c| c.id().clone())
                .collect(),
        };
        if !children.is_empty() {
            return Err(anyhow::anyhow!(
                "restore_join: block {deleted_id} has {} children; the leaf-only inverse cannot \
                 restore a subtree",
                children.len()
            )
            .into());
        }

        // Capture the block's full pre-delete state + its slot so the returned
        // inverse (restore_split) can recreate it exactly.
        let block_content = read_content_via_cells(self.cells(), deleted_id)
            .unwrap_or_else(|| block.content().to_string());
        let block_parent = block
            .parent_id()
            .cloned()
            .unwrap_or_else(EntityUri::no_parent);
        let block_depth = block.depth();
        let after = self
            .get_prev_sibling(deleted_id)
            .await?
            .map(|p| p.id().clone());

        // Capture the target's current content for the redo inverse.
        let target_prior = match read_content_via_cells(self.cells(), target_id) {
            Some(c) => c,
            None => self
                .get_by_id(target_id.as_str())
                .await?
                .ok_or_else(|| anyhow::anyhow!("restore_join: target {target_id} not found"))?
                .content()
                .to_string(),
        };

        // Delete the block through the same seam split/join use.
        let mut changes = Vec::new();
        let wrote_via_cell = delete_block_via_cells(self.cells(), deleted_id).await?;
        if !wrote_via_cell {
            // ALLOW(fallback): SqlOnly / synthetic store — no Loro authority.
            let delete_result = self.delete(deleted_id.as_str()).await?;
            changes.extend(delete_result.changes);
        }
        if let Some(reg) = self.cells() {
            reg.on_entity_deleted(deleted_id);
        }

        // Reset the target's content.
        let content_result = self
            .set_field(target_id.as_str(), "content", Value::String(target_content))
            .await?;
        changes.extend(content_result.changes);

        // Inverse: expand again (recreate `deleted_id` at its slot, restore the
        // target's pre-restore content).
        use crate::__operations_block_operations;
        Ok(OperationResult::new(
            changes,
            __operations_block_operations::restore_split_op(
                "placeholder",
                target_id,
                target_prior,
                deleted_id,
                block_content,
                &block_parent,
                block_depth,
                after.as_ref(),
            ),
        ))
    }

    /// Move a block up (swap with previous sibling)
    #[holon_macros::affects("parent_id", "sort_key")]
    #[holon_macros::menu_exposure(listed)]
    async fn move_up(&self, id: &EntityUri) -> Result<OperationResult> {
        let id_str = id.as_str();
        // Capture old state
        let block = self
            .get_by_id(id_str)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_uri = block
            .parent_id()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Cannot move root block"))?;
        let old_predecessor = self.get_prev_sibling(id).await?;

        let prev_sibling: T = self
            .get_prev_sibling(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Cannot move up: no previous sibling"))?;

        // Get the sibling before prev_sibling
        let before_prev: Option<T> = self.get_prev_sibling(prev_sibling.id()).await?;

        // Execute move and collect FieldDeltas
        let move_result = if let Some(before_id) = before_prev {
            let before_uri = before_id.id().clone();
            self.move_block(id, &parent_uri, Some(&before_uri)).await?
        } else {
            // Move to beginning
            self.move_block(id, &parent_uri, None).await?
        };

        // Return inverse (move down - restore original position) using macro-generated
        // helper Use move_block_op to restore exact old position (move_up_op is
        // relative, not absolute)
        use crate::__operations_block_operations;

        let old_pred_uri = old_predecessor.as_ref().map(|p| p.id().clone());
        Ok(OperationResult::new(
            move_result.changes,
            __operations_block_operations::move_block_op(
                "placeholder", /* OperationDispatcher overwrites this with the resolved
                                * entity_name (see operation_dispatcher.rs:504). EntityName::new
                                * debug-asserts on empty/invalid scheme, so we use a valid
                                * placeholder. */
                id,
                &parent_uri,
                old_pred_uri.as_ref(),
            ),
        ))
    }

    /// Embed another entity inline by inserting a transclusion marker into the
    /// content.
    ///
    /// The `target_uri` is an EntityUri string (e.g. `block:some-id`,
    /// `todoist-task:123`). Inserts `{{transclude:target_uri}}` at the end
    /// of the block's content.
    #[holon_macros::affects("content")]
    #[holon_macros::triggered_by(availability_of = "selected_id", providing = ["target_uri"])]
    #[holon_macros::menu_exposure(listed)]
    async fn embed_entity(
        &self,
        id: &EntityUri,
        target_uri: &EntityUri,
    ) -> Result<OperationResult> {
        let id_str = id.as_str();
        let block = self
            .get_by_id(id_str)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        // Prefer live (Loro) view via the cell registry to capture any
        // pending in-memory edits.
        let embed_uri = id.clone();
        let old_content = read_content_via_cells(self.cells(), &embed_uri)
            .unwrap_or_else(|| block.content().to_string());
        let marker = format!("{{{{transclude:{}}}}}", target_uri.as_str());
        let new_content = if old_content.is_empty() {
            marker
        } else {
            format!("{old_content}\n{marker}")
        };

        // `set_field` is the single content-write seam: it routes through
        // the cell registry (Loro in Full mode) and falls back to a direct
        // SQL write when no cell route exists.
        let changes = self
            .set_field(id_str, "content", Value::String(new_content))
            .await?
            .changes;

        use crate::__operations_crud_operations;
        Ok(OperationResult::new(
            changes,
            __operations_crud_operations::set_field_op(
                "placeholder", /* Overwritten by OperationDispatcher post-execute (see
                                * operation_dispatcher.rs:504) */
                id_str,
                "content",
                Value::String(old_content),
            ),
        ))
    }

    /// Move a block down (swap with next sibling)
    #[holon_macros::affects("parent_id", "sort_key")]
    #[holon_macros::menu_exposure(listed)]
    async fn move_down(&self, id: &EntityUri) -> Result<OperationResult> {
        let id_str = id.as_str();
        // Capture old state
        let block = self
            .get_by_id(id_str)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_uri = block
            .parent_id()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Cannot move root block"))?;
        let old_predecessor = self.get_prev_sibling(id).await?;

        let next_sibling: T = self
            .get_next_sibling(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Cannot move down: no next sibling"))?;

        // Execute move after next_sibling and collect FieldDeltas
        let next_sibling_uri = next_sibling.id().clone();
        let move_result = self
            .move_block(id, &parent_uri, Some(&next_sibling_uri))
            .await?;

        // Return inverse (move up - restore original position) using macro-generated
        // helper
        use crate::__operations_block_operations;

        let old_pred_uri = old_predecessor.as_ref().map(|p| p.id().clone());
        Ok(OperationResult::new(
            move_result.changes,
            __operations_block_operations::move_block_op(
                "placeholder", /* OperationDispatcher overwrites this with the resolved
                                * entity_name (see operation_dispatcher.rs:504). EntityName::new
                                * debug-asserts on empty/invalid scheme, so we use a valid
                                * placeholder. */
                id,
                &parent_uri,
                old_pred_uri.as_ref(),
            ),
        ))
    }
}

/// Rename operations (for entities with a name field)
///
/// This trait provides a rename operation for entities that have a name or
/// title that can be changed.
#[holon_macros::operations_trait]
#[async_trait]
pub trait RenameOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Rename an entity
    #[holon_macros::affects("name")]
    async fn rename(&self, id: &str, name: String) -> Result<OperationResult>;
}

/// Move operations (for entities with hierarchical structure)
///
/// This trait provides a move operation for entities that can be moved within
/// a hierarchical structure, such as directories, files, or blocks.
#[holon_macros::operations_trait]
#[async_trait]
pub trait MoveOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Move an entity to a different position within a hierarchical structure
    ///
    /// # Parameters
    /// * `id` - Entity ID to move
    /// * `parent_id` - Target parent ID
    /// * `after_id` - Optional anchor entity (move after this entity, or
    ///   beginning if None)
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    async fn move_entity(
        &self,
        id: &str,
        parent_id: &str,
        after_id: Option<&str>,
    ) -> Result<OperationResult>;
}

/// Incremental text-edit operations (for entities with editable text content).
///
/// Used by interactive editors that issue per-keystroke / per-IME-event
/// edits. Unlike `set_field("content", new_text)` (wholesale replace),
/// these target a specific Unicode-scalar position so Loro Peritext marks
/// can adjust according to their `ExpandType` policy without losing state.
///
/// Position and length parameters are `i64` at the operation surface
/// (matching the project convention used by `split_block` etc.);
/// implementations validate non-negativity and convert to `usize` internally.
#[holon_macros::operations_trait]
#[async_trait]
pub trait TextOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Insert `text` at Unicode-scalar offset `pos` in the entity's text.
    #[holon_macros::affects("content")]
    async fn insert_text(&self, id: &str, pos: i64, text: String) -> Result<OperationResult>;

    /// Delete `len` Unicode scalars starting at `pos`.
    #[holon_macros::affects("content")]
    async fn delete_text(&self, id: &str, pos: i64, len: i64) -> Result<OperationResult>;
}

/// Inline-mark operations (for entities with rich-text content).
///
/// Incremental commands used by interactive rich-text editors. These do
/// **not** wholesale-replace the mark set the way `set_field("content",
/// Object{text, marks})` does — they target a single Unicode-scalar range
/// without disturbing marks of other keys or disjoint same-key spans.
///
/// Range parameters are `(start, end)` Unicode-scalar offsets, half-open
/// `[start, end)`. `mark_json` carries the JSON form of an `InlineMark`
/// (round-tripped via `holon_api::marks_*_json`-style serializers); `key`
/// is the stable Loro key returned by `InlineMark::loro_key()` (e.g.
/// `"bold"`, `"italic"`, `"link"`).
///
/// Implementations should reject application on entities where rich text is
/// not meaningful (e.g. SQL-only datasources, source-code blocks).
#[holon_macros::operations_trait]
#[async_trait]
pub trait MarkOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Apply a single inline mark over `[range_start, range_end)`.
    /// Other marks (different keys, or same key on disjoint ranges) are
    /// preserved. `mark_json` is the JSON form of an `InlineMark` value.
    ///
    /// Range parameters are `i64` at the operation surface (matching the
    /// project convention used by `split_block` etc.); implementations
    /// validate non-negativity and convert to `usize` internally.
    #[holon_macros::affects("marks")]
    async fn apply_mark(
        &self,
        id: &str,
        range_start: i64,
        range_end: i64,
        mark_json: String,
    ) -> Result<OperationResult>;

    /// Remove the inline mark identified by `key` over `[range_start,
    /// range_end)`. Existing same-key spans that overlap the range are
    /// split or shortened; disjoint portions remain.
    #[holon_macros::affects("marks")]
    async fn remove_mark(
        &self,
        id: &str,
        range_start: i64,
        range_end: i64,
        key: String,
    ) -> Result<OperationResult>;
}

/// Task management operations (for any task-like entity)
///
/// This trait provides operations for managing task properties like completion,
/// priority, and due dates. It requires that the entity type implements
/// `TaskEntity`
#[holon_macros::operations_trait]
#[async_trait]
pub trait TaskOperations<T>: MaybeSendSync
where
    T: TaskEntity + MaybeSendSync + 'static,
{
    /// Set task title
    #[holon_macros::affects("title")]
    #[holon_macros::triggered_by(availability_of = "title")]
    async fn set_title(&self, id: &str, title: &str) -> Result<OperationResult>;

    /// Returns the valid states for this task type with progress information
    ///
    /// Examples:
    /// - Todoist: `[{state: "active", progress: 0.0, is_done: false, is_active:
    ///   true}, ...]`
    /// - Org Mode: `[{state: "TODO", progress: 0.0, ...}, {state: "DOING",
    ///   progress: 50.0, ...}, ...]`
    fn completion_states_with_progress(&self) -> Vec<CompletionStateInfo>;

    /// Set task state (e.g., "completed", "TODO", "DOING", "DONE", "WAITING")
    #[holon_macros::affects("task_state")]
    #[holon_macros::triggered_by(availability_of = "task_state")]
    #[holon_macros::enum_from(method = "completion_states_with_progress", param = "task_state")]
    async fn set_state(&self, id: &str, task_state: String) -> Result<OperationResult>;

    /// Cycle to the next task state. "" → TODO → DOING → DONE → "".
    #[holon_macros::affects("task_state")]
    async fn cycle_task_state(&self, id: &str) -> Result<OperationResult>;

    /// Set task priority (1=highest, 4=lowest)
    #[holon_macros::affects("priority")]
    #[holon_macros::triggered_by(availability_of = "priority")]
    async fn set_priority(&self, id: &str, priority: i64) -> Result<OperationResult>;

    /// Set task due date
    #[holon_macros::affects("due_date")]
    async fn set_due_date(
        &self,
        id: &str,
        due_date: Option<DateTime<Utc>>,
    ) -> Result<OperationResult>;
}

// Types that need BlockDataSourceHelpers and BlockOperations must opt in
// explicitly. Example:
//   impl BlockDataSourceHelpers<MyBlock> for MyDataSource {}
//   impl BlockOperations<MyBlock> for MyDataSource {}

/// Operations on the operation log for undo/redo functionality.
///
/// This trait provides methods for:
/// - Logging new operations with their inverses
/// - Marking operations as undone/redone
/// - Trimming old operations
///
/// Undo/redo candidates are retrieved via PRQL queries, not through this trait.
/// Implementors interact with the persistent `operations` table.
#[async_trait]
pub trait OperationLogOperations: MaybeSendSync {
    /// Log a new operation with its inverse.
    ///
    /// Inserts the operation into the log and trims old entries if needed.
    /// Returns the assigned log entry ID.
    async fn log_operation(&self, operation: Operation, inverse: UndoAction) -> Result<i64>;

    /// Mark an operation as undone.
    async fn mark_undone(&self, id: i64) -> Result<()>;

    /// Mark an operation as redone (restore to normal status).
    async fn mark_redone(&self, id: i64) -> Result<()>;

    /// Clear the redo stack (mark all undone operations as cancelled).
    ///
    /// Called when a new operation is executed to invalidate the redo history.
    async fn clear_redo_stack(&self) -> Result<()>;

    /// Get the maximum number of operations to retain.
    fn max_log_size(&self) -> usize {
        100
    }
}

// =============================================================================
// Block trait implementations for holon_api::Block
// =============================================================================

impl BlockEntity for holon_api::block::Block {
    fn id(&self) -> &EntityUri {
        &self.id
    }

    fn parent_id(&self) -> Option<&EntityUri> {
        // Return the full URI (`block:UUID`) — `BlockOperations` default
        // impls feed this back into `DataSource::get_by_id`, and the SQL
        // `block.id` column stores the prefixed form (per
        // `EntityUri`'s `Value::String` round-trip in
        // `crates/holon-api/src/entity_uri.rs`). Returning the bare path
        // (via `as_block_id().id()`) silently misses every parent →
        // "Parent not found" for non-root outdent / move_block.
        // Non-block parents (doc URIs, sentinel) → `None`, which the
        // trait reads as "no parent block" and errors with "Cannot
        // outdent root block" — that's the right behavior for headings
        // directly under a document.
        self.parent_id.is_block().then_some(&self.parent_id)
    }

    fn depth(&self) -> i64 {
        // Depth not stored in flattened entity - would need to compute from hierarchy
        0
    }

    fn content(&self) -> &str {
        &self.content
    }

    fn tags(&self) -> Tags {
        self.tags.clone()
    }
}

impl TaskEntity for holon_api::block::Block {
    fn completed(&self) -> bool {
        if let Some(state) = self.get_property_str("task_state") {
            return holon_api::TaskState::from_keyword(&state).is_done();
        }
        false
    }

    fn priority(&self) -> Option<i64> {
        let props = self.properties_map();
        if let Some(priority_val) = props.get("PRIORITY") {
            if let Some(i) = priority_val.as_i64() {
                return Some(i);
            }
            if let Some(s) = priority_val.as_string() {
                return Some(
                    holon_api::Priority::from_letter(s)
                        .unwrap_or_else(|e| {
                            panic!("stored PRIORITY property {s:?} is not a valid priority: {e}")
                        })
                        .to_int() as i64,
                );
            }
        }
        None
    }

    fn due_date(&self) -> Option<DateTime<Utc>> {
        if let Some(deadline_str) = self.get_property_str("DEADLINE") {
            let ts = holon_api::types::Timestamp::parse(&deadline_str).unwrap_or_else(|e| {
                panic!("stored DEADLINE property {deadline_str:?} is not a valid timestamp: {e}")
            });
            Some(ts.date().and_hms_opt(0, 0, 0).unwrap().and_utc())
        } else {
            None
        }
    }
}

impl OperationRegistry for holon_api::block::Block {
    fn all_operations() -> Vec<OperationDescriptor> {
        vec![]
    }

    fn entity_name() -> &'static str {
        "block"
    }

    fn short_name() -> Option<&'static str> {
        Some("block")
    }
}

/// Observer for operation execution events
///
/// Observers are notified after an operation is successfully executed.
/// This enables cross-cutting concerns like:
/// - Operation logging for undo/redo
/// - Audit trails
/// - Analytics
/// - Sync queue management
///
/// Unlike OperationProvider (which executes operations), observers only
/// observe the results. They cannot modify or veto operations.
///
/// # Entity Filter
/// Observers specify which entities they're interested in via
/// `entity_filter()`:
/// - Return `"*"` to observe all operations (e.g., operation log, audit)
/// - Return a specific entity name to observe only that entity
#[async_trait]
/// flutter_rust_bridge:ignore
pub trait OperationObserver: Send + Sync {
    /// Entity filter for this observer
    ///
    /// Returns `"*"` to observe all entities, or a specific entity name.
    fn entity_filter(&self) -> &str;

    /// Called after an operation is successfully executed
    ///
    /// # Arguments
    /// * `operation` - The operation that was executed
    /// * `undo_action` - The undo action returned by the operation (may be
    ///   Irreversible)
    ///
    /// # Note
    /// This is called only for successful operations. Failed operations are not
    /// observed. Observers should not perform operations that could fail
    /// and block the main flow.
    async fn on_operation_executed(&self, operation: &Operation, undo_action: &UndoAction);
}

/// Trait for persisting and loading sync tokens
///
/// SyncableProviders use this trait to persist their sync tokens across app
/// restarts. Implementations typically store tokens in a database or file
/// system. Trait for storing sync tokens for external providers.
///
/// This trait is used internally for dependency injection and should not be
/// exposed to FFI. flutter_rust_bridge:ignore
#[async_trait]
pub trait SyncTokenStore: Send + Sync {
    /// Load sync token for a provider
    ///
    /// Returns None if no token exists (first sync).
    async fn load_token(&self, provider_name: &str) -> Result<Option<StreamPosition>>;

    /// Save sync token for a provider
    async fn save_token(&self, provider_name: &str, position: StreamPosition) -> Result<()>;

    /// Clear all sync tokens
    ///
    /// Used for full sync operations where all providers need to start from the
    /// beginning.
    async fn clear_all_tokens(&self) -> Result<()>;
}

/// Type-independent sync trait for providers
///
/// Providers that can sync from external systems implement this trait.
/// Sync operations are generated dynamically when providers are registered,
/// using the format "{provider_name}.sync" (e.g., "todoist.sync", "jira.sync").
///
/// SyncableProviders should:
/// - Load current token using SyncTokenStore before syncing
/// - Perform sync operation
/// - Save new token using SyncTokenStore after syncing
/// - Return the new token
#[async_trait]
pub trait SyncableProvider: Send + Sync {
    /// Get the provider name (e.g., "todoist", "jira")
    ///
    /// This name is used to generate sync operations and identify the provider.
    fn provider_name(&self) -> &str;

    /// Sync data from the external system
    ///
    /// This method should:
    /// - Load current token using SyncTokenStore
    /// - Fetch updates from the external system using the stream position
    /// - Emit changes via streams (if applicable)
    /// - Save new token using SyncTokenStore
    /// - Return the new stream position
    ///
    /// # Arguments
    /// * `position` - Current stream position (StreamPosition::Beginning for
    ///   full sync, StreamPosition::Version(token) for incremental sync)
    ///
    /// # Returns
    /// The new stream position (typically StreamPosition::Version with new
    /// token, or StreamPosition::Beginning if no token)
    async fn sync(&self, position: StreamPosition) -> Result<StreamPosition>;

    /// Sync pending changes after operation execution.
    ///
    /// Default implementation performs a full sync.
    /// Override for targeted sync (e.g., OrgMode syncs only affected files).
    ///
    /// # Arguments
    /// * `changes` - Field-level changes from the operation
    async fn sync_changes(&self, _: &[FieldDelta]) -> Result<()> {
        self.sync(StreamPosition::Beginning).await?;
        Ok(())
    }
}

/// Trait for external sync providers that emit typed change streams
///
/// This trait allows QueryableCache to register and consume change streams
/// from external systems (Todoist, etc.) in a type-safe way.
/// ExternalServiceDiscovery
pub trait StreamProvider<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Get a receiver for changes of type T
    ///
    /// Returns a broadcast receiver that emits batches of changes.
    /// Multiple QueryableCache instances can subscribe to the same stream.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<Change<T>>>;
}

/// Generate a sync operation descriptor for a provider
///
/// This is used by OperationDispatcher when registering SyncableProviders
/// to create operation descriptors with the correct entity_name format.
pub fn generate_sync_operation(provider_name: &str) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: format!("{}.sync", provider_name).into(),
        entity_short_name: "all".to_string(), // Sync operations affect all entities
        id_column: String::new(),             // Sync operations don't need an ID column
        name: "sync".to_string(),
        display_name: format!("Sync {}", provider_name),
        description: format!("Sync data from {} provider", provider_name),
        required_params: vec![],
        affected_fields: vec![], // Sync operations don't affect specific fields
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Block,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::External,
        },
        trigger: None,
        bound_params: std::collections::HashMap::new(),
        precondition: None,
    }
}

/// Hook called after an FDW cache table is primed with data.
/// Implementations can subscribe to resource notifications, update state, etc.
/// Trait-shaped and storage-agnostic (string table/query identifiers); the
/// Turso matview manager consumes it, providers (e.g. holon-mcp-client)
/// implement it without naming the backend.
#[async_trait]
pub trait MatviewHook: Send + Sync {
    /// Called after a successful FDW prime query. `cache_table` is the primed
    /// table (e.g. `"cc_message"`), `fdw_sql` is the executed query
    /// including WHERE clause.
    async fn on_fdw_primed(&self, cache_table: &str, fdw_sql: &str);
}

#[cfg(test)]
mod trait_unit_tests {
    use holon_api::block::Block;

    use super::*;

    #[test]
    fn event_origin_round_trips_through_str() {
        for origin in [
            EventOrigin::Loro,
            EventOrigin::Org,
            EventOrigin::Ui,
            EventOrigin::Other("sql".to_string()),
        ] {
            assert_eq!(EventOrigin::parse_str(origin.as_str()), origin);
        }
        assert_eq!(EventOrigin::Loro.as_str(), "loro");
        assert_eq!(EventOrigin::Org.as_str(), "org");
        assert_eq!(EventOrigin::Ui.as_str(), "ui");
    }

    #[test]
    fn undo_action_semantics() {
        let op = Operation::new("test", "op", "op", std::collections::HashMap::new());
        let undo = UndoAction::from(op);
        assert!(undo.is_reversible());
        let inner = undo.into_option().expect("Undo(op) must yield Some(op)");
        assert_eq!(inner.op_name, "op");

        assert!(!UndoAction::DeclaredIrreversible("x").is_reversible());
        assert!(
            UndoAction::DeclaredIrreversible("x")
                .into_option()
                .is_none()
        );
        assert!(UndoAction::Undeclared.is_undeclared());
    }

    #[test]
    fn unknown_operation_error_detection_and_display() {
        let err = UnknownOperationError::new("BlockOperations", "frobnicate");
        assert!(UnknownOperationError::is_unknown(&err));
        let msg = err.to_string();
        assert!(
            msg.contains("frobnicate"),
            "display must name the operation: {msg}"
        );
        assert!(
            msg.contains("BlockOperations"),
            "display must name the trait: {msg}"
        );

        let other = std::io::Error::other("boom");
        assert!(!UnknownOperationError::is_unknown(&other));
    }

    fn test_block() -> Block {
        Block::new_text(
            EntityUri::block("11111111-1111-1111-1111-111111111111"),
            EntityUri::block("22222222-2222-2222-2222-222222222222"),
            "hello world",
        )
    }

    #[test]
    fn block_entity_view_maps_id_parent_content_tags() {
        let mut block = test_block();
        block.tags.insert("foo");

        assert_eq!(BlockEntity::id(&block), &block.id);
        assert_eq!(
            BlockEntity::parent_id(&block),
            Some(&EntityUri::block("22222222-2222-2222-2222-222222222222")),
            "block-scheme parent must surface as the full URI"
        );
        assert_eq!(BlockEntity::content(&block), "hello world");
        assert!(BlockEntity::tags(&block).contains("foo"));
        assert_eq!(
            BlockEntity::depth(&block),
            0,
            "flattened entity depth is always 0"
        );
        assert!(!block.is_page());

        let mut page = test_block();
        page.set_page(true);
        assert!(
            BlockEntity::is_page(&page),
            "default is_page must derive from the Page tag"
        );

        // Non-block parents (doc URIs) must read as "no parent block".
        let mut doc_child = test_block();
        // ALLOW(entity_uri_from_raw): constructing a doc-scheme parent fixture.
        doc_child.parent_id = EntityUri::from_raw("doc:some-file");
        assert_eq!(BlockEntity::parent_id(&doc_child), None);
    }

    #[test]
    fn task_entity_view_maps_state_priority_due_date() {
        let mut done = test_block();
        done.set_property("task_state", "DONE");
        assert!(TaskEntity::completed(&done));

        let mut todo = test_block();
        todo.set_property("task_state", "TODO");
        assert!(!TaskEntity::completed(&todo));
        assert!(!TaskEntity::completed(&test_block()));

        let mut prioritized = test_block();
        prioritized.set_property("PRIORITY", 2i64);
        assert_eq!(TaskEntity::priority(&prioritized), Some(2));
        assert_eq!(TaskEntity::priority(&test_block()), None);

        let mut due = test_block();
        due.set_property("DEADLINE", "<2026-07-04 Sat>");
        let date = TaskEntity::due_date(&due).expect("DEADLINE must yield a due date");
        assert_eq!(date.date_naive().to_string(), "2026-07-04");
        assert!(TaskEntity::due_date(&test_block()).is_none());
    }

    #[test]
    fn block_operation_registry_metadata() {
        assert_eq!(<Block as OperationRegistry>::entity_name(), "block");
        assert_eq!(<Block as OperationRegistry>::short_name(), Some("block"));
        assert!(<Block as OperationRegistry>::all_operations().is_empty());
    }
}
