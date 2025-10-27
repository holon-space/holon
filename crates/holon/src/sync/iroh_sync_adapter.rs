//! Iroh P2P sync for shared LoroTree collaboration.
//!
//! Protocol (incremental, version-vector-based) and explicit close handshake:
//! 1. Initiator sends its VersionVector (VV)
//! 2. Acceptor receives VV, computes delta, sends delta + its own VV
//! 3. Initiator applies delta, computes its delta using peer's VV, sends it
//! 4. Both sides call `send.finish()` after their last framed write and
//!    drain the receive stream until `Ok(None)` — observing the peer's
//!    EOF proves the peer consumed everything we wrote (in this
//!    protocol, both sides call `send.finish()` only AFTER processing
//!    the peer's last framed message, so an EOF from them implies they
//!    already have our payload).
//! 5. The initiator then calls `conn.close(0, "sync complete")` — a
//!    graceful QUIC CONNECTION_CLOSE frame. The acceptor is awaiting
//!    `conn.closed().await` and returns the moment it arrives. No
//!    timing sleep, no dangling connection, no race between two peers
//!    dropping `Connection` at different moments.
//!
//! Each shared tree is synced on its own ALPN channel: `{prefix}/{shared_tree_id}`.
//!
//! ## Iroh 0.96 gotchas
//!
//! - **Both endpoints must register ALPNs** via `Endpoint::builder().alpns(...)`.
//!   Without this, the QUIC handshake fails ("peer doesn't support any known protocol").
//! - **Close is initiator-driven.** `sync_doc_initiate` returns the
//!   still-open `Connection` after issuing `conn.close(...)`. The caller
//!   can drop the returned handle immediately — the QUIC CONNECTION_CLOSE
//!   frame has already been dispatched, and the acceptor's
//!   `conn.closed().await` will resolve with `ApplicationClosed` rather
//!   than "connection lost".
//! - **RelayMode::Disabled** for local/test use to avoid relay interference.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod adapter {
    use crate::sync::loro_document::LoroDocument;
    use crate::sync::shared_tree::SharedTreeStore;
    use anyhow::{Context, Result};
    use iroh::{Endpoint, EndpointAddr};
    use loro::{ExportMode, LoroDoc};
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;
    use tokio::time::sleep;
    use tracing::{debug, info};

    const MAX_MSG_SIZE: usize = 10 * 1024 * 1024;

    async fn write_framed(stream: &mut iroh::endpoint::SendStream, data: &[u8]) -> Result<()> {
        let len = (data.len() as u32).to_be_bytes();
        stream.write_all(&len).await?;
        stream.write_all(data).await?;
        Ok(())
    }

    async fn read_framed(stream: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .context("Failed to read frame length")?;
        let len = u32::from_be_bytes(len_buf) as usize;
        assert!(len <= MAX_MSG_SIZE, "Message too large: {len} bytes");
        let mut data = vec![0u8; len];
        stream
            .read_exact(&mut data)
            .await
            .context("Failed to read frame body")?;
        Ok(data)
    }

    /// Drain the receive stream until the peer's EOF. `Ok(None)` from
    /// `RecvStream::read` proves the peer called `send.finish()` and
    /// every byte they wrote has landed in our receive buffer — no
    /// further bytes will ever arrive.
    ///
    /// Errors out if the peer sends unexpected trailing bytes (indicates
    /// a protocol bug on the other side).
    async fn drain_until_eof(stream: &mut iroh::endpoint::RecvStream) -> Result<()> {
        let mut buf = [0u8; 64];
        loop {
            match stream.read(&mut buf).await {
                Ok(None) => return Ok(()),
                Ok(Some(n)) => {
                    anyhow::bail!("unexpected {n} trailing byte(s) after protocol completed");
                }
                Err(e) => return Err(anyhow::anyhow!("drain read failed: {e}")),
            }
        }
    }

    /// Make an ALPN identifier from a prefix and doc/tree ID.
    pub fn make_alpn(prefix: &str, id: &str) -> Vec<u8> {
        format!("{}/{}", prefix, id).into_bytes()
    }

    /// Create an Iroh endpoint that can accept connections for the given ALPNs.
    /// Uses a fresh ephemeral secret key — the endpoint identity is
    /// NOT stable across calls.
    pub async fn create_endpoint(alpns: Vec<Vec<u8>>) -> Result<Endpoint> {
        let builder = Endpoint::builder().relay_mode(iroh::RelayMode::Disabled);
        let ep = if alpns.is_empty() {
            builder.bind().await?
        } else {
            builder.alpns(alpns).bind().await?
        };
        Ok(ep)
    }

    /// Create an Iroh endpoint bound to a caller-provided secret key.
    /// Reusing the same key across restarts keeps the iroh endpoint
    /// identity stable — peers can dedupe by id and update the cached
    /// socket addrs without treating the restarted peer as a stranger.
    pub async fn create_endpoint_with_key(
        alpns: Vec<Vec<u8>>,
        secret_key: iroh::SecretKey,
    ) -> Result<Endpoint> {
        let builder = Endpoint::builder()
            .relay_mode(iroh::RelayMode::Disabled)
            .secret_key(secret_key);
        let ep = if alpns.is_empty() {
            builder.bind().await?
        } else {
            builder.alpns(alpns).bind().await?
        };
        Ok(ep)
    }

    // -- Incremental sync protocol --

    /// Initiator side: connect to a peer and sync a LoroDoc.
    /// Returns the Connection so the caller can keep it alive until both sides are done.
    pub async fn sync_doc_initiate(
        endpoint: &Endpoint,
        doc: &LoroDoc,
        alpn: &[u8],
        peer_addr: EndpointAddr,
    ) -> Result<iroh::endpoint::Connection> {
        debug!("[init] connecting...");
        let conn = endpoint
            .connect(peer_addr, alpn)
            .await
            .context("Failed to connect to peer")?;
        debug!("[init] connected, opening bi...");
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to open bi stream: {e}"))?;
        debug!("[init] bi open, sending VV...");

        let our_vv = doc.oplog_vv();
        write_framed(&mut send, &our_vv.encode()).await?;
        debug!("[init] VV sent, reading peer delta...");

        let peer_delta = read_framed(&mut recv)
            .await
            .context("[init] Failed to read peer delta")?;
        debug!(
            "[init] got delta ({} bytes), reading peer VV...",
            peer_delta.len()
        );
        let peer_vv_bytes = read_framed(&mut recv)
            .await
            .context("[init] Failed to read peer VV")?;
        debug!(
            "[init] got VV ({} bytes), importing delta...",
            peer_vv_bytes.len()
        );

        if !peer_delta.is_empty() {
            doc.import(&peer_delta)
                .context("[init] Failed to import peer delta")?;
        }

        let peer_vv = loro::VersionVector::decode(&peer_vv_bytes)?;
        let our_delta = doc.export(ExportMode::updates(&peer_vv))?;
        debug!("[init] sending our delta ({} bytes)...", our_delta.len());
        write_framed(&mut send, &our_delta).await?;
        send.finish()?;
        debug!("[init] send finished, draining recv until peer EOF...");
        // Pull the acceptor's stream to EOF. In the protocol above,
        // the acceptor only calls `send.finish()` AFTER it has imported
        // our delta, so observing `Ok(None)` here proves the acceptor
        // has consumed everything we wrote. Reading past the final
        // framed message also delivers the acceptor's FIN to QUIC,
        // which ACKs `send_accept.stopped()` on the acceptor side
        // (releasing the acceptor from its own drain loop).
        drain_until_eof(&mut recv)
            .await
            .context("[init] drain recv stream")?;
        // Close the connection explicitly with a graceful code so the
        // acceptor's `conn.closed().await` returns promptly with
        // `ApplicationClosed` instead of having to observe a dropped
        // connection as an error. This is the handshake that makes
        // the acceptor side of the protocol sleep-free.
        conn.close(0u32.into(), b"sync complete");
        debug!("[init] done");

        Ok(conn)
    }

    /// Acceptor side: handle an incoming sync connection for a LoroDoc.
    pub async fn sync_doc_accept(endpoint: &Endpoint, doc: &LoroDoc) -> Result<()> {
        debug!("[accept] waiting for incoming...");
        let incoming = endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("No incoming connection"))?;

        debug!("[accept] got incoming, accepting...");
        let conn = incoming
            .await
            .map_err(|e| anyhow::anyhow!("Failed to accept connection: {e}"))?;

        sync_doc_handle_connection(conn, doc).await
    }

    /// Extract an addressable `EndpointAddr` for the peer of an active
    /// `Connection`. Combines the peer's cryptographic identity
    /// (`remote_id`) with the current network paths (`paths().get()`).
    /// The resulting addr is dialable from this process — callers
    /// persist it so that a second launch can reconnect without needing
    /// a fresh ticket.
    pub fn connection_remote_addr(conn: &iroh::endpoint::Connection) -> EndpointAddr {
        use iroh::Watcher as _;
        let id = conn.remote_id();
        let mut watcher = conn.paths();
        let paths = watcher.get();
        let transport_addrs = paths.iter().map(|p| p.remote_addr().clone());
        EndpointAddr::from_parts(id, transport_addrs)
    }

    /// Run the VV-based sync handshake against an already-accepted connection.
    /// Factored out of `sync_doc_accept` so the persistent advertiser loop can
    /// reuse it.
    pub async fn sync_doc_handle_connection(
        conn: iroh::endpoint::Connection,
        doc: &LoroDoc,
    ) -> Result<()> {
        debug!("[accept] connected, accepting bi...");
        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to accept bi stream: {e}"))?;
        debug!("[accept] bi stream open");

        // Receive peer's VV
        debug!("[accept] reading peer VV...");
        let peer_vv_bytes = read_framed(&mut recv)
            .await
            .context("[accept] Failed to read peer VV")?;
        debug!("[accept] got peer VV ({} bytes)", peer_vv_bytes.len());
        let peer_vv = loro::VersionVector::decode(&peer_vv_bytes)
            .context("[accept] Failed to decode peer VV")?;

        // Compute delta + send our VV
        let our_delta = doc
            .export(ExportMode::updates(&peer_vv))
            .context("[accept] Failed to export delta")?;
        let our_vv = doc.oplog_vv();
        debug!("[accept] sending delta ({} bytes) + VV", our_delta.len());
        write_framed(&mut send, &our_delta).await?;
        write_framed(&mut send, &our_vv.encode()).await?;

        // Receive peer's delta
        debug!("[accept] reading peer delta...");
        let peer_delta = read_framed(&mut recv)
            .await
            .context("[accept] Failed to read peer delta")?;
        debug!("[accept] got peer delta ({} bytes)", peer_delta.len());
        if !peer_delta.is_empty() {
            doc.import(&peer_delta)
                .context("Failed to import peer delta")?;
            debug!("Applied {} bytes from peer", peer_delta.len());
        }

        send.finish()?;
        debug!("[accept] send finished, draining recv until peer EOF...");
        // Pull the initiator's stream to EOF. The initiator only calls
        // `send.finish()` AFTER writing its delta, so `Ok(None)` here
        // proves our earlier `read_framed(peer_delta)` got everything.
        // It also delivers the initiator's FIN to our QUIC state.
        drain_until_eof(&mut recv)
            .await
            .context("[accept] drain recv stream")?;
        // Wait for the initiator to explicitly close the connection
        // (see `sync_doc_initiate`). `conn.closed()` resolves the
        // moment the peer's close packet lands — with no timing guess.
        // We drop conn immediately after, so there's no lingering
        // reference to trip up the accept loop.
        let close_reason = conn.closed().await;
        debug!("[accept] connection closed: {close_reason}");

        info!(
            "Sync accepted: sent {} bytes, received {} bytes",
            our_delta.len(),
            peer_delta.len()
        );
        Ok(())
    }

    // -- Legacy LoroDocument adapter (kept for backwards compat) --

    pub struct IrohSyncAdapter {
        endpoint: Arc<Endpoint>,
        alpn_prefix: String,
    }

    impl IrohSyncAdapter {
        pub async fn new(alpn_prefix: &str) -> Result<Self> {
            let endpoint = Endpoint::builder().bind().await?;
            Ok(Self {
                endpoint: Arc::new(endpoint),
                alpn_prefix: alpn_prefix.to_string(),
            })
        }

        pub async fn new_with_alpns(alpn_prefix: &str, accepted_ids: &[&str]) -> Result<Self> {
            let alpns: Vec<Vec<u8>> = accepted_ids
                .iter()
                .map(|id| format!("{}/{}", alpn_prefix, id).into_bytes())
                .collect();
            let endpoint = Endpoint::builder().alpns(alpns).bind().await?;
            Ok(Self {
                endpoint: Arc::new(endpoint),
                alpn_prefix: alpn_prefix.to_string(),
            })
        }

        fn alpn(&self, doc_id: &str) -> Vec<u8> {
            format!("{}/{}", self.alpn_prefix, doc_id).into_bytes()
        }

        pub fn set_peer_id_from_node(&self, doc: &mut LoroDocument) -> Result<()> {
            let id = self.endpoint.id();
            let id_bytes = id.as_bytes();
            let peer_id = u64::from_le_bytes(id_bytes[0..8].try_into()?);
            doc.set_peer_id(peer_id)?;
            Ok(())
        }

        pub fn addr(&self) -> EndpointAddr {
            self.endpoint.addr()
        }

        pub fn endpoint(&self) -> &Arc<Endpoint> {
            &self.endpoint
        }

        /// Legacy full-snapshot sync for LoroDocument.
        pub async fn sync_with_peer(
            &self,
            doc: &LoroDocument,
            peer_addr: EndpointAddr,
        ) -> Result<()> {
            let doc_id = doc.doc_id();
            let alpn = self.alpn(doc_id);
            let conn = self.endpoint.connect(peer_addr, &alpn).await?;

            let snapshot = doc.export_snapshot().await?;
            let mut send_stream = conn.open_uni().await?;
            send_stream.write_all(&snapshot).await?;
            send_stream.finish()?;

            let mut recv_stream = conn.accept_uni().await?;
            let buffer = recv_stream.read_to_end(MAX_MSG_SIZE).await?;
            if !buffer.is_empty() {
                doc.apply_update(&buffer).await?;
            }

            Ok(())
        }

        /// Legacy full-snapshot accept for LoroDocument.
        pub async fn accept_sync(&self, doc: &LoroDocument) -> Result<()> {
            let doc_id = doc.doc_id();
            let incoming = self
                .endpoint
                .accept()
                .await
                .ok_or_else(|| anyhow::anyhow!("No incoming connection"))?;

            let conn = incoming.await?;
            let alpn = conn.alpn();
            let expected = self.alpn(doc_id);
            if alpn != expected.as_slice() {
                anyhow::bail!(
                    "Wrong document! Expected '{}', got ALPN: {:?}",
                    doc_id,
                    String::from_utf8_lossy(alpn)
                );
            }

            let mut recv_stream = conn.accept_uni().await?;
            let buffer = recv_stream.read_to_end(MAX_MSG_SIZE).await?;
            if !buffer.is_empty() {
                doc.apply_update(&buffer).await?;
            }

            let snapshot = doc.export_snapshot().await?;
            let mut send_stream = conn.open_uni().await?;
            send_stream.write_all(&snapshot).await?;
            send_stream.finish()?;
            sleep(Duration::from_millis(100)).await;
            Ok(())
        }
    }

    // -- SyncBackend trait --

    /// Abstraction over how two LoroDoc instances sync.
    /// Allows the same PBT to run against direct Loro sync or real Iroh transport.
    pub trait SyncBackend: Send + Sync {
        /// Bidirectional sync between two LoroDoc instances.
        fn sync_pair(&self, doc_a: &LoroDoc, doc_b: &LoroDoc) -> Result<()>;
    }

    /// Direct Loro sync using export/import — no network, deterministic, fast.
    pub struct DirectSync;

    impl SyncBackend for DirectSync {
        fn sync_pair(&self, a: &LoroDoc, b: &LoroDoc) -> Result<()> {
            let b_vv = b.oplog_vv();
            let a_delta = a.export(ExportMode::updates(&b_vv))?;
            if !a_delta.is_empty() {
                b.import(&a_delta)?;
            }
            let a_vv = a.oplog_vv();
            let b_delta = b.export(ExportMode::updates(&a_vv))?;
            if !b_delta.is_empty() {
                a.import(&b_delta)?;
            }
            Ok(())
        }
    }

    /// Iroh-backed sync — creates ephemeral endpoints per sync_pair call.
    /// Uses the real QUIC transport with VV-based incremental protocol.
    pub struct IrohSync {
        rt: tokio::runtime::Runtime,
    }

    impl IrohSync {
        pub fn new() -> Result<Self> {
            let rt = tokio::runtime::Runtime::new()?;
            Ok(Self { rt })
        }
    }

    impl SyncBackend for IrohSync {
        fn sync_pair(&self, doc_a: &LoroDoc, doc_b: &LoroDoc) -> Result<()> {
            self.rt.block_on(async {
                let label = format!("pbt-{}", rand::random::<u32>());
                let alpn = make_alpn("loro-sync", &label);
                let ep_a = create_endpoint(vec![alpn.clone()]).await?;
                let ep_b = create_endpoint(vec![alpn.clone()]).await?;
                sleep(Duration::from_millis(200)).await;
                let addr_b = ep_b.addr();

                let doc_b_clone = doc_b.clone();
                let handle =
                    tokio::spawn(async move { sync_doc_accept(&ep_b, &doc_b_clone).await });

                sleep(Duration::from_millis(300)).await;
                let _conn = sync_doc_initiate(&ep_a, doc_a, &alpn, addr_b).await?;
                // _conn + ep_a kept alive until acceptor finishes
                handle.await??;
                Ok(())
            })
        }
    }

    // -- SharedTreeSyncManager --

    /// Manages multiple shared tree LoroDocs and their sync state.
    /// Implements SharedTreeStore so LoroBackend can traverse mount nodes.
    pub struct SharedTreeSyncManager {
        trees: Arc<RwLock<HashMap<String, Arc<LoroDoc>>>>,
    }

    impl SharedTreeSyncManager {
        pub fn new() -> Self {
            Self {
                trees: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        pub fn register(&self, shared_tree_id: String, doc: LoroDoc) {
            self.trees
                .write()
                .unwrap()
                .insert(shared_tree_id, Arc::new(doc));
        }

        pub fn register_arc(&self, shared_tree_id: String, doc: Arc<LoroDoc>) {
            self.trees.write().unwrap().insert(shared_tree_id, doc);
        }

        pub fn get_doc(&self, shared_tree_id: &str) -> Option<Arc<LoroDoc>> {
            self.trees.read().unwrap().get(shared_tree_id).cloned()
        }

        pub fn remove(&self, shared_tree_id: &str) -> Option<Arc<LoroDoc>> {
            self.trees.write().unwrap().remove(shared_tree_id)
        }
    }

    impl SharedTreeStore for SharedTreeSyncManager {
        fn get_shared_doc(&self, shared_tree_id: &str) -> Option<Arc<LoroDoc>> {
            self.get_doc(shared_tree_id)
        }

        fn shared_tree_ids(&self) -> Vec<String> {
            self.trees.read().unwrap().keys().cloned().collect()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::api::loro_backend::TREE_NAME;
        use crate::sync::shared_tree::{HistoryRetention, extract_subtree};
        use loro::LoroText;

        fn set_text(tree: &loro::LoroTree, node: loro::TreeID, content: &str) {
            let meta = tree.get_meta(node).unwrap();
            let text: LoroText = meta
                .insert_container("content_raw", LoroText::new())
                .unwrap();
            text.insert(0, content).unwrap();
        }

        fn read_text(tree: &loro::LoroTree, node: loro::TreeID) -> String {
            let meta = tree.get_meta(node).unwrap();
            match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t.to_string(),
                _ => String::new(),
            }
        }

        fn edit_text(tree: &loro::LoroTree, node: loro::TreeID, append: &str) {
            let meta = tree.get_meta(node).unwrap();
            match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => {
                    let len = t.len_unicode();
                    t.insert(len, append).unwrap();
                }
                _ => panic!("no content_raw on node"),
            }
        }

        fn build_and_extract() -> (LoroDoc, LoroDoc, loro::TreeID, loro::TreeID) {
            let doc = LoroDoc::new();
            doc.set_peer_id(1).unwrap();
            let tree = doc.get_tree(TREE_NAME);
            tree.enable_fractional_index(0);

            let root = tree.create(None).unwrap();
            let meta = tree.get_meta(root).unwrap();
            meta.insert("name", "test_doc").unwrap();

            let kept = tree.create(root).unwrap();
            set_text(&tree, kept, "Kept heading");

            let shared_root = tree.create(root).unwrap();
            set_text(&tree, shared_root, "Shared heading");

            let block_b = tree.create(shared_root).unwrap();
            set_text(&tree, block_b, "Block B");

            let block_c = tree.create(shared_root).unwrap();
            set_text(&tree, block_c, "Block C");

            doc.commit();
            let extracted = extract_subtree(&doc, shared_root, HistoryRetention::Full).unwrap();
            (doc, extracted.shared_doc, shared_root, block_b)
        }

        /// Helper: sync two LoroDoc instances via Iroh with the incremental protocol.
        async fn sync_pair(doc1: &LoroDoc, doc2: &LoroDoc, tree_id: &str) -> Result<()> {
            let alpn = make_alpn("loro-sync", tree_id);
            let ep1 = create_endpoint(vec![alpn.clone()]).await?;
            let ep2 = create_endpoint(vec![alpn.clone()]).await?;
            // Wait for endpoints to be ready (Iroh needs time for discovery)
            sleep(Duration::from_millis(500)).await;
            let addr2 = ep2.addr();

            let d2 = doc2.clone();
            let handle = tokio::spawn(async move { sync_doc_accept(&ep2, &d2).await });

            // Wait for acceptor to be ready
            sleep(Duration::from_millis(500)).await;
            let _conn = sync_doc_initiate(&ep1, doc1, &alpn, addr2)
                .await
                .context("sync_doc_initiate failed")?;
            // _conn + ep1 kept alive while acceptor finishes reading
            handle.await?.context("sync_doc_accept failed")?;
            Ok(())
        }

        #[tokio::test]
        async fn test_create_adapter() -> Result<()> {
            let adapter = IrohSyncAdapter::new("loro-sync").await?;
            let _addr = adapter.addr();
            Ok(())
        }

        #[tokio::test]
        async fn test_set_peer_id_from_node() -> Result<()> {
            let adapter = IrohSyncAdapter::new("loro-sync").await?;
            let mut doc = LoroDocument::new("test".to_string())?;
            let original_peer_id = doc.peer_id();
            adapter.set_peer_id_from_node(&mut doc)?;
            assert_ne!(doc.peer_id(), original_peer_id);
            Ok(())
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn incremental_sync_shared_tree() -> Result<()> {
            let (_source, shared_doc, _shared_root, block_b) = build_and_extract();

            let peer2_doc = LoroDoc::new();
            peer2_doc.set_peer_id(2).unwrap();
            peer2_doc.import(&shared_doc.export(ExportMode::Snapshot)?)?;

            // P1 makes an edit
            edit_text(&shared_doc.get_tree(TREE_NAME), block_b, " - edited by P1");
            shared_doc.commit();

            sync_pair(&shared_doc, &peer2_doc, "collab-1").await?;

            assert_eq!(
                read_text(&peer2_doc.get_tree(TREE_NAME), block_b),
                "Block B - edited by P1"
            );
            Ok(())
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn bidirectional_incremental_sync() -> Result<()> {
            let (_source, shared_doc, _shared_root, block_b) = build_and_extract();

            let peer2_doc = LoroDoc::new();
            peer2_doc.set_peer_id(2).unwrap();
            peer2_doc.import(&shared_doc.export(ExportMode::Snapshot)?)?;

            // Both peers edit concurrently
            edit_text(&shared_doc.get_tree(TREE_NAME), block_b, " [P1]");
            shared_doc.commit();
            edit_text(&peer2_doc.get_tree(TREE_NAME), block_b, " [P2]");
            peer2_doc.commit();

            sync_pair(&shared_doc, &peer2_doc, "collab-bi").await?;

            let text1 = read_text(&shared_doc.get_tree(TREE_NAME), block_b);
            let text2 = read_text(&peer2_doc.get_tree(TREE_NAME), block_b);

            assert_eq!(text1, text2, "Both peers should converge");
            assert!(text1.contains("[P1]"), "Should contain P1's edit: {text1}");
            assert!(text1.contains("[P2]"), "Should contain P2's edit: {text1}");
            Ok(())
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn sync_structural_move() -> Result<()> {
            let (_source, shared_doc, shared_root, block_b) = build_and_extract();

            let peer2_doc = LoroDoc::new();
            peer2_doc.set_peer_id(2).unwrap();
            peer2_doc.import(&shared_doc.export(ExportMode::Snapshot)?)?;

            // P1 creates a new parent and moves block_b there
            let tree1 = shared_doc.get_tree(TREE_NAME);
            let new_parent = tree1.create(shared_root).unwrap();
            set_text(&tree1, new_parent, "New parent");
            tree1.mov(block_b, new_parent).unwrap();
            shared_doc.commit();

            sync_pair(&shared_doc, &peer2_doc, "collab-move").await?;

            let tree2 = peer2_doc.get_tree(TREE_NAME);
            assert_eq!(
                tree2.parent(block_b),
                Some(loro::TreeParentId::Node(new_parent)),
                "Block B should have been moved to new_parent on peer 2"
            );
            Ok(())
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn shared_tree_sync_manager_integration() -> Result<()> {
            let (_source, shared_doc, _shared_root, block_b) = build_and_extract();

            let manager = SharedTreeSyncManager::new();
            let stid = "collab-mgr-test".to_string();

            let peer2_doc = LoroDoc::new();
            peer2_doc.set_peer_id(2).unwrap();
            peer2_doc.import(&shared_doc.export(ExportMode::Snapshot)?)?;

            manager.register(stid.clone(), shared_doc);

            let doc_ref = manager.get_doc(&stid).unwrap();
            edit_text(&doc_ref.get_tree(TREE_NAME), block_b, " - via manager");
            doc_ref.commit();

            sync_pair(&doc_ref, &peer2_doc, &stid).await?;

            assert_eq!(
                read_text(&peer2_doc.get_tree(TREE_NAME), block_b),
                "Block B - via manager"
            );
            assert!(manager.get_shared_doc(&stid).is_some());
            assert_eq!(manager.shared_tree_ids().len(), 1);
            Ok(())
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn multiple_shared_trees_independent() -> Result<()> {
            let (_, shared1, _, block_b1) = build_and_extract();
            let (_, shared2, _, block_b2) = build_and_extract();

            let p2_doc1 = LoroDoc::new();
            p2_doc1.set_peer_id(2).unwrap();
            p2_doc1.import(&shared1.export(ExportMode::Snapshot)?)?;

            let p2_doc2 = LoroDoc::new();
            p2_doc2.set_peer_id(3).unwrap();
            p2_doc2.import(&shared2.export(ExportMode::Snapshot)?)?;

            // Edit only shared1
            edit_text(&shared1.get_tree(TREE_NAME), block_b1, " - only in tree1");
            shared1.commit();

            // Sync shared1 only
            sync_pair(&shared1, &p2_doc1, "tree1").await?;

            assert_eq!(
                read_text(&p2_doc1.get_tree(TREE_NAME), block_b1),
                "Block B - only in tree1"
            );
            assert_eq!(
                read_text(&p2_doc2.get_tree(TREE_NAME), block_b2),
                "Block B",
                "Shared tree 2 should be unaffected"
            );
            Ok(())
        }
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::{
    DirectSync, IrohSync, IrohSyncAdapter, SharedTreeSyncManager, SyncBackend,
    connection_remote_addr, create_endpoint, create_endpoint_with_key, make_alpn, sync_doc_accept,
    sync_doc_handle_connection, sync_doc_initiate,
};
