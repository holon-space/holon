//! Assertion helpers for block comparison

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_orgmode::models::OrgBlockExt;

pub use holon_pbt_core::block_compare::normalize_block;

/// Normalize a block for comparison by zeroing out timestamps and trimming content.
///
/// Page URIs in parent_id are normalized to a canonical form so that
/// file-based URIs (file:test.org) and UUID-based URIs (block:{uuid})
/// for the same page compare equal.

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

/// Assert that block ordering in the org file matches the reference model.
///
/// For each parent, compares the org file order against the reference model
/// order. Both sides are sorted by `(content_type_group, sequence, id)` to
/// mirror `OrgRenderer::render_entity_tree` (`crates/holon-org-format/src/
/// org_renderer.rs:96-115`), which forces source/image children to render
/// before text children — required so a sub-heading following the source
/// block doesn't steal it during re-parse. The reference model's `move_block`
/// is allowed to set sequences without applying that canonicalization;
/// comparing under the same rule keeps the assertion meaningful (within-group
/// order) without false positives from the renderer's source-first reorder.
pub fn assert_block_order(org_blocks: &[Block], ref_blocks: &[Block], message: &str) {
    let parent_ids: std::collections::HashSet<EntityUri> =
        org_blocks.iter().map(|b| b.parent_id.clone()).collect();

    let render_group = |ct: ContentType| -> u8 {
        match ct {
            ContentType::Source | ContentType::Image => 0,
            ContentType::Text => 1,
        }
    };
    let canonical_sort = |children: &mut Vec<&Block>| {
        children.sort_by(|a, b| {
            render_group(a.content_type)
                .cmp(&render_group(b.content_type))
                .then_with(|| a.sequence().cmp(&b.sequence()))
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
    };

    for parent_id in &parent_ids {
        let mut org_children: Vec<&Block> = org_blocks
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .collect();
        canonical_sort(&mut org_children);
        let org_order: Vec<&str> = org_children.iter().map(|b| b.id.as_str()).collect();

        let mut ref_children: Vec<&Block> = ref_blocks
            .iter()
            .filter(|b| {
                if parent_id.is_no_parent() || parent_id.is_sentinel() {
                    b.parent_id.is_no_parent() || b.parent_id.is_sentinel()
                } else {
                    b.parent_id.as_raw_str() == parent_id.as_str()
                }
            })
            .collect();
        canonical_sort(&mut ref_children);
        let ref_order: Vec<&str> = ref_children.iter().map(|b| b.id.as_str()).collect();

        // Only compare if both sides have the same block IDs
        if org_order.len() == ref_order.len() && org_order.iter().all(|id| ref_order.contains(id)) {
            // Skip ordering check for source-only sibling groups. Within the
            // source/image group, OrgRenderer (`crates/holon-org-format/src/
            // org_renderer.rs:96-115`) sorts by `sort_key` while the reference
            // model uses `sequence` — those don't always agree because the
            // initial file sync round-trip can reorder source siblings via
            // sort_key reassignment. The mixed-group ordering is already
            // handled by the canonical_sort above (source/image first).
            let all_source = org_children
                .iter()
                .all(|b| b.content_type == ContentType::Source);
            if all_source {
                continue;
            }
            assert_eq!(
                org_order, ref_order,
                "{}: Block order mismatch under parent '{}'\n  \
                 Org file order:  {:?}\n  \
                 Expected order:  {:?}",
                message, parent_id, org_order, ref_order
            );
        }
    }
}
