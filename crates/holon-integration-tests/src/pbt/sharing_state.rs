//! Sharing overlay fragment of the PBT reference model (ADR 0028 C2/H3).
//!
//! Holds the *policy audience* (owner intent, per block) and the *effective
//! container audience* (per doc), plus the sharing `epoch`. This is the model
//! substrate the `inv-audience-never-over-approximates` keystone oracle reads
//! through the [`holon_pbt_core::capabilities::RefAudience`] cap.
//!
//! On the default keystone nothing is shared: both maps are empty and the
//! oracle is vacuously green (every block is local-only, effective ∅ ⊆ policy
//! ∅). The fragment becomes load-bearing when the crossing transitions land
//! (Inc 4): a migration that widens/narrows an audience writes these maps, and
//! the oracle catches any wrong-order (leak-direction) migration that leaves a
//! block observably in a container whose policy no longer covers it.
//!
//! @pbt kind ref
//! @pbt covers sharing-audience — per-block policy audience + per-container
//!   effective audience + sharing epoch (ADR 0028 directional alignment).

use std::collections::BTreeMap;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::Audience;

/// Reference-model sharing overlay: audiences + epoch. Default = nothing
/// shared.
#[derive(Debug, Clone, Default)]
pub struct SharingRefState {
    /// Total-order sharing epoch (ADR 0028 H2). Bumped at a quiescent barrier;
    /// the oracle's quiescent form (`membership = policy extension`) is checked
    /// per-epoch. `0` on a ref modeling no crossings.
    pub epoch: u64,

    /// Owner-intended audience per block (`block_uri → Audience`). Absent ⇒
    /// local-only. Widening a share adds the recipient here FIRST (create in
    /// shared before the block is observable there).
    pub policy_audience: BTreeMap<EntityUri, Audience>,

    /// Effective audience per container/doc (`doc_uri → Audience`). Absent ⇒
    /// local-only. A block's effective audience is the audience of the doc it
    /// currently lives in (`block_documents[block]`).
    pub container_audience: BTreeMap<EntityUri, Audience>,
}

impl SharingRefState {
    /// The owner-intended audience for a block. Absent ⇒ local-only.
    pub fn policy_of(&self, block: &EntityUri) -> Audience {
        self.policy_audience.get(block).cloned().unwrap_or_default()
    }

    /// The effective audience of a container/doc. Absent ⇒ local-only.
    pub fn container_of(&self, doc: &EntityUri) -> Audience {
        self.container_audience
            .get(doc)
            .cloned()
            .unwrap_or_default()
    }

    /// Remap block/doc uris into the SUT id space (used by
    /// `ReferenceState::with_resolved_doc_uris`). A no-op on the empty default.
    pub fn remapped(&self, map: &BTreeMap<EntityUri, EntityUri>) -> Self {
        let resolve = |u: &EntityUri| map.get(u).cloned().unwrap_or_else(|| u.clone());
        Self {
            epoch: self.epoch,
            policy_audience: self
                .policy_audience
                .iter()
                .map(|(k, v)| (resolve(k), v.clone()))
                .collect(),
            container_audience: self
                .container_audience
                .iter()
                .map(|(k, v)| (resolve(k), v.clone()))
                .collect(),
        }
    }
}
