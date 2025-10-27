use serde::{Deserialize, Serialize};

/// Authentication status for an MCP provider. Part of the frontend API contract.
///
/// All frontends (Flutter, MCP, future CLI) must handle all variants.
/// By placing this in holon-api, the compiler enforces exhaustive matching.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderAuthStatus {
    /// No authentication required or already authenticated
    Authenticated { provider_name: String },
    /// OAuth consent needed — frontend must open auth_url in browser
    NeedsConsent {
        auth_url: String,
        provider_name: String,
    },
    /// Authentication failed (e.g., refresh token revoked)
    AuthFailed {
        provider_name: String,
        message: String,
    },
}
