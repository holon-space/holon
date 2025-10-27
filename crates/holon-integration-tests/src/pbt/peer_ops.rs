//! Stable-ID-aware operations on Loro-only peers.
//!
//! Wraps `multi_peer` helpers with UUID-based block identity so that
//! peer-created blocks carry the same stable IDs as the primary instance.

use holon::api::loro_backend::STABLE_ID;
use holon::sync::multi_peer;
use loro::{LoroDoc, TreeID};

/// A lightweight block representation for peer reference state tracking.
#[derive(Debug, Clone)]
pub struct PeerBlock {
    pub stable_id: String,
    pub parent_stable_id: Option<String>,
    pub content: String,
}

/// Read the stable ID from a tree node's metadata.
fn read_node_stable_id(doc: &LoroDoc, node: TreeID) -> Option<String> {
    let tree = doc.get_tree(multi_peer::TREE_NAME);
    let meta = tree.get_meta(node).ok()?;
    meta.get(STABLE_ID).and_then(|v| match v {
        loro::ValueOrContainer::Value(val) => val.as_string().map(|s| s.to_string()),
        _ => None,
    })
}

/// Find a tree node by its stable ID.
pub fn find_node_by_stable_id(doc: &LoroDoc, stable_id: &str) -> Option<TreeID> {
    let tree = doc.get_tree(multi_peer::TREE_NAME);
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        if let Some(sid) = read_node_stable_id(doc, node.id) {
            if sid == stable_id {
                return Some(node.id);
            }
        }
    }
    None
}

/// Create a block on a peer with a specific stable ID.
/// Returns the stable ID.
pub fn peer_create_block(
    doc: &LoroDoc,
    parent_stable_id: Option<&str>,
    content: &str,
    stable_id: &str,
) -> String {
    let parent = parent_stable_id.and_then(|pid| find_node_by_stable_id(doc, pid));
    multi_peer::create_block_with_id(doc, parent, content, stable_id);
    stable_id.to_string()
}

/// Update a block on a peer by stable ID.
pub fn peer_update_block(doc: &LoroDoc, stable_id: &str, content: &str) {
    let node = find_node_by_stable_id(doc, stable_id)
        .unwrap_or_else(|| panic!("peer_update_block: block {} not found", stable_id));
    multi_peer::update_block(doc, node, content);
}

/// Delete a block on a peer by stable ID.
pub fn peer_delete_block(doc: &LoroDoc, stable_id: &str) {
    let node = find_node_by_stable_id(doc, stable_id)
        .unwrap_or_else(|| panic!("peer_delete_block: block {} not found", stable_id));
    multi_peer::delete_block(doc, node);
}

/// Read all alive blocks from a peer's LoroDoc with their stable IDs.
pub fn peer_alive_blocks(doc: &LoroDoc) -> Vec<PeerBlock> {
    let tree = doc.get_tree(multi_peer::TREE_NAME);
    let mut blocks = Vec::new();
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        let Some(stable_id) = read_node_stable_id(doc, node.id) else {
            continue;
        };
        let parent_stable_id = match node.parent {
            loro::TreeParentId::Node(pid) => read_node_stable_id(doc, pid),
            _ => None,
        };
        let content = multi_peer::read_text(&tree, node.id);
        blocks.push(PeerBlock {
            stable_id,
            parent_stable_id,
            content,
        });
    }
    blocks
}
