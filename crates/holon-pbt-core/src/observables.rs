//! Shared correspondence *observables* — the logical quantities the composed
//! PBT compares across stores — lifted to the pbt-core floor (co-location Phase
//! 1a follow-on) so a store arm can be contributed from any companion
//! `*-testing` crate rather than only from the central table.
//!
//! An observable pairs a [`Value`](crate::correspondence::Observable::Value)
//! type with a single reference projection `fn`; each store then contributes a
//! [`StoreProjection`](crate::correspondence::StoreProjection) that extracts
//! the same value from its own store and compares. Only observables with a
//! store arm that has already co-located out of the central table live here;
//! the rest stay central until their subsystem's co-location phase.

use holon_api::Block;
use holon_api::EntityUri;

use crate::capabilities::RefBackend;
use crate::composition::CapMap;
use crate::correspondence::Extraction;
use crate::correspondence::Observable;

/// Whether `id` is a ref-side SYNTHETIC placeholder the SUT replaces with a
/// real UUID. Only split suffixes are: the reference `split_block` transition
/// mints `block::split-N` (note the double colon) and the SUT later binds it to
/// a freshly-minted UUID. Bulk ids (`block:bulk-N-i`) are NOT synthetic — they
/// are written deterministically on BOTH sides, so they appear verbatim in
/// `block_raw`/CDC. Single source of truth (lifted to the pbt-core floor so a
/// co-located store arm can filter synthetic ids without reaching into the
/// central crate); substring sniffs like `contains(":split-")` false-positive
/// on legitimately named blocks (e.g. `block:split-target-block`).
pub fn is_synthetic_ref_id(id: &EntityUri) -> bool {
    id.as_str().starts_with("block::split-")
}

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
/// `block_raw`/`matview`, co-located `loro`) so they compare against one
/// source.
pub fn ref_non_seed_blocks(refs: &CapMap) -> Extraction<Vec<Block>> {
    Extraction::Value(refs.non_seed_blocks())
}
