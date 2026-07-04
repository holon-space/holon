//! The correspondence tables — the registry's declarative entries.
//!
//! Each `pub fn <observable>()` builds one [`Correspondence`]: one reference
//! projection + N SUT store projections. The catalog splices
//! `<observable>().wire()` — adding a store or observable is an entry here,
//! nothing else. Extraction/comparison strategies are named `fn`s in this
//! module (greppable wiring; see the registry module doc's integrity rule).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use holon_api::Block;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefAdvice;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::SutAdviceMatview;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutEditorMirrorRead;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::capabilities::SutOrgRead;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::Needs;
use holon_pbt_core::invariant::InvariantResult;

use crate::pbt::correspondence::Converge;
use crate::pbt::correspondence::Correspondence;
use crate::pbt::correspondence::Extraction;
use crate::pbt::correspondence::NamedCompare;
use crate::pbt::correspondence::Observable;
use crate::pbt::correspondence::StoreProjection;
use crate::pbt::invariants::block_compare::BlockFacet;
use crate::pbt::invariants::block_compare::compare_block_fields;
use crate::pbt::invariants::block_compare::compare_block_subset;
use crate::pbt::invariants::block_compare::compare_blocks;

// ─── Observable: non-seed blocks (the `inv-blocks-match-ref/*` family) ──────

/// The set of non-seed blocks, as each storage-pipeline store sees it. The
/// reference projection is [`RefBackend::non_seed_blocks`]; each store
/// snapshot filters seed rows via [`RefBackend::seed_block_ids`] (context
/// read: it shapes comparability, it never supplies the expected value). The
/// `/org` view stays a hand-written invariant for now — distinct observable
/// facet (renderer-canonical sibling order), Phase 3.
pub struct NonSeedBlocks;

impl Observable for NonSeedBlocks {
    type Value = Vec<Block>;
    const NAME: &'static str = "blocks-match-ref";
}

pub fn non_seed_blocks() -> Correspondence<NonSeedBlocks> {
    Correspondence {
        ref_project: ref_non_seed_blocks,
        stores: vec![
            // Write-side `block_raw` table — the convergent source of truth.
            // Field SUBSET: `block_raw` lacks the junction `tags`/`requires`
            // columns the matview joins, so a full compare would false-fail.
            StoreProjection {
                id: "inv-blocks-match-ref/block_raw",
                store: "block_raw",
                needs: Needs {
                    sut_present: vec![CapId::of::<dyn SutBackend>()],
                    sut_absent: Vec::new(),
                    ref_present: vec![CapId::of::<dyn RefBackend>()],
                },
                extract: extract_block_raw,
                compare: NamedCompare {
                    name: "compare_block_subset{content,properties}",
                    f: compare_block_raw_subset,
                },
                converge: Converge::None,
            },
            // CDC-driven `block` matview mirror. Settle-first: the harness's
            // 3-projection convergence settle covers CDC, so this store runs
            // strict `Converge::None` (the pre-registry body's 5s retry idiom
            // predates the settle hook). If the keystone surfaces a residual
            // in-transition race, downgrade to a DISCLOSED `Converge::Retry`
            // with the observed lag as the reason — never silently.
            StoreProjection {
                id: "inv-blocks-match-ref/matview",
                store: "matview",
                needs: Needs {
                    sut_present: vec![
                        CapId::of::<dyn SutBackend>(),
                        CapId::of::<dyn SutSqlProjection>(),
                    ],
                    sut_absent: Vec::new(),
                    ref_present: vec![CapId::of::<dyn RefBackend>()],
                },
                extract: extract_matview,
                compare: NamedCompare {
                    name: "compare_block_fields",
                    f: compare_matview_fields,
                },
                converge: Converge::None,
            },
            // Live Loro tree. Unobservable (disclosed Skip) when Loro isn't
            // enabled on the variant; strict otherwise — seeds materialize
            // into the Loro store, so a non-seed divergence is a real bug.
            StoreProjection {
                // BLOCKED on Phase 1a: part of the shared cross-store correspondence
                // registry; splitting the /loro arm out fragments it. Stays central.
                id: "inv-blocks-match-ref/loro",
                store: "loro",
                needs: Needs {
                    sut_present: vec![CapId::of::<dyn SutLoroLog>()],
                    sut_absent: Vec::new(),
                    ref_present: vec![CapId::of::<dyn RefBackend>()],
                },
                extract: extract_loro,
                compare: NamedCompare {
                    name: "compare_block_fields",
                    f: compare_loro_fields,
                },
                converge: Converge::None,
            },
        ],
    }
}

fn ref_non_seed_blocks(refs: &CapMap) -> Extraction<Vec<Block>> {
    Extraction::Value(refs.non_seed_blocks())
}

fn extract_block_raw<'a>(
    sut: &'a CapMap,
    refs: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<Vec<Block>>> + 'a>> {
    Box::pin(async move {
        let seed_block_ids = refs.seed_block_ids();
        Extraction::Value(
            sut.block_raw_snapshot()
                .await
                .into_iter()
                .filter(|b| !seed_block_ids.contains(&b.id))
                .collect(),
        )
    })
}

fn extract_matview<'a>(
    sut: &'a CapMap,
    refs: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<Vec<Block>>> + 'a>> {
    Box::pin(async move {
        let seed_block_ids = refs.seed_block_ids();
        Extraction::Value(
            sut.live_block_snapshot()
                .await
                .into_iter()
                .filter(|b| !seed_block_ids.contains(&b.id))
                .collect(),
        )
    })
}

fn extract_loro<'a>(
    sut: &'a CapMap,
    refs: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<Vec<Block>>> + 'a>> {
    Box::pin(async move {
        let Some(loro_blocks) = sut.loro_block_snapshot().await else {
            return Extraction::Unobservable("Loro not enabled on this variant".to_string());
        };
        let seed_block_ids = refs.seed_block_ids();
        Extraction::Value(
            loro_blocks
                .into_iter()
                .filter(|b| !seed_block_ids.contains(&b.id))
                .collect(),
        )
    })
}

fn compare_matview_fields(sut: &Vec<Block>, ref_: &Vec<Block>) -> Result<(), String> {
    compare_block_fields("inv-blocks-match-ref/matview", sut, ref_)
}

fn compare_loro_fields(sut: &Vec<Block>, ref_: &Vec<Block>) -> Result<(), String> {
    compare_block_fields("inv-blocks-match-ref/loro", sut, ref_)
}

fn compare_block_raw_subset(sut: &Vec<Block>, ref_: &Vec<Block>) -> Result<(), String> {
    match compare_block_subset(
        "inv-blocks-match-ref/block_raw",
        sut,
        ref_,
        &[BlockFacet::Content, BlockFacet::Properties],
    ) {
        InvariantResult::Ok => Ok(()),
        InvariantResult::Fail(msg) => Err(msg),
        // `compare_block_subset` never skips; a Skip here would be a harness
        // bug hidden inside a comparator — surface it as a failure.
        InvariantResult::Skipped(reason) => Err(format!(
            "[inv-blocks-match-ref/block_raw] unexpected Skip from compare_block_subset: {reason}"
        )),
    }
}

// ─── Observable: per-block content (the `inv-block-content/*` family) ────────

/// Per-block `content`, keyed by non-seed block id, as each store sees it. The
/// reference projection enumerates the reference's non-synthetic non-seed block
/// ids ([`RefBlockTree::all_non_seed_block_ids`]) and answers content per id
/// via the borrow-returning [`RefBlockTree::block_content`]; ids the reference
/// has no content for are dropped (they cannot be compared — the old body's
/// `None => continue`). Consolidates the two hand-written per-block-content
/// bodies (the `SutSqlProjection` SQL-column read → `/sql`; the `SutBackend`
/// snapshot → `/block_raw`). Synthetic ref ids (`block::split-N`,
/// `block::bulk-N-M`) are remapped to UUIDs production-side, so the SUT has no
/// row at the synthetic id — skipped on both projection paths (content equality
/// on stable ids only; the wider PBT reconciles synthetic-id mapping).
pub struct BlockContent;

impl Observable for BlockContent {
    type Value = BTreeMap<EntityUri, String>;
    const NAME: &'static str = "block-content";
}

pub fn block_content() -> Correspondence<BlockContent> {
    Correspondence {
        ref_project: ref_block_content,
        stores: vec![
            // Write-side `block_raw` snapshot via `SutBackend` — the pure-memory
            // path (no Turso, no matview): the same capability portability that
            // moved `inv-no-orphan-blocks` off `SutSqlProjection`.
            StoreProjection {
                id: "inv-block-content/block_raw",
                store: "block_raw",
                needs: Needs {
                    sut_present: vec![CapId::of::<dyn SutBackend>()],
                    sut_absent: Vec::new(),
                    ref_present: vec![CapId::of::<dyn RefBlockTree>()],
                },
                extract: extract_block_content_backend,
                compare: NamedCompare {
                    name: "compare_block_content{block_raw}",
                    f: compare_block_content_block_raw,
                },
                converge: Converge::None,
            },
            // Direct-column SQL read (`SutSqlProjection::block_content`, a
            // `block_raw.content` probe) — the surface `E2ESut` realizes over
            // Turso. Selected only where a slice supplies `SutSqlProjection`.
            StoreProjection {
                id: "inv-block-content/sql",
                store: "sql",
                needs: Needs {
                    sut_present: vec![CapId::of::<dyn SutSqlProjection>()],
                    sut_absent: Vec::new(),
                    ref_present: vec![CapId::of::<dyn RefBlockTree>()],
                },
                extract: extract_block_content_sql,
                compare: NamedCompare {
                    name: "compare_block_content{sql}",
                    f: compare_block_content_sql,
                },
                converge: Converge::None,
            },
        ],
    }
}

fn ref_block_content(refs: &CapMap) -> Extraction<BTreeMap<EntityUri, String>> {
    let mut out = BTreeMap::new();
    for id in RefBlockTree::all_non_seed_block_ids(refs) {
        if crate::pbt::is_synthetic_ref_id(&id) {
            continue;
        }
        // The borrowing read through `RefBlockTree` — forwarded through
        // `CapMap::expect_ref`. `None` = no ref content to compare.
        if let Some(c) = RefBlockTree::block_content(refs, &id) {
            let content = c.to_string();
            out.insert(id, content);
        }
    }
    Extraction::Value(out)
}

fn extract_block_content_backend<'a>(
    sut: &'a CapMap,
    _: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<BTreeMap<EntityUri, String>>> + 'a>> {
    Box::pin(async move {
        Extraction::Value(
            sut.block_raw_snapshot()
                .await
                .into_iter()
                .map(|b| (b.id, b.content))
                .collect(),
        )
    })
}

fn extract_block_content_sql<'a>(
    sut: &'a CapMap,
    refs: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<BTreeMap<EntityUri, String>>> + 'a>> {
    // `SutSqlProjection` has no bulk content snapshot; probe per id over the
    // reference's non-synthetic non-seed id space (comparability context only —
    // the expected value comes from `ref_block_content`). A `None` probe drops
    // the id, so the comparator reports it "missing from SQL projection".
    Box::pin(async move {
        let mut out = BTreeMap::new();
        for id in RefBlockTree::all_non_seed_block_ids(refs) {
            if crate::pbt::is_synthetic_ref_id(&id) {
                continue;
            }
            if let Some(c) = SutSqlProjection::block_content(sut, &id).await {
                out.insert(id, c);
            }
        }
        Extraction::Value(out)
    })
}

fn compare_block_content_block_raw(
    sut: &BTreeMap<EntityUri, String>,
    ref_: &BTreeMap<EntityUri, String>,
) -> Result<(), String> {
    for (id, ref_content) in ref_ {
        match sut.get(id) {
            Some(sut_c) if sut_c == ref_content => {}
            Some(sut_c) => {
                return Err(format!(
                    "[inv-block-content/block_raw] block {id} content diverges:\n  ref:       \
                     {ref_content:?}\n  block_raw: {sut_c:?}"
                ));
            }
            None => {
                return Err(format!(
                    "[inv-block-content/block_raw] block {id} present in ref (content \
                     {ref_content:?}) but missing from the SUT's block_raw snapshot"
                ));
            }
        }
    }
    Ok(())
}

fn compare_block_content_sql(
    sut: &BTreeMap<EntityUri, String>,
    ref_: &BTreeMap<EntityUri, String>,
) -> Result<(), String> {
    for (id, ref_content) in ref_ {
        match sut.get(id) {
            Some(sql_content) if sql_content == ref_content => {}
            Some(sql_content) => {
                return Err(format!(
                    "[inv-block-content/sql] block {id} content diverges:\n  ref: \
                     {ref_content:?}\n  sql: {sql_content:?}"
                ));
            }
            None => {
                return Err(format!(
                    "[inv-block-content/sql] block {id} present in ref but missing from SQL \
                     projection (ref content = {ref_content:?})"
                ));
            }
        }
    }
    Ok(())
}

// ─── Observable: per-block parent (the `inv-block-parent/*` family) ──────────

/// Per-block `parent_id`, keyed by non-seed block id, normalized so a root /
/// sentinel parent reads as `None` (mirroring [`RefBlockTree::parent_of`]).
/// Closes the re-parent-divergence gap the other block-tree invariants leave
/// open (`blocks-match` skips the `Parent` facet; `no_orphan` /
/// `no_parent_cycles` only check existence / termination). Sound on the memory
/// slice, which has no doc-id remapping across `file:`/`block:` doc identity.
/// Consolidates the hand-written `SutBackend` block-parent body.
pub struct BlockParent;

impl Observable for BlockParent {
    type Value = BTreeMap<EntityUri, Option<EntityUri>>;
    const NAME: &'static str = "block-parent";
}

pub fn block_parent() -> Correspondence<BlockParent> {
    Correspondence {
        ref_project: ref_block_parent,
        stores: vec![StoreProjection {
            id: "inv-block-parent/block_raw",
            store: "block_raw",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutBackend>()],
                sut_absent: Vec::new(),
                ref_present: vec![CapId::of::<dyn RefBlockTree>()],
            },
            extract: extract_block_parent_backend,
            compare: NamedCompare {
                name: "compare_block_parent{block_raw}",
                f: compare_block_parent_block_raw,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_block_parent(refs: &CapMap) -> Extraction<BTreeMap<EntityUri, Option<EntityUri>>> {
    let mut out = BTreeMap::new();
    for id in RefBlockTree::all_non_seed_block_ids(refs) {
        if crate::pbt::is_synthetic_ref_id(&id) {
            continue;
        }
        let parent = RefBlockTree::parent_of(refs, &id);
        out.insert(id, parent);
    }
    Extraction::Value(out)
}

fn extract_block_parent_backend<'a>(
    sut: &'a CapMap,
    _: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<BTreeMap<EntityUri, Option<EntityUri>>>> + 'a>> {
    // SUT parent per id, normalized: a root / sentinel parent reads as `None`,
    // mirroring `RefBlockTree::parent_of`'s contract.
    Box::pin(async move {
        Extraction::Value(
            sut.block_raw_snapshot()
                .await
                .into_iter()
                .map(|b| {
                    let parent = (!b.parent_id.is_no_parent() && !b.parent_id.is_sentinel())
                        .then_some(b.parent_id);
                    (b.id, parent)
                })
                .collect(),
        )
    })
}

fn compare_block_parent_block_raw(
    sut: &BTreeMap<EntityUri, Option<EntityUri>>,
    ref_: &BTreeMap<EntityUri, Option<EntityUri>>,
) -> Result<(), String> {
    for (id, ref_parent) in ref_ {
        // Id-set divergence is `inv-blocks-match-ref/block_raw`'s job; this
        // invariant only speaks to parent equality on shared ids.
        let Some(sut_parent_opt) = sut.get(id) else {
            continue;
        };
        if ref_parent != sut_parent_opt {
            return Err(format!(
                "[inv-block-parent/block_raw] block {id} parent diverges:\n  ref:       \
                 {ref_parent:?}\n  block_raw: {sut_parent_opt:?}"
            ));
        }
    }
    Ok(())
}

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
    _: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<Vec<Block>>> + 'a>> {
    Box::pin(async move { Extraction::Value(sut.org_block_snapshot().await) })
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

// ─── Observable: advice matviews (`inv-advice-matview-matches-ref`) ──────────

/// One (matview name, rows) pair. Rows are `(anchor_id, lesson_id,
/// shared_tag_count)` — the pre-suppression, un-capped matview contract.
type AdviceMatview = (String, Vec<(String, String, u32)>);

/// The synthesized advice matviews (ADR 0022 step-6), SQL-level twin of the
/// snapshot-level `inv-advice-rows-woven`. The reference projects the single
/// active rule's expected matview name ([`RefAdvice::advice_matview_name`]) +
/// its full un-suppressed, un-capped rows ([`RefAdvice::advice_matview_rows`]);
/// the SUT projects every `advice_rule_%` materialized view actually present in
/// `sqlite_master` with its rows. Suppression and top-K are read-time and
/// belong to the snapshot invariant ONLY — this twin is the raw matview
/// contract.
///
/// Driver-ladder localization: this twin flips GREEN the moment step-6
/// synthesis creates the matview, even while `inv-advice-rows-woven` stays RED
/// because the renderer has not yet woven the rows. Until synthesis lands the
/// SUT observes no such matview (empty), so a reference that expects one drives
/// the RED.
pub struct AdviceMatviews;

impl Observable for AdviceMatviews {
    type Value = Vec<AdviceMatview>;
    const NAME: &'static str = "advice-matview-matches-ref";
}

pub fn advice_matviews() -> Correspondence<AdviceMatviews> {
    Correspondence {
        ref_project: ref_advice_matviews,
        stores: vec![StoreProjection {
            id: "inv-advice-matview-matches-ref/matview",
            store: "matview",
            needs: Needs {
                sut_present: vec![CapId::of::<dyn SutAdviceMatview>()],
                sut_absent: Vec::new(),
                ref_present: vec![CapId::of::<dyn RefAdvice>()],
            },
            extract: extract_advice_matviews,
            compare: NamedCompare {
                name: "compare_advice_matviews",
                f: compare_advice_matviews,
            },
            converge: Converge::None,
        }],
    }
}

fn ref_advice_matviews(refs: &CapMap) -> Extraction<Vec<AdviceMatview>> {
    // ≤1 active rule (v1). `Some(name)` ⇒ exactly one expected matview carrying
    // the rule's rows; `None` ⇒ no advice matview may exist.
    match RefAdvice::advice_matview_name(refs) {
        None => Extraction::Value(Vec::new()),
        Some(name) => Extraction::Value(vec![(name, RefAdvice::advice_matview_rows(refs))]),
    }
}

fn extract_advice_matviews<'a>(
    sut: &'a CapMap,
    _: &'a CapMap,
) -> Pin<Box<dyn Future<Output = Extraction<Vec<AdviceMatview>>> + 'a>> {
    Box::pin(async move { Extraction::Value(SutAdviceMatview::advice_matviews(sut).await) })
}

/// Order-free multiset equality of a matview's rows.
fn sorted_rows(mut rows: Vec<(String, String, u32)>) -> Vec<(String, String, u32)> {
    rows.sort();
    rows
}

fn compare_advice_matviews(
    sut: &Vec<AdviceMatview>,
    ref_: &Vec<AdviceMatview>,
) -> Result<(), String> {
    match ref_.as_slice() {
        // No active rule ⇒ no advice matview may exist.
        [] => {
            if sut.is_empty() {
                return Ok(());
            }
            let names: Vec<&String> = sut.iter().map(|(n, _)| n).collect();
            Err(format!(
                "[inv-advice-matview-matches-ref/matview] no active advice rule, but the SUT has \
                 advice_rule_% matviews {names:?} (ghost matviews — synthesis left a view behind \
                 after the rule was deleted/deactivated)"
            ))
        }
        [(name, expected_rows)] => {
            if sut.len() != 1 {
                let names: Vec<&String> = sut.iter().map(|(n, _)| n).collect();
                return Err(format!(
                    "[inv-advice-matview-matches-ref/matview] expected exactly one advice matview \
                     '{name}', but the SUT has {} ({names:?}) — pre-step-6 synthesis has created \
                     none (EXPECTED RED), or created more than one",
                    sut.len(),
                ));
            }
            let (sut_name, sut_rows) = &sut[0];
            if sut_name != name {
                return Err(format!(
                    "[inv-advice-matview-matches-ref/matview] advice matview name diverges: \
                     reference expects '{name}', SUT has '{sut_name}'"
                ));
            }
            let want = sorted_rows(expected_rows.clone());
            let got = sorted_rows(sut_rows.clone());
            if want != got {
                return Err(format!(
                    "[inv-advice-matview-matches-ref/matview] advice matview '{name}' rows \
                     diverge (pre-suppression, un-capped):\n  reference: {want:?}\n  SUT:       \
                     {got:?}"
                ));
            }
            Ok(())
        }
        // v1 guarantees ≤1 active rule → ref side never yields >1 matview.
        _ => Err(format!(
            "[inv-advice-matview-matches-ref/matview] reference produced {} matviews; v1 \
             guarantees at most one active rule (harness bug)",
            ref_.len(),
        )),
    }
}

#[cfg(test)]
mod tests {
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

    // ── Pure comparator tests for the advice-matview twin ────────────────────

    use super::AdviceMatview;
    use super::compare_advice_matviews;

    fn mv(name: &str, rows: &[(&str, &str, u32)]) -> AdviceMatview {
        (
            name.to_string(),
            rows.iter()
                .map(|(a, l, n)| (a.to_string(), l.to_string(), *n))
                .collect(),
        )
    }

    #[test]
    fn advice_no_rule_no_matview_agrees() {
        assert!(compare_advice_matviews(&Vec::new(), &Vec::new()).is_ok());
    }

    #[test]
    fn advice_no_rule_but_ghost_matview_fails() {
        let sut = vec![mv("advice_rule_x", &[])];
        assert!(compare_advice_matviews(&sut, &Vec::new()).is_err());
    }

    #[test]
    fn advice_rule_but_absent_matview_fails_step4_red() {
        // The step-4→step-6 red case: ref expects the matview, synthesis hasn't
        // created it yet.
        let ref_ = vec![mv("advice_rule_l", &[("block:t", "block:a", 1)])];
        assert!(compare_advice_matviews(&Vec::new(), &ref_).is_err());
    }

    #[test]
    fn advice_matching_rows_order_free_agree() {
        let ref_ = vec![mv(
            "advice_rule_l",
            &[("block:t", "block:a", 2), ("block:t", "block:b", 1)],
        )];
        // Same multiset, different order → agree.
        let sut = vec![mv(
            "advice_rule_l",
            &[("block:t", "block:b", 1), ("block:t", "block:a", 2)],
        )];
        assert!(compare_advice_matviews(&sut, &ref_).is_ok());
    }

    #[test]
    fn advice_wrong_name_fails() {
        let ref_ = vec![mv("advice_rule_l", &[("block:t", "block:a", 1)])];
        let sut = vec![mv("advice_rule_other", &[("block:t", "block:a", 1)])];
        assert!(compare_advice_matviews(&sut, &ref_).is_err());
    }

    #[test]
    fn advice_row_divergence_fails() {
        let ref_ = vec![mv("advice_rule_l", &[("block:t", "block:a", 2)])];
        let sut = vec![mv("advice_rule_l", &[("block:t", "block:a", 1)])];
        assert!(compare_advice_matviews(&sut, &ref_).is_err());
    }
}
