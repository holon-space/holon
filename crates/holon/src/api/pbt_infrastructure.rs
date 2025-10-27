//! Public PBT infrastructure for testing CoreOperations implementations
//!
//! This module extracts the core property-based testing logic from loro_backend_pbt.rs
//! so it can be reused to test other CoreOperations implementations like Flutter UI.

#[cfg(not(target_arch = "wasm32"))]
use super::memory_backend::MemoryBackend;
use super::repository::{CoreOperations, Lifecycle};
use super::types::NewBlock;
use holon_api::{ApiError, Block, BlockContent, ContentType, EntityUri};
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

// Re-export proptest types for convenience
#[cfg(not(target_arch = "wasm32"))]
pub use proptest::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
pub use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

pub type WatcherId = usize;

/// Whether property-based testing infrastructure has full runtime support on this target.
pub const fn is_pbt_supported() -> bool {
    !cfg!(target_arch = "wasm32")
}

/// Static reason string for targets where PBT cannot run yet.
pub const PBT_UNSUPPORTED_REASON: &str = "Property-based testing is currently available only on native targets because the \
tokio runtime and proptest runners rely on OS threading APIs that don't compile to wasm32.";

/// Reference state wraps MemoryBackend (our reference implementation)
///
/// Note: This simplified version doesn't support watchers to avoid dependencies on futures crate.
/// For full watcher support, use the test-only version in loro_backend_pbt.rs.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct ReferenceState {
    pub backend: MemoryBackend,
    pub handle: tokio::runtime::Handle,
    /// Optional runtime - Some when we own the runtime (standalone tests), None when using existing runtime (Flutter)
    pub _runtime: Option<Arc<tokio::runtime::Runtime>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for ReferenceState {
    fn default() -> Self {
        // Try to use current runtime handle if available (when called from async context),
        // otherwise create a new runtime (for standalone/sync tests)
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let backend = tokio::task::block_in_place(|| {
                    handle.block_on(MemoryBackend::create_new("reference".to_string()))
                })
                .unwrap();

                Self {
                    backend,
                    handle,
                    _runtime: None, // We don't own the runtime
                }
            }
            Err(_) => {
                // No current runtime, create one (standalone cargo test)
                let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
                let handle = runtime.handle().clone();
                let backend = runtime
                    .block_on(MemoryBackend::create_new("reference".to_string()))
                    .unwrap();

                Self {
                    backend,
                    handle,
                    _runtime: Some(runtime),
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Clone for ReferenceState {
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
            handle: self.handle.clone(),
            _runtime: self._runtime.clone(),
        }
    }
}

/// Transitions/Commands for the block tree
#[derive(Clone, Debug)]
pub enum BlockTransition {
    CreateBlock { parent_id: String, content: String },
    UpdateBlock { id: String, content: String },
    DeleteBlock { id: String },
    MoveBlock { id: String, new_parent: String },
    CreateBlocks { blocks: Vec<(String, String)> },
    DeleteBlocks { ids: Vec<String> },
    WatchChanges { watcher_id: WatcherId },
    UnwatchChanges { watcher_id: WatcherId },
}

/// System under test - generic over any backend implementing CoreOperations + Lifecycle
///
/// Note: This simplified version doesn't support watchers to avoid dependencies on futures crate.
/// For full watcher support, use the test-only version in loro_backend_pbt.rs.
pub struct BlockTreeTest<R: CoreOperations + Lifecycle> {
    pub backend: R,
    /// ID mapping: MemoryBackend ID → Backend ID
    pub id_map: HashMap<String, String>,
}

/// Helper to translate a single ID from MemoryBackend → Backend
///
/// Document URIs and sentinel URIs are never translated - they're the same in all backends
pub fn translate_id(mem_id: &str, id_map: &HashMap<String, String>) -> Option<String> {
    let pr = EntityUri::from_raw(mem_id);
    if pr.is_no_parent() || pr.is_document() {
        return Some(mem_id.to_string());
    }
    id_map.get(mem_id).cloned()
}

/// Translate a BlockTransition from MemoryBackend IDs to Backend IDs
pub fn translate_transition(
    transition: &BlockTransition,
    id_map: &HashMap<String, String>,
) -> BlockTransition {
    match transition {
        BlockTransition::CreateBlock { parent_id, content } => BlockTransition::CreateBlock {
            parent_id: translate_id(parent_id, id_map).unwrap_or_else(|| {
                panic!(
                    "CreateBlock parent: ID '{}' must exist in id_map",
                    parent_id
                )
            }),
            content: content.clone(),
        },
        BlockTransition::UpdateBlock { id, content } => BlockTransition::UpdateBlock {
            id: translate_id(id, id_map)
                .unwrap_or_else(|| panic!("UpdateBlock: ID '{}' must exist in id_map", id)),
            content: content.clone(),
        },
        BlockTransition::DeleteBlock { id } => BlockTransition::DeleteBlock {
            id: translate_id(id, id_map)
                .unwrap_or_else(|| panic!("DeleteBlock: ID '{}' must exist in id_map", id)),
        },
        BlockTransition::MoveBlock { id, new_parent } => BlockTransition::MoveBlock {
            id: translate_id(id, id_map)
                .unwrap_or_else(|| panic!("MoveBlock: ID '{}' must exist in id_map", id)),
            new_parent: translate_id(new_parent, id_map).unwrap_or_else(|| {
                panic!("MoveBlock parent: ID '{}' must exist in id_map", new_parent)
            }),
        },
        BlockTransition::CreateBlocks { blocks } => BlockTransition::CreateBlocks {
            blocks: blocks
                .iter()
                .map(|(parent_id, content)| {
                    (
                        translate_id(parent_id, id_map).unwrap_or_else(|| {
                            panic!(
                                "CreateBlocks parent: ID '{}' must exist in id_map",
                                parent_id
                            )
                        }),
                        content.clone(),
                    )
                })
                .collect(),
        },
        BlockTransition::DeleteBlocks { ids } => BlockTransition::DeleteBlocks {
            ids: ids
                .iter()
                .map(|id| translate_id(id, id_map).expect("ID must exist in map for DeleteBlocks"))
                .collect(),
        },
        BlockTransition::WatchChanges { watcher_id } => BlockTransition::WatchChanges {
            watcher_id: *watcher_id,
        },
        BlockTransition::UnwatchChanges { watcher_id } => BlockTransition::UnwatchChanges {
            watcher_id: *watcher_id,
        },
    }
}

/// Apply a BlockTransition to any CoreOperations implementation
pub async fn apply_transition<R: CoreOperations>(
    backend: &R,
    transition: &BlockTransition,
) -> Result<Vec<Block>, ApiError> {
    match transition {
        BlockTransition::CreateBlock { parent_id, content } => {
            let block = backend
                .create_block(
                    EntityUri::from_raw(parent_id),
                    BlockContent::text(content),
                    None,
                )
                .await?;
            Ok(vec![block])
        }
        BlockTransition::UpdateBlock { id, content } => {
            backend
                .update_block(id, BlockContent::text(content))
                .await?;
            Ok(vec![])
        }
        BlockTransition::DeleteBlock { id } => {
            backend.delete_block(id).await?;
            Ok(vec![])
        }
        BlockTransition::MoveBlock { id, new_parent } => {
            backend
                .move_block(id, EntityUri::from_raw(new_parent), None)
                .await?;
            Ok(vec![])
        }
        BlockTransition::CreateBlocks { blocks } => {
            let new_blocks: Vec<NewBlock> = blocks
                .iter()
                .map(|(parent_id, content)| NewBlock {
                    parent_id: EntityUri::from_raw(parent_id),
                    content: BlockContent::text(content),
                    id: None,
                    after: None,
                })
                .collect();
            let created = backend.create_blocks(new_blocks).await?;
            Ok(created)
        }
        BlockTransition::DeleteBlocks { ids } => {
            backend.delete_blocks(ids.clone()).await?;
            Ok(vec![])
        }
        BlockTransition::WatchChanges { .. } | BlockTransition::UnwatchChanges { .. } => Ok(vec![]),
    }
}

/// Fields compared per-block when verifying backend equivalence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ComparableBlock {
    depth: usize,
    parent_id: String,
    content: String,
    content_type: ContentType,
    source_language: Option<String>,
}

impl std::fmt::Display for ComparableBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "depth={} parent={} type={} lang={:?} content={:?}",
            self.depth, self.parent_id, self.content_type, self.source_language, self.content,
        )
    }
}

/// Verify that two backends have structurally identical state using field-by-field comparison.
///
/// Compares `(depth, parent_id, content, content_type, source_language)` for every block.
/// Block lists are sorted by a stable key so that ordering differences between backends
/// do not cause false negatives.
pub fn verify_backends_match<R1, R2>(
    reference: &R1,
    system_under_test: &R2,
    handle: &tokio::runtime::Handle,
) where
    R1: CoreOperations,
    R2: CoreOperations,
{
    let ref_blocks = handle
        .block_on(reference.get_all_blocks(super::types::Traversal::ALL_BUT_ROOT))
        .expect("Failed to get reference blocks");
    let sut_blocks = handle
        .block_on(system_under_test.get_all_blocks(super::types::Traversal::ALL_BUT_ROOT))
        .expect("Failed to get SUT blocks");

    fn compute_depth_in_slice(block: &Block, all_blocks: &[Block]) -> usize {
        block.depth_from(|id| all_blocks.iter().find(|b| b.id.as_str() == id))
    }

    // Translate parent_ids to depth-relative form so that backend-specific IDs
    // don't cause spurious mismatches. We keep the parent_id only when it is a
    // well-known sentinel (document URI or sentinel:no_parent); otherwise we store a
    // canonical "parent_depth=<n>" placeholder derived from looking up the parent.
    fn normalize_parent(block: &Block, all_blocks: &[Block]) -> String {
        if block.parent_id.is_no_parent() {
            return block.parent_id.to_string();
        }
        // Document URIs differ between backends (e.g., doc:reference vs
        // doc:test-pbt), so normalize them to a canonical placeholder.
        if block.parent_id.is_document() {
            return "@doc_root".to_string();
        }
        if let Some(parent) = all_blocks
            .iter()
            .find(|b| block.parent_id.as_raw_str() == b.id.as_str())
        {
            let parent_depth = compute_depth_in_slice(parent, all_blocks);
            format!("@depth{}:{}", parent_depth, parent.content)
        } else {
            block.parent_id.to_string()
        }
    }

    let mut ref_comparable: Vec<ComparableBlock> = ref_blocks
        .iter()
        .map(|b| ComparableBlock {
            depth: compute_depth_in_slice(b, &ref_blocks),
            parent_id: normalize_parent(b, &ref_blocks),
            content: b.content.clone(),
            content_type: b.content_type,
            source_language: b.source_language.as_ref().map(|l| l.to_string()),
        })
        .collect();

    let mut sut_comparable: Vec<ComparableBlock> = sut_blocks
        .iter()
        .map(|b| ComparableBlock {
            depth: compute_depth_in_slice(b, &sut_blocks),
            parent_id: normalize_parent(b, &sut_blocks),
            content: b.content.clone(),
            content_type: b.content_type,
            source_language: b.source_language.as_ref().map(|l| l.to_string()),
        })
        .collect();

    ref_comparable.sort();
    sut_comparable.sort();

    assert_eq!(
        ref_comparable.len(),
        sut_comparable.len(),
        "Block count mismatch: reference has {} blocks, SUT has {} blocks\n\
         Reference blocks:\n{}\n\nSUT blocks:\n{}",
        ref_comparable.len(),
        sut_comparable.len(),
        ref_comparable
            .iter()
            .map(|b| format!("  {b}"))
            .collect::<Vec<_>>()
            .join("\n"),
        sut_comparable
            .iter()
            .map(|b| format!("  {b}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    for (i, (r, s)) in ref_comparable.iter().zip(sut_comparable.iter()).enumerate() {
        assert_eq!(
            r, s,
            "Block mismatch at sorted position {i}:\n  reference: {r}\n  SUT:       {s}",
        );
    }
}

/// Populate ID map with initial blocks from both backends
///
/// When backends are initialized via `Lifecycle::create_new()`, they may create
/// initial child blocks (e.g., "local://0" in MemoryBackend).
///
/// This function maps these initial blocks between reference and SUT backends
/// by matching top-level blocks (those with document URI parents) with the same content.
pub async fn populate_initial_id_map<R1: CoreOperations, R2: CoreOperations>(
    id_map: &mut HashMap<String, String>,
    ref_backend: &R1,
    sut_backend: &R2,
) -> Result<(), ApiError> {
    use super::types::Traversal;

    // Get all initial blocks from both backends
    let ref_blocks = ref_backend.get_all_blocks(Traversal::ALL).await?;
    let sut_blocks = sut_backend.get_all_blocks(Traversal::ALL).await?;

    // Map initial child blocks by matching parent_id and content
    // We match top-level blocks (those with document URI parents) with the same content
    for ref_block in &ref_blocks {
        if ref_block.parent_id.is_document() && !id_map.contains_key(ref_block.id.as_str()) {
            // Find matching block in SUT by parent_id and content
            if let Some(sut_block) = sut_blocks.iter().find(|b| {
                b.parent_id.is_document()
                    && b.content == ref_block.content
                    && !id_map.values().any(|v| v == b.id.as_str())
            }) {
                id_map.insert(ref_block.id.to_string(), sut_block.id.to_string());
            }
        }
    }

    Ok(())
}

/// Update ID map after create operations
///
/// Matches newly created blocks in reference backend with SUT backend
/// by comparing parent_id and content.
pub fn update_id_map_after_create(
    id_map: &mut HashMap<String, String>,
    transition: &BlockTransition,
    ref_blocks: &[Block],
    created_blocks: &[Block],
) {
    if created_blocks.is_empty() {
        return;
    }

    match transition {
        BlockTransition::CreateBlock { parent_id, content } => {
            // Find the newly created block in reference backend.
            // When multiple siblings match (parent_id, content), pick the last one
            // among the parent's children — newly created blocks are appended at the end.
            let candidates: Vec<&Block> = ref_blocks
                .iter()
                .filter(|b| {
                    !id_map.contains_key(b.id.as_str())
                        && b.content_text() == content
                        && b.parent_id == EntityUri::from_raw(parent_id)
                })
                .collect();
            assert!(
                !candidates.is_empty(),
                "Should find newly created block in reference"
            );
            let ref_block = if candidates.len() == 1 {
                candidates[0]
            } else {
                // Tiebreaker: among siblings with the same content, pick the one
                // appearing last in the parent's child list (the most recently appended).
                let sibling_ids: Vec<&str> = ref_blocks
                    .iter()
                    .filter(|b| b.parent_id == EntityUri::from_raw(parent_id))
                    .map(|b| b.id.as_str())
                    .collect();
                *candidates
                    .iter()
                    .max_by_key(|b| {
                        sibling_ids
                            .iter()
                            .position(|id| *id == b.id.as_str())
                            .unwrap_or(0)
                    })
                    .unwrap()
            };

            id_map.insert(ref_block.id.to_string(), created_blocks[0].id.to_string());
        }
        BlockTransition::CreateBlocks { blocks } => {
            assert_eq!(
                blocks.len(),
                created_blocks.len(),
                "CreateBlocks: batch size ({}) must equal created_blocks size ({})",
                blocks.len(),
                created_blocks.len(),
            );

            // When multiple ref_blocks match (parent_id, content), use sibling
            // position as tiebreaker: prefer later positions (newly appended).
            let mut used_ref_ids: HashSet<String> = HashSet::new();
            for (i, (parent_id, content)) in blocks.iter().enumerate() {
                let candidates: Vec<&Block> = ref_blocks
                    .iter()
                    .filter(|b| {
                        !id_map.contains_key(b.id.as_str())
                            && !used_ref_ids.contains(b.id.as_str())
                            && b.content_text() == content
                            && b.parent_id == EntityUri::from_raw(parent_id)
                    })
                    .collect();
                assert!(
                    !candidates.is_empty(),
                    "Should find newly created block in reference for batch entry {i}"
                );
                let ref_block = if candidates.len() == 1 {
                    candidates[0]
                } else {
                    let sibling_ids: Vec<&str> = ref_blocks
                        .iter()
                        .filter(|b| b.parent_id == EntityUri::from_raw(parent_id))
                        .map(|b| b.id.as_str())
                        .collect();
                    *candidates
                        .iter()
                        .max_by_key(|b| {
                            sibling_ids
                                .iter()
                                .position(|id| *id == b.id.as_str())
                                .unwrap_or(0)
                        })
                        .unwrap()
                };
                used_ref_ids.insert(ref_block.id.to_string());

                let sut_block = &created_blocks[i];
                id_map.insert(ref_block.id.to_string(), sut_block.id.to_string());
            }
        }
        _ => {}
    }
}

/// Generate CRUD transition strategies given a list of blocks
///
/// This is the core transition generator that can be reused by different test implementations.
/// Returns a strategy that generates CreateBlock, UpdateBlock, DeleteBlock, MoveBlock, CreateBlocks, and DeleteBlocks transitions.
#[cfg(not(target_arch = "wasm32"))]
pub fn generate_crud_transitions(
    all_ids: Vec<String>,
    non_root_ids: Vec<String>,
) -> BoxedStrategy<BlockTransition> {
    let create_block = (prop::sample::select(all_ids.clone()), "[a-z]{1,10}")
        .prop_map(|(parent, content)| BlockTransition::CreateBlock {
            parent_id: parent,
            content,
        })
        .boxed();

    let create_blocks = prop::collection::vec(
        (prop::sample::select(all_ids.clone()), "[a-z]{1,10}"),
        1..=3,
    )
    .prop_map(|blocks| BlockTransition::CreateBlocks { blocks })
    .boxed();

    // When we have no user blocks yet (only root), only allow create operations
    if non_root_ids.is_empty() {
        return prop::strategy::Union::new_weighted(vec![(30, create_block), (10, create_blocks)])
            .boxed();
    }

    // When we have user blocks, allow all operations
    let update_block = (prop::sample::select(non_root_ids.clone()), "[a-z]{1,10}")
        .prop_map(|(id, content)| BlockTransition::UpdateBlock { id, content })
        .boxed();

    let delete_block = prop::sample::select(non_root_ids.clone())
        .prop_map(|id| BlockTransition::DeleteBlock { id })
        .boxed();

    let move_block = (
        prop::sample::select(non_root_ids.clone()),
        prop::sample::select(all_ids.clone()),
    )
        .prop_map(|(id, new_parent)| BlockTransition::MoveBlock { id, new_parent })
        .boxed();

    let delete_blocks = prop::collection::vec(prop::sample::select(non_root_ids), 1..=3)
        .prop_map(|mut ids| {
            ids.sort();
            ids.dedup();
            BlockTransition::DeleteBlocks { ids }
        })
        .prop_filter("non-empty after dedup", |t| match t {
            BlockTransition::DeleteBlocks { ids } => !ids.is_empty(),
            _ => unreachable!(),
        })
        .boxed();

    prop::strategy::Union::new_weighted(vec![
        (30, create_block),
        (20, update_block),
        (15, delete_block),
        (15, move_block),
        (10, create_blocks),
        (10, delete_blocks),
    ])
    .boxed()
}

/// Check preconditions for a BlockTransition using the backend's logic
///
/// This async version delegates cycle detection to the backend's `get_ancestor_chain`,
/// ensuring consistent tree traversal logic across the codebase.
pub async fn check_transition_preconditions<B: CoreOperations>(
    transition: &BlockTransition,
    backend: &B,
) -> bool {
    // Get current block IDs for existence checks
    let all_blocks = match backend.get_all_blocks(super::types::Traversal::ALL).await {
        Ok(blocks) => blocks,
        Err(_) => return false,
    };
    let block_ids: HashSet<String> = all_blocks.iter().map(|b| b.id.to_string()).collect();

    match transition {
        BlockTransition::CreateBlock { parent_id, .. } => block_ids.contains(parent_id),
        BlockTransition::UpdateBlock { id, .. } | BlockTransition::DeleteBlock { id } => {
            block_ids.contains(id)
        }
        BlockTransition::MoveBlock { id, new_parent } => {
            // Use backend's cycle detection via get_ancestor_chain
            if !block_ids.contains(id) || !block_ids.contains(new_parent) || id == new_parent {
                false
            } else {
                // Check if new_parent is an ancestor of id (would create cycle)
                match backend.get_ancestor_chain(new_parent).await {
                    Ok(ancestors) => !ancestors.contains(id),
                    Err(_) => false,
                }
            }
        }
        BlockTransition::CreateBlocks { blocks } => blocks
            .iter()
            .all(|(parent_id, _)| block_ids.contains(parent_id)),
        BlockTransition::DeleteBlocks { ids } => ids.iter().all(|id| block_ids.contains(id)),
        BlockTransition::WatchChanges { .. } => true,
        BlockTransition::UnwatchChanges { .. } => {
            // Shared infrastructure has no watcher state — specific implementations
            // (e.g. loro_backend_pbt) must override this with their own check that
            // the watcher_id exists.
            false
        }
    }
}

/// ReferenceStateMachine implementation for MemoryBackend
///
/// This generates random transitions and validates them against the reference implementation.
#[cfg(not(target_arch = "wasm32"))]
impl ReferenceStateMachine for ReferenceState {
    type State = Self;
    type Transition = BlockTransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(ReferenceState::default()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        // Get all blocks including root (root will be parent for top-level user blocks)
        let all_blocks = tokio::task::block_in_place(|| {
            state
                .handle
                .block_on(state.backend.get_all_blocks(super::types::Traversal::ALL))
        })
        .unwrap_or_default();
        let all_ids: Vec<String> = all_blocks.iter().map(|b| b.id.to_string()).collect();
        let non_root_ids: Vec<String> = all_blocks
            .iter()
            .filter(|b| !b.parent_id.is_no_parent() && !b.parent_id.is_document())
            .map(|b| b.id.to_string())
            .collect();

        generate_crud_transitions(all_ids, non_root_ids)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        tokio::task::block_in_place(|| {
            state
                .handle
                .block_on(check_transition_preconditions(transition, &state.backend))
        })
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        // Apply the transition to MemoryBackend
        tokio::task::block_in_place(|| {
            state
                .handle
                .block_on(apply_transition(&state.backend, transition))
        })
        .expect("Reference backend transition should succeed (preconditions validated it)");

        state
    }
}
