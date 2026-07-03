//! The correspondence tables — the registry's declarative entries.
//!
//! Each `pub fn <observable>()` builds one [`Correspondence`]: one reference
//! projection + N SUT store projections. The catalog splices
//! `<observable>().wire()` — adding a store or observable is an entry here,
//! nothing else. Extraction/comparison strategies are named `fn`s in this
//! module (greppable wiring; see the registry module doc's integrity rule).

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use holon_api::Block;
use holon_pbt_core::capabilities::{
    EntityUri, RefBackend, RefBlockTree, SutBackend, SutLoroLog, SutSqlProjection,
};
use holon_pbt_core::composition::{CapId, CapMap, Needs};
use holon_pbt_core::invariant::InvariantResult;

use crate::pbt::correspondence::{
    Converge, Correspondence, Extraction, NamedCompare, Observable, StoreProjection,
};
use crate::pbt::invariants::block_compare::{
    BlockFacet, compare_block_fields, compare_block_subset,
};

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
/// ids ([`RefBlockTree::all_non_seed_block_ids`]) and answers content per id via
/// the borrow-returning [`RefBlockTree::block_content`]; ids the reference has
/// no content for are dropped (they cannot be compared — the old body's
/// `None => continue`). Consolidates the two hand-written per-block-content
/// bodies (the `SutSqlProjection` SQL-column read → `/sql`; the `SutBackend`
/// snapshot → `/block_raw`). Synthetic ref ids (`block::split-N`, `block::bulk-N-M`)
/// are remapped to UUIDs production-side, so the SUT has no row at the synthetic
/// id — skipped on both projection paths (content equality on stable ids only;
/// the wider PBT reconciles synthetic-id mapping).
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
                    "[inv-block-content/block_raw] block {id} content \
                     diverges:\n  ref:       {ref_content:?}\n  block_raw: {sut_c:?}"
                ));
            }
            None => {
                return Err(format!(
                    "[inv-block-content/block_raw] block {id} present in \
                     ref (content {ref_content:?}) but missing from the SUT's \
                     block_raw snapshot"
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
                    "[inv-block-content/sql] block {id} content diverges:\n  \
                     ref: {ref_content:?}\n  sql: {sql_content:?}"
                ));
            }
            None => {
                return Err(format!(
                    "[inv-block-content/sql] block {id} present in ref \
                     but missing from SQL projection (ref content = {ref_content:?})"
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
                "[inv-block-parent/block_raw] block {id} parent diverges:\n  \
                 ref:       {ref_parent:?}\n  block_raw: {sut_parent_opt:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::{run_with_seeded_ref, seed_ref};

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
}
