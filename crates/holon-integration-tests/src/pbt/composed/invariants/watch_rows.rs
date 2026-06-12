//! The B5 watch invariants wired into the composed catalog (E1 — the
//! `SutWatchRows` relocation onto the production reactive watch surface):
//! `inv-active-watches-match-ref` (the registered watch-query-id *set* agrees with
//! the reference) and `inv-watch-rows-match-ref` (each watch's CDC-delivered rows
//! agree, with the two `block_raw` CDC-lag downgrades).
//!
//! Both `Needs SutWatchRows + RefWatches`. Selected only by a slice that supplies
//! `SutWatchRows` — today the `frontend_slice` over its real headless
//! `ReactiveEngine` watch surface; the storage/editor/windowed slices have no
//! `SutWatchRows` and deselect them (disclosed, not faked). With no registered
//! watches both are trivially `Ok` (empty sets), so they are inert until a slice
//! drives `register_query_watch` + seeds the ref's `active_watches`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefWatches, SutWatchRows};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::active_watches_match_ref::InvActiveWatchesMatchRef;
use crate::pbt::invariants::bodies::watch_rows_match_ref::InvWatchRowsMatchRef;

fn needs() -> Needs {
    Needs {
        sut_present: vec![CapId::of::<dyn SutWatchRows>()],
        sut_absent: Vec::new(),
        ref_present: vec![CapId::of::<dyn RefWatches>()],
    }
}

/// `inv-active-watches-match-ref` — the watch query-id *set* agrees with the ref.
pub fn wire_active_watches() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvActiveWatchesMatchRef,
        RunMode::Strict,
        needs(),
    ))
}

/// `inv-watch-rows-match-ref` — each watch's CDC rows agree (with `block_raw`
/// CDC-lag downgrades to `Skipped`).
pub fn wire_watch_rows() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvWatchRowsMatchRef,
        RunMode::Strict,
        needs(),
    ))
}
