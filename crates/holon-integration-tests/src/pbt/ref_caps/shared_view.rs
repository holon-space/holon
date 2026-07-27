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

use holon_pbt_core::capabilities::Audience;
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
}

impl RefSharedViewMut for ReferenceState {
    fn apply_share_vault(&mut self, principal: &str) {
        self.sharing.share_vault_with(principal);
    }

    fn note_owner_to_receiver_round(&mut self) {
        self.sharing.owner_to_receiver_rounds += 1;
    }
}
