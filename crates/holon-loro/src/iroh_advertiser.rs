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
use holon_api::sharing::Capabilities;
/// The iroh transport handles this module's public API already speaks
/// ([`IrohAdvertiser::endpoint_for`], [`IrohAdvertiser::start_share`]'s return,
/// [`crate::ContainerRegistry::replicate_all`]'s). Re-exported so a caller can
/// name them without taking its own direct `iroh` dependency.
pub use iroh::Endpoint;
pub use iroh::EndpointAddr;
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
use crate::peer_import::AdmittedPeer;
use crate::peer_import::PeerReadAccess;
use crate::peer_import::authorize_peer_read;
use crate::share_enrollment::AcceptorRefused;
use crate::share_enrollment::ENROLLMENT_FAILED_CODE;
use crate::share_enrollment::ENROLLMENT_REFUSED_CODE;
use crate::share_enrollment::ShareRoster;
use crate::share_enrollment::acceptor_enroll;
use crate::share_enrollment::peer_fingerprint;

pub const ALPN_PREFIX: &str = "loro-sync";

/// The acceptor-side roster for one share, shared with its accept loop. When a
/// share is advertised WITH a roster, every inbound connection must pass
/// [`acceptor_enroll`] (capability proof or B1 owner-signed admission) BEFORE
/// the sync protocol runs — closing the bearer-`shared_tree_id` forgery hole.
pub type SharedRoster = Arc<tokio::sync::Mutex<ShareRoster>>;

/// How one advertised share decides who may sync it, and what a peer that
/// passes may then do. Every share states one — there is no `None` that means
/// "whatever happens", because that is what let the H5 gate stay un-wired on
/// the path that ships.
#[derive(Clone)]
pub enum ShareAdmission {
    /// Every inbound peer proves the share capability (or presents an
    /// owner-signed device entry) against `roster` BEFORE any sync, and is
    /// admitted with `capabilities`.
    ///
    /// The split is deliberate and mirrors `holon_sharing::policy::Policy`: the
    /// roster answers *is this peer a member*, the share declares *what
    /// membership confers here*.
    Enrolled {
        roster: SharedRoster,
        capabilities: Capabilities,
    },
    /// DISCLOSED HOLE — no enrollment runs, so every peer that reaches the
    /// endpoint is admitted with `capabilities`. The `shared_tree_id` sits in
    /// the ALPN and in projected rows, so "reaches the endpoint" is everyone
    /// who can route to us.
    ///
    /// Spelled per call site rather than implied by a `None`, so
    /// `rg 'ShareAdmission::Ungated'` enumerates exactly the shares still to be
    /// gated. See bugfunnel
    /// `2026-09-08-the-subtree-share-hot-path-advertises-un-gated`.
    Ungated { capabilities: Capabilities },
}

/// Callback fired for an inbound dialer the admission ACCEPTED, so
/// `LoroShareBackend` can remember its `EndpointAddr` for later
/// `sync_with_peers` rounds — including after a restart, when the ticket
/// author's addr is stale.
///
/// It takes the read witness rather than a container name plus an addr,
/// because remembering a peer is what makes this device dial it back with a
/// grant of its own: a peer the admission refuses must never become a peer we
/// dial. The witness is the only source of the container name here, so an
/// unauthorized dialer cannot be handed to the callback at all.
pub type OnPeerConnected = Arc<dyn Fn(&PeerReadAccess, EndpointAddr) + Send + Sync>;

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

    /// Start advertising `doc` on `loro-sync/{shared_tree_id}` with NO
    /// enrollment: every peer that reaches the endpoint is admitted with
    /// `capabilities`. Naming the capabilities is the point — see
    /// [`ShareAdmission::Ungated`].
    pub async fn start_share_ungated(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
        capabilities: Capabilities,
    ) -> Result<EndpointAddr> {
        self.start_share_with_callback(
            shared_tree_id,
            doc,
            None,
            None,
            ShareAdmission::Ungated { capabilities },
        )
        .await
    }

    /// Start advertising WITH an enrollment roster: every inbound peer must
    /// prove the share capability (or present an owner-signed device entry)
    /// before any sync, and is then admitted with `capabilities`. This is the
    /// enforced (H5) boundary.
    pub async fn start_share_gated(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
        roster: SharedRoster,
        capabilities: Capabilities,
        on_peer_connected: Option<OnPeerConnected>,
        preferred_port: Option<u16>,
    ) -> Result<EndpointAddr> {
        self.start_share_with_callback(
            shared_tree_id,
            doc,
            on_peer_connected,
            preferred_port,
            ShareAdmission::Enrolled {
                roster,
                capabilities,
            },
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

    /// Variant of `start_share` that installs a callback fired for each
    /// inbound dialer the admission accepted, BEFORE its sync round runs (the
    /// dialer's addr is only readable while the connection is up). Used by
    /// `LoroShareBackend` to remember those addresses for later bidirectional
    /// sync.
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
        admission: ShareAdmission,
    ) -> Result<EndpointAddr> {
        if let ShareAdmission::Ungated { capabilities } = &admission {
            warn!(
                shared_tree_id = %shared_tree_id,
                granted = %capabilities,
                "[advertiser] share advertised UN-GATED: no enrollment runs, so any peer that \
                 reaches this endpoint is admitted with these capabilities"
            );
        }
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

        let roster = match &admission {
            ShareAdmission::Enrolled { roster, .. } => Some(roster.clone()),
            ShareAdmission::Ungated { .. } => None,
        };
        let accepter_ep = endpoint.clone();
        let task = tokio::spawn(accept_loop(
            accepter_ep,
            doc,
            shared_tree_id.clone(),
            on_peer_connected,
            admission,
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
    admission: ShareAdmission,
) {
    debug!("[advertiser:{shared_tree_id}] accept loop started");
    while let Some(incoming) = endpoint.accept().await {
        let doc = doc.clone();
        let id = shared_tree_id.clone();
        let cb = on_peer_connected.clone();
        let admission = admission.clone();
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
            //
            // The decision's OUTCOME is what the sync leg then runs on: the
            // `AdmittedPeer` below is the only thing that can carry a peer's
            // delta into the doc (D86.a), so an enrollment that never happened
            // cannot be silently followed by an import.
            let admitted = match &admission {
                ShareAdmission::Enrolled {
                    roster,
                    capabilities,
                } => {
                    let now = chrono::Utc::now().timestamp();
                    let mut guard = roster.lock().await;
                    match acceptor_enroll(&conn, &mut guard, now).await {
                        Ok(authorized) => {
                            debug!(
                                "[advertiser:{id}] peer enrolled (newly={}, granted={})",
                                authorized.newly_enrolled(),
                                capabilities
                            );
                            AdmittedPeer::enrolled(&id, &authorized, capabilities.clone())
                        }
                        Err(e) => {
                            // The close code is the ONE signal the dialer can
                            // read to tell a refusal from an I/O failure, so it
                            // is set by matching the typed error, never by
                            // phrasing.
                            let refused = e.downcast_ref::<AcceptorRefused>().is_some();
                            warn!(
                                "[advertiser:{id}] enrollment gate closed the connection \
                                 (refused={refused}): {e:#}"
                            );
                            if refused {
                                conn.close(ENROLLMENT_REFUSED_CODE.into(), b"enrollment refused");
                            } else {
                                conn.close(ENROLLMENT_FAILED_CODE.into(), b"enrollment failed");
                            }
                            return;
                        }
                    }
                }
                ShareAdmission::Ungated { capabilities } => {
                    AdmittedPeer::ungated(&id, peer_fingerprint(&conn), capabilities.clone())
                }
            };
            // The read gate runs HERE, not inside the sync leg, because the
            // callback below makes this peer one we dial BACK (with a grant of
            // our own): a refusal has to stop the addr from being remembered,
            // not merely stop this round.
            let access = match authorize_peer_read(&admitted) {
                Ok(access) => access,
                Err(e) => {
                    warn!("[advertiser:{id}] refusing an admitted peer's read: {e:#}");
                    // Same close code the enrollment refusal uses: from the
                    // dialer's side both are "the acceptor decided against
                    // you", and the code is the only signal it can read.
                    conn.close(ENROLLMENT_REFUSED_CODE.into(), b"capability refused");
                    return;
                }
            };
            // Capture dialer addr BEFORE running the sync protocol —
            // sync reads/writes framed bytes and may drop the
            // connection on errors, at which point `paths()` empties
            // out. Grabbing the addr up-front gives the backend
            // something to persist even if the sync itself fails.
            let remote = connection_remote_addr(&conn);
            if let Some(ref cb) = cb {
                cb(&access, remote);
            }
            if let Err(e) = sync_doc_handle_connection(conn, &doc, &access).await {
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
    use crate::iroh_sync_adapter::sync_doc_initiate_enrolled;
    use crate::loro_backend::TREE_NAME;

    /// Enrolled dial granting the peer a full writer — what every enrollment
    /// test here is about, so the capability argument stays out of the way of
    /// the property under test.
    async fn sync_doc_initiate_enrolled_rw(
        endpoint: &Endpoint,
        doc: &Arc<LoroDoc>,
        alpn: &[u8],
        peer_addr: EndpointAddr,
        capability: &crate::share_enrollment::CapabilitySecret,
        shared_tree_id: &str,
    ) -> Result<iroh::endpoint::Connection> {
        sync_doc_initiate_enrolled(
            endpoint,
            doc,
            alpn,
            peer_addr,
            capability,
            shared_tree_id,
            Capabilities::read_write(),
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn advertiser_start_stop() -> Result<()> {
        let adv = IrohAdvertiser::new();
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1)?;

        let _addr = adv
            .start_share_ungated("t1".into(), doc.clone(), Capabilities::read_write())
            .await?;
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
            let text: LoroText = meta.ensure_mergeable_text("content_raw")?;
            text.insert(0, "hello")?;
        }
        server_doc.commit();

        let addr = adv
            .start_share_ungated(
                "sharedA".into(),
                server_doc.clone(),
                Capabilities::read_write(),
            )
            .await?;

        // Client pulls.
        let client_doc = Arc::new(LoroDoc::new());
        client_doc.set_peer_id(22)?;
        let alpn = make_alpn(ALPN_PREFIX, "sharedA");
        let client_ep = create_endpoint(vec![alpn.clone()]).await?;
        // Iroh needs a beat for endpoints to be discoverable over the local
        // discovery services.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let _conn = sync_doc_initiate(
            &client_ep,
            &client_doc,
            &alpn,
            addr,
            "sharedA",
            Capabilities::read_write(),
        )
        .await?;

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

    /// A peer the admission REFUSES must not be promoted to a peer this device
    /// remembers — and therefore later DIALS.
    ///
    /// `LoroShareBackend` persists whatever the on-peer-connected callback
    /// hands it into the known-peers sidecar, and `sync_with_peers` dials every
    /// addr in that sidecar granting `Capabilities::read_write()`. So firing
    /// the callback before the capability check makes the refusal one-round
    /// only: refusing a peer's read still hands it a full-writer round next
    /// time. The control case in the same test proves the callback is alive,
    /// so an empty recording is the refusal and not a broken dial.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn a_peer_refused_for_read_is_never_remembered_as_a_known_peer() -> Result<()> {
        async fn remembered_after_a_dial(id: &str, granted: Capabilities) -> Result<Vec<String>> {
            let adv = IrohAdvertiser::new();
            let server_doc = Arc::new(LoroDoc::new());
            server_doc.set_peer_id(11)?;
            server_doc.commit();

            let recorded = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let sink = recorded.clone();
            let cb: OnPeerConnected = Arc::new(move |access: &PeerReadAccess, _addr| {
                sink.lock()
                    .expect("remembered-peer sink")
                    .push(access.container().to_string());
            });

            let addr = adv
                .start_share_with_callback(
                    id.to_string(),
                    server_doc.clone(),
                    Some(cb),
                    None,
                    ShareAdmission::Ungated {
                        capabilities: granted,
                    },
                )
                .await?;

            let alpn = make_alpn(ALPN_PREFIX, id);
            let client_doc = Arc::new(LoroDoc::new());
            client_doc.set_peer_id(22)?;
            let client_ep = create_endpoint(vec![alpn.clone()]).await?;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // The refused round fails on the dialer's side too (the acceptor
            // closes on it); the assertion is about what the ACCEPTOR
            // remembered, so the dial's own outcome is not the observable.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                sync_doc_initiate(
                    &client_ep,
                    &client_doc,
                    &alpn,
                    addr,
                    id,
                    Capabilities::read_write(),
                ),
            )
            .await;
            // The callback is fired from the accept task, which outlives the
            // dial by a beat.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            adv.drop_share(id).await?;
            let out = recorded.lock().expect("remembered-peer sink").clone();
            Ok(out)
        }

        let refused = remembered_after_a_dial("refused-read", Capabilities::of([])).await?;
        assert!(
            refused.is_empty(),
            "a peer admitted with NO capabilities was remembered for container(s) {refused:?} — \
             `sync_with_peers` will dial it back granting read+write, so the refusal held for one \
             round only"
        );

        let admitted = remembered_after_a_dial("admitted-read", Capabilities::read_write()).await?;
        assert_eq!(
            admitted,
            vec!["admitted-read".to_string()],
            "control: a peer the admission accepts must still be remembered"
        );
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
            let text: LoroText = meta.ensure_mergeable_text("content_raw")?;
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
                Capabilities::read_write(),
                None,
                None,
            )
            .await?;
        let alpn = make_alpn(ALPN_PREFIX, tree_id);

        // --- Case 1: attacker knows the ALPN but does NOT enroll ---
        let no_enroll_doc = Arc::new(LoroDoc::new());
        no_enroll_doc.set_peer_id(97)?;
        let ep1 = create_endpoint(vec![alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let res1 = sync_doc_initiate(
            &ep1,
            &no_enroll_doc,
            &alpn,
            addr.clone(),
            tree_id,
            Capabilities::read_write(),
        )
        .await;
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
        let forged_doc = Arc::new(LoroDoc::new());
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
            Capabilities::read_write(),
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
        let honest_doc = Arc::new(LoroDoc::new());
        honest_doc.set_peer_id(22)?;
        let ep3 = create_endpoint(vec![alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        sync_doc_initiate_enrolled(
            &ep3,
            &honest_doc,
            &alpn,
            addr,
            &real_cap,
            tree_id,
            Capabilities::read_write(),
        )
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

    /// A refusal must be distinguishable from an I/O failure, by TYPE.
    ///
    /// Every enrollment failure past `connect()` travels under the same
    /// "enrollment" context, so classifying on message text calls a timeout,
    /// a stream that never opens and a dropped connection all "refused" — and
    /// an unshared-vault negative then passes on a network fault instead of on
    /// a security decision. Three live dials, one per shape:
    ///
    /// 1. a GATED share turning down a forged capability — the only refusal;
    /// 2. an acceptor that closes with [`ENROLLMENT_FAILED_CODE`] — the shape
    ///    the real accept loop uses when enrollment fails for any non-roster
    ///    reason;
    /// 3. an acceptor that drops the connection with no application close at
    ///    all — the unclassifiable case, which must stay loud.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn only_a_roster_refusal_is_typed_as_an_enrollment_refusal() -> Result<()> {
        use crate::share_enrollment::CapabilitySecret;
        use crate::share_enrollment::EnrollmentRefused;
        use crate::share_enrollment::ExpiryTime;
        use crate::share_enrollment::ShareRoster;

        fn is_refusal(e: &anyhow::Error) -> bool {
            e.downcast_ref::<EnrollmentRefused>().is_some()
        }

        // --- 1. a real roster refusal ---
        let tree_id = "classifyRefusal";
        let server_doc = Arc::new(LoroDoc::new());
        server_doc.set_peer_id(31)?;
        let roster = Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
            tree_id,
            CapabilitySecret::generate(),
            ExpiryTime(chrono::Utc::now().timestamp() + 3600),
            4,
        )));
        let adv = IrohAdvertiser::new();
        let addr = adv
            .start_share_gated(
                tree_id.into(),
                server_doc,
                roster,
                Capabilities::read_write(),
                None,
                None,
            )
            .await?;
        let alpn = make_alpn(ALPN_PREFIX, tree_id);
        let forged = CapabilitySecret::generate();
        let doc1 = Arc::new(LoroDoc::new());
        doc1.set_peer_id(32)?;
        let ep1 = create_endpoint(vec![alpn.clone()]).await?;
        let refusal = sync_doc_initiate_enrolled(
            &ep1,
            &doc1,
            &alpn,
            addr,
            &forged,
            tree_id,
            Capabilities::read_write(),
        )
        .await
        .expect_err("a forged capability must be refused");
        assert!(
            is_refusal(&refusal),
            "the roster's refusal was not typed as one: {refusal:#}"
        );
        adv.drop_share(tree_id).await?;

        // --- 2. an acceptor that FAILED rather than refused ---
        let fail_id = "classifyFailed";
        let fail_alpn = make_alpn(ALPN_PREFIX, fail_id);
        let fail_ep = create_endpoint(vec![fail_alpn.clone()]).await?;
        let fail_addr = fail_ep.addr();
        let fail_task = tokio::spawn(async move {
            if let Some(incoming) = fail_ep.accept().await
                && let Ok(conn) = incoming.await
            {
                conn.close(
                    crate::share_enrollment::ENROLLMENT_FAILED_CODE.into(),
                    b"enrollment failed",
                );
                conn.closed().await;
            }
        });
        let doc2 = Arc::new(LoroDoc::new());
        doc2.set_peer_id(33)?;
        let ep2 = create_endpoint(vec![fail_alpn.clone()]).await?;
        let failure =
            sync_doc_initiate_enrolled_rw(&ep2, &doc2, &fail_alpn, fail_addr, &forged, fail_id)
                .await
                .expect_err("an acceptor that closes mid-enrollment must fail the dial");
        assert!(
            !is_refusal(&failure),
            "an I/O failure was typed as a REFUSAL, so an unshared-vault negative would pass on a \
             network fault instead of on a security decision: {failure:#}"
        );
        fail_task.abort();

        // --- 3. no application close at all ---
        let drop_id = "classifyDropped";
        let drop_alpn = make_alpn(ALPN_PREFIX, drop_id);
        let drop_ep = create_endpoint(vec![drop_alpn.clone()]).await?;
        let drop_addr = drop_ep.addr();
        let drop_task = tokio::spawn(async move {
            if let Some(incoming) = drop_ep.accept().await {
                drop(incoming.await);
            }
        });
        let doc3 = Arc::new(LoroDoc::new());
        doc3.set_peer_id(34)?;
        let ep3 = create_endpoint(vec![drop_alpn.clone()]).await?;
        let dropped =
            sync_doc_initiate_enrolled_rw(&ep3, &doc3, &drop_alpn, drop_addr, &forged, drop_id)
                .await
                .expect_err("a dropped connection must fail the dial");
        assert!(
            !is_refusal(&dropped),
            "a connection that carried NO application close was typed as a refusal; an \
             unclassifiable failure must stay loud: {dropped:#}"
        );
        drop_task.abort();

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
            let text: LoroText = meta.ensure_mergeable_text("content_raw")?;
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
            .start_share_gated(
                exp_id.into(),
                exp_doc.clone(),
                exp_roster,
                Capabilities::read_write(),
                None,
                None,
            )
            .await?;
        let exp_alpn = make_alpn(ALPN_PREFIX, exp_id);
        let late_doc = Arc::new(LoroDoc::new());
        late_doc.set_peer_id(32)?;
        let ep = create_endpoint(vec![exp_alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let res =
            sync_doc_initiate_enrolled_rw(&ep, &late_doc, &exp_alpn, exp_addr, &exp_cap, exp_id)
                .await;
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
            let text: LoroText = meta.ensure_mergeable_text("content_raw")?;
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
                Capabilities::read_write(),
                None,
                None,
            )
            .await?;
        let cap_alpn = make_alpn(ALPN_PREFIX, cap_id);

        let first = Arc::new(LoroDoc::new());
        first.set_peer_id(42)?;
        let ep_first = create_endpoint(vec![cap_alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        sync_doc_initiate_enrolled_rw(&ep_first, &first, &cap_alpn, cap_addr.clone(), &cap, cap_id)
            .await
            .context("first device fills the single roster slot")?;

        let second = Arc::new(LoroDoc::new());
        second.set_peer_id(43)?;
        let ep_second = create_endpoint(vec![cap_alpn.clone()]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let over =
            sync_doc_initiate_enrolled_rw(&ep_second, &second, &cap_alpn, cap_addr, &cap, cap_id)
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
