//! `RefReadOnlyHomes` — which blocks the oracle believes are homed in a
//! `WriteTier::ReadOnly` format.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::RefReadOnlyHomes;

use crate::pbt::reference_state::ReferenceState;

impl RefReadOnlyHomes for ReferenceState {
    fn read_only_homed_blocks(&self) -> BTreeSet<EntityUri> {
        self.read_only.homes().clone()
    }

    fn refuses_creation_under(&self, parent: &EntityUri) -> bool {
        self.read_only.refusing_file(parent).is_some()
    }

    fn read_only_page_titles(&self) -> BTreeMap<EntityUri, String> {
        self.read_only
            .documents()
            .iter()
            .map(|page| {
                let title = &self
                    .domain
                    .block_state
                    .blocks
                    .get(page)
                    .unwrap_or_else(|| panic!("read-only page {page} is not modeled"))
                    .content;
                (page.clone(), title.clone())
            })
            .collect()
    }
}
