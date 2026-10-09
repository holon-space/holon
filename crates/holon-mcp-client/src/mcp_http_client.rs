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
/// A stream gets only the idle timeout because a connection keeps an SSE
/// stream open for its whole life; rmcp reopens a stream the idle timeout
/// cut. A POST gets a total one because rmcp sends a connection's messages one
/// POST at a time, so one reply that never ends stalls every later request.
#[derive(Clone)]
pub(crate) struct McpHttpClient {
    client: reqwest::Client,
    idle_timeout: Duration,
    request_timeout: Duration,
}

impl McpHttpClient {
    pub(crate) fn new() -> Self {
        Self::with_timeouts(MCP_IDLE_TIMEOUT, REQUEST_TIMEOUT)
    }

    fn with_timeouts(idle_timeout: Duration, request_timeout: Duration) -> Self {
        Self {
            client: crate::secure_client::https_only()
                .read_timeout(idle_timeout)
                .build()
                .expect("a reqwest client with a redirect policy and a read timeout must build"),
            idle_timeout,
            request_timeout,
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

    async fn post(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
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
                    capped_events(response, self.describe()),
                    session_id,
                ))
            }
            Some(ct) if ct.starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let body = read_capped(response, self.describe()).await.map_err(|e| {
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
        let timeout = self.request_timeout;
        tokio::time::timeout(timeout, self.post(uri, message, session_id, auth_token))
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
                Ok(capped_events(response, self.describe()))
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
) -> BoxedSseResponse {
    capped_sse(response.bytes_stream(), describe)
}

fn capped_sse(
    bytes: impl futures::Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
    describe: impl Fn(reqwest::Error) -> String + Send + 'static,
) -> BoxedSseResponse {
    let mut event = EventBytes::default();
    let bytes = bytes.map(move |chunk| {
        let chunk = chunk.map_err(|e| std::io::Error::other(describe(e)))?;
        event.count(&chunk)?;
        Ok::<_, std::io::Error>(chunk)
    });
    sse_stream::SseStream::from_bytes_stream(bytes).boxed()
}

/// The size of the SSE event being received. A blank line ends an event, and
/// a line ends at `\n`, `\r` or `\r\n`.
#[derive(Default)]
struct EventBytes {
    len: usize,
    line_is_empty: bool,
    after_cr: bool,
}

impl EventBytes {
    fn count(&mut self, chunk: &[u8]) -> std::io::Result<()> {
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
            if self.len > MAX_RESPONSE_BODY_BYTES {
                return Err(std::io::Error::other(format!(
                    "an event of the MCP stream is larger than MAX_RESPONSE_BODY_BYTES \
                     ({MAX_RESPONSE_BODY_BYTES} bytes); reading stopped there"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(chunks: &[&[u8]]) -> EventBytes {
        let mut event = EventBytes::default();
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
        let stream = McpHttpClient::with_timeouts(IDLE, REQUEST_TIMEOUT)
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
            McpHttpClient::with_timeouts(MCP_IDLE_TIMEOUT, Duration::from_secs(1)),
            uri,
        )
        .await;
        assert!(err.contains("REQUEST_TIMEOUT"), "{err}");
    }

    #[tokio::test]
    async fn a_json_reply_that_stalls_names_the_idle_timeout() {
        let uri = json_reply_that_never_completes(None).await;
        let err = post_error(
            McpHttpClient::with_timeouts(Duration::from_secs(1), REQUEST_TIMEOUT),
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
                        McpHttpClient::new().describe(),
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

    #[test]
    fn an_event_one_byte_past_the_cap_is_refused() {
        let mut event = EventBytes::default();
        event
            .count(&vec![b'a'; MAX_RESPONSE_BODY_BYTES])
            .expect("exactly the cap is accepted");
        let err = event.count(b"a").expect_err("one byte past the cap");
        assert!(err.to_string().contains("MAX_RESPONSE_BODY_BYTES"), "{err}");
    }
}
