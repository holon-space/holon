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
use std::collections::BTreeSet;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::Audience;
use holon_pbt_core::capabilities::PeerWrite;

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

    /// WHOLE-VAULT share (true-sharing Inc1): the audience the root container —
    /// and therefore every block and every doc in the vault — is shared with.
    /// `None` ⇒ nothing is shared.
    ///
    /// A vault-wide default rather than a per-entry fan-out because H3 requires
    /// the policy audience to widen FIRST and STAY wider: a block created after
    /// the share must already carry the policy, or its effective (container)
    /// audience would over-approximate its policy audience the instant it
    /// lands. A default satisfies that for blocks the model has not seen yet;
    /// an eagerly-materialized map cannot.
    pub vault_share: Option<Audience>,

    /// Owner→receiver sync rounds the model has applied since the share. The
    /// convergence oracle only expects the receiver to hold owner state after
    /// at least one.
    pub owner_to_receiver_rounds: u64,

    /// The owner's non-seed block ids as of the last owner→receiver round —
    /// exactly what the model knows the receiver holds. A peer write must be
    /// parented under one of these, because the receiver cannot create a child
    /// of a block it has never received.
    pub blocks_delivered_to_receiver: BTreeSet<EntityUri>,

    /// Receiver-authored blocks not yet carried to the owner by a modeled
    /// receiver→owner round.
    pub peer_writes_pending: BTreeMap<EntityUri, PeerWrite>,

    /// Receiver-authored blocks the model expects on BOTH peers. The two-writer
    /// oracle asserts membership + parent identity over this map and nothing
    /// else: with two concurrent writers everything order-dependent belongs to
    /// the convergence law, not to a reference model that would have to
    /// re-implement RGA tiebreaks to predict it.
    pub peer_writes_delivered: BTreeMap<EntityUri, PeerWrite>,
}

/// The principal the two-instance slice's receiver acts as. Fixed: the model
/// needs one stable name for the audience, and a drawn principal would add a
/// dimension the oracle cannot observe on the SUT.
pub const RECEIVER_PRINCIPAL: &str = "receiver";

/// The principal the two-instance slice's owner acts as. Needed as its own name
/// because the audience is a function of the round's direction: a
/// receiver→owner round is addressed to the OWNER, and reusing one principal
/// for both was how the owner came to admit envelopes addressed to somebody
/// else.
pub const OWNER_PRINCIPAL: &str = "owner";

/// Environment switch for the TWO-WRITER alphabet — receiver-authored blocks
/// and the receiver→owner round that carries them.
pub const TWO_WRITER_ENV: &str = "HOLON_PBT_TWO_WRITER";

/// Is the two-writer alphabet drawable? Default **off**, and the reason is a
/// defect this switch does not own: once a receiver→owner round runs, the
/// owner's tree holds TWO nodes for every fixed id both instances seed
/// independently (`block:journals`, `block:root-layout`, the fixed-clock
/// journal day block), which reds `inv-blocks-match-ref/loro` on duplicate ids
/// and `inv-birth-contract-satisfied` on re-keyed page roots. That is the
/// fixed-id boot collision the layout-container increment owns; leaving these
/// arms drawable would make every landing gate flaky on it.
///
/// The model itself is NOT gated — it is built, wired, and exercised by
/// `a_receiver_authored_block_reaches_the_owner_and_the_two_writer_oracle_engages`,
/// and the authorship scoping it introduces runs unconditionally on the
/// production wire (whose version-vector exchange is bidirectional, so
/// peer-authored blocks reach the owner with no reverse round at all).
pub fn two_writer_alphabet_enabled() -> bool {
    std::env::var_os(TWO_WRITER_ENV).is_some_and(|v| v == "1")
}

impl SharingRefState {
    /// The owner-intended audience for a block. Falls back to the whole-vault
    /// share, then local-only.
    pub fn policy_of(&self, block: &EntityUri) -> Audience {
        self.policy_audience
            .get(block)
            .cloned()
            .or_else(|| self.vault_share.clone())
            .unwrap_or_default()
    }

    /// The effective audience of a container/doc. Falls back to the whole-vault
    /// share, then local-only.
    pub fn container_of(&self, doc: &EntityUri) -> Audience {
        self.container_audience
            .get(doc)
            .cloned()
            .or_else(|| self.vault_share.clone())
            .unwrap_or_default()
    }

    /// Share the WHOLE vault with `principal` (the Inc1 degenerate case: the
    /// vault is every container in the replication set, not one mega
    /// container). Bumps the epoch — a share is a policy edit.
    pub fn share_vault_with(&mut self, principal: &str) {
        let mut audience = self.vault_share.clone().unwrap_or_default();
        audience.0.insert(principal.to_string());
        self.vault_share = Some(audience);
        self.epoch += 1;
    }

    /// Remap block/doc uris into the SUT id space (used by
    /// `ReferenceState::with_resolved_doc_uris`). A no-op on the empty default.
    pub fn remapped(&self, map: &BTreeMap<EntityUri, EntityUri>) -> Self {
        let resolve = |u: &EntityUri| map.get(u).cloned().unwrap_or_else(|| u.clone());
        let resolve_write = |w: &PeerWrite| PeerWrite {
            parent: resolve(&w.parent),
            content: w.content.clone(),
        };
        Self {
            epoch: self.epoch,
            vault_share: self.vault_share.clone(),
            owner_to_receiver_rounds: self.owner_to_receiver_rounds,
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
            blocks_delivered_to_receiver: self
                .blocks_delivered_to_receiver
                .iter()
                .map(resolve)
                .collect(),
            peer_writes_pending: self
                .peer_writes_pending
                .iter()
                .map(|(k, v)| (resolve(k), resolve_write(v)))
                .collect(),
            peer_writes_delivered: self
                .peer_writes_delivered
                .iter()
                .map(|(k, v)| (resolve(k), resolve_write(v)))
                .collect(),
        }
    }

    /// Record a receiver-authored block, pending delivery to the owner.
    pub fn note_peer_write(&mut self, id: EntityUri, write: PeerWrite) {
        self.peer_writes_pending.insert(id, write);
    }

    /// A receiver→owner round carries every pending peer write to the owner.
    pub fn deliver_peer_writes(&mut self) {
        self.peer_writes_delivered
            .extend(std::mem::take(&mut self.peer_writes_pending));
    }
}
