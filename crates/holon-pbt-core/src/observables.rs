//! Shared correspondence *observables* — the logical quantities the composed PBT
//! compares across stores — lifted to the pbt-core floor (co-location Phase 1a
//! follow-on) so a store arm can be contributed from any companion `*-testing`
//! crate rather than only from the central table.
//!
//! An observable pairs a [`Value`](crate::correspondence::Observable::Value) type
//! with a single reference projection `fn`; each store then contributes a
//! [`StoreProjection`](crate::correspondence::StoreProjection) that extracts the
//! same value from its own store and compares. Only observables with a store arm
//! that has already co-located out of the central table live here; the rest stay
//! central until their subsystem's co-location phase.

use holon_api::Block;

use crate::capabilities::RefBackend;
use crate::composition::CapMap;
use crate::correspondence::{Extraction, Observable};

/// The set of non-seed blocks, as each storage-pipeline store sees it — the
/// `inv-blocks-match-ref/*` family. The reference projection is
/// [`RefBackend::non_seed_blocks`]; each store snapshot filters seed rows via
/// [`RefBackend::seed_block_ids`] (context read: it shapes comparability, it
/// never supplies the expected value).
pub struct NonSeedBlocks;

impl Observable for NonSeedBlocks {
    type Value = Vec<Block>;
    const NAME: &'static str = "blocks-match-ref";
}

/// Reference projection for [`NonSeedBlocks`]: the reference model's own
/// non-seed block set. Shared by every store arm of the observable (central
/// `block_raw`/`matview`, co-located `loro`) so they compare against one source.
pub fn ref_non_seed_blocks(refs: &CapMap) -> Extraction<Vec<Block>> {
    Extraction::Value(refs.non_seed_blocks())
}
