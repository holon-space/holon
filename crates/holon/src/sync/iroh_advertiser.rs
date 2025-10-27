//! Persistent Iroh accepter pool for shared Loro subtrees.
//!
//! Each share owns its own `iroh::Endpoint` bound on
//! `loro-sync/{shared_tree_id}` and a background task that loops over incoming
//! connections, running the VV-based sync protocol against the shared doc.
//!
//! Shutdown: `drop_share` calls `endpoint.close()` which causes the pending
//! `endpoint.accept()` to return `None` and the task to exit.

use crate::sync::iroh_sync_adapter::{
    connection_remote_addr, create_endpoint, create_endpoint_with_key, make_alpn,
    sync_doc_handle_connection,
};
use anyhow::{Context, Result, anyhow};
use iroh::{Endpoint, EndpointAddr, SecretKey};
use loro::LoroDoc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

pub const ALPN_PREFIX: &str = "loro-sync";

/// Callback fired after a successful inbound sync handshake. The
/// advertiser hands the dialer's `EndpointAddr` to the callback so
/// `LoroShareBackend` can remember it for later `sync_with_peers`
/// rounds — including after a restart, when the ticket author's addr
/// is stale.
pub type OnPeerConnected = Arc<dyn Fn(String, EndpointAddr) + Send + Sync>;

struct ShareHandle {
    endpoint: Endpoint,
    task: JoinHandle<()>,
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
        self.start_share_with_callback(shared_tree_id, doc, None)
            .await
    }

    /// Variant of `start_share` that installs a callback fired after each
    /// successful inbound sync handshake. Used by `LoroShareBackend` to
    /// remember dialing peers' addresses for later bidirectional sync.
    pub async fn start_share_with_callback(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
        on_peer_connected: Option<OnPeerConnected>,
    ) -> Result<EndpointAddr> {
        let mut guard = self.shares.write().await;
        if guard.contains_key(&shared_tree_id) {
            return Err(anyhow!(
                "share {shared_tree_id} is already being advertised"
            ));
        }

        let alpn = make_alpn(ALPN_PREFIX, &shared_tree_id);
        let endpoint = match &self.secret_key {
            Some(key) => create_endpoint_with_key(vec![alpn.clone()], key.clone())
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
        ));

        guard.insert(shared_tree_id, ShareHandle { endpoint, task });
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
) {
    debug!("[advertiser:{shared_tree_id}] accept loop started");
    while let Some(incoming) = endpoint.accept().await {
        let doc = doc.clone();
        let id = shared_tree_id.clone();
        let cb = on_peer_connected.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    warn!("[advertiser:{id}] handshake failed: {e}");
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
    use super::*;
    use crate::api::loro_backend::TREE_NAME;
    use crate::sync::iroh_sync_adapter::sync_doc_initiate;
    use loro::{ExportMode, LoroText};

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
}
