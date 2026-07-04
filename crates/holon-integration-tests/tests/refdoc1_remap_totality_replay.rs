#![cfg(feature = "pbt")]
//! Deterministic replay of the armed-keystone shrunk counterexample that dies
//! on the `SutBlockCreate::apply_create_under_focus` remap-totality guard
//! (`frontend_slice/components.rs`, `block:ref-doc-1` unmapped).
//!
//! Shrunk sequence (armed sweep, `PROPTEST_MAX_SHRINK_ITERS` capped at 200):
//!   [CreateDocument(doc_0.org), WriteOrgFile(a_0.org, 4 blocks),
//!    NavigateFocus(main, block:ref-doc-1), CreateBlockUnderFocus(id=Some)]
//!
//! Replayed through the EXACT keystone harness the hand-authored JSONL cases
//! use, so the SAME case can be run against two trees (with / without the
//! `file_sync_controller` sibling-order fix) to decide whether the red is
//! CAUSED by that fix or merely UNMASKED by it.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

const CASE: &str = r#"{"name": "ref-doc-1-remap-totality-born-equal-create-under-focus", "description": "armed-keystone shrunk counterexample: CreateDocument mints oracle block:ref-doc-0, WriteOrgFile mints oracle block:ref-doc-1; NavigateFocus focuses the ORACLE synthetic ref-doc-1 and a born-equal (id:Some) CreateBlockUnderFocus dispatches block.create under it. The per-tick reconcile never mapped ref-doc-1 to its real SUT id, so the remap-totality guard in SutBlockCreate::apply_create_under_focus fires.", "transitions": [{"CreateDocument": {"file_name": "doc_0.org"}}, {"WriteOrgFile": {"filename": "a_0.org", "blocks": [{"id": "block:a", "parent_id": "block:gen-placeholder", "content": "A", "content_type": "text", "source_language": null, "source_name": null, "properties": {"ID": "a"}, "created_at": 0, "updated_at": 0}, {"id": "block:a::src::0", "parent_id": "block:a", "content": "jML_r9A GHQ3iytH1gi", "content_type": "source", "source_language": "python", "source_name": null, "properties": {}, "created_at": 0, "updated_at": 0}, {"id": "block:a::img::0", "parent_id": "block:a", "content": "attachments/o_e28___7_7z.png", "content_type": "image", "source_language": null, "source_name": null, "properties": {}, "created_at": 0, "updated_at": 0}, {"id": "block:6v9k-c---z6y-d56-----zm4-", "parent_id": "block:a", "content": "A6 iwyK04", "content_type": "text", "source_language": null, "source_name": null, "properties": {"ID": "6v9k-c---z6y-d56-----zm4-"}, "created_at": 0, "updated_at": 0}]}}, {"NavigateFocus": {"region": "main", "block_id": "block:ref-doc-1"}}, {"CreateBlockUnderFocus": {"content": "xz", "id": "block:gen-31"}}]}"#;

#[test]
fn refdoc1_remap_totality_replay() {
    // The originating sweep was armed; the hand-authored replay bypasses the
    // generator gate, but any env-guarded code inside a transition must still
    // see the same arming.
    unsafe { std::env::set_var("HOLON_PBT_EXTERNAL_RACES", "1") };
    let case: Fixture<E2ETransition> = serde_json::from_str(CASE).expect("replay case must parse");
    eprintln!(
        "[refdoc1-replay] running case {:?} ({} transitions)",
        case.name,
        case.transitions.len()
    );
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    ComposedSut::<WideE2E>::test_sequential(config, wide_e2e_ref(), case.transitions, None);
    eprintln!("[refdoc1-replay] PASSED case {:?}", case.name);
}
