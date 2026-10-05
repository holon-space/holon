//! `inv-remote-list-peer-matches-ref` wired into the composed catalog — the
//! list app holds what a `RemoteListSync` round must have sent it.
//!
//! `Needs SutRemoteListSync` (SUT) + `RefRemoteListSync` (ref). Only the
//! Turso+frontend arm supplies `SutRemoteListSync`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefRemoteListSync;
use holon_pbt_core::capabilities::SutRemoteListSync;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::remote_list_peer_matches_ref::InvRemoteListPeerMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvRemoteListPeerMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRemoteListSync>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefRemoteListSync>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
