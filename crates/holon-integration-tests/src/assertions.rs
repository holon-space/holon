//! Assertion helpers for block comparison

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_orgmode::models::OrgBlockExt;

use crate::org_utils::INTERNAL_PROPS;

/// Normalize a block for comparison by zeroing out timestamps and trimming content.
///
/// Page URIs in parent_id are normalized to a canonical form so that
/// file-based URIs (file:test.org) and UUID-based URIs (block:{uuid})
/// for the same page compare equal.
pub fn normalize_block(block: &Block) -> Block {
    let mut normalized = block.clone();
    normalized.created_at = 0;
    normalized.updated_at = 0;
    // sort_key is no longer a field of the domain Block (ADR 0005) — ordering is
    // validated separately via `assert_block_order` / `children_of`.
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
    // The `__default__` page is prod's layout-owning root container (a real
    // block, not the sentinel — see `default_doc_block_uri`). The reference
    // model represents the document root as `__document_root__` and parents the
    // layout straight to it, so unify the two roots here.
    if normalized.parent_id.is_no_parent()
        || normalized.parent_id.is_sentinel()
        || normalized.parent_id == holon_api::default_doc_block_uri()
    {
        normalized.parent_id = holon_api::EntityUri::block("__document_root__");
    }
    // document_id removed from Block struct; no normalization needed
    for prop in INTERNAL_PROPS {
        normalized.properties.remove(*prop);
    }
    // Strip Null-valued and empty-string properties: the org parser stores
    // task_state=Null explicitly in the DB but the reference model omits absent
    // properties. Empty-string task_state means "no state" and is lost during
    // org round-trip (not written as a keyword, so not parsed back).
    normalized.properties.retain(|_, v| match v {
        holon_api::Value::Null => false,
        holon_api::Value::String(s) if s.is_empty() => false,
        _ => true,
    });
    // `task_state_category` is `task_state`'s sidecar — without a (non-empty)
    // keyword it carries no information, and the org round-trip drops the PAIR
    // (no keyword rendered → neither parsed back). The retain above already
    // dropped an empty/Null keyword; drop its orphaned sidecar with it, or the
    // ref (which stores ""+"active" after cycling to Clear, exactly like
    // block_raw) diverges from the org-parsed side on a phantom property.
    if !normalized.properties.contains_key("task_state") {
        normalized.properties.remove("task_state_category");
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
