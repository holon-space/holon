//! The HTTP client of an MCP-over-HTTP connection.
//!
//! The requests mirror rmcp's own `reqwest` client; the bodies are read
//! through [`read_capped`] and [`EventBytes`].

use std::borrow::Cow;
use std::sync::Arc;

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
use crate::secure_client::cause_chain;
use crate::secure_client::read_capped;

type HttpError = StreamableHttpError<McpHttpError>;

/// The secure client's redirect policy, [`MCP_IDLE_TIMEOUT`], and every
/// JSON-RPC message capped at [`MAX_RESPONSE_BODY_BYTES`].
///
/// The timeout is an idle one because a connection keeps an SSE stream open
/// for its whole life, and a total timeout would cut a healthy one. rmcp
/// reopens a stream the idle timeout cut.
#[derive(Clone)]
pub(crate) struct McpHttpClient(reqwest::Client);

impl McpHttpClient {
    pub(crate) fn new() -> Self {
        Self::with_idle_timeout(MCP_IDLE_TIMEOUT)
    }

    fn with_idle_timeout(idle: std::time::Duration) -> Self {
        Self(
            crate::secure_client::https_only()
                .read_timeout(idle)
                .build()
                .expect("a reqwest client with a redirect policy and a read timeout must build"),
        )
    }
}

/// A transport failure, with every URL stripped.
#[derive(Debug)]
pub(crate) struct McpHttpError(String);

impl std::fmt::Display for McpHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for McpHttpError {}

fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        format!(
            "no byte from the peer within MCP_IDLE_TIMEOUT ({MCP_IDLE_TIMEOUT:?}): {}",
            cause_chain(e)
        )
    } else {
        cause_chain(e)
    }
}

fn client_error(e: reqwest::Error) -> HttpError {
    StreamableHttpError::Client(McpHttpError(describe(e)))
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
        let mut request = self
            .0
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
        let response = request.send().await.map_err(client_error)?;
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
        let response = response.error_for_status().map_err(client_error)?;
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
            Some(ct) if ct.starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => Ok(
                StreamableHttpPostResponse::Sse(capped_events(response), session_id),
            ),
            Some(ct) if ct.starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let body = read_capped(response).await.map_err(|e| {
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

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
    ) -> Result<(), HttpError> {
        let mut request = self
            .0
            .delete(uri.as_ref())
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(client_error)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        response.error_for_status().map_err(client_error)?;
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
            .0
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(last_event_id) = last_event_id {
            request = request.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(client_error)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response.error_for_status().map_err(client_error)?;
        match content_type(&response) {
            Some(ct)
                if ct.starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    || ct.starts_with(JSON_MIME_TYPE.as_bytes()) =>
            {
                Ok(capped_events(response))
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

fn capped_events(response: reqwest::Response) -> BoxedSseResponse {
    let mut event = EventBytes::default();
    let bytes = response.bytes_stream().map(move |chunk| {
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
        let stream = McpHttpClient::with_idle_timeout(IDLE)
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
