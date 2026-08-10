//! The editable surface is a SOURCE PROJECTION of the task-keyword facet
//! (F2 arm d, ruled 2026-08-11): on focus the editor seeds vault syntax
//! (`TODO milk`), and on commit the buffer is parsed back by the store's
//! convergence rule. That only works if the two are exact inverses, so this
//! asserts the fixed point directly:
//!
//! ```text
//! converge_keyword_headed(source_projection(state)) == state
//! ```
//!
//! It is the pair that matters, not either half — a projection nobody can parse
//! back would silently rewrite the block on the first commit, which is exactly
//! the F2 divergence class this arm exists to close.
//!
//! The two REFUSAL arms are the interesting part. A refusal is not a failure of
//! this test; it is the design saying "these bytes have no representable
//! projection", and each one is a real state reachable from imported or legacy
//! rows. What the test forbids is a projection that is accepted and then parses
//! back to something else.
//!
//! @pbt kind property
//! @pbt covers source-projection-fixed-point — seeding the editor and parsing
//!   the commit are inverses for every representable block state

use holon_api::TaskState;
use holon_org_format::ProjectionRefusal;
use holon_org_format::SourceProjection;
use holon_org_format::TaskKeywordVocabulary;
use holon_org_format::converge_keyword_headed;
use holon_org_format::source_projection;
use proptest::prelude::*;

/// Content a block can legitimately hold: no leading whitespace (the parser
/// eats it — that is the `ContentStartsWithWhitespace` refusal, exercised
/// separately), and no newlines (the first line is the headline).
fn content() -> impl Strategy<Value = String> {
    prop::collection::vec("[a-zA-Z0-9\\[\\]#*_ -]{0,12}", 0..4)
        .prop_map(|parts| parts.join(" ").trim_start().trim_end().to_string())
}

fn declared_vocabulary() -> impl Strategy<Value = TaskKeywordVocabulary> {
    prop_oneof![
        Just(TaskKeywordVocabulary::default()),
        Just(TaskKeywordVocabulary::for_document(
            &["NEXT".to_string(), "WAITING".to_string()],
            &["SHIPPED".to_string()],
        )),
        Just(TaskKeywordVocabulary::for_document(
            &["LATER".to_string()],
            &["CANCELLED".to_string()],
        )),
    ]
}

proptest! {
    /// THE FIXED POINT. For every block state whose projection is accepted,
    /// parsing that projection reproduces the state exactly — same keyword,
    /// same content, byte for byte.
    #[test]
    fn an_accepted_projection_parses_back_to_the_same_state(
        vocabulary in declared_vocabulary(),
        keyword_index in 0usize..8,
        content in content(),
        tasked in any::<bool>(),
    ) {
        let all = vocabulary.all_keywords();
        let task_state = tasked.then(|| {
            let kw = &all[keyword_index % all.len()];
            TaskState::from_keyword_with_done_list(kw, vocabulary.done_keywords())
        });

        let projected = source_projection(task_state.as_ref(), &content, &vocabulary);
        let SourceProjection::Text(text) = projected else {
            // A refusal is a legitimate verdict; the generator only produces
            // representable states, so reaching one is a bug in the strategy,
            // not in the rule — say which.
            prop_assert!(false, "unexpected refusal for {task_state:?} + {content:?}");
            unreachable!()
        };

        match (converge_keyword_headed(&text, &vocabulary), &task_state) {
            (Some(parsed), Some(expected)) => {
                prop_assert_eq!(&parsed.keyword, expected,
                    "projection {:?} parsed back to a different keyword", text);
                prop_assert_eq!(&parsed.stripped, &content,
                    "projection {:?} parsed back to different content", text);
            }
            (None, None) => {
                prop_assert_eq!(&text, &content,
                    "an untasked block projects to its content unchanged");
            }
            (parsed, expected) => prop_assert!(
                false,
                "projection {:?} disagreed about task-ness: parsed {:?}, expected {:?}",
                text, parsed, expected
            ),
        }
    }

    /// The projection never grows or loses the user's text: whatever the
    /// keyword does, the CONTENT survives the round trip verbatim.
    #[test]
    fn the_content_survives_the_round_trip_verbatim(
        vocabulary in declared_vocabulary(),
        content in content(),
    ) {
        let all = vocabulary.all_keywords();
        let task_state =
            TaskState::from_keyword_with_done_list(&all[0], vocabulary.done_keywords());
        let SourceProjection::Text(text) =
            source_projection(Some(&task_state), &content, &vocabulary)
        else {
            prop_assert!(false, "the first declared keyword must always project");
            unreachable!()
        };
        let parsed = converge_keyword_headed(&text, &vocabulary)
            .expect("an accepted projection is always keyword-headed");
        prop_assert_eq!(parsed.stripped, content);
    }
}

/// EMPTY REMAINDER, pinned by example because it is the case the whole F5
/// argument turns on: an empty-titled task projects to the BARE keyword with no
/// trailing space, because the store's content canonicalizer trims one and the
/// projection has to survive that trim unchanged.
#[test]
fn an_empty_titled_task_projects_to_the_bare_keyword() {
    let vocabulary = TaskKeywordVocabulary::default();
    let todo = TaskState::active("TODO");
    assert_eq!(
        source_projection(Some(&todo), "", &vocabulary),
        SourceProjection::Text("TODO".to_string()),
        "a trailing space here would be trimmed on commit and the projection would drift"
    );
    let parsed = converge_keyword_headed("TODO", &vocabulary).expect("the bare keyword parses");
    assert_eq!(parsed.keyword, todo);
    assert_eq!(parsed.stripped, "");
}

/// REFUSAL 1 — the silent-demotion guard. A block carrying a keyword its own
/// document does not declare has NO representable projection: `TODO x` in a
/// `#+TODO: NEXT | DONE` document parses back as ordinary prose, so seeding it
/// and committing would delete the task. Reachable from imported vaults and
/// from rows written before the cycle fix.
#[test]
fn a_keyword_the_document_does_not_declare_is_refused() {
    let vocabulary =
        TaskKeywordVocabulary::for_document(&["NEXT".to_string()], &["DONE".to_string()]);
    let stray = TaskState::active("TODO");

    let refusal = match source_projection(Some(&stray), "x", &vocabulary) {
        SourceProjection::Refused(r) => r,
        other => panic!("projecting an undeclared keyword must refuse, got {other:?}"),
    };
    assert_eq!(refusal.as_str(), "keyword_not_declared");
    assert!(
        refusal.to_string().contains("demote"),
        "the refusal must name the CONSEQUENCE, not just the fact: {refusal}"
    );

    // The proof the refusal is right: the projection it declined to make does
    // not round-trip.
    assert_eq!(
        converge_keyword_headed("TODO x", &vocabulary),
        None,
        "this document reads `TODO x` as prose — that is why projecting it is unsafe"
    );
}

/// REFUSAL 2 — leading whitespace. `TODO  milk` parses back to `milk`, so the
/// space would vanish on the first commit of an untouched buffer.
#[test]
fn content_that_starts_with_whitespace_is_refused() {
    let vocabulary = TaskKeywordVocabulary::default();
    let todo = TaskState::active("TODO");

    let refusal = match source_projection(Some(&todo), " milk", &vocabulary) {
        SourceProjection::Refused(r) => r,
        other => panic!("leading whitespace must refuse, got {other:?}"),
    };
    assert!(matches!(
        refusal,
        ProjectionRefusal::ContentStartsWithWhitespace { .. }
    ));

    // Again, the refusal is right because the projection would not round-trip.
    let parsed = converge_keyword_headed("TODO  milk", &vocabulary).expect("still keyword-headed");
    assert_eq!(
        parsed.stripped, "milk",
        "the parser eats the leading space — projecting would lose it"
    );
}

/// An untasked block projects to its content unchanged, whatever the content
/// looks like. Under eager convergence a keyword-headed untasked block is not
/// representable, so there is nothing to special-case here.
#[test]
fn an_untasked_block_projects_to_its_content() {
    let vocabulary = TaskKeywordVocabulary::default();
    assert_eq!(
        source_projection(None, "buy milk", &vocabulary),
        SourceProjection::Text("buy milk".to_string())
    );
    assert_eq!(
        source_projection(None, "", &vocabulary),
        SourceProjection::Text(String::new())
    );
}
