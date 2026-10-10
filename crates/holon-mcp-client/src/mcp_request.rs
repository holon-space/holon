//! The only handle onto an MCP peer, and the bounds every call through it
//! carries.
//!
//! A peer that answers slowly but never stops sending keeps every idle
//! timeout satisfied, so only a deadline per request ends the wait. A peer that
//! answers every page promptly keeps every per-request deadline satisfied, so
//! only a page count, an item count and a total deadline end the enumeration.
//!
//! [`BudgetedPeer`] holds the rmcp peer in a private field and this module is
//! the only one that touches it, so a new call path cannot reach a peer without
//! going through the bounds — the type is what enforces that, not a convention.

use std::future::Future;
use std::sync::Arc;

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

use crate::peer_budget::PeerBudget;
use crate::secure_client::CONNECT_BUDGET;
use crate::secure_client::MAX_LIST_ITEMS;
use crate::secure_client::MAX_LIST_PAGES;
use crate::secure_client::REQUEST_TIMEOUT;

/// A connected MCP peer, together with the budget every call to it is charged
/// to.
///
/// The inner peer is private on purpose: rmcp's own `Peer` methods carry no
/// deadline and no budget, so handing one out would reopen every hole this
/// module closes.
#[derive(Clone, Debug)]
pub struct BudgetedPeer {
    peer: Peer<RoleClient>,
    budget: Arc<PeerBudget>,
}

impl BudgetedPeer {
    /// Wrap a freshly served peer in the budget that was armed before its
    /// handshake ([`PeerBudget::new`]).
    pub fn new(peer: Peer<RoleClient>, budget: Arc<PeerBudget>) -> Self {
        Self { peer, budget }
    }

    /// The budget this peer's calls are charged to, so the integration built
    /// around it can follow the bounds that trip.
    pub(crate) fn budget(&self) -> &Arc<PeerBudget> {
        &self.budget
    }

    /// What the peer declared about itself at `initialize`. Local state, so no
    /// bound applies.
    pub fn peer_info(&self) -> Option<&rmcp::model::InitializeResult> {
        self.peer.peer_info()
    }

    /// Send `request` and wait at most [`REQUEST_TIMEOUT`] for its answer; past
    /// that the peer is told to stop and the error names `what`.
    pub(crate) async fn request(
        &self,
        what: &str,
        request: ClientRequest,
    ) -> Result<ServerResult, ServiceError> {
        let handle = self
            .peer
            .send_request_with_option(request, PeerRequestOptions::no_options())
            .await?;
        let request_id = handle.id.clone();
        match tokio::time::timeout(REQUEST_TIMEOUT, handle.await_response()).await {
            Ok(answer) => answer,
            Err(_) => {
                let reason =
                    format!("{what} got no response within REQUEST_TIMEOUT ({REQUEST_TIMEOUT:?})");
                // Spawned: telling the peer goes through the same transport
                // that is not answering.
                let peer = self.peer.clone();
                let notice = CancelledNotificationParam {
                    request_id,
                    reason: Some(reason.clone()),
                };
                tokio::spawn(async move {
                    if let Err(e) = peer.notify_cancelled(notice).await {
                        warn!(
                            "[mcp_request] the peer was not told to cancel a timed-out request: {e}"
                        );
                    }
                });
                Err(ServiceError::Cancelled {
                    reason: Some(reason),
                })
            }
        }
    }

    /// `resources/subscribe`, bounded by [`REQUEST_TIMEOUT`].
    pub(crate) async fn subscribe(&self, uri: &str) -> Result<(), ServiceError> {
        let what = format!("subscribe '{uri}'");
        let request = ClientRequest::SubscribeRequest(SubscribeRequest {
            method: Default::default(),
            params: SubscribeRequestParam {
                uri: uri.to_string(),
            },
            extensions: Default::default(),
        });
        match self.request(&what, request).await? {
            ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    /// Every tool the peer lists, bounded by the enumeration budget.
    pub(crate) async fn list_all_tools(&self) -> Result<Vec<Tool>, ServiceError> {
        self.all_pages("list_tools", |cursor| async move {
            let request = ClientRequest::ListToolsRequest(ListToolsRequest {
                method: Default::default(),
                params: Some(PaginatedRequestParam { cursor }),
                extensions: Default::default(),
            });
            match self.request("list_tools", request).await? {
                ServerResult::ListToolsResult(page) => Ok((page.tools, page.next_cursor)),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        })
        .await
    }

    /// Every resource template the peer lists, bounded by the enumeration
    /// budget.
    pub(crate) async fn list_all_resource_templates(
        &self,
    ) -> Result<Vec<ResourceTemplate>, ServiceError> {
        self.all_pages("list_resource_templates", |cursor| async move {
            let request =
                ClientRequest::ListResourceTemplatesRequest(ListResourceTemplatesRequest {
                    method: Default::default(),
                    params: Some(PaginatedRequestParam { cursor }),
                    extensions: Default::default(),
                });
            match self.request("list_resource_templates", request).await? {
                ServerResult::ListResourceTemplatesResult(page) => {
                    Ok((page.resource_templates, page.next_cursor))
                }
                _ => Err(ServiceError::UnexpectedResponse),
            }
        })
        .await
    }

    /// Walk `page` until the peer stops handing out cursors, bounded by
    /// [`MAX_LIST_PAGES`], [`MAX_LIST_ITEMS`] and the connect deadline.
    ///
    /// All three are needed, because each catches a shape the others do not: a
    /// peer sending full pages forever hits the item bound, one sending empty
    /// pages forever hits the page bound, and one sending slow pages forever
    /// hits the deadline.
    async fn all_pages<T, F, Fut>(&self, what: &str, mut page: F) -> Result<Vec<T>, ServiceError>
    where
        F: FnMut(Option<String>) -> Fut,
        Fut: Future<Output = Result<(Vec<T>, Option<String>), ServiceError>>,
    {
        let deadline = self.budget.connect_deadline;
        // A connect runs several enumerations against this one budget and the
        // PEER owns how long each takes, so arriving here with it already spent
        // is a state a peer can drive — a refusal naming the bound, never an
        // invariant.
        if tokio::time::Instant::now() >= deadline {
            return Err(ServiceError::Cancelled {
                reason: Some(format!(
                    "{what} could not start: this connection's CONNECT_BUDGET ({CONNECT_BUDGET:?}) \
                     was already spent by the handshake and the enumerations before it"
                )),
            });
        }
        let mut all = Vec::new();
        let mut cursor = None;
        for fetched in 1..=MAX_LIST_PAGES {
            let (items, next) = match tokio::time::timeout_at(deadline, page(cursor)).await {
                Ok(page) => page?,
                Err(_) => {
                    return Err(ServiceError::Cancelled {
                        reason: Some(format!(
                            "{what} was still paginating when this connection's CONNECT_BUDGET \
                             ({CONNECT_BUDGET:?}) ran out, after {fetched} pages and {} items",
                            all.len()
                        )),
                    });
                }
            };
            all.extend(items);
            if all.len() > MAX_LIST_ITEMS {
                return Err(ServiceError::Cancelled {
                    reason: Some(format!(
                        "{what} listed more than MAX_LIST_ITEMS ({MAX_LIST_ITEMS}) items \
                         ({} after {fetched} pages) and the peer offered another page",
                        all.len()
                    )),
                });
            }
            cursor = next;
            if cursor.is_none() {
                return Ok(all);
            }
        }
        Err(ServiceError::Cancelled {
            reason: Some(format!(
                "{what} offered another page after MAX_LIST_PAGES ({MAX_LIST_PAGES}) pages and {} \
                 items",
                all.len()
            )),
        })
    }
}

impl PeerBudget {
    /// The `initialize` handshake a `serve` call runs, bounded by
    /// [`REQUEST_TIMEOUT`] and by this budget's share of [`CONNECT_BUDGET`].
    ///
    /// A method on the budget rather than a free function so that the connect
    /// deadline is armed before the handshake it covers.
    pub async fn handshake<S, E>(
        &self,
        serve: impl Future<Output = Result<S, E>>,
    ) -> anyhow::Result<S>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        let deadline = (tokio::time::Instant::now() + REQUEST_TIMEOUT).min(self.connect_deadline);
        match tokio::time::timeout_at(deadline, serve).await {
            Ok(served) => Ok(served?),
            Err(_) => anyhow::bail!(
                "the MCP initialize handshake got no response within REQUEST_TIMEOUT \
                 ({REQUEST_TIMEOUT:?}) / CONNECT_BUDGET ({CONNECT_BUDGET:?})"
            ),
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
        let budget = PeerBudget::new();
        let outcome =
            tokio::time::timeout(REQUEST_TIMEOUT * 2, budget.handshake(().serve(client_io)))
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
