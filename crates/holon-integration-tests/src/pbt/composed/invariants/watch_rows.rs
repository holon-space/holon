//! The B5 watch-rows invariant wired into the composed catalog (E1 — the
//! `SutWatch` relocation onto the production reactive watch surface):
//! `inv-watch-rows-match-ref` — each watch's CDC-delivered rows agree, with the
//! two `block_raw` CDC-lag downgrades. Its sibling
//! `inv-active-watches-match-ref` (the registered watch-query-id *set*) is
//! auto-derived by the `capability_pair! { pub trait Watch … }` `#[compare]`
//! and registered in the catalog via `inv_pair_watch_watch_query_ids()`.
//!
//! `Needs SutWatch + RefWatch`. Selected only by a slice that supplies
//! `SutWatch` — today the `frontend_slice` over its real headless
//! `ReactiveEngine` watch surface; the storage/editor/windowed slices have no
//! `SutWatch` and deselect it (disclosed, not faked). With no registered
//! watches it is trivially `Ok` (empty sets), so it is inert until a slice
//! drives `register_query_watch` + seeds the ref's `active_watches`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefWatch, SutWatch};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::watch_rows_match_ref::InvWatchRowsMatchRef;

fn needs() -> Needs {
    Needs {
        sut_present: vec![CapId::of::<dyn SutWatch>()],
        sut_absent: Vec::new(),
        ref_present: vec![CapId::of::<dyn RefWatch>()],
    }
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
