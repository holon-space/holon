//! `inv-blocks-match-ref/*` — the block-equivalence composite.
//!
//! One field comparison ([`crate::pbt::invariants::block_compare`]) run against
//! every store that holds blocks. Per-store *thin structs* keep MINIMAL caps so
//! a slim slice can run a single store (e.g. a SQL-only slice runs `/matview`
//! without needing Loro caps) — this is why it's separate structs sharing the
//! comparison fn, not one union-bound enum impl.
//!
//! Each store normalises to a `Vec<holon_api::Block>` (the snapshot, excluding
//! seed blocks) and delegates to `compare_block_fields`. Per-store nuance —
//! CDC-lag tolerance, "Loro disabled" — lives in the body; the comparison stays
//! dumb.
//!
//! Stores:
//! - **`/matview`** — the `block` matview / live mirror (`SutBackend`), with the
//!   Turso-IVM CDC-lag → `Skipped` downgrade. Strict.
//! - **`/loro`** — the live Loro tree (`SutLoroLog::loro_block_snapshot`).
//!   Strict (see the registry — seeds materialize in Loro since Jun 2026, so a
//!   divergence is a real bug). `Skipped` when Loro isn't enabled.
//! - **`/block_raw`** — the write-side `block_raw` table (`SutBackend::block_raw_snapshot`),
//!   compared on the `{content, properties}` SUBSET (it lacks the junction
//!   `tags`/`requires` columns the matview joins). Strict.
//! - **`/org`** — the blocks parsed back off the on-disk org files
//!   (`SutOrgRead::org_block_snapshot`) vs `RefBackend::org_blocks`. The only
//!   store that ALSO checks per-parent sibling ORDER (disk = the renderer's
//!   canonical order), subsuming the prior `assert_blocks_equivalent` +
//!   `assert_block_order`. Strict.
//!
//! `inv-live-children-match-ref` stays its own body: it checks SQL `sort_key`
//! order, not the org store's renderer-canonical sequence order.

use std::time::Duration;

use holon_api::block::Block;
use holon_pbt_core::capabilities::{
    RefBackend, SutBackend, SutLoroLog, SutOrgRead, SutSqlProjection,
};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::pbt::invariants::block_compare::{
    BlockFacet, compare_block_fields, compare_block_subset, compare_blocks,
};
use crate::pbt::retry::retry_until_ok;

/// `/matview` store — deep field equality between the CDC-driven `block`
/// matview mirror (`live_block_snapshot`) and the reference, EXCLUDING seed
/// blocks. The `block` matview is a chained Turso IVM view, so a field update
/// (re-parent, content edit) can lag the write side; the check re-reads and
/// re-compares until it converges or a deadline elapses (see [`retry_until_ok`]).
pub struct InvBlocksMatchRefMatview;

impl InvBlocksMatchRefMatview {
    pub const ID: InvariantId = InvariantId("inv-blocks-match-ref/matview");
    const LABEL: &'static str = "inv-blocks-match-ref/matview";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlocksMatchRefMatview
where
    R: RefBackend,
    S: SutBackend + SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let seed_block_ids = ref_.seed_block_ids();
        let ref_blocks_no_seed = ref_.non_seed_blocks();

        // The `block` matview is a chained Turso IVM view fed by CDC, so a
        // field update (re-parent, content edit) can lag the write side. Keep
        // the eventual-consistency tolerance in the predicate that already
        // knows what it reads: re-read the mirror and re-compare to the
        // reference until it converges or the deadline elapses. No global
        // settle barrier coupling, no `Skip` — a genuinely stuck or wrong
        // matview still fails loudly after 5s, and the write side is
        // independently guarded by the Strict `/block_raw` store.
        let result = retry_until_ok(
            Duration::from_secs(5),
            Duration::from_millis(50),
            async || {
                let matview_blocks_no_seed: Vec<Block> = sut
                    .live_block_snapshot()
                    .await
                    .into_iter()
                    .filter(|b| !seed_block_ids.contains(&b.id))
                    .collect();
                compare_block_fields(Self::LABEL, &matview_blocks_no_seed, &ref_blocks_no_seed)
            },
        )
        .await;
        match result {
            Ok(()) => InvariantResult::Ok,
            Err(msg) => InvariantResult::Fail(msg),
        }
    }
}

/// `/block_raw` store — field-SUBSET equality between the write-side
/// `block_raw` table (the convergent source of truth) and the reference,
/// EXCLUDING seed blocks. Compares `{content, properties}` only: `block_raw`
/// lacks the junction `tags`/`requires` columns the matview joins, so a full
/// compare would false-fail. Strict — `block_raw` is settled by invariant time
/// (the `/matview` CDC-lag gate already treats it as truth). Subsumes the
/// former inline properties-in-cache check at a per-field-equality strength.
pub struct InvBlocksMatchRefBlockRaw;

impl InvBlocksMatchRefBlockRaw {
    pub const ID: InvariantId = InvariantId("inv-blocks-match-ref/block_raw");
    const LABEL: &'static str = "inv-blocks-match-ref/block_raw";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlocksMatchRefBlockRaw
where
    R: RefBackend,
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let seed_block_ids = ref_.seed_block_ids();
        let ref_blocks_no_seed = ref_.non_seed_blocks();
        let block_raw_no_seed: Vec<Block> = sut
            .block_raw_snapshot()
            .await
            .into_iter()
            .filter(|b| !seed_block_ids.contains(&b.id))
            .collect();

        compare_block_subset(
            Self::LABEL,
            &block_raw_no_seed,
            &ref_blocks_no_seed,
            &[BlockFacet::Content, BlockFacet::Properties],
        )
    }
}

/// `/loro` store — deep field equality between the live Loro tree and the
/// reference, EXCLUDING seed blocks. Strict (seeds now materialize into the
/// Loro store as `Block` instances, so a non-seed divergence is a real bug).
/// `Skipped` when Loro isn't enabled on the variant.
pub struct InvBlocksMatchRefLoro;

impl InvBlocksMatchRefLoro {
    pub const ID: InvariantId = InvariantId("inv-blocks-match-ref/loro");
    const LABEL: &'static str = "inv-blocks-match-ref/loro";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlocksMatchRefLoro
where
    R: RefBackend,
    S: SutLoroLog,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let Some(loro_blocks) = sut.loro_block_snapshot().await else {
            return InvariantResult::Skipped(format!(
                "[{}] Loro not enabled on this variant",
                Self::LABEL
            ));
        };

        let seed_block_ids = ref_.seed_block_ids();
        let ref_blocks_no_seed = ref_.non_seed_blocks();
        let loro_blocks_no_seed: Vec<Block> = loro_blocks
            .into_iter()
            .filter(|b| !seed_block_ids.contains(&b.id))
            .collect();

        match compare_block_fields(Self::LABEL, &loro_blocks_no_seed, &ref_blocks_no_seed) {
            Ok(()) => InvariantResult::Ok,
            Err(msg) => InvariantResult::Fail(msg),
        }
    }
}

/// `/org` store — the blocks parsed back off the on-disk org files
/// ([`SutOrgRead::org_block_snapshot`]) match the reference's org view
/// ([`RefBackend::org_blocks`] — non-seed, non-page, with the org parser's
/// `file:<filename>` parent for unresolved docs). Runs BOTH facets: field
/// equality AND per-parent sibling ORDER (`compare_blocks(check_order=true)`),
/// subsuming a field-equality + per-parent order check. Strict. This is the
/// only block store whose order is checked here: disk order is the renderer's
/// canonical order, whereas
/// `inv-live-children-match-ref` checks SQL `sort_key` order separately.
pub struct InvBlocksMatchRefOrg;

impl InvBlocksMatchRefOrg {
    pub const ID: InvariantId = InvariantId("inv-blocks-match-ref/org");
    const LABEL: &'static str = "inv-blocks-match-ref/org";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlocksMatchRefOrg
where
    R: RefBackend,
    S: SutOrgRead,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let org_blocks = sut.org_block_snapshot().await;
        let ref_blocks_org = ref_.org_blocks();
        compare_blocks(Self::LABEL, &org_blocks, &ref_blocks_org, true)
    }
}
