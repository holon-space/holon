//! Cross-cutting tests of the frontend slice. The headline: the shared catalog's
//! `SutViewSelection`/`SutRenderer` invariants run over a **real** headless
//! `ReactiveEngine` (the production CDC→watch→interpret render path, windowless),
//! and the `SutBackend` block-tree invariants run over the same engine's
//! `block_raw` — a fourth realization backing the same catalog, zero duplication.

use std::sync::Arc;
use std::time::Duration;

use crate::pbt::composed::fixtures::*;
use crate::pbt::frontend_slice::builders::frontend_wide;
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;

const NOTES_ORG: &str = "\
* First note
* Second note
";

async fn new_component() -> Arc<HeadlessFrontendComponent> {
    Arc::new(
        HeadlessFrontendComponent::new(&[("notes.org", NOTES_ORG)], Duration::from_millis(300))
            .await,
    )
}

/// E1 make-or-break PROBE (Step-A): does `SutWatchRows` over the PRODUCTION reactive
/// watch surface actually deliver rows in the windowless session? Register an
/// `AllBlocks` query watch through `register_query_watch` (→
/// `ReactiveEngine::watch_query_live`, the real CDC pump), settle, and read it back
/// through the `SutWatchRows` cap. If this returns the seeded blocks, the redesign
/// (read the real `ReactiveRenderedRows`, not E2ESut's bespoke `ui_model`) is viable.
#[tokio::test]
async fn frontend_slice_watch_rows_deliver_over_production_reactive_surface() {
    use crate::pbt::query::{QuerySource, QueryTable, TestQuery};
    use holon_api::QueryLanguage;
    use holon_pbt_core::capabilities::SutWatchRows;

    let comp = new_component().await;
    let query = TestQuery {
        table: QueryTable::Blocks,
        columns: vec!["id".to_string()],
        predicates: vec![],
        source: QuerySource::AllBlocks,
    };
    // Diagnostic: is the `block` MATVIEW (what the watch query reads) hydrated in
    // the headless session, vs the `block_raw` write-side table?
    let raw_ids = comp.block_raw_query_ids("SELECT id FROM block_raw").await;
    let matview_ids = comp.block_raw_query_ids("SELECT id FROM block").await;
    eprintln!(
        "[watch-probe] block_raw has {} rows; block matview has {} rows",
        raw_ids.len(),
        matview_ids.len()
    );

    comp.register_query_watch("query-probe", &query, QueryLanguage::HolonSql);

    let ids = comp.watch_query_ids().await;
    assert_eq!(
        ids,
        vec!["query-probe".to_string()],
        "the component must report exactly the registered watch query-id"
    );

    let rows = comp.watch_rows("query-probe").await;
    eprintln!(
        "[watch-probe] AllBlocks watch delivered {} rows over the production reactive \
         surface; sample={:?}",
        rows.len(),
        rows.first()
    );
    assert!(
        !rows.is_empty(),
        "the production reactive watch must deliver the seeded blocks' rows headlessly \
         (else SutWatchRows can't read the real ReactiveRenderedRows); got 0 rows"
    );
    // Every row must carry an `id` column (the AllBlocks projection).
    assert!(
        rows.iter().all(|r| r.get("id").is_some()),
        "every watched row must have an id column; rows={rows:?}"
    );
}

/// The deep `/viewmodel` content oracle, headless edition: graft the fixed
/// `parent`/`c1`/`c2` tree under the Main focus root (`block:journals`), seed the
/// ref with the same ids+content, and prove `inv-displayed-text/viewmodel`
/// *compares the nested Main-panel content* (not just panel chrome) — reaching
/// `Ok` clean and `Fail` on a planted `c1` divergence. This works because the
/// component resolves the FULL tree via the engine's recursive `snapshot` warmed
/// to a fixed point (rich ViewModel: no window needed). `/widget` is deselected
/// (no `SutLayout` geometry headless), so `/viewmodel` carries it.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_displayed_text_viewmodel_bites_on_nested_content() {
    use crate::pbt::composed::seed_primitives::{
        C1, C2, PARENT, Plant, apply_plant, fixed_ids, seed_ref_tree,
    };
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::state_machine::fresh_reference_state;
    use holon_pbt_core::invariant::InvariantResult;

    let comp = new_component().await;
    let ids = fixed_ids();
    comp.create_block(ids.parent.as_str(), "block:journals", PARENT)
        .await;
    comp.create_block(ids.c1.as_str(), ids.parent.as_str(), C1)
        .await;
    comp.create_block(ids.c2.as_str(), ids.parent.as_str(), C2)
        .await;
    let sut = frontend_wide(comp);

    let seeded = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        seed_ref_tree(&mut s);
        s
    };
    // This test scopes to the `/viewmodel` content arm. The component also
    // provides `SutBackend`, so the block-tree-vs-ref invariants
    // (`inv-blocks-match-ref`, `inv-block-parent-matches-ref`) also select against
    // this *partial* ref (which knows only parent/c1/c2, not the whole booted
    // vault) and legitimately diverge — those need a full-vault oracle and are
    // covered elsewhere. So we assert the `/viewmodel` arm directly, not overall
    // emptiness. `/viewmodel` skips ref-unknown blocks, so the partial ref is fine.
    // (`run_with_seeded_ref` drops the `ReferenceState`'s tokio runtime off-thread.)
    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(seeded),
    )
    .await;
    let vm_result = report
        .ran
        .iter()
        .find(|(id, _)| id.0 == "inv-displayed-text/viewmodel")
        .map(|(_, r)| r.clone());
    assert!(
        matches!(vm_result, Some(InvariantResult::Ok)),
        "headless /viewmodel must reach Ok over the grafted nested content (NOT \
         Skipped — that would mean the recursive snapshot didn't resolve the \
         Main-panel content), got {vm_result:?}; ran={:?}",
        report.ran_ids(),
    );

    let planted = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        seed_ref_tree(&mut s);
        apply_plant(&mut s, Plant::Content);
        s
    };
    let planted_report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(planted),
    )
    .await;
    let failed: Vec<&str> = planted_report
        .failures()
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert!(
        failed.contains(&"inv-displayed-text/viewmodel"),
        "planted c1 divergence must make headless /viewmodel FAIL (the oracle \
         bites on nested content); failures={:?}",
        planted_report.failures(),
    );
}

/// E1 teeth: the relocated `SutWatchRows` (production reactive watch surface) makes
/// the B5 watch invariants **bite** on the composition path. Graft fixed
/// `parent`/`c1`/`c2` (c1,c2 are direct children of `parent`), register a
/// `DirectChildren(parent)` watch projecting only `id`, and seed the ref with a
/// matching `active_watches` entry + block tree:
///   - clean → `inv-active-watches-match-ref` (id-set agrees) AND
///     `inv-watch-rows-match-ref` (the watched children {c1,c2} agree) both `Ok`.
///   - drop c2 from the ref → the watch's id-set diverges, block_raw truth
///     ({c1,c2}) ≠ the ref ({c1}) → `inv-watch-rows-match-ref` `Fail` (a real
///     write-pipeline divergence, not a CDC-lag skip).
///   - mismatched watch id on the ref → `inv-active-watches-match-ref` `Fail`.
/// `columns:[id]` keeps the row check to the id-set (no parent_id/content
/// alignment), so the oracle is exact.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_watch_invariants_bite_over_production_watches() {
    use crate::pbt::composed::seed_primitives::{C1, C2, PARENT, fixed_ids, seed_ref_tree};
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::query::{QuerySource, QueryTable, TestQuery, WatchSpec};
    use crate::pbt::state_machine::fresh_reference_state;
    use holon_api::QueryLanguage;
    use holon_pbt_core::invariant::InvariantResult;

    const WATCH_ID: &str = "query-children";

    let comp = new_component().await;
    let ids = fixed_ids();
    comp.create_block(ids.parent.as_str(), "block:journals", PARENT)
        .await;
    comp.create_block(ids.c1.as_str(), ids.parent.as_str(), C1)
        .await;
    comp.create_block(ids.c2.as_str(), ids.parent.as_str(), C2)
        .await;

    // Register a production reactive watch for parent's direct children, id only.
    let children_query = TestQuery {
        table: QueryTable::Blocks,
        columns: vec!["id".to_string()],
        predicates: vec![],
        source: QuerySource::DirectChildren {
            context: ids.parent.clone(),
        },
    };
    comp.register_query_watch(WATCH_ID, &children_query, QueryLanguage::HolonSql);
    let sut = frontend_wide(comp);

    let watch_spec = || WatchSpec {
        query: children_query.clone(),
        language: QueryLanguage::HolonSql,
    };
    let result_of = |report: &holon_pbt_core::composition::RunReport, id: &str| {
        report
            .ran
            .iter()
            .find(|(rid, _)| rid.0 == id)
            .map(|(_, r)| r.clone())
    };

    // ── clean: SUT watch {c1,c2} == ref expected {c1,c2}; watch ids agree ──
    let clean = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        seed_ref_tree(&mut s);
        s.mcp
            .active_watches
            .insert(WATCH_ID.to_string(), watch_spec());
        s
    };
    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(clean),
    )
    .await;
    assert!(
        matches!(
            result_of(&report, "inv-active-watches-match-ref"),
            Some(InvariantResult::Ok)
        ),
        "active-watches must agree (sut {{{WATCH_ID}}} == ref {{{WATCH_ID}}}); got {:?}; ran={:?}",
        result_of(&report, "inv-active-watches-match-ref"),
        report.ran_ids(),
    );
    assert!(
        matches!(
            result_of(&report, "inv-watch-rows-match-ref"),
            Some(InvariantResult::Ok)
        ),
        "watch-rows must reach Ok over the production watch (children {{c1,c2}} agree); \
         got {:?}; ran={:?}",
        result_of(&report, "inv-watch-rows-match-ref"),
        report.ran_ids(),
    );
    eprintln!(
        "[watch-teeth] clean: inv-active-watches-match-ref + inv-watch-rows-match-ref both Ok"
    );

    // ── catch A: ref drops c2 → watch id-set diverges → watch-rows Fail ──
    let dropped_child = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        seed_ref_tree(&mut s);
        s.domain.block_state.blocks.remove(&ids.c2);
        s.mcp
            .active_watches
            .insert(WATCH_ID.to_string(), watch_spec());
        s
    };
    let dropped_report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(dropped_child),
    )
    .await;
    let dropped_failures: Vec<&str> = dropped_report
        .failures()
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert!(
        dropped_failures.contains(&"inv-watch-rows-match-ref"),
        "dropping c2 from the ref must make inv-watch-rows-match-ref FAIL (block_raw \
         truth {{c1,c2}} ≠ ref {{c1}}, not a CDC-lag skip); failures={dropped_failures:?}",
    );

    // ── catch B: ref watches a DIFFERENT id → active-watches Fail ──
    let wrong_watch = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        seed_ref_tree(&mut s);
        s.mcp
            .active_watches
            .insert("query-OTHER".to_string(), watch_spec());
        s
    };
    let wrong_report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(wrong_watch),
    )
    .await;
    let wrong_failures: Vec<&str> = wrong_report.failures().iter().map(|(id, _)| *id).collect();
    assert!(
        wrong_failures.contains(&"inv-active-watches-match-ref"),
        "a mismatched watch id on the ref must make inv-active-watches-match-ref FAIL; \
         failures={wrong_failures:?}",
    );
    eprintln!(
        "[watch-teeth] catch: watch-rows fails on dropped child, active-watches fails on \
         mismatched id — both B5 invariants bite on the production reactive surface"
    );
}

/// INC 3 teeth: the `SetupWatch` transition, decomposed onto the `SutWatchRegister`
/// cap, registers the watch by driving the **composed `CapMap`** through
/// `apply_to_sut` — the same path the wide PBT's `StateMachineTest` would use —
/// rather than calling the `register_query_watch` test helper directly. This
/// proves the decomposition end-to-end: a watch set up through the composed cap
/// makes the B5 watch invariants bite exactly as a directly-registered one does.
///   - clean → `inv-watch-rows-match-ref` `Ok` (the composed-registered watch's
///     children {c1,c2} agree with the ref).
///   - drop c2 from the ref → `inv-watch-rows-match-ref` `Fail` (real divergence).
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_setup_watch_via_cap_makes_invariants_bite() {
    use crate::pbt::composed::seed_primitives::{C1, C2, PARENT, fixed_ids, seed_ref_tree};
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::query::{QuerySource, QueryTable, TestQuery, WatchSpec};
    use crate::pbt::state_machine::fresh_reference_state;
    use crate::pbt::transitions::SetupWatch;
    use holon_api::QueryLanguage;
    use holon_pbt_core::TransitionImpl;
    use holon_pbt_core::invariant::InvariantResult;

    const WATCH_ID: &str = "query-children";

    let comp = new_component().await;
    let ids = fixed_ids();
    comp.create_block(ids.parent.as_str(), "block:journals", PARENT)
        .await;
    comp.create_block(ids.c1.as_str(), ids.parent.as_str(), C1)
        .await;
    comp.create_block(ids.c2.as_str(), ids.parent.as_str(), C2)
        .await;

    let children_query = TestQuery {
        table: QueryTable::Blocks,
        columns: vec!["id".to_string()],
        predicates: vec![],
        source: QuerySource::DirectChildren {
            context: ids.parent.clone(),
        },
    };

    let mut sut = frontend_wide(comp);
    let setup = SetupWatch {
        query_id: WATCH_ID.to_string(),
        query: children_query.clone(),
        language: QueryLanguage::HolonSql,
    };

    let watch_spec = || WatchSpec {
        query: children_query.clone(),
        language: QueryLanguage::HolonSql,
    };
    let result_of = |report: &holon_pbt_core::composition::RunReport, id: &str| {
        report
            .ran
            .iter()
            .find(|(rid, _)| rid.0 == id)
            .map(|(_, r)| r.clone())
    };

    // ── clean: composed-registered watch {c1,c2} == ref expected {c1,c2} ──
    let clean = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        seed_ref_tree(&mut s);
        s.mcp
            .active_watches
            .insert(WATCH_ID.to_string(), watch_spec());
        s
    };
    // Drive `SetupWatch` through the composed `CapMap`'s `SutWatchRegister` adapter
    // (NOT `comp.register_query_watch`): the transition compiles the query at the
    // boundary and calls `caps.register_watch(..)`, which forwards to the same
    // production `watch_query_live` surface. This is the decomposed drive path.
    // `SetupWatch::apply_to_sut` ignores the ref, so we borrow `clean` here (it is
    // then moved into `run_with_seeded_ref`, which drops it off-thread — a
    // `ReferenceState` owns a `tokio::Runtime` that panics if dropped inline in an
    // async context). The watch persists on the component for the later arm too.
    setup.apply_to_sut(&clean, &mut sut).await;
    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(clean),
    )
    .await;
    assert!(
        matches!(
            result_of(&report, "inv-watch-rows-match-ref"),
            Some(InvariantResult::Ok)
        ),
        "watch-rows must reach Ok over the SetupWatch-via-cap registered watch; got {:?}; ran={:?}",
        result_of(&report, "inv-watch-rows-match-ref"),
        report.ran_ids(),
    );

    // ── catch: ref drops c2 → watch id-set diverges → watch-rows Fail ──
    let dropped_child = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        seed_ref_tree(&mut s);
        s.domain.block_state.blocks.remove(&ids.c2);
        s.mcp
            .active_watches
            .insert(WATCH_ID.to_string(), watch_spec());
        s
    };
    let dropped_report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(dropped_child),
    )
    .await;
    let dropped_failures: Vec<&str> = dropped_report
        .failures()
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert!(
        dropped_failures.contains(&"inv-watch-rows-match-ref"),
        "dropping c2 from the ref must make inv-watch-rows-match-ref FAIL for the \
         SetupWatch-via-cap registered watch; failures={dropped_failures:?}",
    );
    eprintln!(
        "[setup-watch-cap-teeth] SetupWatch driven through the composed CapMap registers a \
         watch that makes inv-watch-rows-match-ref bite (Ok clean, Fail on dropped child)"
    );
}

/// E1 PROBE: `SutOrgRead::org_block_snapshot` parses the on-disk org files back
/// into blocks via the production `holon_orgmode` parser. Prints the parsed blocks
/// so I can see the id/parent shape before aligning a ref for `inv-blocks-match-ref/org`.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_org_read_parses_on_disk_files() {
    use holon_pbt_core::capabilities::SutOrgRead;

    use std::collections::BTreeSet;
    let comp = new_component().await;
    let blocks = comp.org_block_snapshot().await;
    eprintln!(
        "[org-probe] org_block_snapshot returned {} blocks",
        blocks.len()
    );
    for b in &blocks {
        eprintln!("[org-probe]   {b:?}");
    }
    assert!(
        !blocks.is_empty(),
        "parsing the seed org file must yield blocks (org→block parse path)"
    );
    // Stability: a second parse of the same files yields the same id set (the
    // parser is deterministic for these files — required for ref alignment).
    let again = comp.org_block_snapshot().await;
    let ids1: BTreeSet<String> = blocks.iter().map(|b| b.id.to_string()).collect();
    let ids2: BTreeSet<String> = again.iter().map(|b| b.id.to_string()).collect();
    eprintln!("[org-probe] re-parse id-set stable: {}", ids1 == ids2);
}

/// E1 PROBE (make-or-break): does `SutOrgRender::snapshot_org_render_pairs` render
/// each tracked org file from the headless SQL state so it matches the on-disk bytes
/// (the render-fixed-point the `FileSyncController`'s echo-suppression depends on)?
/// If disk == rendered for every pair, the production render path works headlessly.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_org_render_pairs_reach_fixed_point() {
    use holon_pbt_core::capabilities::SutOrgRender;

    let comp = new_component().await;
    let pairs = comp.snapshot_org_render_pairs().await;
    eprintln!("[org-render-probe] {} render pairs", pairs.len());
    assert!(!pairs.is_empty(), "must render ≥1 tracked org file");
    for (path, disk, rendered) in &pairs {
        let matches = disk == rendered;
        eprintln!("[org-render-probe]   {path}: disk==rendered = {matches}");
        if !matches {
            eprintln!("[org-render-probe]   --- disk ---\n{disk}\n--- rendered ---\n{rendered}");
        }
    }
}

/// E1 teeth: the relocated `SutOrgRender` (production `CacheBlockReader` +
/// `OrgRenderer`) makes `inv-org-render-fixed-point` **bite** on the composition
/// path — no ref needed (it compares the SUT's render against the SUT's disk). Clean
/// → `Ok` (the booted session settled the seed file to a fixed point); overwrite the
/// disk file so it diverges from the SQL render → `Fail`.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_org_render_fixed_point_bites() {
    use holon_pbt_core::composition::CapMap;
    use holon_pbt_core::invariant::InvariantResult;

    let comp = new_component().await;
    let sut = frontend_wide(comp.clone());
    let ref_ = CapMap::new();
    let wf_id = "inv-org-render-fixed-point";
    let result_of = |report: &holon_pbt_core::composition::RunReport| {
        report
            .ran
            .iter()
            .find(|(id, _)| id.0 == wf_id)
            .map(|(_, r)| r.clone())
    };

    // clean: the on-disk seed file == the render from SQL.
    let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;
    assert!(
        matches!(result_of(&report), Some(InvariantResult::Ok)),
        "{wf_id} must reach Ok on the settled seed file; got {:?}; ran={:?}",
        result_of(&report),
        report.ran_ids(),
    );
    eprintln!("[org-render-teeth] clean: inv-org-render-fixed-point Ok");

    // catch: corrupt the disk so it diverges from the SQL render → Fail.
    comp.overwrite_first_org_file("* a totally different on-disk heading\n")
        .await;
    let corrupted = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;
    let failed: Vec<&str> = corrupted.failures().iter().map(|(id, _)| *id).collect();
    assert!(
        failed.contains(&wf_id),
        "a disk/render divergence must make {wf_id} FAIL; failures={failed:?}",
    );
    eprintln!("[org-render-teeth] catch: inv-org-render-fixed-point FAILED on disk divergence");
}

/// E1 teeth: the relocated `SutOrgRead` (production `holon_orgmode` parser) makes
/// `inv-blocks-match-ref/org` **bite** on the composition path. The org block ids are
/// random-per-boot, so read the parsed blocks at runtime and seed the ref's
/// `block_state` with exactly them (`RefBackend::org_blocks` returns non-seed blocks
/// verbatim): clean → `/org` `Ok`; mutate one block's content on the ref → `/org`
/// `Fail`. Scoped to the `/org` arm — the component's `SutBackend` sees the whole
/// booted vault, so the `block_raw` block-tree invariants diverge against this
/// 2-block partial ref (covered elsewhere).
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_org_blocks_match_ref_bites() {
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::state_machine::fresh_reference_state;
    use holon_pbt_core::capabilities::SutOrgRead;
    use holon_pbt_core::invariant::InvariantResult;

    let comp = new_component().await;
    let parsed = comp.org_block_snapshot().await;
    assert!(parsed.len() >= 2, "seed org file must parse to ≥2 blocks");
    let sut = frontend_wide(comp);

    let result_of = |report: &holon_pbt_core::composition::RunReport| {
        report
            .ran
            .iter()
            .find(|(id, _)| id.0 == "inv-blocks-match-ref/org")
            .map(|(_, r)| r.clone())
    };

    // clean: the ref's org view == the on-disk parse.
    let clean = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        for b in &parsed {
            s.domain.block_state.blocks.insert(b.id.clone(), b.clone());
        }
        s
    };
    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(clean),
    )
    .await;
    assert!(
        matches!(result_of(&report), Some(InvariantResult::Ok)),
        "inv-blocks-match-ref/org must reach Ok when the ref's org view matches the \
         on-disk parse; got {:?}; ran={:?}",
        result_of(&report),
        report.ran_ids(),
    );
    eprintln!("[org-teeth] clean: inv-blocks-match-ref/org reached Ok");

    // catch: diverge one block's content on the ref → /org must FAIL.
    let planted = {
        let mut s = fresh_reference_state(holon_pbt_core::Wiring::full());
        for (i, b) in parsed.iter().enumerate() {
            let mut b = b.clone();
            if i == 0 {
                b.content = format!("{}-WRONG", b.content);
            }
            s.domain.block_state.blocks.insert(b.id.clone(), b);
        }
        s
    };
    let planted_report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(planted),
    )
    .await;
    let failed: Vec<&str> = planted_report
        .failures()
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert!(
        failed.contains(&"inv-blocks-match-ref/org"),
        "a content divergence on the ref must make inv-blocks-match-ref/org FAIL (the \
         org-parse oracle bites); failures={failed:?}",
    );
    eprintln!("[org-teeth] catch: inv-blocks-match-ref/org FAILED on the content divergence");
}

/// The §6 payoff, frontend edition: a composed `CapMap` over a real headless
/// `ReactiveEngine` selects `inv-viewmodel-no-error-widgets` by capability
/// presence and runs it over the actual rendered ViewModel tree — which, for a
/// valid layout, has no error widgets.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_runs_no_error_widgets_over_real_render() {
    let comp = new_component().await;
    let sut = frontend_wide(comp);
    let ref_ = CapMap::new();

    let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

    assert!(
        report.ran_ids().contains(&"inv-viewmodel-no-error-widgets"),
        "the frontend slice must select the no-error-widgets invariant; ran={:?}",
        report.ran_ids(),
    );
    assert!(
        report.failures().is_empty(),
        "the real rendered tree has no error widgets: {:?}",
        report.failures(),
    );
}

/// The frontend component also provides `SutBackend` over `block_raw`, so the
/// structural block-tree invariants run over this realization too (no ref
/// needed): a fourth storage backing the same catalog.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_runs_structural_block_invariants() {
    let comp = new_component().await;
    let sut = frontend_wide(comp);
    let ref_ = CapMap::new();

    let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

    for id in ["inv-no-parent-cycles", "inv-source-language-iff-source"] {
        assert!(
            report.ran_ids().contains(&id),
            "{id} must run over the frontend slice's block_raw; ran={:?}",
            report.ran_ids(),
        );
    }
    assert!(
        report.failures().is_empty(),
        "structural invariants hold on a valid store: {:?}",
        report.failures(),
    );
}

/// Bundle D minimal positive: the degraded ("shows source") twin. A real no-Turso
/// block-query frontend (no query engine) boots over a Loro tree whose root has a
/// query-source child, so the production `derive_render_expr` degrades to a
/// `source_editor` render. With NO `SutQueryResults` cap wired, the catalog selects
/// the degraded `inv-viewmodel-shows-source-when-no-query` (and DESELECTS the
/// full-mode `inv-viewmodel-decompiled-rows-match-query`). The twin must pass — the
/// root really renders `source_editor`.
#[tokio::test(flavor = "multi_thread")]
async fn frontend_slice_degraded_shows_source_twin_selects_and_passes() {
    use crate::pbt::frontend_slice::block_query_component::BlockQueryFrontendComponent;
    use crate::pbt::frontend_slice::builders::block_query_degraded;
    use holon_pbt_core::capabilities::SutRenderer;
    use holon_pbt_core::invariant::InvariantResult;

    let comp = Arc::new(BlockQueryFrontendComponent::new().await);

    // The no-Turso seed actually renders `source_editor` (else the twin would Skip
    // forever with no teeth).
    assert_eq!(
        comp.root_render_kind().await.as_deref(),
        Some("source_editor"),
        "the degraded no-Turso block-query frontend must render `source_editor`",
    );

    let sut = block_query_degraded(comp);
    let ref_ = CapMap::new();
    let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

    let degraded = "inv-viewmodel-shows-source-when-no-query";
    let full = "inv-viewmodel-decompiled-rows-match-query";

    let degraded_result = report
        .ran
        .iter()
        .find(|(id, _)| id.0 == degraded)
        .map(|(_, r)| r.clone());
    assert!(
        matches!(degraded_result, Some(InvariantResult::Ok)),
        "the degraded twin must SELECT and pass (root renders source_editor); got {degraded_result:?}; ran={:?}",
        report.ran_ids(),
    );

    // The full-mode twin is mutually exclusive: deselected (disclosed) here because
    // no `SutQueryResults` is wired.
    assert!(
        !report.ran_ids().contains(&full),
        "the full-mode twin must be DESELECTED when no query engine is wired; ran={:?}",
        report.ran_ids(),
    );
    assert!(
        report.deselected.iter().any(|d| d.0 == full),
        "the full-mode twin must be disclosed in deselected; deselected={:?}",
        report.deselected,
    );
}
