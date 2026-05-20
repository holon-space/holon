//! Shared, non-panicking block-equivalence core for the `inv-blocks-match-ref`
//! composite.
//!
//! The composite checks the SAME comparison against every store that holds
//! blocks (Loro, Org, `block_raw`, the `block` matview/live mirror): each store
//! normalises to a `Vec<holon_api::Block>` (the snapshot) and this module
//! compares it to the reference's snapshot. Two facets, both derived from the
//! normalised `Vec<Block>`:
//!
//! - **fields** — `normalize_block` + sort-by-id + `==` (the proven
//!   [`crate::assertions::assert_blocks_equivalent`] rule, but returning a
//!   `Result` instead of unwinding). `normalize_block` already zeroes
//!   `sort_key`/timestamps and strips internal/null/empty properties, so this
//!   compares content, content_type, parent, properties, tags, requires,
//!   task_state, source_language — everything *except* sibling order.
//! - **order** — per-parent sibling order under the renderer's canonical sort
//!   (source/image before text, then `sequence`, then id), the `Result` port of
//!   [`crate::assertions::assert_block_order`].
//!
//! Bodies stay dumb: they pick a store snapshot, a readiness gate, and which
//! facets apply, then call [`compare_blocks`]. The per-store `RunMode` and
//! CDC-lag → `Skipped` decision live in the body/runner, not here.

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_orgmode::models::OrgBlockExt;
use holon_pbt_core::invariant::InvariantResult;

use crate::assertions::normalize_block;

/// Field-equality facet: `Ok(())` when the two snapshots are
/// `normalize_block`-equivalent (id-set + every non-order field), else an
/// `Err` carrying the normalized diff. Mirrors
/// [`crate::assertions::assert_blocks_equivalent`] as a `Result`.
pub fn compare_block_fields(
    label: &str,
    actual: &[Block],
    expected: &[Block],
) -> Result<(), String> {
    let mut actual_sorted: Vec<_> = actual.iter().map(normalize_block).collect();
    let mut expected_sorted: Vec<_> = expected.iter().map(normalize_block).collect();
    actual_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    expected_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    if actual_sorted == expected_sorted {
        return Ok(());
    }
    Err(format!(
        "[{label}] fields diverge from reference\n  \
         {label} (normalized, {} blocks): {actual_sorted:#?}\n  \
         reference (normalized, {} blocks): {expected_sorted:#?}",
        actual_sorted.len(),
        expected_sorted.len(),
    ))
}

/// Ordering facet: per-parent sibling order under the renderer's canonical
/// sort. `Result` port of [`crate::assertions::assert_block_order`] — same
/// canonicalisation (source/image first, then `sequence`, then id), same
/// "only compare when both sides hold the same id set" and "skip all-source
/// sibling groups" guards. Returns the first divergent parent as an `Err`.
pub fn compare_block_order(
    label: &str,
    actual: &[Block],
    expected: &[Block],
) -> Result<(), String> {
    let parent_ids: std::collections::HashSet<EntityUri> =
        actual.iter().map(|b| b.parent_id.clone()).collect();

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
        let mut actual_children: Vec<&Block> = actual
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .collect();
        canonical_sort(&mut actual_children);
        let actual_order: Vec<&str> = actual_children.iter().map(|b| b.id.as_str()).collect();

        let mut ref_children: Vec<&Block> = expected
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

        // Only compare when both sides hold the same id set under this parent.
        if actual_order.len() != ref_order.len()
            || !actual_order.iter().all(|id| ref_order.contains(id))
        {
            continue;
        }
        // Render-artifact exemption via the shared rule. Predicate
        // reconciled to Source|Image (was Source-only here, Source|Image
        // in the children bodies): Image rows are render artifacts with
        // the same reassigned-sort_key behaviour as Source.
        let content_types: std::collections::HashMap<&str, ContentType> = actual_children
            .iter()
            .map(|b| (b.id.as_str(), b.content_type))
            .collect();
        compare_sibling_order(label, parent_id, &ref_order, &actual_order, |id| {
            matches!(content_types[*id], ContentType::Source | ContentType::Image)
        })?;
    }
    Ok(())
}

/// Shared per-parent sibling-order comparison with the render-artifact
/// order exemption. The single home for the exemption rule previously
/// duplicated (with drifted predicates: Source-only here, Source|Image in
/// the children bodies) across `compare_block_order`,
/// `inv-live-children-match-ref` and `inv-loro-children-match-ref`.
///
/// Order is exempt iff the membership (set) is identical AND every child
/// satisfies `exempt` — render artifacts (`::src::`, `::render::`,
/// Source|Image) whose relative order legitimately differs because the
/// stores sort by different keys and a file-sync round trip reassigns
/// sort_keys. Missing / extra / ghost children still fail (set
/// inequality is never exempt).
pub fn compare_sibling_order<T: Ord + std::fmt::Debug>(
    label: &str,
    parent: &dyn std::fmt::Display,
    ref_children: &[T],
    sut_children: &[T],
    exempt: impl Fn(&T) -> bool,
) -> Result<(), String> {
    if ref_children == sut_children {
        return Ok(());
    }
    let ref_set: std::collections::BTreeSet<&T> = ref_children.iter().collect();
    let sut_set: std::collections::BTreeSet<&T> = sut_children.iter().collect();
    if ref_set == sut_set && ref_children.iter().all(exempt) {
        return Ok(());
    }
    Err(format!(
        "[{label}] sibling order diverges under parent {parent}.\n  \
         ref order: {ref_children:?}\n  \
         sut order: {sut_children:?}"
    ))
}

/// Run the field facet (always) and, when `check_order`, the ordering facet.
/// Returns `Fail` on the first divergence, else `Ok`. Stores with a CDC-lag /
/// readiness gate convert their own "not ready" into `Skipped` *before*
/// calling this — the comparison itself never `Skip`s.
pub fn compare_blocks(
    label: &str,
    actual: &[Block],
    expected: &[Block],
    check_order: bool,
) -> InvariantResult {
    if let Err(msg) = compare_block_fields(label, actual, expected) {
        return InvariantResult::Fail(msg);
    }
    if check_order {
        if let Err(msg) = compare_block_order(label, actual, expected) {
            return InvariantResult::Fail(msg);
        }
    }
    InvariantResult::Ok
}

/// A single comparable facet of a block. Used by [`compare_block_subset`] for
/// stores that natively hold only some fields — e.g. `block_raw` has
/// `content`/`properties`/`parent` columns but NOT the junction-derived
/// `tags`/`requires`, so comparing those would always diverge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockFacet {
    Content,
    Properties,
    Parent,
    ContentType,
    SourceLanguage,
}

/// Compare only `facets` (plus the id-set, always) between two snapshots, both
/// `normalize_block`-normalised. For stores that don't natively carry every
/// `Block` field — comparing the full struct would false-fail on fields the
/// store can't represent. Returns `Fail` on the first divergence.
pub fn compare_block_subset(
    label: &str,
    actual: &[Block],
    expected: &[Block],
    facets: &[BlockFacet],
) -> InvariantResult {
    use std::collections::BTreeMap;

    let a: BTreeMap<EntityUri, Block> = actual
        .iter()
        .map(normalize_block)
        .map(|b| (b.id.clone(), b))
        .collect();
    let e: BTreeMap<EntityUri, Block> = expected
        .iter()
        .map(normalize_block)
        .map(|b| (b.id.clone(), b))
        .collect();

    let a_ids: std::collections::BTreeSet<&EntityUri> = a.keys().collect();
    let e_ids: std::collections::BTreeSet<&EntityUri> = e.keys().collect();
    if a_ids != e_ids {
        let missing: Vec<&&EntityUri> = e_ids.difference(&a_ids).collect();
        let spurious: Vec<&&EntityUri> = a_ids.difference(&e_ids).collect();
        return InvariantResult::Fail(format!(
            "[{label}] block id set diverges from reference\n  missing in {label}: {missing:?}\n  \
             spurious in {label}: {spurious:?}"
        ));
    }

    for (id, eb) in &e {
        let ab = &a[id];
        for facet in facets {
            let diverged = match facet {
                BlockFacet::Content => ab.content != eb.content,
                BlockFacet::Properties => ab.properties != eb.properties,
                BlockFacet::Parent => ab.parent_id != eb.parent_id,
                BlockFacet::ContentType => ab.content_type != eb.content_type,
                BlockFacet::SourceLanguage => ab.source_language != eb.source_language,
            };
            if diverged {
                return InvariantResult::Fail(format!(
                    "[{label}] block {id} diverges from reference on {facet:?}\n  \
                     {label}: {ab:#?}\n  reference: {eb:#?}"
                ));
            }
        }
    }
    InvariantResult::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::block::Block;

    fn blk(id: &str, parent: &str, content: &str) -> Block {
        Block::new_text(
            EntityUri::block(id),
            EntityUri::block(parent),
            content.to_string(),
        )
    }

    #[test]
    fn identical_snapshots_match() {
        let a = vec![blk("1", "root", "hello"), blk("2", "root", "world")];
        let b = vec![blk("2", "root", "world"), blk("1", "root", "hello")];
        // order-insensitive on the field facet (sorted by id)
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Ok
        ));
    }

    #[test]
    fn content_divergence_fails() {
        let a = vec![blk("1", "root", "hello")];
        let b = vec![blk("1", "root", "HELLO")];
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Fail(_)
        ));
    }

    #[test]
    fn missing_block_fails() {
        let a = vec![blk("1", "root", "hello")];
        let b = vec![blk("1", "root", "hello"), blk("2", "root", "world")];
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Fail(_)
        ));
    }

    #[test]
    fn subset_ignores_unlisted_facets() {
        // Same id+content, different parent. Subset on [Content] only → Ok,
        // even though parent diverges (the facet isn't compared).
        let mut a = blk("1", "parentA", "hello");
        let mut b = blk("1", "parentB", "hello");
        a.parent_id = EntityUri::block("parentA");
        b.parent_id = EntityUri::block("parentB");
        assert!(matches!(
            compare_block_subset("test", &[a], &[b], &[BlockFacet::Content]),
            InvariantResult::Ok
        ));
    }

    #[test]
    fn subset_catches_listed_facet_and_id_set() {
        let a = vec![blk("1", "root", "hello")];
        let b = vec![blk("1", "root", "WORLD")];
        assert!(matches!(
            compare_block_subset("test", &a, &b, &[BlockFacet::Content]),
            InvariantResult::Fail(_)
        ));
        // id-set always checked even with empty facets
        let a2 = vec![blk("1", "root", "x")];
        let b2 = vec![blk("1", "root", "x"), blk("2", "root", "y")];
        assert!(matches!(
            compare_block_subset("test", &a2, &b2, &[]),
            InvariantResult::Fail(_)
        ));
    }

    #[test]
    fn trailing_whitespace_normalized_away() {
        let a = vec![blk("1", "root", "hello   ")];
        let b = vec![blk("1", "root", "hello")];
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Ok
        ));
    }
}
