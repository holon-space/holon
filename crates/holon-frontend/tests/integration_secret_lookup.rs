//! Where an integration's `${VAR}` value comes from, and which layer wins.
//!
//! Three layers, in this order:
//!
//! 1. the ENVIRONMENT, so a sandbox launched with an exported credential
//!    authenticates as that credential regardless of the persisted profile;
//! 2. the KEYCHAIN, the protected store;
//! 3. the PREFERENCE in `holon.toml`, which is PLAINTEXT.
//!
//! The keychain outranks the preference because of what the third line says.
//! If a value sits in both, the protected one must be the one in force —
//! otherwise moving a token into the keychain and forgetting to clear the old
//! preference leaves the cleartext copy silently authenticating, which is the
//! silent-degradation failure the project forbids.

use std::collections::HashMap;

use holon_frontend::integration_vars::preference_var_lookup;
use holon_frontend::preferences::PrefKey;
use holon_secrets::InMemoryKeychainStore;
use holon_secrets::KeychainStore;
use holon_secrets::secret_account;

/// Synthetic throughout: a real value here would be a live credential in a
/// committed file.
const ENV_VALUE: &str = "https://shop.example/!SYNTHETIC-env/api";
const KEYCHAIN_VALUE: &str = "https://shop.example/!SYNTHETIC-keychain/api";
const PREF_VALUE: &str = "https://shop.example/!SYNTHETIC-pref/api";

fn prefs(pairs: &[(&str, &str)]) -> HashMap<PrefKey, toml::Value> {
    pairs
        .iter()
        .map(|(k, v)| (PrefKey::new(k), toml::Value::String((*v).into())))
        .collect()
}

fn keychain(pairs: &[(&str, &str)]) -> InMemoryKeychainStore {
    let store = InMemoryKeychainStore::new();
    for (var, value) in pairs {
        store
            .store(&secret_account(var), value.as_bytes())
            .expect("the test keychain stores");
    }
    store
}

#[test]
fn a_keychain_entry_resolves_the_variable() {
    let kc = keychain(&[("SHOPPING_LIST_URL", KEYCHAIN_VALUE)]);
    let lookup = preference_var_lookup(&prefs(&[]), |_| None, &kc);
    assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(KEYCHAIN_VALUE));
}

#[test]
fn the_keychain_outranks_the_plaintext_preference() {
    let kc = keychain(&[("SHOPPING_LIST_URL", KEYCHAIN_VALUE)]);
    let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", PREF_VALUE)]), |_| None, &kc);
    assert_eq!(
        lookup("SHOPPING_LIST_URL").as_deref(),
        Some(KEYCHAIN_VALUE),
        "the protected copy must be the one in force"
    );
}

/// Unchanged from before the keychain existed, and deliberately so: the
/// environment-first rule is what makes a sandbox authenticate as what it was
/// launched with.
#[test]
fn the_environment_still_outranks_everything() {
    let kc = keychain(&[("SHOPPING_LIST_URL", KEYCHAIN_VALUE)]);
    let lookup = preference_var_lookup(
        &prefs(&[("shopping.list_url", PREF_VALUE)]),
        |name| (name == "SHOPPING_LIST_URL").then(|| ENV_VALUE.to_string()),
        &kc,
    );
    assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(ENV_VALUE));
}

/// A preference still resolves when nothing better exists: the legacy layer is
/// read, not dropped, so an existing install keeps working.
#[test]
fn a_preference_still_resolves_when_no_secret_is_stored() {
    let kc = keychain(&[]);
    let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", PREF_VALUE)]), |_| None, &kc);
    assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(PREF_VALUE));
}

#[test]
fn an_unconfigured_variable_stays_unresolved() {
    let kc = keychain(&[]);
    let lookup = preference_var_lookup(&prefs(&[]), |_| None, &kc);
    assert_eq!(lookup("SHOPPING_LIST_URL"), None);
}

/// An empty stored value is not a configured one, in every layer — otherwise a
/// cleared field reads as "configured" and the integration fails at request
/// time instead of saying it is unconfigured.
#[test]
fn an_empty_keychain_entry_falls_through() {
    let kc = keychain(&[("SHOPPING_LIST_URL", "")]);
    let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", PREF_VALUE)]), |_| None, &kc);
    assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(PREF_VALUE));
}

/// The dotted preference key and the underscored variable are one account, so
/// a secret stored under either spelling is found by the other.
#[test]
fn the_dotted_and_underscored_spellings_are_one_account() {
    let kc = keychain(&[("shopping.list_url", KEYCHAIN_VALUE)]);
    let lookup = preference_var_lookup(&prefs(&[]), |_| None, &kc);
    assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(KEYCHAIN_VALUE));
}

/// An exported-but-blank variable is not a configured value: it must fall
/// through rather than fail the expansion as if it had been set. Carried over
/// from the unit tests this file replaced, now with the keychain in the chain.
#[test]
fn an_empty_export_falls_through() {
    let kc = keychain(&[]);
    let lookup = preference_var_lookup(
        &prefs(&[("shopping.list_url", PREF_VALUE)]),
        |_| Some(String::new()),
        &kc,
    );
    assert_eq!(lookup("SHOPPING_LIST_URL").as_deref(), Some(PREF_VALUE));
}

#[test]
fn an_empty_preference_is_not_a_configured_value() {
    let kc = keychain(&[]);
    let lookup = preference_var_lookup(&prefs(&[("shopping.list_url", "")]), |_| None, &kc);
    assert_eq!(lookup("SHOPPING_LIST_URL"), None);
}

/// The dotted preference key and the underscored variable normalize alike, so
/// one value serves both spellings across all three layers.
#[test]
fn a_dotted_key_and_an_underscored_variable_are_the_same_name() {
    assert_eq!(
        holon_frontend::integration_vars::normalize_var_name("SHOPPING_LIST_URL"),
        holon_frontend::integration_vars::normalize_var_name("shopping.list_url")
    );
}
