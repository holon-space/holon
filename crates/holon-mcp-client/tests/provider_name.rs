//! A provider id is PARSED, so a name read off disk cannot be a path.
//!
//! Presence is becoming a union of the compiled-in bundle and the files a user
//! drops into the integrations directory. A file stem therefore reaches the
//! loader, and from there the state file's path, the sidecar's path and the
//! keychain account. Every one of those is composed by joining the name onto a
//! directory, so a name that can hold a separator or `..` is a directory
//! escape at three sites at once.
//!
//! [`ProviderName::parse`] is the one place that decides. These cases are the
//! shapes that escape would take.

use holon_mcp_client::ProviderName;

#[test]
fn a_plain_name_parses() {
    for raw in ["github", "gcal", "claude-history", "jsonplaceholder", "x1"] {
        let name = ProviderName::parse(raw)
            .unwrap_or_else(|e| panic!("'{raw}' must be a usable provider name: {e:#}"));
        assert_eq!(name.as_str(), raw);
    }
}

#[test]
fn an_empty_name_is_refused() {
    assert!(ProviderName::parse("").is_err());
}

/// `..yaml` has the stem `..`, so this is the shape a traversal actually
/// arrives in — not a name somebody typed.
#[test]
fn a_parent_traversal_is_refused() {
    for raw in ["..", ".", "../etc", "..\\etc"] {
        assert!(
            ProviderName::parse(raw).is_err(),
            "'{raw}' must not parse as a provider name"
        );
    }
}

#[test]
fn a_path_separator_is_refused() {
    for raw in ["a/b", "/abs", "a\\b", "a b"] {
        assert!(
            ProviderName::parse(raw).is_err(),
            "'{raw}' must not parse as a provider name"
        );
    }
}

/// A case-insensitive filesystem would make `GitHub.yaml` and `github.yaml`
/// two names for one file, and two entries for one connection.
#[test]
fn an_uppercase_name_is_refused() {
    assert!(ProviderName::parse("GitHub").is_err());
}

#[test]
fn an_over_long_name_is_refused() {
    let ok = "a".repeat(64);
    let too_long = "a".repeat(65);
    assert!(ProviderName::parse(&ok).is_ok(), "64 bytes is the limit");
    assert!(ProviderName::parse(&too_long).is_err());
}

/// A name must not carry anything the shell or a URL would read specially;
/// these are the characters a hand-written filename is most likely to hold.
#[test]
fn punctuation_is_refused() {
    for raw in ["a.b", "a:b", "a$b", "a;b", "a\0b", "a\nb", "-lead"] {
        assert!(
            ProviderName::parse(raw).is_err(),
            "{raw:?} must not parse as a provider name"
        );
    }
}

/// The bundle's own names must satisfy the same parser the user's files face.
/// A bundled name that stopped parsing would take the whole boot down at
/// `ProviderName::bundled`, so this states it as a test rather than leaving it
/// to a panic in production.
#[test]
fn every_bundled_name_parses() {
    for sidecar in holon_mcp_client::BUNDLED_SIDECARS {
        ProviderName::parse(sidecar.provider).unwrap_or_else(|e| {
            panic!(
                "bundled provider '{}' is not a usable provider name: {e:#}",
                sidecar.provider
            )
        });
    }
}
