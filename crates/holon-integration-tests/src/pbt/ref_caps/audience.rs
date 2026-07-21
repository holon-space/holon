//! `RefAudience` — the ADR 0028 C2/H3 directional-alignment oracle surface.
//!
//! @pbt kind ref
//! @pbt covers sharing-audience — per-block policy audience + per-block
//!   effective (container) audience + sharing epoch, read by
//!   `inv-audience-never-over-approximates`.
//!
//! Effective audience of a block = the audience of the container (doc) it is
//! currently in (`block_documents[block]`). Policy audience = owner intent.
//! Both default to local-only (owner-only). `shared_block_ids` is the union of
//! blocks carrying a non-local policy OR living in a non-local container — the
//! only blocks the over-approximation check is meaningful for. Empty on the
//! default keystone, so the oracle is vacuously green until crossings land.

use std::collections::BTreeSet;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::Audience;
use holon_pbt_core::capabilities::RefAudience;

use super::super::reference_state::ReferenceState;

impl RefAudience for ReferenceState {
    fn audience_epoch(&self) -> u64 {
        self.sharing.epoch
    }

    fn shared_block_ids(&self) -> BTreeSet<EntityUri> {
        let mut ids: BTreeSet<EntityUri> = BTreeSet::new();
        // Blocks with an explicit non-local policy audience.
        for (block, audience) in &self.sharing.policy_audience {
            if !audience.is_local_only() {
                ids.insert(block.clone());
            }
        }
        // Blocks living in a container with a non-local effective audience.
        let non_local_docs: BTreeSet<&EntityUri> = self
            .sharing
            .container_audience
            .iter()
            .filter(|(_, a)| !a.is_local_only())
            .map(|(doc, _)| doc)
            .collect();
        if !non_local_docs.is_empty() {
            for (block, doc) in &self.domain.block_state.block_documents {
                if non_local_docs.contains(doc) {
                    ids.insert(block.clone());
                }
            }
        }
        ids
    }

    fn block_policy_audience(&self, block: &EntityUri) -> Audience {
        self.sharing.policy_of(block)
    }

    fn block_effective_audience(&self, block: &EntityUri) -> Audience {
        // The block's effective audience is its container's audience.
        match self.domain.block_state.block_documents.get(block) {
            Some(doc) => self.sharing.container_of(doc),
            None => Audience::local_only(),
        }
    }
}
