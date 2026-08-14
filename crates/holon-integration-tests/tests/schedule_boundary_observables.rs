#![cfg(feature = "pbt")]
//! The completion boundaries a seeded schedule can wait on, against a booted
//! wide SUT: an intent settling and a CDC batch are observable, a boundary
//! nothing in flight can cross is refused before waiting, and work still in
//! flight past the deadline is reported as a wedge rather than degraded away.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::Value;
use holon_frontend::operations::OperationIntent;
use holon_integration_tests::pbt::composed::boundary::Boundary;
use holon_integration_tests::pbt::composed::boundary::BoundaryOutcome;
use holon_integration_tests::pbt::composed::boundary::BoundaryWindow;
use holon_integration_tests::pbt::composed::harness::ComposedSlice;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideHandle;
use holon_integration_tests::pbt::composed::wide_e2e::boot_and_seed_wide;
use holon_integration_tests::pbt::composed::wide_e2e::frontend_wired;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;

/// The seeded block every `WIDE_TREE_ORG` draw carries.
const TARGET: &str = "block:c1";

fn wedge_budget() -> Duration {
    Duration::from_secs(10)
}

/// Boot a wide SUT and run `body` against it, in the keystone harness's shape:
/// the ref state (which extracts the cap set on a throwaway runtime of its own)
/// is built first, and the SUT is dropped on this thread while its runtime is
/// still alive.
fn with_wide_sut<F>(body: F)
where
    F: AsyncFnOnce(&WideHandle),
{
    let ref_state = frontend_wired(wide_e2e_ref());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("multi-thread runtime");
    let resolver = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    let (caps, handle, _) = rt.block_on(boot_and_seed_wide(&resolver, &ref_state));
    rt.block_on(body(&handle));
    drop(caps);
    drop(handle);
}

/// One content write through the fire-and-forget door — the door an armed
/// transition dispatches through, so the intent is in flight on return.
async fn dispatch_detached(handle: &WideHandle, content: &str) {
    let engine = handle
        .reactive()
        .expect("a frontend draw has a reactive engine");
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(TARGET.to_string()));
    params.insert("field".to_string(), Value::String("content".to_string()));
    params.insert("value".to_string(), Value::String(content.to_string()));
    engine.ui_state().set_detached_dispatch(true);
    holon_frontend::reactive::dispatch_intent_through_armed_door(
        &engine,
        OperationIntent::new(EntityName::new("block"), "set_field".to_string(), params),
    )
    .await
    .expect("the detached door accepts the intent");
    engine.ui_state().set_detached_dispatch(false);
}

#[test]
fn an_in_flight_intent_settles_within_the_window() {
    with_wide_sut(async |handle| {
        let window = BoundaryWindow::open(WideE2E::dispatch_journal(handle).as_deref());
        dispatch_detached(handle, "typed").await;

        let outcome =
            WideE2E::await_boundary(handle, Boundary::AfterIntents(1), window, wedge_budget())
                .await;

        let evidence = outcome
            .evidence()
            .unwrap_or_else(|| panic!("AfterIntents(1) over one in-flight intent: {outcome:?}"));
        assert_eq!(evidence.boundary, Boundary::AfterIntents(1));
    });
}

#[test]
fn a_write_advances_the_cdc_watermark() {
    with_wide_sut(async |handle| {
        let window = BoundaryWindow::open(WideE2E::dispatch_journal(handle).as_deref());
        dispatch_detached(handle, "cdc").await;

        let outcome =
            WideE2E::await_boundary(handle, Boundary::AfterCdcBatch, window, wedge_budget()).await;

        assert!(
            outcome.evidence().is_some(),
            "a content write must emit CDC: {outcome:?}"
        );
    });
}

/// The degrade arm: refused before waiting, with its reason, because nothing
/// was dispatched in the window.
#[test]
fn a_boundary_nothing_can_cross_is_refused_before_waiting() {
    with_wide_sut(async |handle| {
        let window = BoundaryWindow::open(WideE2E::dispatch_journal(handle).as_deref());

        let started = std::time::Instant::now();
        let outcome =
            WideE2E::await_boundary(handle, Boundary::AfterIntents(1), window, wedge_budget())
                .await;

        match outcome {
            BoundaryOutcome::Unobservable(reason) => {
                assert!(
                    reason.contains("in flight"),
                    "the reason must name what was missing, got {reason:?}"
                );
                assert!(
                    started.elapsed() < wedge_budget(),
                    "an unobservable boundary must not consume its deadline"
                );
            }
            other => panic!("no dispatch in the window must be Unobservable, got {other:?}"),
        }
    });
}

/// The same refusal on the CDC arm: no write means no CDC row, so waiting out
/// the deadline would report a wedge over a system that is merely idle.
#[test]
fn a_cdc_batch_with_nothing_written_is_refused_before_waiting() {
    with_wide_sut(async |handle| {
        let window = BoundaryWindow::open(WideE2E::dispatch_journal(handle).as_deref());

        let started = std::time::Instant::now();
        let outcome =
            WideE2E::await_boundary(handle, Boundary::AfterCdcBatch, window, wedge_budget()).await;

        match outcome {
            BoundaryOutcome::Unobservable(reason) => {
                assert!(reason.contains("no intent in flight"), "got {reason:?}");
                assert!(started.elapsed() < wedge_budget());
            }
            other => panic!("an idle SUT must refuse the CDC boundary, got {other:?}"),
        }
    });
}

/// The wedge arm. A settle takes milliseconds, so a zero deadline over a live
/// intent has the shape of a system that stopped making progress — and must not
/// come back as a degrade.
#[test]
fn in_flight_work_past_the_deadline_is_a_wedge_not_a_degrade() {
    with_wide_sut(async |handle| {
        let window = BoundaryWindow::open(WideE2E::dispatch_journal(handle).as_deref());
        dispatch_detached(handle, "wedged").await;

        let outcome =
            WideE2E::await_boundary(handle, Boundary::AfterIntents(1), window, Duration::ZERO)
                .await;

        match outcome {
            BoundaryOutcome::TimedOutWithPendingWork { pending, .. } => {
                assert_eq!(pending, 1, "the wedge report must carry what was stuck");
            }
            other => panic!("pending work past the deadline must be a wedge, got {other:?}"),
        }
    });
}
