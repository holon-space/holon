use rmcp::handler::client::ClientHandler;
use rmcp::model::{ClientInfo, Implementation, ResourceUpdatedNotificationParam};
use rmcp::service::{NotificationContext, RoleClient};
use tokio::sync::mpsc;
use tracing::debug;

/// A channel-based ClientHandler that forwards resource update notifications.
///
/// Implements `ClientHandler` to intercept `notifications/resources/updated` from the
/// MCP server and forward the URI through a channel. All other notifications use defaults.
pub struct NotifyingClientHandler {
    client_info: ClientInfo,
    sender: mpsc::UnboundedSender<String>,
}

/// Receiver end of the resource update notification channel.
pub struct ResourceUpdateReceiver(pub mpsc::UnboundedReceiver<String>);

impl NotifyingClientHandler {
    /// Create a new handler and its paired receiver.
    ///
    /// The handler should be passed to the MCP connection function.
    /// The receiver should be given to the sync engine for processing updates.
    pub fn new() -> (Self, ResourceUpdateReceiver) {
        let (sender, receiver) = mpsc::unbounded_channel();
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
            Self {
                client_info,
                sender,
            },
            ResourceUpdateReceiver(receiver),
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
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        debug!("[NotifyingClientHandler] Resource updated: {}", params.uri);
        let _ = self.sender.send(params.uri);
        std::future::ready(())
    }
}

use std::future::Future;
