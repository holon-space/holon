//! A capability URL carries its credential in a PATH segment, so query-string
//! stripping cannot protect it. These tests drive the real `rest` transport
//! against a LOCAL mock server (no network) and assert that the token a
//! `${VAR}` supplied never appears in an error string or a log line — including
//! when the upstream response body echoes the request URL back at us.
//!
//! Every token here is synthetic and generated for this file.

use std::io::Write;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::Once;

use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::rest_transport::RestCallSurface;
use rmcp::model::CallToolRequestParam;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// The synthetic credential that sits in the URL path. Nothing else in this
/// file may contain this literal, so any occurrence in an error or log is a
/// leak.
const CAP_TOKEN: &str = "cap-4Qk3vR7mNp2xW5tLb9dHs4gJc6yFa0e";

/// The `${VAR}` the sidecar references; the test lookup resolves it to
/// [`CAP_TOKEN`], which is what makes the value a secret by construction.
const CAP_TOKEN_VAR: &str = "HOLON_TEST_CAPABILITY_TOKEN";

/// A secret containing a character the URL layer percent-encodes (a space goes
/// on the wire as `%20`), so the form an echoed body carries differs from the
/// form the config registered.
const CAP_TOKEN_ODD: &str = "cap 4Qk3vR7mNp2xW5tLb9dHs4gJc6yFa0e";

const CAP_TOKEN_ODD_VAR: &str = "HOLON_TEST_CAPABILITY_TOKEN_ODD";

/// A secret the URL layer encodes only PARTIALLY: `<` becomes `%3C`, `|` goes
/// through untouched. Neither the raw nor a fully-encoded form appears on the
/// wire.
const CAP_TOKEN_MIXED: &str = "cap|4Qk3<vR7mNp2xW5tLb9dHs4gJc6yFa0e";

const CAP_TOKEN_MIXED_VAR: &str = "HOLON_TEST_CAPABILITY_TOKEN_MIXED";

/// A secret containing a backslash, which a URL parser rewrites to `/` rather
/// than escaping — a transform no encoding table covers.
const CAP_TOKEN_BACKSLASH: &str = r"cap\4Qk3vR7mNp2xW5tLb9dHs4gJc6yFa0e";

const CAP_TOKEN_BACKSLASH_VAR: &str = "HOLON_TEST_CAPABILITY_TOKEN_BACKSLASH";

/// What the mock's token endpoint mints. Never configured anywhere — it exists
/// only at runtime, which is the point of the rung that uses it.
const MINTED_TOKEN: &str = "access-tok-7Hn4pQ2sVb9eLxTm";

// ---------------------------------------------------------------------------
// Log capture
// ---------------------------------------------------------------------------

static LOG_BUF: LazyLock<Arc<Mutex<Vec<u8>>>> = LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));

#[derive(Clone)]
struct CaptureWriter;

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOG_BUF.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for CaptureWriter {
    type Writer = CaptureWriter;
    fn make_writer(&self) -> CaptureWriter {
        self.clone()
    }
}

fn init_log_capture() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        tracing_subscriber::fmt()
            .with_writer(CaptureWriter)
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .init();
    });
}

fn captured_logs() -> String {
    String::from_utf8_lossy(&LOG_BUF.lock().unwrap()).to_string()
}

// ---------------------------------------------------------------------------
// Mock server
// ---------------------------------------------------------------------------

/// How the mock answers a protected GET. Each arm reproduces one shape of
/// upstream failure the transport turns into an error string.
#[derive(Clone, Copy)]
enum Mode {
    /// HTTP 500 whose JSON body quotes the request target back.
    EchoUrlIn500,
    /// HTTP 200 whose body is not JSON and quotes the request target back.
    EchoUrlInNonJson,
    /// HTTP 401 on every attempt, so an OAuth2 call refreshes and retries once.
    Always401,
    /// HTTP 500 whose body quotes the `Authorization` header back — the shape
    /// that discloses a token minted at runtime rather than configured.
    EchoBearerIn500,
}

fn respond(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: \
         close\r\n\r\n{body}",
        body.len()
    )
}

async fn start_mock(mode: Mode) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");

    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let mut parts = head.lines().next().unwrap_or("").split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("/").to_string();

                let response = if method == "POST" && target.ends_with("/token") {
                    respond(
                        "200 OK",
                        "application/json",
                        &format!(
                            r#"{{"access_token":"{MINTED_TOKEN}","expires_in":3600,"token_type":"Bearer"}}"#
                        ),
                    )
                } else {
                    let authorization = head
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                        .and_then(|l| l.split_once(':'))
                        .map(|(_, v)| v.trim().to_string())
                        .unwrap_or_default();
                    match mode {
                        Mode::EchoUrlIn500 => respond(
                            "500 Internal Server Error",
                            "application/json",
                            &format!(r#"{{"error":"upstream failed for {target}"}}"#),
                        ),
                        Mode::EchoUrlInNonJson => respond(
                            "200 OK",
                            "text/html",
                            &format!("<html>no route for {target}</html>"),
                        ),
                        Mode::Always401 => respond(
                            "401 Unauthorized",
                            "application/json",
                            &format!(r#"{{"error":"denied for {target}"}}"#),
                        ),
                        Mode::EchoBearerIn500 => respond(
                            "500 Internal Server Error",
                            "application/json",
                            &format!(r#"{{"error":"cannot verify {authorization}"}}"#),
                        ),
                    }
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    format!("http://{addr}")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn lookup(name: &str) -> Option<String> {
    match name {
        CAP_TOKEN_VAR => Some(CAP_TOKEN.to_string()),
        CAP_TOKEN_ODD_VAR => Some(CAP_TOKEN_ODD.to_string()),
        CAP_TOKEN_MIXED_VAR => Some(CAP_TOKEN_MIXED.to_string()),
        CAP_TOKEN_BACKSLASH_VAR => Some(CAP_TOKEN_BACKSLASH.to_string()),
        "REDACTION_TEST_CLIENT_ID" => Some("client-id-123".to_string()),
        "REDACTION_TEST_CLIENT_SECRET" => Some("client-secret-xyz".to_string()),
        _ => None,
    }
}

fn surface_from(yaml: &str) -> RestCallSurface {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("sidecar parses");
    let mcp = cfg
        .into_mcp_config_with("capability".to_string(), &lookup)
        .expect("into_mcp_config");
    match mcp.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("expected rest transport, got {other:?}"),
    }
}

/// A sidecar whose `base_url` puts the credential in a path segment — the
/// capability-URL shape.
fn capability_yaml(base: &str) -> String {
    capability_yaml_with(base, CAP_TOKEN_VAR)
}

fn capability_yaml_with(base: &str, token_var: &str) -> String {
    format!(
        r#"
transport:
  rest:
    base_url: {base}/c/${{{token_var}}}
    calls:
      get-things:
        method: GET
        path: /things
entities: {{}}
tools: {{}}
"#
    )
}

/// The same capability URL, behind the OAuth2 auth arm so the 401 retry path
/// (the transport's only `warn!`) runs.
fn oauth_capability_yaml(base: &str, refresh_file: &str) -> String {
    format!(
        r#"
transport:
  rest:
    base_url: {base}/c/${{{CAP_TOKEN_VAR}}}
    auth:
      oauth2:
        token_url: {base}/token
        client_id_env: REDACTION_TEST_CLIENT_ID
        client_secret_env: REDACTION_TEST_CLIENT_SECRET
        refresh_token_file: {refresh_file}
        scopes: [scope.readonly]
    calls:
      get-things:
        method: GET
        path: /things
entities: {{}}
tools: {{}}
"#
    )
}

fn temp_refresh_token(contents: &str) -> (tempfile::TempDir, String) {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capability-refresh-token");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    (dir, path.to_str().unwrap().to_string())
}

async fn call_err(surface: &RestCallSurface) -> String {
    surface
        .call_tool(CallToolRequestParam {
            name: "get-things".to_string().into(),
            arguments: None,
        })
        .await
        .map(|r| format!("{r:?}"))
        .expect_err("the mock always fails this call")
        .to_string()
}

fn assert_no_leak(what: &str, text: &str) {
    assert_secret_absent(what, text, CAP_TOKEN);
}

fn assert_secret_absent(what: &str, text: &str, secret: &str) {
    assert!(!text.contains(secret), "{what} leaked a secret: {text}");
}

/// The positive half of the contract: the message names the request and says
/// `<redacted>` where the secret stood, rather than having gone silent.
fn assert_redacted_marker(what: &str, text: &str) {
    assert!(
        text.contains("<redacted>"),
        "{what} carries no redaction marker, so nothing was replaced: {text}"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_error_hides_a_capability_token_in_the_url_path() {
    let base = start_mock(Mode::EchoUrlIn500).await;
    let surface = surface_from(&capability_yaml(&base));

    let err = call_err(&surface).await;
    // The failure stays actionable...
    assert!(err.contains("500"), "status missing from error: {err}");
    // ...without the credential, in the URL or in the echoed body.
    assert_no_leak("the HTTP error", &err);
    assert_redacted_marker("the HTTP error", &err);
}

#[tokio::test]
async fn a_partially_encoded_capability_token_is_hidden_in_an_echoed_body() {
    let base = start_mock(Mode::EchoUrlIn500).await;
    let surface = surface_from(&capability_yaml_with(&base, CAP_TOKEN_MIXED_VAR));

    let err = call_err(&surface).await;
    assert!(err.contains("500"), "status missing from error: {err}");
    // Neither the raw form nor the wire form (`<` escaped, `|` not) survives.
    assert_secret_absent("the echoed body", &err, CAP_TOKEN_MIXED);
    assert_secret_absent(
        "the echoed body",
        &err,
        &CAP_TOKEN_MIXED.replace('<', "%3C"),
    );
    assert_redacted_marker("the echoed body", &err);
}

#[tokio::test]
async fn a_backslash_capability_token_is_hidden_in_an_echoed_body() {
    let base = start_mock(Mode::EchoUrlIn500).await;
    let surface = surface_from(&capability_yaml_with(&base, CAP_TOKEN_BACKSLASH_VAR));

    let err = call_err(&surface).await;
    assert!(err.contains("500"), "status missing from error: {err}");
    // The URL parser rewrote `\` to `/` before the request went out, so the
    // echoed body carries a form the registered bytes never had.
    assert_secret_absent("the echoed body", &err, CAP_TOKEN_BACKSLASH);
    assert_secret_absent(
        "the echoed body",
        &err,
        &CAP_TOKEN_BACKSLASH.replace('\\', "/"),
    );
    assert_redacted_marker("the echoed body", &err);
}

#[tokio::test]
async fn non_json_body_error_hides_a_capability_token_in_the_url_path() {
    let base = start_mock(Mode::EchoUrlInNonJson).await;
    let surface = surface_from(&capability_yaml(&base));

    let err = call_err(&surface).await;
    assert!(
        err.contains("not JSON"),
        "decode failure missing from error: {err}"
    );
    assert_no_leak("the decode error", &err);
    assert_redacted_marker("the decode error", &err);
}

#[tokio::test]
async fn debug_of_the_manual_hides_a_capability_token_in_the_base_url() {
    let base = start_mock(Mode::EchoUrlIn500).await;
    let surface = surface_from(&capability_yaml(&base));

    let shown = format!("{surface:?}");
    assert_no_leak("RestCallSurface's Debug", &shown);
    assert_redacted_marker("RestCallSurface's Debug", &shown);
}

#[tokio::test]
async fn oauth2_401_retry_log_hides_a_capability_token_in_the_url_path() {
    init_log_capture();
    let base = start_mock(Mode::Always401).await;
    let (_dir, refresh_file) = temp_refresh_token("refresh-abc\n");
    let surface = surface_from(&oauth_capability_yaml(&base, &refresh_file));

    let err = call_err(&surface).await;
    assert!(
        err.contains("after token refresh"),
        "retry failure missing from error: {err}"
    );
    assert_no_leak("the post-refresh error", &err);
    assert_redacted_marker("the post-refresh error", &err);

    let logs = captured_logs();
    assert!(
        logs.contains("refreshing OAuth2 token"),
        "the 401 retry warning never reached the subscriber: {logs}"
    );
    assert_no_leak("the 401 retry warning", &logs);
    assert_redacted_marker("the 401 retry warning", &logs);
}

#[tokio::test]
async fn an_echoed_bearer_header_hides_the_token_minted_at_runtime() {
    let base = start_mock(Mode::EchoBearerIn500).await;
    let (_dir, refresh_file) = temp_refresh_token("refresh-tok-3Wq8zRc5NvKd\n");
    let surface = surface_from(&oauth_capability_yaml(&base, &refresh_file));

    let err = call_err(&surface).await;
    assert!(err.contains("500"), "status missing from error: {err}");
    // The token was never configured — it exists only because the token
    // endpoint minted it mid-request.
    assert_secret_absent("the echoed-bearer error", &err, MINTED_TOKEN);
    assert_secret_absent("the echoed-bearer error", &err, "refresh-tok-3Wq8zRc5NvKd");
    assert_redacted_marker("the echoed-bearer error", &err);
}

#[tokio::test]
async fn a_percent_encoded_capability_token_is_hidden_in_an_echoed_body() {
    let base = start_mock(Mode::EchoUrlIn500).await;
    let surface = surface_from(&capability_yaml_with(&base, CAP_TOKEN_ODD_VAR));

    let err = call_err(&surface).await;
    assert!(err.contains("500"), "status missing from error: {err}");
    // The URL carries the raw form and the echoed body the `%20` form; both
    // are the same secret.
    assert_secret_absent("the echoed body", &err, CAP_TOKEN_ODD);
    assert_secret_absent("the echoed body", &err, &CAP_TOKEN_ODD.replace(' ', "%20"));
    assert_redacted_marker("the echoed body", &err);
}
