//! Repro: `get_blocks`' `ORDER BY sort_key, id` is a GLOBAL sort that
//! interleaves sibling groups, so its returned sequence is NOT a document
//! order. Fractional indices are minted per sibling group, so every group
//! restarts at the same low key ("80" in the live vault: 1861/1889 blocks
//! share a sort_key with another row) and cross-parent ties fall back to the
//! `id` tiebreak.
//!
//! Both halves matter for the Option C Inc-2 cutover:
//!   1. the flat sequence differs from hierarchical document order, so
//!      comparing it against a pre-order walk is invalid by construction;
//!   2. the flat sequence nonetheless preserves WITHIN-PARENT relative order,
//!      which is the only thing prod consumers read — so org rendering is
//!      byte-identical either way and this is NOT a prod ordering bug.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::OrgRenderer;

/// The four blocks of the `stale_rewrite_sibling_order` shape: three children
/// of the file root plus ONE grandchild under `auto-create`.
fn fixture(file_id: &EntityUri) -> (Block, Block, Block, Block) {
    let src = Block::new_source(
        EntityUri::block("journals::src::0"),
        file_id.clone(),
        "prql",
        "from blocks",
    );
    let render = Block::new_source(
        EntityUri::block("journals::render::0"),
        file_id.clone(),
        "render",
        "list(#{})",
    );
    let heading = Block::new_text(
        EntityUri::block("journals::auto-create"),
        file_id.clone(),
        "Journal Auto-Create",
    );
    let rule = Block::new_source(
        EntityUri::block("journals::action::0"),
        EntityUri::block("journals::auto-create"),
        "holon_rule",
        "name: daily_journal",
    );
    (src, render, heading, rule)
}

/// Per-sibling-group fractional indices, exactly as the Loro projector mints
/// them: each group independently starts at "80" and steps by one hex unit.
fn per_parent_sort_key(id: &str) -> &'static str {
    match id {
        // children of the file root, in document order
        "block:journals::src::0" => "80",
        "block:journals::render::0" => "8180",
        "block:journals::auto-create" => "8280",
        // the ONLY child of `auto-create` — its own group, so it restarts at 80
        "block:journals::action::0" => "80",
        other => panic!("unexpected id {other}"),
    }
}

/// `get_blocks`' exact comparator: `ORDER BY sort_key, id`, global over the
/// whole doc-scoped descendant set (no parent/depth term).
fn get_blocks_order(blocks: &[Block]) -> Vec<String> {
    let mut v: Vec<&Block> = blocks.iter().collect();
    v.sort_by(|a, b| {
        per_parent_sort_key(a.id.as_str())
            .cmp(per_parent_sort_key(b.id.as_str()))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });
    v.into_iter().map(|b| b.id.as_str().to_string()).collect()
}

#[test]
fn flat_sort_key_order_pulls_a_grandchild_to_the_front() {
    let file_id = EntityUri::block("journals");
    let (src, render, heading, rule) = fixture(&file_id);
    let doc_order = [src.clone(), render.clone(), heading.clone(), rule.clone()];

    // Hierarchical document order (what `BlockOrdering::children` + a pre-order
    // walk yields, and what `LoroBlockReader::collect_subtree` returns).
    let hierarchical: Vec<String> = doc_order
        .iter()
        .map(|b| b.id.as_str().to_string())
        .collect();
    assert_eq!(
        hierarchical,
        vec![
            "block:journals::src::0",
            "block:journals::render::0",
            "block:journals::auto-create",
            "block:journals::action::0",
        ]
    );

    // `get_blocks` scrambles it: `action::0` is a DEPTH-2 block whose own
    // group's key "80" ties with depth-1 `src::0`, and it wins the `id`
    // tiebreak. This is the reported divergence, exactly.
    assert_eq!(
        get_blocks_order(&doc_order),
        vec![
            "block:journals::action::0",
            "block:journals::src::0",
            "block:journals::render::0",
            "block:journals::auto-create",
        ],
        "cross-parent sort_key tie must reorder the flat sequence"
    );
}

#[test]
fn within_parent_relative_order_survives_so_rendering_is_unaffected() {
    let file_id = EntityUri::block("journals");
    let (src, render, heading, rule) = fixture(&file_id);

    let doc_order = [src.clone(), render.clone(), heading.clone(), rule.clone()];
    // The same set in `get_blocks`' flat order.
    let flat_order = [rule, src, render, heading];

    let from_doc_order =
        OrgRenderer::render_entitys(&doc_order, Path::new("/test/Journals.org"), &file_id);
    let from_flat_order =
        OrgRenderer::render_entitys(&flat_order, Path::new("/test/Journals.org"), &file_id);

    assert_eq!(
        from_doc_order, from_flat_order,
        "the renderer re-nests by parent_id and only reads within-parent \
         relative order, which the global sort preserves — so the scrambled \
         flat sequence is NOT a prod ordering bug"
    );
}
