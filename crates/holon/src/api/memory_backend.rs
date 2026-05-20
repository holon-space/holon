//! In-memory implementation of DocumentRepository
//!
//! This module provides a simple HashMap-based implementation for testing
//! and as a reference implementation. It implements only `CoreOperations`
//! and `Lifecycle` traits (no networking, no change notifications).

use holon_api::streaming::ChangeSubscribers;

use super::event_ring::{DEFAULT_EVENT_RING_CAPACITY, EventRing, deliver_to_subscribers};
use super::repository::{CoreOperations, Lifecycle};
use super::types::NewBlock;
use async_trait::async_trait;
use holon_api::block_mutation::{BlockMutation, BlockTreeView};
use holon_api::streaming::ChangeNotifications;
use holon_api::{
    ApiError, Block, BlockChange, BlockContent, Change, ChangeOrigin, EntityUri, StreamPosition,
};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt, wrappers::ReceiverStream};
/// In-memory block storage using HashMaps.
///
/// This is a lightweight, non-persistent backend useful for:
/// - Unit testing without CRDT overhead
/// - Mocking in frontend development
/// - Reference implementation for documentation
/// - Property-based testing baseline
///
/// # Example
///
/// ```rust,no_run
/// use holon::api::{MemoryBackend, CoreOperations, Lifecycle};
///
/// async fn example() -> anyhow::Result<()> {
///     let backend = MemoryBackend::create_new("test-doc".to_string()).await?;
///
///     let block = backend.create_block(None, "Hello".to_string(), None).await?;
///     let retrieved = backend.get_block(&block.id).await?;
///
///     assert_eq!(retrieved.content, "Hello");
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct MemoryBackend {
    /// Document ID
    doc_id: String,
    /// Internal state
    state: Arc<RwLock<MemoryState>>,
}

impl Clone for MemoryBackend {
    fn clone(&self) -> Self {
        let state = self.state.read().unwrap();
        let cloned_state = MemoryState {
            blocks: state.blocks.clone(),
            deleted_ids: state.deleted_ids.clone(),
            children_by_parent: state.children_by_parent.clone(),
            next_id_counter: state.next_id_counter,
            subscribers: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            event_log: state.event_log.clone(),
        };

        Self {
            doc_id: self.doc_id.clone(),
            state: Arc::new(RwLock::new(cloned_state)),
        }
    }
}

/// flutter_rust_bridge:ignore
#[derive(Debug)]
struct MemoryState {
    /// All blocks by ID (using flattened Block directly)
    blocks: HashMap<String, Block>,
    /// Soft-deleted block IDs (blocks remain in `blocks` but are treated as deleted)
    deleted_ids: HashSet<String>,
    /// Children by parent ID
    children_by_parent: HashMap<String, Vec<String>>,
    /// Counter for deterministic ID generation (increments with each create)
    next_id_counter: u64,
    /// Active change notification subscribers
    subscribers: ChangeSubscribers<Block>,
    /// Bounded ring of past events for watermark replay to new watchers.
    /// `next_seq` doubles as the version counter (one event = one version
    /// step, including each element of a batch op — the old separate
    /// per-*operation* `version_counter` drifted from the log on batch ops).
    event_log: EventRing<BlockChange>,
}

/// Lets the domain run ADR-0005 move/cycle preconditions
/// ([`BlockMutation::validate`]) directly against the in-memory tree.
impl BlockTreeView for MemoryState {
    fn block_exists(&self, id: &EntityUri) -> bool {
        let k = id.as_str();
        self.blocks.contains_key(k) && !self.deleted_ids.contains(k)
    }

    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
        self.blocks.get(id.as_str()).and_then(|b| {
            if b.parent_id.is_no_parent() {
                None
            } else {
                Some(b.parent_id.clone())
            }
        })
    }

    fn children_of(&self, parent: &EntityUri) -> Vec<EntityUri> {
        self.children_by_parent
            .get(parent.as_str())
            // ALLOW(entity_uri_from_raw): id/parent key String from MemoryState store
            .map(|kids| kids.iter().map(|s| EntityUri::from_raw(s)).collect())
            .unwrap_or_default()
    }
}

impl Default for MemoryState {
    fn default() -> Self {
        Self {
            blocks: HashMap::new(),
            deleted_ids: HashSet::new(),
            children_by_parent: HashMap::new(),
            next_id_counter: 0,
            subscribers: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            event_log: EventRing::new(DEFAULT_EVENT_RING_CAPACITY),
        }
    }
}

impl MemoryBackend {
    /// Generate a local URI-based block ID.
    /// Generate a deterministic block ID using a counter.
    /// This ensures the same sequence of operations always generates the same IDs,
    /// which is crucial for property-based testing with proptest where states are cloned.
    fn generate_block_id(state: &mut MemoryState) -> String {
        let id = format!("local://{}", state.next_id_counter);
        state.next_id_counter += 1;
        id
    }

    /// Get current Unix timestamp in milliseconds.
    fn now_millis() -> i64 {
        crate::util::now_unix_millis()
    }

    /// Notify all active subscribers of a change event and add to event log.
    /// Removes closed channels automatically.
    /// Sends the change as a single-item batch.
    /// Note: This spawns a task to avoid blocking on async lock.
    fn notify_subscribers(state: &mut MemoryState, change: Change<Block>) {
        state.event_log.push(change.clone());

        let batch = vec![change];
        let subscribers = state.subscribers.clone();
        tokio::spawn(async move {
            let mut subscribers = subscribers.lock().await;
            deliver_to_subscribers(&mut subscribers, batch).await;
        });
    }

    /// Count of non-deleted blocks.
    pub fn non_deleted_count(&self) -> usize {
        let state = self.state.read().unwrap();
        state.blocks.len() - state.deleted_ids.len()
    }

    /// Whether any non-deleted blocks exist.
    pub fn has_blocks(&self) -> bool {
        self.non_deleted_count() > 0
    }
}

#[async_trait]
impl Lifecycle for MemoryBackend {
    async fn create_new(doc_id: String) -> Result<Self, ApiError> {
        Ok(Self {
            doc_id,
            state: Arc::new(RwLock::new(MemoryState::default())),
        })
    }

    async fn open_existing(_: String) -> Result<Self, ApiError> {
        Err(ApiError::InvalidOperation {
            message: "MemoryBackend does not support persistence".to_string(),
        })
    }

    async fn dispose(&self) -> Result<(), ApiError> {
        // No resources to clean up
        Ok(())
    }
}

#[async_trait]
impl CoreOperations for MemoryBackend {
    async fn get_block(&self, id: &str) -> Result<Block, ApiError> {
        let state = self.state.read().unwrap();

        // Treat deleted blocks as not found
        if state.deleted_ids.contains(id) {
            return Err(ApiError::BlockNotFound { id: id.to_string() });
        }

        let block = state
            .blocks
            .get(id)
            .ok_or_else(|| ApiError::BlockNotFound { id: id.to_string() })?;

        Ok(block.clone())
    }

    async fn get_all_blocks(
        &self,
        traversal: super::types::Traversal,
    ) -> Result<Vec<Block>, ApiError> {
        let state = self.state.read().unwrap();
        let mut result = Vec::new();

        // Helper function for depth-first traversal with level tracking
        fn traverse(
            block_id: &str,
            current_level: usize,
            state: &MemoryState,
            traversal: &super::types::Traversal,
            result: &mut Vec<Block>,
        ) {
            // Skip deleted blocks
            if state.deleted_ids.contains(block_id) {
                return;
            }

            let block = match state.blocks.get(block_id) {
                Some(b) => b,
                None => return, // Skip non-existent blocks
            };

            // Add current block if it's within the level range
            if traversal.includes_level(current_level) {
                result.push(block.clone());
            }

            // Recursively traverse children only if we haven't reached max_level
            if current_level < traversal.max_level {
                let children = state
                    .children_by_parent
                    .get(block_id)
                    .cloned()
                    .unwrap_or_default();
                for child_id in &children {
                    traverse(child_id, current_level + 1, state, traversal, result);
                }
            }
        }

        // Start traversal from root nodes
        for parent_key in state.children_by_parent.keys() {
            // ALLOW(entity_uri_from_raw): id/parent key String from MemoryState store
            let uri = EntityUri::from_raw(parent_key);
            if (uri.is_no_parent() || uri.is_sentinel())
                && let Some(children) = state.children_by_parent.get(parent_key)
            {
                for child_id in children {
                    traverse(child_id, 1, &state, &traversal, &mut result);
                }
            }
        }

        Ok(result)
    }

    async fn list_children(&self, parent_id: &str) -> Result<Vec<String>, ApiError> {
        let state = self.state.read().unwrap();

        // Verify parent exists
        if !state.blocks.contains_key(parent_id) {
            return Err(ApiError::BlockNotFound {
                id: parent_id.to_string(),
            });
        }

        Ok(state
            .children_by_parent
            .get(parent_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn create_block(
        &self,
        parent_id: EntityUri,
        content: BlockContent,
        id: Option<EntityUri>,
    ) -> Result<Block, ApiError> {
        let mut state = self.state.write().unwrap();
        let block_id_str = id
            .as_ref()
            .map(|u| u.as_str().to_string())
            .unwrap_or_else(|| Self::generate_block_id(&mut state));
        let block_id = id.unwrap_or_else(|| EntityUri::parse(&block_id_str).unwrap());
        let parent_id_str = parent_id.as_str().to_string();

        // Validate parent exists (sentinel parents are virtual — always valid)
        let parent_is_virtual = parent_id.is_no_parent() || parent_id.is_sentinel();
        if !parent_is_virtual && !state.blocks.contains_key(&parent_id_str) {
            return Err(ApiError::BlockNotFound { id: parent_id_str });
        }

        let block = Block::from_block_content(block_id, parent_id, content);

        state.blocks.insert(block_id_str.clone(), block.clone());

        // Add to children list
        state
            .children_by_parent
            .entry(parent_id_str)
            .or_default()
            .push(block_id_str);

        Self::notify_subscribers(
            &mut state,
            Change::Created {
                data: block.clone(),
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        );

        Ok(block)
    }

    async fn update_block(&self, id: &str, content: BlockContent) -> Result<(), ApiError> {
        let mut state = self.state.write().unwrap();

        // Check if deleted
        if state.deleted_ids.contains(id) {
            return Err(ApiError::BlockNotFound { id: id.to_string() });
        }

        // Check if block exists
        if !state.blocks.contains_key(id) {
            return Err(ApiError::BlockNotFound { id: id.to_string() });
        }

        // Now update the block
        let block = state.blocks.get_mut(id).unwrap();
        block.set_block_content(content);

        let result_block = block.clone();

        Self::notify_subscribers(
            &mut state,
            Change::Updated {
                id: id.to_string(),
                data: result_block,
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        );

        Ok(())
    }

    async fn delete_block(&self, id: &str) -> Result<(), ApiError> {
        let mut state = self.state.write().unwrap();

        // Check if block exists and not already deleted
        if state.deleted_ids.contains(id) {
            return Err(ApiError::BlockNotFound { id: id.to_string() });
        }

        let block = state
            .blocks
            .get(id)
            .ok_or_else(|| ApiError::BlockNotFound { id: id.to_string() })?;

        let parent_id = block.parent_id.as_raw_str().to_string();

        // Mark as deleted
        state.deleted_ids.insert(id.to_string());

        // Remove from children list
        if let Some(children) = state.children_by_parent.get_mut(&parent_id) {
            children.retain(|child_id| child_id != id);
        }

        // Notify subscribers
        Self::notify_subscribers(
            &mut state,
            Change::Deleted {
                id: id.to_string(),
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        );

        Ok(())
    }

    async fn move_block(
        &self,
        id: &EntityUri,
        new_parent: EntityUri,
        after: Option<EntityUri>,
    ) -> Result<(), ApiError> {
        let new_parent_str = new_parent.as_str().to_string();

        // Domain-level precondition gate (ADR 0005): existence + cycle (+ the
        // `after` table) live in `BlockMutation::validate`, not re-derived here.
        {
            let state = self.state.read().unwrap();
            BlockMutation::Move {
                id: id.clone(),
                new_parent: new_parent.clone(),
                after: after.clone(),
            }
            .validate(&*state)?;
        }

        let mut state = self.state.write().unwrap();

        // Check if deleted
        if state.deleted_ids.contains(id.as_str()) {
            return Err(ApiError::BlockNotFound { id: id.to_string() });
        }

        // Get block and verify it exists
        let block = state
            .blocks
            .get(id.as_str())
            .ok_or_else(|| ApiError::BlockNotFound { id: id.to_string() })?;

        let old_parent = block.parent_id.as_raw_str().to_string();

        // Verify new parent exists and not deleted
        if !state.blocks.contains_key(&new_parent_str)
            || state.deleted_ids.contains(&new_parent_str)
        {
            return Err(ApiError::BlockNotFound {
                id: new_parent_str.clone(),
            });
        }

        // Remove from old location
        if let Some(children) = state.children_by_parent.get_mut(&old_parent) {
            children.retain(|child_id| child_id != id.as_str());
        }

        // Add to new location
        let target_children = state.children_by_parent.entry(new_parent_str).or_default();

        // Insert after specified sibling, or at end
        if let Some(ref after_uri) = after {
            let after_str = after_uri.as_str();
            if let Some(pos) = target_children.iter().position(|cid| cid == after_str) {
                target_children.insert(pos + 1, id.to_string());
            } else {
                target_children.push(id.to_string());
            }
        } else {
            target_children.push(id.to_string());
        }

        // Update block's parent_id and updated_at
        let block = state.blocks.get_mut(id.as_str()).unwrap();
        block.parent_id = new_parent;
        block.updated_at = Self::now_millis();

        let result_block = block.clone();

        // Notify subscribers
        Self::notify_subscribers(
            &mut state,
            Change::Updated {
                id: id.to_string(),
                data: result_block,
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        );

        Ok(())
    }

    async fn get_blocks(&self, ids: Vec<String>) -> Result<Vec<Block>, ApiError> {
        let state = self.state.read().unwrap();
        let mut blocks = Vec::new();

        for id in ids {
            // Skip deleted blocks
            if state.deleted_ids.contains(&id) {
                continue;
            }

            if let Some(block) = state.blocks.get(&id) {
                blocks.push(block.clone());
            }
        }

        Ok(blocks)
    }

    async fn create_blocks(&self, blocks: Vec<NewBlock>) -> Result<Vec<Block>, ApiError> {
        let mut state = self.state.write().unwrap();
        let mut created = Vec::new();

        for new_block in blocks {
            let block_id_str = new_block
                .id
                .as_ref()
                .map(|u| u.as_str().to_string())
                .unwrap_or_else(|| Self::generate_block_id(&mut state));
            let block_id = new_block
                .id
                .unwrap_or_else(|| EntityUri::parse(&block_id_str).unwrap());
            let parent_id_str = new_block.parent_id.as_str().to_string();

            let parent_is_virtual =
                new_block.parent_id.is_no_parent() || new_block.parent_id.is_sentinel();
            if !parent_is_virtual
                && (!state.blocks.contains_key(&parent_id_str)
                    || state.deleted_ids.contains(&parent_id_str))
            {
                return Err(ApiError::BlockNotFound { id: parent_id_str });
            }
            let block = Block::from_block_content(block_id, new_block.parent_id, new_block.content);

            state.blocks.insert(block_id_str.clone(), block.clone());

            // Add to parent's children list
            let children = state.children_by_parent.entry(parent_id_str).or_default();

            if let Some(after_uri) = new_block.after {
                let after_str = after_uri.as_str();
                if let Some(pos) = children.iter().position(|id| id == after_str) {
                    children.insert(pos + 1, block_id_str);
                } else {
                    children.push(block_id_str);
                }
            } else {
                children.push(block_id_str);
            }

            // Notify subscribers
            Self::notify_subscribers(
                &mut state,
                Change::Created {
                    data: block.clone(),
                    origin: ChangeOrigin::Local {
                        operation_id: None,
                        trace_id: None,
                    },
                },
            );

            created.push(block);
        }

        Ok(created)
    }

    async fn delete_blocks(&self, ids: Vec<String>) -> Result<(), ApiError> {
        let mut state = self.state.write().unwrap();

        // Deduplicate IDs to handle cases where the same ID appears multiple times
        let mut seen = std::collections::HashSet::new();

        for id in ids {
            // Skip if we've already processed this ID
            if !seen.insert(id.clone()) {
                continue;
            }

            // Skip if already deleted
            if state.deleted_ids.contains(&id) {
                continue;
            }

            let block = state
                .blocks
                .get(&id)
                .ok_or_else(|| ApiError::BlockNotFound { id: id.clone() })?;

            let parent_id = block.parent_id.as_raw_str().to_string();

            // Mark as deleted
            state.deleted_ids.insert(id.clone());

            // Remove from children list
            if let Some(children) = state.children_by_parent.get_mut(&parent_id) {
                children.retain(|child_id| child_id != &id);
            }

            // Notify subscribers
            Self::notify_subscribers(
                &mut state,
                Change::Deleted {
                    id: id.clone(),
                    origin: ChangeOrigin::Local {
                        operation_id: None,
                        trace_id: None,
                    },
                },
            );
        }

        Ok(())
    }
}

// ChangeNotifications trait implementation
#[async_trait]
impl ChangeNotifications<Block> for MemoryBackend {
    async fn watch_changes_since(
        &self,
        position: StreamPosition,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<Change<Block>>, ApiError>> + Send>> {
        // Collect replay events/blocks synchronously
        let replay_items = match position {
            StreamPosition::Beginning => {
                // Collect blocks while holding the lock
                let state = self.state.read().unwrap();
                state
                    .blocks
                    .iter()
                    .filter_map(|(id, block)| {
                        // Skip deleted blocks
                        if state.deleted_ids.contains(id) {
                            return None;
                        }

                        Some(Change::Created {
                            data: block.clone(),
                            origin: ChangeOrigin::Remote {
                                operation_id: None,
                                trace_id: None,
                            },
                        })
                    })
                    .collect::<Vec<_>>()
            }
            StreamPosition::Version(version) => {
                // Watermark replay from the bounded ring. An evicted
                // watermark fails loud — re-subscribe from `Beginning` —
                // instead of silently replaying partial history.
                let state = self.state.read().unwrap();
                let watermark = u64::from_le_bytes(version.as_slice().try_into().unwrap_or([0; 8]));

                match state.event_log.replay_since(watermark) {
                    Ok(events) => events,
                    Err(expired) => {
                        tracing::error!("[MemoryBackend] watch_changes_since: {expired}");
                        let error_stream = tokio_stream::iter(vec![Err(ApiError::InternalError {
                            message: expired.to_string(),
                        })]);
                        let (_tx, rx) =
                            mpsc::channel::<std::result::Result<Vec<Change<Block>>, ApiError>>(100);
                        return Box::pin(error_stream.chain(ReceiverStream::new(rx)));
                    }
                }
            }
        };

        // Create channel for live updates
        let (tx, rx) = mpsc::channel::<std::result::Result<Vec<Change<Block>>, ApiError>>(100);

        // Subscribe to future changes
        let subscribers = {
            let state = self.state.read().unwrap();
            state.subscribers.clone()
        }; // Drop read lock before async operation
        let mut subscribers = subscribers.lock().await;
        subscribers.push(tx);

        // Create a stream that first yields replay items as a batch, then live updates
        // This avoids spawning tasks which can cause runtime deadlocks
        let replay_batch = if replay_items.is_empty() {
            vec![]
        } else {
            vec![replay_items]
        };
        let replay_stream = tokio_stream::iter(replay_batch.into_iter().map(Ok));
        let live_stream = ReceiverStream::new(rx);
        let combined = replay_stream.chain(live_stream);

        Box::pin(combined)
    }

    async fn get_current_version(&self) -> std::result::Result<Vec<u8>, ApiError> {
        let state = self.state.read().unwrap();
        Ok(state.event_log.next_seq().to_le_bytes().to_vec())
    }
}
