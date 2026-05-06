//! Integration test for the inbound runtime gate (Phase 3.3 step 2, item 3).
//!
//! Verifies that production wiring engages `disable_inbound_runtime()` at
//! startup, that whitelisted origins (`Org`) pass the gate as `Apply`, and
//! that non-whitelisted non-Loro origins (`Ui`) are dropped without
//! reflecting into Loro.
//!
//! Companion to the `inbound_gate_tests` pure-function unit tests in
//! `holon::sync::loro_sync_controller`. Those cover the decision matrix
//! against a synthetic `InboundEventDecision`; this exercises the wiring
//! end-to-end against a real EventBus + LoroSyncController.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon::sync::event_bus::{AggregateType, Event, EventBus, EventKind, EventOrigin};
use holon_integration_tests::TestEnvironmentBuilder;
use serde_json::Value;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

const ORG_CONTENT: &str = "\
* Gate test heading
:PROPERTIES:
:ID: gate-test-1
:END:
";

#[test]
fn gate_is_disabled_in_production_wiring() {
    init_tracing();
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            !env.loro_sync_inbound_runtime_enabled(),
            "production LoroModule must call disable_inbound_runtime() at \
             startup so SQL→Loro reflection of non-Loro-origin block events \
             is off — see crates/holon/src/sync/loro_module.rs"
        );
    });
}

#[test]
fn org_origin_events_pass_the_gate_as_apply() {
    init_tracing();
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("gate.org", ORG_CONTENT)
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("block:gate-test-1", SYNC_TIMEOUT).await,
            "block did not sync from org file within {SYNC_TIMEOUT:?}"
        );
        env.wait_for_consumers(&["loro"], SYNC_TIMEOUT).await;
        let applied_before = env.loro_sync_applied_count();
        let drop_before = env.loro_sync_drop_count();
        let error_before = env.loro_sync_error_count();

        // Tightened post-item-3: `LiveDocumentManager::create` now tags its
        // event with `EventOrigin::Org` instead of the legacy
        // `Other("sql")`, so the initial page-create flow on org file
        // discovery passes the gate as Apply. Combined with the heading
        // block.created (also Org via `OrgSyncController::execute_batch_with_origin`),
        // the startup + first-write phase should leave `drop_count` at 0.
        assert_eq!(
            drop_before, 0,
            "drop_count after fresh-boot Org-driven page create + heading \
             sync must be 0 — non-zero means a legitimate Org-flow event \
             still emits with origin=Other(\"sql\"). Migrate the offending \
             call site to `execute_operation_with_origin(.., EventOrigin::Org)`."
        );

        // Publish a synthetic Org-origin block.updated event for the block
        // that just synced. `inbound_event_decision(Org, _) == Apply`, so
        // the gate should pass it through regardless of gate state.
        let bus = env
            .event_bus
            .as_ref()
            .expect("event bus is wired when Loro is enabled");
        let mut payload: HashMap<String, Value> = HashMap::new();
        payload.insert("id".to_string(), Value::String("gate-test-1".into()));
        payload.insert(
            "content".to_string(),
            Value::String("synthetic Org-origin edit".into()),
        );
        let event = Event::new(
            EventKind::Updated,
            AggregateType::Block,
            "gate-test-1",
            EventOrigin::Org,
            payload,
        );
        bus.publish(event, None)
            .await
            .expect("publish synthetic Org event");

        env.wait_for_consumers(&["loro"], SYNC_TIMEOUT).await;

        assert_eq!(
            env.loro_sync_drop_count(),
            drop_before,
            "Org-origin block events must not be dropped — the gate \
             whitelists Org (decision: Apply)"
        );
        assert_eq!(
            env.loro_sync_applied_count(),
            applied_before + 1,
            "Org-origin block event should tick applied_count once"
        );
        // Downstream apply errors are out of scope for the gate-decision
        // test (this synthetic payload deliberately doesn't carry the full
        // block fields a real Org-origin update would). The decision side
        // is what the gate guarantees; downstream invariants live in
        // controller/backend tests.
        let _ = error_before;
    });
}

#[test]
fn ui_origin_events_are_dropped() {
    init_tracing();
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("gate.org", ORG_CONTENT)
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("block:gate-test-1", SYNC_TIMEOUT).await,
            "block did not sync from org file within {SYNC_TIMEOUT:?}"
        );
        env.wait_for_consumers(&["loro"], SYNC_TIMEOUT).await;
        let applied_before = env.loro_sync_applied_count();
        let drop_before = env.loro_sync_drop_count();
        let error_before = env.loro_sync_error_count();

        let bus = env
            .event_bus
            .as_ref()
            .expect("event bus is wired when Loro is enabled");
        let mut payload: HashMap<String, Value> = HashMap::new();
        payload.insert("id".to_string(), Value::String("gate-test-1".into()));
        payload.insert(
            "content".to_string(),
            Value::String("synthetic Ui edit that must not reach Loro".into()),
        );
        let event = Event::new(
            EventKind::Updated,
            AggregateType::Block,
            "gate-test-1",
            EventOrigin::Ui,
            payload,
        );
        bus.publish(event, None)
            .await
            .expect("publish synthetic Ui event");

        env.wait_for_consumers(&["loro"], SYNC_TIMEOUT).await;

        assert_eq!(
            env.loro_sync_drop_count(),
            drop_before + 1,
            "Ui-origin event must be dropped while the gate is disabled \
             (decision: Drop)"
        );
        assert_eq!(
            env.loro_sync_applied_count(),
            applied_before,
            "Ui-origin event must not reach the apply path"
        );
        assert_eq!(
            env.loro_sync_error_count(),
            error_before,
            "dropping a Ui-origin event is not an error"
        );
    });
}
