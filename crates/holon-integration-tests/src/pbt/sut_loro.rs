//! Optional Loro validation layer for PBT.
//!
//! When Loro is enabled, reads all blocks from the LoroTree and compares them
//! against the reference model. With stable IDs, blocks already have UUID-based
//! IDs in their CRDT metadata, so no ID normalization is needed.

use std::collections::HashSet;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use holon::api::CoreOperations;
use holon::sync::LoroDocumentStore;
use holon::sync::LoroSyncControllerHandle;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_loro::LoroBackend;
use holon_pbt_core::capabilities::PeerEditOp;
use holon_pbt_core::capabilities::SutLoro;
use holon_pbt_core::capabilities::TextOp;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;
use holon_pbt_core::retry::retry_until_ok;
use holon_pbt_core::types::DocUriMap;
use tokio::sync::RwLock;

use crate::assertions::normalize_block;
use crate::test_environment::wait_for_loro_quiescence_on;

/// Encapsulates Loro-specific PBT validation **and** ownership of the
/// multi-peer sync surface. With stable IDs, blocks from the LoroTree already
/// carry UUID-based IDs in their metadata — no external_id mapping is needed.
///
/// Self-sufficient: holds the primary `doc_store`, the `LoroSyncController`
/// handle (for reactive quiescence after a peer import), and a shared
/// `doc_uri_map` (for resolving reference stable-ids to real Loro UUIDs).
/// `E2ESut` keeps a one-line-per-method forwarding `impl SutLoro` that simply
/// delegates here.
// BLOCKED on Phase 1a: binds concrete ReferenceState/SutHandle — cannot
// co-locate to holon-loro-testing until those are lifted to a shared crate.
pub struct LoroSut {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    /// Loro-only peer instances for multi-instance sync testing.
    peers: std::cell::RefCell<Vec<holon::sync::multi_peer::PeerState<()>>>,
    /// `LoroSyncController` handle for waiting on quiescence; `None` only if
    /// Loro was enabled without a controller (not expected in the wide PBT).
    sync_handle: Option<Arc<LoroSyncControllerHandle>>,
    /// Reference-model stable-id → resolved-UUID map, shared live with
    /// `E2ESut` so resolution sees ids minted after `LoroSut` construction.
    doc_uri_map: DocUriMap,
}

impl LoroSut {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        sync_handle: Option<Arc<LoroSyncControllerHandle>>,
        doc_uri_map: DocUriMap,
    ) -> Self {
        Self {
            doc_store,
            peers: std::cell::RefCell::new(Vec::new()),
            sync_handle,
            doc_uri_map,
        }
    }

    /// Resolve a reference-model stable_id to the actual stable_id used in the
    /// Loro tree. The reference model uses `b.id.id()` (e.g. "ref-doc-2"); the
    /// Loro tree uses the resolved UUID. Consults the shared `doc_uri_map`.
    fn resolve_stable_id(&self, stable_id: &str) -> String {
        let map = self.doc_uri_map.lock().unwrap();
        let block_uri = EntityUri::block(stable_id);
        if let Some(resolved) = map.get(&block_uri) {
            return resolved.id().to_string();
        }
        let file_uri = EntityUri::file(stable_id);
        if let Some(resolved) = map.get(&file_uri) {
            return resolved.id().to_string();
        }
        stable_id.to_string()
    }

    /// Wait for the controller to import + reconcile peer changes into SQL.
    async fn wait_for_quiescence(&self, timeout: Duration) {
        let Some(handle) = self.sync_handle.as_ref() else {
            return;
        };
        wait_for_loro_quiescence_on(handle, &self.doc_store, timeout).await;
    }

    /// Read all blocks from the LoroTree.
    /// Blocks already have stable UUID-based IDs from their CRDT metadata.
    pub async fn read_blocks(&self) -> anyhow::Result<Vec<Block>> {
        let store = self.doc_store.read().await;
        let collab_doc = store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get global doc: {}", e))?;

        let backend = LoroBackend::from_document(collab_doc);
        backend
            .get_all_blocks(holon::api::types::Traversal::ALL)
            .await
            .map_err(|e| anyhow::anyhow!("get_all_blocks failed: {}", e))
    }

    /// Snapshot blocks paired with their internal Loro fractional index — the
    /// ordering encoding the adapter keeps inside the tree (ADR 0005). Domain
    /// `Block` no longer carries it, so ordering invariants read it here.
    pub async fn read_block_snapshots(&self) -> anyhow::Result<Vec<holon::api::SnapshotBlock>> {
        let store = self.doc_store.read().await;
        let collab_doc = store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get global doc: {}", e))?;
        let doc_arc = collab_doc.doc();
        let doc = &*doc_arc;
        Ok(holon_loro::snapshot_blocks_from_doc(doc)
            .into_values()
            .collect())
    }

    /// Assert that the Loro tree matches the reference model.
    ///
    /// Reads all blocks from Loro, then compares against the reference blocks
    /// using the same normalization as SQL checks.
    /// Retries for up to 5s to allow reverse sync to complete.
    pub async fn assert_matches_reference(
        &self,
        ref_blocks: &[Block],
        seed_block_ids: &std::collections::HashSet<EntityUri>,
    ) {
        // Reverse sync may still be running — retry the read+compare until the
        // Loro tree converges to the reference or 5s elapses. `Err` carries the
        // last (loro, ref) snapshots so the final assert can diff them.
        let result = retry_until_ok(
            Duration::from_secs(5),
            Duration::from_millis(100),
            async || {
                let loro_blocks = self
                    .read_blocks()
                    .await
                    .unwrap_or_else(|e| panic!("[LoroSut] Failed to read Loro blocks: {e}"));

                let loro_filtered: Vec<_> = loro_blocks
                    .iter()
                    .filter(|b| !seed_block_ids.contains(&b.id))
                    .filter(|b| !b.is_page())
                    // Exclude page placeholder roots created by reverse sync.
                    .filter(|b| {
                        !(b.parent_id.is_no_parent() && b.content.is_empty() && b.tags.is_empty())
                    })
                    .cloned()
                    .collect();

                // Normalize page parent_ids on both sides. Pages are managed
                // separately (DocumentManager) and their identity mapping is tested
                // by the SQL assertions, not the Loro assertion.
                let ref_filtered: Vec<_> = ref_blocks.iter().filter(|b| !b.is_page()).collect();

                let loro_content_ids: HashSet<&EntityUri> =
                    loro_filtered.iter().map(|b| &b.id).collect();
                let ref_content_ids: HashSet<&EntityUri> =
                    ref_filtered.iter().map(|b| &b.id).collect();

                let normalize_doc_parent = |block: &Block, content_ids: &HashSet<&EntityUri>| {
                    let mut normalized = normalize_block(block);
                    if !normalized.parent_id.is_no_parent()
                        && !normalized.parent_id.is_sentinel()
                        && !content_ids.contains(&block.parent_id)
                    {
                        normalized.parent_id = EntityUri::block("__document_root__");
                    }
                    normalized
                };

                let mut loro_sorted: Vec<_> = loro_filtered
                    .iter()
                    .map(|b| normalize_doc_parent(b, &loro_content_ids))
                    .collect();
                let mut ref_sorted: Vec<_> = ref_filtered
                    .iter()
                    .map(|b| normalize_doc_parent(b, &ref_content_ids))
                    .collect();
                loro_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
                ref_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

                if loro_sorted == ref_sorted {
                    Ok(())
                } else {
                    Err((loro_sorted, ref_sorted))
                }
            },
        )
        .await;

        if let Err((loro_sorted, ref_sorted)) = result {
            let diagnostic = build_diagnostic(&loro_sorted, &ref_sorted);
            assert_eq!(
                loro_sorted, ref_sorted,
                "[LoroSut] Block content mismatch\n{}",
                diagnostic,
            );
        }
    }
}

// `?Send` because `LoroSut` is single-threaded (`RefCell` peers + `block_on`);
// no `RefCell` borrow is ever held across an `.await` (the one `peers.push` is
// the final synchronous statement of `apply_add_peer`; every indexed read is a
// short scoped borrow).
#[async_trait::async_trait(?Send)]
impl SutLoro for LoroSut {
    async fn apply_add_peer(&self) {
        tracing::trace!("[apply] AddPeer (peer_idx={})", self.peers.borrow().len());
        let store = self.doc_store.read().await;
        let global_doc = store
            .get_global_doc()
            .await
            .expect("Failed to get global doc for AddPeer");
        let snapshot = global_doc
            .export_snapshot()
            .expect("Failed to export snapshot for AddPeer");
        let peer_id = (self.peers.borrow().len() as u64) + 100;
        let peer_doc = holon::sync::multi_peer::init_doc(peer_id);
        peer_doc
            .import(&snapshot)
            .expect("Failed to import snapshot into peer");
        self.peers
            .borrow_mut()
            .push(holon::sync::multi_peer::PeerState {
                doc: peer_doc,
                peer_id,
                online: true,
                data: (),
            });
    }

    async fn apply_peer_edit(&self, peer_idx: usize, op: &PeerEditOp) {
        let peers = self.peers.borrow();
        let peer = &peers[peer_idx];
        tracing::trace!("[apply] PeerEdit peer_idx={} op={:?}", peer_idx, op);
        match op {
            PeerEditOp::Create {
                parent_stable_id,
                content,
                stable_id,
            } => {
                super::peer_ops::peer_create_block(
                    &peer.doc,
                    parent_stable_id.as_deref(),
                    content,
                    stable_id,
                );
            }
            PeerEditOp::Update { stable_id, content } => {
                let resolved = self.resolve_stable_id(stable_id);
                super::peer_ops::peer_update_block(&peer.doc, &resolved, content);
            }
            PeerEditOp::Delete { stable_id } => {
                let resolved = self.resolve_stable_id(stable_id);
                super::peer_ops::peer_delete_block(&peer.doc, &resolved);
            }
        }
    }

    async fn apply_peer_create(
        &self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        self.apply_peer_edit(
            peer_idx,
            &PeerEditOp::Create {
                parent_stable_id: parent_stable_id.map(str::to_owned),
                content: content.to_owned(),
                stable_id: stable_id.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_update(&self, peer_idx: usize, stable_id: &str, content: &str) {
        self.apply_peer_edit(
            peer_idx,
            &PeerEditOp::Update {
                stable_id: stable_id.to_owned(),
                content: content.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_delete(&self, peer_idx: usize, stable_id: &str) {
        self.apply_peer_edit(
            peer_idx,
            &PeerEditOp::Delete {
                stable_id: stable_id.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_char_insert(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    ) {
        self.apply_peer_char_edit(
            peer_idx,
            stable_id,
            &TextOp::Insert {
                pos_codepoint,
                text: text.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_char_delete(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    ) {
        self.apply_peer_char_edit(
            peer_idx,
            stable_id,
            &TextOp::Delete {
                pos_codepoint,
                len_codepoint,
            },
        )
        .await;
    }

    async fn apply_peer_char_edit(
        &self,
        peer_idx: usize,
        block_id: &str,
        op: &holon_pbt_core::capabilities::TextOp,
    ) {
        use holon_pbt_core::capabilities::TextOp;
        let peers = self.peers.borrow();
        let peer = &peers[peer_idx];
        let resolved_id = self.resolve_stable_id(block_id);
        match op {
            TextOp::Insert {
                pos_codepoint,
                text,
            } => {
                super::peer_ops::peer_insert_text(&peer.doc, &resolved_id, *pos_codepoint, text);
            }
            TextOp::Delete {
                pos_codepoint,
                len_codepoint,
            } => {
                super::peer_ops::peer_delete_text(
                    &peer.doc,
                    &resolved_id,
                    *pos_codepoint,
                    *len_codepoint,
                );
            }
        }
    }

    async fn apply_sync_with_peer(&self, peer_idx: usize) {
        tracing::trace!("[apply] SyncWithPeer peer_idx={}", peer_idx);
        {
            let store = self.doc_store.read().await;
            let global_doc = store
                .get_global_doc()
                .await
                .expect("Failed to get global doc for SyncWithPeer");
            let primary_doc = global_doc.doc();
            let primary = &*primary_doc;
            let peers = self.peers.borrow();
            let peer = &peers[peer_idx];
            holon::sync::multi_peer::sync_docs_direct(primary, &peer.doc);
        }
        // Give the controller's spawned task time to process the
        // peer import via subscribe_root → on_loro_changed → SQL.
        self.wait_for_quiescence(Duration::from_secs(10)).await;
    }

    async fn apply_merge_from_peer(&self, peer_idx: usize) {
        tracing::trace!("[apply] MergeFromPeer peer_idx={}", peer_idx);
        {
            let store = self.doc_store.read().await;
            let global_doc = store
                .get_global_doc()
                .await
                .expect("Failed to get global doc for MergeFromPeer");
            // One-directional merge: export the peer's delta relative
            // to the primary's current version and import it into the
            // primary. The raw `doc.import` is enough — the
            // `LoroSyncController`'s `subscribe_root` will fire and
            // reconcile the diff into SQL via the command bus.
            let primary_doc = global_doc.doc();
            let primary = &*primary_doc;
            let peers = self.peers.borrow();
            let peer = &peers[peer_idx];
            let peer_vv = primary.oplog_vv();
            let delta = peer
                .doc
                .export(loro::ExportMode::updates(&peer_vv))
                .expect("Failed to export peer delta");
            if !delta.is_empty() {
                primary.import(&delta).expect("Failed to import peer delta");
            }
        }
        self.wait_for_quiescence(Duration::from_secs(10)).await;
    }

    /// TODO: Not wired: the capability trait models lag-based stale peers
    /// (`lag_steps: usize`) but `SutHandle::apply_create_stale_loro` takes
    /// `(org_filename, LoroCorruptionType)` — a pre-startup file-corruption
    /// concept. Phase 7 will decide whether to reconcile the two models or
    /// keep them separate. Until then this panics loudly if called.
    async fn apply_create_stale_peer(&self, _: usize) {
        unimplemented!(
            "SutLoro::apply_create_stale_peer: lag_steps-based peer snapshots are not wired yet. \
             The file-corruption variant lives on SutHandle::apply_create_stale_loro \
             (org_filename, LoroCorruptionType) — a different model. Wire in Phase 7 once the \
             semantics are reconciled."
        )
    }
}

/// `LoroSut` IS the composed peer-mesh surface: registering it on a `CapMap`
/// hosts the `SutLoro` cap, so a Loro-only composed SUT can drive the peer
/// transitions (`AddPeer`/`PeerEdit`/`SyncWithPeer`/`MergeFromPeer`) — the
/// loro-only fast-config payoff. The `&self` peer methods (PCG-4) make
/// `SutLoro` dyn-compatible, so the one `Arc` backs the cap through the
/// adapter.
impl CapProvider for LoroSut {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutLoro>);
    }
}

/// Build a diagnostic string showing exactly what differs between Loro and
/// reference.
fn build_diagnostic(loro: &[Block], reference: &[Block]) -> String {
    let mut out = String::new();

    let loro_ids: Vec<_> = loro.iter().map(|b| b.id.as_str()).collect();
    let ref_ids: Vec<_> = reference.iter().map(|b| b.id.as_str()).collect();

    let _ = writeln!(out, "Loro ({} blocks): {:?}", loro.len(), loro_ids);
    let _ = writeln!(out, "Ref  ({} blocks): {:?}", reference.len(), ref_ids);

    // IDs only in one side
    let only_loro: Vec<_> = loro_ids.iter().filter(|id| !ref_ids.contains(id)).collect();
    let only_ref: Vec<_> = ref_ids.iter().filter(|id| !loro_ids.contains(id)).collect();
    if !only_loro.is_empty() {
        let _ = writeln!(out, "Only in Loro: {:?}", only_loro);
    }
    if !only_ref.is_empty() {
        let _ = writeln!(out, "Only in Ref:  {:?}", only_ref);
    }

    // Per-block diffs for shared IDs
    for ref_block in reference {
        if let Some(loro_block) = loro.iter().find(|b| b.id == ref_block.id)
            && loro_block != ref_block
        {
            let _ = writeln!(out, "DIFF {}:", ref_block.id);
            if loro_block.content != ref_block.content {
                let _ = writeln!(
                    out,
                    "  content: {:?} vs {:?}",
                    loro_block.content, ref_block.content
                );
            }
            if loro_block.parent_id != ref_block.parent_id {
                let _ = writeln!(
                    out,
                    "  parent_id: {} vs {}",
                    loro_block.parent_id, ref_block.parent_id
                );
            }
            if loro_block.content_type != ref_block.content_type {
                let _ = writeln!(
                    out,
                    "  content_type: {:?} vs {:?}",
                    loro_block.content_type, ref_block.content_type
                );
            }
            if loro_block.properties != ref_block.properties {
                let _ = writeln!(
                    out,
                    "  properties: {:?} vs {:?}",
                    loro_block.properties, ref_block.properties
                );
            }
        }
    }

    out
}
