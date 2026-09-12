//! A connection a user installed gets a Settings field for its credential.
//!
//! The secret fields were a compile-time list of two keys, written when every
//! connection was compiled in. The dogfood pass switched on an introduced
//! connection, watched it connect and authenticate, and found no field for its
//! token anywhere in Settings — the surface that exists to configure
//! connections could not configure the one kind of connection this feature
//! added.
//!
//! The property that makes a derived field WORK rather than merely appear is
//! the one asserted hardest here: the key must address the same keychain
//! account the `${VAR}` resolver looks the value up under. A field that stores
//! a credential nothing reads is the silent-degradation case, and it is exactly
//! what a key spelled to mirror the bundled `todoist.api_key` convention would
//! produce for a hyphenated connection name.
//!
//! @pbt kind harness
//! @pbt covers introduced-connection-secret-field — an introduced connection's
//! `${VAR}` becomes a secret Settings field addressing the same account the
//! resolver reads

use holon_app::introduced_secrets::introduced_secret_fields;
use holon_frontend::integration_vars::normalize_var_name;
use holon_frontend::preferences::PrefType;
use holon_frontend::preferences::introduced_secret_preferences;

/// A connection calling one host with `var` in its query string.
fn sidecar_using(var: &str) -> String {
    format!(
        "schema_version: {}\ndisplay_name: \"Calendar\"\nutcp:\n  utcp_version: \"1.1.3\"\n  \
         manual_version: \"1.0.0\"\n  tools:\n    - name: list\n      tool_call_template:\n        \
         call_template_type: http\n        http_method: GET\n        url: \
         https://api.example.com/things?t=${{{var}}}\nholon:\n  tools:\n    list: \
         {{}}\nentities: {{}}\ntools: {{}}\n",
        holon_mcp_client::SIDECAR_SCHEMA_VERSION
    )
}

#[test]
fn an_introduced_connections_variable_becomes_a_secret_settings_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("fixturebox.yaml"),
        sidecar_using("FIXTUREBOX_TOKEN"),
    )
    .expect("write the connection file");

    let fields = introduced_secret_fields(dir.path());
    let defs = introduced_secret_preferences(&fields);

    let def = defs
        .iter()
        .find(|d| d.env_override.as_deref() == Some("FIXTUREBOX_TOKEN"))
        .unwrap_or_else(|| {
            panic!(
                "an installed connection asking for ${{FIXTUREBOX_TOKEN}} must get a Settings \
                 field for it — without one it can be switched on and connect with nowhere in the \
                 app to type its credential. Derived fields: {:?}",
                defs.iter().map(|d| d.key.to_string()).collect::<Vec<_>>()
            )
        });

    assert!(
        matches!(def.pref_type, PrefType::Secret),
        "the field must be typed Secret: that is what routes the value to the keychain instead of \
         plaintext holon.toml, and what makes the row paint a mask"
    );
    assert_eq!(
        normalize_var_name(def.key.as_str()),
        normalize_var_name("FIXTUREBOX_TOKEN"),
        "the field's key must address the SAME keychain account the `${{VAR}}` resolver reads, or \
         the user types a credential that nothing ever looks up"
    );
    assert!(
        def.description.contains("fixturebox.yaml"),
        "a credential field for a connection nobody reviewed must name the file that asked for \
         it; got {:?}",
        def.description
    );
}

/// A hyphenated connection name is where a key spelled like the bundled
/// convention would silently stop matching. Stated separately because it is the
/// case a reader would otherwise assume is covered by the one above.
#[test]
fn a_hyphenated_connections_field_still_addresses_the_right_account() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("my-own-thing.yaml"),
        sidecar_using("MY_OWN_THING_TOKEN"),
    )
    .expect("write the connection file");

    let defs = introduced_secret_preferences(&introduced_secret_fields(dir.path()));
    let def = defs
        .iter()
        .find(|d| d.env_override.as_deref() == Some("MY_OWN_THING_TOKEN"))
        .expect("the hyphen-named connection must get its field too");

    assert_eq!(
        normalize_var_name(def.key.as_str()),
        normalize_var_name("MY_OWN_THING_TOKEN"),
        "'-' and '_' are one separator when a name is COMPARED but not when a keychain account is \
         SPELLED, which is the trap this case exists to hold shut"
    );
}

/// The rule that keeps a derived key from colliding with a declared one, found
/// by the gate rather than by design: an installed file referencing
/// `${TODOIST_API_KEY}` derived the key `todoist_api_key`, which folds onto the
/// same keychain account as the bundled `todoist.api_key`, and the boot's
/// distinctness check then refused to start the app at all.
///
/// Only the connection's OWN namespace is derived. That is the same rule
/// `check_secret_namespace` refuses the connection for, so a field here would
/// have offered to fill in a credential this build will not use.
#[test]
fn a_reference_outside_the_connections_namespace_derives_no_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("calendar.yaml"),
        sidecar_using("TODOIST_API_KEY"),
    )
    .expect("write the connection file");

    assert!(
        introduced_secret_fields(dir.path()).is_empty(),
        "a connection reaching for another connection's credential must derive no Settings field \
         — it is refused at load anyway, and the derived key would collide with the bundled one"
    );
}

/// Two connections whose names differ only in '-' versus '_' own the SAME
/// namespace, and the roster refuses both. This asserts the refusal reaches the
/// derivation — the account boundary is upstream of this module, so a field for
/// an ambiguous credential is not merely filtered here, it never arrives.
///
/// The boundary rule itself is owned by
/// `holon-mcp-client/tests/introduced_connection_account_claims.rs`.
#[test]
fn a_refused_connection_derives_no_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    for stem in ["my-thing", "my_thing"] {
        std::fs::write(
            dir.path().join(format!("{stem}.yaml")),
            sidecar_using("MY_THING_TOKEN"),
        )
        .expect("write the connection file");
    }

    assert!(
        introduced_secret_fields(dir.path()).is_empty(),
        "neither connection is admitted, so neither may contribute a credential field — a value \
         typed into one would authenticate whichever of them the scan reached last"
    );
}

/// Bundled connections keep their declared fields and gain no derived twin — a
/// credential with two fields is a credential the user can set in two places
/// and read back in one.
#[test]
fn a_bundled_connection_gets_no_derived_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        introduced_secret_fields(dir.path()).is_empty(),
        "a directory with no installed file introduces no connection, so it derives no field"
    );
}
