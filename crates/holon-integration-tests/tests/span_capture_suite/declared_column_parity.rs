//! Declared-column gaps at the enrich seat.
//!
//! `warn_missing_declared_column` (`holon-api/src/computed.rs`) is the LOUD
//! half of type-aware binding: it fires when a computed field's required
//! column belongs to the entity's declared schema but the row that reached the
//! enrich seat did not carry it. Every such line is a projection gap — a
//! subscription whose SELECT is narrower than the entity profile resolving its
//! rows — and the computed field becomes `Null` for the whole render.
//!
//! Two tests, and the split between them matters:
//!
//! - [`a_narrow_subscription_unbinds_the_declared_columns_it_omits`] is the
//!   MEASUREMENT. It subscribes a deliberately narrow query and asserts SET
//!   EQUALITY against [`EXPECTED_GAPS`], so both a lost pair and a new one
//!   fail. This is the durable evidence that the gaps come from the SELECT list
//!   and from nothing on the CDC path.
//! - [`default_boot_and_edit_path_carries_its_declared_columns`] covers only
//!   the boot-and-edit path, which does NOT reach the outline's narrow
//!   subscriptions. Read its own doc comment before trusting it for more.
//!
//! Neither is a parity guarantee over all subscriptions. That is
//! `inv-no-declared-column-absent`, a composed keystone invariant, whose
//! transitions drive the outline subscriptions no boot-only test reaches.
//!
//! Both need both halves of the capture contract: `SpanCollector::global()`
//! before the SUT boots (a subscriber installed later collects nothing), and
//! `reset_missing_declared_warnings()` before the boot (the dedup is
//! process-global and would otherwise suppress the signal under assertion).
//!
//! Run: `cargo nextest run -p holon-integration-tests --features pbt \
//!       --test span_capture_suite declared_column_parity`

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;

/// The LOUD projection-gap signal. Matched on the message text because the
/// capture layer flattens an event's fields into it.
const SIGNAL: &str = "DECLARED column absent from row";

/// Pick a seeded block that a `set_field` can edit: a `block:` content block
/// that is not a structural `::src::` / `::render::` node.
async fn pick_target(session: &holon_frontend::FrontendSession) -> String {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    snap.iter_blocks()
        .find(|b| {
            b.id.as_str().starts_with("block:")
                && !b.content.trim().is_empty()
                && !b.content.contains("::src::")
                && !b.content.contains("::render::")
        })
        .map(|b| b.id.as_str().to_string())
        .expect("seeded vault must contain an editable content block")
}

async fn set_field(
    reactive: &holon_frontend::reactive::ReactiveEngine,
    id: &str,
    field: &str,
    value: Value,
) {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(id.to_string()));
    params.insert("field".to_string(), Value::String(field.to_string()));
    params.insert("value".to_string(), value);
    params.insert(
        "write_seq".to_string(),
        Value::Integer(holon_api::write_seq::next().get()),
    );

    holon_frontend::reactive::BuilderServices::dispatch_intent_sync(
        reactive,
        holon_frontend::operations::OperationIntent::new(
            holon_api::EntityName::new("block"),
            "set_field".to_string(),
            params,
        ),
    )
    .await
    .unwrap_or_else(|e| panic!("set_field {field} dispatch: {e}"));
}

/// Every `(computed field, declared column)` pair a block-profile row loses
/// when its subscription projects only `id` and `content`.
///
/// Asserted as SET EQUALITY, not membership: the defect this whole change
/// documents is a count drifting from three to eight with nothing going red, so
/// a pin that tolerates drift would reproduce the original failure. A
/// legitimate change to the block profile's computed fields SHOULD fail here
/// and be updated deliberately.
const EXPECTED_GAPS: &[(&str, &str)] = &[
    ("bullet_shape", "collapsed"),
    ("is_holon_source", "source_language"),
    ("is_image", "content_type"),
    ("is_legacy_rule", "source_language"),
    ("is_program", "parent_id"),
    ("is_rule_head", "source_language"),
    ("is_source", "content_type"),
    ("is_widget_only", "widget_only"),
];

/// Pull `(context, column)` out of a captured warning. The capture layer
/// flattens an event's fields into the message as `context="x" column="y"`.
///
/// Panics rather than skipping when a SIGNAL-carrying warning does not parse:
/// silently dropping it would leave the set-equality assertion comparing an
/// empty set against an empty set and passing, so a capture-format change would
/// read as "no gaps" instead of "cannot read the gaps".
fn gap_pair(message: &str) -> (String, String) {
    let field = |key: &str| {
        let head = message.find(&format!("{key}=\""))? + key.len() + 2;
        let rest = &message[head..];
        Some(rest[..rest.find('"')?].to_string())
    };
    match (field("context"), field("column")) {
        (Some(context), Some(column)) => (context, column),
        _ => panic!(
            "a warning matched the declared-column signal but its context/column fields did \
             not parse — the capture format changed and this test can no longer read its own \
             input. Raw message:\n{message}"
        ),
    }
}

/// Characterises the mechanism the parity oracle guards against: a
/// subscription whose SELECT omits declared block columns leaves the block
/// profile's computed fields structurally unbound, and the enrich seat says so.
///
/// This is the causal proof that the warnings come from the SELECT list and not
/// from anything on the CDC path. Without it, a green parity oracle cannot be
/// told apart from an oracle that never exercised the mechanism at all.
#[test]
fn a_narrow_subscription_unbinds_the_declared_columns_it_omits() {
    let collector = SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = Arc::new(builder.build().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        holon_api::computed::reset_missing_declared_warnings();

        let env = TestEnvironment::new(runtime.clone()).expect("TestEnvironment");
        env.start_app(true).await.expect("start_app");

        collector.reset();
        holon_api::computed::reset_missing_declared_warnings();

        // `id` alone resolves the block profile; every other declared column the
        // profile's computed fields need is deliberately absent.
        let _stream = holon_api::QueryEngine::watch_query(
            env.engine().as_ref(),
            "SELECT id, content FROM block_raw",
            holon_api::QueryLanguage::HolonSql,
            HashMap::new(),
            None,
        )
        .await
        .expect("narrow watch_query");

        let expected: BTreeSet<(String, String)> = EXPECTED_GAPS
            .iter()
            .map(|(c, col)| (c.to_string(), col.to_string()))
            .collect();

        // Settle on the full set rather than the first arrival: the gaps land
        // as rows flow through the enrich seat, so stopping at the first one
        // would pin whichever raced ahead.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
            seen = collector
                .captured_warnings()
                .iter()
                .filter(|w| w.message.contains(SIGNAL))
                .map(|w| gap_pair(&w.message))
                .collect();
            if seen == expected {
                break;
            }
        }

        println!("NARROW-SUBSCRIPTION GAPS ({}):", seen.len());
        for (context, column) in &seen {
            println!("  - {context} / {column}");
        }

        let missing: Vec<_> = expected.difference(&seen).collect();
        let unexpected: Vec<_> = seen.difference(&expected).collect();
        assert!(
            missing.is_empty() && unexpected.is_empty(),
            "the set of declared-column gaps drifted from the pinned {} pair(s).\n\
             MISSING (pinned but not observed): {missing:?}\n\
             UNEXPECTED (observed but not pinned): {unexpected:?}\n\
             If the block profile's computed fields changed on purpose, update \
             EXPECTED_GAPS and the table in the bugfunnel entry together.",
            expected.len(),
        );
    });
}

/// The boot-and-edit path delivers rows carrying the declared columns the
/// block profile's computed fields need.
///
/// SCOPE, because a passing absence-assertion is easy to over-read: this drives
/// a boot plus `set_field` edits, and MEASURED GREEN on a tree where the
/// outline's narrow subscriptions are still unfixed. It does not reach them —
/// `focused_children` needs a navigation cursor and `descendants` needs an
/// embedded page to render, and nothing here produces either. So it proves the
/// boot-and-edit path specifically, and is a tripwire for a narrow subscription
/// being added to it; it is NOT the parity guarantee. That is
/// `inv-no-declared-column-absent`.
///
/// It still earns its place: it fails the moment this path starts carrying
/// short rows, and the vacuity guard below fails if it stops exercising the
/// enrich seat at all.
#[test]
fn default_boot_and_edit_path_carries_its_declared_columns() {
    // Must claim the global subscriber before the SUT boots, or the boot's
    // warnings go uncollected and the assertion passes vacuously.
    let collector = SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = Arc::new(builder.build().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        holon_api::computed::reset_missing_declared_warnings();

        let env = TestEnvironment::new(runtime.clone()).expect("TestEnvironment");
        env.start_app(true).await.expect("start_app");

        let session = env.session_arc();
        let reactive = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();

        let target_id = pick_target(&session).await;

        // A representative write set: an edit that re-emits the block through
        // every subscription carrying it, and a `collapsed` flip, whose value
        // IS one of the declared columns a narrow projection drops.
        set_field(
            reactive.as_ref(),
            &target_id,
            "content",
            Value::String("declared-column-parity-probe".to_string()),
        )
        .await;
        set_field(
            reactive.as_ref(),
            &target_id,
            "collapsed",
            Value::Integer(1),
        )
        .await;
        set_field(
            reactive.as_ref(),
            &target_id,
            "collapsed",
            Value::Integer(0),
        )
        .await;

        // The enrich seat runs on the CDC delivery task, which trails the op
        // return. Warnings that arrive after the read are missed, so settle on
        // a quiet window rather than a fixed sleep.
        let mut last = usize::MAX;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let now = collector.captured_warnings().len();
            if now == last {
                break;
            }
            last = now;
        }

        // The assertion below is an ABSENCE, so it passes for free if the live
        // path never ran. `live_data.apply_batch` sits downstream of
        // `enrich_stream`, so at least one proves rows actually reached the
        // enrich seat.
        let applied = collector
            .finished_spans()
            .iter()
            .filter(|s| s.name == "live_data.apply_batch")
            .count();
        assert!(
            applied > 0,
            "vacuity guard: no CDC batch reached the enrich seat, so zero \
             warnings proves nothing about declared-column parity"
        );

        let offenders: BTreeSet<String> = collector
            .captured_warnings()
            .iter()
            .filter(|w| w.message.contains(SIGNAL))
            .map(|w| w.message.clone())
            .collect();

        assert!(
            offenders.is_empty(),
            "{} projection gap(s) on the boot-and-edit path: a subscription \
             delivered rows short of the declared columns its entity profile's \
             computed fields require, so those fields rendered as Null.\n{}",
            offenders.len(),
            offenders
                .iter()
                .map(|m| format!("  - {m}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    });
}
