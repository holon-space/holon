//! **Step-6 GREEN proof (ADR 0021/0022)** — the deterministic composed sequence
//! that was the step-4 RED window, flipped now that synthesis (engine rule
//! reconciler → `advice_rule_{slug}` matview) and the weave (frontend
//! `AdviceWeaver`) are live. (File keeps its historical `advice_step4_red`
//! name; the doc trail is ADR 0022 step 4 → step 6.)
//!
//! What it proves, through the SAME transition-apply path and the SAME catalog
//! check (`ComposedSut::<WideE2E>::apply` + `check_invariants`) the keystone
//! `general_e2e_composed_pbt` uses:
//!
//! 1. With an ACTIVE rule, a rendered tagged anchor and ≥3 scored lessons at
//!    k=2 (truncation live), `check_invariants` is GREEN — and both advice
//!    invariants (`inv-advice-rows-woven`,
//!    `inv-advice-matview-matches-ref/matview`) actually RAN (asserted against
//!    the run report's `ran` set; a silent deselect fails here, it cannot pass
//!    vacuously).
//! 2. Dispatching the production `dismiss_advice` operation (the same
//!    `BackendEngine::execute_operation` dispatch the frontend op_button
//!    reaches) on the TOP woven lesson removes it and BACKFILLS the 3rd
//!    candidate (top-K read-time semantics), with the reference mutated in
//!    lockstep (the `advice_suppressed` edge on the anchor — the same field
//!    `SetEdgeField`'s AdviceSuppressed arm writes).
//!
//! Settle: the weave arrives via two async legs (reconciler DDL + weaver
//! recompute) on top of the CDC/Loro/org projections. The dismiss dispatch is
//! outside the transition alphabet, so the test runs the slice's own
//! post-apply settle explicitly (`ComposedSut::settle_projections` — the
//! combined fixed-point converge, fail-loud on non-convergence) before
//! re-checking. No invariant is softened.

use std::collections::HashMap;

use holon_api::EdgeFieldUpdate;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Region;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::NavigateFocus;
use holon_integration_tests::pbt::transitions::SetEdgeField;
use holon_integration_tests::pbt::transitions::WriteOrgFile;
use holon_orgmode::models::OrgBlockExt;
use holon_pbt_core::capabilities::RefAdvice;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

/// The generator's placeholder document uri (`WriteOrgFile::GEN_PLACEHOLDER`):
/// top-level headings are parented to it and `apply_to_ref` remaps them to the
/// resolved per-document uri.
fn gen_placeholder() -> EntityUri {
    EntityUri::block("gen-placeholder")
}

fn heading(id: &str, headline: &str) -> Block {
    let uri = EntityUri::block(id);
    let mut b = Block::new_text(uri, gen_placeholder(), headline);
    b.set_property("ID", Value::String(id.to_string()));
    b
}

/// The exact YAML template shape `file_with_advice_rule` in
/// `pbt/generators.rs` mints (anchor `has_tag: task`, source `has_tag:
/// lesson`, `k: 2`, ACTIVE).
fn advice_rule_file() -> WriteOrgFile {
    let rule_heading = heading("advice-rule-h", "Advice rules");
    let yaml = "name: pbt_lessons\nactive: true\nanchor:\n  has_tag: task\ncandidates:\n  \
                tag_overlap_recency:\n    source:\n      has_tag: lesson\nk: 2\n";
    // Same fail-loud round-trip guard as the generator arm.
    let parsed = holon_advice::parse_advice_rule(yaml).expect("hand-built advice rule must parse");
    assert!(parsed.active);
    assert_eq!(parsed.k.get(), 2);

    let mut rule_block = Block::new_source(
        EntityUri::block("advice-rule-h::src::0"),
        EntityUri::block("advice-rule-h"),
        "holon_advice_rule_yaml",
        yaml,
    );
    rule_block.set_sequence(1);
    WriteOrgFile {
        filename: "advice_1.org".to_string(),
        blocks: vec![rule_heading, rule_block],
        keyword_set: None,
    }
}

/// One anchor + three lessons, so k=2 truncation is live (the 3rd candidate is
/// the read-time backfill after a dismissal).
fn notes_file() -> WriteOrgFile {
    WriteOrgFile {
        filename: "notes_1.org".to_string(),
        blocks: vec![
            heading("anchor-a", "Anchor block"),
            heading("lesson-b", "Lesson block B"),
            heading("lesson-c", "Lesson block C"),
            heading("lesson-d", "Lesson block D"),
        ],
        keyword_set: None,
    }
}

fn set_tags(id: &str, csv: &str) -> SetEdgeField {
    SetEdgeField {
        block_id: EntityUri::block(id),
        update: EdgeFieldUpdate::Tags(Tags::from_csv(csv)),
    }
}

type Sut = ComposedSut<WideE2E>;

/// Precondition-checked apply through the keystone's exact path.
fn step(mut ref_state: ReferenceState, sut: Sut, t: E2ETransition) -> (ReferenceState, Sut) {
    assert!(
        WideE2EMachine::preconditions(&ref_state, &t),
        "preconditions failed for {t:?}"
    );
    ref_state = WideE2EMachine::apply(ref_state, &t);
    let sut = <Sut as StateMachineTest>::apply(sut, &ref_state, t);
    (ref_state, sut)
}

/// GREEN check with vacuity teeth: the full catalog check must not fail, AND
/// both advice invariants must be in the report's RAN set — a deselected
/// invariant fails here instead of passing silently.
fn check_green_with_advice_ran(sut: &Sut, ref_state: &ReferenceState, phase: &str) {
    let report = sut.run_report_now(ref_state);
    let failures = report.failures();
    assert!(
        failures.is_empty(),
        "[{phase}] composed catalog must be GREEN post step-6; failures: {failures:?}"
    );
    for id in [
        "inv-advice-rows-woven",
        "inv-advice-matview-matches-ref/matview",
    ] {
        assert!(
            report.ran_ids().contains(&id),
            "[{phase}] {id} must have RUN (not vacuously deselected); ran: {:?}, deselected: {:?}",
            report.ran_ids(),
            report.deselected.iter().map(|d| d.0).collect::<Vec<_>>(),
        );
    }
    // The harness path the keystone uses — full catalog + per-draw
    // non-vacuity floor. Must not panic.
    <Sut as StateMachineTest>::check_invariants(sut, ref_state);
}

#[test]
fn advice_step6_synthesis_weave_and_dismiss_green() {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let ref_state = wide_e2e_ref();
    // The composed keystone oracle boots PRE-STARTED (`build_started_ref`), so
    // there is no StartApp step: `WriteOrgFile` lands post-boot through the
    // production `FileSyncController` watcher ingest — the only form reachable
    // in the keystone.
    assert!(
        ref_state.action.app_started,
        "wide_e2e_ref boots pre-started"
    );
    let sut = Sut::init_test(&ref_state);

    // 1–2: org files — one ACTIVE advice rule, one file with the anchor +
    //      three lesson carrier blocks.
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::WriteOrgFile(advice_rule_file()),
    );
    let (ref_state, sut) = step(ref_state, sut, E2ETransition::WriteOrgFile(notes_file()));

    // 3: render the anchor — navigate the main panel to the notes page so the
    //    weave invariant sees the anchor node (not just the journals page).
    let notes_doc = ref_state
        .doc_uri_by_name("notes_1")
        .expect("notes_1 page block must exist after WriteOrgFile");
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::NavigateFocus(NavigateFocus {
            region: Region::Main,
            block_id: notes_doc,
        }),
    );

    // 4–7: pool-tag the carriers with distinct overlap scores so top-K order
    //      is deterministic (no equal-score recency ambiguity):
    //      anchor {task,p1,p2,p3}; b∩a=3, c∩a=2, d∩a=1; k=2 → woven {b,c},
    //      d is the live truncation (the backfill candidate).
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("anchor-a", "task,p1,p2,p3")),
    );
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("lesson-b", "lesson,p1,p2,p3")),
    );
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("lesson-c", "lesson,p1,p2")),
    );
    let (mut ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("lesson-d", "lesson,p1")),
    );

    // Sanity: the reference expects the matview and the truncated weave — if
    // these fail the SEQUENCE is broken, not the invariants.
    let anchor = EntityUri::block("anchor-a");
    let (lesson_b, lesson_c, lesson_d) = (
        EntityUri::block("lesson-b"),
        EntityUri::block("lesson-c"),
        EntityUri::block("lesson-d"),
    );
    assert_eq!(
        ref_state.advice_matview_name().as_deref(),
        Some("advice_rule_pbt_lessons"),
        "reference must project the active rule's matview name"
    );
    let exp = ref_state.advice_expectation(anchor.as_str());
    assert_eq!(
        exp.scored,
        vec![
            (lesson_b.as_str().to_string(), 3),
            (lesson_c.as_str().to_string(), 2),
            (lesson_d.as_str().to_string(), 1),
        ],
        "anchor-a must score b=3, c=2, d=1 via the shared pool tags"
    );
    assert_eq!(
        exp.k, 2,
        "k=2 keeps truncation live (d is dropped pre-dismiss)"
    );

    // Step-6 flip: synthesis + weave are live, so the keystone check is GREEN
    // and both advice invariants demonstrably RAN.
    //
    // KNOWN RED (2026-07-07, final integration wave): the SQL leg is fully
    // green — the reconciler creates `advice_rule_pbt_lessons`, its rows match
    // the reference, and the canonical read returns (lesson-b, lesson-c) —
    // but `inv-advice-rows-woven` reds with "wove 0": the `AdviceWeaver` is
    // wired ONLY inside `ReactiveView::create_driver`, which runs solely on
    // the `watch_live` + `start_reactive_views` (windowed/GPUI) path. The
    // composed headless render surface snapshots via
    // `ReactiveEngine::ensure_watching` + pure interpret, where collection
    // drivers never start — so the weave is structurally invisible to this
    // test AND to the keystone AND to MCP `describe_ui`. Not a settle gap
    // (verified: matview + canonical read correct at check time; a 2 s grace
    // changes nothing; zero `[ReactiveView::start]` at debug). Fix belongs in
    // holon-frontend: surface the woven suffix on the snapshot path (e.g. a
    // session-level weave sidecar the pure interpreter appends, mirroring how
    // the creation slot materializes purely), not in this harness.
    check_green_with_advice_ran(&sut, &ref_state, "post-weave");

    // Dismiss the TOP woven lesson (b) through the production dispatch the
    // frontend op_button reaches: `execute_operation("block", "dismiss_advice",
    // {anchor_id, lesson_id})` — headless, RMW-append on the anchor's
    // `advice_suppressed` set.
    {
        let engine = sut
            .handle()
            .engine()
            .expect("full_headless wiring boots a Turso engine")
            .clone();
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert(
            "anchor_id".into(),
            Value::String(anchor.as_str().to_string()),
        );
        params.insert(
            "lesson_id".into(),
            Value::String(lesson_b.as_str().to_string()),
        );
        sut.runtime()
            .block_on(engine.execute_operation(&EntityName::new("block"), "dismiss_advice", params))
            .expect("dismiss_advice dispatch must succeed");
    }
    // The dispatch sits outside the transition alphabet, so run the slice's
    // own post-apply settle (combined fixed-point converge over CDC/Loro/org,
    // fail-loud) before reading invariants.
    sut.settle_projections();

    // Reference lockstep: the dismissal lands as the `advice_suppressed` edge
    // on the ANCHOR — the same field `SetEdgeField`'s AdviceSuppressed arm
    // assigns in `apply_to_ref` (dismiss is the RMW-append form of it).
    ref_state
        .domain
        .block_state
        .blocks
        .get_mut(&anchor)
        .expect("anchor block exists in the reference")
        .advice_suppressed
        .push(lesson_b.clone());

    // Top-K read-time semantics: b is gone, the 3rd candidate (d) BACKFILLS.
    let exp = ref_state.advice_expectation(anchor.as_str());
    assert_eq!(
        exp.scored,
        vec![
            (lesson_c.as_str().to_string(), 2),
            (lesson_d.as_str().to_string(), 1),
        ],
        "post-dismiss expectation must drop b and keep c=2, d=1 (d backfills at k=2)"
    );

    check_green_with_advice_ran(&sut, &ref_state, "post-dismiss");
}
