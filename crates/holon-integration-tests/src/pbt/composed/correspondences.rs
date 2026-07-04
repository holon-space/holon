//! The correspondence tables — the registry's declarative entries.
//!
//! Each `pub fn <observable>()` builds one [`Correspondence`]: one reference
//! projection + N SUT store projections. The catalog splices
//! `<observable>().wire()` — adding a store or observable is an entry here,
//! nothing else. Extraction/comparison strategies are named `fn`s in this
//! module (greppable wiring; see the registry module doc's integrity rule).

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use holon_api::Block;
use holon_pbt_core::block_compare::compare_blocks;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefHistoryExpectation;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::SutEditorMirrorRead;
use holon_pbt_core::capabilities::SutHistory;
use holon_pbt_core::capabilities::SutOrgRead;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;
use holon_pbt_core::correspondence::Converge;
use holon_pbt_core::correspondence::Correspondence;
use holon_pbt_core::correspondence::Extraction;
use holon_pbt_core::correspondence::NamedCompare;
use holon_pbt_core::correspondence::Observable;
use holon_pbt_core::correspondence::StoreProjection;
use holon_pbt_core::invariant::InvariantResult;

// ─── Turso storage-pipeline arms co-located to `holon-turso-testing` (Phase 2)
// ─
//
// The `inv-blocks-match-ref/{block_raw,matview}` arms of the shared
// `NonSeedBlocks` observable, plus the whole `block_content` / `block_parent` /
// `advice_matviews` observables (every arm Turso-owned), now live in
// `holon_turso_testing::correspondences` and are folded into the composed
// catalog via `holon_turso_testing::pbt_contribution()`. What remains central
// below: the editor-mirror families, the `/org` block store, and the
// root-layout ghost-row check (renderer-observed).

// ─── Observable: active-editor text (the `inv-editor-text/*` family) ─────────

/// The live text of the actively-edited block, as the SUT's `MutableText`
/// mirror sees it vs the reference's `active_editor_text()`. The *reference*
/// owns which block is active (`active_editor_block()`); both sides are keyed
/// off it. Both are 3-valued: the ref is `Unobservable` with no active editor,
/// and the SUT's `editor_live_text` is `Unobservable` (`Err`) when no frontend
/// engine / `MutableText` is resolvable yet. Consolidates the hand-written
/// `inv-editor-text-matches-ref` body.
///
/// `Value` carries the active block id alongside the text so the compare's
/// fail message can name the block (the two-value compare has no other
/// context).
pub struct ActiveEditorText;

impl Observable for ActiveEditorText {
    type Value = (EntityUri, String);
    const NAME: &'static str = "editor-text";
}

pub fn active_editor_text() -> Correspondence<ActiveEditorText> {
    Correspondence {
        ref_project: ref_active_editor_text,
        stores: vec![StoreProjection {
            id: "inv-editor-text/mirror",
            attribution: Attribution::at(Layer::ViewModel, file!()),
            store: "mirror",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutEditorMirrorRead>()],
                sut_absent: Vec::new(),
                ref_present: vec![CapId::of::<dyn RefEditorMirror>()],
            },
            extract: extract_editor_text_mirror,
            compare: NamedCompare {
                name: "compare_editor_text",
                f: compare_editor_text,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_active_editor_text(refs: &CapMap) -> Extraction<(EntityUri, String)> {
    let Some(block) = RefEditorMirror::active_editor_block(refs) else {
        return Extraction::Unobservable("no active editor in reference model".to_string());
    };
    let text = RefEditorMirror::active_editor_text(refs)
        .expect("ref invariant: active_editor_block() implies active_editor_text()")
        .to_string();
    Extraction::Value((block, text))
}

fn extract_editor_text_mirror<'a>(
    sut: &'a CapMap,
    refs: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<(EntityUri, String)>> + 'a>> {
    Box::pin(async move {
        // `ref_project` already proved the active block is `Some`; the registry
        // short-circuits to Skip otherwise, so `extract` never runs without it.
        let block = RefEditorMirror::active_editor_block(refs)
            .expect("registry: extract runs only after ref_project yielded a value");
        match SutEditorMirrorRead::editor_live_text(sut, &block) {
            Err(reason) => Extraction::Unobservable(format!("live text unobservable: {reason}")),
            Ok(text) => Extraction::Value((block, text)),
        }
    })
}

fn compare_editor_text(
    sut: &(EntityUri, String),
    ref_: &(EntityUri, String),
) -> Result<(), String> {
    let (block, sut_text) = sut;
    let (_, ref_text) = ref_;
    if sut_text == ref_text {
        return Ok(());
    }
    Err(format!(
        "[inv-editor-text/mirror] Live editor text mismatch on {block}:\n  reference: \
         {ref_text:?}\n  SUT MutableText: {sut_text:?}"
    ))
}

// ─── Observable: active-editor caret (the `inv-editor-caret/*` family) ───────

/// The tracked caret byte of the actively-edited block (SUT mirror vs the
/// reference's `active_editor_cursor()`). Keyed off the reference's active
/// block like [`ActiveEditorText`]. 3-valued on both sides plus one extra SUT
/// Skip: `Ok(None)` = the SUT tracks no caret yet (no keystroke since focus —
/// the headless mirror initializes lazily). Consolidates the hand-written
/// `inv-editor-caret-matches-ref` body.
///
/// NOTE: the old body appended the ref editor text to its fail message as a
/// diagnostic breadcrumb; the registry's two-value compare carries only the
/// (block, caret) pair, so that breadcrumb is dropped. The block id and both
/// caret bytes remain.
pub struct ActiveEditorCaret;

impl Observable for ActiveEditorCaret {
    type Value = (EntityUri, usize);
    const NAME: &'static str = "editor-caret";
}

pub fn active_editor_caret() -> Correspondence<ActiveEditorCaret> {
    Correspondence {
        ref_project: ref_active_editor_caret,
        stores: vec![StoreProjection {
            id: "inv-editor-caret/mirror",
            attribution: Attribution::at(Layer::ViewModel, file!()),
            store: "mirror",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutEditorMirrorRead>()],
                sut_absent: Vec::new(),
                ref_present: vec![CapId::of::<dyn RefEditorMirror>()],
            },
            extract: extract_editor_caret_mirror,
            compare: NamedCompare {
                name: "compare_editor_caret",
                f: compare_editor_caret,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_active_editor_caret(refs: &CapMap) -> Extraction<(EntityUri, usize)> {
    let Some(block) = RefEditorMirror::active_editor_block(refs) else {
        return Extraction::Unobservable("no active editor in reference model".to_string());
    };
    let cursor = RefEditorMirror::active_editor_cursor(refs)
        .expect("ref invariant: active_editor_block() implies active_editor_cursor()");
    Extraction::Value((block, cursor))
}

fn extract_editor_caret_mirror<'a>(
    sut: &'a CapMap,
    refs: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<(EntityUri, usize)>> + 'a>> {
    Box::pin(async move {
        let block = RefEditorMirror::active_editor_block(refs)
            .expect("registry: extract runs only after ref_project yielded a value");
        match SutEditorMirrorRead::editor_caret_byte(sut, &block) {
            Err(reason) => Extraction::Unobservable(format!("caret unobservable: {reason}")),
            Ok(None) => Extraction::Unobservable(format!(
                "SUT tracks no caret for {block} yet (no keystroke since focus)"
            )),
            Ok(Some(caret)) => Extraction::Value((block, caret)),
        }
    })
}

fn compare_editor_caret(sut: &(EntityUri, usize), ref_: &(EntityUri, usize)) -> Result<(), String> {
    let (block, sut_cursor) = sut;
    let (_, ref_cursor) = ref_;
    if sut_cursor == ref_cursor {
        return Ok(());
    }
    Err(format!(
        "[inv-editor-caret/mirror] Caret mismatch on {block}: reference model \
         cursor_byte={ref_cursor}, SUT tracked caret={sut_cursor}"
    ))
}

// ─── Observable: org store of block-equivalence (`inv-blocks-match-ref/org`)
// ──

/// The blocks parsed back off the on-disk org files vs the reference's org
/// view. Shares the `blocks-match-ref` family stem with [`NonSeedBlocks`] but
/// is a SEPARATE correspondence: its reference value comes from a DIFFERENT
/// projection (`RefBackend::org_blocks` — non-seed, non-page, with the org
/// parser's `file:<filename>` parent for unresolved docs), so it cannot share
/// `NonSeedBlocks`' single `ref_project`. It is also the only block store whose
/// per-parent sibling ORDER is checked (disk order = the renderer's canonical
/// order; `compare_blocks(check_order = true)`) — `inv-live-children-match-ref`
/// checks SQL `sort_key` order separately. Consolidates the last hand-written
/// `blocks_match` body.
pub struct OrgBlocks;

impl Observable for OrgBlocks {
    type Value = Vec<Block>;
    const NAME: &'static str = "blocks-match-ref";
}

pub fn org_blocks() -> Correspondence<OrgBlocks> {
    Correspondence {
        ref_project: ref_org_blocks,
        stores: vec![StoreProjection {
            id: "inv-blocks-match-ref/org",
            attribution: Attribution::at(Layer::OrgRoundTrip, file!()),
            store: "org",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutOrgRead>()],
                sut_absent: Vec::new(),
                ref_present: vec![CapId::of::<dyn RefBackend>()],
            },
            extract: extract_org_snapshot,
            compare: NamedCompare {
                name: "compare_blocks{fields+order}",
                f: compare_org_blocks,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_org_blocks(refs: &CapMap) -> Extraction<Vec<Block>> {
    Extraction::Value(RefBackend::org_blocks(refs))
}

fn extract_org_snapshot<'a>(
    sut: &'a CapMap,
    refs: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<Vec<Block>>> + 'a>> {
    // Filter the on-disk-parsed SUT blocks by the SAME `seed_block_ids` the ref's
    // `org_blocks` projection excludes (scaffold-injected boot layout: the
    // `block:journals` page + its `src::0`/`render::0` display sources). This is
    // the symmetric twin of `extract_block_raw` in holon-turso-testing — the
    // block_raw arm has always filtered seed on the SUT side; the org arm only
    // "matched" by accident while the parser silently dropped top-level
    // `#+BEGIN_SRC` blocks (the seed sources render at the page's top level). Once
    // the parser correctly round-trips top-level sources (row-28 data-loss fix),
    // those seed sources surface here and MUST be filtered to stay symmetric —
    // otherwise they read as spurious `only_in_actual` blocks. Non-seed content
    // (the `journals::auto-create` heading + its `holon_rule` action, and all
    // user blocks) is NOT in `seed_block_ids`, so it is still compared.
    Box::pin(async move {
        let seed_block_ids = RefBackend::seed_block_ids(refs);
        Extraction::Value(
            sut.org_block_snapshot()
                .await
                .into_iter()
                .filter(|b| !seed_block_ids.contains(&b.id))
                .collect(),
        )
    })
}

fn compare_org_blocks(sut: &Vec<Block>, ref_: &Vec<Block>) -> Result<(), String> {
    match compare_blocks("inv-blocks-match-ref/org", sut, ref_, true) {
        InvariantResult::Ok => Ok(()),
        InvariantResult::Fail(msg) => Err(msg),
        InvariantResult::Skipped(reason) => Err(format!(
            "[inv-blocks-match-ref/org] unexpected Skip from compare_blocks: {reason}"
        )),
    }
}

// ─── Observable: root-layout ghost rows (`inv-matview-consistent-with-ref/*`)
// ─

/// The id set the root-layout matview surfaces as `data_rows`, guarded against
/// GHOST ROWS: ids present in the matview but outside the reference universe
/// (every ref block incl. seed + source, plus layout scaffolding, plus profile
/// blocks) — stale rows left by an IVM inconsistency. Asymmetric: this is a
/// SUBSET check (`data ⊆ ref universe`), not equality — under-projection of
/// content is `inv-block-ids-match-ref` / `inv-live-children-match-ref`'s job.
/// Disclosed Skip when the matview snapshot is empty (engine not warmed up).
/// Consolidates the hand-written `inv-matview-consistent-with-ref` body.
pub struct MatviewGhostRows;

impl Observable for MatviewGhostRows {
    type Value = BTreeSet<EntityUri>;
    const NAME: &'static str = "matview-consistent-with-ref";
}

pub fn matview_ghost_rows() -> Correspondence<MatviewGhostRows> {
    Correspondence {
        ref_project: ref_block_universe,
        stores: vec![StoreProjection {
            id: "inv-matview-consistent-with-ref/root_layout",
            attribution: Attribution::at(Layer::Projection, file!()),
            store: "root_layout",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutRenderer>()],
                sut_absent: Vec::new(),
                ref_present: vec![CapId::of::<dyn RefLayout>()],
            },
            extract: extract_root_data_rows,
            compare: NamedCompare {
                name: "compare_no_ghost_rows{subset}",
                f: compare_no_ghost_rows,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_block_universe(refs: &CapMap) -> Extraction<BTreeSet<EntityUri>> {
    // Every id the ref model knows: all blocks (incl. seed + source) ∪ layout
    // scaffolding ∪ profile blocks. Any matview id outside this is a ghost.
    Extraction::Value(
        RefLayout::all_block_ids(refs)
            .into_iter()
            .chain(RefLayout::layout_block_ids(refs))
            .chain(RefLayout::profile_block_ids(refs))
            .collect(),
    )
}

fn extract_root_data_rows<'a>(
    sut: &'a CapMap,
    _: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<BTreeSet<EntityUri>>> + 'a>> {
    Box::pin(async move {
        let data_block_ids = SutRenderer::root_data_row_ids(sut).await;
        if data_block_ids.is_empty() {
            return Extraction::Unobservable(
                "matview snapshot empty (engine not warmed up / still loading)".to_string(),
            );
        }
        Extraction::Value(data_block_ids)
    })
}

fn compare_no_ghost_rows(
    data_block_ids: &BTreeSet<EntityUri>,
    ref_universe: &BTreeSet<EntityUri>,
) -> Result<(), String> {
    let extra: Vec<&EntityUri> = data_block_ids
        .iter()
        .filter(|id| !ref_universe.contains(*id))
        .collect();
    if extra.is_empty() {
        return Ok(());
    }
    Err(format!(
        "[inv-matview-consistent-with-ref/root_layout] IVM MATVIEW GHOST ROW DETECTED!\n  data \
         rows (from root-layout matview): {} ids\n  reference model: {} known ids\n  extra in \
         matview (stale/ghost, not in ref universe): {extra:?}",
        data_block_ids.len(),
        ref_universe.len(),
    ))
}

// NOTE: the advice-matview SQL twin (`inv-advice-matview-matches-ref/matview`)
// co-located to `holon-turso-testing` (Phase 2) — its observable + comparator
// (and their unit tests) now live in `holon_turso_testing::correspondences`.

// ─── Observable: C2 provenance — no PHANTOM history (subset) ─────────────────

/// Every `block_id` recorded in the SUT's `block_history` op/effect stream must
/// be a block the reference model created or knew (G9, phantom guard). The
/// reference anchor is the union of the live block universe and every id the
/// oracle minted (`RefHistoryExpectation::ever_created_ids`, which retains
/// create-then-deleted ids from the reconcile map). Asymmetric SUBSET check:
/// history ⊆ known-universe. A recorded id outside it is a phantom/ghost
/// history row (a mis-keyed or leaked recording). Cap-gated on `SutHistory`, so
/// an org-only draw (no recording substrate) deselects cleanly.
pub struct HistoryNoPhantomRows;

impl Observable for HistoryNoPhantomRows {
    type Value = BTreeSet<EntityUri>;
    const NAME: &'static str = "history-no-phantom-rows";
}

pub fn history_no_phantom_rows() -> Correspondence<HistoryNoPhantomRows> {
    Correspondence {
        ref_project: ref_history_universe,
        stores: vec![StoreProjection {
            id: "inv-history-no-phantom-rows/block_history",
            attribution: Attribution::at(Layer::Projection, file!()),
            store: "block_history",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutHistory>()],
                sut_absent: Vec::new(),
                ref_present: vec![
                    CapId::of::<dyn RefHistoryExpectation>(),
                    CapId::of::<dyn RefLayout>(),
                ],
            },
            extract: extract_history_block_ids,
            compare: NamedCompare {
                name: "compare_history_subset{no_phantom}",
                f: compare_history_no_phantom,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_history_universe(refs: &CapMap) -> Extraction<BTreeSet<EntityUri>> {
    let universe: BTreeSet<EntityUri> = RefLayout::all_block_ids(refs)
        .into_iter()
        .chain(RefLayout::layout_block_ids(refs))
        .chain(RefLayout::profile_block_ids(refs))
        .chain(RefHistoryExpectation::ever_created_ids(refs))
        .collect();
    Extraction::Value(universe)
}

fn extract_history_block_ids<'a>(
    sut: &'a CapMap,
    _: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<BTreeSet<EntityUri>>> + 'a>> {
    Box::pin(async move { Extraction::Value(SutHistory::history_block_ids(sut).await) })
}

fn compare_history_no_phantom(
    history_ids: &BTreeSet<EntityUri>,
    universe: &BTreeSet<EntityUri>,
) -> Result<(), String> {
    let phantom: Vec<&EntityUri> = history_ids
        .iter()
        .filter(|id| !universe.contains(*id))
        .collect();
    if phantom.is_empty() {
        return Ok(());
    }
    Err(format!(
        "[inv-history-no-phantom-rows/block_history] PHANTOM HISTORY: {} block id(s) recorded in \
         block_history are unknown to the reference (never created/known): {phantom:?}\n  history \
         ids: {} recorded\n  ref universe (live ∪ ever-created): {} known",
        phantom.len(),
        history_ids.len(),
        universe.len(),
    ))
}

// ─── Observable: C2 provenance — no MISSED history (op-group floor) ──────────

/// The SUT's `block_history` must record at least as many distinct `op_group`s
/// as the oracle drove UI creates (G9, missed-history guard). Each UI-driven
/// create routes through `execute_operation` and records ≥1 field delta = one
/// `op_group`; the reference floor is the count of synthetic→real reconciled
/// mints (`RefHistoryExpectation::min_recorded_op_groups`), which excludes
/// born-equal external/peer creates that record no engine history. A
/// conservative LOWER BOUND (`sut ≥ ref`): extra SUT recordings (edits,
/// boot-rule firings) only help. A shortfall means a create silently failed to
/// record — the missed-history prod bug this guards. Cap-gated on `SutHistory`.
pub struct HistoryOpGroupFloor;

impl Observable for HistoryOpGroupFloor {
    type Value = usize;
    const NAME: &'static str = "history-records-all-creates";
}

pub fn history_records_all_creates() -> Correspondence<HistoryOpGroupFloor> {
    Correspondence {
        ref_project: ref_min_op_groups,
        stores: vec![StoreProjection {
            id: "inv-history-records-all-creates/block_history",
            attribution: Attribution::at(Layer::Projection, file!()),
            store: "block_history",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutHistory>()],
                sut_absent: Vec::new(),
                ref_present: vec![CapId::of::<dyn RefHistoryExpectation>()],
            },
            extract: extract_history_op_group_count,
            compare: NamedCompare {
                name: "compare_op_group_floor{>=}",
                f: compare_op_group_floor,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_min_op_groups(refs: &CapMap) -> Extraction<usize> {
    Extraction::Value(RefHistoryExpectation::min_recorded_op_groups(refs))
}

fn extract_history_op_group_count<'a>(
    sut: &'a CapMap,
    _: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<usize>> + 'a>> {
    Box::pin(async move { Extraction::Value(SutHistory::history_op_group_count(sut).await) })
}

fn compare_op_group_floor(sut_count: &usize, ref_floor: &usize) -> Result<(), String> {
    if sut_count >= ref_floor {
        return Ok(());
    }
    Err(format!(
        "[inv-history-records-all-creates/block_history] MISSED HISTORY: block_history has \
         {sut_count} distinct op_group(s) but the oracle drove {ref_floor} UI create(s) that each \
         must record ≥1 — {} create(s) went unrecorded",
        ref_floor - sut_count,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use holon_pbt_core::capabilities::EntityUri;
    use holon_pbt_core::capabilities::SutHistory;
    use holon_pbt_core::composition::CapMap;

    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
    use crate::pbt::composed::subsystem_seed::seed_ref;
    use crate::pbt::composed::subsystem_seed::seed_ref_with_editor;

    /// Catch (doc §6 gate): with the ref wired, a SUT `block_raw` whose content
    /// diverged from the reference is caught by the registry-emitted
    /// `inv-blocks-match-ref/block_raw` (`NonSeedBlocks`' block_raw store).
    #[tokio::test]
    async fn blocks_match_block_raw_catches_divergence_from_ref() {
        let id = uri("local://d");
        let sut = fixture_slice(vec![Block::new_text(
            id.clone(),
            EntityUri::no_parent(),
            "sut-content",
        )]);
        let ref_state = seed_ref(vec![Block::new_text(
            id,
            EntityUri::no_parent(),
            "ref-content",
        )]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-blocks-match-ref/block_raw"),
            "the content divergence must be caught; failures={failures:?}",
        );
    }

    /// Catch (doc §6 gate): a `block_raw` content that diverged from the
    /// reference's `RefBlockTree` view — the borrow-returning read driving a
    /// real failure against the registry-emitted `inv-block-content/block_raw`.
    #[tokio::test]
    async fn block_content_block_raw_catches_divergence() {
        let id = uri("local://d");
        let sut = fixture_slice(vec![Block::new_text(
            id.clone(),
            EntityUri::no_parent(),
            "sut-content",
        )]);
        let ref_state = seed_ref(vec![Block::new_text(
            id,
            EntityUri::no_parent(),
            "ref-content",
        )]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-block-content/block_raw"),
            "the content divergence must be caught via RefBlockTree::block_content; \
             failures={failures:?}",
        );
    }

    /// Catch (doc §6 gate): a SQL `block_raw.content` that diverged from the
    /// reference is caught by the registry-emitted `inv-block-content/sql`.
    #[tokio::test]
    async fn block_content_sql_catches_divergence() {
        let id = uri("block:d");
        let sut = sql_projection_map(vec![(id.clone(), "sql-content")]);
        let ref_state = seed_ref(vec![Block::new_text(
            id,
            EntityUri::no_parent(),
            "ref-content",
        )]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-block-content/sql"),
            "the SQL content divergence must be caught; failures={failures:?}",
        );
    }

    /// Catch (doc §6 gate): a block whose SUT parent is a *different but valid*
    /// (present, acyclic) block than the reference says. `no_orphan` /
    /// `no_parent_cycles` pass (the wrong parent exists and the chain
    /// terminates) and `blocks-match` passes (it skips the `Parent` facet), so
    /// only `inv-block-parent/block_raw` fails — exercising `parent_of`.
    #[tokio::test]
    async fn block_parent_block_raw_catches_reparent() {
        let x = uri("local://x");
        let p1 = uri("local://p1");
        let p2 = uri("local://p2");
        // SUT: X parented under P2 (which exists → not an orphan, no cycle).
        let sut = fixture_slice(vec![
            Block::new_text(p1.clone(), EntityUri::no_parent(), "p1"),
            Block::new_text(p2.clone(), EntityUri::no_parent(), "p2"),
            Block::new_text(x.clone(), p2, "x"),
        ]);
        // Ref: same blocks/content/id-set, but X belongs under P1.
        let ref_state = seed_ref(vec![
            Block::new_text(p1.clone(), EntityUri::no_parent(), "p1"),
            Block::new_text(uri("local://p2"), EntityUri::no_parent(), "p2"),
            Block::new_text(x, p1, "x"),
        ]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-block-parent/block_raw"),
            "the re-parent must be caught by the parent invariant; failures={failures:?}",
        );
        // Isolation: the existence/termination/content invariants must NOT fire —
        // the wrong parent is valid and only the parent linkage diverged.
        for clean in [
            "inv-no-orphan-blocks",
            "inv-no-parent-cycles",
            "inv-blocks-match-ref/block_raw",
        ] {
            assert!(
                !failures.iter().any(|(id, _)| *id == clean),
                "{clean} must stay green (only parent linkage diverged); failures={failures:?}",
            );
        }
    }

    /// Catch (doc §6 gate): a SUT editor whose `MutableText` lost a character
    /// relative to the reference is caught by the registry-emitted
    /// `inv-editor-text/mirror` (reads the borrow-returning
    /// `RefEditorMirror::active_editor_text` off the ref `CapMap`).
    #[tokio::test]
    async fn editor_text_mirror_catches_live_text_divergence() {
        let block = uri("local://e");
        let sut = buggy_editor_map(BuggyEditor {
            block: block.clone(),
            text: "helo".to_string(),
            caret: 4,
        });
        // The real oracle holds "hello" (caret = len = 5); the buggy SUT dropped
        // a char to "helo".
        let ref_state = seed_ref_with_editor(Vec::new(), block, "hello");

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-editor-text/mirror"),
            "the live-text divergence must be caught; failures={failures:?}",
        );
    }

    /// Catch (doc §6 gate): a SUT editor whose tracked byte caret is off by one
    /// (the `MoveCursor` byte/keystroke-conflation bug class). Text agrees, so
    /// only `inv-editor-caret/mirror` fires — proving caret isolation.
    #[tokio::test]
    async fn editor_caret_mirror_catches_caret_divergence() {
        let block = uri("local://e");
        let sut = buggy_editor_map(BuggyEditor {
            block: block.clone(),
            text: "hello".to_string(),
            caret: 4,
        });
        // The real oracle opens the editor at end-of-text (caret = len = 5); the
        // buggy SUT reports caret 4. Text agrees on "hello".
        let ref_state = seed_ref_with_editor(Vec::new(), block, "hello");

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-editor-caret/mirror"),
            "the caret divergence must be caught; failures={failures:?}",
        );
        assert!(
            !failures
                .iter()
                .any(|(id, _)| *id == "inv-editor-text/mirror"),
            "text agrees, so only the caret invariant fires; failures={failures:?}",
        );
    }

    // NOTE: the pure comparator tests for the advice-matview twin co-located to
    // `holon-turso-testing` (Phase 2) alongside `compare_advice_matviews`.

    /// A controllable `SutHistory` double: returns the exact `block_history`
    /// block-id set + op-group count the test wants, so the two C2 provenance
    /// correspondences can be driven to catch / pass without a real recording
    /// engine (the journals ingest-loss RED blocks full keystone sequences).
    struct StubHistory {
        block_ids: BTreeSet<EntityUri>,
        op_group_count: usize,
    }

    #[async_trait::async_trait(?Send)]
    impl SutHistory for StubHistory {
        async fn history_block_ids(&self) -> BTreeSet<EntityUri> {
            self.block_ids.clone()
        }
        async fn history_op_group_count(&self) -> usize {
            self.op_group_count
        }
    }

    impl holon_pbt_core::composition::CapProvider for StubHistory {
        fn register(self: std::sync::Arc<Self>, caps: &mut holon_pbt_core::composition::CapMap) {
            caps.insert(self as std::sync::Arc<dyn SutHistory>);
        }
    }

    fn stub_history_sut(block_ids: Vec<EntityUri>, op_group_count: usize) -> CapMap {
        holon_pbt_core::composition::Config::new()
            .with(StubHistory {
                block_ids: block_ids.into_iter().collect(),
                op_group_count,
            })
            .build()
    }

    /// Catch (doc §6 gate): a `block_history` row whose `block_id` the
    /// reference never created/knew (a phantom/ghost recording) is caught
    /// by `inv-history-no-phantom-rows/block_history`.
    #[tokio::test]
    async fn history_phantom_row_is_caught() {
        let sut = stub_history_sut(vec![uri("block:ghost")], 1);
        let ref_state = seed_ref(vec![Block::new_text(
            uri("block:c1"),
            EntityUri::no_parent(),
            "c1",
        )]);
        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;
        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-history-no-phantom-rows/block_history"),
            "a phantom history block_id must be caught; failures={failures:?}",
        );
    }

    /// Pass: every recorded `block_id` is a block the reference created/knew,
    /// so the phantom-history subset check is green (id-space +
    /// ever-created anchor wired correctly).
    #[tokio::test]
    async fn history_known_rows_pass_phantom_check() {
        let sut = stub_history_sut(vec![uri("block:c1")], 1);
        let ref_state = seed_ref(vec![Block::new_text(
            uri("block:c1"),
            EntityUri::no_parent(),
            "c1",
        )]);
        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;
        assert!(
            !report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-history-no-phantom-rows/block_history"),
            "recorded ids are all known; the subset check must pass; failures={:?}",
            report.failures(),
        );
    }

    /// Catch (doc §6 gate): the oracle drove more UI creates than
    /// `block_history` recorded op_groups (a missed-history recording) —
    /// caught by `inv-history-records-all-creates/block_history`.
    #[tokio::test]
    async fn history_missed_create_is_caught() {
        let sut = stub_history_sut(vec![], 1);
        let mut ref_state = seed_ref(vec![]);
        ref_state.history_min_op_groups = 3;
        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;
        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-history-records-all-creates/block_history"),
            "a create recorded fewer op_groups than driven must be caught; failures={failures:?}",
        );
    }

    /// Pass: `block_history` recorded at least as many op_groups as UI creates
    /// driven (the lower bound holds; extra recordings are fine).
    #[tokio::test]
    async fn history_op_group_floor_is_met() {
        let sut = stub_history_sut(vec![], 5);
        let mut ref_state = seed_ref(vec![]);
        ref_state.history_min_op_groups = 3;
        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;
        assert!(
            !report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-history-records-all-creates/block_history"),
            "the op-group floor is met; the check must pass; failures={:?}",
            report.failures(),
        );
    }
}
