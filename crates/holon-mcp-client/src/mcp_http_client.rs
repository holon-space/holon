//! The HTTP client of an MCP-over-HTTP connection.
//!
//! The requests mirror rmcp's own `reqwest` client; the bodies are read
//! through [`read_capped`] and [`EventBytes`].

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt as _;
use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::WWW_AUTHENTICATE;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::common::http_header::EVENT_STREAM_MIME_TYPE;
use rmcp::transport::common::http_header::HEADER_LAST_EVENT_ID;
use rmcp::transport::common::http_header::HEADER_SESSION_ID;
use rmcp::transport::common::http_header::JSON_MIME_TYPE;
use rmcp::transport::streamable_http_client::AuthRequiredError;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp::transport::streamable_http_client::StreamableHttpError;
use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;

use crate::peer_budget::HeldEventBytes;
use crate::peer_budget::PeerBudget;
use crate::secure_client::MAX_CONCURRENT_GET_STREAMS;
use crate::secure_client::MAX_CONCURRENT_POST_STREAMS;
use crate::secure_client::MAX_RESPONSE_BODY_BYTES;
use crate::secure_client::MCP_IDLE_TIMEOUT;
use crate::secure_client::REQUEST_TIMEOUT;
use crate::secure_client::cause_chain;
use crate::secure_client::read_capped;

type HttpError = StreamableHttpError<McpHttpError>;

/// A transport failure, with every URL stripped.
#[derive(Debug)]
pub(crate) struct McpHttpError(String);

impl std::fmt::Display for McpHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for McpHttpError {}

/// The secure client's redirect policy, [`MCP_IDLE_TIMEOUT`] on every read,
/// [`REQUEST_TIMEOUT`] on every POST, and every JSON-RPC message capped at
/// [`MAX_RESPONSE_BODY_BYTES`].
///
/// A GET stream gets only the idle timeout because a connection keeps one open
/// for its whole life; rmcp reopens a stream the idle timeout cut. A POST gets
/// a total one because rmcp sends a connection's messages one POST at a time,
/// so one reply that never ends stalls every later request.
///
/// Both kinds of stream are bounded in number, and the partial events of all
/// of them share one byte allowance: rmcp keeps polling the reply stream of a
/// request whose deadline already passed and reopens a notification stream
/// without a ceiling, so the peer would otherwise decide how many partial
/// events the process holds. All of that lives in the connection's
/// [`PeerBudget`], which this client shares with its peer handle.
#[derive(Clone)]
pub(crate) struct McpHttpClient {
    client: reqwest::Client,
    idle_timeout: Duration,
    request_timeout: Duration,
    /// Shared by every clone rmcp makes of this client AND by the connection's
    /// [`crate::mcp_request::BudgetedPeer`], so the bounds are per connection.
    budget: Arc<PeerBudget>,
}

impl McpHttpClient {
    pub(crate) fn new(budget: Arc<PeerBudget>) -> Self {
        Self::with_budget(MCP_IDLE_TIMEOUT, REQUEST_TIMEOUT, budget)
    }

    fn with_budget(
        idle_timeout: Duration,
        request_timeout: Duration,
        budget: Arc<PeerBudget>,
    ) -> Self {
        Self {
            client: crate::secure_client::https_only()
                .read_timeout(idle_timeout)
                .build()
                .expect("a reqwest client with a redirect policy and a read timeout must build"),
            idle_timeout,
            request_timeout,
            budget,
        }
    }

    fn describe(&self) -> impl Fn(reqwest::Error) -> String + Send + Sync + 'static {
        let idle = self.idle_timeout;
        move |e| {
            if e.is_timeout() {
                format!(
                    "no byte from the peer within MCP_IDLE_TIMEOUT ({idle:?}): {}",
                    cause_chain(e)
                )
            } else {
                cause_chain(e)
            }
        }
    }

    fn client_error(&self) -> impl Fn(reqwest::Error) -> HttpError {
        let describe = self.describe();
        move |e| StreamableHttpError::Client(McpHttpError(describe(e)))
    }

    /// One slot of a stream budget, waited for rather than refused: Holon's own
    /// enumeration fan-out calls a connection concurrently and rmcp replaces a
    /// notification stream while the old one is still closing, so a full budget
    /// is contention and only a peer that holds every stream open past the
    /// deadline turns it into a failure.
    ///
    /// Waiting is also what rate-limits a peer-driven reconnect storm: rmcp
    /// retries a notification stream without a ceiling, and each retry then
    /// waits for a slot rather than opening a socket.
    async fn stream_slot(
        &self,
        which: &str,
        budget: &Arc<tokio::sync::Semaphore>,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, HttpError> {
        let timeout = self.request_timeout;
        match tokio::time::timeout(timeout, budget.clone().acquire_owned()).await {
            Ok(slot) => Ok(slot.expect("a stream budget is never closed")),
            Err(_) => Err(StreamableHttpError::Client(McpHttpError(format!(
                "the peer held all {which} streams of this connection open for REQUEST_TIMEOUT \
                 ({timeout:?})"
            )))),
        }
    }

    async fn post_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, HttpError> {
        self.stream_slot(
            &format!("MAX_CONCURRENT_POST_STREAMS ({MAX_CONCURRENT_POST_STREAMS}) reply"),
            &self.budget.post_streams,
        )
        .await
    }

    async fn get_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, HttpError> {
        self.stream_slot(
            &format!("MAX_CONCURRENT_GET_STREAMS ({MAX_CONCURRENT_GET_STREAMS}) notification"),
            &self.budget.get_streams,
        )
        .await
    }

    async fn post(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        slot: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        let mut request = self
            .client
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(CONTENT_TYPE, JSON_MIME_TYPE)
            .body(serde_json::to_vec(&message)?);
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.send().await.map_err(self.client_error())?;
        if response.status() == StatusCode::UNAUTHORIZED
            && let Some(header) = response.headers().get(WWW_AUTHENTICATE)
        {
            let www_authenticate_header = header
                .to_str()
                .map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(Cow::from(
                        "invalid www-authenticate header value",
                    ))
                })?
                .to_string();
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                www_authenticate_header,
            }));
        }
        let status = response.status();
        let response = response.error_for_status().map_err(self.client_error())?;
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .map(|v| {
                v.to_str().map(str::to_string).map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(Cow::from(
                        "the Mcp-Session-Id header is not visible ASCII",
                    ))
                })
            })
            .transpose()?;
        match content_type(&response) {
            Some(ct) if ct.starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                Ok(StreamableHttpPostResponse::Sse(
                    BoundedPostStream::new(
                        capped_events(
                            response,
                            self.describe(),
                            self.budget.held_event_bytes.clone(),
                        ),
                        self.request_timeout,
                        slot,
                    )
                    .boxed(),
                    session_id,
                ))
            }
            Some(ct) if ct.starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let cap = self.budget.listed_bytes.body_cap();
                let body = read_capped(response, self.describe(), cap)
                    .await
                    .map_err(|e| {
                        StreamableHttpError::UnexpectedServerResponse(Cow::Owned(format!("{e:#}")))
                    })?;
                Ok(StreamableHttpPostResponse::Json(
                    serde_json::from_slice(&body)?,
                    session_id,
                ))
            }
            other => Err(StreamableHttpError::UnexpectedContentType(
                other.map(|ct| String::from_utf8_lossy(ct).into_owned()),
            )),
        }
    }
}

impl StreamableHttpClient for McpHttpClient {
    type Error = McpHttpError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        let slot = self.post_slot().await?;
        let timeout = self.request_timeout;
        tokio::time::timeout(
            timeout,
            self.post(uri, message, session_id, auth_token, slot),
        )
        .await
        .unwrap_or_else(|_| {
            Err(StreamableHttpError::Client(McpHttpError(format!(
                "no complete reply to a POST within REQUEST_TIMEOUT ({timeout:?})"
            ))))
        })
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
    ) -> Result<(), HttpError> {
        let mut request = self
            .client
            .delete(uri.as_ref())
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(self.client_error())?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        response.error_for_status().map_err(self.client_error())?;
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> Result<BoxedSseResponse, HttpError> {
        let slot = self.get_slot().await?;
        let mut request = self
            .client
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(last_event_id) = last_event_id {
            request = request.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(self.client_error())?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response.error_for_status().map_err(self.client_error())?;
        match content_type(&response) {
            Some(ct)
                if ct.starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    || ct.starts_with(JSON_MIME_TYPE.as_bytes()) =>
            {
                Ok(SlotHolding {
                    inner: capped_events(
                        response,
                        self.describe(),
                        self.budget.held_event_bytes.clone(),
                    ),
                    _slot: slot,
                }
                .boxed())
            }
            other => Err(StreamableHttpError::UnexpectedContentType(
                other.map(|ct| String::from_utf8_lossy(ct).into_owned()),
            )),
        }
    }
}

fn content_type(response: &reqwest::Response) -> Option<&[u8]> {
    response.headers().get(CONTENT_TYPE).map(|ct| ct.as_bytes())
}

fn capped_events(
    response: reqwest::Response,
    describe: impl Fn(reqwest::Error) -> String + Send + 'static,
    held: Arc<HeldEventBytes>,
) -> BoxedSseResponse {
    capped_sse(response.bytes_stream(), describe, held)
}

fn capped_sse(
    bytes: impl futures::Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
    describe: impl Fn(reqwest::Error) -> String + Send + 'static,
    held: Arc<HeldEventBytes>,
) -> BoxedSseResponse {
    let mut event = EventBytes::new(held);
    let bytes = bytes.map(move |chunk| {
        let chunk = chunk.map_err(|e| std::io::Error::other(describe(e)))?;
        event.count(&chunk)?;
        Ok::<_, std::io::Error>(chunk)
    });
    sse_stream::SseStream::from_bytes_stream(bytes).boxed()
}

/// One unfinished POST reply stream: it holds a slot of its connection's
/// budget and ends when `timeout` has passed.
///
/// It ends instead of raising, because rmcp reconnects a POST reply stream that
/// errors and the request this stream answers already failed at its own
/// deadline. The warning is what discloses the close.
struct BoundedPostStream {
    /// Dropped the moment the stream finishes, which frees the socket and the
    /// partial event held in it however long rmcp keeps the stream itself.
    inner: Option<BoxedSseResponse>,
    deadline: std::pin::Pin<Box<tokio::time::Sleep>>,
    timeout: Duration,
    slot: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl BoundedPostStream {
    fn new(
        inner: BoxedSseResponse,
        timeout: Duration,
        slot: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            inner: Some(inner),
            deadline: Box::pin(tokio::time::sleep(timeout)),
            timeout,
            slot: Some(slot),
        }
    }
}

impl futures::Stream for BoundedPostStream {
    type Item = <BoxedSseResponse as futures::Stream>::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        let me = &mut *self;
        if me.inner.is_none() {
            return Poll::Ready(None);
        }
        if std::future::Future::poll(me.deadline.as_mut(), cx).is_ready() {
            tracing::warn!(
                "[mcp_http_client] closing a POST reply stream the peer left unfinished for \
                 REQUEST_TIMEOUT ({:?})",
                me.timeout
            );
            me.inner = None;
            me.slot = None;
            return Poll::Ready(None);
        }
        let inner = me.inner.as_mut().expect("checked just above");
        let polled = futures::Stream::poll_next(inner.as_mut(), cx);
        if matches!(polled, Poll::Ready(None)) {
            me.inner = None;
            me.slot = None;
        }
        polled
    }
}

/// A stream that holds a slot of its connection's budget for as long as it
/// lives. The notification stream's lifetime IS the connection's, so unlike a
/// POST reply it carries no deadline of its own — only a slot.
struct SlotHolding {
    inner: BoxedSseResponse,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

impl futures::Stream for SlotHolding {
    type Item = <BoxedSseResponse as futures::Stream>::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        futures::Stream::poll_next(self.inner.as_mut(), cx)
    }
}

/// The size of the SSE event being received, charged to the allowance its
/// connection's streams share. A blank line ends an event, and a line ends at
/// `\n`, `\r` or `\r\n`.
struct EventBytes {
    len: usize,
    line_is_empty: bool,
    after_cr: bool,
    /// Set once the shared allowance refused a chunk. Without it a stream whose
    /// consumer ignores the error would keep charging.
    refused: bool,
    held: Arc<HeldEventBytes>,
}

impl EventBytes {
    fn new(held: Arc<HeldEventBytes>) -> Self {
        Self {
            len: 0,
            line_is_empty: false,
            after_cr: false,
            refused: false,
            held,
        }
    }

    fn count(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        if self.refused {
            return Err(std::io::Error::other(format!(
                "this MCP stream was already stopped at MAX_RESPONSE_BODY_BYTES \
                 ({MAX_RESPONSE_BODY_BYTES} bytes)"
            )));
        }
        let before = self.len;
        for &b in chunk {
            let lf_of_crlf = self.after_cr && b == b'\n';
            self.after_cr = b == b'\r';
            if lf_of_crlf {
                continue;
            }
            if b == b'\r' || b == b'\n' {
                if self.line_is_empty {
                    self.len = 0;
                }
                self.line_is_empty = true;
                continue;
            }
            self.line_is_empty = false;
            self.len += 1;
        }
        // One charge per chunk rather than per byte: a chunk is a socket read,
        // so the overshoot before the check is at most one of those.
        if let Err(e) = self.held.recharge(before, self.len) {
            self.len = before;
            self.refused = true;
            return Err(e);
        }
        Ok(())
    }
}

impl Drop for EventBytes {
    fn drop(&mut self) {
        self.held.release(self.len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer_budget::ListedBytes;

    /// The streams of a connection with no enumeration running, where
    /// `MAX_RESPONSE_BODY_BYTES` is the only cap on what they hold.
    fn held_bytes() -> Arc<HeldEventBytes> {
        Arc::new(HeldEventBytes::new(Arc::new(ListedBytes::default())))
    }

    fn fed(chunks: &[&[u8]]) -> EventBytes {
        let mut event = EventBytes::new(held_bytes());
        for chunk in chunks {
            event.count(chunk).expect("under the cap");
        }
        event
    }

    #[test]
    fn a_blank_line_ends_the_event_for_every_line_ending() {
        for chunks in [
            &[&b"data: abc\n\n"[..]][..],
            &[b"data: abc\r\n\r\n"],
            &[b"data: abc\r\r"],
            &[b"data: abc\r", b"\n\r", b"\n"],
        ] {
            assert_eq!(fed(chunks).len, 0, "{chunks:?} ends its event");
        }
    }

    #[test]
    fn data_lines_without_a_blank_line_add_up() {
        assert_eq!(fed(&[b"data: ab\r\n", b"data: cd\r", b"\ndata: e"]).len, 23);
    }

    #[tokio::test]
    async fn a_stream_that_keeps_sending_outlives_the_idle_timeout() {
        use tokio::io::AsyncReadExt as _;
        use tokio::io::AsyncWriteExt as _;
        const IDLE: std::time::Duration = std::time::Duration::from_secs(1);
        const GAP: std::time::Duration = std::time::Duration::from_millis(50);
        const GAPS: u32 = 30;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            if sock.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            for _ in 0..GAPS {
                tokio::time::sleep(GAP).await;
                if sock.write_all(b": still working\n").await.is_err() {
                    return;
                }
            }
            let _ = sock.shutdown().await;
        });
        let started = std::time::Instant::now();
        let stream = McpHttpClient::with_budget(IDLE, REQUEST_TIMEOUT, PeerBudget::new())
            .get_stream(format!("http://{addr}/mcp").into(), "s".into(), None, None)
            .await
            .unwrap_or_else(|e| panic!("the stream did not open: {e}"));
        let first_error =
            Box::pin(stream.filter_map(|item| async move { item.err().map(|e| e.to_string()) }))
                .next()
                .await;
        assert!(
            first_error.is_none() && started.elapsed() >= GAP * GAPS,
            "a stream that sent a byte every {GAP:?} was cut after {:?}: {first_error:?}",
            started.elapsed()
        );
    }

    /// The URL of a peer that answers one POST with a JSON reply that never
    /// completes: one space every `gap`, or nothing at all after the head.
    async fn json_reply_that_never_completes(gap: Option<Duration>) -> String {
        use tokio::io::AsyncReadExt as _;
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                        Content-Length: 1000000\r\n\r\n{";
            if sock.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            loop {
                match gap {
                    Some(gap) => tokio::time::sleep(gap).await,
                    None => std::future::pending().await,
                }
                if sock.write_all(b" ").await.is_err() {
                    return;
                }
            }
        });
        format!("http://{addr}/mcp")
    }

    async fn post_error(client: McpHttpClient, uri: String) -> String {
        let ping: ClientJsonRpcMessage = serde_json::from_value(
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
        )
        .expect("a ping request");
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            client.post_message(uri.into(), ping, None, None),
        )
        .await
        .expect("the POST was still open after 10s");
        match outcome {
            Ok(reply) => panic!("a reply that never completes was accepted: {reply:?}"),
            Err(e) => e.to_string(),
        }
    }

    #[tokio::test]
    async fn a_json_reply_that_trickles_is_cut_at_the_request_timeout() {
        let uri = json_reply_that_never_completes(Some(Duration::from_millis(100))).await;
        let err = post_error(
            McpHttpClient::with_budget(MCP_IDLE_TIMEOUT, Duration::from_secs(1), PeerBudget::new()),
            uri,
        )
        .await;
        assert!(err.contains("REQUEST_TIMEOUT"), "{err}");
    }

    #[tokio::test]
    async fn a_json_reply_that_stalls_names_the_idle_timeout() {
        let uri = json_reply_that_never_completes(None).await;
        let err = post_error(
            McpHttpClient::with_budget(Duration::from_secs(1), REQUEST_TIMEOUT, PeerBudget::new()),
            uri,
        )
        .await;
        assert!(
            err.contains("MCP_IDLE_TIMEOUT") && !err.contains("REQUEST_TIMEOUT"),
            "the error must name the timeout that fired: {err}"
        );
    }

    #[test]
    fn one_event_of_the_largest_size_in_small_chunks_parses_in_linear_time() {
        const CHUNK: usize = 4 * 1024;
        const CEILING: Duration = Duration::from_secs(10);
        static FILL: [u8; CHUNK] = [b'x'; CHUNK];
        let data_len = MAX_RESPONSE_BODY_BYTES - b"data: ".len();
        let mut chunks = vec![Bytes::from_static(b"data: ")];
        chunks.extend(std::iter::repeat_n(
            Bytes::from_static(&FILL),
            data_len / CHUNK,
        ));
        chunks.push(Bytes::from_static(&FILL[..data_len % CHUNK]));
        chunks.push(Bytes::from_static(b"\n\n"));
        let (done, finished) = std::sync::mpsc::channel();
        // The stack of a tokio worker, where rmcp polls the stream.
        let parser = std::thread::Builder::new().stack_size(2 * 1024 * 1024);
        let _parser = parser
            .spawn(move || {
                let started = std::time::Instant::now();
                let events = futures::executor::block_on(
                    capped_sse(
                        futures::stream::iter(chunks.into_iter().map(Ok)),
                        McpHttpClient::new(PeerBudget::new()).describe(),
                        held_bytes(),
                    )
                    .collect::<Vec<_>>(),
                );
                let lens: Vec<_> = events
                    .into_iter()
                    .map(|e| {
                        e.map(|sse| sse.data.map(|d| d.len()))
                            .map_err(|e| e.to_string())
                    })
                    .collect();
                done.send((started.elapsed(), lens))
                    .expect("the test waits");
            })
            .expect("the parser thread starts");
        let (elapsed, lens) = finished.recv_timeout(CEILING).unwrap_or_else(|_| {
            panic!(
                "one {MAX_RESPONSE_BODY_BYTES}-byte event in {CHUNK}-byte chunks took longer \
                 than {CEILING:?} to parse"
            )
        });
        assert_eq!(lens, vec![Ok(Some(data_len))], "parsed in {elapsed:?}");
    }

    /// The URL of a peer that answers every POST with an SSE stream that sends
    /// `data: ` and then one byte every `gap` and never a blank line, plus the
    /// channel on which it reports the connection it first lost.
    async fn post_streams_that_never_finish(
        gap: Duration,
    ) -> (String, tokio::sync::mpsc::UnboundedReceiver<()>) {
        use tokio::io::AsyncReadExt as _;
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("addr");
        let (closed, closes) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let closed = closed.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
                                data: ";
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        let _ = closed.send(());
                        return;
                    }
                    loop {
                        tokio::time::sleep(gap).await;
                        if sock.write_all(b"x").await.is_err() {
                            let _ = closed.send(());
                            return;
                        }
                    }
                });
            }
        });
        (format!("http://{addr}/mcp"), closes)
    }

    fn ping() -> ClientJsonRpcMessage {
        serde_json::from_value(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .expect("a ping request")
    }

    /// rmcp keeps the reply stream of a request that already timed out, so the
    /// stream has to end on its own; otherwise its partial event and its socket
    /// stay held.
    #[tokio::test]
    async fn a_post_reply_stream_that_never_finishes_is_closed_at_the_request_timeout() {
        const REQUEST: Duration = Duration::from_secs(2);
        let (uri, mut closes) = post_streams_that_never_finish(Duration::from_millis(50)).await;
        let client = McpHttpClient::with_budget(MCP_IDLE_TIMEOUT, REQUEST, PeerBudget::new());
        let reply = client
            .post_message(uri.into(), ping(), None, None)
            .await
            .expect("the peer answered with a stream");
        let mut stream = match reply {
            StreamableHttpPostResponse::Sse(stream, _) => stream,
            other => panic!("expected an SSE reply, got {other:?}"),
        };
        let drained = tokio::time::timeout(REQUEST * 3, async {
            while stream.next().await.is_some() {}
        })
        .await;
        assert!(
            drained.is_ok(),
            "the reply stream was still open {:?} after the deadline",
            REQUEST * 2
        );
        // Still held, as rmcp holds it: the close may not wait for the drop.
        assert!(
            tokio::time::timeout(Duration::from_secs(2), closes.recv())
                .await
                .is_ok(),
            "the socket of the finished stream is still open"
        );
        drop(stream);
    }

    /// Each unfinished stream holds up to [`MAX_RESPONSE_BODY_BYTES`] of a
    /// partial event, so their number is what bounds the memory a peer can make
    /// the process hold.
    #[tokio::test]
    async fn no_more_post_reply_streams_than_the_budget_are_open_at_once() {
        let (uri, _closes) = post_streams_that_never_finish(Duration::from_millis(50)).await;
        let client = McpHttpClient::with_budget(
            MCP_IDLE_TIMEOUT,
            Duration::from_secs(600),
            PeerBudget::new(),
        );
        let open = |client: McpHttpClient, uri: String| async move {
            client.post_message(uri.into(), ping(), None, None).await
        };
        let mut held = Vec::new();
        for i in 0..MAX_CONCURRENT_POST_STREAMS {
            match open(client.clone(), uri.clone()).await {
                Ok(StreamableHttpPostResponse::Sse(stream, _)) => held.push(stream),
                other => panic!("stream #{i} within the budget was not opened: {other:?}"),
            }
        }

        // One past the budget waits instead of opening a stream, because
        // Holon's own fan-out is concurrent and a full budget is contention.
        let mut past = Box::pin(open(client.clone(), uri.clone()));
        assert!(
            tokio::time::timeout(Duration::from_millis(500), &mut past)
                .await
                .is_err(),
            "a POST past the budget opened a {}th stream",
            held.len() + 1
        );
        held.pop();
        match tokio::time::timeout(Duration::from_secs(5), past).await {
            Ok(Ok(StreamableHttpPostResponse::Sse(stream, _))) => held.push(stream),
            other => panic!("the freed slot did not let the waiting POST through: {other:?}"),
        }
        assert_eq!(held.len(), MAX_CONCURRENT_POST_STREAMS);

        // A budget the peer never frees is a failure that names it.
        let starved = McpHttpClient {
            budget: client.budget.clone(),
            ..McpHttpClient::with_budget(
                MCP_IDLE_TIMEOUT,
                Duration::from_millis(200),
                PeerBudget::new(),
            )
        };
        let err = starved
            .post_message(uri.into(), ping(), None, None)
            .await
            .err()
            .expect("a POST with no slot and no time cannot succeed")
            .to_string();
        assert!(
            err.contains("MAX_CONCURRENT_POST_STREAMS") && err.contains("REQUEST_TIMEOUT"),
            "the failure must name the budget and the deadline; got: {err}"
        );
    }

    #[test]
    fn an_event_one_byte_past_the_cap_is_refused() {
        let mut event = EventBytes::new(held_bytes());
        event
            .count(&vec![b'a'; MAX_RESPONSE_BODY_BYTES])
            .expect("exactly the cap is accepted");
        let err = event.count(b"a").expect_err("one byte past the cap");
        assert!(err.to_string().contains("MAX_RESPONSE_BODY_BYTES"), "{err}");
    }

    /// The notification stream carries the same partial event a POST reply
    /// carries, so the same number of them is what the connection may hold.
    /// rmcp reopens one whenever the old one ends, and its retry policy has no
    /// ceiling, so the budget is also what rate-limits a reconnect storm.
    #[tokio::test]
    async fn no_more_notification_streams_than_the_budget_are_open_at_once() {
        let (uri, _closes) = post_streams_that_never_finish(Duration::from_millis(50)).await;
        let client = McpHttpClient::with_budget(
            MCP_IDLE_TIMEOUT,
            Duration::from_secs(600),
            PeerBudget::new(),
        );
        let open = |client: McpHttpClient, uri: String| async move {
            client
                .get_stream(uri.into(), "session".into(), None, None)
                .await
        };
        let mut held = Vec::new();
        for i in 0..MAX_CONCURRENT_GET_STREAMS {
            match open(client.clone(), uri.clone()).await {
                Ok(stream) => held.push(stream),
                Err(e) => panic!("stream #{i} within the budget was not opened: {e}"),
            }
        }

        let mut past = Box::pin(open(client.clone(), uri.clone()));
        assert!(
            tokio::time::timeout(Duration::from_millis(500), &mut past)
                .await
                .is_err(),
            "a GET past the budget opened a {}th notification stream",
            held.len() + 1
        );
        held.pop();
        match tokio::time::timeout(Duration::from_secs(5), past).await {
            Ok(Ok(stream)) => held.push(stream),
            Ok(Err(e)) => panic!("the freed slot did not let the waiting GET through: {e}"),
            Err(_) => panic!("the waiting GET did not take the freed slot within 5s"),
        }
        assert_eq!(held.len(), MAX_CONCURRENT_GET_STREAMS);

        let starved = McpHttpClient {
            budget: client.budget.clone(),
            ..McpHttpClient::with_budget(
                MCP_IDLE_TIMEOUT,
                Duration::from_millis(200),
                PeerBudget::new(),
            )
        };
        let err = open(starved, uri)
            .await
            .err()
            .expect("a GET with no slot and no time cannot succeed")
            .to_string();
        assert!(
            err.contains("MAX_CONCURRENT_GET_STREAMS") && err.contains("REQUEST_TIMEOUT"),
            "the failure must name the budget and the deadline; got: {err}"
        );
    }

    /// Counting each stream's partial event on its own bounds the memory at
    /// the stream count times the cap. One allowance for the whole connection
    /// is what makes the number a bound.
    #[test]
    fn the_partial_events_of_a_connection_share_one_byte_allowance() {
        let held = held_bytes();
        let half = MAX_RESPONSE_BODY_BYTES / 2;
        let mut first = EventBytes::new(held.clone());
        let mut second = EventBytes::new(held.clone());
        first.count(&vec![b'a'; half]).expect("half the allowance");
        second.count(&vec![b'b'; half]).expect("the other half");
        let err = second
            .count(b"x")
            .expect_err("one byte past the shared allowance");
        assert!(err.to_string().contains("MAX_RESPONSE_BODY_BYTES"), "{err}");

        // A completed event hands its bytes back; so does a dropped stream.
        first
            .count(b"\n\n")
            .expect("a blank line ends the event under the cap");
        let mut third = EventBytes::new(held.clone());
        third
            .count(&vec![b'c'; half])
            .expect("a finished event released its half");
        drop(third);
        EventBytes::new(held)
            .count(&vec![b'd'; half])
            .expect("a dropped stream released its half");
    }
}
