//! Two secrets must never share one keychain entry — and one secret must keep
//! its two spellings.
//!
//! These pull in opposite directions and both are required, which is why the
//! answer is not "make the mapping injective".
//!
//! REQUIRED COLLAPSE. A sidecar references `${SHOPPING_LIST_URL}`; the
//! Settings field that fills it is the preference `shopping.list_url`. The
//! resolver matches them by normalizing both, and the keychain account is that
//! same normal form, so Settings writes and the sidecar reads hit ONE entry.
//! An injective mapping would put the written value and the read value in two
//! different accounts and the integration would silently never be configured.
//! `crates/holon-app/tests/settings_shopping_list_url_credential.rs` has
//! asserted this equality since before the keychain existed.
//!
//! FORBIDDEN COLLAPSE. Two DISTINCT declared preference keys — `foo.bar` and
//! `foo_bar` — also normalize alike, and there the collapse is a defect: the
//! second save silently overwrites the first, and one of the two integrations
//! authenticates with the other's credential.
//!
//! Since the mapping cannot distinguish them, the schema must not contain such
//! a pair, and that is checked rather than assumed.

use holon_frontend::preferences::PrefKey;
use holon_frontend::preferences::PreferenceDef;
use holon_secrets::secret_account;

#[test]
fn the_two_spellings_of_one_secret_still_share_an_account() {
    assert_eq!(
        secret_account("SHOPPING_LIST_URL"),
        secret_account("shopping.list_url"),
        "the variable a sidecar references and the preference key that fills it MUST address one \
         keychain entry, or Settings writes where nothing reads"
    );
}

#[test]
fn distinct_keys_that_would_share_an_account_are_refused() {
    let defs = vec![
        PreferenceDef::secret_for_test(PrefKey::new("foo.bar")),
        PreferenceDef::secret_for_test(PrefKey::new("foo_bar")),
    ];
    let err = holon_frontend::preferences::assert_secret_accounts_are_distinct(&defs)
        .expect_err("two keys sharing one keychain account must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("foo.bar") && msg.contains("foo_bar"),
        "the refusal must name BOTH colliding keys so the author can see the pair; got: {msg}"
    );
}

#[test]
fn the_shipped_schema_has_no_colliding_secrets() {
    let registry = holon_frontend::theme::ThemeRegistry::load(None);
    let defs = holon_frontend::preferences::define_preferences(&registry);
    holon_frontend::preferences::assert_secret_accounts_are_distinct(&defs)
        .expect("the shipped preference schema must not declare two secrets sharing an account");
}

/// The check is about SECRETS, because only those become keychain accounts.
/// Two ordinary keys that normalize alike are merely two settings rows.
#[test]
fn ordinary_keys_may_normalize_alike() {
    let defs = vec![
        PreferenceDef::text_for_test(PrefKey::new("foo.bar")),
        PreferenceDef::text_for_test(PrefKey::new("foo_bar")),
    ];
    holon_frontend::preferences::assert_secret_accounts_are_distinct(&defs)
        .expect("non-secret keys never reach the keychain, so they cannot collide there");
}
