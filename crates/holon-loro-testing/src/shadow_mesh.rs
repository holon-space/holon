//! E-solid shadow Loro peer mesh — the oracle-side CRDT predictor.
//!
//! @pbt kind oracle
//! @pbt covers loro-slice (peer-mesh) — reference-side CRDT convergence
//!   predictor; runs REAL Loro merges fed from ref intent (see audit caveat:
//!   same engine as SUT ⇒ a pure Loro merge-ordering bug is in the blind spot).
//!
//! A `ShadowMesh` is a fresh Loro universe (primary doc, peer id 1, matching
//! the PBT-pinned production primary; shadow peers at `100 + idx`, matching
//! `LoroSut::apply_add_peer`) that the reference model drives through the SAME
//! `multi_peer`/`peer_ops` helpers the SUT uses. It PREDICTS CRDT-arbitrary
//! outcomes (fractional-index tie-breaks = op-id order, concurrent-text
//! interleaving) instead of adopting them from the SUT at check time.
//!
//! The only value that ever crosses SUT→oracle is a **scalar Lamport height**
//! (`SutLoroLog::loro_lamport_height`, fed through
//! `ReferenceState::clock_feed`): the shadow primary is clock-padded to the
//! SUT's height at each fork/sync/primary-edit boundary, which preserves
//! relative op-id order even though the production doc carries boot/engine
//! history the shadow never replays. Mechanism proven in
//! `holon_loro::multi_peer::clock_parity_spike` (s1–s9, incl. the s7 negative
//! control and the s9 fork+set_peer_id Clone spike); proven against the real
//! production boot in `structural_pbt::teeth::shadow_mesh_predicts_*`.

use std::collections::BTreeMap;
use std::fmt;

use holon::sync::multi_peer::TREE_NAME;
use holon::sync::multi_peer::{self};
use holon_api::Block;
use holon_api::EntityUri;
use loro::LoroDoc;

use crate::peer_ops;

/// One shadow Loro doc pinned to a fixed peer id.
///
/// `Clone` is a **deep fork**: `LoroDoc::fork()` mints a NEW random peer id
/// (continuing ops under it would change op-id tie-breaks), so the original id
/// is restored with `set_peer_id` — op counters continue seamlessly. proptest
/// clones `ReferenceState` per step and per case, so an `Arc`-shared alias
/// would corrupt replays; the deep fork is mandatory (spike s9).
pub struct ShadowDoc {
    doc: LoroDoc,
    peer_id: u64,
}

impl ShadowDoc {
    fn new(peer_id: u64) -> Self {
        Self {
            doc: multi_peer::init_doc(peer_id),
            peer_id,
        }
    }

    pub fn doc(&self) -> &LoroDoc {
        &self.doc
    }
}

impl Clone for ShadowDoc {
    fn clone(&self) -> Self {
        let doc = self.doc.fork();
        doc.set_peer_id(self.peer_id)
            .expect("restore original peer id on shadow fork");
        Self {
            doc,
            peer_id: self.peer_id,
        }
    }
}

impl fmt::Debug for ShadowDoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShadowDoc")
            .field("peer_id", &self.peer_id)
            .field("lamport_height", &multi_peer::lamport_height(&self.doc))
            .finish()
    }
}

/// The shadow universe: a primary doc + forked shadow peers, mirroring the
/// SUT's global doc + `LoroSut` peers. Lives on `ReferenceState` as
/// `Option<ShadowMesh>`, created lazily at the first `AddPeer`.
#[derive(Clone, Debug)]
pub struct ShadowMesh {
    pub primary: ShadowDoc,
    pub peers: Vec<ShadowDoc>,
}

impl ShadowMesh {
    /// Build a mesh whose primary holds exactly the ref's block map (bare
    /// stable ids, `content_text()` strings). Base op *shapes* don't matter —
    /// only base strings and the peers' own op ids do (spike s8) — so a
    /// one-shot seed is as good as replaying the production boot.
    pub fn seeded_from_blocks(blocks: &BTreeMap<EntityUri, Block>) -> Self {
        let mesh = Self {
            primary: ShadowDoc::new(1),
            peers: Vec::new(),
        };
        mesh.catch_up_primary(blocks);
        mesh
    }

    /// Clock-pad the shadow primary up to `target`. Lenient: a `target` at or
    /// below the current height is a no-op — generation-phase ref evolutions
    /// read a stale (cross-case shared) clock feed and are discarded before
    /// execution, which re-evolves the ref against fresh, monotonic heights.
    pub fn pad_primary_to(&self, target: u32) {
        if target > multi_peer::lamport_height(self.primary.doc()) {
            multi_peer::pad_to_height(self.primary.doc(), target);
        }
    }

    /// Fork a new shadow peer off the (already padded, already caught-up)
    /// primary. Peer id `100 + idx` — identical to `LoroSut::apply_add_peer`.
    pub fn fork_peer(&mut self) -> u64 {
        let peer_id = 100 + self.peers.len() as u64;
        let doc = multi_peer::init_doc(peer_id);
        doc.import(
            &self
                .primary
                .doc()
                .export(loro::ExportMode::Snapshot)
                .expect("export shadow primary snapshot"),
        )
        .expect("import shadow primary snapshot into shadow peer");
        self.peers.push(ShadowDoc { doc, peer_id });
        peer_id
    }

    /// Make the shadow primary's alive tree (membership, parent, content
    /// string) match the ref block map — the centralized primary mirror.
    /// Runs after every ref transition (padded first), so primary edits land
    /// lamport-exact inside peer-concurrency windows, new transitions are
    /// auto-covered, and a missed site self-heals at the next catch-up.
    /// Multiple edits in one transition collapse to one diff (accepted).
    pub fn catch_up_primary(&self, blocks: &BTreeMap<EntityUri, Block>) {
        let doc = self.primary.doc();

        let mut want: BTreeMap<String, (Option<String>, String)> = BTreeMap::new();
        for b in blocks.values() {
            let parent = if blocks.contains_key(&b.parent_id) {
                Some(b.parent_id.id().to_string())
            } else {
                None
            };
            want.insert(
                b.id.id().to_string(),
                (parent, b.content_text().to_string()),
            );
        }

        let mut have: BTreeMap<String, (Option<String>, String)> = BTreeMap::new();
        for (node, parent, content) in multi_peer::get_alive_nodes(doc) {
            let sid = peer_ops::read_node_stable_id(doc, node)
                .unwrap_or_else(|| panic!("shadow primary node {node:?} lacks a stable id"));
            let parent_sid = parent.and_then(|p| peer_ops::read_node_stable_id(doc, p));
            have.insert(sid, (parent_sid, content));
        }

        // Deletes first (a deleted ancestor cascades, so re-find each node).
        for sid in have.keys() {
            if !want.contains_key(sid)
                && let Some(node) = peer_ops::find_node_by_stable_id(doc, sid)
            {
                multi_peer::delete_block(doc, node);
            }
        }

        // Creates, parents-first (iterate to fixpoint over the BTreeMap's
        // deterministic order — determinism matters for replay stability).
        let mut pending: Vec<&str> = want
            .keys()
            .filter(|sid| !have.contains_key(*sid))
            .map(String::as_str)
            .collect();
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|sid| {
                let (parent, content) = &want[*sid];
                let parent_node = match parent {
                    None => None,
                    Some(p) => match peer_ops::find_node_by_stable_id(doc, p) {
                        Some(n) => Some(n),
                        None => return true, // parent not created yet — retry
                    },
                };
                multi_peer::create_block_with_id(doc, parent_node, content, sid);
                false
            });
            assert!(
                pending.len() < before,
                "shadow catch_up_primary: unsatisfiable parents for {pending:?}"
            );
        }

        // Moves + content updates for pre-existing nodes.
        for (sid, (want_parent, want_content)) in &want {
            let Some((have_parent, have_content)) = have.get(sid) else {
                continue; // freshly created above with the right parent+content
            };
            let node = peer_ops::find_node_by_stable_id(doc, sid)
                .unwrap_or_else(|| panic!("shadow primary lost alive node {sid}"));
            if have_parent != want_parent {
                let new_parent = want_parent.as_ref().map(|p| {
                    peer_ops::find_node_by_stable_id(doc, p)
                        .unwrap_or_else(|| panic!("shadow move target parent {p} missing"))
                });
                multi_peer::move_block(doc, node, new_parent).expect("shadow primary move_block");
            }
            if have_content != want_content {
                multi_peer::update_block(doc, node, want_content);
            }
        }
    }

    /// Bidirectional shadow sync — mirrors `LoroSut::apply_sync_with_peer`
    /// (`sync_docs_direct` on the production global doc).
    pub fn sync_peer_bidirectional(&self, peer_idx: usize) {
        multi_peer::sync_docs_direct(self.primary.doc(), &self.peers[peer_idx].doc);
    }

    /// Unidirectional peer→primary merge — mirrors
    /// `LoroSut::apply_merge_from_peer` (peer delta vs the primary's version
    /// vector, imported into the primary; the peer doc is untouched).
    pub fn merge_peer_into_primary(&self, peer_idx: usize) {
        let primary = self.primary.doc();
        let peer = &self.peers[peer_idx].doc;
        let vv = primary.oplog_vv();
        let delta = peer
            .export(loro::ExportMode::updates(&vv))
            .expect("export shadow peer delta");
        if !delta.is_empty() {
            primary.import(&delta).expect("import shadow peer delta");
        }
    }

    // ── Shadow peer ops (same `peer_ops` helpers `LoroSut` drives) ──

    pub fn peer_create(
        &self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        peer_ops::peer_create_block(
            &self.peers[peer_idx].doc,
            parent_stable_id,
            content,
            stable_id,
        );
    }

    pub fn peer_update(&self, peer_idx: usize, stable_id: &str, content: &str) {
        peer_ops::peer_update_block(&self.peers[peer_idx].doc, stable_id, content);
    }

    pub fn peer_delete(&self, peer_idx: usize, stable_id: &str) {
        peer_ops::peer_delete_block(&self.peers[peer_idx].doc, stable_id);
    }

    pub fn peer_char_insert(&self, peer_idx: usize, stable_id: &str, pos: usize, text: &str) {
        peer_ops::peer_insert_text(&self.peers[peer_idx].doc, stable_id, pos, text);
    }

    pub fn peer_char_delete(&self, peer_idx: usize, stable_id: &str, pos: usize, len: usize) {
        peer_ops::peer_delete_text(&self.peers[peer_idx].doc, stable_id, pos, len);
    }

    // ── Reads (the consume side) ──

    /// The shadow primary's converged text for `stable_id`.
    pub fn primary_content(&self, stable_id: &str) -> Option<String> {
        let doc = self.primary.doc();
        let node = peer_ops::find_node_by_stable_id(doc, stable_id)?;
        Some(multi_peer::read_text(&doc.get_tree(TREE_NAME), node))
    }

    /// The shadow peer's current text for `stable_id` (post char-edit reads).
    pub fn peer_content(&self, peer_idx: usize, stable_id: &str) -> Option<String> {
        let doc = &self.peers[peer_idx].doc;
        let node = peer_ops::find_node_by_stable_id(doc, stable_id)?;
        Some(multi_peer::read_text(&doc.get_tree(TREE_NAME), node))
    }

    /// The shadow primary's true child order under `parent_stable_id`
    /// (`None` = root level), as bare stable ids — the predicted tie-break
    /// order consumed by `merge_peer_blocks_into_primary`.
    pub fn primary_children_order(&self, parent_stable_id: Option<&str>) -> Vec<String> {
        let doc = self.primary.doc();
        let tree = doc.get_tree(TREE_NAME);
        let children = match parent_stable_id {
            Some(p) => {
                let parent = peer_ops::find_node_by_stable_id(doc, p)
                    .unwrap_or_else(|| panic!("shadow primary lacks parent {p}"));
                tree.children(parent).unwrap_or_default()
            }
            None => tree.roots(),
        };
        children
            .into_iter()
            .map(|c| {
                peer_ops::read_node_stable_id(doc, c)
                    .unwrap_or_else(|| panic!("shadow child {c:?} lacks a stable id"))
            })
            .collect()
    }
}
