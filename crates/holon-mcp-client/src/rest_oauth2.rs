//! Generic OAuth2 (refresh-token grant) auth arm for the `rest` transport.
//!
//! This is a *second* auth mode for `transport: rest`, orthogonal to the static
//! header the transport already supports. It is deliberately generic — nothing
//! here is Google-specific; a sidecar wires it up entirely from YAML
//! ([`RestOAuth2Config`]) plus environment/keychain/file references.
//!
//! Flow: a long-lived **refresh token** (obtained once, out of band, by the
//! user's consent flow) is exchanged at the provider's `token_url` for a
//! short-lived **access token** via the OAuth2 refresh-token grant
//! (RFC 6749 §6). The access token is cached in memory with its expiry and
//! attached as `Authorization: Bearer <token>` on every request. It is
//! refreshed proactively at ~90% of its lifetime, and once more on a 401.
//!
//! # Security invariants (audited)
//!
//! - **Access tokens never touch disk.** Only the long-lived refresh token
//!   lives in a file (written by the user's bootstrap helper, never by Holon);
//!   the access token exists only in this process's memory.
//! - **No token or secret is ever logged.** [`OAuth2TokenProvider`]'s [`Debug`]
//!   redacts every credential; error messages redact token-request URLs' query
//!   strings and never include the request body (which carries the refresh
//!   token + client secret) or a success response body (which carries the
//!   access token). The client secret, the refresh token, and every minted
//!   access token are also registered with the shared
//!   [`crate::redaction::Redactor`], which strips them from anything the `rest`
//!   transport emits — see that module for the full contract.
//! - **Credential files must be private.** A refresh-token (or client-secret)
//!   file that is group/world-accessible is *refused loudly* at startup — a
//!   readable secret is a compromised secret.
//! - **Fail loud, never fake.** A missing env var / absent credential file /
//!   absent keychain entry surfaces as the typed [`UnresolvedVar`] ("not
//!   configured yet" → the integration is disclosed-skipped), while a
//!   *misconfigured* credential (bad perms, unreadable, empty, unusable
//!   keychain, non-JSON token response, refresh failure) is a hard error with
//!   an actionable message. Nothing silently degrades.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::credential_path::ConfinedPath;
use crate::credential_path::CredentialRoot;
use crate::integration_config::UnresolvedVar;
use crate::integration_config::VarLookup;
use crate::redaction::Redactor;

/// OAuth2 refresh-token-grant configuration, as declared under
/// `transport.rest.auth.oauth2` in a sidecar.
///
/// Secrets are never inlined: `client_id`/`client_secret` are referenced by env
/// name (`*_env`), file path (`*_file`) or OS-keychain entry (`*_keychain`),
/// and the long-lived refresh token lives in `refresh_token_file` (which the
/// user's bootstrap helper writes with mode 0600). `scopes` is informational
/// only (the refresh grant does not send scopes; they document what the refresh
/// token was consented for).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestOAuth2Config {
    /// The provider's OAuth2 token endpoint (e.g.
    /// `https://oauth2.googleapis.com/token`). POSTed to for the refresh grant.
    pub token_url: String,
    /// The provider's OAuth2 *authorization* endpoint — where the in-app
    /// consent flow sends the system browser
    /// ([`crate::oauth_bootstrap`]). Absent means this sidecar can refresh a
    /// refresh token it already has but cannot obtain one, so the Configure
    /// affordance refuses rather than dead-ending in the browser.
    #[serde(default)]
    pub auth_url: Option<String>,
    /// Extra query parameters appended to the authorization request, for the
    /// provider-specific knobs that govern whether a refresh token is issued at
    /// all (Google needs `access_type=offline` and `prompt=consent`). Keeping
    /// them in the sidecar is what lets the flow engine stay provider-generic.
    #[serde(default)]
    pub auth_params: std::collections::HashMap<String, String>,
    /// Env var holding the OAuth client id. Exactly one of `client_id_env` /
    /// `client_id_file` / `client_id_keychain` must be set.
    #[serde(default)]
    pub client_id_env: Option<String>,
    /// File holding the OAuth client id (contents trimmed).
    #[serde(default)]
    pub client_id_file: Option<String>,
    /// OS-keychain entry holding the OAuth client id.
    #[serde(default)]
    pub client_id_keychain: Option<KeychainRef>,
    /// Env var holding the OAuth client secret. Exactly one of
    /// `client_secret_env` / `client_secret_file` / `client_secret_keychain`
    /// must be set.
    #[serde(default)]
    pub client_secret_env: Option<String>,
    /// File holding the OAuth client secret. Enforced to be mode 0600.
    #[serde(default)]
    pub client_secret_file: Option<String>,
    /// OS-keychain entry holding the OAuth client secret.
    #[serde(default)]
    pub client_secret_keychain: Option<KeychainRef>,
    /// Path to the long-lived refresh token. Written by the user's one-time
    /// bootstrap helper (never by Holon), enforced to be mode 0600. Resolved
    /// against the active profile's config directory
    /// ([`crate::credential_path`]), so write `${CONFIG_DIR}/<file>`.
    pub refresh_token_file: String,
    /// The scopes the refresh token is consented for. Not sent on the refresh
    /// grant (RFC 6749 §6 reuses the original grant's scopes), but load-bearing
    /// for the in-app consent flow, which puts them on the authorization
    /// request.
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// The credential files of one OAuth2 arm, each proved to sit inside the
/// active profile's config directory.
///
/// Every credential read below takes these rather than the declared strings,
/// so a location outside the profile cannot reach a `File::open` at all — the
/// refusal happens once, here, instead of being a check each reader has to
/// remember.
#[derive(Debug, Clone)]
pub struct ConfinedOAuth2Files {
    pub client_id: Option<ConfinedPath>,
    pub client_secret: Option<ConfinedPath>,
    pub refresh_token: ConfinedPath,
}

impl RestOAuth2Config {
    /// Parse this arm's declared credential locations against `root`.
    ///
    /// Fails loudly on a declaration that names anywhere else. A sidecar that
    /// points at `$HOME` makes every instance on the machine — a sandbox
    /// launched with `HOLON_CONFIG_DIR` included — authenticate as the same
    /// account, which is exactly the isolation break this refuses.
    pub fn confine(&self, root: &CredentialRoot) -> anyhow::Result<ConfinedOAuth2Files> {
        let confine_opt =
            |field: &str, declared: &Option<String>| -> anyhow::Result<Option<ConfinedPath>> {
                declared
                    .as_deref()
                    .map(|d| {
                        root.confine(d)
                            .with_context(|| format!("oauth2: `{field}` is not usable"))
                    })
                    .transpose()
            };
        Ok(ConfinedOAuth2Files {
            client_id: confine_opt("client_id_file", &self.client_id_file)?,
            client_secret: confine_opt("client_secret_file", &self.client_secret_file)?,
            refresh_token: root
                .confine(&self.refresh_token_file)
                .context("oauth2: `refresh_token_file` is not usable")?,
        })
    }
}

/// Where a credential sits in the OS keychain.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeychainRef {
    pub service: String,
    pub account: String,
}

/// The parsed subset of an OAuth2 token-endpoint response we rely on.
///
/// A refresh grant returns a fresh `access_token` (+ its `expires_in`); it does
/// NOT return a new refresh token (the long-lived one is reused), so we
/// deliberately ignore any other fields.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    /// Seconds until the access token expires. Defaults to a conservative one
    /// hour if the provider omits it (rare) so we never treat a token as
    /// eternal.
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    3600
}

/// A cached access token plus the two instants that govern its reuse.
struct CachedToken {
    access_token: String,
    /// Proactive-refresh point: ~90% of the lifetime. Past this we refresh even
    /// though the token may still be technically valid.
    refresh_after: Instant,
}

/// Holds the resolved OAuth2 credentials and an in-memory access-token cache,
/// serving fresh `Bearer` tokens behind a mutex. One provider is shared (via
/// `Arc`) across every call of a `rest` integration, so all calls share one
/// cache and one refresh.
pub struct OAuth2TokenProvider {
    token_url: String,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    scopes: Vec<String>,
    client: reqwest::Client,
    cached: Mutex<Option<CachedToken>>,
    /// Shared with the transport this provider authenticates, so a token minted
    /// here is stripped from messages emitted there.
    redactor: Redactor,
}

impl std::fmt::Debug for OAuth2TokenProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact EVERY credential. Only non-secret shape is printed.
        f.debug_struct("OAuth2TokenProvider")
            .field("token_url", &redact_url(&self.token_url))
            .field("scopes", &self.scopes)
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

impl OAuth2TokenProvider {
    /// Resolve credentials and build the provider. All I/O and permission
    /// checks happen here (at startup), so a misconfiguration fails before the
    /// first request.
    ///
    /// Returns [`UnresolvedVar`] (a disclosed skip) when the integration is
    /// simply *not configured yet* (an env ref is unset, a keychain entry is
    /// absent, or the refresh-token file does not exist). Every other problem —
    /// an ambiguous/absent source choice, a group/world-readable credential
    /// file, an unusable keychain, an unreadable or empty credential — is a
    /// hard error with an actionable message.
    pub fn from_config(
        cfg: &RestOAuth2Config,
        lookup: &VarLookup<'_>,
        redactor: &Redactor,
        root: &CredentialRoot,
    ) -> anyhow::Result<Self> {
        if cfg.token_url.trim().is_empty() {
            anyhow::bail!("oauth2.token_url must not be empty");
        }
        let files = cfg.confine(root)?;
        let client_id = resolve_secret(
            "client_id",
            SecretSources {
                env: cfg.client_id_env.as_deref(),
                file: files.client_id.as_ref(),
                keychain: cfg.client_id_keychain.as_ref(),
            },
            lookup,
            &holon_secrets::platform_keychain,
            /* enforce_private_file */ false,
        )?;
        let client_secret = resolve_secret(
            "client_secret",
            SecretSources {
                env: cfg.client_secret_env.as_deref(),
                file: files.client_secret.as_ref(),
                keychain: cfg.client_secret_keychain.as_ref(),
            },
            lookup,
            &holon_secrets::platform_keychain,
            /* enforce_private_file */ true,
        )?;
        let refresh_token = read_refresh_token(&files.refresh_token)?;

        // These two reach the wire in the token-grant POST body. Registering
        // them covers a provider that echoes a submitted field back in an error.
        redactor.register(&client_secret);
        redactor.register(&refresh_token);

        Ok(Self {
            token_url: cfg.token_url.clone(),
            client_id,
            client_secret,
            refresh_token,
            scopes: cfg.scopes.clone(),
            client: reqwest::Client::new(),
            cached: Mutex::new(None),
            redactor: redactor.clone(),
        })
    }

    /// Build with a caller-supplied HTTP client (tests point it at a mock).
    #[cfg(test)]
    fn with_client_for_test(
        token_url: String,
        client_id: String,
        client_secret: String,
        refresh_token: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            token_url,
            client_id,
            client_secret,
            refresh_token,
            scopes: Vec::new(),
            client,
            cached: Mutex::new(None),
            redactor: Redactor::new(),
        }
    }

    /// A valid access token: the cached one while it is still within ~90% of
    /// its lifetime, otherwise a freshly refreshed one.
    pub async fn access_token(&self) -> anyhow::Result<String> {
        let mut guard = self.cached.lock().await;
        if let Some(tok) = guard.as_ref()
            && Instant::now() < tok.refresh_after
        {
            return Ok(tok.access_token.clone());
        }
        let fresh = self.do_refresh().await?;
        let access = fresh.access_token.clone();
        *guard = Some(fresh);
        Ok(access)
    }

    /// Force a refresh regardless of cache state (used on a 401 to recover from
    /// a token the server rejected before we thought it expired).
    pub async fn force_refresh(&self) -> anyhow::Result<String> {
        let mut guard = self.cached.lock().await;
        let fresh = self.do_refresh().await?;
        let access = fresh.access_token.clone();
        *guard = Some(fresh);
        Ok(access)
    }

    /// POST the refresh-token grant and parse the response into a cache entry.
    ///
    /// Redaction: the request BODY (refresh token + client secret) is never
    /// logged; the URL is stripped of any query string; a non-2xx body is
    /// surfaced only via its safe OAuth `error`/`error_description` fields; a
    /// 2xx body is never echoed (it carries the access token).
    async fn do_refresh(&self) -> anyhow::Result<CachedToken> {
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", self.refresh_token.as_str()),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
        ];
        let resp = self
            .client
            .post(&self.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "oauth2 token refresh POST to {} failed: {}",
                    redact_url(&self.token_url),
                    // `without_url` strips the URL (and thus any query) from the
                    // reqwest error before it reaches a log.
                    e.without_url()
                )
            })?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            anyhow::anyhow!(
                "oauth2 token refresh: reading response from {} failed: {}",
                redact_url(&self.token_url),
                e.without_url()
            )
        })?;

        if !status.is_success() {
            anyhow::bail!(
                "oauth2 token refresh to {} returned HTTP {}: {}",
                redact_url(&self.token_url),
                status,
                redact_token_error_body(&body)
            );
        }

        let parsed: TokenResponse = serde_json::from_str(&body).map_err(|e| {
            // Never echo the body — a success-shaped body would carry the token.
            anyhow::anyhow!(
                "oauth2 token refresh to {} returned an unexpected/non-JSON response (body \
                 redacted): {e}. Check token_url and that the refresh token is still valid.",
                redact_url(&self.token_url)
            )
        })?;

        if parsed.access_token.is_empty() {
            anyhow::bail!(
                "oauth2 token refresh to {} returned an empty access_token",
                redact_url(&self.token_url)
            );
        }

        // The token now goes out on every request's `Authorization` header, so a
        // server that echoes that header into an error body would disclose it.
        self.redactor.register_minted(&parsed.access_token);

        // Refresh proactively at 90% of the lifetime so a request never rides an
        // about-to-expire token.
        let lifetime = Duration::from_secs(parsed.expires_in.max(1));
        let refresh_after = Instant::now() + lifetime.mul_f64(0.9);
        Ok(CachedToken {
            access_token: parsed.access_token,
            refresh_after,
        })
    }
}

/// Resolve the OAuth *client* credentials (id + secret) through the sidecar's
/// declared arms — the same resolution the transport performs at startup.
///
/// The consent flow needs these before it can build an authorization request,
/// and it must read them from exactly where the transport will: resolving them
/// differently is how a flow ends up reporting success over a configuration the
/// next launch cannot use.
pub(crate) fn resolve_client_credentials(
    cfg: &RestOAuth2Config,
    lookup: &VarLookup<'_>,
    root: &CredentialRoot,
) -> anyhow::Result<(String, String)> {
    let files = cfg.confine(root)?;
    let client_id = resolve_secret(
        "client_id",
        SecretSources {
            env: cfg.client_id_env.as_deref(),
            file: files.client_id.as_ref(),
            keychain: cfg.client_id_keychain.as_ref(),
        },
        lookup,
        &holon_secrets::platform_keychain,
        /* enforce_private_file */ false,
    )?;
    let client_secret = resolve_secret(
        "client_secret",
        SecretSources {
            env: cfg.client_secret_env.as_deref(),
            file: files.client_secret.as_ref(),
            keychain: cfg.client_secret_keychain.as_ref(),
        },
        lookup,
        &holon_secrets::platform_keychain,
        /* enforce_private_file */ true,
    )?;
    Ok((client_id, client_secret))
}

/// Build a shared provider (`Arc`) from config — the shape the transport holds.
pub fn build_provider(
    cfg: &RestOAuth2Config,
    lookup: &VarLookup<'_>,
    redactor: &Redactor,
    root: &CredentialRoot,
) -> anyhow::Result<Arc<OAuth2TokenProvider>> {
    Ok(Arc::new(OAuth2TokenProvider::from_config(
        cfg, lookup, redactor, root,
    )?))
}

// ---------------------------------------------------------------------------
// Credential resolution helpers
// ---------------------------------------------------------------------------

/// The three places one credential may be declared. Exactly one may be set.
struct SecretSources<'a> {
    env: Option<&'a str>,
    file: Option<&'a ConfinedPath>,
    keychain: Option<&'a KeychainRef>,
}

/// Opens the OS keychain for a given service.
type KeychainOpener<'a> = dyn Fn(&str) -> Box<dyn holon_secrets::KeychainStore> + 'a;

/// Resolve a credential from exactly one of its declared sources.
///
/// - env set, value present   → value
/// - env set, value absent    → `UnresolvedVar` (disclosed skip: not
///   configured)
/// - file set, absent         → `UnresolvedVar` (disclosed skip: not
///   provisioned)
/// - file set, present        → contents (trimmed); 0600-enforced when
///   `enforce_private_file`
/// - keychain set, no entry   → `UnresolvedVar` (disclosed skip: not
///   provisioned)
/// - keychain set, entry      → the secret (trimmed)
/// - none / more than one set → hard error (structural config mistake)
fn resolve_secret(
    field: &str,
    sources: SecretSources<'_>,
    lookup: &VarLookup<'_>,
    keychain: &KeychainOpener<'_>,
    enforce_private_file: bool,
) -> anyhow::Result<String> {
    match (sources.env, sources.file, sources.keychain) {
        (Some(env), None, None) => lookup(env).ok_or_else(|| {
            anyhow::Error::new(UnresolvedVar {
                var: env.to_string(),
            })
        }),
        (None, Some(path), None) => read_credential_file(path, enforce_private_file),
        (None, None, Some(entry)) => read_keychain_entry(entry, keychain),
        (None, None, None) => anyhow::bail!(
            "oauth2: one of `{field}_env`, `{field}_file` or `{field}_keychain` must be set"
        ),
        _ => anyhow::bail!(
            "oauth2: set only one of `{field}_env`, `{field}_file` or `{field}_keychain`"
        ),
    }
}

/// Read a credential out of the OS keychain.
///
/// No entry → [`UnresolvedVar`] (a disclosed skip; the integration is simply
/// not provisioned yet). An unusable keychain, or an entry holding non-UTF-8 or
/// blank material, is a hard error.
fn read_keychain_entry(
    entry: &KeychainRef,
    keychain: &KeychainOpener<'_>,
) -> anyhow::Result<String> {
    let store = keychain(&entry.service);
    let Some(bytes) = store.load(&entry.account).map_err(|e| {
        anyhow::anyhow!(
            "oauth2: failed to read keychain entry {}/{}: {e}",
            entry.service,
            entry.account
        )
    })?
    else {
        return Err(anyhow::Error::new(UnresolvedVar {
            var: format!("keychain entry {}/{}", entry.service, entry.account),
        }));
    };
    let secret = String::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!(
            "oauth2: keychain entry {}/{} is not valid UTF-8",
            entry.service,
            entry.account
        )
    })?;
    let trimmed = secret.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!(
            "oauth2: keychain entry {}/{} is empty",
            entry.service,
            entry.account
        );
    }
    Ok(trimmed)
}

/// Read the long-lived refresh token from its file. Absent file → not
/// provisioned yet (disclosed skip); present file → 0600-enforced, trimmed,
/// non-empty.
fn read_refresh_token(path: &ConfinedPath) -> anyhow::Result<String> {
    read_credential_file(path, /* enforce_private_file */ true).map_err(|e| {
        // Enrich the "not provisioned" case with the bootstrap pointer while
        // preserving the typed UnresolvedVar so the caller still disclosed-skips.
        if e.downcast_ref::<UnresolvedVar>().is_some() {
            anyhow::Error::new(UnresolvedVar {
                var: format!(
                    "refresh token file {path} (run scripts/google-oauth-bootstrap.sh to create it)"
                ),
            })
        } else {
            e
        }
    })
}

/// Read a credential file's trimmed contents.
///
/// Absent file → [`UnresolvedVar`] (a disclosed skip; the integration is simply
/// not provisioned yet). A symbolic link, or a present but
/// group/world-accessible file → hard refusal. Present but unreadable/empty →
/// hard error.
///
/// The link refusal is what makes [`ConfinedPath`] mean anything at the point
/// of use: `exists`, `metadata` and `read_to_string` all follow links, so a
/// link placed at a credential's NAME reads a file the confined path does not
/// name — another profile's token, whose own 0600 mode passes the privacy
/// check. The write leg never creates a credential through a link either
/// (`O_EXCL` + 0600), so refusing here makes the two halves agree.
fn read_credential_file(path: &ConfinedPath, enforce_private_file: bool) -> anyhow::Result<String> {
    let expanded = path.path();
    // `symlink_metadata` describes the NAME, not what it points at — the whole
    // point of the check below.
    let meta = match std::fs::symlink_metadata(expanded) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(anyhow::Error::new(UnresolvedVar {
                var: format!("credential file {}", expanded.display()),
            }));
        }
        Err(e) => {
            anyhow::bail!(
                "oauth2: cannot stat credential file {}: {e}",
                expanded.display()
            )
        }
    };
    anyhow::ensure!(
        !meta.file_type().is_symlink(),
        "oauth2: credential file {} is a symbolic link. A credential is read from the profile that \
         owns it, never through a link that can point at another profile's secret. Replace the \
         link with the credential itself, or point the sidecar at where it really lives.",
        expanded.display()
    );
    if enforce_private_file {
        assert_file_private(expanded, &meta)?;
    }
    let contents = std::fs::read_to_string(expanded).map_err(|e| {
        anyhow::anyhow!(
            "oauth2: failed to read credential file {}: {e}",
            expanded.display()
        )
    })?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("oauth2: credential file {} is empty", expanded.display());
    }
    Ok(trimmed)
}

/// Refuse a credential file that is readable/writable/executable by group or
/// other. A secret the rest of the machine can read is a compromised secret, so
/// this is a hard, loud refusal rather than a warning.
#[cfg(unix)]
fn assert_file_private(path: &Path, meta: &std::fs::Metadata) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        anyhow::bail!(
            "oauth2: credential file {} is group/world-accessible (mode {:o}); it must be private. \
             Run: chmod 600 {}",
            path.display(),
            mode,
            path.display()
        );
    }
    Ok(())
}

/// On non-Unix we cannot verify POSIX permissions, so we refuse rather than
/// silently skip the security control.
#[cfg(not(unix))]
fn assert_file_private(path: &Path, _: &std::fs::Metadata) -> anyhow::Result<()> {
    anyhow::bail!(
        "oauth2: refusing to read credential file {} on a non-Unix platform where its private \
         (0600) permissions cannot be verified",
        path.display()
    )
}

/// Strip any query string from a URL so it is safe to log.
pub(crate) fn redact_url(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?<redacted>"),
        None => url.to_string(),
    }
}

/// Surface only the safe, standard OAuth error fields from a non-2xx token
/// response; never echo the raw body (which may not conform and could contain
/// unexpected material).
pub(crate) fn redact_token_error_body(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => {
            let err = v.get("error").and_then(|e| e.as_str());
            let desc = v.get("error_description").and_then(|e| e.as_str());
            match (err, desc) {
                (Some(e), Some(d)) => format!("error={e}, error_description={d}"),
                (Some(e), None) => format!("error={e}"),
                _ => "<redacted non-standard error body>".to_string(),
            }
        }
        Err(_) => "<redacted non-JSON error body>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use holon_secrets::KeychainStore;

    use super::*;

    #[test]
    fn redact_url_strips_query() {
        assert_eq!(
            redact_url("https://oauth2.googleapis.com/token?client_secret=abc"),
            "https://oauth2.googleapis.com/token?<redacted>"
        );
        assert_eq!(
            redact_url("https://oauth2.googleapis.com/token"),
            "https://oauth2.googleapis.com/token"
        );
    }

    #[test]
    fn redact_token_error_body_keeps_only_safe_fields() {
        let body = r#"{"error":"invalid_grant","error_description":"Token has been expired"}"#;
        let red = redact_token_error_body(body);
        assert!(red.contains("invalid_grant"));
        assert!(red.contains("expired"));
        // A body with a token-shaped field must not be echoed verbatim.
        let sneaky = r#"{"access_token":"ya29.SECRET","foo":"bar"}"#;
        let red = redact_token_error_body(sneaky);
        assert!(!red.contains("SECRET"), "redaction leaked a token: {red}");
    }

    #[test]
    fn debug_impl_redacts_all_credentials() {
        let p = OAuth2TokenProvider::with_client_for_test(
            "https://example.com/token".into(),
            "my-client-id".into(),
            "my-client-secret".into(),
            "my-refresh-token".into(),
            reqwest::Client::new(),
        );
        let dbg = format!("{p:?}");
        assert!(!dbg.contains("my-client-id"), "{dbg}");
        assert!(!dbg.contains("my-client-secret"), "{dbg}");
        assert!(!dbg.contains("my-refresh-token"), "{dbg}");
    }

    fn no_sources<'a>() -> SecretSources<'a> {
        SecretSources {
            env: None,
            file: None,
            keychain: None,
        }
    }

    fn no_keychain(_: &str) -> Box<dyn holon_secrets::KeychainStore> {
        Box::new(holon_secrets::InMemoryKeychainStore::new())
    }

    #[test]
    fn resolve_secret_missing_env_is_unresolved_var() {
        let err = resolve_secret(
            "client_id",
            SecretSources {
                env: Some("HOLON_TEST_UNSET_OAUTH_CLIENT_ID"),
                ..no_sources()
            },
            &|_| None,
            &no_keychain,
            false,
        )
        .unwrap_err();
        let uv = err
            .downcast_ref::<UnresolvedVar>()
            .expect("missing env must surface as UnresolvedVar (disclosed skip)");
        assert_eq!(uv.var, "HOLON_TEST_UNSET_OAUTH_CLIENT_ID");
    }

    #[test]
    fn resolve_secret_reads_the_keychain_entry() {
        let services = std::sync::Mutex::new(Vec::new());
        let opener = |service: &str| -> Box<dyn holon_secrets::KeychainStore> {
            services.lock().unwrap().push(service.to_string());
            let store = holon_secrets::InMemoryKeychainStore::new();
            store.store("gcal", b"kc-client-secret\n").unwrap();
            Box::new(store)
        };

        let kc = KeychainRef {
            service: "space.holon.test".into(),
            account: "gcal".into(),
        };
        let secret = resolve_secret(
            "client_secret",
            SecretSources {
                keychain: Some(&kc),
                ..no_sources()
            },
            &|_| None,
            &opener,
            true,
        )
        .unwrap();
        assert_eq!(secret, "kc-client-secret");
        assert_eq!(services.into_inner().unwrap(), ["space.holon.test"]);
    }

    #[test]
    fn resolve_secret_absent_keychain_entry_is_disclosed_skip() {
        let kc = KeychainRef {
            service: "space.holon.test".into(),
            account: "never-provisioned".into(),
        };
        let err = resolve_secret(
            "client_secret",
            SecretSources {
                keychain: Some(&kc),
                ..no_sources()
            },
            &|_| None,
            &no_keychain,
            true,
        )
        .unwrap_err();
        let uv = err
            .downcast_ref::<UnresolvedVar>()
            .expect("absent keychain entry must surface as UnresolvedVar (disclosed skip)");
        assert!(uv.var.contains("never-provisioned"), "{}", uv.var);
    }

    #[test]
    fn resolve_secret_keychain_backend_failure_is_a_hard_error() {
        let kc = KeychainRef {
            service: "space.holon.test".into(),
            account: "gcal".into(),
        };
        let err = resolve_secret(
            "client_secret",
            SecretSources {
                keychain: Some(&kc),
                ..no_sources()
            },
            &|_| None,
            &|_| -> Box<dyn holon_secrets::KeychainStore> {
                Box::new(holon_secrets::UnavailableKeychainStore::new())
            },
            true,
        )
        .unwrap_err();
        assert!(
            err.downcast_ref::<UnresolvedVar>().is_none(),
            "an unusable keychain must NOT be mistaken for an unprovisioned one"
        );
    }

    #[test]
    fn resolve_secret_keychain_plus_env_is_a_hard_error() {
        let kc = KeychainRef {
            service: "space.holon.test".into(),
            account: "gcal".into(),
        };
        let err = resolve_secret(
            "client_secret",
            SecretSources {
                env: Some("X"),
                keychain: Some(&kc),
                file: None,
            },
            &|_| Some("from-env".into()),
            &no_keychain,
            true,
        )
        .unwrap_err();
        assert!(err.downcast_ref::<UnresolvedVar>().is_none());
        assert!(err.to_string().contains("only one of"));
    }

    #[test]
    fn keychain_is_a_recognized_secret_source_in_config() {
        let cfg: RestOAuth2Config = serde_yaml::from_str(
            "token_url: https://example.test/token\n\
             client_id_env: HOLON_TEST_CLIENT_ID\n\
             client_secret_keychain:\n  \
             service: space.holon.test\n  \
             account: gcal\n\
             refresh_token_file: /nonexistent\n",
        )
        .expect("client_secret_keychain must be a recognized oauth2 secret source");
        assert!(cfg.client_secret_env.is_none());
        assert!(cfg.client_secret_file.is_none());
    }

    #[test]
    fn resolve_secret_both_sources_is_hard_error() {
        let y = confined(Path::new("/tmp"), "y");
        let err = resolve_secret(
            "client_id",
            SecretSources {
                env: Some("X"),
                file: Some(&y),
                keychain: None,
            },
            &|_| None,
            &no_keychain,
            false,
        )
        .unwrap_err();
        assert!(err.downcast_ref::<UnresolvedVar>().is_none());
        assert!(err.to_string().contains("only one of"));
    }

    #[test]
    fn resolve_secret_neither_source_is_hard_error() {
        let err =
            resolve_secret("client_id", no_sources(), &|_| None, &no_keychain, false).unwrap_err();
        assert!(err.downcast_ref::<UnresolvedVar>().is_none());
        assert!(err.to_string().contains("must be set"));
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_credential_file_is_refused() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"1//refresh").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = read_credential_file(&confined(dir.path(), "refresh-token"), true).unwrap_err();
        assert!(
            err.downcast_ref::<UnresolvedVar>().is_none(),
            "a bad-perms file is a HARD error, not a disclosed skip"
        );
        let msg = err.to_string();
        assert!(msg.contains("group/world-accessible"), "{msg}");
        assert!(msg.contains("chmod 600"), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn mode_0600_credential_file_is_accepted() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"  1//refresh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let got = read_credential_file(&confined(dir.path(), "refresh-token"), true).unwrap();
        assert_eq!(got, "1//refresh", "contents trimmed");
    }

    /// A credential path confined to `root_dir`, which the unit tests use as
    /// the profile's config directory.
    fn confined(root_dir: &Path, name: &str) -> ConfinedPath {
        CredentialRoot::new(root_dir)
            .confine(name)
            .expect("a bare name under the root confines")
    }

    #[cfg(unix)]
    fn write_file(mode: u32, contents: &[u8]) -> (tempfile::TempDir, ConfinedPath) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cred");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let confined = confined(dir.path(), "cred");
        (dir, confined)
    }

    // The `client_secret_file` variant flows through resolve_secret with
    // enforce_private_file=true, so it enforces 0600 exactly like
    // refresh_token_file — a group/world-readable secret file is refused loudly.
    #[cfg(unix)]
    #[test]
    fn client_secret_file_variant_enforces_0600() {
        let (_dir, path) = write_file(0o644, b"GOCSPX-secret");
        let err = resolve_secret(
            "client_secret",
            SecretSources {
                file: Some(&path),
                ..no_sources()
            },
            &|_| None,
            &no_keychain,
            true,
        )
        .expect_err("world-readable client_secret file must be refused");
        assert!(
            err.downcast_ref::<UnresolvedVar>().is_none(),
            "must be a hard error"
        );
        assert!(err.to_string().contains("group/world-accessible"), "{err}");

        let (_dir, path) = write_file(0o600, b"GOCSPX-secret\n");
        let got = resolve_secret(
            "client_secret",
            SecretSources {
                file: Some(&path),
                ..no_sources()
            },
            &|_| None,
            &no_keychain,
            true,
        )
        .unwrap();
        assert_eq!(got, "GOCSPX-secret");
    }

    // `client_id_file` is NOT perm-checked: the OAuth client id is not secret, so
    // a world-readable client-id file is accepted (deliberate asymmetry — we do
    // not over-restrict a non-secret). Martin's file is 0600 regardless.
    #[cfg(unix)]
    #[test]
    fn client_id_file_variant_does_not_enforce_0600() {
        let (_dir, path) = write_file(0o644, b"1234.apps.googleusercontent.com\n");
        let got = resolve_secret(
            "client_id",
            SecretSources {
                file: Some(&path),
                ..no_sources()
            },
            &|_| None,
            &no_keychain,
            false,
        )
        .expect("client_id file need not be 0600");
        assert_eq!(got, "1234.apps.googleusercontent.com");
    }

    #[test]
    fn absent_credential_file_is_disclosed_skip() {
        let err = read_credential_file(
            &confined(Path::new("/nonexistent/holon"), "refresh-token"),
            true,
        )
        .unwrap_err();
        assert!(
            err.downcast_ref::<UnresolvedVar>().is_some(),
            "an absent (not-yet-provisioned) file must be a disclosed skip"
        );
    }

    #[cfg(unix)]
    #[test]
    fn empty_credential_file_is_hard_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");
        std::fs::File::create(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let err = read_credential_file(&confined(dir.path(), "refresh-token"), true).unwrap_err();
        assert!(err.downcast_ref::<UnresolvedVar>().is_none());
        assert!(err.to_string().contains("is empty"));
    }
}
