//! Pins the renderer's re-nesting contract, which Option C Inc 2 makes
//! load-bearing (docs/Plans/option-c-holder-design.md §10.2.3).
//!
//! `OrgRenderer::render_walk` builds a `parent_id -> [children]` index from the
//! input slice and walks it from the file root. The ONLY ordering it reads is
//! the relative order of blocks sharing a parent; the cross-parent sequence of
//! the input is discarded. Today `doc_blocks` hands it `get_blocks`' flat
//! global `ORDER BY sort_key, id`; after cutover the holder hands it a genuine
//! document order. Byte-identity across those two shapes holds only because of
//! this contract, so it is asserted here directly rather than inferred from one
//! pair of orders:
//!
//!   * three input permutations that all preserve within-parent relative order
//!     (document pre-order, the flat global sort, and an adversarial
//!     depth-inverted shuffle) render byte-identically;
//!   * one permutation that SWAPS two siblings renders differently — the teeth,
//!     proving the assertion can fail if a renderer change ever starts trusting
//!     (or ignoring) input sequence.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::OrgRenderer;

const FILE: &str = "renest";

fn root() -> EntityUri {
    EntityUri::block(FILE)
}

fn uri(id: &str) -> EntityUri {
    EntityUri::block(&format!("{FILE}::{id}"))
}

/// Four sibling groups over three levels of depth:
///
/// ```text
/// <file root>
///   alpha              beta                gamma
///     alpha-1            beta-1
///       alpha-1-x        beta-2
///     alpha-2            beta-3
///                        beta-src (Source)
/// ```
///
/// `gamma` is a childless leaf so a group of size 1 and a group of size 0 are
/// both represented; `alpha-1-x` puts a block at depth 3. `beta`'s trailing
/// Source child exercises the one re-ordering the renderer DOES impose — a
/// stable sort pulling Source/Image ahead of Text within a sibling group — so
/// the identity assertions cover a fixture where input order and output order
/// genuinely differ.
fn document_order() -> Vec<Block> {
    let root = root();
    let text = |id: &str, parent: EntityUri| Block::new_text(uri(id), parent, format!("body {id}"));
    vec![
        text("alpha", root.clone()),
        text("alpha-1", uri("alpha")),
        text("alpha-1-x", uri("alpha-1")),
        text("alpha-2", uri("alpha")),
        text("beta", root.clone()),
        text("beta-1", uri("beta")),
        text("beta-2", uri("beta")),
        text("beta-3", uri("beta")),
        Block::new_source(uri("beta-src"), uri("beta"), "prql", "from blocks"),
        text("gamma", root),
    ]
}

fn render(blocks: &[Block]) -> String {
    OrgRenderer::render_entitys(blocks, Path::new("/test/Renest.org"), &root())
}

/// Reorder `document_order()` by id, panicking on any id that is not in the
/// fixture or is listed twice — a permutation that silently dropped a block
/// would make byte-identity vacuous.
fn permute(order: &[&str]) -> Vec<Block> {
    let blocks = document_order();
    assert_eq!(
        order.len(),
        blocks.len(),
        "permutation must list every fixture block exactly once"
    );
    let mut out = Vec::with_capacity(blocks.len());
    for id in order {
        let want = uri(id);
        let block = blocks
            .iter()
            .find(|b| b.id == want)
            .unwrap_or_else(|| panic!("permutation names unknown block {id}"));
        assert!(
            !out.iter().any(|b: &Block| b.id == block.id),
            "permutation lists {id} twice"
        );
        out.push(block.clone());
    }
    out
}

/// `get_blocks`' comparator, reproduced: fractional indices are minted PER
/// SIBLING GROUP, so every group restarts at "80" and cross-parent ties fall to
/// the `id` tiebreak. The resulting sequence interleaves depths — see
/// `get_blocks_flat_order_is_not_document_order.rs` for the live-vault
/// evidence.
fn flat_global_sort() -> Vec<Block> {
    fn key(id: &str) -> &'static str {
        match id {
            "alpha" | "alpha-1" | "alpha-1-x" | "beta-1" => "80",
            "beta" | "alpha-2" | "beta-2" => "8180",
            "gamma" | "beta-3" => "8280",
            "beta-src" => "8380",
            other => panic!("no sort_key for {other}"),
        }
    }
    let short = |b: &Block| {
        b.id.as_str()
            .trim_start_matches("block:")
            .trim_start_matches(FILE)
            .trim_start_matches("::")
            .to_string()
    };
    let mut blocks = document_order();
    blocks.sort_by(|a, b| {
        key(&short(a))
            .cmp(key(&short(b)))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });
    blocks
}

#[test]
fn flat_global_sort_really_scrambles_across_parents() {
    // Guards the fixture itself: if the sort_key table ever degenerated into
    // document order, the identity assertion below would prove nothing.
    let flat: Vec<String> = flat_global_sort()
        .iter()
        .map(|b| b.id.as_str().to_string())
        .collect();
    let doc: Vec<String> = document_order()
        .iter()
        .map(|b| b.id.as_str().to_string())
        .collect();
    assert_ne!(
        flat, doc,
        "the flat global sort must differ from document order or this fixture tests nothing"
    );
}

#[test]
fn rendering_ignores_cross_parent_input_order() {
    let from_doc_order = render(&document_order());
    assert!(
        from_doc_order.contains("alpha-1-x"),
        "fixture must actually render its depth-3 block: {from_doc_order}"
    );

    let from_flat_sort = render(&flat_global_sort());
    assert_eq!(
        from_doc_order, from_flat_sort,
        "the renderer re-nests by parent_id, so `get_blocks`' flat global sort and a document \
         pre-order must render byte-identically"
    );

    // Adversarial: deepest-first, groups fully separated and emitted in reverse
    // depth order, with every child listed before its own parent. Within each
    // sibling group the relative order is untouched.
    let adversarial = permute(&[
        "alpha-1-x", // depth 3
        "beta-1",
        "beta-2",
        "beta-3",
        "beta-src", // depth 2, in group order
        "alpha-1",
        "alpha-2", // depth 2, in group order
        "alpha",
        "beta",
        "gamma", // depth 1, in group order
    ]);
    assert_eq!(
        from_doc_order,
        render(&adversarial),
        "an input where every child precedes its parent must still render identically — the \
         renderer reads within-parent relative order and nothing else"
    );
}

#[test]
fn swapping_two_siblings_changes_the_output() {
    // The teeth. If this ever passes, the two assertions above are vacuous:
    // the renderer would be deriving order from something other than the input
    // sequence (a per-block key), which is exactly the regression §10.2.3
    // exists to catch.
    let swapped = permute(&[
        "alpha",
        "alpha-1",
        "alpha-1-x",
        "alpha-2",
        "beta",
        "beta-2", // <- beta-1 and beta-2
        "beta-1", // <- swapped within their group
        "beta-3",
        "beta-src",
        "gamma",
    ]);
    assert_ne!(
        render(&document_order()),
        render(&swapped),
        "within-parent relative order IS load-bearing: swapping two siblings must change the org \
         output, otherwise the byte-identity assertions prove nothing"
    );
}
