//! Reference state for a Loro-only peer. Extracted from `reference_state.rs`.

use std::collections::HashMap;

/// Reference state for a Loro-only peer.
#[derive(Debug, Clone)]
pub struct PeerRefState {
    pub peer_id: u64,
    pub blocks: HashMap<String, super::peer_ops::PeerBlock>,
    /// Stable IDs this peer has deleted since its last sync with the
    /// primary. Propagated by `SyncWithPeer`/`MergeFromPeer` so the
    /// primary's reference block map reflects the delete the production
    /// controller just applied via `subscribe_root`.
    pub deleted_stable_ids: std::collections::HashSet<String>,
    /// Stable IDs explicitly modified by PeerEdit::Update since AddPeer.
    /// Used by `merge_peer_blocks_into_primary` to distinguish peer edits
    /// from inherited-at-AddPeer blocks.
    pub modified_stable_ids: std::collections::HashSet<String>,
    /// Stable IDs created by PeerEdit::Create since the last sync. Only
    /// these are added to the primary on merge — inherited-at-AddPeer
    /// blocks the primary may have since deleted must NOT be re-added,
    /// because the actual Loro CRDT keeps primary-side deletes.
    pub created_stable_ids: std::collections::HashSet<String>,
}
