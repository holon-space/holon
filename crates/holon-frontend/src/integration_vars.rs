//! Resolving the `${VAR}` references an MCP integration sidecar declares.
//!
//! A sidecar names its credentials as `${TODOIST_API_KEY}` /
//! `${SHOPPING_LIST_URL}` so the value stays out of the committed YAML. This
//! module says where the value comes from: the environment first, then the
//! preference the Settings UI writes to `holon.toml`.
//!
//! Environment-first is what makes a sandbox launched with an exported
//! credential authenticate as that credential regardless of the persisted
//! profile. The cost is that a stale export silently outranks what the user
//! typed into Settings, so [`crate::preferences::env_shadowed_keys`] marks
//! those fields read-only rather than letting the UI show a value nothing
//! reads.

use std::collections::HashMap;

use crate::preferences::PrefKey;

/// The form a variable name and a preference key are compared in: lowercase,
/// with `.` and `_` as the same separator. `${SHOPPING_LIST_URL}` and the
/// `shopping.list_url` preference both normalize to `shopping_list_url`.
pub fn normalize_var_name(s: &str) -> String {
    s.to_ascii_lowercase().replace('.', "_")
}

/// A `${VAR}` resolver over `preferences`, consulting `env` first.
///
/// Empty values resolve as unset in both layers, so an exported-but-blank
/// variable falls through to the preference instead of failing the
/// integration's expansion as if it had been configured.
pub fn preference_var_lookup(
    preferences: &HashMap<PrefKey, toml::Value>,
    env: impl Fn(&str) -> Option<String> + Send + Sync,
) -> impl Fn(&str) -> Option<String> + Send + Sync {
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
            .or_else(|| by_norm.get(&normalize_var_name(name)).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic throughout: a real list URL is a live credential and never
    /// belongs in a fixture.
    const PREF_URL: &str = "https://shop.example/!abc123SYNTHETICprefTOKEN/api";
    const ENV_URL: &str = "https://shop.example/!abc123SYNTHETICenvTOKEN/api";

    fn prefs(pairs: &[(&str, &str)]) -> HashMap<PrefKey, toml::Value> {
        pairs
            .iter()
            .map(|(k, v)| (PrefKey::new(k), toml::Value::String((*v).into())))
            .collect()
    }

    #[test]
    fn a_preference_resolves_the_variable_when_the_environment_is_unset() {
        let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", PREF_URL)]), |_| None);
        assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(PREF_URL));
    }

    #[test]
    fn the_environment_outranks_the_preference() {
        let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", PREF_URL)]), |name| {
            (name == "SHOPPING_LIST_URL").then(|| ENV_URL.to_string())
        });
        assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(ENV_URL));
    }

    #[test]
    fn an_empty_export_falls_through_to_the_preference() {
        let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", PREF_URL)]), |_| {
            Some(String::new())
        });
        assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(PREF_URL));
    }

    #[test]
    fn an_unconfigured_variable_stays_unresolved() {
        let lookup = preference_var_lookup(&prefs(&[]), |_| None);
        assert_eq!(lookup("SHOPPING_LIST_URL"), None);
    }

    #[test]
    fn an_empty_preference_is_not_a_configured_value() {
        let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", "")]), |_| None);
        assert_eq!(lookup("SHOPPING_LIST_URL"), None);
    }

    #[test]
    fn a_dotted_key_and_an_underscored_variable_are_the_same_name() {
        assert_eq!(
            normalize_var_name("SHOPPING_LIST_URL"),
            normalize_var_name("shopping.list_url")
        );
    }
}
