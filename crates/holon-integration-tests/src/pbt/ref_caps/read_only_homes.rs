//! `RefReadOnlyHomes` — which blocks the oracle believes are homed in a
//! `WriteTier::ReadOnly` format.

use std::collections::BTreeSet;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::RefReadOnlyHomes;

use crate::pbt::reference_state::ReferenceState;

impl RefReadOnlyHomes for ReferenceState {
    fn read_only_homed_blocks(&self) -> BTreeSet<EntityUri> {
        self.read_only.homes().clone()
    }
}
