//! Shared tree management: extract subtrees for collaboration via fork-and-prune.
//!
//! The fork-and-prune algorithm:
//! 1. Reparent the subtree root to tree root (CRITICAL: must happen before parent deletion)
//! 2. Fork the global LoroDoc
//! 3. Collect all TreeIDs to keep (the subtree)
//! 4. Delete everything else from the forked doc
//! 5. Export as shallow_snapshot (optionally preserving recent history)
//! 6. Import into a fresh LoroDoc → the shared tree
//!
//! In the original (personal) tree, the extracted subtree is replaced with a mount node
//! that references the shared tree's ID.

use crate::api::loro_backend::TREE_NAME;
use anyhow::{Context, Result, bail};
use loro::{ExportMode, Frontiers, LoroDoc, LoroTree, LoroValue, TreeID, ValueOrContainer};
use std::collections::HashSet;
use std::sync::Arc;

/// Registry that maps shared_tree_id → LoroDoc for mount-node traversal.
/// LoroBackend uses this to follow mount nodes into shared trees transparently.
pub trait SharedTreeStore: Send + Sync {
    /// Look up a shared tree's LoroDoc by its shared_tree_id.
    fn get_shared_doc(&self, shared_tree_id: &str) -> Option<Arc<LoroDoc>>;

    /// List all shared tree IDs currently in the store.
    fn shared_tree_ids(&self) -> Vec<String>;
}

/// Simple in-memory implementation of SharedTreeStore for testing.
pub struct InMemorySharedTreeStore {
    trees: std::collections::HashMap<String, Arc<LoroDoc>>,
}

impl InMemorySharedTreeStore {
    pub fn new() -> Self {
        Self {
            trees: std::collections::HashMap::new(),
        }
    }

    pub fn insert(&mut self, shared_tree_id: String, doc: LoroDoc) {
        self.trees.insert(shared_tree_id, Arc::new(doc));
    }

    pub fn insert_arc(&mut self, shared_tree_id: String, doc: Arc<LoroDoc>) {
        self.trees.insert(shared_tree_id, doc);
    }
}

impl SharedTreeStore for InMemorySharedTreeStore {
    fn get_shared_doc(&self, shared_tree_id: &str) -> Option<Arc<LoroDoc>> {
        self.trees.get(shared_tree_id).cloned()
    }

    fn shared_tree_ids(&self) -> Vec<String> {
        self.trees.keys().cloned().collect()
    }
}

// Mount node metadata keys
const MOUNT_KIND: &str = "mount_kind";
const MOUNT_KIND_VALUE: &str = "shared_tree";
const MOUNT_SHARED_TREE_ID: &str = "shared_tree_id";
const MOUNT_SHARED_ROOT: &str = "shared_root";

/// Block property key that marks a row as a share-participating node.
/// Value `"mount"` identifies the local mount block; other roles
/// (e.g. `"participant"`) are reserved for future descendant projection.
pub const SHARE_ROLE_PROPERTY: &str = "share-role";
/// Value of `SHARE_ROLE_PROPERTY` for a shared-tree mount row.
pub const SHARE_ROLE_MOUNT: &str = "mount";
/// Block property key storing the shared tree's UUID.
/// Mirrors `MOUNT_SHARED_TREE_ID` in Loro metadata so SQL queries can locate
/// mount rows without traversing Loro.
pub const SHARED_TREE_ID_PROPERTY: &str = "shared-tree-id";

/// Result of extracting a subtree for sharing.
pub struct ExtractedSubtree {
    /// The shared LoroDoc containing only the extracted subtree
    pub shared_doc: LoroDoc,
    /// TreeID of the subtree root in the shared doc (same as in the original)
    pub subtree_root: TreeID,
    /// Number of nodes in the extracted subtree
    pub node_count: usize,
    /// Size of the shared doc snapshot in bytes
    pub snapshot_size: usize,
}

/// Controls how much history to preserve in the extracted subtree.
pub enum HistoryRetention {
    /// Keep all history (including operations on now-deleted non-subtree nodes).
    /// Largest output but preserves full undo history of the subtree.
    Full,
    /// Keep history from a specific frontier forward.
    /// Use `doc.oplog_frontiers()` captured before edits you want to preserve.
    Since(Frontiers),
    /// Discard all history. Only current state is preserved.
    /// Smallest output. Use when history doesn't matter (e.g., one-time sharing).
    None,
}

/// Extract a subtree from a LoroDoc into a new independent shared LoroDoc.
///
/// The source doc is NOT modified — the caller is responsible for replacing the
/// subtree with a mount node in the source doc after extraction.
///
/// The subtree root is reparented to the tree root in the forked copy before
/// pruning, so the shared doc has the subtree as a top-level root.
pub fn extract_subtree(
    source_doc: &LoroDoc,
    subtree_root: TreeID,
    retention: HistoryRetention,
) -> Result<ExtractedSubtree> {
    let forked = source_doc.fork();
    let tree = forked.get_tree(TREE_NAME);

    // Step 1: Reparent subtree to tree root.
    // CRITICAL: LoroTree.delete() hides all descendants. If we delete the parent
    // before reparenting, the subtree becomes invisible.
    tree.mov(subtree_root, None::<TreeID>)
        .context("Failed to reparent subtree root to tree root")?;
    forked.commit();

    // Step 2: Collect all node IDs to keep (the subtree)
    let keep = collect_subtree_ids(&tree, subtree_root);
    let node_count = keep.len();

    // Step 3: Collect ALL root-level nodes and their descendants, delete non-subtree ones.
    // We iterate roots to find all top-level trees, then walk each one.
    let all_roots: Vec<TreeID> = tree.roots();
    let mut to_delete: Vec<TreeID> = Vec::new();
    for root in &all_roots {
        if !keep.contains(root) {
            to_delete.push(*root);
        }
        // Also find descendants of roots that aren't in keep
        collect_non_subtree_descendants(&tree, *root, &keep, &mut to_delete);
    }

    for node in &to_delete {
        // Nodes may already be hidden (descendant of a deleted parent),
        // but delete is idempotent for already-deleted nodes.
        let _ = tree.delete(*node);
    }
    forked.commit();

    // Step 4: Export based on retention policy
    let snapshot = match retention {
        HistoryRetention::Full => forked
            .export(ExportMode::Snapshot)
            .context("Failed to export full snapshot")?,
        HistoryRetention::Since(frontiers) => forked
            .export(ExportMode::shallow_snapshot(&frontiers))
            .context("Failed to export shallow snapshot")?,
        HistoryRetention::None => {
            let current_frontiers = forked.oplog_frontiers();
            forked
                .export(ExportMode::shallow_snapshot(&current_frontiers))
                .context("Failed to export state-only snapshot")?
        }
    };

    let snapshot_size = snapshot.len();

    // Step 5: Import into a fresh LoroDoc. Configure mark styles before
    // any import so subsequent mark applications honor `ExpandType` —
    // see `configure_text_styles` doc and Phase 0.1 spike S3.
    let shared_doc = LoroDoc::new();
    crate::api::loro_backend::configure_text_styles(&shared_doc);
    shared_doc.set_peer_id(rand::random::<u64>())?;
    shared_doc
        .import(&snapshot)
        .context("Failed to import snapshot into shared doc")?;

    Ok(ExtractedSubtree {
        shared_doc,
        subtree_root,
        node_count,
        snapshot_size,
    })
}

/// After extraction, compact the source doc by creating a shallow snapshot.
/// This reclaims space from the extracted subtree's history.
///
/// Returns the compacted LoroDoc (caller should replace their reference).
pub fn gc_after_extraction(source_doc: &LoroDoc) -> Result<LoroDoc> {
    source_doc.commit();
    let current_frontiers = source_doc.oplog_frontiers();
    let shallow = source_doc
        .export(ExportMode::shallow_snapshot(&current_frontiers))
        .context("Failed to export shallow snapshot for GC")?;

    let compacted = LoroDoc::new();
    crate::api::loro_backend::configure_text_styles(&compacted);
    compacted.set_peer_id(rand::random::<u64>())?;
    compacted
        .import(&shallow)
        .context("Failed to import shallow snapshot for GC")?;

    Ok(compacted)
}

/// Collect all TreeIDs in the subtree rooted at `root` (inclusive).
/// Uses an iterative BFS to avoid stack overflow on deep trees.
fn collect_subtree_ids(tree: &LoroTree, root: TreeID) -> HashSet<TreeID> {
    let mut result = HashSet::new();
    let mut queue = vec![root];

    while let Some(node) = queue.pop() {
        result.insert(node);
        for child in tree.children(node).unwrap_or_default() {
            queue.push(child);
        }
    }

    result
}

/// Collect descendants of `node` that are NOT in `keep`, adding them to `to_delete`.
/// Iterative to avoid stack overflow.
fn collect_non_subtree_descendants(
    tree: &LoroTree,
    node: TreeID,
    keep: &HashSet<TreeID>,
    to_delete: &mut Vec<TreeID>,
) {
    let mut queue = vec![node];

    while let Some(current) = queue.pop() {
        for child in tree.children(current).unwrap_or_default() {
            if !keep.contains(&child) {
                to_delete.push(child);
            }
            queue.push(child);
        }
    }
}

// -- Mount node management --

/// Information about a mount node pointing to a shared tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    /// Unique identifier for the shared tree (used to look up the shared LoroDoc)
    pub shared_tree_id: String,
    /// TreeID of the root node in the shared tree
    pub shared_root: TreeID,
}

/// Result of a share_subtree operation: extraction + mount replacement.
pub struct ShareResult {
    pub extracted: ExtractedSubtree,
    /// The mount node created in the source tree
    pub mount_node: TreeID,
    /// The unique shared tree ID assigned to this collaboration
    pub shared_tree_id: String,
}

/// Phase A of the share: the shared doc has been forked off, but the
/// source doc is still untouched. Persistence of `shared_doc` can now
/// succeed or fail without leaving the source in a partial state.
///
/// Pass to [`commit_share_prune`] once the shared snapshot is durable.
pub struct ExtractedShare {
    pub shared_doc: LoroDoc,
    /// TreeID of the root in the shared doc (== source's subtree_root).
    pub shared_root: TreeID,
    /// TreeID of the subtree root in the source doc (to be pruned).
    pub subtree_root_in_source: TreeID,
    /// Parent of the subtree in the source doc. None = was a root.
    pub parent_in_source: Option<TreeID>,
    pub shared_tree_id: String,
    pub node_count: usize,
    pub snapshot_size: usize,
}

/// Phase A (non-destructive): fork the source doc, extract the subtree
/// into a new shared `LoroDoc`, but **do not** modify the source doc.
///
/// The caller must subsequently call [`commit_share_prune`] to apply
/// the destructive half (delete subtree + create mount node). Splitting
/// these phases lets the caller persist the shared snapshot *between*
/// them: if the snapshot save fails, the caller drops the returned
/// `ExtractedShare` and the source doc is untouched — no rollback.
pub fn extract_for_share(
    source_doc: &LoroDoc,
    subtree_root: TreeID,
    parent_in_source: Option<TreeID>,
    shared_tree_id: String,
    retention: HistoryRetention,
) -> Result<ExtractedShare> {
    let extracted = extract_subtree(source_doc, subtree_root, retention)?;
    Ok(ExtractedShare {
        shared_doc: extracted.shared_doc,
        shared_root: extracted.subtree_root,
        subtree_root_in_source: subtree_root,
        parent_in_source,
        shared_tree_id,
        node_count: extracted.node_count,
        snapshot_size: extracted.snapshot_size,
    })
}

/// Phase B (destructive): delete the subtree from the source doc,
/// create a mount node at the original position, and commit the source
/// doc. Returns the new mount node's `TreeID`.
pub fn commit_share_prune(source_doc: &LoroDoc, extracted: &ExtractedShare) -> Result<TreeID> {
    let tree = source_doc.get_tree(TREE_NAME);
    tree.delete(extracted.subtree_root_in_source)
        .context("Failed to delete subtree from source after extraction")?;

    let mount_node = create_mount_node(
        &tree,
        extracted.parent_in_source,
        &extracted.shared_tree_id,
        extracted.shared_root,
    )?;
    source_doc.commit();
    Ok(mount_node)
}

/// Extract a subtree and replace it with a mount node in the source doc.
///
/// This is the one-shot, back-compat helper. New callers that need to
/// persist the shared snapshot between phases should use
/// [`extract_for_share`] + [`commit_share_prune`] directly.
pub fn share_subtree(
    source_doc: &LoroDoc,
    subtree_root: TreeID,
    parent_in_source: Option<TreeID>,
    shared_tree_id: String,
    retention: HistoryRetention,
) -> Result<ShareResult> {
    let extracted = extract_for_share(
        source_doc,
        subtree_root,
        parent_in_source,
        shared_tree_id,
        retention,
    )?;
    let mount_node = commit_share_prune(source_doc, &extracted)?;

    // Rebuild the legacy `ShareResult` shape from the two-phase output.
    let ExtractedShare {
        shared_doc,
        shared_root,
        shared_tree_id,
        node_count,
        snapshot_size,
        ..
    } = extracted;

    Ok(ShareResult {
        extracted: ExtractedSubtree {
            shared_doc,
            subtree_root: shared_root,
            node_count,
            snapshot_size,
        },
        mount_node,
        shared_tree_id,
    })
}

/// Create a mount node in a tree that references a shared tree.
pub fn create_mount_node(
    tree: &LoroTree,
    parent: Option<TreeID>,
    shared_tree_id: &str,
    shared_root: TreeID,
) -> Result<TreeID> {
    let mount = tree.create(parent).context("Failed to create mount node")?;
    let meta = tree
        .get_meta(mount)
        .context("Failed to get mount node metadata")?;
    meta.insert(MOUNT_KIND, MOUNT_KIND_VALUE)?;
    meta.insert(MOUNT_SHARED_TREE_ID, shared_tree_id)?;
    meta.insert(
        MOUNT_SHARED_ROOT,
        format!("{}:{}", shared_root.peer, shared_root.counter),
    )?;
    Ok(mount)
}

/// Check if a tree node is a mount node.
pub fn is_mount_node(tree: &LoroTree, node: TreeID) -> bool {
    let meta = match tree.get_meta(node) {
        Ok(m) => m,
        Err(_) => return false,
    };
    matches!(
        meta.get(MOUNT_KIND),
        Some(ValueOrContainer::Value(LoroValue::String(s))) if s.as_ref() == MOUNT_KIND_VALUE
    )
}

/// Read mount info from a mount node. Returns None if the node is not a mount.
pub fn read_mount_info(tree: &LoroTree, node: TreeID) -> Option<MountInfo> {
    let meta = tree.get_meta(node).ok()?; // ALLOW(ok): node may be deleted

    let kind = match meta.get(MOUNT_KIND) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) if s.as_ref() == MOUNT_KIND_VALUE => {}
        _ => return None,
    };
    let _ = kind;

    let shared_tree_id = match meta.get(MOUNT_SHARED_TREE_ID) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
        _ => return None,
    };

    let shared_root = match meta.get(MOUNT_SHARED_ROOT) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => parse_tree_id_str(&s)?,
        _ => return None,
    };

    Some(MountInfo {
        shared_tree_id,
        shared_root,
    })
}

/// Remove a mount node and optionally re-integrate the shared subtree into the personal tree.
/// If `reintegrate_doc` is Some, the shared tree's contents are imported back.
/// If None, the mount node is simply deleted (unshare without keeping content).
///
/// IMPORTANT: Reintegration only works if the shared doc was extracted with
/// `HistoryRetention::Full` or `HistoryRetention::Since`. Shallow snapshots
/// (`HistoryRetention::None`) break CRDT lineage and edits won't merge back.
pub fn unmount(
    source_doc: &LoroDoc,
    mount_node: TreeID,
    reintegrate_doc: Option<&LoroDoc>,
) -> Result<()> {
    let tree = source_doc.get_tree(TREE_NAME);
    let mount_info = read_mount_info(&tree, mount_node);

    if mount_info.is_none() {
        bail!("Node is not a mount node");
    }

    // Get the mount's parent before deleting it
    let parent = tree.parent(mount_node);
    let parent_tid = match parent {
        Some(loro::TreeParentId::Node(pid)) => Some(pid),
        _ => None,
    };

    tree.delete(mount_node)
        .context("Failed to delete mount node")?;

    if let Some(shared_doc) = reintegrate_doc {
        // Import the shared doc's updates to merge CRDT state (text edits, etc.)
        let snapshot = shared_doc
            .export(ExportMode::Snapshot)
            .context("Failed to export shared doc for reintegration")?;
        source_doc
            .import(&snapshot)
            .context("Failed to import shared doc into source")?;

        // After import, the shared tree's nodes exist in the source doc but may
        // be in "deleted" state (because share_subtree deleted them from source).
        // The CRDT merge may or may not revive them depending on causal ordering.
        // We explicitly move the shared roots to the mount's former parent to
        // ensure they're visible in the tree.
        let shared_tree = shared_doc.get_tree(TREE_NAME);
        let shared_roots = shared_tree.roots();

        let source_tree = source_doc.get_tree(TREE_NAME);
        for root in &shared_roots {
            source_tree.mov(*root, parent_tid).with_context(|| {
                format!(
                    "Failed to reintegrate shared tree root {root:?} under parent {parent_tid:?}"
                )
            })?;
        }
    }

    source_doc.commit();
    Ok(())
}

fn parse_tree_id_str(s: &str) -> Option<TreeID> {
    let (peer_str, counter_str) = s.split_once(':')?;
    let peer = peer_str.parse::<u64>().ok()?; // ALLOW(ok): boundary parse for TreeID
    let counter = counter_str.parse::<i32>().ok()?; // ALLOW(ok): boundary parse for TreeID
    Some(TreeID::new(peer, counter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::LoroText;

    fn set_text(tree: &LoroTree, node: TreeID, content: &str) {
        let meta = tree.get_meta(node).unwrap();
        let text: LoroText = meta
            .insert_container("content_raw", LoroText::new())
            .unwrap();
        text.insert(0, content).unwrap();
    }

    fn read_text(tree: &LoroTree, node: TreeID) -> String {
        let meta = tree.get_meta(node).unwrap();
        match meta.get("content_raw") {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t.to_string(),
            _ => String::new(),
        }
    }

    fn build_test_tree(doc: &LoroDoc) -> (TreeID, TreeID, TreeID, TreeID) {
        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);

        // doc_root
        //   +-- heading_kept
        //   |   +-- block_a
        //   +-- heading_shared    <-- extract this
        //       +-- block_b
        //       +-- block_c

        let doc_root = tree.create(None).unwrap();
        let meta = tree.get_meta(doc_root).unwrap();
        meta.insert("name", "test_doc").unwrap();

        let heading_kept = tree.create(doc_root).unwrap();
        set_text(&tree, heading_kept, "Kept heading");
        let _block_a = tree.create(heading_kept).unwrap();
        set_text(&tree, _block_a, "Block A - stays");

        let heading_shared = tree.create(doc_root).unwrap();
        set_text(&tree, heading_shared, "Shared heading");
        let block_b = tree.create(heading_shared).unwrap();
        set_text(&tree, block_b, "Block B - shared");
        let block_c = tree.create(heading_shared).unwrap();
        set_text(&tree, block_c, "Block C - shared");

        doc.commit();
        (doc_root, heading_kept, heading_shared, block_b)
    }

    #[test]
    fn extract_subtree_preserves_content() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (_doc_root, _kept, shared_root, block_b) = build_test_tree(&doc);

        let result = extract_subtree(&doc, shared_root, HistoryRetention::None).unwrap();

        assert_eq!(result.node_count, 3); // shared_root + block_b + block_c
        assert!(result.snapshot_size > 0);

        let shared_tree = result.shared_doc.get_tree(TREE_NAME);
        assert_eq!(read_text(&shared_tree, shared_root), "Shared heading");
        assert_eq!(read_text(&shared_tree, block_b), "Block B - shared");

        // shared_root should be a root node in the shared doc
        let roots = shared_tree.roots();
        assert!(
            roots.contains(&shared_root),
            "Subtree root should be at tree root level"
        );
    }

    #[test]
    fn extract_does_not_modify_source() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (doc_root, _kept, shared_root, _block_b) = build_test_tree(&doc);

        let _result = extract_subtree(&doc, shared_root, HistoryRetention::None).unwrap();

        // Source doc should be unchanged
        let tree = doc.get_tree(TREE_NAME);
        assert_eq!(tree.children_num(doc_root).unwrap_or(0), 2);
        assert_eq!(read_text(&tree, shared_root), "Shared heading");
    }

    #[test]
    fn full_history_is_larger_than_no_history() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (_doc_root, _kept, shared_root, _block_b) = build_test_tree(&doc);

        let full = extract_subtree(&doc, shared_root, HistoryRetention::Full).unwrap();
        let none = extract_subtree(&doc, shared_root, HistoryRetention::None).unwrap();

        println!("Full history: {} bytes", full.snapshot_size);
        println!("No history: {} bytes", none.snapshot_size);
        assert!(
            full.snapshot_size >= none.snapshot_size,
            "Full history ({}) should be >= no history ({})",
            full.snapshot_size,
            none.snapshot_size
        );
    }

    #[test]
    fn extract_with_edit_history() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (_doc_root, _kept, shared_root, block_b) = build_test_tree(&doc);

        let before_edits = doc.oplog_frontiers();

        // Make edits to build history
        let tree = doc.get_tree(TREE_NAME);
        let meta = tree.get_meta(block_b).unwrap();
        match meta.get("content_raw") {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => {
                t.insert(16, " - EDITED").unwrap();
            }
            _ => panic!("expected text"),
        }
        doc.commit();

        let result =
            extract_subtree(&doc, shared_root, HistoryRetention::Since(before_edits)).unwrap();
        let shared_tree = result.shared_doc.get_tree(TREE_NAME);
        assert_eq!(
            read_text(&shared_tree, block_b),
            "Block B - shared - EDITED"
        );
    }

    #[test]
    fn gc_reduces_parent_size() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();

        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);

        let root = tree.create(None).unwrap();
        // Create many blocks to have significant content
        for i in 0..100 {
            let block = tree.create(root).unwrap();
            set_text(&tree, block, &format!("Block {i}: some content for size"));
        }
        doc.commit();

        let size_before = doc.export(ExportMode::Snapshot).unwrap().len();

        // Delete half the blocks (simulating extraction)
        let children = tree.children(root).unwrap();
        for child in children.iter().skip(50) {
            tree.delete(*child).unwrap();
        }
        doc.commit();

        let compacted = gc_after_extraction(&doc).unwrap();
        let size_after = compacted.export(ExportMode::Snapshot).unwrap().len();

        println!("Before GC: {} bytes", size_before);
        println!("After GC: {} bytes", size_after);
        assert!(
            size_after < size_before,
            "GC'd doc ({size_after}) should be smaller than original ({size_before})"
        );
    }

    #[test]
    fn extracted_subtree_is_syncable() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (_doc_root, _kept, shared_root, block_b) = build_test_tree(&doc);

        let result = extract_subtree(&doc, shared_root, HistoryRetention::None).unwrap();

        // Peer 2 joins by importing the shared doc's snapshot
        let peer2_doc = LoroDoc::new();
        peer2_doc.set_peer_id(2).unwrap();
        let snapshot = result.shared_doc.export(ExportMode::Snapshot).unwrap();
        peer2_doc.import(&snapshot).unwrap();

        // Peer 1 edits in the shared doc
        let shared_tree = result.shared_doc.get_tree(TREE_NAME);
        let meta = shared_tree.get_meta(block_b).unwrap();
        match meta.get("content_raw") {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => {
                t.insert(0, "[P1] ").unwrap();
            }
            _ => panic!("expected text"),
        }
        result.shared_doc.commit();

        // Sync to peer 2
        let vv = peer2_doc.oplog_vv();
        let update = result.shared_doc.export(ExportMode::updates(&vv)).unwrap();
        peer2_doc.import(&update).unwrap();

        let peer2_tree = peer2_doc.get_tree(TREE_NAME);
        assert_eq!(read_text(&peer2_tree, block_b), "[P1] Block B - shared");
    }

    #[test]
    fn large_tree_extraction() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);

        let root = tree.create(None).unwrap();
        let mut target = None;

        // 100 headings x 10 blocks = 1000 blocks
        for i in 0..100 {
            let heading = tree.create(root).unwrap();
            set_text(
                &tree,
                heading,
                &format!("Heading {i} with realistic content"),
            );
            if i == 50 {
                target = Some(heading);
            }
            for j in 0..10 {
                let block = tree.create(heading).unwrap();
                set_text(
                    &tree,
                    block,
                    &format!("Block {i}.{j}: Lorem ipsum dolor sit amet."),
                );
            }
        }
        doc.commit();

        let full_size = doc.export(ExportMode::Snapshot).unwrap().len();
        let result = extract_subtree(&doc, target.unwrap(), HistoryRetention::None).unwrap();

        println!(
            "Full tree (1001 nodes): {} bytes ({:.1} KB)",
            full_size,
            full_size as f64 / 1024.0
        );
        println!(
            "Extracted subtree ({} nodes): {} bytes ({:.1} KB)",
            result.node_count,
            result.snapshot_size,
            result.snapshot_size as f64 / 1024.0
        );
        println!(
            "Ratio: {:.1}x smaller",
            full_size as f64 / result.snapshot_size as f64
        );

        assert_eq!(result.node_count, 11); // 1 heading + 10 blocks
    }

    // -- Mount node tests --

    #[test]
    fn share_subtree_replaces_with_mount() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (doc_root, _kept, shared_root, block_b) = build_test_tree(&doc);

        let result = share_subtree(
            &doc,
            shared_root,
            Some(doc_root),
            "collab-123".to_string(),
            HistoryRetention::None,
        )
        .unwrap();

        let tree = doc.get_tree(TREE_NAME);

        // Original subtree should be gone, replaced by mount node
        // doc_root should now have: heading_kept + mount_node
        assert_eq!(tree.children_num(doc_root).unwrap_or(0), 2);

        // The mount node should be detectable
        assert!(is_mount_node(&tree, result.mount_node));

        // The mount node should reference the shared tree
        let info = read_mount_info(&tree, result.mount_node).unwrap();
        assert_eq!(info.shared_tree_id, "collab-123");
        assert_eq!(info.shared_root, shared_root);

        // The shared doc should have the subtree content
        let shared_tree = result.extracted.shared_doc.get_tree(TREE_NAME);
        assert_eq!(read_text(&shared_tree, block_b), "Block B - shared");
    }

    #[test]
    fn non_mount_node_is_not_detected_as_mount() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (doc_root, _kept, _shared_root, _block_b) = build_test_tree(&doc);

        let tree = doc.get_tree(TREE_NAME);
        assert!(!is_mount_node(&tree, doc_root));
        assert!(read_mount_info(&tree, doc_root).is_none());
    }

    #[test]
    fn mount_node_round_trip() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);

        let root = tree.create(None).unwrap();
        let shared_root_id = TreeID::new(42, 7);
        let mount = create_mount_node(&tree, Some(root), "my-share-id", shared_root_id).unwrap();
        doc.commit();

        // Snapshot and restore
        let snapshot = doc.export(ExportMode::Snapshot).unwrap();
        let restored = LoroDoc::new();
        restored.import(&snapshot).unwrap();
        let restored_tree = restored.get_tree(TREE_NAME);

        assert!(is_mount_node(&restored_tree, mount));
        let info = read_mount_info(&restored_tree, mount).unwrap();
        assert_eq!(info.shared_tree_id, "my-share-id");
        assert_eq!(info.shared_root, shared_root_id);
    }

    #[test]
    fn unmount_removes_mount_node() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (doc_root, _kept, shared_root, _block_b) = build_test_tree(&doc);

        let result = share_subtree(
            &doc,
            shared_root,
            Some(doc_root),
            "collab-456".to_string(),
            HistoryRetention::None,
        )
        .unwrap();

        let tree = doc.get_tree(TREE_NAME);
        assert_eq!(tree.children_num(doc_root).unwrap_or(0), 2); // kept + mount

        // Unmount without reintegration
        unmount(&doc, result.mount_node, None).unwrap();

        assert_eq!(tree.children_num(doc_root).unwrap_or(0), 1); // only kept remains
    }

    #[test]
    fn unmount_with_reintegration() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (doc_root, _kept, shared_root, block_b) = build_test_tree(&doc);

        let result = share_subtree(
            &doc,
            shared_root,
            Some(doc_root),
            "collab-789".to_string(),
            HistoryRetention::Full,
        )
        .unwrap();

        // Edit the shared doc (simulating collaboration)
        let shared_tree = result.extracted.shared_doc.get_tree(TREE_NAME);
        let meta = shared_tree.get_meta(block_b).unwrap();
        match meta.get("content_raw") {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => {
                t.insert(0, "[COLLAB] ").unwrap();
            }
            _ => panic!("expected text"),
        }
        result.extracted.shared_doc.commit();

        // Unmount WITH reintegration
        unmount(&doc, result.mount_node, Some(&result.extracted.shared_doc)).unwrap();

        let tree = doc.get_tree(TREE_NAME);
        // The shared content should be back in the personal tree
        assert_eq!(read_text(&tree, block_b), "[COLLAB] Block B - shared");
    }

    #[test]
    fn unmount_on_non_mount_fails() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let (doc_root, _kept, _shared_root, _block_b) = build_test_tree(&doc);

        let result = unmount(&doc, doc_root, None);
        assert!(result.is_err());
    }
}
