//! Core datasource traits
//!
//! This module provides traits for datasource operations.
//! These traits are designed to work with external datasources that provide
//! both read and write capabilities.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::fractional_index::{gen_key_between, gen_n_keys, MAX_SORT_KEY_LENGTH};
use holon_api::{Operation, OperationDescriptor, Value};

// Define Result type using Send + Sync for error
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

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

/// Represents the undo capability of an operation.
///
/// Operations return this type to indicate whether they can be undone
/// and if so, what operation would undo them.
#[derive(Debug, Clone)]
pub enum UndoAction {
    /// The operation can be undone by executing the contained inverse operation.
    Undo(Operation),
    /// The operation cannot be undone (e.g., complex operations like split_block).
    Irreversible,
}

impl UndoAction {
    /// Convert to Option<Operation> for backward compatibility
    pub fn into_option(self) -> Option<Operation> {
        match self {
            UndoAction::Undo(op) => Some(op),
            UndoAction::Irreversible => None,
        }
    }

    /// Check if this action is reversible
    pub fn is_reversible(&self) -> bool {
        matches!(self, UndoAction::Undo(_))
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
            None => UndoAction::Irreversible,
        }
    }
}

/// Represents a single field change with old and new values.
/// Used for change propagation (cache/sync), NOT for undo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDelta {
    pub entity_id: String,
    pub field: String,
    pub old_value: Value,
    pub new_value: Value,
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
        }
    }
}

/// Result of an operation, containing changes for propagation and undo action.
///
/// - `changes`: Field-level changes for propagation to cache/sync systems
/// - `undo`: Semantic undo operation (same code path as forward)
/// - `follow_ups`: Operations to execute after this one completes (e.g., cursor update after split)
#[derive(Debug, Clone)]
pub struct OperationResult {
    pub changes: Vec<FieldDelta>,
    pub undo: UndoAction,
    /// Optional response payload from the operation (e.g. MCP tool call results).
    /// Non-MCP providers return `None`.
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

    /// Create an irreversible operation result
    pub fn irreversible(changes: Vec<FieldDelta>) -> Self {
        Self {
            changes,
            undo: UndoAction::Irreversible,
            response: None,
            follow_ups: vec![],
        }
    }

    /// Backward compatibility during migration
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

/// Error raised when a trait's dispatch helper does not recognize an operation name.
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
    fn id(&self) -> &str;

    fn parent_id(&self) -> Option<&str>;
    fn sort_key(&self) -> &str;
    fn depth(&self) -> i64;

    /// Get the block content (text content of the block)
    fn content(&self) -> &str;

    /// Tags attached to this block. The literal `"Page"` tag marks the
    /// block as a page (org file root).
    fn tags(&self) -> &[String];

    /// Whether this block is a page (its `tags` contains `"Page"`).
    fn is_page(&self) -> bool {
        self.tags().iter().any(|t| t == holon_api::PAGE_TAG)
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
///
/// **Note**: This trait is conceptually `CrudOperations` but is named
/// `CrudOperations` for backward compatibility with macro-generated code.
/// New code should refer to it as `CrudOperations` in documentation.
#[holon_macros::operations_trait]
#[async_trait]
pub trait CrudOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Set single field (returns changes and inverse operation for undo)
    /// Note: affected_fields is determined dynamically based on the field parameter
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult>;

    /// Create new entity (returns new ID, changes, and inverse operation for undo)
    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)>;

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

    /// Returns the entity name for this registry (e.g., "todoist_task", "block")
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
    async fn get_children(&self, parent_id: &str) -> Result<Vec<T>>
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
    /// over `get_children()`. Implementations may override with a recursive CTE.
    async fn get_descendants(&self, parent_id: &str) -> Result<Vec<T>>
    where
        T: BlockEntity,
    {
        let mut result = Vec::new();
        let mut queue = vec![parent_id.to_string()];
        while let Some(pid) = queue.pop() {
            let children = self.get_children(&pid).await?;
            for child in children {
                queue.push(child.id().to_string());
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
    /// Get all siblings of a block, sorted by sort_key
    async fn get_siblings(&self, block_id: &str) -> Result<Vec<T>> {
        let block: T = self
            .get_by_id(block_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_id = block.parent_id();

        let siblings: Vec<T> = if let Some(pid) = parent_id {
            self.get_children(pid).await?
        } else {
            return Ok(vec![]);
        };

        Ok(siblings
            .into_iter()
            .filter(|s: &T| s.id() != block_id)
            .collect())
    }

    /// Get the previous sibling (sibling with sort_key < current sort_key)
    async fn get_prev_sibling(&self, block_id: &str) -> Result<Option<T>> {
        let block: T = self
            .get_by_id(block_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_id = block.parent_id();

        let siblings: Vec<T> = if let Some(pid) = parent_id {
            self.get_children(pid).await?
        } else {
            return Ok(None);
        };

        let prev = siblings
            .into_iter()
            .filter(|s: &T| s.sort_key() < block.sort_key())
            .max_by(|a, b| a.sort_key().cmp(b.sort_key()));
        Ok(prev)
    }

    /// Get the next sibling (sibling with sort_key > current sort_key)
    async fn get_next_sibling(&self, block_id: &str) -> Result<Option<T>> {
        let block: T = self
            .get_by_id(block_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_id = block.parent_id();

        let siblings: Vec<T> = if let Some(pid) = parent_id {
            self.get_children(pid).await?
        } else {
            return Ok(None);
        };

        let next = siblings
            .into_iter()
            .filter(|s: &T| s.sort_key() > block.sort_key())
            .min_by(|a: &T, b: &T| a.sort_key().cmp(b.sort_key()));
        Ok(next)
    }

    /// Get the first child of a parent (lowest sort_key)
    async fn get_first_child(&self, parent_id: Option<&str>) -> Result<Option<T>> {
        let children: Vec<T> = if let Some(pid) = parent_id {
            self.get_children(pid).await?
        } else {
            return Ok(None);
        };

        Ok(children
            .into_iter()
            .min_by(|a, b| a.sort_key().cmp(b.sort_key())))
    }

    /// Get the last child of a parent (highest sort_key)
    async fn get_last_child(&self, parent_id: Option<&str>) -> Result<Option<T>> {
        let children: Vec<T> = if let Some(pid) = parent_id {
            self.get_children(pid).await?
        } else {
            return Ok(None);
        };

        Ok(children
            .into_iter()
            .max_by(|a: &T, b: &T| a.sort_key().cmp(b.sort_key())))
    }
}

/// Mutating maintenance operations on block hierarchies (depth updates, rebalancing)
#[async_trait]
pub trait BlockMaintenanceHelpers<T>: CrudOperations<T> + DataSource<T>
where
    T: BlockEntity + MaybeSendSync + 'static,
{
    /// Recursively update depths of all descendants when a parent's depth changes
    async fn update_descendant_depths(&self, parent_id: &str, depth_delta: i64) -> Result<()> {
        if depth_delta == 0 {
            return Ok(());
        }

        let mut queue = vec![parent_id.to_string()];

        while let Some(current_parent_id) = queue.pop() {
            let children: Vec<T> = self.get_children(&current_parent_id).await?;

            for child in children {
                let current_depth = child.depth();
                let new_depth = current_depth + depth_delta;
                self.set_field(child.id(), "depth", Value::Integer(new_depth))
                    .await?;
                queue.push(child.id().to_string());
            }
        }

        Ok(())
    }

    /// Rebalance all siblings of a parent to create uniform spacing
    async fn rebalance_siblings(&self, parent_id: Option<&str>) -> Result<()> {
        let children: Vec<T> = if let Some(pid) = parent_id {
            self.get_children(pid).await?
        } else {
            return Ok(());
        };

        let mut sorted_children: Vec<_> = children.into_iter().collect();
        sorted_children.sort_by(|a, b| a.sort_key().cmp(b.sort_key()));

        let new_keys = gen_n_keys(sorted_children.len())?;

        for (child, new_key) in sorted_children.iter().zip(new_keys.iter()) {
            self.set_field(child.id(), "sort_key", Value::String(new_key.clone()))
                .await?;
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
    async fn indent(&self, id: &str) -> Result<OperationResult> {
        let block = self
            .get_by_id(id)
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
        let new_parent_id = prev_sibling.id().to_string();

        // Indent semantics: the indented block becomes the LAST child of the
        // previous sibling. `move_block` interprets `after_block_id = None`
        // as "insert at the beginning", so we look up the new parent's
        // current last child and pass its id as the anchor.
        let new_parent_children: Vec<T> = self.get_children(&new_parent_id).await?;
        let after_id_owned = new_parent_children.last().map(|c| c.id().to_string());

        self.move_block(id, &new_parent_id, after_id_owned.as_deref())
            .await
    }

    /// Move block to different position (reorder within same parent or different parent)
    ///
    /// # Parameters
    /// * `id` - Block ID to move
    /// * `parent_id` - Target parent ID (must always have a parent)
    /// * `after_block_id` - Optional anchor block (move after this block, or beginning if None)
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    #[holon_macros::triggered_by(availability_of = "tree_position", providing = ["parent_id", "after_block_id"])]
    #[holon_macros::triggered_by(availability_of = "selected_id", providing = ["parent_id"])]
    async fn move_block(
        &self,
        id: &str,
        parent_id: &str,
        after_block_id: Option<&str>,
    ) -> Result<OperationResult> {
        // Capture old state before mutation
        let maybe_block: Option<T> = self.get_by_id(id).await?;
        let block: T = maybe_block.ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let old_parent_id = block
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot move root block"))?
            .to_string();
        let old_predecessor = self.get_prev_sibling(id).await?;
        let old_depth = block.depth();

        // Query predecessor and successor sort_keys
        let (prev_key, next_key): (Option<String>, Option<String>) = if after_block_id.is_none() {
            // No after_block_id means "move to beginning" - insert before first child
            let first_child: Option<T> = self.get_first_child(Some(parent_id)).await?;
            let first_key = first_child.map(|c| c.sort_key().to_string());
            (None, first_key)
        } else {
            // Insert after specific block
            let maybe_after_block: Option<T> = self.get_by_id(after_block_id.unwrap()).await?;
            let after_block: T =
                maybe_after_block.ok_or_else(|| anyhow::anyhow!("Reference block not found"))?;
            let prev_key = Some(after_block.sort_key().to_string());

            // Find next sibling after the anchor block
            let next_sibling: Option<T> = self.get_next_sibling(after_block_id.unwrap()).await?;
            let next_key: Option<String> = next_sibling.map(|s: T| s.sort_key().to_string());
            (prev_key, next_key)
        };

        // Generate new sort_key
        let mut new_sort_key = gen_key_between(prev_key.as_deref(), next_key.as_deref())
            .map_err(|e| anyhow::anyhow!(e))?;

        // Check if rebalancing needed
        if new_sort_key.len() > MAX_SORT_KEY_LENGTH {
            self.rebalance_siblings(Some(parent_id)).await?;

            // Re-query neighbors after rebalancing
            let (prev_key, next_key): (Option<String>, Option<String>) = if after_block_id.is_none()
            {
                let first_child: Option<T> = self.get_first_child(Some(parent_id)).await?;
                let first_key = first_child.map(|c| c.sort_key().to_string());
                (None, first_key)
            } else {
                let maybe_after_block: Option<T> = self.get_by_id(after_block_id.unwrap()).await?;
                let after_block: T = maybe_after_block
                    .ok_or_else(|| anyhow::anyhow!("Reference block not found"))?;
                let prev_key = Some(after_block.sort_key().to_string());
                let next_sibling: Option<T> =
                    self.get_next_sibling(after_block_id.unwrap()).await?;
                let next_key: Option<String> = next_sibling.map(|s: T| s.sort_key().to_string());
                (prev_key, next_key)
            };

            new_sort_key = gen_key_between(prev_key.as_deref(), next_key.as_deref())
                .map_err(|e| anyhow::anyhow!(e))?;
        }

        // Calculate new depth based on parent
        let maybe_parent: Option<T> = self.get_by_id(parent_id).await?;
        let parent: T = maybe_parent.ok_or_else(|| anyhow::anyhow!("Parent not found"))?;
        let new_depth = parent.depth() + 1;

        // Calculate depth delta for recursive updates
        let depth_delta = new_depth - old_depth;

        // Update block atomically and collect FieldDeltas
        let mut changes = Vec::new();
        let parent_id_result = self
            .set_field(id, "parent_id", Value::String(parent_id.to_string()))
            .await?;
        changes.extend(parent_id_result.changes);
        let sort_key_result = self
            .set_field(id, "sort_key", Value::String(new_sort_key))
            .await?;
        changes.extend(sort_key_result.changes);
        let depth_result = self
            .set_field(id, "depth", Value::Integer(new_depth))
            .await?;
        changes.extend(depth_result.changes);

        // Recursively update all descendants' depths by the same delta
        // Note: update_descendant_depths calls set_field internally, so it will also return FieldDeltas
        // For now, we'll skip collecting those to avoid complexity
        if depth_delta != 0 {
            self.update_descendant_depths(id, depth_delta).await?;
        }

        // Return inverse operation using macro-generated helper
        use crate::__operations_block_operations;

        // Entity name will be set by OperationProvider when operation is executed
        Ok(OperationResult::new(
            changes,
            __operations_block_operations::move_block_op(
                "placeholder", // OperationDispatcher overwrites this with the resolved entity_name (see operation_dispatcher.rs:504). EntityName::new debug-asserts on empty/invalid scheme, so we use a valid placeholder.
                id,
                &old_parent_id,
                old_predecessor.as_ref().map(|p| p.id()),
            ),
        ))
    }

    /// Move block out to parent's level (decrease indentation)
    #[holon_macros::affects("parent_id", "depth", "sort_key")]
    async fn outdent(&self, id: &str) -> Result<OperationResult> {
        let maybe_block: Option<T> = self.get_by_id(id).await?;
        let block: T = maybe_block.ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_id = block
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot outdent root block"))?;

        let maybe_parent: Option<T> = self.get_by_id(parent_id).await?;
        let parent: T = maybe_parent.ok_or_else(|| anyhow::anyhow!("Parent not found"))?;
        let grandparent_id = parent
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot outdent: parent is already at root level"))?;

        // Capture old predecessor before move (for inverse operation)
        let old_parent_id = parent_id.to_string();
        let old_predecessor = self.get_prev_sibling(id).await?;

        // Move to grandparent's children, after parent
        let move_result = self.move_block(id, grandparent_id, Some(parent_id)).await?;

        // Return inverse: move_block back to old parent after old predecessor.
        // We can't use indent_op here because indent now resolves the previous sibling
        // dynamically, which wouldn't restore the exact original position.
        use crate::__operations_block_operations;

        // Entity name will be set by OperationProvider when operation is executed
        Ok(OperationResult::new(
            move_result.changes,
            __operations_block_operations::move_block_op(
                "placeholder", // OperationDispatcher overwrites this with the resolved entity_name (see operation_dispatcher.rs:504). EntityName::new debug-asserts on empty/invalid scheme, so we use a valid placeholder.
                id,
                &old_parent_id,
                old_predecessor.as_ref().map(|p| p.id()),
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
    /// * `position` - Character position to split at (as i64, will be converted to usize)
    #[holon_macros::affects("content")]
    async fn split_block(&self, id: &str, position: i64) -> Result<OperationResult> {
        use uuid::Uuid;

        let maybe_block: Option<T> = self.get_by_id(id).await?;
        let block: T = maybe_block.ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        let content = block.content();

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
        let new_block_id = format!("block:{}", Uuid::new_v4());

        // Get next sibling's sort_key to position new block correctly
        let next_sibling: Option<T> = self.get_next_sibling(id).await?;
        let next_sort_key: Option<String> = next_sibling.map(|s: T| s.sort_key().to_string());

        // Generate sort_key for new block (between current block and next sibling)
        let new_sort_key = gen_key_between(Some(block.sort_key()), next_sort_key.as_deref())
            .map_err(|e| anyhow::anyhow!(e))?;

        // Get current timestamp
        let now = chrono::Utc::now().timestamp_millis();

        // Create new block using create method
        let mut new_block_fields = HashMap::new();
        new_block_fields.insert("id".to_string(), Value::String(new_block_id.clone()));
        new_block_fields.insert("content".to_string(), Value::String(content_after));
        new_block_fields.insert("parent_id".to_string(), {
            if let Some(ref pid) = block.parent_id() {
                Value::String(pid.to_string())
            } else {
                Value::Null
            }
        });
        new_block_fields.insert("depth".to_string(), Value::Integer(block.depth()));
        new_block_fields.insert("sort_key".to_string(), Value::String(new_sort_key));
        new_block_fields.insert("created_at".to_string(), Value::Integer(now));
        new_block_fields.insert("updated_at".to_string(), Value::Integer(now));
        new_block_fields.insert("collapsed".to_string(), Value::Boolean(false));
        new_block_fields.insert("completed".to_string(), Value::Boolean(false));
        new_block_fields.insert("block_type".to_string(), Value::String("text".to_string()));

        let (_new_block_id, create_result) = self.create(new_block_fields).await?;
        let mut changes = create_result.changes;

        // Update current block with truncated content
        let content_result = self
            .set_field(id, "content", Value::String(content_before))
            .await?;
        changes.extend(content_result.changes);
        // TODO: Do we need this? self.set_field(id, "updated_at", Value::Integer(now))
        // TODO: Do we need this?     .await?;

        // Follow-up: move editor cursor to the new block at position 0.
        // The editor_focus_op is dispatched as a follow-up operation, where
        // the dispatcher resolves the entity_name from the operation's
        // explicit `entity_name` field — the placeholder we pass here gets
        // overwritten before the descriptor's `EntityName` is consulted.
        // We must still pass a valid URI scheme because `EntityName::new`
        // debug-asserts (`is_valid_uri_scheme`) immediately on construction.
        use crate::__operations_editor_cursor_operations;
        let editor_focus = __operations_editor_cursor_operations::editor_focus_op(
            "navigation",
            "main",
            &new_block_id,
            0,
        );

        // TODO: Return inverse operation (combine set_field inverses + delete for new block)
        Ok(OperationResult::irreversible(changes)
            .with_response(Value::String(new_block_id))
            .with_follow_ups(vec![editor_focus]))
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
    ///   `Ok` with no changes). Real frontends only dispatch this op when
    ///   the cursor is at byte 0, but the SQL caller path may pass through
    ///   stale positions, so we re-check here.
    #[holon_macros::affects("content", "parent_id", "sort_key")]
    async fn join_block(&self, id: &str, position: i64) -> Result<OperationResult> {
        if position != 0 {
            return Ok(OperationResult::irreversible(vec![]));
        }

        let block: T = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let block_content = block.content().to_string();
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
            self.get_by_id(parent_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Cannot join: parent {parent_id} not found"))?
        };

        let target_content = target.content().to_string();
        let join_offset = target_content.len();
        let new_content = format!("{}{}", target_content, block_content);
        let target_id = target.id().to_string();

        let mut changes = Vec::new();

        // Re-parent children of `id` under `target_id`. Placement differs:
        //   - prev-sibling path: append after target's existing children
        //   - parent path: place at `id`'s old slot (before the remaining
        //     siblings of `id` under `target`).
        let block_children: Vec<T> = self.get_children(&block_id_str).await?;
        if into_parent {
            // Find the immediate next sibling of `id` under `target` (i.e.
            // `id`'s former following sibling). Block's children's new
            // sort_keys must lex-sort below that sibling's sort_key so they
            // occupy `id`'s old slot. If there is no following sibling,
            // pass `None` for the upper bound (sort to end).
            let target_children_now: Vec<T> = self.get_children(&target_id).await?;
            let next_sibling_sort_key: Option<String> = target_children_now
                .iter()
                .filter(|c| c.id() != block_id_str)
                .find(|c| c.sort_key() > block.sort_key())
                .map(|c| c.sort_key().to_string());
            let mut last_sort_key: Option<String> = None;
            for child in block_children.iter() {
                let new_sort_key =
                    gen_key_between(last_sort_key.as_deref(), next_sibling_sort_key.as_deref())
                        .map_err(|e| anyhow::anyhow!(e))?;
                let pid_result = self
                    .set_field(child.id(), "parent_id", Value::String(target_id.clone()))
                    .await?;
                changes.extend(pid_result.changes);
                let sort_result = self
                    .set_field(child.id(), "sort_key", Value::String(new_sort_key.clone()))
                    .await?;
                changes.extend(sort_result.changes);
                last_sort_key = Some(new_sort_key);
            }
        } else {
            // Prev-sibling path: append block's children after target's
            // existing children. Same logic as the original implementation.
            let target_children: Vec<T> = self.get_children(&target_id).await?;
            let mut last_sort_key: Option<String> =
                target_children.last().map(|c| c.sort_key().to_string());
            for child in block_children.iter() {
                let new_sort_key = gen_key_between(last_sort_key.as_deref(), None)
                    .map_err(|e| anyhow::anyhow!(e))?;
                let pid_result = self
                    .set_field(child.id(), "parent_id", Value::String(target_id.clone()))
                    .await?;
                changes.extend(pid_result.changes);
                let sort_result = self
                    .set_field(child.id(), "sort_key", Value::String(new_sort_key.clone()))
                    .await?;
                changes.extend(sort_result.changes);
                last_sort_key = Some(new_sort_key);
            }
        }

        // Append `id`'s content to the merge target.
        let content_result = self
            .set_field(&target_id, "content", Value::String(new_content))
            .await?;
        changes.extend(content_result.changes);

        // Delete `id` (its children have already been re-parented).
        let delete_result = self.delete(&block_id_str).await?;
        changes.extend(delete_result.changes);

        // Follow-up: park the editor cursor on the merge target at the
        // join boundary, mirroring `split_block`'s editor_focus handoff.
        use crate::__operations_editor_cursor_operations;
        let editor_focus = __operations_editor_cursor_operations::editor_focus_op(
            "navigation",
            "main",
            &target_id,
            join_offset as i64,
        );

        Ok(OperationResult::irreversible(changes)
            .with_response(Value::String(target_id))
            .with_follow_ups(vec![editor_focus]))
    }

    /// Move a block up (swap with previous sibling)
    #[holon_macros::affects("parent_id", "sort_key")]
    async fn move_up(&self, id: &str) -> Result<OperationResult> {
        // Capture old state
        let block = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_id = block
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot move root block"))?
            .to_string();
        let old_predecessor = self.get_prev_sibling(id).await?;

        let prev_sibling: T = self
            .get_prev_sibling(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Cannot move up: no previous sibling"))?;

        // Get the sibling before prev_sibling
        let before_prev: Option<T> = self.get_prev_sibling(prev_sibling.id()).await?;

        // Execute move and collect FieldDeltas
        let move_result = if let Some(before_id) = before_prev {
            self.move_block(id, &parent_id, Some(before_id.id()))
                .await?
        } else {
            // Move to beginning
            self.move_block(id, &parent_id, None).await?
        };

        // Return inverse (move down - restore original position) using macro-generated helper
        // Use move_block_op to restore exact old position (move_up_op is relative, not absolute)
        use crate::__operations_block_operations;

        Ok(OperationResult::new(
            move_result.changes,
            __operations_block_operations::move_block_op(
                "placeholder", // OperationDispatcher overwrites this with the resolved entity_name (see operation_dispatcher.rs:504). EntityName::new debug-asserts on empty/invalid scheme, so we use a valid placeholder.
                id,
                &parent_id,
                old_predecessor.as_ref().map(|p| p.id()),
            ),
        ))
    }

    /// Embed another entity inline by inserting a transclusion marker into the content.
    ///
    /// The `target_uri` is an EntityUri string (e.g. `block:some-id`, `todoist-task:123`).
    /// Inserts `{{transclude:target_uri}}` at the end of the block's content.
    #[holon_macros::affects("content")]
    #[holon_macros::triggered_by(availability_of = "selected_id", providing = ["target_uri"])]
    async fn embed_entity(&self, id: &str, target_uri: &str) -> Result<OperationResult> {
        let block = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        let old_content = block.content().to_string();
        let marker = format!("{{{{transclude:{target_uri}}}}}");
        let new_content = if old_content.is_empty() {
            marker
        } else {
            format!("{old_content}\n{marker}")
        };

        let result = self
            .set_field(id, "content", Value::String(new_content))
            .await?;

        use crate::__operations_crud_operations;
        Ok(OperationResult::new(
            result.changes,
            __operations_crud_operations::set_field_op(
                "placeholder", // Overwritten by OperationDispatcher post-execute (see operation_dispatcher.rs:504)
                id,
                "content",
                Value::String(old_content),
            ),
        ))
    }

    /// Move a block down (swap with next sibling)
    #[holon_macros::affects("parent_id", "sort_key")]
    async fn move_down(&self, id: &str) -> Result<OperationResult> {
        // Capture old state
        let block = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
        let parent_id = block
            .parent_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot move root block"))?
            .to_string();
        let old_predecessor = self.get_prev_sibling(id).await?;

        let next_sibling: T = self
            .get_next_sibling(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Cannot move down: no next sibling"))?;

        // Execute move after next_sibling and collect FieldDeltas
        let move_result = self
            .move_block(id, &parent_id, Some(next_sibling.id()))
            .await?;

        // Return inverse (move up - restore original position) using macro-generated helper
        use crate::__operations_block_operations;

        Ok(OperationResult::new(
            move_result.changes,
            __operations_block_operations::move_block_op(
                "placeholder", // OperationDispatcher overwrites this with the resolved entity_name (see operation_dispatcher.rs:504). EntityName::new debug-asserts on empty/invalid scheme, so we use a valid placeholder.
                id,
                &parent_id,
                old_predecessor.as_ref().map(|p| p.id()),
            ),
        ))
    }

    /// Set whether this block is a page (org file root).
    ///
    /// Promoting (`is_document=true`) adds the literal tag `"Page"` to the
    /// block's `tags` list. Demoting removes it. The block's title is the
    /// first line of `content`, so no separate name field is written.
    #[holon_macros::affects("tags")]
    async fn set_is_document(&self, id: &str, is_document: bool) -> Result<OperationResult> {
        let block = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

        let old_value = block.is_page();
        let mut changes = Vec::new();

        let mut new_tags = block.tags().to_vec();
        let already = new_tags.iter().any(|t| t == holon_api::PAGE_TAG);
        if is_document && !already {
            new_tags.push(holon_api::PAGE_TAG.to_string());
        } else if !is_document && already {
            new_tags.retain(|t| t != holon_api::PAGE_TAG);
        } else {
            // No change required.
            use crate::__operations_block_operations;
            return Ok(OperationResult::new(
                Vec::new(),
                __operations_block_operations::set_is_document_op("placeholder", id, old_value),
            ));
        }

        let arr: Vec<Value> = new_tags.iter().map(|t| Value::String(t.clone())).collect();
        let tags_result = self.set_field(id, "tags", Value::Array(arr)).await?;
        changes.extend(tags_result.changes);

        use crate::__operations_block_operations;
        Ok(OperationResult::new(
            changes,
            __operations_block_operations::set_is_document_op("placeholder", id, old_value),
        ))
    }
}

/// Rename operations (for entities with a name field)
///
/// This trait provides a rename operation for entities that have a name or title
/// that can be changed.
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
    /// * `after_id` - Optional anchor entity (move after this entity, or beginning if None)
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
/// priority, and due dates. It requires that the entity type implements `TaskEntity`
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
    /// - Todoist: `[{state: "active", progress: 0.0, is_done: false, is_active: true}, ...]`
    /// - Org Mode: `[{state: "TODO", progress: 0.0, ...}, {state: "DOING", progress: 50.0, ...}, ...]`
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

// Types that need BlockDataSourceHelpers and BlockOperations must opt in explicitly.
// Example:
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
    fn id(&self) -> &str {
        self.id.as_str()
    }

    fn parent_id(&self) -> Option<&str> {
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
        if self.parent_id.is_block() {
            Some(self.parent_id.as_str())
        } else {
            None
        }
    }

    fn sort_key(&self) -> &str {
        // Read the actual fractional-index sort_key from the struct field.
        // The previous impl returned `self.id.as_str()` (the full URI like
        // `block:UUID`), which contains non-hex `:`/`-` characters and
        // panicked `gen_key_between` (`from_hex_string` calls
        // `u8::from_str_radix(.., 16).unwrap()`) inside any `move_block` /
        // `outdent` / `split_block` / `indent` flow.
        &self.sort_key
    }

    fn depth(&self) -> i64 {
        // Depth not stored in flattened entity - would need to compute from hierarchy
        0
    }

    fn content(&self) -> &str {
        &self.content
    }

    fn tags(&self) -> &[String] {
        &self.tags
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

// ── Editor cursor operations ────────────────────────────────────────────

/// Editor cursor operations for tracking focus state.
///
/// Generates `__operations_editor_cursor_operations::editor_focus_op()`
/// helper used by `split_block` to set cursor on the new block.
#[holon_macros::operations_trait]
#[async_trait]
pub trait EditorCursorOperations {
    /// Set editor cursor focus on a block at a given byte offset
    #[holon_macros::affects("block_id", "cursor_offset")]
    async fn editor_focus(
        &self,
        region: &str,
        block_id: &str,
        cursor_offset: i64,
    ) -> Result<OperationResult>;
}
