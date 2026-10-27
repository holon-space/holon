use super::*;

#[test]
fn test_extract_described_link() {
    let content = "This is a [[block:uuid-123][link to block]] in text.";
    let links = extract_links(content, &LinkTargetClassifier::default());

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "block:uuid-123");
    assert_eq!(links[0].text, "link to block");
    assert!(
        matches!(&links[0].classified, LinkTarget::Resolved(uri) if uri.as_str() == "block:uuid-123")
    );
}

#[test]
fn test_extract_bare_link() {
    let content = "See [[ProjectNotes]] for details.";
    let links = extract_links(content, &LinkTargetClassifier::default());

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "ProjectNotes");
    assert_eq!(links[0].text, "ProjectNotes");
    assert!(
        matches!(&links[0].classified, LinkTarget::CreationIntent { scheme, name, .. } if scheme == "block" && name == "ProjectNotes")
    );
}

#[test]
fn test_extract_mixed_links() {
    let content = "A [[PageOne]] then [[block:2][described]] then [[PageThree]].";
    let links = extract_links(content, &LinkTargetClassifier::default());

    assert_eq!(links.len(), 3);
    assert_eq!(links[0].target, "PageOne");
    assert_eq!(links[0].text, "PageOne");
    assert_eq!(links[1].target, "block:2");
    assert_eq!(links[1].text, "described");
    assert_eq!(links[2].target, "PageThree");
    assert_eq!(links[2].text, "PageThree");
}

#[test]
fn test_bare_link_not_confused_with_described() {
    let content = "Only [[target][text]] here.";
    let links = extract_links(content, &LinkTargetClassifier::default());

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "target");
    assert_eq!(links[0].text, "text");
}

#[test]
fn test_extract_multiple_described_links() {
    let content = "First [[block:1][one]] and second [[block:2][two]].";
    let links = extract_links(content, &LinkTargetClassifier::default());

    assert_eq!(links.len(), 2);
    assert_eq!(links[0].target, "block:1");
    assert_eq!(links[1].target, "block:2");
}

#[test]
fn test_extract_link_targets() {
    let content = "[[block:a][A]] and [[PageB]] and [[block:a][A again]].";
    let targets = extract_link_targets(content, &LinkTargetClassifier::default());

    assert_eq!(targets.len(), 2);
    assert!(targets.contains("block:a"));
    assert!(targets.contains("PageB"));
}

#[test]
fn test_strip_links() {
    let content = "See [[block:123][this block]] and [[PageName]] for details.";
    let stripped = strip_links(content, &LinkTargetClassifier::default());

    assert_eq!(stripped, "See this block and PageName for details.");
}

#[test]
fn test_no_links() {
    let content = "Plain text without any links.";
    let links = extract_links(content, &LinkTargetClassifier::default());
    assert!(links.is_empty());
}

#[test]
fn test_positions_are_correct() {
    let content = "A [[Page]] B";
    let links = extract_links(content, &LinkTargetClassifier::default());

    assert_eq!(links.len(), 1);
    assert_eq!(&content[links[0].start..links[0].end], "[[Page]]");
}

// --- New tests for classification + deterministic IDs ---

/// The `doc:` scheme is retired (H7, 2026-07-02) and unregistered, so a
/// `doc:` target resolves to nothing. It is still scheme-SHAPED, so it is
/// an unknown-scheme link rather than a page named `doc:existing-uuid`.
#[test]
fn test_doc_scheme_no_longer_resolved() {
    let target = LinkTargetClassifier::default().classify("doc:existing-uuid");
    assert_eq!(
        target,
        LinkTarget::UnknownScheme("doc:existing-uuid".into())
    );
}

// --- F1a: the three-state classifier ---

#[test]
fn scheme_shape_follows_rfc_3986() {
    let shape = |t: &'static str| link_scheme_shape(t).map(|s| s.as_str());
    assert_eq!(shape("cc-session:abc"), Some("cc-session"));
    assert_eq!(shape("Areas:Work"), Some("Areas"));
    assert_eq!(shape("a+b.c-d:x"), Some("a+b.c-d"));
    // No colon, colon-space, leading non-letter, and an illegal scheme char
    // are all NOT scheme-shaped.
    assert_eq!(shape("Areas/Work"), None);
    assert_eq!(shape("Ketosis: How to lose weight"), None);
    assert_eq!(shape("1up:x"), None);
    assert_eq!(shape("two words:x"), None);
}

#[test]
fn registered_scheme_resolves_to_the_full_uri() {
    let classifier = LinkTargetClassifier::with_schemes(["cc-session"]);
    assert!(
        matches!(classifier.classify("cc-session:abc"), LinkTarget::Resolved(uri) if uri.as_str() == "cc-session:abc")
    );
    assert!(
        matches!(classifier.classify("tag:rust"), LinkTarget::Resolved(uri) if uri.as_str() == "tag:rust")
    );
}

/// The registered set only ever moves a target between `Resolved` and
/// `UnknownScheme`. It can never move one across the page/entity boundary,
/// which is what makes installing or removing an integration safe.
#[test]
fn unregistering_a_scheme_never_turns_its_links_into_pages() {
    let target = "cc-session:abc";
    assert!(matches!(
        LinkTargetClassifier::with_schemes(["cc-session"]).classify(target),
        LinkTarget::Resolved(_)
    ));
    assert_eq!(
        LinkTargetClassifier::default().classify(target),
        LinkTarget::UnknownScheme(target.into())
    );
}

#[test]
fn unregistered_scheme_is_never_a_creation_intent() {
    for target in ["Areas:Work", "cc-sesion:abc", "ftp:example.com"] {
        assert_eq!(
            LinkTargetClassifier::default().classify(target),
            LinkTarget::UnknownScheme(target.into()),
            "{target} must reserve the scheme shape"
        );
    }
}

#[test]
fn colon_space_title_stays_a_page() {
    let target = LinkTargetClassifier::default().classify("Ketosis: How to lose weight");
    assert!(
        matches!(&target, LinkTarget::CreationIntent { name, .. } if name == "Ketosis: How to lose weight"),
        "got {target:?}"
    );
}

#[test]
fn page_creation_rejects_a_scheme_shaped_name() {
    let err = PageId::for_path("cc-session:abc").expect_err("must be rejected");
    assert!(err.contains("scheme-shaped"), "{err}");
    assert!(err.contains("use '/' for hierarchy"), "{err}");

    let err = PageId::for_page_under("Areas", "cc-session:abc").expect_err("must be rejected");
    assert!(err.contains("scheme-shaped"), "{err}");

    // A nested segment is guarded too, and colon-space titles still pass.
    assert!(PageId::for_path("Areas/doc:x/Leaf").is_err());
    PageId::for_path("Areas/Ketosis: How to lose weight").expect("colon-space title is a page");
}

/// Over the SCHEME-SHAPE rule the classifier and the writer agree exactly:
/// nothing the classifier calls a creation intent is refused by
/// `PageId::for_path` for its shape, and everything the writer refuses for
/// its shape classifies as a disclosed unknown scheme.
///
/// Scoped deliberately — the empty-segment rule is NOT yet aligned; see
/// [`empty_segment_targets_still_diverge_from_the_writer`].
#[test]
fn creation_intents_match_the_writer_on_scheme_shape() {
    let classifier = LinkTargetClassifier::default();
    for target in [
        "cc-session:abc",
        "Areas/cc-session:abc",
        "Areas/doc:x/Leaf",
        "Projects/New thing",
        "Ketosis: How to lose weight",
        "Areas/Ketosis: How to lose weight",
    ] {
        match classifier.classify(target) {
            LinkTarget::CreationIntent { .. } => {
                PageId::for_path(target).unwrap_or_else(|e| {
                    panic!(
                        "{target:?} classified as a creation intent but the writer refuses \
                             it ({e}) — an unfulfillable intent"
                    )
                });
            }
            LinkTarget::UnknownScheme(_) => {
                assert!(
                    PageId::for_path(target).is_err(),
                    "{target:?} classified as unknown-scheme but the writer would accept it \
                         as a page — the two rules disagree"
                );
            }
            other => panic!("{target:?} unexpectedly classified as {other:?}"),
        }
    }
}

/// KNOWN DIVERGENCE, pinned so it cannot drift unnoticed.
///
/// `PageId::for_path` refuses an empty/whitespace segment as a malformed
/// path, but the classifier still calls these creation intents and mints an
/// optimistic id for them — an intent the writer can never fulfil, exactly
/// the class of bug the scheme-shape rule above was tightened to remove.
///
/// Not fixed here because `LinkTarget` has no honest bucket for it:
/// `UnknownScheme` means "scheme-shaped, scheme unclaimed" and these have
/// no scheme at all, so a faithful fix needs a new variant that ripples
/// through `EntityRef`, its serde tag, the Loro codec, the org renderer
/// and the GPUI link styling. TODO: add that variant and fold these
/// cases into the test above.
///
/// The failure is at least DISCLOSED rather than silent: clicking such a
/// link routes to `follow_dangling_link`, which calls the writer and fails
/// loudly.
#[test]
fn empty_segment_targets_still_diverge_from_the_writer() {
    let classifier = LinkTargetClassifier::default();
    for target in ["a//b", " /b", "Areas/"] {
        assert!(
            PageId::for_path(target).is_err(),
            "{target:?} must be refused by the writer as a malformed path"
        );
        assert!(
            matches!(
                classifier.classify(target),
                LinkTarget::CreationIntent { .. }
            ),
            "{target:?} is expected to STILL classify as a creation intent — if this now                  fails the divergence was fixed: fold these cases into \
                 `creation_intents_match_the_writer_on_scheme_shape` and delete this test"
        );
    }
}

#[test]
fn test_classify_resolved_block() {
    let target = LinkTargetClassifier::default().classify("block:some-id");
    assert!(matches!(target, LinkTarget::Resolved(uri) if uri.as_str() == "block:some-id"));
}

#[test]
fn test_classify_external_https() {
    let target = LinkTargetClassifier::default().classify("https://example.com");
    assert!(matches!(target, LinkTarget::External(url) if url == "https://example.com"));
}

#[test]
fn test_classify_external_mailto() {
    let target = LinkTargetClassifier::default().classify("mailto:test@example.com");
    assert!(matches!(target, LinkTarget::External(url) if url == "mailto:test@example.com"));
}

#[test]
fn test_classify_creation_intent_simple() {
    let target = LinkTargetClassifier::default().classify("ProjectNotes");
    match &target {
        LinkTarget::CreationIntent {
            scheme,
            path,
            name,
            parent_path,
            target_id,
        } => {
            assert_eq!(scheme, "block");
            assert_eq!(path, "ProjectNotes");
            assert_eq!(name, "ProjectNotes");
            assert!(parent_path.is_none());
            assert!(target_id.as_str().starts_with("block:"));
        }
        _ => panic!("Expected CreationIntent, got {:?}", target),
    }
}

#[test]
fn test_classify_creation_intent_with_path() {
    let target = LinkTargetClassifier::default().classify("Projects/New thing");
    match &target {
        LinkTarget::CreationIntent {
            scheme,
            path,
            name,
            parent_path,
            ..
        } => {
            assert_eq!(scheme, "block");
            assert_eq!(path, "Projects/New thing");
            assert_eq!(name, "New thing");
            assert_eq!(parent_path.as_deref(), Some("Projects"));
        }
        _ => panic!("Expected CreationIntent, got {:?}", target),
    }
}

#[test]
fn test_classify_person_scheme() {
    let target = LinkTargetClassifier::default().classify("Person/Alice");
    match &target {
        LinkTarget::CreationIntent { scheme, name, .. } => {
            assert_eq!(scheme, "person");
            assert_eq!(name, "Alice");
        }
        _ => panic!("Expected CreationIntent, got {:?}", target),
    }
}

#[test]
fn test_deterministic_id_stability() {
    let id1 = deterministic_entity_id("block", "projects/new thing");
    let id2 = deterministic_entity_id("block", "projects/new thing");
    assert_eq!(id1, id2);
}

#[test]
fn test_deterministic_id_uuid_format() {
    let id = deterministic_entity_id("block", "test");
    let path = id.id();
    // UUID format: 8-4-4-4-12
    assert_eq!(path.len(), 36);
    assert_eq!(path.chars().nth(8), Some('-'));
    assert_eq!(path.chars().nth(13), Some('-'));
    assert_eq!(path.chars().nth(18), Some('-'));
    assert_eq!(path.chars().nth(23), Some('-'));
}

#[test]
fn test_case_insensitive_convergence() {
    let target1 = LinkTargetClassifier::default().classify("Projects/Thing");
    let target2 = LinkTargetClassifier::default().classify("projects/thing");

    let id1 = target1.entity_id().unwrap();
    let id2 = target2.entity_id().unwrap();
    assert_eq!(id1, id2);
}

#[test]
fn test_normalize_for_hash_whitespace() {
    assert_eq!(normalize_for_hash("  Hello   World  "), "hello world");
    assert_eq!(normalize_for_hash("A\t\tB"), "a b");
}

#[test]
fn test_same_target_same_id_across_links() {
    let content = "See [[Projects/Test]] and also [[Projects/Test]].";
    let links = extract_links(content, &LinkTargetClassifier::default());
    assert_eq!(links.len(), 2);

    let id1 = links[0].classified.entity_id().unwrap();
    let id2 = links[1].classified.entity_id().unwrap();
    assert_eq!(id1, id2);
}

#[test]
fn spaced_separators_converge_with_writer() {
    // H2: a link typed with spaces around '/' must classify to the SAME id
    // as the tight form AND as the id the write paths mint via
    // `PageId::for_path` — the parser trims each segment.
    let spaced = LinkTargetClassifier::default()
        .classify("Areas / Sub")
        .entity_id()
        .unwrap()
        .clone();
    let tight = LinkTargetClassifier::default()
        .classify("Areas/Sub")
        .entity_id()
        .unwrap()
        .clone();
    assert_eq!(spaced, tight, "spaced vs tight parser id must agree");

    let minted = PageId::for_path("Areas/Sub").unwrap().into_entity_uri();
    assert_eq!(spaced, minted, "parser id must equal writer-minted id");
    assert_eq!(
        PageId::for_path("Areas / Sub").unwrap().into_entity_uri(),
        minted,
        "for_path must be insensitive to separator spacing"
    );
    // name/parent are trimmed too.
    match LinkTargetClassifier::default().classify("Areas / Sub") {
        LinkTarget::CreationIntent {
            name, parent_path, ..
        } => {
            assert_eq!(name, "Sub");
            assert_eq!(parent_path.as_deref(), Some("Areas"));
        }
        other => panic!("expected CreationIntent, got {other:?}"),
    }
}

#[test]
fn a_page_beside_a_held_id_gets_a_distinct_convergent_id() {
    let held = PageId::for_path("doc_904").unwrap().into_entity_uri();
    let beside = PageId::for_path_beside("doc_904", &held)
        .unwrap()
        .into_entity_uri();
    assert_ne!(beside, held);
    assert_eq!(
        PageId::for_path_beside(" Doc_904 ", &held)
            .unwrap()
            .into_entity_uri(),
        beside
    );
    let other_holder = PageId::for_path("doc_905").unwrap().into_entity_uri();
    assert_ne!(
        PageId::for_path_beside("doc_904", &other_holder)
            .unwrap()
            .into_entity_uri(),
        beside
    );
    assert!(PageId::for_path_beside("a//b", &held).is_err());
}

#[test]
fn for_path_rejects_empty_segments() {
    for bad in ["a//b", "/b", "a/", "  /  b", ""] {
        assert!(
            PageId::for_path(bad).is_err(),
            "malformed page path {bad:?} must be rejected (fail loud)"
        );
    }
    assert!(PageId::for_path("a/b").is_ok());
    assert!(PageId::for_path("a").is_ok());
}

#[test]
fn for_page_under_treats_leaf_as_a_single_segment() {
    // Bug (C): the "turn block into page" leaf comes from block CONTENT (a
    // title), so a `/` inside it is part of the title, never a separator —
    // it must NOT trip the empty-segment guard nor mint phantom hierarchy.
    assert!(
        PageId::for_page_under("", "Promote me to page/").is_ok(),
        "a trailing '/' in the title is one leaf segment, not a malformed path"
    );
    assert!(
        PageId::for_page_under("Home", "buy milk/eggs").is_ok(),
        "an embedded '/' in the title stays one leaf segment"
    );
    // Equivalence to for_path for slash-free content keeps ids born-equal
    // with the existing write paths.
    assert_eq!(
        PageId::for_page_under("Home", "Section").unwrap(),
        PageId::for_path("Home/Section").unwrap(),
    );
    assert_eq!(
        PageId::for_page_under("", "Loose note").unwrap(),
        PageId::for_path("Loose note").unwrap(),
    );
    // The DESTINATION path is still a real path — malformed destinations and
    // empty leaves fail loud.
    assert!(PageId::for_page_under("a//b", "leaf").is_err());
    assert!(PageId::for_page_under("Home", "   ").is_err());
}

#[test]
fn a_spelling_difference_names_what_differs() {
    let named = |a: &str, b: &str| {
        SpellingDifference::between(a, b)
            .unwrap_or_else(|| panic!("{a:?} / {b:?} share no key"))
            .to_string()
    };
    assert_eq!(named("My Notes", "My Notes"), "nothing");
    assert_eq!(named("My Notes", "MY NOTES"), "case");
    assert_eq!(named("Stra\u{df}e", "STRASSE"), "case");
    assert_eq!(named("caf\u{e9}", "cafe\u{301}"), "Unicode normalization");
    assert_eq!(named("My Notes", "My  Notes"), "spacing");
    assert_eq!(named("My Notes", "My Notes "), "spacing");
    assert_eq!(named("My Notes", "my\u{a0}notes"), "case and spacing");
    assert_eq!(
        named("Caf\u{e9} Notes", "cafe\u{301}  notes"),
        "case, Unicode normalization and spacing"
    );
    assert_eq!(SpellingDifference::between("My Notes", "Other"), None);
}
