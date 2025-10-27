//! BrowserRelay: forwards MCP tool calls to the Dioxus browser page via WebSocket hub.
//!
//! Architecture:
//!   Claude Code → HTTP /mcp → serve.mjs (proxy) → this relay (RELAY_PORT)
//!     → serve.mjs WebSocket hub (/mcp-hub?role=native)
//!       ↔ browser page (role=browser) → engineMcpTool → holon-worker WASM
//!
//! Wire protocol:
//!   native→browser: {"id": "uuid", "tool": "...", "arguments": {...}}
//!   browser→native: {"id": "uuid", "content": "[{\"type\":\"text\",\"text\":\"...\"}]"}
//!               or: {"id": "uuid", "is_error": true, "content": "..."}
//!
//! The relay connects to the hub as role=native and reconnects automatically on
//! disconnect (handles trunk --watch restarts without dropping in-flight MCP sessions).

use std::{
    collections::HashMap,
    sync::Arc,
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, Content, ListToolsResult, PaginatedRequestParam,
        ServerInfo,
    },
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler,
};
use tokio::sync::{oneshot, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

use crate::server::HolonMcpServer;

type ResultSender = oneshot::Sender<Result<CallToolResult, McpError>>;

pub struct BrowserRelay {
    hub_url: String,
    pending: Arc<Mutex<HashMap<String, ResultSender>>>,
    /// Write half of the active WS connection. None when disconnected.
    ws_tx: Arc<Mutex<Option<futures::channel::mpsc::UnboundedSender<Message>>>>,
}

impl BrowserRelay {
    /// Connect to the hub and spawn a reconnect loop. Returns immediately once
    /// the background task is running (first connection attempt is async).
    pub fn start(hub_url: String) -> Arc<Self> {
        let relay = Arc::new(Self {
            hub_url,
            pending: Arc::new(Mutex::new(HashMap::new())),
            ws_tx: Arc::new(Mutex::new(None)),
        });
        let relay_clone = relay.clone();
        tokio::spawn(async move {
            relay_clone.connection_loop().await;
        });
        relay
    }

    async fn connection_loop(&self) {
        loop {
            match self.try_connect().await {
                Ok(()) => tracing::debug!("[mcp-relay] hub connection closed, reconnecting…"),
                Err(e) => {
                    tracing::warn!("[mcp-relay] connect failed: {e} — retrying in 1s");
                }
            }
            // Drain pending requests with a disconnection error.
            {
                let mut pending = self.pending.lock().await;
                for (_, tx) in pending.drain() {
                    let _ = tx.send(Err(McpError::internal_error(
                        "browser relay disconnected",
                        None,
                    )));
                }
            }
            *self.ws_tx.lock().await = None;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn try_connect(&self) -> Result<(), anyhow::Error> {
        let connect_url = if self.hub_url.contains('?') {
            format!("{}&role=native", self.hub_url)
        } else {
            format!("{}?role=native", self.hub_url)
        };
        let (ws_stream, _) = connect_async(&connect_url).await?;
        tracing::info!("[mcp-relay] connected to hub as native: {}", connect_url);

        let (mut sink, mut stream) = ws_stream.split();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Message>();

        // Store the write half so forward() can send messages.
        *self.ws_tx.lock().await = Some(tx);

        // Pump outgoing messages from the channel to the WS sink.
        let send_task = tokio::spawn(async move {
            while let Some(msg) = rx.next().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Process incoming messages (tool responses from browser).
        let pending = self.pending.clone();
        while let Some(msg) = stream.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("[mcp-relay] stream error: {e}");
                    break;
                }
            };
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            dispatch_response(pending.clone(), &text).await;
        }

        send_task.abort();
        Ok(())
    }

    /// Send a tool call to the browser and wait for the response.
    pub async fn forward(&self, req: CallToolRequestParam) -> Result<CallToolResult, McpError> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), tx);
        }

        let arguments: serde_json::Value = req
            .arguments
            .map(|a| serde_json::Value::Object(a.into_iter().collect()))
            .unwrap_or(serde_json::json!({}));

        let msg = serde_json::json!({
            "id": id,
            "tool": req.name,
            "arguments": arguments,
        });
        let json = serde_json::to_string(&msg)
            .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;

        {
            let ws_tx_guard = self.ws_tx.lock().await;
            match ws_tx_guard.as_ref() {
                Some(tx) => {
                    if tx.unbounded_send(Message::Text(json.into())).is_err() {
                        let mut pending = self.pending.lock().await;
                        pending.remove(&id);
                        return Err(McpError::internal_error("relay not connected", None));
                    }
                }
                None => {
                    let mut pending = self.pending.lock().await;
                    pending.remove(&id);
                    return Err(McpError::internal_error("relay not connected", None));
                }
            }
        }

        rx.await
            .map_err(|_| McpError::internal_error("relay sender dropped", None))?
    }
}

async fn dispatch_response(pending: Arc<Mutex<HashMap<String, ResultSender>>>, text: &str) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[mcp-relay] unparseable response: {e} — {text}");
            return;
        }
    };
    let id = match msg.get("id").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            tracing::warn!("[mcp-relay] response missing id: {text}");
            return;
        }
    };
    let tx = {
        let mut pending = pending.lock().await;
        pending.remove(&id)
    };
    let tx = match tx {
        Some(t) => t,
        None => {
            tracing::warn!("[mcp-relay] response for unknown id={id}");
            return;
        }
    };

    let is_error = msg
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let content_str = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("[]");

    // content_str is a JSON-encoded Vec<Content>: "[{\"type\":\"text\",\"text\":\"...\"}]"
    let content: Vec<Content> = serde_json::from_str(content_str).unwrap_or_else(|_| {
        vec![Content::text(content_str.to_string())]
    });

    let result = if is_error {
        Ok(CallToolResult::error(content))
    } else {
        Ok(CallToolResult::success(content))
    };
    let _ = tx.send(result);
}

/// MCP `ServerHandler` that lists schemas from the Rust tool definitions but
/// forwards all `call_tool` requests to the browser engine via WebSocket hub.
pub struct BrowserRelayServer {
    relay: Arc<BrowserRelay>,
}

impl BrowserRelayServer {
    pub fn new(relay: Arc<BrowserRelay>) -> Self {
        Self { relay }
    }
}

impl ServerHandler for BrowserRelayServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = (HolonMcpServer::tool_router_ui() + HolonMcpServer::tool_router_backend())
            .list_all();
        async move { Ok(ListToolsResult::with_all_items(tools)) }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let relay = self.relay.clone();
        async move { relay.forward(request).await }
    }
}
