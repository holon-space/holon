//! The Turso storage-pipeline correspondence arms, co-located out of the central
//! table (co-location Phase 2) now that the correspondence framework + shared
//! `NonSeedBlocks` observable + `block_compare` leaves live on the pbt-core floor.
//!
//! Each `pub fn <observable>()` builds one [`Correspondence`]: one reference
//! projection + N SUT store projections. The crate's [`pbt_contribution`] splices
//! `<observable>().wire()` into the composed catalog — adding a store or
//! observable is an entry here, nothing else. Extraction/comparison strategies
//! are named `fn`s in this module (greppable wiring).
//!
//! ## What lives here
//! - `non_seed_blocks()` — the two Turso arms (`block_raw`, `matview`) of the
//!   SHARED [`NonSeedBlocks`] observable (whose struct + `ref_non_seed_blocks`
//!   stay on the pbt-core floor; the `/loro` arm is contributed by
//!   `holon-loro-testing`).
//! - `block_content()`, `block_parent()`, `advice_matviews()` — whole observables
//!   whose every arm is Turso-owned, so their observable struct + `impl
//!   Observable` + `ref_project` move here entirely (the `Observable` trait is the
//!   foreign trait; the struct is local — the orphan rule is satisfied).
//!
//! ## Ref-state independence (plan §4)
//! Every projection reads `Ref*`/`Sut*` capability traits through the composed
//! [`CapMap`] only — never the central concrete `ReferenceState`. A guard test
//! (`tests/no_ref_state_dep.rs`) fails the build if this crate ever reaches for it.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;

use holon_api::{Block, EntityUri};
use holon_pbt_core::block_compare::{BlockFacet, compare_block_fields, compare_block_subset};
use holon_pbt_core::capabilities::{
    RefAdvice, RefBackend, RefBlockTree, SutAdviceMatview, SutBackend, SutSqlProjection,
};
use holon_pbt_core::composition::{CapId, CapInvariant, CapMap, Needs};
use holon_pbt_core::correspondence::{
    Converge, Correspondence, Extraction, NamedCompare, Observable, StoreProjection,
};
use holon_pbt_core::invariant::InvariantResult;
use holon_pbt_core::observables::{NonSeedBlocks, is_synthetic_ref_id, ref_non_seed_blocks};

/// Collect every wired `CapInvariant` this crate contributes, in the order the
/// static footprint enumerates them (the anti-rot test holds them in lockstep).
pub fn wire_all() -> Vec<Box<dyn CapInvariant>> {
    let mut wired: Vec<Box<dyn CapInvariant>> = Vec::new();
    wired.extend(non_seed_blocks().wire());
    wired.extend(block_content().wire());
    wired.extend(block_parent().wire());
    wired.extend(advice_matviews().wire());
    wired
}

// ─── Observable: non-seed blocks (the `inv-blocks-match-ref/*` family) ──────
//
// The SHARED `NonSeedBlocks` observable lives on the pbt-core floor (its struct +
// `ref_non_seed_blocks`). This crate contributes its two Turso storage-pipeline
// arms — `block_raw` (write-side source of truth) and `matview` (CDC mirror). The
// `/loro` arm is contributed by `holon-loro-testing`; the `/org` arm stays a
// hand-written central invariant (renderer-canonical sibling order, Phase 3).

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
        ],
    }
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

fn compare_matview_fields(sut: &Vec<Block>, ref_: &Vec<Block>) -> Result<(), String> {
    let label = "inv-blocks-match-ref/matview";
    check_block_id_set(label, sut, ref_)?;
    compare_block_fields(label, sut, ref_)
}

fn compare_block_raw_subset(sut: &Vec<Block>, ref_: &Vec<Block>) -> Result<(), String> {
    let label = "inv-blocks-match-ref/block_raw";
    check_block_id_set(label, sut, ref_)?;
    match compare_block_subset(
        label,
        sut,
        ref_,
        &[BlockFacet::Content, BlockFacet::Properties],
    ) {
        InvariantResult::Ok => Ok(()),
        InvariantResult::Fail(msg) => Err(msg),
        // `compare_block_subset` never skips; a Skip here would be a harness
        // bug hidden inside a comparator — surface it as a failure.
        InvariantResult::Skipped(reason) => Err(format!(
            "[{label}] unexpected Skip from compare_block_subset: {reason}"
        )),
    }
}

/// The block-id-set half of the `inv-blocks-match-ref/*` SQL-projection arms,
/// checked as TWO independently-reported, independently-scopable directions
/// BEFORE the field comparison. Splitting the directions is what makes this
/// invariant "powerful": a ref-expected id that never landed reads as loud
/// INGEST DATA LOSS, distinct from a projection carrying an extra id.
///
/// The id-set is the same non-seed set both sides already filtered to (the
/// extractors drop `seed_block_ids`); `normalize_block` never touches `id`, so
/// comparing raw ids here matches the key space the field comparators use.
fn check_block_id_set(label: &str, sut: &[Block], ref_: &[Block]) -> Result<(), String> {
    let sut_ids: BTreeSet<EntityUri> = sut.iter().map(|b| b.id.clone()).collect();
    let ref_ids: BTreeSet<EntityUri> = ref_.iter().map(|b| b.id.clone()).collect();
    // MISSING first, then SPURIOUS. The two checks are separate functions on
    // purpose (requirement of the consolidation ruling): the missing-side one
    // takes NO filter and never will, so no future scoping can ever weaken the
    // data-loss direction — see `check_no_missing_ids`.
    check_no_missing_ids(label, &sut_ids, &ref_ids)?;
    // The spurious direction alone is filterable. Today `|_| true` admits every
    // spurious id (no exclusion applied); a future keystone-wiring fork may pass
    // a real predicate here — see `check_no_spurious_ids`.
    check_no_spurious_ids(label, &sut_ids, &ref_ids, |_| true)
}

/// MISSING-in-SUT — the INGEST DATA LOSS direction. Every reference-expected
/// non-seed block id must be PRESENT in the SUT projection; one that was parsed
/// but never landed is silent ingest data loss (e.g. a forward
/// `:REQUIRES:`/`:BLOCKED-BY:` target-FK abort dropping every block from the
/// offending one onward — dogfood 2026-07-10 P0).
///
/// This function takes NO filter, BY DESIGN: the missing direction is
/// structurally unscopable. There is never a legitimate reason for a
/// reference-ingested block to be absent from the projection, so — unlike the
/// spurious direction — no future exclusion may ever be threaded through it.
fn check_no_missing_ids(
    label: &str,
    sut_ids: &BTreeSet<EntityUri>,
    ref_ids: &BTreeSet<EntityUri>,
) -> Result<(), String> {
    let missing: Vec<&EntityUri> = ref_ids.difference(sut_ids).collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "[{label}] INGEST DATA LOSS: {} reference-expected block id(s) were parsed but never \
         landed in the SUT projection (a forward `:REQUIRES:`/`:BLOCKED-BY:` target-FK abort \
         silently drops every block from the offending one onward), missing: {:?}",
        missing.len(),
        missing.iter().take(10).collect::<Vec<_>>(),
    ))
}

/// SPURIOUS-in-SUT — an id present in the projection but absent from the
/// reference. Keeps the pre-consolidation reporting.
///
/// Unlike the missing direction, this one takes a `keep` predicate so a future
/// keystone-wiring fork (the AdvanceDay capstone) can EXCLUDE rule-produced
/// blocks — which are legitimately absent from the ref model — from the spurious
/// set. The exclusion is confined to THIS direction by construction: the missing
/// side takes no filter, so it is physically impossible to weaken the data-loss
/// direction the same way. The rule-block exclusion itself is NOT implemented
/// here — today every caller passes `|_| true`.
fn check_no_spurious_ids(
    label: &str,
    sut_ids: &BTreeSet<EntityUri>,
    ref_ids: &BTreeSet<EntityUri>,
    keep: impl Fn(&EntityUri) -> bool,
) -> Result<(), String> {
    let spurious: Vec<&EntityUri> = sut_ids.difference(ref_ids).filter(|id| keep(id)).collect();
    if spurious.is_empty() {
        return Ok(());
    }
    Err(format!(
        "[{label}] block id set diverges from reference\n  spurious in {label}: {:?}",
        spurious.iter().take(10).collect::<Vec<_>>(),
    ))
}

// ─── Observable: per-block content (the `inv-block-content/*` family) ────────

/// Per-block `content`, keyed by non-seed block id, as each store sees it. The
/// reference projection enumerates the reference's non-synthetic non-seed block
/// ids ([`RefBlockTree::all_non_seed_block_ids`]) and answers content per id via
/// the borrow-returning [`RefBlockTree::block_content`]; ids the reference has
/// no content for are dropped (they cannot be compared — the old body's
/// `None => continue`). Consolidates the two hand-written per-block-content
/// bodies (the `SutSqlProjection` SQL-column read → `/sql`; the `SutBackend`
/// snapshot → `/block_raw`). Synthetic ref ids (`block::split-N`) are remapped to
/// UUIDs production-side, so the SUT has no row at the synthetic id — skipped on
/// both projection paths (content equality on stable ids only; the wider PBT
/// reconciles synthetic-id mapping).
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
        if is_synthetic_ref_id(&id) {
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
            if is_synthetic_ref_id(&id) {
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
        if is_synthetic_ref_id(&id) {
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

// ─── Observable: advice matviews (`inv-advice-matview-matches-ref`) ──────────

/// One (matview name, rows) pair. Rows are `(anchor_id, lesson_id,
/// shared_tag_count)` — the pre-suppression, un-capped matview contract.
type AdviceMatview = (String, Vec<(String, String, u32)>);

/// The synthesized advice matviews (ADR 0022 step-6), SQL-level twin of the
/// snapshot-level `inv-advice-rows-woven`. The reference projects the single
/// active rule's expected matview name ([`RefAdvice::advice_matview_name`]) + its
/// full un-suppressed, un-capped rows ([`RefAdvice::advice_matview_rows`]); the
/// SUT projects every `advice_rule_%` materialized view actually present in
/// `sqlite_master` with its rows. Suppression and top-K are read-time and belong
/// to the snapshot invariant ONLY — this twin is the raw matview contract.
///
/// Driver-ladder localization: this twin flips GREEN the moment step-6 synthesis
/// creates the matview, even while `inv-advice-rows-woven` stays RED because the
/// renderer has not yet woven the rows. Until synthesis lands the SUT observes no
/// such matview (empty), so a reference that expects one drives the RED.
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
                "[inv-advice-matview-matches-ref/matview] no active advice rule, but the SUT \
                 has advice_rule_% matviews {names:?} (ghost matviews — synthesis left a view \
                 behind after the rule was deleted/deactivated)"
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
                    "[inv-advice-matview-matches-ref/matview] advice matview '{name}' rows diverge \
                     (pre-suppression, un-capped):\n  reference: {want:?}\n  SUT:       {got:?}"
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
    use super::{AdviceMatview, compare_advice_matviews};

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
