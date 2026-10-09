//! Requests to an MCP peer, each bounded by [`REQUEST_TIMEOUT`].
//!
//! A peer that answers slowly but never stops sending keeps every idle
//! timeout satisfied, so only a deadline per request ends the wait.

use std::future::Future;

use rmcp::RoleClient;
use rmcp::model::CancelledNotificationParam;
use rmcp::model::ClientRequest;
use rmcp::model::ListResourceTemplatesRequest;
use rmcp::model::ListToolsRequest;
use rmcp::model::PaginatedRequestParam;
use rmcp::model::ResourceTemplate;
use rmcp::model::ServerResult;
use rmcp::model::SubscribeRequest;
use rmcp::model::SubscribeRequestParam;
use rmcp::model::Tool;
use rmcp::service::Peer;
use rmcp::service::PeerRequestOptions;
use rmcp::service::ServiceError;
use tracing::warn;

use crate::secure_client::REQUEST_TIMEOUT;

/// Send `request` and wait at most [`REQUEST_TIMEOUT`] for its answer; past
/// that the peer is told to stop and the error names `what`.
pub(crate) async fn request(
    peer: &Peer<RoleClient>,
    what: &str,
    request: ClientRequest,
) -> Result<ServerResult, ServiceError> {
    let handle = peer
        .send_request_with_option(request, PeerRequestOptions::no_options())
        .await?;
    let request_id = handle.id.clone();
    match tokio::time::timeout(REQUEST_TIMEOUT, handle.await_response()).await {
        Ok(answer) => answer,
        Err(_) => {
            let reason =
                format!("{what} got no response within REQUEST_TIMEOUT ({REQUEST_TIMEOUT:?})");
            // Spawned: telling the peer goes through the same transport that
            // is not answering.
            let peer = peer.clone();
            let notice = CancelledNotificationParam {
                request_id,
                reason: Some(reason.clone()),
            };
            tokio::spawn(async move {
                if let Err(e) = peer.notify_cancelled(notice).await {
                    warn!("[mcp_request] the peer was not told to cancel a timed-out request: {e}");
                }
            });
            Err(ServiceError::Cancelled {
                reason: Some(reason),
            })
        }
    }
}

/// `resources/subscribe`, bounded by [`REQUEST_TIMEOUT`].
pub(crate) async fn subscribe(peer: &Peer<RoleClient>, uri: &str) -> Result<(), ServiceError> {
    let what = format!("subscribe '{uri}'");
    let request = ClientRequest::SubscribeRequest(SubscribeRequest {
        method: Default::default(),
        params: SubscribeRequestParam {
            uri: uri.to_string(),
        },
        extensions: Default::default(),
    });
    match self::request(peer, &what, request).await? {
        ServerResult::EmptyResult(_) => Ok(()),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}

/// Every tool the peer lists, each page bounded by [`REQUEST_TIMEOUT`].
pub(crate) async fn list_all_tools(peer: &Peer<RoleClient>) -> Result<Vec<Tool>, ServiceError> {
    all_pages(|cursor| async move {
        let request = ClientRequest::ListToolsRequest(ListToolsRequest {
            method: Default::default(),
            params: Some(PaginatedRequestParam { cursor }),
            extensions: Default::default(),
        });
        match self::request(peer, "list_tools", request).await? {
            ServerResult::ListToolsResult(page) => Ok((page.tools, page.next_cursor)),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    })
    .await
}

/// Every resource template the peer lists, each page bounded by
/// [`REQUEST_TIMEOUT`].
pub(crate) async fn list_all_resource_templates(
    peer: &Peer<RoleClient>,
) -> Result<Vec<ResourceTemplate>, ServiceError> {
    all_pages(|cursor| async move {
        let request = ClientRequest::ListResourceTemplatesRequest(ListResourceTemplatesRequest {
            method: Default::default(),
            params: Some(PaginatedRequestParam { cursor }),
            extensions: Default::default(),
        });
        match self::request(peer, "list_resource_templates", request).await? {
            ServerResult::ListResourceTemplatesResult(page) => {
                Ok((page.resource_templates, page.next_cursor))
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    })
    .await
}

/// The `initialize` handshake a `serve` call runs, bounded by
/// [`REQUEST_TIMEOUT`].
pub(crate) async fn handshake<S, E>(serve: impl Future<Output = Result<S, E>>) -> anyhow::Result<S>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match tokio::time::timeout(REQUEST_TIMEOUT, serve).await {
        Ok(served) => Ok(served?),
        Err(_) => anyhow::bail!(
            "the MCP initialize handshake got no response within REQUEST_TIMEOUT \
             ({REQUEST_TIMEOUT:?})"
        ),
    }
}

async fn all_pages<T, F, Fut>(mut page: F) -> Result<Vec<T>, ServiceError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<String>), ServiceError>>,
{
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let (items, next) = page(cursor).await?;
        all.extend(items);
        cursor = next;
        if cursor.is_none() {
            return Ok(all);
        }
    }
}

#[cfg(test)]
mod tests {
    use rmcp::ServiceExt as _;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_never_answers_initialize_is_cut_at_the_request_timeout() {
        let (client_io, _silent_peer) = tokio::io::duplex(1 << 16);
        let outcome = tokio::time::timeout(REQUEST_TIMEOUT * 2, handshake(().serve(client_io)))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "the handshake was still open after {:?}",
                    REQUEST_TIMEOUT * 2
                )
            });
        let err = format!(
            "{:#}",
            outcome.err().expect("no handshake without an answer")
        );
        assert!(
            err.contains("REQUEST_TIMEOUT") && err.contains("initialize"),
            "the error must name the deadline and the handshake; got: {err}"
        );
    }
}
