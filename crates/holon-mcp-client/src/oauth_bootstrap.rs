//! The in-app OAuth2 consent flow: the one-time dance that turns an integration
//! whose OAuth client exists but has never been consented into a `Configured`
//! one.
//!
//! Shape (RFC 8252, "OAuth 2.0 for Native Apps"): the system browser is sent to
//! the provider's authorization endpoint with a loopback redirect back to an
//! ephemeral `http://127.0.0.1:<port>` this process is listening on, PKCE binds
//! the returned code to this flow instance, and a `state` nonce binds the
//! callback to this request. The code is exchanged for a long-lived refresh
//! token, which is written to the location the SIDECAR declares.
//!
//! What this flow does NOT do: create the provider's OAuth client, or mint the
//! client id/secret. Those come from the provider's console and are already
//! resolved through the sidecar's declared credential arms — this flow reads
//! them, and writes only the refresh token. That split is why the flow can be
//! generic: everything provider-specific lives in the sidecar.
//!
//! # Why this is not the MCP OAuth flow
//!
//! Holon has a *second*, unrelated OAuth implementation: the MCP spec's OAuth
//! 2.1 for HTTP-transport MCP servers, where rmcp's `AuthorizationManager` owns
//! discovery (RFC 9728), dynamic client registration (RFC 7591), PKCE and the
//! exchange, and tokens land in the Turso `mcp_oauth_credentials` table via
//! [`crate::credential_store`] — see [`crate::mcp_integration`]'s
//! `McpConnectionResult::NeedsAuth` / `PendingOAuthFlows::complete_oauth`.
//!
//! That machinery cannot serve this surface, for two reasons that are about
//! kind rather than effort:
//!
//! - **The providers are not MCP servers.** `gcal`/`gmail` are plain REST APIs
//!   on the `rest` transport. Google publishes no protected-resource metadata
//!   and offers no dynamic client registration, so the discovery the MCP flow
//!   is built around has nothing to discover.
//! - **It is a connect-time flow, not a configure-time one.** `complete_oauth`
//!   returns a live, fully-wired `McpIntegration` and needs the whole runtime
//!   stack (`DbHandle`, `CacheFactory`, `SyncTokenStore`, `SyncGate`). A
//!   settings screen configures a provider that is switched OFF and will not
//!   connect until the next launch; there is no integration to return.
//!
//! So the two stay separate, and no third credential store is introduced: this
//! flow writes where the `rest` transport already READS (the sidecar's declared
//! file/keychain arm), and the MCP flow writes where rmcp already reads. A
//! future unified Configure affordance branches on the transport family at
//! [`crate::integration_config::IntegrationFileConfig`] and reuses both
//! backends unchanged — no bundled provider needs the MCP arm today
//! (`claude-history` is stdio, `todoist` declares `oauth: false`).
//!
//! # Security invariants (audited)
//!
//! - **The sidecar decides where secrets live.** The runtime resolves OAuth
//!   credentials from the sidecar's arms alone
//!   ([`crate::integration_config::RestAuthConfig::resolve`]); the state file
//!   is a record. So this flow writes to the sidecar-declared location and
//!   records that same location. Writing anywhere else would leave a
//!   `Configured` badge over an integration the next launch cannot resolve.
//! - **No secret is ever logged.** The verifier, the state nonce and the
//!   refresh token have redacting `Debug`s; token-endpoint failures surface
//!   only the standard `error`/`error_description` fields; a 2xx body is never
//!   echoed.
//! - **Nothing is recorded on failure.** The state write is the last step, so a
//!   refused callback or a failed exchange leaves both axes untouched.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use rand::RngCore as _;
use sha2::Digest as _;
use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWriteExt as _;

use crate::integration_state::CredentialRef;
use crate::integration_state::Credentials;
use crate::integration_state::IntegrationState;
use crate::rest_oauth2::KeychainRef;
use crate::rest_oauth2::RestOAuth2Config;

/// 32 bytes of CSPRNG output, base64url-encoded without padding — the shape
/// both the PKCE verifier and the `state` nonce want (43 unreserved characters,
/// 256 bits of entropy).
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A PKCE (RFC 7636) `S256` verifier/challenge pair.
///
/// The verifier is secret material for the lifetime of one flow: it is sent
/// only on the token exchange, and it is what makes an intercepted
/// authorization code useless to whoever intercepted it.
pub struct Pkce {
    verifier: String,
    challenge: String,
}

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

impl Pkce {
    /// A fresh pair from the OS CSPRNG.
    pub fn generate() -> Self {
        let verifier = random_token();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }

    /// The `code_challenge` sent on the authorization request (public).
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    /// The `code_verifier` sent on the token exchange (secret).
    pub fn verifier(&self) -> &str {
        &self.verifier
    }
}

/// The CSRF `state` nonce: generated per flow, echoed by the provider, and
/// compared before anything else in the callback is believed.
pub struct AuthState(String);

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthState(<redacted>)")
    }
}

impl AuthState {
    pub fn generate() -> Self {
        Self(random_token())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length-independent, byte-wise comparison. The nonce is 256 bits and the
    /// comparison is local, so this is belt-and-braces rather than a defence
    /// against a practical timing oracle — but a short-circuiting `==` on a
    /// security check is the kind of thing that gets copied somewhere it does
    /// matter.
    fn matches(&self, candidate: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), candidate.as_bytes());
        // A plain `!=` flag rather than folding the lengths in with `as u8`: the
        // cast truncated, so a candidate padded to exactly 256 bytes longer
        // contributed 0 and — since the byte loop zero-pads the shorter side —
        // a NUL-padded nonce compared EQUAL.
        let mut diff: u8 = u8::from(a.len() != b.len());
        for i in 0..a.len().max(b.len()) {
            diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
        }
        diff == 0
    }
}

/// A long-lived refresh token. Newtype so it cannot be logged by accident: the
/// only ways out are [`Self::expose`] (used at the single write site) and a
/// redacting `Debug`.
pub struct RefreshToken(String);

impl std::fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RefreshToken(<redacted>)")
    }
}

impl RefreshToken {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Sends the user's system browser to a URL.
///
/// A seam rather than a direct call so every test can run the whole flow
/// without launching anything, and so a frontend that owns a better opener can
/// supply it.
pub trait BrowserOpener: Send + Sync {
    fn open(&self, url: &str) -> anyhow::Result<()>;
}

/// The production opener: hands the URL to the desktop's own URL handler.
///
/// RFC 8252 §8.12 requires the system browser rather than an embedded webview —
/// the user must be able to see the address bar and reuse their existing
/// session, and an embedded view would give this process the credentials it is
/// specifically not supposed to see.
pub struct SystemBrowser;

impl BrowserOpener for SystemBrowser {
    fn open(&self, url: &str) -> anyhow::Result<()> {
        let mut command = if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("open");
            c.arg(url);
            c
        } else if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("rundll32");
            c.args(["url.dll,FileProtocolHandler", url]);
            c
        } else {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(url);
            c
        };

        let status = command
            .status()
            .context("could not launch the system browser for the consent page")?;
        anyhow::ensure!(
            status.success(),
            "the system browser launcher exited with {status}. Open the consent page manually to \
             continue."
        );
        Ok(())
    }
}

/// Everything the authorization request needs, extracted from a sidecar's
/// `oauth2` block. Parsed once so the flow never re-derives it.
#[derive(Debug)]
pub struct AuthorizationRequest {
    /// Parsed and scheme-checked at construction, so building the URL later
    /// cannot fail. A sidecar is user-editable config: carrying its `auth_url`
    /// as an unparsed string to a `.expect()` deeper in the flow turned a typo
    /// into a panic on the flow's own thread, which left the row waiting
    /// forever with nothing to show.
    pub auth_url: reqwest::Url,
    pub scopes: Vec<String>,
    pub extra_params: HashMap<String, String>,
}

/// Whether `url`'s host is this machine, so cleartext never leaves it.
///
/// Decided on the PARSED host, not the host string: `Url::host_str` serializes
/// an IPv6 host with its brackets (`[::1]`), so a literal string comparison
/// silently excluded every IPv6 loopback. Parsing also keeps the exemption
/// exact — `localhost.evil.com` and `127.0.0.1.evil.com` are ordinary domains
/// that resolve wherever their owner points them, and neither is loopback.
fn is_loopback_host(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        // The one name every resolver is required to map to loopback.
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Parse an OAuth endpoint and require TLS.
///
/// A cleartext `token_url` would put the client secret, the authorization code
/// and the PKCE verifier on the wire in the clear; a cleartext `auth_url` sends
/// the user to a consent page an on-path attacker can rewrite. Loopback is the
/// documented exception (RFC 8252 §7.3 mandates `http` for the loopback
/// redirect, and local mock servers are how this flow is tested) — it never
/// leaves the machine.
pub(crate) fn parse_secure_endpoint(field: &str, raw: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|e| {
        anyhow::anyhow!("the sidecar's `{field}` is not a valid URL ({e}): {raw:?}")
    })?;
    anyhow::ensure!(
        url.scheme() == "https" || is_loopback_host(&url),
        "the sidecar's `{field}` uses {}://, but OAuth endpoints must use https — a cleartext \
         endpoint exposes the client secret, the authorization code and the PKCE verifier to \
         anyone on the path. (Loopback addresses are the only exception.)",
        url.scheme()
    );
    Ok(url)
}

impl AuthorizationRequest {
    /// Read the authorization half of a sidecar's `oauth2` block.
    ///
    /// Fails loud when `auth_url` is absent: a sidecar that declares only a
    /// `token_url` can refresh an existing token but cannot obtain one, and
    /// offering the user a Configure button that dead-ends is worse than saying
    /// the sidecar is missing the field.
    pub fn from_config(cfg: &RestOAuth2Config) -> anyhow::Result<Self> {
        let auth_url = cfg
            .auth_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "this integration's sidecar declares no `transport.rest.auth.oauth2.auth_url`, \
                     so the consent flow has no authorization endpoint to send the browser to. Add \
                     the provider's authorization endpoint to the sidecar."
                )
            })?;
        anyhow::ensure!(
            !cfg.scopes.is_empty(),
            "this integration's sidecar declares no `transport.rest.auth.oauth2.scopes`, so the \
             consent flow would ask for no access at all."
        );
        Ok(Self {
            auth_url: parse_secure_endpoint("auth_url", auth_url)?,
            scopes: cfg.scopes.clone(),
            extra_params: cfg.auth_params.clone(),
        })
    }

    /// The full authorization URL to open in the browser.
    pub fn url(
        &self,
        client_id: &str,
        redirect_uri: &str,
        state: &AuthState,
        pkce: &Pkce,
    ) -> String {
        let mut url = self.auth_url.clone();
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("response_type", "code")
                .append_pair("client_id", client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("scope", &self.scopes.join(" "))
                .append_pair("state", state.as_str())
                .append_pair("code_challenge", pkce.challenge())
                .append_pair("code_challenge_method", "S256");
            // Sorted so the URL is stable across runs — an unordered map would
            // otherwise make every log line and every test look different.
            let mut extra: Vec<_> = self.extra_params.iter().collect();
            extra.sort();
            for (k, v) in extra {
                q.append_pair(k, v);
            }
        }
        url.to_string()
    }
}

/// A one-shot loopback listener bound to `127.0.0.1` on an ephemeral port.
pub struct LoopbackRedirect {
    listener: tokio::net::TcpListener,
    redirect_uri: String,
}

impl LoopbackRedirect {
    /// Bind `127.0.0.1:0`.
    ///
    /// Port 0 (rather than a fixed port) is a security property, not a
    /// convenience: an attacker cannot pre-bind a port whose number is not
    /// chosen until the flow starts. Binding the loopback address rather than
    /// `0.0.0.0` keeps the callback off the LAN.
    pub async fn bind() -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("could not bind a loopback listener for the OAuth redirect")?;
        let port = listener
            .local_addr()
            .context("the loopback listener has no local address")?
            .port();
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}"),
        })
    }

    /// The `redirect_uri` to send on the authorization request.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Accept exactly one request, answer it with a human-readable page, and
    /// return its authorization code.
    ///
    /// Refuses, without returning a code, when: the `state` does not match
    /// (possible CSRF or a hijacked redirect), the provider reported an error,
    /// no code is present, or nothing arrives within `timeout`.
    pub async fn wait_for_code(
        self,
        expected_state: &AuthState,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let redirect_uri = self.redirect_uri.clone();
        let deadline = tokio::time::Instant::now() + timeout;
        let timed_out = || {
            anyhow::anyhow!(
                "timed out after {}s waiting for the authorization redirect on {redirect_uri}. The \
                 consent page was opened in your browser — finish or retry it.",
                timeout.as_secs()
            )
        };

        // Accept in a LOOP until the deadline, not once. Browsers speculatively
        // preconnect: an empty connection that consumed the single accept used
        // to leave the flow waiting forever for a callback it could no longer
        // receive. Each connection also gets its own read budget, so one silent
        // peer cannot spend the whole consent window either.
        let (mut sock, target) = loop {
            let accepted = tokio::time::timeout_at(deadline, self.listener.accept())
                .await
                .map_err(|_| timed_out())?
                .context("the loopback listener failed to accept the redirect")?;
            let (mut sock, _) = accepted;

            let read_budget =
                deadline.min(tokio::time::Instant::now() + PER_CONNECTION_READ_BUDGET);
            match tokio::time::timeout_at(read_budget, read_request_target(&mut sock)).await {
                // A request line arrived: this is the callback, for good or ill.
                Ok(Ok(Some(target))) => break (sock, target),
                // Connected and closed without saying anything — a preconnect.
                // Keep waiting for the real redirect.
                Ok(Ok(None)) => continue,
                // A malformed or oversized request is not something to keep
                // waiting through; it is refused loudly.
                Ok(Err(e)) => return Err(e),
                // This peer went quiet. Drop it and keep the window open for the
                // genuine callback, unless the whole window is gone.
                Err(_) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(timed_out());
                    }
                    continue;
                }
            }
        };

        let outcome = parse_callback(&target, expected_state);

        // The browser tab is answered either way: a user staring at a hung tab
        // has no idea the desktop app already refused the callback.
        let body = match &outcome {
            Ok(_) => "Holon: authorization received. You can close this tab.",
            Err(_) => "Holon: this authorization was REFUSED. Return to Holon for the reason.",
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: \
             {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(response.as_bytes()).await;
        let _ = sock.flush().await;

        outcome
    }
}

/// How long any ONE connection may stay silent before it is dropped and the
/// listener goes back to waiting. A speculative preconnect must not be able to
/// spend the user's whole consent window.
const PER_CONNECTION_READ_BUDGET: Duration = Duration::from_secs(10);

/// Read the request target (the path+query of the first request line).
///
/// `Ok(None)` means the peer closed without sending anything — a speculative
/// preconnect, not a callback. Distinguishing that from a real request is what
/// lets the caller keep waiting instead of failing.
///
/// Bounded read: a loopback redirect is a short GET, and an unbounded read from
/// a socket anything on the machine may connect to is a denial-of-service hole.
async fn read_request_target(sock: &mut tokio::net::TcpStream) -> anyhow::Result<Option<String>> {
    const MAX_REQUEST_LINE: usize = 8 * 1024;

    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = sock
            .read(&mut byte)
            .await
            .context("could not read the authorization redirect from the loopback socket")?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            anyhow::bail!("the authorization redirect closed mid-request-line");
        }
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            buf.push(byte[0]);
        }
        anyhow::ensure!(
            buf.len() <= MAX_REQUEST_LINE,
            "the authorization redirect sent an oversized request line ({MAX_REQUEST_LINE} byte \
             limit) — refusing it"
        );
    }

    let line = String::from_utf8(buf)
        .context("the authorization redirect's request line was not valid UTF-8")?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    anyhow::ensure!(
        method == "GET",
        "the authorization redirect used HTTP {method}, expected GET — refusing it"
    );
    parts
        .next()
        .map(str::to_string)
        .map(Some)
        .context("the authorization redirect's request line carried no target")
}

/// Decide what a callback target means, with the `state` check first.
///
/// State before everything else: an `error` or a `code` in a callback whose
/// nonce does not match is somebody else's redirect, and neither the code nor
/// the error message should be believed or acted on.
fn parse_callback(target: &str, expected_state: &AuthState) -> anyhow::Result<String> {
    // The target is origin-form (`/?code=…`); a base is needed only to parse it.
    let url = reqwest::Url::parse("http://127.0.0.1")
        .expect("a literal loopback base URL parses")
        .join(target)
        .context("the authorization redirect's target was not a valid URL")?;
    let params: HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let state = params.get("state").map(String::as_str).unwrap_or_default();
    anyhow::ensure!(
        expected_state.matches(state),
        "the authorization redirect's `state` did not match the one this flow generated. That is \
         either a stale browser tab or a hijacked redirect — nothing was stored and no code was \
         exchanged. Start the flow again."
    );

    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("(no description)");
        anyhow::bail!("the provider refused the authorization: {error}: {description}");
    }

    params
        .get("code")
        .filter(|c| !c.is_empty())
        .cloned()
        .context(
            "the authorization redirect carried neither a `code` nor an `error`. Nothing was \
             stored; start the flow again.",
        )
}

/// The parsed subset of an authorization-code token response we rely on.
#[derive(serde::Deserialize)]
struct CodeExchangeResponse {
    refresh_token: Option<String>,
}

/// Exchange an authorization code for a refresh token (RFC 6749 §4.1.3, with
/// the RFC 7636 `code_verifier`).
///
/// Redaction contract, mirroring [`crate::rest_oauth2`]: the request body is
/// never logged; a non-2xx response is surfaced only through the standard
/// `error`/`error_description` fields; a 2xx body is never echoed, because a
/// success-shaped body carries tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> anyhow::Result<RefreshToken> {
    // Checked here, at this function's own boundary, so no caller can send a
    // secret to a cleartext endpoint by forgetting to check first.
    let token_url = parse_secure_endpoint("token_url", token_url)?;
    let token_url = token_url.as_str();
    let safe_url = crate::rest_oauth2::redact_url(token_url);
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", redirect_uri),
        ("code_verifier", pkce.verifier()),
    ];

    let resp = http.post(token_url).form(&form).send().await.map_err(|e| {
        anyhow::anyhow!(
            "oauth2 code exchange POST to {safe_url} failed: {}",
            e.without_url()
        )
    })?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
        anyhow::anyhow!(
            "oauth2 code exchange: reading the response from {safe_url} failed: {}",
            e.without_url()
        )
    })?;

    anyhow::ensure!(
        status.is_success(),
        "oauth2 code exchange at {safe_url} returned HTTP {status}: {}",
        crate::rest_oauth2::redact_token_error_body(&body)
    );

    let parsed: CodeExchangeResponse = serde_json::from_str(&body).map_err(|e| {
        anyhow::anyhow!(
            "oauth2 code exchange at {safe_url} returned an unexpected/non-JSON response (body \
             redacted): {e}"
        )
    })?;

    let refresh_token = parsed
        .refresh_token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .context(
            "the token endpoint returned no `refresh_token`. Providers commonly withhold one when \
             the account has already consented to this OAuth client: revoke Holon's access in the \
             provider's account settings and run Configure again. (An access token alone is not \
             stored — it expires within the hour.)",
        )?;

    Ok(RefreshToken(refresh_token))
}

/// Write the refresh token to `path`, atomically and privately.
///
/// The only secret this flow writes is the refresh token, and the model gives
/// it exactly one home: `RestOAuth2Config.refresh_token_file` and
/// `Credentials.refresh_token_file` are both hard file paths, with no keychain
/// arm anywhere in the schema. So there is no arm to dispatch on, and this is a
/// file writer rather than a target-directed one. (The client id/secret DO have
/// all three arms, but this flow reads them — it never writes them; see
/// [`readable_ref`].)
///
/// Atomic: the token is written to a private temporary sibling and renamed over
/// the destination. `rename` is atomic within a directory, so a reader sees
/// either the old token or the new one, and a failure at any point leaves the
/// user's working token exactly as it was. Truncating the destination in place
/// — or unlinking it first — puts the user in a strictly worse position than
/// before the flow ran if anything then goes wrong.
#[cfg(unix)]
fn write_refresh_token(path: &Path, secret: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("could not create the directory for {}", parent.display()))?;

    // The temporary is a sibling so the rename stays within one filesystem, and
    // it is created 0600 with O_EXCL: private from birth, and O_EXCL refuses to
    // follow a symlink someone planted at the name.
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("credential"))
            .to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&temp);

    let write_temp = || -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .with_context(|| format!("could not create the temporary file {}", temp.display()))?;
        writeln!(file, "{secret}")
            .with_context(|| format!("could not write the temporary file {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("could not flush {} to disk", temp.display()))
    };

    if let Err(e) = write_temp() {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(e).with_context(|| {
            format!(
                "could not move the new credential into place at {}; your previous credential is \
                 untouched",
                path.display()
            )
        });
    }
    Ok(())
}

/// Writing a credential file needs POSIX permissions to make it private. Where
/// they cannot be set, the refusal is loud rather than a world-readable secret.
#[cfg(not(unix))]
fn write_refresh_token(path: &Path, _: &str) -> anyhow::Result<()> {
    anyhow::bail!(
        "refusing to write the credential file {} on this platform: its permissions cannot be \
         restricted to this user. Declare the credential's `*_keychain` arm in the sidecar \
         instead.",
        path.display()
    )
}

/// The credential locations a completed flow records, derived from the
/// sidecar's declared arms (never from where the flow "felt like" writing).
pub fn recorded_credentials(cfg: &RestOAuth2Config) -> anyhow::Result<Credentials> {
    Ok(Credentials {
        client_id: readable_ref(
            "client_id",
            cfg.client_id_env.as_deref(),
            cfg.client_id_file.as_deref(),
            cfg.client_id_keychain.as_ref(),
        )?,
        client_secret: readable_ref(
            "client_secret",
            cfg.client_secret_env.as_deref(),
            cfg.client_secret_file.as_deref(),
            cfg.client_secret_keychain.as_ref(),
        )?,
        refresh_token_file: expand_tilde(&cfg.refresh_token_file),
    })
}

/// The [`CredentialRef`] for a credential this flow READS rather than writes.
///
/// The `*_env` arm is fine for these: the flow only has to record that the
/// value comes from an environment variable, not put one there. (Holon could
/// not set one in the user's session anyway — which is exactly why the refresh
/// token, the one credential this flow WRITES, has no env arm in the schema.)
fn readable_ref(
    field: &str,
    env: Option<&str>,
    file: Option<&str>,
    keychain: Option<&KeychainRef>,
) -> anyhow::Result<CredentialRef> {
    match (env, file, keychain) {
        (Some(var), None, None) => Ok(CredentialRef::Env {
            var: var.to_string(),
        }),
        (None, Some(path), None) => Ok(CredentialRef::File {
            path: expand_tilde(path),
        }),
        (None, None, Some(entry)) => Ok(CredentialRef::Keychain {
            service: entry.service.clone(),
            account: entry.account.clone(),
        }),
        (None, None, None) => anyhow::bail!(
            "the sidecar declares none of `{field}_env`, `{field}_file` or `{field}_keychain`, so \
             there is nothing to record"
        ),
        _ => anyhow::bail!(
            "the sidecar declares more than one of `{field}_env`, `{field}_file` or \
             `{field}_keychain` — exactly one location is required"
        ),
    }
}

/// How long the flow waits at the loopback for the user to finish consenting.
pub const DEFAULT_CONSENT_TIMEOUT: Duration = Duration::from_secs(300);

/// Run the whole consent flow for one provider and record the result.
///
/// On success the provider's state file carries `enabled = true` and
/// `Configured` with the credential LOCATIONS. On any failure nothing is
/// recorded: a half-written state would claim a configuration the next launch
/// cannot resolve.
pub async fn configure_integration(
    provider: &str,
    cfg: &RestOAuth2Config,
    store: &crate::integration_state::IntegrationConfigStore,
    lookup: &crate::integration_config::VarLookup<'_>,
    browser: &dyn BrowserOpener,
    timeout: Duration,
) -> anyhow::Result<()> {
    let http = &reqwest::Client::new();
    // Everything that can be known before the user is involved is checked
    // first: sending someone to a consent page and only then discovering the
    // sidecar cannot store the result wastes a consent that some providers
    // will not issue twice.
    let request = AuthorizationRequest::from_config(cfg)
        .with_context(|| format!("cannot start the consent flow for '{provider}'"))?;
    // BOTH endpoints are checked before the browser opens. `exchange_code`
    // re-checks the token endpoint at its own boundary, but that runs after
    // consent: refusing there sent the user to a real consent page, took a real
    // grant, and only then failed — and a consent is not a free resource, since
    // providers commonly withhold a second refresh token until a manual revoke.
    parse_secure_endpoint("token_url", &cfg.token_url)
        .with_context(|| format!("cannot start the consent flow for '{provider}'"))?;
    let credentials = recorded_credentials(cfg)
        .with_context(|| format!("cannot start the consent flow for '{provider}'"))?;
    let (client_id, client_secret) = crate::rest_oauth2::resolve_client_credentials(cfg, lookup)
        .with_context(|| {
            format!(
                "'{provider}' has no usable OAuth client credentials yet. Create an OAuth client \
                 in the provider's console and provision the client_id/client_secret where the \
                 sidecar points, then run Configure again."
            )
        })?;

    let redirect = LoopbackRedirect::bind().await?;
    // Held separately because `wait_for_code` consumes the listener, and the
    // token exchange must repeat the SAME `redirect_uri` — providers reject an
    // exchange whose value differs from the authorization request's by so much
    // as a trailing slash.
    let redirect_uri = redirect.redirect_uri().to_string();
    let state = AuthState::generate();
    let pkce = Pkce::generate();
    let url = request.url(&client_id, &redirect_uri, &state, &pkce);

    tracing::info!(
        provider,
        redirect_uri,
        "opening the system browser for the OAuth consent flow"
    );
    browser.open(&url)?;

    let code = redirect.wait_for_code(&state, timeout).await?;
    let refresh_token = exchange_code(
        http,
        &cfg.token_url,
        &client_id,
        &client_secret,
        &code,
        &redirect_uri,
        &pkce,
    )
    .await?;

    write_refresh_token(&credentials.refresh_token_file, refresh_token.expose())?;

    store.set(
        provider,
        IntegrationState {
            enabled: true,
            configuration: crate::integration_state::Configuration::Configured(credentials),
        },
    )?;
    tracing::info!(provider, "OAuth consent flow completed");
    Ok(())
}

/// Expand a leading `~/` to `$HOME`, matching the sidecar path convention.
fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => Path::new(&home).join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}
