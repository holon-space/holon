//! Persistent Iroh accepter pool for shared Loro subtrees.
//!
//! Each share owns its own `iroh::Endpoint` bound on
//! `loro-sync/{shared_tree_id}` and a background task that loops over incoming
//! connections, running the VV-based sync protocol against the shared doc.
//!
//! Shutdown: `drop_share` calls `endpoint.close()` which causes the pending
//! `endpoint.accept()` to return `None` and the task to exit.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use iroh::Endpoint;
use iroh::EndpointAddr;
use iroh::SecretKey;
use loro::LoroDoc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::debug;
use tracing::warn;

use crate::iroh_sync_adapter::connection_remote_addr;
use crate::iroh_sync_adapter::create_endpoint;
use crate::iroh_sync_adapter::create_endpoint_with_key;
use crate::iroh_sync_adapter::make_alpn;
use crate::iroh_sync_adapter::sync_doc_handle_connection;
use crate::share_enrollment::ShareRoster;
use crate::share_enrollment::acceptor_enroll;

pub const ALPN_PREFIX: &str = "loro-sync";

/// The acceptor-side roster for one share, shared with its accept loop. When a
/// share is advertised WITH a roster, every inbound connection must pass
/// [`acceptor_enroll`] (capability proof or B1 owner-signed admission) BEFORE
/// the sync protocol runs — closing the bearer-`shared_tree_id` forgery hole.
pub type SharedRoster = Arc<tokio::sync::Mutex<ShareRoster>>;

/// Callback fired after a successful inbound sync handshake. The
/// advertiser hands the dialer's `EndpointAddr` to the callback so
/// `LoroShareBackend` can remember it for later `sync_with_peers`
/// rounds — including after a restart, when the ticket author's addr
/// is stale.
pub type OnPeerConnected = Arc<dyn Fn(String, EndpointAddr) + Send + Sync>;

struct ShareHandle {
    endpoint: Endpoint,
    task: JoinHandle<()>,
    /// The acceptor roster, if this share enforces enrollment. Kept so the
    /// backend can snapshot the pinned-peer set into the C1 sidecar.
    roster: Option<SharedRoster>,
}

#[derive(Clone)]
pub struct IrohAdvertiser {
    shares: Arc<RwLock<HashMap<String, ShareHandle>>>,
    /// Optional stable secret key used to bind every share's
    /// `Endpoint`. When `Some`, iroh endpoint identity is stable
    /// across process restarts — critical for `known_peers` dedup on
    /// the remote side (the id is the dedup key). When `None`, a
    /// fresh key is generated per share — the legacy path used by
    /// standalone tests that don't care about restart identity.
    secret_key: Option<SecretKey>,
}

impl IrohAdvertiser {
    pub fn new() -> Self {
        Self {
            shares: Arc::new(RwLock::new(HashMap::new())),
            secret_key: None,
        }
    }

    /// Construct with a fixed device secret key. See field docs for
    /// why identity stability matters.
    pub fn new_with_key(secret_key: SecretKey) -> Self {
        Self {
            shares: Arc::new(RwLock::new(HashMap::new())),
            secret_key: Some(secret_key),
        }
    }

    /// Start advertising `doc` on `loro-sync/{shared_tree_id}`.
    /// Returns the `EndpointAddr` peers can dial (to put into the ticket).
    pub async fn start_share(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
    ) -> Result<EndpointAddr> {
        self.start_share_with_callback(shared_tree_id, doc, None, None, None)
            .await
    }

    /// Start advertising WITH an enrollment roster: every inbound peer must
    /// prove the share capability (or present an owner-signed device entry)
    /// before any sync. This is the enforced (H5) boundary.
    pub async fn start_share_gated(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
        roster: SharedRoster,
        on_peer_connected: Option<OnPeerConnected>,
        preferred_port: Option<u16>,
    ) -> Result<EndpointAddr> {
        self.start_share_with_callback(
            shared_tree_id,
            doc,
            on_peer_connected,
            preferred_port,
            Some(roster),
        )
        .await
    }

    /// The enrollment roster for a live share, if it is gated. Lets the backend
    /// snapshot the pinned-peer set into the signed sidecar.
    pub async fn roster_for(&self, shared_tree_id: &str) -> Option<SharedRoster> {
        self.shares
            .read()
            .await
            .get(shared_tree_id)
            .and_then(|h| h.roster.clone())
    }

    /// Variant of `start_share` that installs a callback fired after each
    /// successful inbound sync handshake. Used by `LoroShareBackend` to
    /// remember dialing peers' addresses for later bidirectional sync.
    ///
    /// `preferred_port` rebinds the same UDP port across restarts (keyed
    /// endpoints only) so peers' persisted addrs for this share stay
    /// dialable — see `create_endpoint_with_key`.
    pub async fn start_share_with_callback(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
        on_peer_connected: Option<OnPeerConnected>,
        preferred_port: Option<u16>,
        roster: Option<SharedRoster>,
    ) -> Result<EndpointAddr> {
        let mut guard = self.shares.write().await;
        if guard.contains_key(&shared_tree_id) {
            return Err(anyhow!(
                "share {shared_tree_id} is already being advertised"
            ));
        }

        let alpn = make_alpn(ALPN_PREFIX, &shared_tree_id);
        let endpoint = match &self.secret_key {
            Some(key) => create_endpoint_with_key(vec![alpn.clone()], key.clone(), preferred_port)
                .await
                .context("create iroh endpoint for advertiser (keyed)")?,
            None => create_endpoint(vec![alpn.clone()])
                .await
                .context("create iroh endpoint for advertiser")?,
        };
        // Iroh endpoints need a beat to publish their discovery info before
        // `addr()` returns something a peer can dial.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let addr = endpoint.addr();

        let accepter_ep = endpoint.clone();
        let task = tokio::spawn(accept_loop(
            accepter_ep,
            doc,
            shared_tree_id.clone(),
            on_peer_connected,
            roster.clone(),
        ));

        guard.insert(
            shared_tree_id,
            ShareHandle {
                endpoint,
                task,
                roster,
            },
        );
        tracing::debug!(addr = ?addr, "[advertiser] share endpoint bound");
        Ok(addr)
    }

    /// Stop advertising. Closes the endpoint and awaits the loop task.
    pub async fn drop_share(&self, shared_tree_id: &str) -> Result<()> {
        let handle = {
            let mut guard = self.shares.write().await;
            guard.remove(shared_tree_id)
        };
        let Some(handle) = handle else {
            return Err(anyhow!("no active share {shared_tree_id}"));
        };
        handle.endpoint.close().await;
        match handle.task.await {
            Ok(()) => Ok(()),
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(anyhow!("advertiser task panicked: {e}")),
        }
    }

    /// Close all active shares. Used on shutdown.
    pub async fn close_all(&self) {
        let handles: Vec<ShareHandle> = {
            let mut guard = self.shares.write().await;
            guard.drain().map(|(_, h)| h).collect()
        };
        for h in handles {
            h.endpoint.close().await;
            let _ = h.task.await;
        }
    }

    pub async fn is_active(&self, shared_tree_id: &str) -> bool {
        self.shares.read().await.contains_key(shared_tree_id)
    }

    /// Clone the accept-loop's endpoint for outbound dials.
    ///
    /// When we dial a peer for this share from a fresh endpoint, the
    /// peer's accept-loop records the *fresh* endpoint's addr — which
    /// dies as soon as the sync completes. Reusing the advertiser's
    /// long-lived endpoint means the addr the peer records is one
    /// that can be dialled later.
    pub async fn endpoint_for(&self, shared_tree_id: &str) -> Option<Endpoint> {
        self.shares
            .read()
            .await
            .get(shared_tree_id)
            .map(|h| h.endpoint.clone())
    }
}

impl Default for IrohAdvertiser {
    fn default() -> Self {
        Self::new()
    }
}

async fn accept_loop(
    endpoint: Endpoint,
    doc: Arc<LoroDoc>,
    shared_tree_id: String,
    on_peer_connected: Option<OnPeerConnected>,
    roster: Option<SharedRoster>,
) {
    debug!("[advertiser:{shared_tree_id}] accept loop started");
    while let Some(incoming) = endpoint.accept().await {
        let doc = doc.clone();
        let id = shared_tree_id.clone();
        let cb = on_peer_connected.clone();
        let roster = roster.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    warn!("[advertiser:{id}] handshake failed: {e}");
                    return;
                }
            };
            // Defence-in-depth ALPN re-check. Each share binds its own
            // single-ALPN endpoint, so iroh already refuses a mismatched
            // ALPN at the QUIC layer — but assert the invariant explicitly
            // in the handler so a future multi-ALPN endpoint can't silently
            // route a stranger's connection into this share's doc.
            let expected_alpn = make_alpn(ALPN_PREFIX, &id);
            if conn.alpn() != expected_alpn.as_slice() {
                warn!(
                    "[advertiser:{id}] rejecting connection with unexpected ALPN {:?}",
                    conn.alpn()
                );
                return;
            }
            // ENROLLMENT GATE (ADR 0028 H5). When this share is gated, the
            // dialer must prove the capability (or present an owner-signed
            // device entry) on a dedicated stream BEFORE we run sync. A peer
            // that merely knows the (leaky) `shared_tree_id` — a forged ticket
            // — cannot pass this and never reaches `sync_doc_handle_connection`,
            // so it can neither read nor write the shared doc. An UN-gated
            // share keeps the legacy behaviour (used by standalone transport
            // tests that construct the advertiser directly).
            if let Some(roster) = roster.as_ref() {
                let now = chrono::Utc::now().timestamp();
                let mut guard = roster.lock().await;
                match acceptor_enroll(&conn, &mut guard, now).await {
                    Ok(authorized) => {
                        debug!(
                            "[advertiser:{id}] peer enrolled (newly={})",
                            authorized.newly_enrolled()
                        );
                    }
                    Err(e) => {
                        warn!(
                            "[advertiser:{id}] REJECTED unauthorized peer at enrollment gate: {e:#}"
                        );
                        conn.close(1u32.into(), b"enrollment refused");
                        return;
                    }
                }
            }
            // Capture dialer addr BEFORE running the sync protocol —
            // sync reads/writes framed bytes and may drop the
            // connection on errors, at which point `paths()` empties
            // out. Grabbing the addr up-front gives the backend
            // something to persist even if the sync itself fails.
            let remote = connection_remote_addr(&conn);
            if let Some(ref cb) = cb {
                cb(id.clone(), remote);
            }
            if let Err(e) = sync_doc_handle_connection(conn, &doc).await {
                warn!("[advertiser:{id}] sync connection failed: {e:#}");
            }
        });
    }
    debug!("[advertiser:{shared_tree_id}] accept loop exited");
}

#[cfg(test)]
mod tests {
    use loro::ExportMode;
    use loro::LoroText;

    use super::*;
    use crate::iroh_sync_adapter::sync_doc_initiate;
    use crate::loro_backend::TREE_NAME;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn advertiser_start_stop() -> Result<()> {
        let adv = IrohAdvertiser::new();
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1)?;

        let _addr = adv.start_share("t1".into(), doc.clone()).await?;
        assert!(adv.is_active("t1").await);
        adv.drop_share("t1").await?;
        assert!(!adv.is_active("t1").await);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn advertiser_serves_initiator() -> Result<()> {
        let adv = IrohAdvertiser::new();

        // Set up shared doc with one node on the advertiser side.
        let server_doc = Arc::new(LoroDoc::new());
        server_doc.set_peer_id(11)?;
        {
            let tree = server_doc.get_tree(TREE_NAME);
            tree.enable_fractional_index(0);
            let root = tree.create(None)?;
            let meta = tree.get_meta(root)?;
            let text: LoroText = meta.insert_container("content_raw", LoroText::new())?;
            text.insert(0, "hello")?;
        }
        server_doc.commit();

        let addr = adv
            .start_share("sharedA".into(), server_doc.clone())
            .await?;

        // Client pulls.
        let client_doc = LoroDoc::new();
        client_doc.set_peer_id(22)?;
        let alpn = make_alpn(ALPN_PREFIX, "sharedA");
        let client_ep = create_endpoint(vec![alpn.clone()]).await?;
        // Iroh needs a beat for endpoints to be discoverable over the local
        // discovery services.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let _conn = sync_doc_initiate(&client_ep, &client_doc, &alpn, addr).await?;

        let snap = server_doc.export(ExportMode::Snapshot)?;
        let expected = {
            let d = LoroDoc::new();
            d.import(&snap)?;
            d.get_deep_value()
        };
        assert_eq!(client_doc.get_deep_value(), expected);

        adv.drop_share("sharedA").await?;
        Ok(())
    }

    /// FLAGSHIP (ADR 0028 H5): the enrollment gate closes the bearer-
    /// `shared_tree_id` forgery hole over the LIVE iroh transport.
    ///
    /// Three peers dial a GATED share, all knowing the leaky `shared_tree_id`
    /// (the ALPN) and the dialable `addr` — everything a forged ticket carries:
    ///
    /// 1. an attacker that does NOT enroll (plain `sync_doc_initiate`, the
    ///    `advertiser_serves_initiator` path that used to just work) — the
    ///    gated acceptor waits for the enrollment stream, reads the sync bytes
    ///    as a malformed proof, and rejects. The attacker pulls NOTHING.
    /// 2. an attacker with a FORGED capability (its own freshly-minted
    ///    `CapabilitySecret`) — enrollment proof fails, rejected, pulls
    ///    NOTHING.
    /// 3. the honest recipient holding the REAL capability (as delivered in the
    ///    ticket) — enrolls and syncs the content.
    ///
    /// Red-first: on an UN-gated share (case 1's `sync_doc_initiate` against a
    /// plain `start_share`) the same attacker succeeds — that is exactly the
    /// hole `advertiser_serves_initiator` demonstrates and this gate closes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn gated_share_rejects_forged_ticket_serves_enrolled_peer() -> Result<()> {
        use crate::iroh_sync_adapter::sync_doc_initiate_enrolled;
        use crate::share_enrollment::CapabilitySecret;
        use crate::share_enrollment::ExpiryTime;
        use crate::share_enrollment::ShareRoster;

        let tree_id = "gatedShare";
        let server_doc = Arc::new(LoroDoc::new());
        server_doc.set_peer_id(11)?;
        {
            let tree = server_doc.get_tree(TREE_NAME);
            tree.enable_fractional_index(0);
            let root = tree.create(None)?;
            let meta = tree.get_meta(root)?;
            let text: LoroText = meta.insert_container("content_raw", LoroText::new())?;
            text.insert(0, "secret-shared-content")?;
        }
        server_doc.commit();

        let expected = {
            let snap = server_doc.export(ExportMode::Snapshot)?;
            let d = LoroDoc::new();
            d.import(&snap)?;
            d.get_deep_value()
        };

        // The REAL capability the honest recipient's ticket carries. A generous
        // expiry so the window is never the reason for a rejection here.
        let real_cap = CapabilitySecret::generate();
        let roster = Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
            tree_id,
            real_cap.clone(),
            ExpiryTime(chrono::Utc::now().timestamp() + 3600),
            4,
        )));

        let adv = IrohAdvertiser::new();
        let addr = adv
            .start_share_gated(
                tree_id.into(),
                server_doc.clone(),
                roster.clone(),
                None,
                None,
            )
            .await?;
        let alpn = make_alpn(ALPN_PREFIX, tree_id);

        // --- Case 1: attacker knows the ALPN but does NOT enroll ---
        let no_enroll_doc = LoroDoc::new();
        no_enroll_doc.set_peer_id(97)?;
        let ep1 = create_endpoint(vec![alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let res1 = sync_doc_initiate(&ep1, &no_enroll_doc, &alpn, addr.clone()).await;
        // Whether the dial errors or completes, the attacker must not have
        // pulled the content.
        assert_ne!(
            no_enroll_doc.get_deep_value(),
            expected,
            "un-enrolled peer must not receive the shared content"
        );
        let _ = res1;

        // --- Case 2: attacker with a FORGED capability ---
        let forged_cap = CapabilitySecret::generate();
        let forged_doc = LoroDoc::new();
        forged_doc.set_peer_id(98)?;
        let ep2 = create_endpoint(vec![alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let res2 = sync_doc_initiate_enrolled(
            &ep2,
            &forged_doc,
            &alpn,
            addr.clone(),
            &forged_cap,
            tree_id,
        )
        .await;
        assert!(
            res2.is_err(),
            "forged capability must be rejected at the enrollment gate, got Ok"
        );
        assert_ne!(
            forged_doc.get_deep_value(),
            expected,
            "peer with a forged capability must not receive the shared content"
        );

        // --- Case 3: honest recipient with the REAL capability ---
        let honest_doc = LoroDoc::new();
        honest_doc.set_peer_id(22)?;
        let ep3 = create_endpoint(vec![alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        sync_doc_initiate_enrolled(&ep3, &honest_doc, &alpn, addr, &real_cap, tree_id)
            .await
            .context("honest recipient with the real capability must enroll and sync")?;
        assert_eq!(
            honest_doc.get_deep_value(),
            expected,
            "enrolled honest recipient must receive the shared content"
        );

        // The roster pinned exactly the one honest peer.
        assert_eq!(roster.lock().await.enrolled_count(), 1);

        adv.drop_share(tree_id).await?;
        Ok(())
    }

    /// Live-gate rejections beyond forgery: an EXPIRED enrollment window and a
    /// peer that exceeds the roster cap (the bound on a leaked capability's
    /// blast radius) are both refused at the live transport, holding the real
    /// capability. (Replay is structurally prevented — the acceptor mints a
    /// fresh per-connection `Challenge`, so a captured proof never re-verifies;
    /// that binding is locked by the `share_enrollment` state-machine tests.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn gated_gate_rejects_expired_and_over_cap() -> Result<()> {
        use crate::iroh_sync_adapter::sync_doc_initiate_enrolled;
        use crate::share_enrollment::CapabilitySecret;
        use crate::share_enrollment::ExpiryTime;
        use crate::share_enrollment::ShareRoster;

        // --- EXPIRED window: honest capability, but enrollment past expiry ---
        let exp_id = "expiredShare";
        let exp_doc = Arc::new(LoroDoc::new());
        exp_doc.set_peer_id(31)?;
        {
            let tree = exp_doc.get_tree(TREE_NAME);
            tree.enable_fractional_index(0);
            let root = tree.create(None)?;
            let meta = tree.get_meta(root)?;
            let text: LoroText = meta.insert_container("content_raw", LoroText::new())?;
            text.insert(0, "expired-content")?;
        }
        exp_doc.commit();
        let exp_cap = CapabilitySecret::generate();
        let exp_roster = Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
            exp_id,
            exp_cap.clone(),
            ExpiryTime(chrono::Utc::now().timestamp() - 100), // already expired
            4,
        )));
        let adv = IrohAdvertiser::new();
        let exp_addr = adv
            .start_share_gated(exp_id.into(), exp_doc.clone(), exp_roster, None, None)
            .await?;
        let exp_alpn = make_alpn(ALPN_PREFIX, exp_id);
        let late_doc = LoroDoc::new();
        late_doc.set_peer_id(32)?;
        let ep = create_endpoint(vec![exp_alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let res =
            sync_doc_initiate_enrolled(&ep, &late_doc, &exp_alpn, exp_addr, &exp_cap, exp_id).await;
        assert!(res.is_err(), "expired enrollment window must be rejected");
        adv.drop_share(exp_id).await?;

        // --- OVER-CAP: max_peers=1, a second distinct device is refused even
        // with the real capability (bounds a leaked capability). ---
        let cap_id = "cappedShare";
        let cap_doc = Arc::new(LoroDoc::new());
        cap_doc.set_peer_id(41)?;
        {
            let tree = cap_doc.get_tree(TREE_NAME);
            tree.enable_fractional_index(0);
            let root = tree.create(None)?;
            let meta = tree.get_meta(root)?;
            let text: LoroText = meta.insert_container("content_raw", LoroText::new())?;
            text.insert(0, "capped-content")?;
        }
        cap_doc.commit();
        let cap = CapabilitySecret::generate();
        let capped_roster = Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
            cap_id,
            cap.clone(),
            ExpiryTime(chrono::Utc::now().timestamp() + 3600),
            1, // room for exactly one device
        )));
        let adv2 = IrohAdvertiser::new();
        let cap_addr = adv2
            .start_share_gated(
                cap_id.into(),
                cap_doc.clone(),
                capped_roster.clone(),
                None,
                None,
            )
            .await?;
        let cap_alpn = make_alpn(ALPN_PREFIX, cap_id);

        let first = LoroDoc::new();
        first.set_peer_id(42)?;
        let ep_first = create_endpoint(vec![cap_alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        sync_doc_initiate_enrolled(&ep_first, &first, &cap_alpn, cap_addr.clone(), &cap, cap_id)
            .await
            .context("first device fills the single roster slot")?;

        let second = LoroDoc::new();
        second.set_peer_id(43)?;
        let ep_second = create_endpoint(vec![cap_alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let over =
            sync_doc_initiate_enrolled(&ep_second, &second, &cap_alpn, cap_addr, &cap, cap_id)
                .await;
        assert!(
            over.is_err(),
            "a second device beyond the roster cap must be rejected"
        );
        assert_eq!(capped_roster.lock().await.enrolled_count(), 1);
        adv2.drop_share(cap_id).await?;
        Ok(())
    }
}
