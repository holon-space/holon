//! `RefSharedView` — the receiver-entitlement half of the two-instance sharing
//! oracle (true-sharing plan Inc1).
//!
//! @pbt kind ref
//! @pbt covers two-instance-share — whether the vault is shared, with whom, and
//!   how many owner→receiver sync rounds the model has applied.
//!
//! Reads the [`crate::pbt::sharing_state::SharingRefState`] whole-vault share.
//! On every single-instance draw nothing is shared, so `is_shared()` is false
//! and both two-instance invariants take their "receiver must hold nothing"
//! branch — which is also vacuously true there, because no receiver cap is
//! wired for them to select against.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_pbt_core::capabilities::Audience;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::PeerWrite;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::RefSharedViewMut;

use super::super::reference_state::ReferenceState;
use crate::pbt::sharing_state::RECEIVER_PRINCIPAL;

impl RefSharedView for ReferenceState {
    fn is_shared(&self) -> bool {
        self.sharing
            .vault_share
            .as_ref()
            .is_some_and(|a| !a.is_local_only())
    }

    fn receiver_principal(&self) -> String {
        RECEIVER_PRINCIPAL.to_string()
    }

    fn owner_to_receiver_rounds(&self) -> u64 {
        self.sharing.owner_to_receiver_rounds
    }

    fn shared_audience(&self) -> Audience {
        self.sharing.vault_share.clone().unwrap_or_default()
    }

    fn blocks_delivered_to_receiver(&self) -> BTreeSet<EntityUri> {
        self.sharing.blocks_delivered_to_receiver.clone()
    }

    fn peer_writes_delivered(&self) -> BTreeMap<EntityUri, PeerWrite> {
        self.sharing.peer_writes_delivered.clone()
    }

    fn peer_writes_pending(&self) -> BTreeMap<EntityUri, PeerWrite> {
        self.sharing.peer_writes_pending.clone()
    }
}

impl RefSharedViewMut for ReferenceState {
    fn apply_share_vault(&mut self, principal: &str) {
        self.sharing.share_vault_with(principal);
    }

    fn note_owner_to_receiver_round(&mut self) {
        self.sharing.owner_to_receiver_rounds += 1;
        // What the round could carry is what the owner holds now, so this is
        // also the set a later peer write may parent under.
        self.sharing.blocks_delivered_to_receiver =
            holon_pbt_core::capabilities::RefBlockTree::all_non_seed_block_ids(self);
    }

    fn note_receiver_to_owner_round(&mut self) {
        self.sharing.deliver_peer_writes();
    }

    fn note_peer_write(&mut self, id: EntityUri, write: PeerWrite) {
        self.sharing.note_peer_write(id, write);
    }
}
