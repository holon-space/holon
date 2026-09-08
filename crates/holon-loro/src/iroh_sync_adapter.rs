//! Iroh P2P sync for shared LoroTree collaboration.
//!
//! Protocol (incremental, version-vector-based) and explicit close handshake:
//! 1. Initiator sends its VersionVector (VV)
//! 2. Acceptor receives VV, computes delta, sends delta + its own VV
//! 3. Initiator applies delta, computes its delta using peer's VV, sends it
//! 4. Both sides call `send.finish()` after their last framed write and drain
//!    the receive stream until `Ok(None)` — observing the peer's EOF proves the
//!    peer consumed everything we wrote (in this protocol, both sides call
//!    `send.finish()` only AFTER processing the peer's last framed message, so
//!    an EOF from them implies they already have our payload).
//! 5. The initiator then calls `conn.close(0, "sync complete")` — a graceful
//!    QUIC CONNECTION_CLOSE frame. The acceptor is awaiting
//!    `conn.closed().await` and returns the moment it arrives. No timing sleep,
//!    no dangling connection, no race between two peers dropping `Connection`
//!    at different moments.
//!
//! Each shared tree is synced on its own ALPN channel:
//! `{prefix}/{shared_tree_id}`.
//!
//! ## Iroh 0.96 gotchas
//!
//! - **Both endpoints must register ALPNs** via
//!   `Endpoint::builder().alpns(...)`. Without this, the QUIC handshake fails
//!   ("peer doesn't support any known protocol").
//! - **Close is initiator-driven.** `sync_doc_initiate` returns the still-open
//!   `Connection` after issuing `conn.close(...)`. The caller can drop the
//!   returned handle immediately — the QUIC CONNECTION_CLOSE frame has already
//!   been dispatched, and the acceptor's `conn.closed().await` will resolve
//!   with `ApplicationClosed` rather than "connection lost".
//! - **RelayMode::Disabled** for local/test use to avoid relay interference.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod adapter {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::RwLock;
    use std::time::Duration;

    use anyhow::Context;
    use anyhow::Result;
    use holon_api::sharing::Capabilities;
    use iroh::Endpoint;
    use iroh::EndpointAddr;
    use loro::ExportMode;
    use loro::LoroDoc;
    #[cfg(any(test, feature = "test-helpers"))]
    use tokio::time::sleep;
    use tokio::time::timeout;
    use tracing::debug;
    use tracing::info;
    use tracing::warn;

    use crate::peer_import::AdmittedPeer;
    use crate::peer_import::PeerReadAccess;
    use crate::peer_import::authorize_peer_read;
    use crate::peer_import::import_peer_delta;
    use crate::share_enrollment::peer_fingerprint;
    use crate::shared_tree::SharedTreeStore;

    const MAX_MSG_SIZE: usize = 10 * 1024 * 1024;

    /// Upper bound on any single acceptor-side network await. The initiator is
    /// already bounded by `CONNECT_TIMEOUT` at its call sites; this stops a
    /// stalled/malicious peer from pinning an accept task forever (slowloris).
    const ACCEPT_IO_TIMEOUT: Duration = Duration::from_secs(30);

    /// Export the updates the peer is missing (relative to `peer_vv`).
    ///
    /// A **shallow** doc (a state-only share exported via `shallow_snapshot`,
    /// or any doc whose history has been compacted) cannot hand a peer
    /// incremental updates that predate its shallow start: those ops are
    /// gone. If we send `ExportMode::updates` anyway, the peer buffers our
    /// delta as un-appliable "pending" ops (their dependencies are missing)
    /// and its tree stays empty — a fresh accepter of a state-only share
    /// would end up with zero roots. So when the peer does not already
    /// include our shallow base, send a self-contained snapshot instead.
    /// This is the path a fresh accepter (empty VV) always takes for a
    /// `retention="none"` share; once bootstrapped, the peer includes the
    /// base and subsequent syncs use incremental updates.
    ///
    /// The pre-existing `Err` recovery path (below) still covers any other case
    /// where `updates` refuses. `label` tags the log line.
    fn export_delta_or_full_snapshot(
        doc: &LoroDoc,
        peer_vv: &loro::VersionVector,
        label: &str,
    ) -> Result<Vec<u8>> {
        if doc.is_shallow() {
            let shallow_base = doc.shallow_since_vv().to_vv();
            if !peer_vv.includes_vv(&shallow_base) {
                // ALLOW(fallback): peer is missing our shallow base; a snapshot
                // is the only self-contained payload that converges them.
                return doc.export(ExportMode::Snapshot).with_context(|| {
                    format!("[{label}] snapshot export for peer below shallow base")
                });
            }
        }
        match doc.export(ExportMode::updates(peer_vv)) {
            Ok(delta) => Ok(delta),
            Err(e) => {
                warn!(
                    "[{label}] delta export failed ({e}) — peer is behind compacted history; \
                     sending full snapshot instead"
                );
                // ALLOW(fallback): disclosed via the warn! above — convergence
                // for peers behind compacted history is the designed behavior.
                doc.export(ExportMode::Snapshot)
                    .with_context(|| format!("[{label}] full-snapshot export for stale peer"))
            }
        }
    }

    async fn write_framed(stream: &mut iroh::endpoint::SendStream, data: &[u8]) -> Result<()> {
        if data.len() > MAX_MSG_SIZE {
            anyhow::bail!(
                "refusing to send oversize frame: {} bytes (max {MAX_MSG_SIZE})",
                data.len()
            );
        }
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
        if len > MAX_MSG_SIZE {
            anyhow::bail!("peer sent oversize frame: {len} bytes (max {MAX_MSG_SIZE})");
        }
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
        match stream.read(&mut buf).await {
            Ok(None) => Ok(()),
            Ok(Some(n)) => {
                anyhow::bail!("unexpected {n} trailing byte(s) after protocol completed")
            }
            Err(e) => Err(anyhow::anyhow!("drain read failed: {e}")),
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
    ///
    /// `preferred_port` requests the same UDP port across restarts.
    /// With relay and discovery disabled, peers can only reconnect via
    /// the socket addrs they persisted — a stable port keeps those
    /// addrs valid after a restart (a stable key alone is not enough).
    /// If the port is taken, falls back to an ephemeral port with a
    /// warning: peers then can't reach us until we dial them first.
    pub async fn create_endpoint_with_key(
        alpns: Vec<Vec<u8>>,
        secret_key: iroh::SecretKey,
        preferred_port: Option<u16>,
    ) -> Result<Endpoint> {
        let build = |port: Option<u16>| -> Result<iroh::endpoint::Builder> {
            let mut builder = Endpoint::builder()
                .relay_mode(iroh::RelayMode::Disabled)
                .secret_key(secret_key.clone());
            if !alpns.is_empty() {
                builder = builder.alpns(alpns.clone());
            }
            if let Some(p) = port {
                builder = builder
                    .bind_addr(std::net::SocketAddr::from((
                        std::net::Ipv4Addr::UNSPECIFIED,
                        p,
                    )))
                    .context("bind_addr for preferred port")?;
            }
            Ok(builder)
        };
        match preferred_port {
            Some(p) => match build(Some(p))?.bind().await {
                Ok(ep) => Ok(ep),
                Err(e) => {
                    tracing::warn!(
                        port = p,
                        error = %e,
                        "preferred port bind failed; using ephemeral port — \
                         peers' persisted addrs for this endpoint are stale \
                         until we dial them"
                    );
                    Ok(build(None)?.bind().await?)
                }
            },
            None => Ok(build(None)?.bind().await?),
        }
    }

    // -- Incremental sync protocol --

    /// Initiator side: connect to a peer and sync a LoroDoc.
    /// Returns the Connection so the caller can keep it alive until both sides
    /// are done.
    ///
    /// `grant` is what THIS device allows the peer it dials to do here — there
    /// is no roster on the initiating side, so the caller states it and must be
    /// able to say why. It gates both directions of the round
    /// ([`crate::peer_import`]).
    ///
    /// Dials WITHOUT enrolling, so it can only ever complete a round against an
    /// un-gated acceptor. Production dials go through
    /// [`sync_doc_initiate_enrolled`]; this one is compiled out of a production
    /// build and kept for the transport harness and for the tests that dial a
    /// gated share AS a stranger.
    #[cfg(any(test, feature = "test-helpers"))]
    pub(crate) async fn sync_doc_initiate(
        endpoint: &Endpoint,
        doc: &Arc<LoroDoc>,
        alpn: &[u8],
        peer_addr: EndpointAddr,
        container: &str,
        grant: Capabilities,
    ) -> Result<iroh::endpoint::Connection> {
        debug!("[init] connecting...");
        let conn = endpoint
            .connect(peer_addr, alpn)
            .await
            .context("Failed to connect to peer")?;
        // The peer identity comes off the QUIC-authenticated connection, never
        // off the addr we aimed at, so the witness names who actually answered.
        let admitted = AdmittedPeer::dialed(container, peer_fingerprint(&conn), grant);
        sync_on_connection_initiator(conn, doc, &admitted).await
    }

    /// Initiator side WITH enrollment (ADR 0028 H5 acceptor gate): connect,
    /// prove capability possession on a dedicated enrollment stream FIRST, then
    /// run the VV sync. The acceptor's gated `accept_loop` reads the enrollment
    /// stream before the sync stream, so a peer that cannot prove the
    /// capability never reaches the sync primitive — closing the
    /// bearer-`shared_tree_id` forgery hole.
    pub async fn sync_doc_initiate_enrolled(
        endpoint: &Endpoint,
        doc: &Arc<LoroDoc>,
        alpn: &[u8],
        peer_addr: EndpointAddr,
        capability: &crate::share_enrollment::CapabilitySecret,
        shared_tree_id: &str,
        grant: Capabilities,
    ) -> Result<iroh::endpoint::Connection> {
        debug!("[init] connecting (enrolled)...");
        let conn = endpoint
            .connect(peer_addr, alpn)
            .await
            .context("Failed to connect to peer")?;
        // Classify BEFORE adding context: every failure past `connect()` would
        // otherwise share one message, and a timeout would be indistinguishable
        // from the acceptor's decision to refuse. `classify_enrollment_failure`
        // reads the QUIC close code and tags a genuine refusal with the typed
        // `EnrollmentRefused`, which the caller recovers by downcast.
        if let Err(e) =
            crate::share_enrollment::initiator_enroll(&conn, capability, shared_tree_id).await
        {
            return Err(
                crate::share_enrollment::classify_enrollment_failure(&conn, e)
                    .context("[init] enrollment did not complete"),
            );
        }
        let admitted = AdmittedPeer::dialed(shared_tree_id, peer_fingerprint(&conn), grant);
        sync_on_connection_initiator(conn, doc, &admitted).await
    }

    /// Run the initiator side of the VV sync protocol on an already-connected
    /// (and, on the gated path, already-enrolled) connection.
    async fn sync_on_connection_initiator(
        conn: iroh::endpoint::Connection,
        doc: &Arc<LoroDoc>,
        admitted: &AdmittedPeer,
    ) -> Result<iroh::endpoint::Connection> {
        // Our VV and delta go out on this stream, so the peer reads us here.
        authorize_peer_read(admitted).context("[init] peer may not read this container")?;
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
            import_peer_delta(doc, admitted, &peer_delta)
                .context("[init] Failed to import peer delta")?;
        }

        let peer_vv = loro::VersionVector::decode(&peer_vv_bytes)?;
        let our_delta = export_delta_or_full_snapshot(doc, &peer_vv, "init")?;
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

    /// Acceptor side: handle ONE incoming sync connection for a LoroDoc, with
    /// no enrollment — whoever reaches the endpoint is admitted with `grant`.
    ///
    /// The gated production accept loop is
    /// [`crate::iroh_advertiser::IrohAdvertiser::start_share_gated`]. This
    /// entry point is the ONE un-gated acceptor left in the crate, so it is
    /// compiled out of a production build and is `pub(crate)`: transport
    /// tests and the [`IrohSync`] PBT harness reach it, nothing that ships
    /// does, and no other crate can.
    #[cfg(any(test, feature = "test-helpers"))]
    pub(crate) async fn sync_doc_accept(
        endpoint: &Endpoint,
        doc: &Arc<LoroDoc>,
        container: &str,
        grant: Capabilities,
    ) -> Result<()> {
        debug!("[accept] waiting for incoming...");
        let incoming = endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("No incoming connection"))?;

        debug!("[accept] got incoming, accepting...");
        let conn = incoming
            .await
            .map_err(|e| anyhow::anyhow!("Failed to accept connection: {e}"))?;

        let admitted = AdmittedPeer::ungated(container, peer_fingerprint(&conn), grant);
        let access =
            authorize_peer_read(&admitted).context("[accept] peer may not read this container")?;
        sync_doc_handle_connection(conn, doc, &access).await
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

    /// Bound an acceptor-side await by `ACCEPT_IO_TIMEOUT`, turning a stall
    /// into a loud error instead of a task pinned forever by a slowloris
    /// peer.
    async fn with_accept_timeout<F, T>(what: &str, fut: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        match timeout(ACCEPT_IO_TIMEOUT, fut).await {
            Ok(r) => r,
            Err(_) => anyhow::bail!("[accept] {what} timed out after {ACCEPT_IO_TIMEOUT:?}"),
        }
    }

    /// Run the VV-based sync handshake against an already-accepted connection.
    /// Factored out of `sync_doc_accept` so the persistent advertiser loop can
    /// reuse it.
    ///
    /// Takes the read witness, not the raw admission: exporting our delta is
    /// this peer reading here, so the caller has to have passed
    /// [`authorize_peer_read`] to reach this function at all.
    pub async fn sync_doc_handle_connection(
        conn: iroh::endpoint::Connection,
        doc: &Arc<LoroDoc>,
        access: &PeerReadAccess,
    ) -> Result<()> {
        let admitted = access.admitted();
        debug!("[accept] connected, accepting bi...");
        let (mut send, mut recv) = match timeout(ACCEPT_IO_TIMEOUT, conn.accept_bi()).await {
            Ok(r) => r.map_err(|e| anyhow::anyhow!("Failed to accept bi stream: {e}"))?,
            Err(_) => anyhow::bail!("[accept] accept_bi timed out after {ACCEPT_IO_TIMEOUT:?}"),
        };
        debug!("[accept] bi stream open");

        // Receive peer's VV
        debug!("[accept] reading peer VV...");
        let peer_vv_bytes = with_accept_timeout("read peer VV", read_framed(&mut recv))
            .await
            .context("[accept] Failed to read peer VV")?;
        debug!("[accept] got peer VV ({} bytes)", peer_vv_bytes.len());
        let peer_vv = loro::VersionVector::decode(&peer_vv_bytes)
            .context("[accept] Failed to decode peer VV")?;

        // Compute delta + send our VV
        let our_delta = export_delta_or_full_snapshot(doc, &peer_vv, "accept")?;
        let our_vv = doc.oplog_vv();
        debug!("[accept] sending delta ({} bytes) + VV", our_delta.len());
        // Bound the writes too: a peer that stops reading stalls `write_all`
        // once QUIC flow control fills the send buffer (a write-side slowloris).
        with_accept_timeout("send delta", write_framed(&mut send, &our_delta)).await?;
        with_accept_timeout("send VV", write_framed(&mut send, &our_vv.encode())).await?;

        // Receive peer's delta
        debug!("[accept] reading peer delta...");
        let peer_delta = with_accept_timeout("read peer delta", read_framed(&mut recv))
            .await
            .context("[accept] Failed to read peer delta")?;
        debug!("[accept] got peer delta ({} bytes)", peer_delta.len());
        if !peer_delta.is_empty() {
            import_peer_delta(doc, admitted, &peer_delta)
                .context("[accept] Failed to import peer delta")?;
            debug!("Applied {} bytes from peer", peer_delta.len());
        }

        send.finish()?;
        debug!("[accept] send finished, draining recv until peer EOF...");
        // Pull the initiator's stream to EOF. The initiator only calls
        // `send.finish()` AFTER writing its delta, so `Ok(None)` here
        // proves our earlier `read_framed(peer_delta)` got everything.
        // It also delivers the initiator's FIN to our QUIC state.
        with_accept_timeout("drain recv stream", drain_until_eof(&mut recv))
            .await
            .context("[accept] drain recv stream")?;
        // Wait for the initiator to explicitly close the connection
        // (see `sync_doc_initiate`). `conn.closed()` resolves the
        // moment the peer's close packet lands — with no timing guess.
        // We drop conn immediately after, so there's no lingering
        // reference to trip up the accept loop. A peer that never closes
        // is misbehaving, so a timeout here is an error, not a clean exit.
        let close_reason = match timeout(ACCEPT_IO_TIMEOUT, conn.closed()).await {
            Ok(reason) => reason,
            Err(_) => anyhow::bail!(
                "[accept] peer never closed connection (timed out after {ACCEPT_IO_TIMEOUT:?})"
            ),
        };
        debug!("[accept] connection closed: {close_reason}");

        info!(
            "Sync accepted: sent {} bytes, received {} bytes",
            our_delta.len(),
            peer_delta.len()
        );
        Ok(())
    }

    // -- SyncBackend trait --

    /// Abstraction over how two LoroDoc instances sync.
    /// Allows the same PBT to run against direct Loro sync or real Iroh
    /// transport.
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
    ///
    /// Accepts un-gated (see [`sync_doc_accept`]) on an endpoint it binds
    /// itself under a random per-call label, so it is compiled out of a
    /// production build: a PBT harness is the only thing that may accept a
    /// connection with no admission behind it.
    #[cfg(any(test, feature = "test-helpers"))]
    pub struct IrohSync {
        rt: tokio::runtime::Runtime,
    }

    #[cfg(any(test, feature = "test-helpers"))]
    impl IrohSync {
        pub fn new() -> Result<Self> {
            let rt = tokio::runtime::Runtime::new()?;
            Ok(Self { rt })
        }
    }

    #[cfg(any(test, feature = "test-helpers"))]
    impl SyncBackend for IrohSync {
        fn sync_pair(&self, doc_a: &LoroDoc, doc_b: &LoroDoc) -> Result<()> {
            self.rt.block_on(async {
                let label = format!("pbt-{}", rand::random::<u32>());
                let alpn = make_alpn("loro-sync", &label);
                let ep_a = create_endpoint(vec![alpn.clone()]).await?;
                let ep_b = create_endpoint(vec![alpn.clone()]).await?;
                sleep(Duration::from_millis(200)).await;
                let addr_b = ep_b.addr();

                // A PBT pair is two halves of one convergence property, so each
                // side grants the other a full writer.
                let doc_a = Arc::new(doc_a.clone());
                let doc_b_clone = Arc::new(doc_b.clone());
                let accept_label = label.clone();
                let handle = tokio::spawn(async move {
                    sync_doc_accept(
                        &ep_b,
                        &doc_b_clone,
                        &accept_label,
                        Capabilities::read_write(),
                    )
                    .await
                });

                sleep(Duration::from_millis(300)).await;
                let _conn = sync_doc_initiate(
                    &ep_a,
                    &doc_a,
                    &alpn,
                    addr_b,
                    &label,
                    Capabilities::read_write(),
                )
                .await?;
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

    impl Default for SharedTreeSyncManager {
        fn default() -> Self {
            Self::new()
        }
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
        use loro::LoroText;

        use super::*;
        use crate::loro_backend::TREE_NAME;
        use crate::shared_tree::HistoryRetention;
        use crate::shared_tree::extract_subtree;

        fn set_text(tree: &loro::LoroTree, node: loro::TreeID, content: &str) {
            let meta = tree.get_meta(node).unwrap();
            let text: LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
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

        /// A fresh peer replica, already an `Arc` because that is what the
        /// sync entry points take: the doc-boundary lock is keyed by the `Arc`
        /// identity, so a peer must be ONE `Arc` for the seal to mean anything.
        fn fresh_peer(peer_id: u64) -> Arc<LoroDoc> {
            let doc = LoroDoc::new();
            doc.set_peer_id(peer_id).unwrap();
            Arc::new(doc)
        }

        fn build_and_extract() -> (LoroDoc, Arc<LoroDoc>, loro::TreeID, loro::TreeID) {
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
            (doc, Arc::new(extracted.shared_doc), shared_root, block_b)
        }

        /// Helper: sync two LoroDoc instances via Iroh with the incremental
        /// protocol.
        async fn sync_pair(doc1: &Arc<LoroDoc>, doc2: &Arc<LoroDoc>, tree_id: &str) -> Result<()> {
            let alpn = make_alpn("loro-sync", tree_id);
            let ep1 = create_endpoint(vec![alpn.clone()]).await?;
            let ep2 = create_endpoint(vec![alpn.clone()]).await?;
            // Wait for endpoints to be ready (Iroh needs time for discovery)
            sleep(Duration::from_millis(500)).await;
            let addr2 = ep2.addr();

            let d2 = doc2.clone();
            let accept_id = tree_id.to_string();
            let handle = tokio::spawn(async move {
                sync_doc_accept(&ep2, &d2, &accept_id, Capabilities::read_write()).await
            });

            // Wait for acceptor to be ready
            sleep(Duration::from_millis(500)).await;
            let _conn = sync_doc_initiate(
                &ep1,
                doc1,
                &alpn,
                addr2,
                tree_id,
                Capabilities::read_write(),
            )
            .await
            .context("sync_doc_initiate failed")?;
            // _conn + ep1 kept alive while acceptor finishes reading
            handle.await?.context("sync_doc_accept failed")?;
            Ok(())
        }

        /// Both directions of the production iroh round must land the peer's
        /// delta through the document's own write guard, tagged
        /// `sync_import` — the origin the relay leg
        /// (`holon_sharing::sync::pull_once`) already uses. Without it a
        /// subscriber cannot tell a peer's write from a local one, and the
        /// import races the interior of a concurrent local batch.
        #[tokio::test]
        #[serial_test::serial]
        async fn both_iroh_legs_tag_a_peer_delta_sync_import() -> Result<()> {
            let (_source, shared_doc, _shared_root, block_b) = build_and_extract();

            let peer2_doc = fresh_peer(2);
            peer2_doc.import(&shared_doc.export(ExportMode::Snapshot)?)?;

            // One edit per side, so each side has a delta the other must import.
            edit_text(&shared_doc.get_tree(TREE_NAME), block_b, " - by P1");
            shared_doc.commit();
            edit_text(&peer2_doc.get_tree(TREE_NAME), block_b, " - by P2");
            peer2_doc.commit();

            let origins = |doc: &LoroDoc| {
                let seen = Arc::new(RwLock::new(Vec::<String>::new()));
                let sink = seen.clone();
                let sub = doc.subscribe_root(Arc::new(move |event| {
                    sink.write()
                        .expect("origin sink")
                        .push(event.origin.to_string());
                }));
                (seen, sub)
            };
            let (init_origins, _init_sub) = origins(&shared_doc);
            let (acc_origins, _acc_sub) = origins(&peer2_doc);

            sync_pair(&shared_doc, &peer2_doc, "origin-tag-1").await?;

            for (leg, seen) in [("initiator", &init_origins), ("acceptor", &acc_origins)] {
                let seen = seen.read().expect("origin sink").clone();
                assert!(
                    seen.iter()
                        .any(|o| o == crate::loro_document::SYNC_IMPORT_ORIGIN),
                    "the {leg} leg imported a peer delta under origin(s) {seen:?}, none of them \
                     `{}` — the import bypassed `LoroDocument`'s write guard",
                    crate::loro_document::SYNC_IMPORT_ORIGIN
                );
            }
            Ok(())
        }

        /// A Read-only peer's WRITE is refused over the LIVE iroh transport,
        /// with the typed `PeerAccessRefused` naming `Write` as missing.
        ///
        /// The unit tests in `peer_import` pin the same rule over a bare
        /// `Arc<LoroDoc>`; this one pins that a transport leg cannot hand the
        /// import a stronger admission than the share granted. The read
        /// direction is the control: the same round still delivers OUR delta to
        /// the peer, so an unchanged acceptor replica is the capability
        /// refusing the write and not a connection that never ran.
        #[tokio::test]
        #[serial_test::serial]
        async fn a_read_only_peers_write_over_iroh_is_refused_as_peer_access_refused() -> Result<()>
        {
            let (_source, initiator_doc, _shared_root, block_b) = build_and_extract();
            let acceptor_doc = fresh_peer(2);
            acceptor_doc.import(&initiator_doc.export(ExportMode::Snapshot)?)?;

            edit_text(&acceptor_doc.get_tree(TREE_NAME), block_b, " - by ACC");
            acceptor_doc.commit();
            edit_text(&initiator_doc.get_tree(TREE_NAME), block_b, " - by INIT");
            initiator_doc.commit();

            let tree_id = "read-only-write-refused";
            let alpn = make_alpn("loro-sync", tree_id);
            let ep1 = create_endpoint(vec![alpn.clone()]).await?;
            let ep2 = create_endpoint(vec![alpn.clone()]).await?;
            sleep(Duration::from_millis(500)).await;
            let addr2 = ep2.addr();

            let acc = acceptor_doc.clone();
            let accept_id = tree_id.to_string();
            let handle = tokio::spawn(async move {
                sync_doc_accept(&ep2, &acc, &accept_id, Capabilities::read_only()).await
            });
            sleep(Duration::from_millis(500)).await;
            // The initiator's own leg fails once the acceptor refuses and stops
            // writing; the observables are the acceptor's error and the two
            // replicas, so the dial's outcome is deliberately not asserted.
            let _ = sync_doc_initiate(
                &ep1,
                &initiator_doc,
                &alpn,
                addr2,
                tree_id,
                Capabilities::read_write(),
            )
            .await;

            let refusal = handle.await?.expect_err(
                "a read-only peer's delta must not be imported over the live transport",
            );
            let typed = refusal
                .chain()
                .find_map(|e| e.downcast_ref::<crate::peer_import::PeerAccessRefused>())
                .unwrap_or_else(|| {
                    panic!(
                        "the acceptor failed, but not with the typed `PeerAccessRefused` a \
                         capability refusal must be distinguishable by: {refusal:#}"
                    )
                });
            assert_eq!(
                typed.missing,
                holon_api::sharing::Capability::Write,
                "the refusal names the wrong missing capability: {typed}"
            );

            let acceptor_text = read_text(&acceptor_doc.get_tree(TREE_NAME), block_b);
            assert!(
                !acceptor_text.contains("by INIT"),
                "a read-only peer's edit landed in the acceptor's replica: {acceptor_text:?}"
            );
            let initiator_text = read_text(&initiator_doc.get_tree(TREE_NAME), block_b);
            assert!(
                initiator_text.contains("by ACC"),
                "control: the READ direction must still have run, so that the untouched acceptor \
                 replica is the write refusal and not a dead connection; initiator sees \
                 {initiator_text:?}"
            );
            Ok(())
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn incremental_sync_shared_tree() -> Result<()> {
            let (_source, shared_doc, _shared_root, block_b) = build_and_extract();

            let peer2_doc = fresh_peer(2);
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

            let peer2_doc = fresh_peer(2);
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

            let peer2_doc = fresh_peer(2);
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

            let peer2_doc = fresh_peer(2);
            peer2_doc.import(&shared_doc.export(ExportMode::Snapshot)?)?;

            manager.register_arc(stid.clone(), shared_doc);

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

            let p2_doc1 = fresh_peer(2);
            p2_doc1.import(&shared1.export(ExportMode::Snapshot)?)?;

            let p2_doc2 = fresh_peer(3);
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

        /// A peer that sends a length header exceeding `MAX_MSG_SIZE` must
        /// produce a loud `Err` (not a panic, not a giant allocation).
        #[tokio::test]
        #[serial_test::serial]
        async fn read_framed_rejects_oversize_length_without_panic() -> Result<()> {
            let alpn = make_alpn("loro-sync", "oversize-test");
            let ep1 = create_endpoint(vec![alpn.clone()]).await?;
            let ep2 = create_endpoint(vec![alpn.clone()]).await?;
            sleep(Duration::from_millis(500)).await;
            let addr2 = ep2.addr();

            let handle = tokio::spawn(async move {
                let incoming = ep2
                    .accept()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("no incoming connection"))?;
                let conn = incoming
                    .await
                    .map_err(|e| anyhow::anyhow!("accept failed: {e}"))?;
                let (mut _send, mut recv) = conn
                    .accept_bi()
                    .await
                    .map_err(|e| anyhow::anyhow!("accept_bi failed: {e}"))?;
                read_framed(&mut recv).await
            });

            sleep(Duration::from_millis(500)).await;

            let conn = ep1.connect(addr2, &alpn).await.context("connect failed")?;
            let (mut send, mut _recv) = conn
                .open_bi()
                .await
                .map_err(|e| anyhow::anyhow!("open_bi failed: {e}"))?;
            // Only a raw oversize length header, no body — the reader must
            // reject on the length alone before allocating.
            let oversize_len = ((MAX_MSG_SIZE + 1) as u32).to_be_bytes();
            send.write_all(&oversize_len).await?;
            send.finish()?;

            let result = handle.await?;
            let err = result.expect_err("read_framed should reject an oversize length");
            assert!(
                err.to_string().contains("oversize"),
                "expected an oversize error, got: {err}"
            );
            // Keep the initiator side alive until the acceptor has read.
            drop(conn);
            Ok(())
        }

        /// A peer that opens a stream, sends a length header, then stalls
        /// without sending the body must NOT pin the accept task forever:
        /// `sync_doc_accept` returns a timeout `Err` after `ACCEPT_IO_TIMEOUT`.
        /// Ignored by default because it waits the full 30s timeout; the fast
        /// oversize test above covers the framing bound in the default suite.
        #[tokio::test(flavor = "multi_thread")]
        #[serial_test::serial]
        #[ignore = "waits ACCEPT_IO_TIMEOUT (~30s); run with --ignored"]
        async fn acceptor_times_out_on_stalled_peer() -> Result<()> {
            let alpn = make_alpn("loro-sync", "slowloris-test");
            let ep1 = create_endpoint(vec![alpn.clone()]).await?;
            let ep2 = create_endpoint(vec![alpn.clone()]).await?;
            sleep(Duration::from_millis(500)).await;
            let addr2 = ep2.addr();

            let doc = fresh_peer(9);
            let handle = tokio::spawn(async move {
                sync_doc_accept(&ep2, &doc, "stall-test", Capabilities::read_write()).await
            });

            sleep(Duration::from_millis(500)).await;

            let conn = ep1.connect(addr2, &alpn).await.context("connect failed")?;
            let (mut send, mut _recv) = conn
                .open_bi()
                .await
                .map_err(|e| anyhow::anyhow!("open_bi failed: {e}"))?;
            // Announce a 16-byte body then stall: never write the body, never
            // finish. This is the slowloris the acceptor timeout defends against.
            send.write_all(&16u32.to_be_bytes()).await?;

            let outcome = tokio::time::timeout(ACCEPT_IO_TIMEOUT + Duration::from_secs(5), handle)
                .await
                .context("acceptor did not return within ACCEPT_IO_TIMEOUT bound")?;
            let result = outcome.context("acceptor task panicked")?;
            let err = result.expect_err("acceptor should error on a stalled peer");
            assert!(
                format!("{err:#}").contains("timed out"),
                "expected a timeout error, got: {err:#}"
            );
            // Hold the stalling initiator open until the assertion completes.
            drop(send);
            drop(conn);
            Ok(())
        }
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::DirectSync;
#[cfg(all(
    not(all(target_arch = "wasm32", target_os = "unknown")),
    any(test, feature = "test-helpers")
))]
pub use adapter::IrohSync;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::SharedTreeSyncManager;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::SyncBackend;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::connection_remote_addr;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::create_endpoint;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::create_endpoint_with_key;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::make_alpn;
// `sync_doc_accept` — the one un-gated acceptor left — is deliberately NOT
// re-exported: it is reachable only from inside the private `adapter` module,
// and only in a build where `test` or `test-helpers` is on.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::sync_doc_handle_connection;
// The un-enrolled dialer is reachable only under `cfg(test)`, and only from
// inside this crate: the tests that dial a gated share AS a stranger name it
// here, and nothing else can.
#[cfg(all(not(all(target_arch = "wasm32", target_os = "unknown")), test))]
pub(crate) use adapter::sync_doc_initiate;
pub use adapter::sync_doc_initiate_enrolled;
