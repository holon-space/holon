//! The in-app OAuth2 consent flow, against a LOCAL mock authorization server
//! (no network, no browser).
//!
//! What is proven here: PKCE is computed per RFC 7636 `S256`; a callback whose
//! `state` does not match is refused BEFORE any token exchange happens; the
//! provider's own errors and a silent listener both fail loud; the exchange
//! carries the verifier; the refresh token lands 0600 at the location the
//! SIDECAR declares; the state file records LOCATIONS and never secret
//! material.
//!
//! What is NOT proven here: the browser hop itself. The flow opens the system
//! browser through the [`BrowserOpener`] seam, and every test supplies a
//! recording opener instead. The real hop is on the verify-live checklist.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::CredentialRef;
use holon_mcp_client::oauth_bootstrap::AuthState;
use holon_mcp_client::oauth_bootstrap::AuthorizationRequest;
use holon_mcp_client::oauth_bootstrap::BrowserOpener;
use holon_mcp_client::oauth_bootstrap::LoopbackRedirect;
use holon_mcp_client::oauth_bootstrap::Pkce;
use holon_mcp_client::rest_oauth2::KeychainRef;
use holon_mcp_client::rest_oauth2::RestOAuth2Config;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// A mock token endpoint, following the `rest_oauth2_mock.rs` precedent.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockState {
    /// Bodies of every POST the token endpoint received.
    token_bodies: Vec<String>,
    /// When set, the token endpoint answers with this (status, body) instead of
    /// a successful token response.
    canned_failure: Option<(&'static str, String)>,
    /// When true, a successful response omits `refresh_token` (Google's
    /// "already consented, no fresh grant" shape).
    omit_refresh_token: bool,
}

struct Mock {
    token_url: String,
    state: Arc<Mutex<MockState>>,
}

impl Mock {
    fn with<R>(&self, f: impl FnOnce(&mut MockState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    fn token_post_count(&self) -> usize {
        self.state.lock().unwrap().token_bodies.len()
    }

    fn last_token_body(&self) -> String {
        self.state
            .lock()
            .unwrap()
            .token_bodies
            .last()
            .cloned()
            .expect("the token endpoint was never POSTed to")
    }
}

/// The refresh token the mock mints. A distinctive, greppable literal so a
/// leak assertion cannot pass by accident.
const MINTED_REFRESH_TOKEN: &str = "mock-refresh-token-4Kq9zP";
/// The client secret the fixture provisions, likewise greppable.
const FIXTURE_CLIENT_SECRET: &str = "mock-client-secret-7Xv2Ln";
const FIXTURE_CLIENT_ID: &str = "mock-client-id.apps.example.com";

async fn start_mock() -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(Mutex::new(MockState::default()));
    let bg = state.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let bg = bg.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();

                let (status, payload) = {
                    let mut st = bg.lock().unwrap();
                    st.token_bodies.push(body);
                    if let Some((status, body)) = st.canned_failure.clone() {
                        (status, body)
                    } else if st.omit_refresh_token {
                        (
                            "200 OK",
                            r#"{"access_token":"access-only","expires_in":3599}"#.to_string(),
                        )
                    } else {
                        (
                            "200 OK",
                            format!(
                                r#"{{"access_token":"access-1","expires_in":3599,"refresh_token":"{MINTED_REFRESH_TOKEN}"}}"#
                            ),
                        )
                    }
                };

                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });

    Mock {
        token_url: format!("http://{addr}/token"),
        state,
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A browser opener that records instead of launching. Also carries the
/// callback the test wants driven once the URL is "opened", so a full-flow test
/// can play the provider's part.
struct RecordingBrowser {
    opened: Mutex<Vec<String>>,
    /// Query string appended to the redirect_uri when driving the callback.
    /// `None` means: record the URL and drive nothing.
    respond: Option<Box<dyn Fn(&str) -> String + Send + Sync>>,
}

impl RecordingBrowser {
    fn recording_only() -> Self {
        Self {
            opened: Mutex::new(Vec::new()),
            respond: None,
        }
    }

    fn driving(f: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        Self {
            opened: Mutex::new(Vec::new()),
            respond: Some(Box::new(f)),
        }
    }

    fn opened_urls(&self) -> Vec<String> {
        self.opened.lock().unwrap().clone()
    }
}

impl BrowserOpener for RecordingBrowser {
    fn open(&self, url: &str) -> anyhow::Result<()> {
        self.opened.lock().unwrap().push(url.to_string());
        if let Some(respond) = &self.respond {
            let callback = respond(url);
            std::thread::spawn(move || {
                // The flow's listener is bound before the browser is opened, so
                // a plain blocking GET from another thread is the callback.
                let _ = ureq_get(&callback);
            });
        }
        Ok(())
    }
}

/// Minimal blocking HTTP GET — enough to deliver an OAuth redirect to a
/// loopback listener that answers once and closes.
fn ureq_get(url: &str) -> std::io::Result<String> {
    use std::io::Read;
    use std::io::Write;

    let rest = url
        .strip_prefix("http://")
        .expect("loopback callback is http");
    let (authority, path_and_query) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut sock = std::net::TcpStream::connect(authority)?;
    write!(
        sock,
        "GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )?;
    sock.flush()?;
    let mut out = String::new();
    sock.read_to_string(&mut out)?;
    Ok(out)
}

/// Extract a query parameter from a URL the flow built.
fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// A sidecar `oauth2` block with file-declared client credentials, pointing at
/// the mock token endpoint.
fn config_for(dir: &Path, token_url: &str) -> RestOAuth2Config {
    RestOAuth2Config {
        token_url: token_url.to_string(),
        auth_url: Some("https://provider.example/o/oauth2/v2/auth".to_string()),
        auth_params: HashMap::from([
            ("access_type".to_string(), "offline".to_string()),
            ("prompt".to_string(), "consent".to_string()),
        ]),
        client_id_env: None,
        client_id_file: Some(dir.join("client-id").to_string_lossy().into_owned()),
        client_id_keychain: None,
        client_secret_env: None,
        client_secret_file: Some(dir.join("client-secret").to_string_lossy().into_owned()),
        client_secret_keychain: None,
        refresh_token_file: dir.join("refresh-token").to_string_lossy().into_owned(),
        scopes: vec!["https://provider.example/auth/thing.readonly".to_string()],
    }
}

/// Provision the client id/secret the way the console walkthrough tells the
/// user to: 0600 files the flow reads but never writes.
fn provision_client_credentials(dir: &Path) {
    write_private(&dir.join("client-id"), FIXTURE_CLIENT_ID);
    write_private(&dir.join("client-secret"), FIXTURE_CLIENT_SECRET);
}

fn write_private(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn no_env_lookup(_: &str) -> Option<String> {
    None
}

/// An integrations directory holding the bundled `gcal` sidecar's state file.
fn store_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// ---------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------

/// RFC 7636 §4.2: `code_challenge = BASE64URL-ENCODE(SHA256(ASCII(verifier)))`,
/// unpadded. Recomputed here independently — a challenge the engine merely
/// agrees with itself about would pass a self-consistency check and still be
/// rejected by every real provider.
#[test]
fn pkce_challenge_is_the_unpadded_base64url_sha256_of_the_verifier() {
    use base64::Engine as _;
    use sha2::Digest as _;

    let pkce = Pkce::generate();
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(pkce.verifier().as_bytes()));

    assert_eq!(
        pkce.challenge(),
        expected,
        "the S256 challenge must be the unpadded base64url SHA-256 of the verifier"
    );
}

/// RFC 7636 §4.1: 43–128 characters from the unreserved set, with enough
/// entropy that guessing it is not a path around the binding it provides.
#[test]
fn pkce_verifiers_are_unreserved_long_enough_and_unique_per_flow() {
    let a = Pkce::generate();
    let b = Pkce::generate();

    for pkce in [&a, &b] {
        let v = pkce.verifier();
        assert!(
            (43..=128).contains(&v.len()),
            "verifier length {} is outside RFC 7636's 43..=128",
            v.len()
        );
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)),
            "verifier must use only the unreserved set, got {v:?}"
        );
    }

    assert_ne!(
        a.verifier(),
        b.verifier(),
        "two flows must not share a verifier"
    );
}

/// The secret half must not be reachable through a log line.
#[test]
fn pkce_and_state_debug_redact_their_secret_halves() {
    let pkce = Pkce::generate();
    let state = AuthState::generate();

    let pkce_debug = format!("{pkce:?}");
    assert!(
        !pkce_debug.contains(pkce.verifier()),
        "Debug for Pkce leaked the verifier: {pkce_debug}"
    );
    let state_debug = format!("{state:?}");
    assert!(
        !state_debug.contains(state.as_str()),
        "Debug for AuthState leaked the nonce: {state_debug}"
    );
}

// ---------------------------------------------------------------------------
// The authorization request
// ---------------------------------------------------------------------------

#[test]
fn the_authorization_url_carries_pkce_state_scopes_and_the_sidecars_extra_params() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_for(dir.path(), "http://unused/token");
    let req = AuthorizationRequest::from_config(&cfg).unwrap();
    let pkce = Pkce::generate();
    let state = AuthState::generate();

    let url = req.url(FIXTURE_CLIENT_ID, "http://127.0.0.1:41234", &state, &pkce);

    assert!(
        url.starts_with("https://provider.example/o/oauth2/v2/auth?"),
        "url must extend the sidecar's auth_url, got {url}"
    );
    assert_eq!(query_param(&url, "response_type").as_deref(), Some("code"));
    assert_eq!(
        query_param(&url, "client_id").as_deref(),
        Some(FIXTURE_CLIENT_ID)
    );
    assert_eq!(
        query_param(&url, "redirect_uri").as_deref(),
        Some("http://127.0.0.1:41234")
    );
    assert_eq!(query_param(&url, "state").as_deref(), Some(state.as_str()));
    assert_eq!(
        query_param(&url, "code_challenge").as_deref(),
        Some(pkce.challenge())
    );
    assert_eq!(
        query_param(&url, "code_challenge_method").as_deref(),
        Some("S256"),
        "plain PKCE is not acceptable — S256 is what binds the code"
    );
    assert_eq!(
        query_param(&url, "scope").as_deref(),
        Some("https://provider.example/auth/thing.readonly")
    );
    assert_eq!(query_param(&url, "access_type").as_deref(), Some("offline"));
    assert_eq!(query_param(&url, "prompt").as_deref(), Some("consent"));

    assert!(
        !url.contains(pkce.verifier()),
        "the verifier must never travel on the authorization request"
    );
}

/// A sidecar that can refresh but cannot consent must say so, rather than
/// opening a browser at a URL that does not exist.
#[test]
fn a_sidecar_without_an_auth_url_refuses_to_start_the_flow() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config_for(dir.path(), "http://unused/token");
    cfg.auth_url = None;

    let err = AuthorizationRequest::from_config(&cfg).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("auth_url"),
        "the refusal must name the missing field, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// The loopback listener
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listener_binds_loopback_only_on_an_ephemeral_port() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let uri = redirect.redirect_uri().to_string();

    assert!(
        uri.starts_with("http://127.0.0.1:"),
        "the redirect must be loopback-only (never 0.0.0.0 or a hostname), got {uri}"
    );
    let port: u16 = uri.rsplit(':').next().unwrap().parse().unwrap();
    assert_ne!(port, 0, "the OS-assigned port must be reflected in the URI");
}

/// The CSRF check. A callback bearing the wrong `state` is somebody else's
/// (or an attacker's) redirect, and the code in it must never be exchanged.
#[tokio::test]
async fn a_state_mismatch_is_refused_and_no_code_is_returned() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let uri = redirect.redirect_uri().to_string();
    let expected = AuthState::generate();

    std::thread::spawn(move || {
        let _ = ureq_get(&format!(
            "{uri}/?code=stolen-code&state=not-the-right-state"
        ));
    });

    let err = redirect
        .wait_for_code(&expected, Duration::from_secs(5))
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("state"),
        "the refusal must name the state mismatch, got: {msg}"
    );
    assert!(
        !msg.contains("stolen-code"),
        "the refusal must not echo the code it refused: {msg}"
    );
}

#[tokio::test]
async fn a_provider_error_in_the_callback_is_surfaced_verbatim_in_its_safe_fields() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let uri = redirect.redirect_uri().to_string();
    let state = AuthState::generate();
    let state_value = state.as_str().to_string();

    std::thread::spawn(move || {
        let _ = ureq_get(&format!(
            "{uri}/?error=access_denied&error_description=The%20user%20declined&state={state_value}"
        ));
    });

    let err = redirect
        .wait_for_code(&state, Duration::from_secs(5))
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("access_denied"),
        "the provider's error code must reach the user, got: {msg}"
    );
}

#[tokio::test]
async fn a_callback_that_never_arrives_times_out_loudly() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let state = AuthState::generate();

    let err = redirect
        .wait_for_code(&state, Duration::from_millis(150))
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("timed out") || msg.to_lowercase().contains("timeout"),
        "a silent listener must report a timeout, not hang or succeed emptily, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// The token exchange
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_exchange_sends_the_pkce_verifier_and_returns_the_refresh_token() {
    let mock = start_mock().await;
    let pkce = Pkce::generate();

    let token = holon_mcp_client::oauth_bootstrap::exchange_code(
        &reqwest::Client::new(),
        &mock.token_url,
        FIXTURE_CLIENT_ID,
        FIXTURE_CLIENT_SECRET,
        "auth-code-1",
        "http://127.0.0.1:41234",
        &pkce,
    )
    .await
    .unwrap();

    assert_eq!(token.expose(), MINTED_REFRESH_TOKEN);

    let body = mock.last_token_body();
    assert!(
        body.contains("grant_type=authorization_code"),
        "body: {body}"
    );
    assert!(body.contains("code=auth-code-1"), "body: {body}");
    assert!(
        body.contains(&format!("code_verifier={}", pkce.verifier())),
        "the exchange must carry the PKCE verifier, body: {body}"
    );
}

/// Google returns no `refresh_token` when the user has already consented and
/// the request did not force a fresh grant. Silently storing the access token
/// instead would produce an integration that dies in an hour.
#[tokio::test]
async fn a_response_without_a_refresh_token_fails_loud_and_says_how_to_recover() {
    let mock = start_mock().await;
    mock.with(|s| s.omit_refresh_token = true);

    let err = holon_mcp_client::oauth_bootstrap::exchange_code(
        &reqwest::Client::new(),
        &mock.token_url,
        FIXTURE_CLIENT_ID,
        FIXTURE_CLIENT_SECRET,
        "auth-code-1",
        "http://127.0.0.1:41234",
        &Pkce::generate(),
    )
    .await
    .unwrap_err();

    let msg = format!("{err:#}");
    assert!(
        msg.contains("refresh_token"),
        "the failure must name what was missing, got: {msg}"
    );
    assert!(
        !msg.contains("access-only"),
        "a success-shaped body carries tokens and must never be echoed: {msg}"
    );
}

#[tokio::test]
async fn a_token_endpoint_error_surfaces_only_its_safe_fields() {
    let mock = start_mock().await;
    mock.with(|s| {
        s.canned_failure = Some((
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"Bad code","access_token":"leaked-should-not-appear"}"#
                .to_string(),
        ))
    });

    let err = holon_mcp_client::oauth_bootstrap::exchange_code(
        &reqwest::Client::new(),
        &mock.token_url,
        FIXTURE_CLIENT_ID,
        FIXTURE_CLIENT_SECRET,
        "auth-code-1",
        "http://127.0.0.1:41234",
        &Pkce::generate(),
    )
    .await
    .unwrap_err();

    let msg = format!("{err:#}");
    assert!(msg.contains("invalid_grant"), "got: {msg}");
    assert!(
        !msg.contains("leaked-should-not-appear"),
        "the raw body must never be echoed: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Where secrets land
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_credential_ref_records_the_keychain_arm_when_the_sidecar_declares_it() {
    // The client id/secret DO have a keychain arm, and the flow records where
    // they live even though it never writes them.
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config_for(dir.path(), "https://provider.example/token");
    cfg.client_secret_file = None;
    cfg.client_secret_keychain = Some(KeychainRef {
        service: "space.holon.test".to_string(),
        account: "client-secret".to_string(),
    });

    let recorded = holon_mcp_client::oauth_bootstrap::recorded_credentials(
        &cfg,
        &CredentialRoot::new(dir.path()),
    )
    .unwrap();
    assert_eq!(
        recorded.client_secret,
        CredentialRef::Keychain {
            service: "space.holon.test".to_string(),
            account: "client-secret".to_string(),
        }
    );
}

// ---------------------------------------------------------------------------
// The whole flow
// ---------------------------------------------------------------------------

/// The end-to-end success path with the browser hop stubbed: the flow opens a
/// URL, the "browser" delivers the provider's redirect to the loopback, the
/// code is exchanged, the refresh token lands where the sidecar says, and the
/// state file flips to enabled + Configured.
#[tokio::test]
async fn a_completed_flow_writes_the_refresh_token_and_records_configured_state() {
    let creds_dir = tempfile::tempdir().unwrap();
    provision_client_credentials(creds_dir.path());
    let mock = start_mock().await;
    let cfg = config_for(creds_dir.path(), &mock.token_url);

    let dir = store_dir();
    let store = IntegrationConfigStore::load(dir.path()).unwrap();

    let browser = RecordingBrowser::driving(|url| {
        let redirect = query_param(url, "redirect_uri").expect("redirect_uri on the auth request");
        let state = query_param(url, "state").expect("state on the auth request");
        format!("{redirect}/?code=auth-code-1&state={state}")
    });

    holon_mcp_client::oauth_bootstrap::configure_integration(
        "gcal",
        &cfg,
        &store,
        &no_env_lookup,
        &browser,
        Duration::from_secs(10),
        &CredentialRoot::new(creds_dir.path()),
    )
    .await
    .unwrap();

    assert_eq!(browser.opened_urls().len(), 1, "exactly one browser hop");
    assert_eq!(mock.token_post_count(), 1, "exactly one code exchange");

    let refresh_path = creds_dir.path().join("refresh-token");
    assert_eq!(
        std::fs::read_to_string(&refresh_path).unwrap().trim(),
        MINTED_REFRESH_TOKEN,
        "the refresh token must land at the sidecar-declared location"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&refresh_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the token file must be 0600, got {mode:o}");
    }
    // The replace goes through a temporary sibling; a leftover one would be a
    // second copy of the refresh token sitting in the credentials directory.
    let residue: Vec<_> = std::fs::read_dir(creds_dir.path())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(
        residue.is_empty(),
        "the atomic replace must leave no temporary copy of the token behind, found: {residue:?}"
    );

    let state = store.get("gcal").unwrap();
    assert!(
        state.enabled,
        "a completed flow switches the integration on"
    );
    match state.configuration {
        Configuration::Configured(creds) => {
            assert_eq!(
                creds.client_id,
                CredentialRef::File {
                    path: creds_dir.path().join("client-id")
                }
            );
            assert_eq!(
                creds.client_secret,
                CredentialRef::File {
                    path: creds_dir.path().join("client-secret")
                }
            );
            assert_eq!(creds.refresh_token_file, refresh_path);
        }
        other => panic!("expected Configured, got {other:?}"),
    }
}

/// The state file is plain-text user config. It records LOCATIONS; a secret in
/// it would be a secret at mode 0644 in the integrations directory.
#[tokio::test]
async fn the_state_file_records_locations_and_never_secret_material() {
    let creds_dir = tempfile::tempdir().unwrap();
    provision_client_credentials(creds_dir.path());
    let mock = start_mock().await;
    let cfg = config_for(creds_dir.path(), &mock.token_url);

    let dir = store_dir();
    let store = IntegrationConfigStore::load(dir.path()).unwrap();
    let browser = RecordingBrowser::driving(|url| {
        let redirect = query_param(url, "redirect_uri").unwrap();
        let state = query_param(url, "state").unwrap();
        format!("{redirect}/?code=auth-code-1&state={state}")
    });

    holon_mcp_client::oauth_bootstrap::configure_integration(
        "gcal",
        &cfg,
        &store,
        &no_env_lookup,
        &browser,
        Duration::from_secs(10),
        &CredentialRoot::new(creds_dir.path()),
    )
    .await
    .unwrap();

    let text = std::fs::read_to_string(store.state_path("gcal").unwrap()).unwrap();
    assert!(
        !text.contains(MINTED_REFRESH_TOKEN),
        "the refresh token reached the state file: {text}"
    );
    assert!(
        !text.contains(FIXTURE_CLIENT_SECRET),
        "the client secret reached the state file: {text}"
    );
    assert!(
        text.contains("client-secret"),
        "the state file must still record WHERE the secret lives: {text}"
    );
}

/// Nothing is recorded when the flow fails. A `Configured` state written before
/// the token landed would claim a configuration the next launch cannot resolve.
#[tokio::test]
async fn a_failed_flow_records_nothing() {
    let creds_dir = tempfile::tempdir().unwrap();
    provision_client_credentials(creds_dir.path());
    let mock = start_mock().await;
    mock.with(|s| {
        s.canned_failure = Some((
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"Bad code"}"#.to_string(),
        ))
    });
    let cfg = config_for(creds_dir.path(), &mock.token_url);

    let dir = store_dir();
    let store = IntegrationConfigStore::load(dir.path()).unwrap();
    let browser = RecordingBrowser::driving(|url| {
        let redirect = query_param(url, "redirect_uri").unwrap();
        let state = query_param(url, "state").unwrap();
        format!("{redirect}/?code=auth-code-1&state={state}")
    });

    let err = holon_mcp_client::oauth_bootstrap::configure_integration(
        "gcal",
        &cfg,
        &store,
        &no_env_lookup,
        &browser,
        Duration::from_secs(10),
        &CredentialRoot::new(creds_dir.path()),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("invalid_grant"));

    assert_eq!(
        store.get("gcal").unwrap().configuration,
        Configuration::Unconfigured,
        "a failed flow must leave the configuration axis untouched"
    );
    assert!(
        !creds_dir.path().join("refresh-token").exists(),
        "a failed flow must not leave a credential file behind"
    );
}

/// The flow reads the client credentials, it does not mint them. An integration
/// whose OAuth client was never provisioned must be told that, not sent to a
/// consent page that will reject an empty client id.
#[tokio::test]
async fn a_flow_without_provisioned_client_credentials_refuses_before_opening_a_browser() {
    let creds_dir = tempfile::tempdir().unwrap();
    // Deliberately NOT provisioned.
    let mock = start_mock().await;
    let cfg = config_for(creds_dir.path(), &mock.token_url);

    let dir = store_dir();
    let store = IntegrationConfigStore::load(dir.path()).unwrap();
    let browser = RecordingBrowser::recording_only();

    let err = holon_mcp_client::oauth_bootstrap::configure_integration(
        "gcal",
        &cfg,
        &store,
        &no_env_lookup,
        &browser,
        Duration::from_secs(10),
        &CredentialRoot::new(creds_dir.path()),
    )
    .await
    .unwrap_err();

    assert!(
        browser.opened_urls().is_empty(),
        "no browser hop may happen before the client credentials resolve"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("client_id") || msg.contains("client-id"),
        "the refusal must name the unresolved credential, got: {msg}"
    );
    assert_eq!(mock.token_post_count(), 0);
}

// ===========================================================================
// Round 2 — regressions found by adversarial verification.
//
// Each case here failed BEHAVIOURALLY against the code as first written (a
// hang, a panic, a cleartext send, a destroyed credential, a false match), not
// at a stub boundary.
// ===========================================================================

/// D1. The timeout must bound the WHOLE wait, not just `accept()`.
///
/// A peer that connects and sends nothing pinned the flow forever: the timeout
/// wrapped only `accept()`, and the byte-at-a-time read that followed had no
/// deadline. No attacker is needed — browsers speculatively preconnect.
#[tokio::test]
async fn a_connected_but_silent_peer_cannot_pin_the_flow_past_its_timeout() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let authority = redirect
        .redirect_uri()
        .trim_start_matches("http://")
        .to_string();
    let state = AuthState::generate();

    // Connect and send NOTHING, holding the socket open for the whole test.
    let silent = std::net::TcpStream::connect(&authority).expect("preconnect");

    let started = std::time::Instant::now();
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        redirect.wait_for_code(&state, Duration::from_millis(400)),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "wait_for_code did not return within 10s despite a 400ms timeout — a connected but \
             silent peer pins the consent flow forever"
        )
    })
    .expect_err("a silent peer must not yield a code");

    drop(silent);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait must be bounded by its own timeout, took {:?}",
        started.elapsed()
    );
    assert!(
        format!("{err:#}").to_lowercase().contains("time"),
        "the failure must read as a timeout, got: {err:#}"
    );
}

/// D1b. A preconnect must not consume the one accept the real callback needs.
///
/// The listener accepted exactly once, so an empty speculative connection ate
/// the flow's only chance and the genuine redirect was never read.
#[tokio::test]
async fn a_speculative_preconnect_does_not_swallow_the_real_callback() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let uri = redirect.redirect_uri().to_string();
    let authority = uri.trim_start_matches("http://").to_string();
    let state = AuthState::generate();
    let state_value = state.as_str().to_string();

    std::thread::spawn(move || {
        // A browser preconnect: open, send nothing, close.
        drop(std::net::TcpStream::connect(&authority).expect("preconnect"));
        std::thread::sleep(std::time::Duration::from_millis(120));
        let _ = ureq_get(&format!("{uri}/?code=real-code&state={state_value}"));
    });

    let code = tokio::time::timeout(
        Duration::from_secs(10),
        redirect.wait_for_code(&state, Duration::from_secs(5)),
    )
    .await
    .expect("wait_for_code must not hang after a preconnect")
    .expect("the real callback must still be accepted after a preconnect");

    assert_eq!(code, "real-code");
}

/// D2. A sidecar is user-editable config, so a malformed `auth_url` must be a
/// loud refusal at parse time — not a panic deep in URL building.
///
/// The panic happened on the flow's own thread, so nothing ever wrote the
/// progress cell and the row sat on "Waiting…" forever: silent degradation.
#[test]
fn a_malformed_auth_url_is_refused_when_the_config_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config_for(dir.path(), "https://provider.example/token");
    cfg.auth_url = Some("not a url".to_string());

    let err = AuthorizationRequest::from_config(&cfg)
        .expect_err("a malformed auth_url must be refused, not carried to a panic site");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("auth_url"),
        "the refusal must name the field, got: {msg}"
    );
}

/// D3. Both OAuth endpoints must be TLS. A cleartext `token_url` puts the
/// client secret, the authorization code and the PKCE verifier on the wire in
/// the clear; a cleartext `auth_url` sends the user to a consent page an
/// on-path attacker can rewrite.
#[test]
fn a_cleartext_auth_url_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config_for(dir.path(), "https://provider.example/token");
    cfg.auth_url = Some("http://provider.example/authorize".to_string());

    let err = AuthorizationRequest::from_config(&cfg).expect_err("cleartext auth_url must refuse");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("https"),
        "the refusal must say TLS is required, got: {msg}"
    );
}

#[tokio::test]
async fn a_cleartext_token_url_is_refused_before_any_secret_is_sent() {
    let mock = start_mock().await;
    // The mock is plain http on a NON-loopback-looking host name, which is what
    // a real misconfiguration looks like.
    let cleartext = mock.token_url.replace("127.0.0.1", "localhost.example");

    let err = holon_mcp_client::oauth_bootstrap::exchange_code(
        &reqwest::Client::new(),
        &cleartext,
        FIXTURE_CLIENT_ID,
        FIXTURE_CLIENT_SECRET,
        "auth-code-1",
        "http://127.0.0.1:41234",
        &Pkce::generate(),
    )
    .await
    .expect_err("a cleartext token endpoint must be refused");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("https"),
        "the refusal must say TLS is required, got: {msg}"
    );
    assert_eq!(
        mock.token_post_count(),
        0,
        "nothing may be sent to a cleartext token endpoint"
    );
}

/// D5a. The refused-callback leg of "a failed flow records nothing".
#[tokio::test]
async fn a_refused_callback_records_nothing() {
    let creds_dir = tempfile::tempdir().unwrap();
    provision_client_credentials(creds_dir.path());
    let mock = start_mock().await;
    let cfg = config_for(creds_dir.path(), &mock.token_url);

    let dir = store_dir();
    let store = IntegrationConfigStore::load(dir.path()).unwrap();
    // The provider answers with an error instead of a code.
    let browser = RecordingBrowser::driving(|url| {
        let redirect = query_param(url, "redirect_uri").unwrap();
        let state = query_param(url, "state").unwrap();
        format!("{redirect}/?error=access_denied&state={state}")
    });

    let err = holon_mcp_client::oauth_bootstrap::configure_integration(
        "gcal",
        &cfg,
        &store,
        &no_env_lookup,
        &browser,
        Duration::from_secs(10),
        &CredentialRoot::new(creds_dir.path()),
    )
    .await
    .expect_err("a refused callback must fail the flow");
    assert!(format!("{err:#}").contains("access_denied"));

    assert_eq!(mock.token_post_count(), 0, "no code, no exchange");
    assert_eq!(
        store.get("gcal").unwrap().configuration,
        Configuration::Unconfigured
    );
    assert!(!creds_dir.path().join("refresh-token").exists());
}

/// D5b. A failed WRITE must not destroy the refresh token the user already
/// had.
///
/// The replace unlinked the existing file before creating the new one, so a
/// create/write failure after a successful unlink left the user with NO token
/// and a state file still claiming `Configured` — strictly worse than before
/// the flow ran.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_write_preserves_the_previous_refresh_token() {
    let creds_dir = tempfile::tempdir().unwrap();
    provision_client_credentials(creds_dir.path());
    let mock = start_mock().await;
    let mut cfg = config_for(creds_dir.path(), &mock.token_url);

    // A pre-existing token from an earlier, working configuration.
    let token_path = creds_dir.path().join("locked").join("refresh-token");
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    write_private(&token_path, "PREVIOUS-TOKEN-DO-NOT-DESTROY");
    cfg.refresh_token_file = token_path.to_string_lossy().into_owned();

    // Make the directory unwritable so the new file cannot be created.
    let dir_perm_target = token_path.parent().unwrap().to_path_buf();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir_perm_target, std::fs::Permissions::from_mode(0o500)).unwrap();
    }

    let store_dir = store_dir();
    let store = IntegrationConfigStore::load(store_dir.path()).unwrap();
    let browser = RecordingBrowser::driving(|url| {
        let redirect = query_param(url, "redirect_uri").unwrap();
        let state = query_param(url, "state").unwrap();
        format!("{redirect}/?code=auth-code-1&state={state}")
    });

    let outcome = holon_mcp_client::oauth_bootstrap::configure_integration(
        "gcal",
        &cfg,
        &store,
        &no_env_lookup,
        &browser,
        Duration::from_secs(10),
        &CredentialRoot::new(creds_dir.path()),
    )
    .await;

    // Restore permissions before asserting so the tempdir can clean up.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir_perm_target, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    outcome.expect_err("an unwritable destination must fail the flow");
    assert_eq!(
        std::fs::read_to_string(&token_path).unwrap().trim(),
        "PREVIOUS-TOKEN-DO-NOT-DESTROY",
        "a failed write must leave the user's working refresh token intact"
    );
    assert_eq!(
        store.get("gcal").unwrap().configuration,
        Configuration::Unconfigured,
        "a failed write must not record a configuration"
    );
}

/// D8. The `state` guard must reject a candidate that differs in length.
///
/// The length term was folded in as `(a.len() ^ b.len()) as u8`, which
/// truncates: a candidate NUL-padded to exactly 256 bytes longer compared
/// EQUAL, because the byte loop zero-pads the shorter side too.
#[tokio::test]
async fn a_nul_padded_state_does_not_match() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let uri = redirect.redirect_uri().to_string();
    let state = AuthState::generate();
    // 43-char verifier-shaped nonce + 256 NULs = a length difference of exactly
    // 256, the value the truncating XOR collapsed to zero.
    let padded: String = format!("{}{}", state.as_str(), "%00".repeat(256));

    std::thread::spawn(move || {
        let _ = ureq_get(&format!("{uri}/?code=stolen&state={padded}"));
    });

    let err = redirect
        .wait_for_code(&state, Duration::from_secs(5))
        .await
        .expect_err("a state of a different length must never match");
    assert!(
        format!("{err:#}").to_lowercase().contains("state"),
        "the refusal must name the state mismatch, got: {err:#}"
    );
}

// ===========================================================================
// Round 3 — TLS-check residuals found by delta re-probe.
// ===========================================================================

/// R3-1. A cleartext `token_url` must be refused BEFORE the browser opens.
///
/// The scheme check lived only inside `exchange_code`, which runs after
/// consent: the user was sent to a real consent page, granted real access, and
/// only then hit the refusal — then watched the flow sit waiting for a redirect
/// that could never complete. A consent is not a free resource; some providers
/// will not issue a second refresh token without a manual revoke.
#[tokio::test]
async fn a_cleartext_token_url_is_refused_before_the_browser_opens() {
    let creds_dir = tempfile::tempdir().unwrap();
    provision_client_credentials(creds_dir.path());
    let mut cfg = config_for(creds_dir.path(), "https://provider.example/token");
    // A non-loopback cleartext endpoint: what a real misconfiguration looks like.
    cfg.token_url = "http://provider.example/token".to_string();

    let dir = store_dir();
    let store = IntegrationConfigStore::load(dir.path()).unwrap();
    let browser = RecordingBrowser::recording_only();

    let err = holon_mcp_client::oauth_bootstrap::configure_integration(
        "gcal",
        &cfg,
        &store,
        &no_env_lookup,
        &browser,
        Duration::from_secs(5),
        &CredentialRoot::new(creds_dir.path()),
    )
    .await
    .expect_err("a cleartext token endpoint must fail the flow");

    assert!(
        browser.opened_urls().is_empty(),
        "the browser was opened {} time(s) before the cleartext token_url was refused — the user \
         burns a real consent on a flow that cannot complete",
        browser.opened_urls().len()
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("https") && msg.contains("token_url"),
        "the refusal must name the endpoint and say TLS is required, got: {msg}"
    );
    assert_eq!(
        store.get("gcal").unwrap().configuration,
        Configuration::Unconfigured
    );
}

/// R3-2. The loopback exemption must cover IPv6.
///
/// The predicate compared the host STRING, and `Url::host_str` serializes an
/// IPv6 host with its brackets (`[::1]`), so the literal `"::1"` arm never
/// matched and a local IPv6 test provider was refused — contradicting the
/// documented exemption.
#[test]
fn the_loopback_exemption_covers_ipv6() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config_for(dir.path(), "https://provider.example/token");
    cfg.auth_url = Some("http://[::1]:9/auth".to_string());

    AuthorizationRequest::from_config(&cfg).unwrap_or_else(|e| {
        panic!("loopback [::1] must be exempt from the TLS requirement, got: {e:#}")
    });
}

/// …and the exemption must stay exactly that narrow. Hostile lookalikes must
/// still be refused: a host is loopback because it resolves to this machine,
/// not because it happens to start with the right characters.
#[test]
fn the_loopback_exemption_does_not_leak_to_lookalikes() {
    let dir = tempfile::tempdir().unwrap();
    for hostile in [
        "http://localhost.evil.com/auth",
        "http://127.0.0.1.evil.com/auth",
        "http://notlocalhost/auth",
        "http://evil.com/auth",
        "http://localhost@evil.com/auth",
    ] {
        let mut cfg = config_for(dir.path(), "https://provider.example/token");
        cfg.auth_url = Some(hostile.to_string());
        let err = AuthorizationRequest::from_config(&cfg)
            .expect_err("a non-loopback cleartext host must be refused: {hostile}");
        assert!(
            format!("{err:#}").contains("https"),
            "{hostile} must be refused for lack of TLS"
        );
    }

    // The genuine loopback forms all stay exempt.
    for loopback in [
        "http://127.0.0.1:9/auth",
        "http://localhost:9/auth",
        "http://[::1]:9/auth",
    ] {
        let mut cfg = config_for(dir.path(), "https://provider.example/token");
        cfg.auth_url = Some(loopback.to_string());
        AuthorizationRequest::from_config(&cfg)
            .unwrap_or_else(|e| panic!("{loopback} must be exempt, got: {e:#}"));
    }
}
