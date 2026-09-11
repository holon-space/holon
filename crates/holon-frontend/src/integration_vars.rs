//! Resolving the `${VAR}` references an MCP integration sidecar declares.
//!
//! A sidecar names its credentials as `${TODOIST_API_KEY}` /
//! `${SHOPPING_LIST_URL}` so the value stays out of the committed YAML. This
//! module says where the value comes from: the environment first, then the OS
//! keychain, then the preference in `holon.toml`.
//!
//! Environment-first is what makes a sandbox launched with an exported
//! credential authenticate as that credential regardless of the persisted
//! profile. The cost is that a stale export silently outranks what the user
//! typed into Settings, so [`crate::preferences::env_shadowed_keys`] marks
//! those fields read-only rather than letting the UI show a value nothing
//! reads.
//!
//! Keychain-over-preference is the other ordering that is not arbitrary:
//! `holon.toml` is PLAINTEXT and the keychain is not. A user who moves a token
//! into the keychain and forgets to clear the old preference must end up with
//! the protected copy in force, not the cleartext one still quietly
//! authenticating.

use std::collections::HashMap;

use crate::preferences::PrefKey;

/// The form a variable name and a preference key are compared in: lowercase,
/// with `.` and `_` as the same separator. `${SHOPPING_LIST_URL}` and the
/// `shopping.list_url` preference both normalize to `shopping_list_url`.
pub fn normalize_var_name(s: &str) -> String {
    holon_secrets::secret_account(s)
}

/// A `${VAR}` resolver over `preferences`, consulting `env`, then `keychain`.
///
/// An empty value resolves as unset in EVERY layer, so a blank export or a
/// cleared field falls through to the next one rather than failing the
/// integration's expansion as if it had been configured.
pub fn preference_var_lookup<'a>(
    preferences: &HashMap<PrefKey, toml::Value>,
    env: impl Fn(&str) -> Option<String> + Send + Sync + 'a,
    keychain: &'a dyn holon_secrets::KeychainStore,
) -> impl Fn(&str) -> Option<String> + Send + Sync + 'a {
    let by_norm: HashMap<String, String> = preferences
        .iter()
        .filter_map(|(k, v)| {
            let s = v.as_str()?;
            (!s.is_empty()).then(|| (normalize_var_name(k.as_str()), s.to_string()))
        })
        .collect();

    move |name: &str| {
        env(name)
            .filter(|v| !v.is_empty())
            .or_else(|| keychain_value(keychain, name))
            .or_else(|| by_norm.get(&normalize_var_name(name)).cloned())
    }
}

/// `name`'s stored secret, or nothing.
///
/// A backend FAILURE (a locked keychain, a denied prompt) is disclosed and
/// then treated as absent, rather than taken down the boot: the integration
/// that needed it reports itself unconfigured, which is a state the user can
/// act on. Silence is the one option not taken — an unreadable keychain and an
/// empty one are different facts and must not look alike in the log.
fn keychain_value(keychain: &dyn holon_secrets::KeychainStore, name: &str) -> Option<String> {
    let account = normalize_var_name(name);
    let stored = match keychain.load(&account) {
        Ok(stored) => stored,
        Err(e) => {
            tracing::warn!(
                "[integration_vars] could not read '{name}' from the keychain ({e:#}); treating \
                 it as unset, so any integration referencing it will report itself unconfigured"
            );
            return None;
        }
    };
    let bytes = stored?;
    match String::from_utf8(bytes) {
        Ok(value) => (!value.is_empty()).then_some(value),
        Err(_) => {
            // Length and name only: the bytes are the secret.
            tracing::warn!(
                "[integration_vars] the keychain entry for '{name}' is not valid UTF-8, so it \
                 cannot be substituted into a sidecar; treating it as unset"
            );
            None
        }
    }
}
