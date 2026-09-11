//! A secret the user types into Settings does not land in `holon.toml`.
//!
//! `holon.toml` is plaintext user config. It is read by anything that can read
//! the home directory, it gets copied into a new profile, and it is what a
//! user pastes into a bug report. A credential belongs in the OS keychain, and
//! the preference schema already says which fields are credentials
//! (`PrefType::Secret`).
//!
//! The legacy plaintext layer is still READ — an existing install keeps
//! working — but nothing new is written into it.

use std::collections::HashSet;

use holon_frontend::config::HolonConfig;
use holon_frontend::preferences::PrefKey;

/// Synthetic. Distinctive enough that a substring search over the whole file
/// cannot miss it, and obviously not a real credential.
const SECRET: &str = "SYNTHETIC-SECRET-VALUE-MUST-NOT-REACH-DISK";

fn secret_keys() -> HashSet<PrefKey> {
    [PrefKey::new("todoist.api_key")].into_iter().collect()
}

#[test]
fn a_secret_preference_is_never_written_to_holon_toml() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = HolonConfig::default();
    config.set_secret_keys(secret_keys());
    config.set_preference(
        &PrefKey::new("todoist.api_key"),
        toml::Value::String(SECRET.to_string()),
    );

    config.save_runtime(dir.path()).expect("save");

    let written = std::fs::read_to_string(dir.path().join("holon.toml")).expect("holon.toml");
    assert!(
        !written.contains(SECRET),
        "a secret preference reached plaintext config:\n{written}"
    );
}

/// The other half: an ordinary preference must still persist, so the exclusion
/// cannot be a blanket one that quietly stops saving settings.
#[test]
fn an_ordinary_preference_is_still_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = HolonConfig::default();
    config.set_secret_keys(secret_keys());
    config.set_preference(
        &PrefKey::new("ui.theme"),
        toml::Value::String("dracula".to_string()),
    );

    config.save_runtime(dir.path()).expect("save");

    let written = std::fs::read_to_string(dir.path().join("holon.toml")).expect("holon.toml");
    assert!(
        written.contains("dracula"),
        "an ordinary preference must persist:\n{written}"
    );
}

/// A pre-existing cleartext copy is REMOVED by the next save, so the first
/// time a user edits anything the old plaintext token stops being on disk.
/// This is the migration, and it costs nothing.
#[test]
fn an_existing_cleartext_secret_is_dropped_on_the_next_save() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("holon.toml"),
        format!("[preferences]\n\"todoist.api_key\" = \"{SECRET}\"\n"),
    )
    .expect("seed holon.toml");

    let mut config = HolonConfig::default();
    config.set_secret_keys(secret_keys());
    config.set_preference(
        &PrefKey::new("ui.theme"),
        toml::Value::String("dracula".to_string()),
    );
    config.save_runtime(dir.path()).expect("save");

    let written = std::fs::read_to_string(dir.path().join("holon.toml")).expect("holon.toml");
    assert!(
        !written.contains(SECRET),
        "the pre-existing cleartext secret survived a save:\n{written}"
    );
}

#[test]
fn the_schema_says_which_keys_are_secrets() {
    let registry = holon_frontend::theme::ThemeRegistry::load(None);
    let defs = holon_frontend::preferences::define_preferences(&registry);
    let secrets = holon_frontend::preferences::secret_keys(&defs);
    assert!(
        secrets.contains(&PrefKey::new("todoist.api_key")),
        "the Todoist API key is a credential"
    );
    assert!(
        secrets.contains(&PrefKey::new("shopping.list_url")),
        "the shopping list URL carries a capability token"
    );
    assert!(
        !secrets.contains(&PrefKey::new("ui.theme")),
        "a theme name is not a credential"
    );
}
