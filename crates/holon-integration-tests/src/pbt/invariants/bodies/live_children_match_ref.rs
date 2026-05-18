//! Phase 7 — `inv-live-children-match-ref` (DEFERRED).
//!
//! Inline body lives at `sut.rs:4370–4413` (called via
//! `self.assert_live_children_match_ref(ref_state)` at sut.rs:4378).
//! The helper's full body is at sut.rs:7047–7105.
//!
//! # Why deferred
//!
//! The body requires:
//! - `ref_state.block_state.blocks` — to enumerate parent URIs; no Ref* cap today
//! - `self.resolve_uri` (private) — to translate synthetic parent IDs to real UUIDs
//! - Direct engine SQL via `engine.execute_query("SELECT id FROM block_raw … ORDER BY sort_key")`
//!   — `SutSqlProjection` provides `all_block_ids` and `block_raw_row` but not a
//!   per-parent ordered-children query
//! - `ref_state.children_of(parent)` — no `RefBlockTree::sorted_children` cap yet
//!   bound to the wide-PBT's `ReferenceState`
//!
//! Also gated on `!nav_only` in `check_invariants_async` — `nav_only` is a
//! private field on `E2ESut`. Wire when: `SutSqlProjection` grows
//! `children_of_ordered(parent_id)`, `RefBlockTree` impl covers
//! `ReferenceState::children_of`, and resolve-uri is exposed via a cap accessor.

pub struct InvLiveChildrenMatchRef;

impl InvLiveChildrenMatchRef {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-live-children-match-ref");
}

// `Invariant` impl intentionally omitted — body migration deferred.
