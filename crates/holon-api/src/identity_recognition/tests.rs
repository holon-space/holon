use super::*;

fn root() -> EntityUri {
    EntityUri::no_parent()
}

fn held(title: &str) -> PageHolder {
    PageHolder {
        title: title.to_string(),
        parent: root(),
    }
}

fn slot(path: &str, title: &str, holders: &[(EntityUri, PageHolder)]) -> PageSlot {
    let holders: std::collections::HashMap<EntityUri, PageHolder> =
        holders.iter().cloned().collect();
    futures::executor::block_on(page_slot(path, &root(), title, |id| {
        std::future::ready(Ok(holders.get(&id).cloned()))
    }))
    .expect("page_slot")
}

fn path_id(path: &str) -> PageId {
    PageId::for_path(path).unwrap()
}

fn beside(path: &str, id: &PageId) -> PageId {
    PageId::for_path_beside(path, id.as_entity_uri()).unwrap()
}

#[test]
fn page_slot_on_a_free_path_is_the_path_id() {
    assert_eq!(slot("A/B", "B", &[]), PageSlot::Create(path_id("A/B")));
}

#[test]
fn page_slot_steps_past_each_renamed_holder() {
    let first = path_id("doc_904");
    let second = beside("doc_904", &first);
    let third = beside("doc_904", &second);
    assert_eq!(
        slot(
            "doc_904",
            "doc_904",
            &[(first.as_entity_uri().clone(), held("doc_905"))]
        ),
        PageSlot::Create(second.clone())
    );
    assert_eq!(
        slot(
            "doc_904",
            "doc_904",
            &[
                (first.as_entity_uri().clone(), held("doc_905")),
                (second.as_entity_uri().clone(), held("doc_906")),
            ]
        ),
        PageSlot::Create(third)
    );
}

#[test]
fn page_slot_finds_the_page_of_that_title_beside_a_renamed_holder() {
    let first = path_id("doc_904");
    let second = beside("doc_904", &first);
    assert_eq!(
        slot(
            "doc_904",
            "doc_904",
            &[
                (first.as_entity_uri().clone(), held("doc_905")),
                (second.as_entity_uri().clone(), held("doc_904")),
            ]
        ),
        PageSlot::Existing(second)
    );
}

#[test]
fn page_slot_steps_past_a_page_of_that_title_under_another_parent() {
    let first = path_id("doc_904");
    let moved = PageHolder {
        title: "doc_904".to_string(),
        parent: EntityUri::block("61133fe7"),
    };
    assert_eq!(
        slot(
            "doc_904",
            "doc_904",
            &[(first.as_entity_uri().clone(), moved)]
        ),
        PageSlot::Create(beside("doc_904", &first))
    );
}

#[test]
fn page_slot_adopts_a_title_that_differs_only_in_case_or_spacing() {
    let first = path_id("My Notes");
    for variant in ["My  Notes", "my notes", " My Notes"] {
        assert_eq!(
            slot(
                "My Notes",
                "My Notes",
                &[(first.as_entity_uri().clone(), held(variant))]
            ),
            PageSlot::Existing(first.clone()),
            "holder {variant:?}"
        );
    }
}

#[test]
fn page_slot_steps_past_a_placeholder_under_another_parent() {
    let first = path_id("Music");
    let elsewhere = PageHolder {
        title: String::new(),
        parent: EntityUri::block("61133fe7"),
    };
    assert_eq!(
        slot(
            "Music",
            "Music",
            &[(first.as_entity_uri().clone(), elsewhere)]
        ),
        PageSlot::Create(beside("Music", &first))
    );
}

#[test]
fn page_slot_completes_an_untitled_placeholder() {
    let first = path_id("Music");
    assert_eq!(
        slot(
            "Music",
            "Music",
            &[(first.as_entity_uri().clone(), held(""))]
        ),
        PageSlot::Create(first)
    );
}

fn id() -> EntityUri {
    EntityUri::block("61133fe7")
}

#[test]
fn unheld_id_is_free() {
    assert_eq!(
        recognize_derived_id(&id(), None, "2026-01-15"),
        Recognition::Free
    );
}

#[test]
fn same_normalized_title_is_already_satisfied() {
    // Case/space differences that `normalize_for_hash` folds must NOT read
    // as a collision — that is the convergent re-fire, not a clobber.
    assert_eq!(
        recognize_derived_id(&id(), Some("2026-01-15"), "2026-01-15"),
        Recognition::AlreadySatisfied
    );
}

#[test]
fn blank_holder_is_an_unnamed_placeholder_not_a_collision() {
    // The dogfood 2026-07-28 shape: a Loro placeholder root projected an
    // empty-content row over a page's id. No name is held, so the create
    // must ADOPT the id rather than be refused as a rival-entity clobber.
    for held in ["", "   ", "\n"] {
        assert_eq!(
            recognize_derived_id(&id(), Some(held), "Music"),
            Recognition::UnnamedPlaceholder,
            "held {held:?}"
        );
    }
}

#[test]
fn renamed_holder_is_a_collision_carrying_the_marker() {
    let r = recognize_derived_id(&id(), Some("Renamed"), "2026-01-15");
    match r {
        Recognition::Collision(c) => {
            assert_eq!(c.id, id());
            assert_eq!(c.held_title, "Renamed");
            assert_eq!(c.requested_title, "2026-01-15");
            assert!(
                c.to_string().contains(crate::IDENTITY_COLLISION_MARKER),
                "collision must carry the stable marker so the seam survives re-wrapping"
            );
        }
        other => panic!("expected Collision, got {other:?}"),
    }
}

#[test]
fn sanitize_then_recognize_is_stable_for_a_trailing_slash_title() {
    // A holder stored under the SANITIZED title, re-recognized from the same
    // sanitized title, reads AlreadySatisfied — the idempotent convergent
    // re-fire, not a false Collision.
    let raw = "My Page/";
    let sanitized = sanitize_page_title(raw).expect("non-empty after sanitize");
    assert_eq!(sanitized, "My Page");
    assert_eq!(
        recognize_derived_id(&id(), Some(&sanitized), &sanitized),
        Recognition::AlreadySatisfied
    );
    // Recognizing the SAME holder with the RAW trailing-slash title would
    // DIVERGE (normalize_for_hash keeps '/'): it reads Collision, not
    // AlreadySatisfied. That is exactly the SUT/oracle split the single-source
    // sanitize closes — both sides MUST sanitize before recognizing.
    assert!(
        matches!(
            recognize_derived_id(&id(), Some(&sanitized), raw),
            Recognition::Collision(_)
        ),
        "raw trailing-slash title must NOT match the sanitized holder — proving \
             why planner and reference must both sanitize before recognition"
    );
}
