//! The headless driver's post-gesture barrier (`settle_after`) returns only
//! when the engine owes no CDC delivery. A subscriber that applies each batch
//! far later than any quiet window holds it, so a barrier that returns on
//! quiet alone is red. While the subscriber lags, the engine names what it
//! owes: stage, target and last value.
//!
//! @pbt kind harness
//! @pbt covers router-settle-waits-for-delivery — the headless driver waits
//! for delivery targets, never for quiet

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use holon::api::BackendEngine;
use holon_api::Value;
use holon_api::live_data::DeliveryCursor;
use holon_api::live_data::reporting_to;
use holon_frontend::user_driver::ReactiveEngineDriver;
use holon_frontend::user_driver::UserDriver;
use holon_integration_tests::TestEnvironment;
use proptest::prelude::*;
use tokio_stream::StreamExt;

const SLOW_APPLY: Duration = Duration::from_millis(300);

const VAULT_ORG: &str = "\
* Alpha
:PROPERTIES:
:ID: alpha
:END:
* Beta
:PROPERTIES:
:ID: beta
:END:
";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// A watch whose consumer applies each batch only after [`SLOW_APPLY`].
/// Returns the count of applied rows.
async fn slow_subscriber(engine: &Arc<BackendEngine>) -> Arc<AtomicU64> {
    engine
        .db_handle()
        .execute_ddl("CREATE TABLE router_probe (id TEXT PRIMARY KEY, owner TEXT NOT NULL)")
        .await
        .expect("create the probe table");
    let cursor = Arc::new(DeliveryCursor::default());
    let (_view, mut stream) = reporting_to(
        cursor.clone(),
        engine.matview_manager().ensure_and_subscribe(
            "SELECT id, owner AS watch_key FROM router_probe",
            Some("mine"),
        ),
    )
    .await
    .expect("watch the probe view");
    let applied = Arc::new(AtomicU64::new(0));
    let consumer_applied = applied.clone();
    tokio::spawn(async move {
        while let Some(batch) = stream.next().await {
            tokio::time::sleep(SLOW_APPLY).await;
            consumer_applied.fetch_add(batch.inner.items.len() as u64, Ordering::SeqCst);
            cursor.applied_through(batch.metadata.seq);
        }
    });
    applied
}

async fn gestures_settle_on_delivery(rt: Arc<tokio::runtime::Runtime>, rows_per_gesture: &[u8]) {
    let env = TestEnvironment::new(rt).expect("TestEnvironment::new");
    env.write_org_file("vault.org", VAULT_ORG)
        .await
        .expect("write vault.org");
    env.start_app(true).await.expect("start_app");
    env.settle().await;
    let applied = slow_subscriber(env.engine()).await;
    env.settle().await;

    let reactive = env
        .reactive_engine
        .get()
        .expect("start_app wires the reactive engine")
        .clone();
    let query_engine = reactive
        .session()
        .query_engine()
        .expect("a Turso session has a query engine");
    let driver = ReactiveEngineDriver::new(reactive);
    driver
        .warm_for_block(&holon_api::root_layout_block_uri())
        .await
        .expect("warm the router on the root layout");

    let mut inserted = 0u64;
    for (gesture, &rows) in rows_per_gesture.iter().enumerate() {
        // Alternating focus targets: each gesture re-renders the main panel.
        let target = if gesture % 2 == 0 { "alpha" } else { "beta" };
        let mut params = HashMap::new();
        params.insert("region".into(), Value::from(holon_api::Region::Main));
        params.insert("block_id".into(), Value::String(format!("block:{target}")));
        let tick = driver.snapshot_emission_tick();
        driver
            .synthetic_dispatch("navigation", "focus", params)
            .await
            .expect("focus the main region");
        for _ in 0..rows {
            env.engine()
                .db_handle()
                .execute(
                    &format!("INSERT INTO router_probe (id, owner) VALUES ('r{inserted}', 'mine')"),
                    vec![],
                )
                .await
                .expect("insert a probe row");
            inserted += 1;
        }
        if rows > 0 {
            let owed = query_engine
                .delivery_owed()
                .await
                .expect("the engine reports what it owes");
            assert!(
                owed.iter().any(|line| {
                    (line.starts_with("cdc-applied: target sent seq")
                        && line.contains(", at applied seq "))
                        || line.starts_with("cdc-routed: target emitted seq")
                }),
                "a lagging subscriber is owed a delivery, named by stage, target and value: \
                 {owed:?}"
            );
        }
        driver
            .settle_after(tick)
            .await
            .expect("the gesture settles");
        let applied_rows = applied.load(Ordering::SeqCst);
        assert_eq!(
            applied_rows, inserted,
            "settle_after returned before the subscriber applied its batch \
             (gestures {rows_per_gesture:?}, after gesture {gesture})"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_gesture_settles_only_when_every_delivery_is_applied(
        rows_per_gesture in proptest::collection::vec(0u8..3, 1..4)
    ) {
        let rt = runtime();
        rt.block_on(gestures_settle_on_delivery(rt.clone(), &rows_per_gesture));
    }
}
