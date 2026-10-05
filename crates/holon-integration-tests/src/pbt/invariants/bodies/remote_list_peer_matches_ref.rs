//! `inv-remote-list-peer-matches-ref` — the list app holds what Holon's
//! changes must have sent it, in the app's own fields.
//!
//! @pbt kind oracle correspondence
//! @pbt covers remote-list-peer-matches-ref — the `RemoteListSync` transition
//!   runs the production round against the fake list app; this compares the
//!   app's stored entries against the reference model's app.
//! @pbt slips-if-removed a round that keeps a Holon edit locally but never
//!   sends it, or sends a command the app stores differently than its own
//!   client would, leaves the mirror right and the app wrong.
//!
//! `Needs SutRemoteListSync` (SUT) + `RefRemoteListSync` (ref).

use holon_pbt_core::capabilities::RefRemoteListSync;
use holon_pbt_core::capabilities::SutRemoteListSync;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvRemoteListPeerMatchesRef;

impl InvRemoteListPeerMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-remote-list-peer-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvRemoteListPeerMatchesRef
where
    R: RefRemoteListSync,
    S: SutRemoteListSync,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let expected = ref_.remote_list_expected_peer_entries();
        let actual = sut.remote_list_peer_entries().await;
        if actual != expected {
            return InvariantResult::Fail(format!(
                "[inv-remote-list-peer-matches-ref] the list app does not hold what Holon's \
                 changes must have sent it ([place, name, cat, count, pickedCount])\n  \
                 expected: {expected:?}\n  actual:   {actual:?}"
            ));
        }
        InvariantResult::Ok
    }
}
