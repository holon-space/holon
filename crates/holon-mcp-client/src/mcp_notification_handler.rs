//! The inbound leg of a connection: the `resources/updated` notices a peer
//! sends unasked, and the bounds they are parked under.
//!
//! Nothing drains this path until the boot org scan opens the sync gate, so
//! what a peer pushes in the meantime is held — which makes both the number of
//! notices and the length of each URI a peer-chosen amount of memory. The
//! handler is therefore built from the connection's [`PeerBudget`], and
//! refusing a notice NEVER drops the signal: it widens it into one full
//! re-sync through [`InboundNotices`].

use std::future::Future;
use std::sync::Arc;

use rmcp::handler::client::ClientHandler;
use rmcp::model::ClientInfo;
use rmcp::model::Implementation;
use rmcp::model::ResourceUpdatedNotificationParam;
use rmcp::service::NotificationContext;
use rmcp::service::RoleClient;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

use crate::peer_budget::InboundNotices;
use crate::peer_budget::PeerBudget;
use crate::secure_client::MAX_NOTIFICATION_URI_BYTES;
use crate::secure_client::MAX_PENDING_SYNC_URIS;

/// A `ClientHandler` that forwards `notifications/resources/updated` to its
/// connection's sync loop, under that connection's inbound bounds.
pub struct NotifyingClientHandler {
    client_info: ClientInfo,
    /// Bounded by [`MAX_PENDING_SYNC_URIS`]: this queue is the only place an
    /// un-drained notice is held, so its capacity is the bound.
    sender: mpsc::Sender<String>,
    notices: Arc<InboundNotices>,
}

/// Receiver end of the resource update notification channel, plus the signal
/// that says a bound replaced its per-URI re-syncs with a full one.
pub struct ResourceUpdateReceiver {
    /// The notices that fit the bound, in arrival order.
    pub uris: mpsc::Receiver<String>,
    /// Where a bound that refused a notice announced itself.
    pub notices: Arc<InboundNotices>,
}

impl PeerBudget {
    /// The client handler for this connection and the receiving end its sync
    /// loop reads.
    ///
    /// A method on the budget because the queue's capacity and the collapse
    /// signal are the budget's inbound half; a handler built without one would
    /// park whatever its peer sends.
    pub fn notifying_handler(self: &Arc<Self>) -> (NotifyingClientHandler, ResourceUpdateReceiver) {
        let (sender, uris) = mpsc::channel(MAX_PENDING_SYNC_URIS);
        let client_info = ClientInfo {
            protocol_version: Default::default(),
            capabilities: Default::default(),
            client_info: Implementation {
                name: "holon-mcp-client".into(),
                title: None,
                version: env!("CARGO_PKG_VERSION").into(),
                icons: None,
                website_url: None,
            },
        };
        (
            NotifyingClientHandler {
                client_info,
                sender,
                notices: self.inbound.clone(),
            },
            ResourceUpdateReceiver {
                uris,
                notices: self.inbound.clone(),
            },
        )
    }
}

impl ClientHandler for NotifyingClientHandler {
    fn get_info(&self) -> ClientInfo {
        self.client_info.clone()
    }

    fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        let uri = params.uri;
        if uri.len() > MAX_NOTIFICATION_URI_BYTES {
            self.notices.collapse(format!(
                "a resource-updated notice carried a {}-byte URI, past \
                 MAX_NOTIFICATION_URI_BYTES ({MAX_NOTIFICATION_URI_BYTES})",
                uri.len()
            ));
            return std::future::ready(());
        }
        debug!("[NotifyingClientHandler] Resource updated: {uri}");
        match self.sender.try_send(uri) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => self.notices.collapse(format!(
                "MAX_PENDING_SYNC_URIS ({MAX_PENDING_SYNC_URIS}) resource URIs of this \
                 connection are already waiting for a re-sync"
            )),
            Err(mpsc::error::TrySendError::Closed(_)) => warn!(
                "[NotifyingClientHandler] this connection's sync loop has stopped; its \
                 resource-updated notices change nothing"
            ),
        }
        std::future::ready(())
    }
}
