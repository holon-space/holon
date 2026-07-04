//! Org file serialization utilities

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_orgmode::OrgRenderer;
use holon_orgmode::models::OrgBlockExt;
use holon_orgmode::models::ToOrg;

/// Extract the first :ID: property value from org content.
///
/// This is useful for waiting on a specific block to sync after writing an org
/// file.
pub fn extract_first_block_id(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(":ID:") {
            return Some(trimmed.strip_prefix(":ID:")?.trim().to_string());
        }
    }
    None
}

/// Serialize Blocks to Org file format
pub fn serialize_blocks_to_org(blocks: &[&Block], doc_uri: &EntityUri) -> String {
    serialize_blocks_to_org_with_doc(blocks, doc_uri, None)
}

/// Serialize blocks to org format, optionally including a document header
/// (#+TITLE, #+TODO) from the document block. Without the header, non-default
/// task keywords (e.g. WAITING) are not recognized on re-parse, causing content
/// corruption through keyword-in-title echo loops.
pub fn serialize_blocks_to_org_with_doc(
    blocks: &[&Block],
    doc_uri: &EntityUri,
    doc_block: Option<&Block>,
) -> String {
    let mut root_blocks: Vec<&&Block> = blocks.iter().filter(|b| b.parent_id == *doc_uri).collect();
    // Match production OrgRenderer sorting: section content (Source/Image) first,
    // then sequence, then ID — via the one domain rule (ADR 0005).
    root_blocks.sort_by(|a, b| {
        a.content_type
            .sibling_order_group()
            .cmp(&b.content_type.sibling_order_group())
            .then_with(|| a.sequence().cmp(&b.sequence()))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    let mut result = String::new();

    if let Some(doc) = doc_block {
        let header = holon_orgmode::models::render_document_header(doc);
        result.push_str(&header);
    }

    for block in root_blocks {
        serialize_block_recursive(block, blocks, &mut result, 1);
    }

    result
}

/// Recursively serialize a block and its children
pub fn serialize_block_recursive(
    block: &Block,
    all_blocks: &[&Block],
    result: &mut String,
    level: usize,
) {
    // HARNESS-OWNED: only the tree walk (level → depth, sibling order, child
    // recursion). Every byte for the block itself comes from production's own
    // entry point — `OrgRenderer::prepare_block_for_org` → `Block::to_org` —
    // the same pair `OrgRenderer::render_entity_tree` uses. Re-deriving any of
    // it here (drawer, headline, source header args) is what let harness-only
    // keys such as `_drawer_order` reach disk and made the expected file
    // disagree with what write-back actually writes.
    let mut prepared = block.clone();
    // No owning document in hand here, so no vocabulary is known — the same
    // `None` production's document-less render path passes.
    OrgRenderer::prepare_block_for_org(&mut prepared, level - 1, None);
    result.push_str(&prepared.to_org());

    let mut children: Vec<&&Block> = all_blocks
        .iter()
        .filter(|b| b.parent_id.as_raw_str() == block.id.as_str())
        .collect();
    // Match production OrgRenderer sorting: section content (Source/Image) first,
    // then sequence, then ID — via the one domain rule (ADR 0005).
    children.sort_by(|a, b| {
        a.content_type
            .sibling_order_group()
            .cmp(&b.content_type.sibling_order_group())
            .then_with(|| a.sequence().cmp(&b.sequence()))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    for child in children {
        serialize_block_recursive(child, all_blocks, result, level + 1);
    }
}

/// Assign sequence numbers to blocks that don't already have them set.
///
/// For each parent group where no child has a non-zero sequence (i.e.,
/// sequences were not set from file order by WriteOrgFile), assigns the
/// canonical ordering: source blocks first, then text blocks, sorted by ID
/// within each group. This matches the ordering used by
/// `serialize_blocks_to_org`.
///
/// Parent groups where any child already has sequence > 0 are skipped, since
/// those sequences were set from actual file order and should be preserved.
pub fn assign_reference_sequences(blocks: &mut [Block]) {
    use std::collections::HashMap;
    use std::collections::HashSet;

    let parent_ids: HashSet<String> = blocks.iter().map(|b| b.parent_id.to_string()).collect();

    let mut seq_map: HashMap<String, i64> = HashMap::new();
    for parent_id in &parent_ids {
        let children: Vec<&Block> = blocks
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .collect();
        // Skip if any child already has a sequence set (from file order)
        if children.iter().any(|b| b.sequence() > 0) {
            continue;
        }
        let mut sorted: Vec<(String, u8)> = children
            .iter()
            .map(|b| (b.id.to_string(), b.content_type.sibling_order_group()))
            .collect();
        sorted
            .sort_by(|(a_id, a_grp), (b_id, b_grp)| a_grp.cmp(b_grp).then_with(|| a_id.cmp(b_id)));
        for (i, (id, _)) in sorted.iter().enumerate() {
            seq_map.insert(id.clone(), i as i64);
        }
    }

    for block in blocks.iter_mut() {
        if let Some(&seq) = seq_map.get(block.id.as_str()) {
            block.set_sequence(seq);
        }
    }
}

/// Force-assign canonical sequence numbers to all blocks, overwriting any
/// existing values.
///
/// Used when the org file is re-written via `serialize_blocks_to_org` (e.g.,
/// after an external mutation), which always sorts in canonical order
/// regardless of existing sequences.
pub fn assign_reference_sequences_canonical(blocks: &mut [Block]) {
    use std::collections::HashMap;
    use std::collections::HashSet;

    let parent_ids: HashSet<String> = blocks.iter().map(|b| b.parent_id.to_string()).collect();

    let mut seq_map: HashMap<String, i64> = HashMap::new();
    for parent_id in &parent_ids {
        let mut children: Vec<(String, u8, i64)> = blocks
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .map(|b| {
                (
                    b.id.to_string(),
                    b.content_type.parse_order_rank(),
                    b.sequence(),
                )
            })
            .collect();
        // Reproduce the store's post-round-trip sibling order: the renderer
        // hoists section content (Source/Image) ahead of headings (Text), and
        // the org parser additionally re-emits all Source blocks before all
        // Image blocks (source loop precedes image loop in `process_headlines`).
        // So the finer `parse_order_rank` (Source < Image < Text) is the primary
        // key — NOT the coarse `sibling_order_group` — then sequence, then ID.
        children.sort_by(|(a_id, a_rank, a_seq), (b_id, b_rank, b_seq)| {
            a_rank
                .cmp(b_rank)
                .then_with(|| a_seq.cmp(b_seq))
                .then_with(|| a_id.cmp(b_id))
        });
        for (i, (id, _, _)) in children.iter().enumerate() {
            seq_map.insert(id.clone(), i as i64);
        }
    }

    for block in blocks.iter_mut() {
        if let Some(&seq) = seq_map.get(block.id.as_str()) {
            block.set_sequence(seq);
        }
    }
}
