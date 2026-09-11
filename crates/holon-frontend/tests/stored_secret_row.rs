//! A secret that IS configured must not read as "Not set".
//!
//! Once Settings writes credentials to the OS keychain instead of to plaintext
//! `holon.toml`, the preference map is empty for that key on the next boot.
//! The integration works — the `${VAR}` resolver reads the keychain — but the
//! settings row renders from the preference map, so it says "Not set" about a
//! value that is in force.
//!
//! That is the silent-degradation failure the project's error policy puts in
//! the one tier that is never acceptable: a fallback is allowed only when it is
//! DISCLOSED. The row has to say the secret is stored, without showing it.
//!
//! Three states, three different things to say:
//!   nothing stored anywhere        -> "Not set"
//!   stored in the keychain         -> a mask plus "Stored in the keychain"
//!   overridden by the environment  -> the existing locked row
//!
//! The row DATA is asserted here rather than pixels: this is the decision the
//! renderer paints, and it is the layer where all three states can be reached.
//! `frontends/gpui/tests/` covers what the window does with the flag.

use std::collections::HashMap;
use std::collections::HashSet;

use holon_frontend::preferences::PrefKey;
use holon_frontend::preferences::preferences_to_rows;
use holon_frontend::theme::ThemeRegistry;

const SECRET_KEY: &str = "todoist.api_key";

fn rows_with(
    stored: &HashSet<PrefKey>,
    current: &HashMap<PrefKey, toml::Value>,
) -> HashMap<String, holon_api::Value> {
    let registry = ThemeRegistry::load(None);
    let defs = holon_frontend::preferences::define_preferences(&registry);
    let rows = preferences_to_rows(&defs, current, &HashSet::new(), stored);
    rows.into_iter()
        .find(|r| {
            r.get("key")
                .and_then(|v| v.as_string())
                .is_some_and(|k| k == SECRET_KEY)
        })
        .expect("the Todoist API key is a settings row")
}

fn stored_flag(row: &HashMap<String, holon_api::Value>) -> bool {
    matches!(
        row.get("secret_stored"),
        Some(holon_api::Value::Boolean(true))
    )
}

#[test]
fn a_secret_held_only_in_the_keychain_reports_itself_stored() {
    let stored = HashSet::from([PrefKey::new(SECRET_KEY)]);
    let row = rows_with(&stored, &HashMap::new());
    assert!(
        stored_flag(&row),
        "the value is not in holon.toml but IS in the keychain — the row must say so rather than \
         read as unset: {row:?}"
    );
}

#[test]
fn a_secret_stored_nowhere_reports_itself_unset() {
    let row = rows_with(&HashSet::new(), &HashMap::new());
    assert!(
        !stored_flag(&row),
        "nothing is stored, so the row must not claim a secret is held: {row:?}"
    );
}

/// The legacy plaintext layer still counts as stored: an install that has not
/// re-saved its settings keeps a working, visible configuration.
#[test]
fn a_legacy_plaintext_secret_still_reports_itself_stored() {
    let current = HashMap::from([(
        PrefKey::new(SECRET_KEY),
        toml::Value::String("SYNTHETIC-LEGACY-VALUE".into()),
    )]);
    let row = rows_with(&HashSet::new(), &current);
    assert!(
        stored_flag(&row),
        "a value still in holon.toml is configured, whatever the keychain holds: {row:?}"
    );
}

/// The flag must never carry the secret itself, in any of the three states.
#[test]
fn no_row_state_carries_the_secret_value() {
    let secret = "SYNTHETIC-MUST-NOT-APPEAR";
    let current = HashMap::from([(PrefKey::new(SECRET_KEY), toml::Value::String(secret.into()))]);
    let stored = HashSet::from([PrefKey::new(SECRET_KEY)]);
    // The row DOES carry `value` for the editable field; what must not happen
    // is the new flag duplicating it anywhere else.
    let row = rows_with(&stored, &current);
    for (name, v) in &row {
        if name == "value" {
            continue;
        }
        if let holon_api::Value::String(s) = v {
            assert!(
                !s.contains(secret),
                "row field {name:?} carries the secret value: {s:?}"
            );
        }
    }
}

/// A non-secret preference never reports itself stored, whatever is in the
/// keychain — the flag is about credentials, and a theme name is not one.
#[test]
fn an_ordinary_preference_never_reports_itself_stored() {
    let registry = ThemeRegistry::load(None);
    let defs = holon_frontend::preferences::define_preferences(&registry);
    let stored = HashSet::from([PrefKey::new("ui.theme")]);
    let rows = preferences_to_rows(&defs, &HashMap::new(), &HashSet::new(), &stored);
    let theme = rows
        .into_iter()
        .find(|r| {
            r.get("key")
                .and_then(|v| v.as_string())
                .is_some_and(|k| k == "ui.theme")
        })
        .expect("the theme is a settings row");
    assert!(!stored_flag(&theme), "{theme:?}");
}
