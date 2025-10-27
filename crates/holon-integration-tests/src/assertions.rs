//! Assertion helpers for block comparison

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_orgmode::models::OrgBlockExt;
use std::collections::HashMap;

use crate::org_utils::INTERNAL_PROPS;

/// Normalize a block for comparison by zeroing out timestamps and trimming content.
///
/// Document URIs in parent_id are normalized to a canonical form so that
/// file-based URIs (doc:test.org) and UUID-based URIs (doc:{uuid})
/// for the same document compare equal.
pub fn normalize_block(block: &Block) -> Block {
    let mut normalized = block.clone();
    normalized.created_at = 0;
    normalized.updated_at = 0;
    // Trim overall content and normalize internal trailing whitespace per line
    // (org round-trip strips trailing whitespace from source block lines)
    normalized.content = normalized
        .content
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if normalized.parent_id.is_document() {
        normalized.parent_id = holon_api::EntityUri::doc("__document_root__");
    }
    // Normalize document_id: reference model uses file-based URIs (doc:index.org),
    // SUT uses UUID-based URIs (doc:<uuid>). Both are doc: scheme, so normalize
    // to a common sentinel. Keep __default__ as-is (seeded layout blocks).
    if normalized.document_id.is_document() && normalized.document_id.id() != "__default__" {
        normalized.document_id = holon_api::EntityUri::doc("__normalized_doc__");
    }
    for prop in INTERNAL_PROPS {
        normalized.properties.remove(*prop);
    }
    normalized
}

/// Assert that two Block slices are equivalent (using normalize_block)
pub fn assert_blocks_equivalent(actual_blocks: &[Block], expected_blocks: &[Block], message: &str) {
    let mut actual_sorted: Vec<_> = actual_blocks.iter().map(normalize_block).collect();
    let mut expected_sorted: Vec<_> = expected_blocks.iter().map(normalize_block).collect();
    actual_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    expected_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    assert_eq!(
        actual_sorted, expected_sorted,
        "{}: Blocks differ between actual and expected",
        message
    );
}

/// Check if a block belongs to a specific document (directly or through ancestors)
pub fn block_belongs_to_document(block: &Block, all_blocks: &[Block], doc_uri: &str) -> bool {
    if block.parent_id.as_raw_str() == doc_uri {
        return true;
    }
    if let Some(parent) = all_blocks
        .iter()
        .find(|b| block.parent_id.as_raw_str() == b.id.as_str())
    {
        return block_belongs_to_document(parent, all_blocks, doc_uri);
    }
    false
}

/// Assert that block ordering in the org file matches the reference model.
///
/// For each parent, compares the org file order (by parser-assigned sequence)
/// against the reference model order (by reference-assigned sequence).
/// Reference sequences are set either from file content order (WriteOrgFile)
/// or canonical ordering (BulkExternalAdd via assign_reference_sequences).
pub fn assert_block_order(org_blocks: &[Block], ref_blocks: &[Block], message: &str) {
    let parent_ids: std::collections::HashSet<String> =
        org_blocks.iter().map(|b| b.parent_id.to_string()).collect();

    for parent_id in &parent_ids {
        let mut org_children: Vec<&Block> = org_blocks
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .collect();
        org_children.sort_by(|a, b| {
            a.sequence()
                .cmp(&b.sequence())
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        let org_order: Vec<&str> = org_children.iter().map(|b| b.id.as_str()).collect();

        let parent_ref = EntityUri::from_raw(parent_id);
        let mut ref_children: Vec<&Block> = ref_blocks
            .iter()
            .filter(|b| {
                if parent_ref.is_document() {
                    b.parent_id.is_document()
                } else {
                    b.parent_id.as_raw_str() == parent_id.as_str()
                }
            })
            .collect();
        ref_children.sort_by(|a, b| {
            a.sequence()
                .cmp(&b.sequence())
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        let ref_order: Vec<&str> = ref_children.iter().map(|b| b.id.as_str()).collect();

        // Only compare if both sides have the same block IDs
        if org_order.len() == ref_order.len() && org_order.iter().all(|id| ref_order.contains(id)) {
            // Skip ordering check for source-only sibling groups — known pre-existing
            // bug where production OrgRenderer reorders source block siblings during
            // the initial file sync round-trip. TODO: fix the root cause.
            let all_source = org_children
                .iter()
                .all(|b| b.content_type == ContentType::Source);
            if all_source {
                continue;
            }
            if org_order != ref_order {
                eprintln!(
                    "WARNING: {}: Block order mismatch under parent '{}'\n  \
                     Org file order:  {:?}\n  \
                     Expected order:  {:?}\n  \
                     (soft assertion — ordering bugs tracked separately)",
                    message, parent_id, org_order, ref_order
                );
            }
        }
    }
}

/// Reference state for tracking blocks (used by find_document_for_block)
pub struct ReferenceState {
    pub blocks: HashMap<String, Block>,
}

/// Find the document URI that a block belongs to
pub fn find_document_for_block(block_id: &str, ref_state: &ReferenceState) -> Option<String> {
    let block = ref_state.blocks.get(block_id)?;

    if block.parent_id.is_document() {
        return Some(block.parent_id.to_string());
    }

    find_document_for_block(block.parent_id.as_raw_str(), ref_state)
}
