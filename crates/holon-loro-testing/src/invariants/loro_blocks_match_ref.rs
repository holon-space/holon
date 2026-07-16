//! `inv-blocks-match-ref/loro` — the Loro store's arm of the shared
//! `NonSeedBlocks` correspondence, co-located here (Phase 1a follow-on) now
//! that the correspondence framework + observable + `block_compare` leaf live
//! on the pbt-core floor.
//!
//! @pbt oracle correspondence
//! @pbt covers loro-blocks-field-match — Loro store block-field divergence
//!   from the reference model (content/parent/marks drift in the SQL→Loro
//!   mirror), non-seed blocks only
//! @pbt slips-if-removed the SQL→Loro mirror drops or corrupts a block field
//!   (e.g. marks or parent) on outdent/split; the SQL arm still matches ref so
//!   the Loro-only divergence — the tree the CRDT-backed UI reads — ships
//!   silently
//!
//! The live Loro tree's non-seed blocks must field-match the reference model.
//! Unobservable (disclosed Skip) when Loro isn't enabled on the variant; strict
//! (`Converge::None`) otherwise — seeds materialize into the Loro store, so a
//! non-seed divergence is a real bug, and the harness settle guarantees
//! convergence before the check.
//!
//! Contributed as a SINGLE-store `Correspondence<NonSeedBlocks>`.
//! `Correspondence::wire()` emits one `CapInvariant` per store and
//! `run_selected` runs all invariants order-independently, so this per-crate
//! arm is exactly equivalent to holding it in the central multi-store table.

use std::future::Future;
use std::pin::Pin;

use holon_api::Block;
use holon_pbt_core::block_compare::compare_block_fields;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::Needs;
use holon_pbt_core::correspondence::Converge;
use holon_pbt_core::correspondence::Correspondence;
use holon_pbt_core::correspondence::Extraction;
use holon_pbt_core::correspondence::NamedCompare;
use holon_pbt_core::correspondence::StoreProjection;
use holon_pbt_core::observables::NonSeedBlocks;
use holon_pbt_core::observables::ref_non_seed_blocks;

/// The single-store Loro correspondence for the `NonSeedBlocks` observable.
pub fn loro_blocks_correspondence() -> Correspondence<NonSeedBlocks> {
    Correspondence {
        ref_project: ref_non_seed_blocks,
        stores: vec![StoreProjection {
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
        }],
    }
}

/// Wire the single Loro store arm into one `CapInvariant` for the catalog fold.
/// Fail-loud: the correspondence declares exactly one store, so `wire()` must
/// emit exactly one invariant.
pub fn wire() -> Box<dyn CapInvariant> {
    let mut wired = loro_blocks_correspondence().wire();
    assert_eq!(
        wired.len(),
        1,
        "loro_blocks_correspondence must emit exactly one store invariant",
    );
    wired.pop().expect("length asserted to be 1")
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

fn compare_loro_fields(sut: &Vec<Block>, ref_: &Vec<Block>) -> Result<(), String> {
    compare_block_fields("inv-blocks-match-ref/loro", sut, ref_)
}
