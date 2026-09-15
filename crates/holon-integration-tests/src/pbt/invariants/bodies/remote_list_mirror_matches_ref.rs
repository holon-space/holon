//! `inv-remote-list-mirror-matches-ref` — the composed keystone's mirror of a
//! configured remote list equals the list the peer serves.
//!
//! @pbt kind oracle correspondence
//! @pbt covers remote-list-mirror-matches-ref — the `RemoteListSync`
//!   transition runs the production round over the fixture peer and writes the
//!   local intents into the real mirror table; this compares that table,
//!   projected to the peer-authoritative columns, against the peer's list in
//!   the reference model.
//! @pbt slips-if-removed a round that stops reconciling — a pull that drops a
//!   column, a local intent that never reaches the table, a deletion the peer
//!   served that the mirror keeps — would leave the mirror diverged with
//!   nothing in the keystone to notice.
//!
//! `Needs SutRemoteListSync` (SUT) + `RefRemoteListSync` (ref); only the
//! Turso+frontend arm supplies `SutRemoteListSync`, so a Loro-only /
//! storage-only slice deselects honestly.

use holon_pbt_core::capabilities::RefRemoteListSync;
use holon_pbt_core::capabilities::SutRemoteListSync;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvRemoteListMirrorMatchesRef;

impl InvRemoteListMirrorMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-remote-list-mirror-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvRemoteListMirrorMatchesRef
where
    R: RefRemoteListSync,
    S: SutRemoteListSync,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let expected = ref_.remote_list_expected_rows();
        let actual = sut.remote_list_mirror_rows().await;
        if actual != expected {
            return InvariantResult::Fail(format!(
                "[inv-remote-list-mirror-matches-ref] the mirror does not match the peer's list\n  \
                 expected: {expected:?}\n  actual:   {actual:?}"
            ));
        }
        InvariantResult::Ok
    }
}
