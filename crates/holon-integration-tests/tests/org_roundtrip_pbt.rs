//! Org parse-render round-trip PBT — proves the org renderer + parser form
//! a round-trip that converges to a fixed point after one parse-render cycle
//! and drops no structural information on the way through.
//!
//! ## Phase 5 consolidation
//!
//! Generation and comparison are shared with the rest of the round-trip
//! family via `holon-block-roundtrip-testing`:
//! - inputs come from `root_headlines_strategy` + `build_blocks` (the same
//!   block-tree generator `org_block_round_trip_pbt` and
//!   `turso_block_round_trip_pbt` use),
//! - structural equality is `NormalizedDocument` +
//!   `assert_normalized_docs_equal`.
//!
//! The bespoke `GenBlock`/`arb_tree`/`lower_to_blocks` generator and the
//! `StructRow`/`structural_view` comparison are gone. What stays unique to
//! this test is the **render → parse → render fixed-point** assertion
//! (`inv-org-render-fixed-point`): the block round-trips above only prove
//! structure preservation; this test additionally proves the renderer is
//! idempotent after one cycle, exercising `OrgRenderer`/`parse_org_file`
//! directly rather than the public `OrgFormatAdapter` surface.
//!
//! Note: the shared `Vec<Block> in → out` property is expressed as a free
//! function (`assert_normalized_docs_equal`), not as an `Invariant<R, S>` —
//! that trait models proptest-state-machine ref-vs-SUT *steps*, which a
//! stateless render→parse round-trip has none of.
//!
//! Bug classes pinned by the sanity tests (per MEMORY entries):
//! - `:edge_abstraction:` tag drop on round-trip (May 2026)
//! - generic `:PROPERTIES:` drawer keys must survive
//! - headline TODO keyword must survive
//! - nested headline trees must survive

#![cfg(feature = "pbt")]

use std::collections::HashMap;
use std::path::PathBuf;

use holon_api::Block;
use holon_api::EntityUri;
use holon_api::TaskState;
use holon_block_roundtrip_testing::HeadlineSpec;
use holon_block_roundtrip_testing::NormalizedDocument;
use holon_block_roundtrip_testing::PropertiesDrawer;
use holon_block_roundtrip_testing::assert_normalized_docs_equal;
use holon_block_roundtrip_testing::build_blocks;
use holon_block_roundtrip_testing::root_headlines_strategy;
use holon_orgmode::models::OrgDocumentExt;
use holon_orgmode::org_renderer::OrgRenderer;
use holon_orgmode::parser::parse_org_file;
use proptest::prelude::*;

const DOC_ID: &str = "rtdoc";

fn doc_uri() -> EntityUri {
    EntityUri::block(DOC_ID)
}

fn doc_path() -> PathBuf {
    PathBuf::from("/test/rtdoc.org")
}

fn doc_root() -> PathBuf {
    PathBuf::from("/test")
}

fn make_doc_block() -> Block {
    let mut doc = Block::new_text(doc_uri(), EntityUri::no_parent(), "rtdoc".to_string());
    doc.set_page(true);
    doc.set_file_title(Some("rtdoc".to_string()));
    doc
}

/// Render → parse → render and assert both shared properties:
/// 1. `inv-org-render-fixed-point`: the second render equals the first.
/// 2. structure preservation: the parsed tree normalizes equal to the input
///    (via the shared `NormalizedDocument` comparison).
fn run_case(doc: &Block, blocks: &[Block]) -> Result<(), TestCaseError> {
    let path = doc_path();
    let root = doc_root();

    // First render — the canonical org text.
    let rendered_1 = OrgRenderer::render_document(doc, blocks, &path, &doc_uri());

    // Parse it back.
    let parsed =
        parse_org_file(&path, &rendered_1, &EntityUri::no_parent(), &root).map_err(|e| {
            TestCaseError::fail(format!("parse failed: {e}\n--- TEXT ---\n{rendered_1}"))
        })?;

    // Re-render the parsed result and assert the fixed point.
    let rendered_2 =
        OrgRenderer::render_document(&parsed.document, &parsed.blocks, &path, &doc_uri());
    if rendered_1 != rendered_2 {
        return Err(TestCaseError::fail(format!(
            "[inv-org-render-fixed-point] render→parse→render diverged\n--- FIRST \
             ---\n{rendered_1}\n--- SECOND ---\n{rendered_2}"
        )));
    }

    // Structural invariant — parse must preserve the tree we put in.
    let expected = NormalizedDocument::from_blocks(doc.file_title(), blocks);
    let actual = NormalizedDocument::from_blocks(parsed.document.file_title(), &parsed.blocks);
    assert_normalized_docs_equal(&expected, &actual, "org_round_trip")?;
    Ok(())
}

/// Minimal `HeadlineSpec` builder for the fixed-input sanity tests. `level`
/// must match the headline's depth (top-level = 1) so the parser-derived
/// level compares equal.
fn spec(
    id: &str,
    level: i64,
    title: &str,
    tags: &[&str],
    props: &[(&str, &str)],
    task_state: Option<TaskState>,
    child_headlines: Vec<HeadlineSpec>,
) -> HeadlineSpec {
    HeadlineSpec {
        block_id: EntityUri::block(id),
        properties_drawer: PropertiesDrawer {
            explicit_id: None,
            other_props: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
        },
        level,
        task_state,
        priority: None,
        title: title.to_string(),
        tags: (!tags.is_empty()).then(|| tags.iter().map(|t| t.to_string()).collect()),
        body: None,
        scheduled: None,
        deadline: None,
        source_blocks: Vec::new(),
        child_headlines,
    }
}

fn run_sanity(specs: Vec<HeadlineSpec>, msg: &str) {
    let doc = make_doc_block();
    let blocks = build_blocks(&doc.id, &specs);
    run_case(&doc, &blocks).unwrap_or_else(|e| panic!("{msg}: {e:?}"));
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        max_shrink_iters: 200,
        .. ProptestConfig::default()
    })]

    /// Org parse-render fixed-point + structure preservation over the shared
    /// block-tree generator.
    #[test]
    fn org_roundtrip_fixed_point(headlines in root_headlines_strategy()) {
        let doc = make_doc_block();
        let blocks = build_blocks(&doc.id, &headlines);
        run_case(&doc, &blocks)?;
    }
}

// ─────────────────────────────────────────────────────────────────
// Sanity tests — fixed inputs that pin the bug classes named in MEMORY
// so a regression in the parser/renderer shows up as a unit-test
// failure, not just a PBT flake.
// ─────────────────────────────────────────────────────────────────

#[test]
fn sanity_edge_abstraction_tag_roundtrips() {
    // The May 2026 :edge_abstraction: drop case.
    run_sanity(
        vec![spec(
            "n0",
            1,
            "headline",
            &["edge_abstraction"],
            &[],
            None,
            vec![],
        )],
        "edge_abstraction tag must survive round-trip",
    );
}

#[test]
fn sanity_property_drawer_roundtrips() {
    run_sanity(
        vec![spec(
            "n0",
            1,
            "headline",
            &[],
            &[("MY_KEY", "hello")],
            None,
            vec![],
        )],
        "drawer property must survive round-trip",
    );
}

#[test]
fn sanity_todo_keyword_roundtrips() {
    run_sanity(
        vec![spec(
            "n0",
            1,
            "task",
            &[],
            &[],
            Some(TaskState::active("TODO")),
            vec![],
        )],
        "TODO keyword must survive round-trip",
    );
}

#[test]
fn sanity_nested_tree_roundtrips() {
    run_sanity(
        vec![spec(
            "n0",
            1,
            "parent",
            &[],
            &[],
            None,
            vec![spec("n1", 2, "child", &["work"], &[], None, vec![])],
        )],
        "nested tree must survive round-trip",
    );
}
