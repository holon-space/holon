# Handoff: MCP OAuth Client Implementation

## Goal

Implement the MCP spec's OAuth 2.1 authorization flow for HTTP-based MCP servers,
so Holon can connect to OAuth-protected servers (Gmail, Calendar, etc.) without
forking or configuring auth per-server.

## Context

The [MCP Authorization spec (draft)](https://modelcontextprotocol.io/specification/draft/basic/authorization)
defines a standard OAuth 2.1 flow for HTTP transport. The flow is:

1. Client sends request → server returns 401 with `WWW-Authenticate` header
2. Client discovers authorization server from Protected Resource Metadata (RFC 9728)
3. Client runs OAuth 2.1 + PKCE (browser consent, code exchange)
4. Client sends Bearer token on every subsequent request
5. On token expiry → refresh using refresh_token

For stdio transport, the spec says: "retrieve credentials from the environment."
Our env-var approach already handles this correctly.

## Key Discovery: rmcp Already Has Full OAuth2

**rmcp has a complete OAuth2 implementation** gated behind `#[cfg(feature = "auth")]`.

Location: `~/.cargo/git/checkouts/rust-sdk-*/*/crates/rmcp/src/transport/auth.rs` (1461 lines)

It includes:
- `AuthorizationManager` — full OAuth2 flow orchestration
- RFC 9728 metadata discovery (well-known endpoints)
- Dynamic client registration (RFC 7591)
- Authorization code exchange with PKCE
- **Automatic token refresh** via `get_access_token()` (checks expiry, refreshes if needed)
- `CredentialStore` trait for pluggable token persistence
- `InMemoryCredentialStore` default (not persisted)

**None of this is currently used.** The `auth` feature is not enabled in Cargo.toml.

## Current State

| What | Status | Where |
|------|--------|-------|
| Static Bearer tokens | ✅ Works | `connect_mcp(uri, auth_token)` → `StreamableHttpClientTransportConfig::auth_header` |
| 401 detection in rmcp | ✅ Exists | Returns `StreamableHttpError::AuthRequired(AuthRequiredError { www_authenticate_header })` |
| OAuth2 flow in rmcp | ✅ Exists (unused) | `rmcp/src/transport/auth.rs`, behind `auth` feature |
| `CredentialStore` trait | ✅ Exists (in-memory only) | `rmcp/src/transport/auth.rs:34-75` |
| Auth feature enabled | ❌ No | `Cargo.toml` only has `client`, `transport-streamable-http-client-reqwest`, `transport-child-process` |
| Persistent token storage | ❌ No | Need to implement `CredentialStore` backed by Turso/keychain |
| Browser consent UI trigger | ❌ No | Flutter needs to open auth URL and capture redirect |

## Implementation Plan

### Phase 1: Enable rmcp auth feature and wire up AuthorizationManager

**File: `crates/holon-mcp-client/Cargo.toml`**
- Add `"auth"` to rmcp features list

**File: `crates/holon-mcp-client/src/mcp_provider.rs`**
- Update `connect_mcp` to accept an optional `CredentialStore` instead of (or in addition to) a raw `auth_token`
- When a `CredentialStore` is provided, construct rmcp's `AuthorizationManager` and use it for token management
- When only `auth_token` is provided, keep current behavior (static Bearer)

**File: `crates/holon-mcp-client/src/mcp_integration.rs`**
- Update `McpIntegrationConfig` to support `AuthMode`:
  ```rust
  pub enum AuthMode {
      None,
      StaticToken(String),
      OAuth { credential_store: Arc<dyn CredentialStore> },
  }
  ```

### Phase 2: Persistent CredentialStore backed by Turso

**File: `crates/holon-mcp-client/src/credential_store.rs`** (new)
- Implement rmcp's `CredentialStore` trait backed by a Turso table
- Table: `mcp_oauth_credentials(server_uri TEXT PRIMARY KEY, credentials_json TEXT, updated_at TEXT)`
- Stores serialized OAuth2 token responses (access_token, refresh_token, expiry)
- This is similar in pattern to `DatabaseSyncTokenStore` in `holon/src/storage/sync_token_store.rs`

### Phase 3: Browser consent flow integration

The OAuth flow requires opening a browser for user consent and capturing the redirect.

**Option A (simple — localhost callback):**
- Holon spawns a temporary localhost HTTP server to receive the OAuth callback
- `AuthorizationManager` generates the auth URL
- Holon opens the URL in the system browser (or tells Flutter to open it)
- Callback server captures the authorization code
- `AuthorizationManager` exchanges code for tokens

**Option B (Flutter-native):**
- Flutter opens the auth URL via `url_launcher`
- Uses a custom URL scheme (`holon://oauth/callback`) or localhost redirect
- Passes the authorization code back to Rust via FFI

Option A is simpler and doesn't require Flutter changes. rmcp's `AuthorizationManager`
may already handle the localhost callback server — check its `authorize()` method.

### Phase 4: Automatic retry on 401

**File: `crates/holon-mcp-client/src/mcp_provider.rs`**
- In `execute_operation`, catch `AuthRequired` errors
- Call `AuthorizationManager::get_access_token()` (which handles refresh)
- If token was refreshed, retry the request
- If no valid token exists, trigger the full OAuth consent flow
- Surface "needs authentication" to the UI if consent is required

**File: `crates/holon-mcp-client/src/mcp_sync_engine.rs`**
- Same pattern in `sync_entity` — catch 401, refresh, retry

### Phase 5: Flutter UI for auth status

- Add an `auth_status` field to provider state (Authenticated / NeedsAuth { auth_url })
- Flutter shows a "Connect [Provider]" button when NeedsAuth
- Button opens browser → OAuth consent → callback → tokens stored → retry sync

## Key Files to Read

| File | Why |
|------|-----|
| `crates/holon-mcp-client/src/mcp_provider.rs` | Current `connect_mcp`, `McpOperationProvider`, 401 handling point |
| `crates/holon-mcp-client/src/mcp_integration.rs` | `McpIntegrationConfig`, `build_mcp_integration` |
| `crates/holon-mcp-client/src/mcp_sync_engine.rs` | `McpSyncEngine::sync_entity` — needs 401 retry |
| `~/.cargo/git/checkouts/rust-sdk-*/*/crates/rmcp/src/transport/auth.rs` | rmcp's OAuth2 implementation |
| `~/.cargo/git/checkouts/rust-sdk-*/*/crates/rmcp/src/transport/common/reqwest/streamable_http_client.rs` | 401 detection, `AuthRequiredError` |
| `crates/holon/src/storage/sync_token_store.rs` | Pattern for Turso-backed persistent store |
| MCP spec: https://modelcontextprotocol.io/specification/draft/basic/authorization | The spec we're implementing |

## rmcp Auth Feature Investigation Needed

Before coding, verify:
1. What does enabling `auth` feature expose? Check `rmcp/Cargo.toml` for the feature definition
2. Does `AuthorizationManager` handle the localhost callback server automatically?
3. What does the `CredentialStore` trait signature look like exactly? (check if it's async, what it stores)
4. Does rmcp's `StreamableHttpClientTransport` integrate with `AuthorizationManager` automatically when the feature is enabled, or do we need to wire them manually?
5. Check if there's a newer rmcp version (we're on v0.12.0) that has better auth integration

## Stdio Transport Auth (Already Handled)

For child process MCP servers, auth works via env vars. This is spec-compliant.
The sidecar YAML declares which env vars the server needs:

```yaml
# Future: sidecar could declare required env vars
transport:
  command: mcp-gmailcal
  env:
    GOOGLE_ACCESS_TOKEN: "${secret:google_access_token}"
```

Holon resolves `${secret:...}` references from its secure storage before spawning.
This is a separate, simpler task from the OAuth flow.

## What NOT to Do

- Don't implement a custom OAuth flow — use rmcp's `AuthorizationManager`
- Don't add per-server OAuth config to sidecar YAML — the MCP spec's discovery mechanism makes this unnecessary for HTTP servers
- Don't store tokens in plaintext config files — use the Turso-backed credential store
