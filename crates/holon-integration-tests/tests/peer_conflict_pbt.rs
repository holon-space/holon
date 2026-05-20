//! `peer_conflict_pbt` — Phase 3 axis 3 (same-block concurrency).
//!
//! Two parts:
//!
//! - **Fixture replay** (`peer_conflict_pbt_fixtures`): deterministic CI
//!   regression for the conflict path — a peer edits a block's content, the
//!   primary concurrently edits the same block (External org write), then
//!   `MergeFromPeer` runs the real Loro merge. The reference models the
//!   outcome via the `loro_merge_text` oracle
//!   (`merge_peer_blocks_into_primary`); divergence means the oracle
//!   mis-models production merge/tie-break semantics. This replaces a
//!   generator-eligibility unit test: the fixture exercises the real SUT
//!   merge end-to-end, not just strategy sampling, and runs env-free
//!   (fixtures replay literal transitions — the `extended_gen_enabled()`
//!   gate on the generator arm doesn't apply).
//!
//! - **Sweep**: a dense peer-flow slice (AddPeer/PeerEdit/ApplyMutation/
//!   merge/sync) — the full-coverage slices sample these transitions too
//!   thinly to open conflict windows (0 firings in 24 weighted cases).
//!   Under `HOLON_PBT_EXTENDED_GEN=1` the `ApplyMutation` conflict arm
//!   participates, generating primary edits on peer-modified blocks.

#![cfg(feature = "pbt")]

use holon_integration_tests::declare_pbt_slice;
use holon_integration_tests::pbt::invariants::bodies::block_content_matches_ref::InvBlockContentMatchesRef;
use holon_integration_tests::pbt::standard_pbt_config;
use holon_integration_tests::pbt::transitions::{
    AddPeer, ApplyMutation, MergeFromPeer, PeerEdit, SyncWithPeer,
};

/// `standard_pbt_config` + the primary Loro peer id pinned below the SUT's
/// peer ids (peer_idx+100) so concurrent-insert tie-breaking matches the
/// reference's `loro_merge_text` oracle (primary=1 < peer=2). Without the
/// pin the primary gets a random u64 and the merge interleaving flips per
/// run. (`run_fixtures` pins independently for the fixtures test.)
fn peer_conflict_pbt_config() -> proptest::test_runner::Config {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    standard_pbt_config("peer_conflict_pbt")
}

declare_pbt_slice! {
    test_fn: peer_conflict_pbt,
    wiring: holon_pbt_core::ComponentSet::full_headless().wiring,
    transitions: [
        preset lifecycle,
        preset org_writes,
        AddPeer,
        PeerEdit,
        ApplyMutation,
        MergeFromPeer,
        SyncWithPeer,
    ],
    invariants: [InvBlockContentMatchesRef],
    proptest_config: peer_conflict_pbt_config(),
    steps: 3..12,
    fixtures_dir: "tests/fixtures/peer_conflict_pbt",
}
