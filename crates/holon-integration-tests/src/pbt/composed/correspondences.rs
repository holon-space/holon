//! The correspondence tables — the registry's declarative entries.
//!
//! Each `pub fn <observable>()` builds one [`Correspondence`]: one reference
//! projection + N SUT store projections. The catalog splices
//! `<observable>().wire()` — adding a store or observable is an entry here,
//! nothing else. Extraction/comparison strategies are named `fn`s in this
//! module (greppable wiring; see the registry module doc's integrity rule).

use std::future::Future;
use std::pin::Pin;

use holon_api::Block;
use holon_pbt_core::capabilities::{RefBackend, SutBackend, SutLoroLog, SutSqlProjection};
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
