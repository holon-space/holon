//! Phase 7 — `inv-backend-blocks-match-ref` (DEFERRED).
//!
//! Inline body lives at `sut.rs:4115–4291`.
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.live_blocks()` — a private `LiveData<Block>` cell on `E2ESut`
//! - `self.doc_uri_map` — private URI translation map
//! - `self.seed_block_ids` — computed from `ref_state.block_state.block_documents`
//! - `self.ref_blocks_no_seed` / `self.backend_blocks_no_seed` — computed locally
//! - The full `assert_blocks_equivalent` helper (holon-internal, not in pbt-core)
//!
//! None of these are exposed via any `Sut*` capability trait today. The
//! capability surface would need `SutSqlProjection::live_block_snapshot()` or
//! equivalent returning a typed `Vec<Block>` together with a `RefBlockTree`-side
//! `seed_block_ids()` helper. Wire in the phase that migrates
//! `inv-backend-blocks-match-ref` proper.

pub struct InvBackendBlocksMatchRef;

impl InvBackendBlocksMatchRef {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-backend-blocks-match-ref");
}

// `Invariant` impl intentionally omitted — body migration deferred.
// The wide PBT's inline assertion in `check_invariants_async` remains the
// live check until Phase 10.
