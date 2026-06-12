//! `inv-block-content-matches-ref/block_raw` — per-block `content` equality
//! between the SUT's write-side `block_raw` snapshot and the reference model's
//! [`RefBlockTree`] view.
//!
//! The `SutSqlProjection`-bound sibling (`inv-block-content-matches-ref`) reads
//! the matview and only runs where Turso is wired. This variant reads the
//! convergent `block_raw` store via [`SutBackend::block_raw_snapshot`], so a
//! pure-memory slice (no Turso, no matview) can run it — the same capability
//! portability that moved `inv-no-orphan-blocks` off `SutSqlProjection`.
//!
//! Bound on `RefBlockTree + SutBackend`. Reference content comes through
//! [`RefBlockTree::block_content`], whose `Option<&str>` return is the reason
//! `CapMap` had to learn to hand out a borrowed provider (`expect_ref`); this
//! body is the first registry invariant to exercise that path through the
//! composed map.

use std::collections::HashMap;

use holon_pbt_core::capabilities::{EntityUri, RefBlockTree, SutBackend};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvBlockContentMatchesRefBackend;

impl InvBlockContentMatchesRefBackend {
    pub const ID: InvariantId = InvariantId("inv-block-content-matches-ref/block_raw");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlockContentMatchesRefBackend
where
    R: RefBlockTree,
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let sut_content: HashMap<EntityUri, String> = sut
            .block_raw_snapshot()
            .await
            .into_iter()
            .map(|b| (b.id, b.content))
            .collect();

        for id in ref_.all_non_seed_block_ids() {
            // Synthetic ref-side ids (`block::split-N`, …) are remapped to UUIDs
            // production-side; the convergent store has no row at the synthetic
            // id. The slice's job here is content equality on stable ids.
            if crate::pbt::is_synthetic_ref_id(&id) {
                continue;
            }
            // The borrowing read through `RefBlockTree` — forwarded through
            // `CapMap::expect_ref` when `ref_` is a composed map.
            let ref_content = match ref_.block_content(&id) {
                Some(c) => c.to_string(),
                None => continue,
            };
            match sut_content.get(&id) {
                Some(sut_c) if *sut_c == ref_content => {}
                Some(sut_c) => {
                    return InvariantResult::Fail(format!(
                        "[inv-block-content-matches-ref/block_raw] block {id} content \
                         diverges:\n  ref:       {ref_content:?}\n  block_raw: {sut_c:?}"
                    ));
                }
                None => {
                    return InvariantResult::Fail(format!(
                        "[inv-block-content-matches-ref/block_raw] block {id} present in \
                         ref (content {ref_content:?}) but missing from the SUT's \
                         block_raw snapshot"
                    ));
                }
            }
        }
        InvariantResult::Ok
    }
}
