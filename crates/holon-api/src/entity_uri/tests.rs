use super::*;

#[test]
fn parse_valid_opaque() {
    let uri = EntityUri::parse("block:abc-123").unwrap();
    assert_eq!(uri.scheme(), "block");
    assert_eq!(uri.id(), "abc-123");
    assert!(uri.is_block());
}

#[test]
fn parse_valid_hierarchical() {
    let uri = EntityUri::parse("https://jira.example.com/ISSUE-1").unwrap();
    assert_eq!(uri.scheme(), "https");
    assert!(!uri.is_sentinel());
    assert!(!uri.is_block());
}

#[test]
fn parse_invalid() {
    // No scheme → not a valid absolute URI
    assert!(EntityUri::parse("just-a-string").is_err());
}

/// Bare synthetic ids with `::` separators must parse as BLOCK ids, not
/// as a URI with the id's head for a scheme. `root-layout::src::0` is a
/// valid RFC 3986 URI (scheme `root-layout`, path `:src::0`) — accepting
/// that parse made `is_block()` false and `resolve_to_tree_id` miss
/// existing Loro nodes (layout-swap Loro divergence, 2026-06-11).
#[test]
fn from_raw_double_colon_synthetic_id_is_block() {
    for raw in [
        "root-layout::src::0",
        "default-main-panel::render::0",
        "block:root-layout::src::0",
    ] {
        // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
        let uri = EntityUri::from_raw(raw);
        assert!(uri.is_block(), "{raw:?} → {uri:?} must be a block URI");
        assert_eq!(uri.id(), raw.strip_prefix("block:").unwrap_or(raw));
        // Idempotent: re-parsing the schemed form round-trips.
        // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
        let again = EntityUri::from_raw(uri.as_str());
        assert_eq!(again, uri);
    }
    // Real schemes are untouched.
    // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
    assert_eq!(EntityUri::from_raw("person:abc").scheme(), "person");
    // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
    assert!(EntityUri::from_raw("sentinel:no_parent").is_sentinel());
    assert_eq!(
        // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
        EntityUri::from_raw("https://jira.example.com/ISSUE-1").scheme(),
        "https"
    );
}

#[test]
fn constructors() {
    let block = EntityUri::block("my-id");
    assert_eq!(block.as_str(), "block:my-id");
    assert!(block.is_block());

    let block2 = EntityUri::block("b-1");
    assert_eq!(block2.as_str(), "block:b-1");
    assert!(block2.is_block());

    let np = EntityUri::no_parent();
    assert_eq!(np.as_str(), "sentinel:no_parent");
    assert!(np.is_sentinel());
}

#[test]
fn display() {
    let uri = EntityUri::block("test");
    assert_eq!(uri.to_string(), "block:test");
}

#[test]
fn value_round_trip() {
    let uri = EntityUri::block("x");
    let v: Value = uri.clone().into();
    assert_eq!(v, Value::String("block:x".into()));
    let uri2: EntityUri = v.try_into().unwrap();
    assert_eq!(uri, uri2);
}

#[test]
fn serde_round_trip() {
    let uri = EntityUri::block("abc");
    let json = serde_json::to_string(&uri).unwrap();
    assert_eq!(json, "\"block:abc\"");
    let parsed: EntityUri = serde_json::from_str(&json).unwrap();
    assert_eq!(uri, parsed);
}

#[test]
fn random_constructors_are_unique() {
    let a = EntityUri::block_random();
    let b = EntityUri::block_random();
    assert_ne!(a, b);
}

#[test]
fn full_https_uri() {
    let uri = EntityUri::parse("https://todoist.com/tasks/12345").unwrap();
    assert_eq!(uri.scheme(), "https");
    // For hierarchical URIs, path includes the leading /
    assert_eq!(uri.id(), "/tasks/12345");
}

#[test]
fn parse_uuid_block_uri() {
    let uri = EntityUri::parse("block:f3c6fd2d-4784-45b4-9b7c-c05300474ff4").unwrap();
    assert_eq!(uri.scheme(), "block");
    assert!(uri.is_block());
}

#[test]
fn equality_and_hash() {
    let a = EntityUri::block("same");
    let b = EntityUri::block("same");
    let c = EntityUri::block("different");
    assert_eq!(a, b);
    assert_ne!(a, c);

    let mut set = std::collections::HashSet::new();
    set.insert(a.clone());
    assert!(set.contains(&b));
    assert!(!set.contains(&c));
}

#[test]
fn file_uri_with_spaces_is_percent_encoded() {
    let uri = EntityUri::file("Projects/Holon/Entity Identity.org");
    assert_eq!(uri.as_str(), "file:Projects/Holon/Entity%20Identity.org");
    assert!(uri.is_file());
    assert_eq!(uri.id(), "Projects/Holon/Entity%20Identity.org");
}

#[test]
fn file_uri_with_special_chars_is_percent_encoded() {
    let uri = EntityUri::file("path/to/file#1.org");
    assert_eq!(uri.as_str(), "file:path/to/file%231.org");
}

#[test]
fn file_uri_absolute_path() {
    let uri = EntityUri::file("/absolute/path/file.org");
    assert_eq!(uri.as_str(), "file:/absolute/path/file.org");
}

#[test]
fn file_uri_preserves_hyphens_and_dots() {
    let uri = EntityUri::file("my-dir/sub_dir/file.name-v1.org");
    assert_eq!(uri.as_str(), "file:my-dir/sub_dir/file.name-v1.org");
}

#[test]
fn file_uri_round_trips_through_parse() {
    let uri = EntityUri::file("Projects/Holon/Entity Identity.org");
    let s = uri.to_string();
    let parsed = EntityUri::parse(&s).unwrap();
    assert_eq!(uri, parsed);
    assert!(parsed.is_file());
}

/// The `condition:` scheme is an ENTITY scheme, so a condition row's id
/// must survive the one classifier every `id` column goes through.
#[test]
fn a_condition_uri_classifies_as_its_own_entity() {
    let uri = EntityUri::condition("todoist", "integration-connect-failed");
    assert_eq!(
        uri.to_string(),
        "condition:todoist:integration-connect-failed"
    );
    assert!(!uri.is_block(), "a condition is not a block");
    assert_eq!(
        crate::widget_spec::row_id_of_str(uri.as_str()).entity(),
        Some(uri.clone()),
        "the classifier must return the same URI, not a re-schemed one"
    );
}

/// A subject can be a canonical file path (`loro_seams.rs`), so the
/// separator must stay unambiguous when the subject carries `/` or `:`.
#[test]
fn an_awkward_subject_still_round_trips_through_the_classifier() {
    for subject in ["/Users/m/a b/vault.org", "host:1234", "a%b", "todoist"] {
        let uri = EntityUri::condition(subject, "integration-connect-failed");
        assert_eq!(
            crate::widget_spec::row_id_of_str(uri.as_str()).entity(),
            Some(uri.clone()),
            "subject {subject:?} must round-trip, got {uri}"
        );
    }
}

/// A subject spelled like a known scheme is well-formed here — the
/// double-scheme guard that rejects it is about BARE ids, not about a
/// two-part condition name.
#[test]
fn a_subject_spelled_like_a_scheme_is_accepted() {
    for subject in ["block", "file", "sentinel"] {
        let uri = EntityUri::condition(subject, "integration-connect-failed");
        assert_eq!(uri.scheme(), "condition", "got {uri}");
        assert!(
            crate::widget_spec::row_id_of_str(uri.as_str())
                .entity()
                .is_some()
        );
    }
}

#[test]
#[should_panic(expected = "names the thing it is about")]
fn an_empty_condition_subject_fails_loudly() {
    let _ = EntityUri::condition("", "integration-connect-failed");
}

#[test]
#[should_panic(expected = "names its kind")]
fn an_empty_condition_kind_fails_loudly() {
    let _ = EntityUri::condition("todoist", "");
}

fn decoded(component: &str) -> String {
    fluent_uri::encoding::EStr::<fluent_uri::encoding::encoder::Path>::new_or_panic(component)
        .decode()
        .into_string()
        .expect("an encoded str decodes to UTF-8")
        .into_owned()
}

mod from_segments {
    use proptest::prelude::*;

    use super::super::EntityUri;
    use super::decoded;

    proptest! {
        /// Whatever a file name holds, the id names its own entity and
        /// decodes back to exactly the segments and parts it was built from.
        #[test]
        fn any_segments_round_trip_as_their_own_entity(
            scheme in "[a-z][a-z0-9-]{0,8}",
            segments in prop::collection::vec("[^/]{1,6}", 1..4),
            parts in prop::collection::vec(".{1,6}", 0..3),
        ) {
            let uri = EntityUri::from_segments(&scheme, &segments, &parts);
            let schemed = EntityUri::schemed(uri.as_str()).expect("the id names its entity");
            prop_assert_eq!(schemed.scheme(), scheme.as_str());
            prop_assert_eq!(&schemed, &uri);

            let mut components = uri.id().split("::");
            let path = components.next().expect("split yields a first piece");
            let back: Vec<String> = path.split('/').map(decoded).collect();
            prop_assert_eq!(back, segments);
            let back: Vec<String> = components.map(decoded).collect();
            prop_assert_eq!(back, parts);
        }

        /// An id of unreserved ASCII is spelled as it was before encoding
        /// existed, so no stored id of such a file changes.
        #[test]
        fn unreserved_ascii_is_spelled_unchanged(
            scheme in "[a-z][a-z0-9-]{0,8}",
            segments in prop::collection::vec("[A-Za-z0-9._~-]{1,8}", 1..4),
            parts in prop::collection::vec("[a-z0-9-]{1,8}", 0..3),
        ) {
            let joined: String = parts.iter().map(|p| format!("::{p}")).collect();
            prop_assert_eq!(
                EntityUri::from_segments(&scheme, &segments, &parts).to_string(),
                format!("{scheme}:{}{joined}", segments.join("/"))
            );
        }
    }

    #[test]
    fn a_space_and_umlauts_are_encoded_like_a_file_path() {
        let uri =
            EntityUri::from_segments("recipe", &["Rezepte", "Grüne Soße.cook"], &["iu", "mehl-0"]);
        assert_eq!(
            uri.as_str(),
            "recipe:Rezepte/Gr%C3%BCne%20So%C3%9Fe.cook::iu::mehl-0"
        );
        assert_eq!(
            uri.id().split("::").next(),
            Some(EntityUri::file("Rezepte/Grüne Soße.cook").id())
        );
    }

    #[test]
    #[should_panic(expected = "non-empty segments")]
    fn an_empty_segment_fails_loudly() {
        let _ = EntityUri::from_segments("recipe", &["Rezepte", ""], &[] as &[&str]);
    }
}
