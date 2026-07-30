//! Contract: a live block whose watch target does not exist YET discloses the
//! failure AND recovers on its own once the table appears.
//!
//! The real-world sequence: an MCP integration is still creating its `cc_*`
//! tables and ingesting (minutes) while the user navigates to a page whose
//! sections watch those tables. The watches are installed against tables that
//! do not exist. Disclosure alone is not enough — the block must deliver its
//! rows once the ingest lands, with no re-navigation.
//!
//! @pbt kind harness
//! @pbt covers watch-missing-table-recovery — a failed query watcher discloses
//! and then retries until its table exists

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::di::DbHandleProvider;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::table_expr;
use holon_integration_tests::TestEnvironment;

const QUERY: &str = "SELECT id, val FROM cc_probe_t ORDER BY id";

#[test]
fn a_watch_on_a_not_yet_created_table_discloses_then_recovers() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    env.start_app(false).await.expect("start_app");

    let db = env
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();

    // The race only exists once registration is closed: in SchemaInit the DDL
    // queue legitimately parks instead of failing.
    db.transition_to_ready()
        .await
        .expect("transition the actor to Ready");

    let reactive: Arc<ReactiveEngine> = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: Arc<dyn BuilderServices> = reactive.clone();

    // `cc_probe_t` does not exist yet — exactly the state the sections are in
    // while the integration is still ingesting.
    let (key, live) = reactive.watch_query_live(
        QUERY.to_string(),
        QueryLanguage::HolonSql,
        table_expr(),
        None,
        services.clone(),
    );
    let rows = reactive.ensure_watching(&key);

    let deadline = Instant::now() + Duration::from_secs(20);
    while rows.error().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        rows.error().is_some(),
        "a watch over a table that does not exist must DISCLOSE the failure, not sit silently \
         empty"
    );
    assert!(
        !rows.is_loading(),
        "a disclosed failure must stop reporting as loading"
    );

    // The ingest lands.
    db.execute_ddl("CREATE TABLE cc_probe_t (id TEXT PRIMARY KEY, val TEXT)")
        .await
        .expect("create the late table");
    for (id, val) in [("a", "alpha"), ("b", "beta")] {
        db.execute_values(
            "INSERT INTO cc_probe_t (id, val) VALUES (?, ?)",
            vec![
                Value::String(id.to_string()),
                Value::String(val.to_string()),
            ],
        )
        .await
        .expect("insert row");
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (_expr, snapshot) = rows.snapshot();
        if snapshot.len() == 2 && rows.error().is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the block NEVER populated after its table appeared: rows={} error={:?} — the watcher \
             gave up on the first failure instead of retrying",
            snapshot.len(),
            rows.error()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(
        rows.error(),
        None,
        "recovery must CLEAR the disclosed error, not leave the block showing a stale failure"
    );
    assert!(
        !rows.is_loading(),
        "a recovered block must render its rows, not the loading sentinel"
    );

    drop(live);
}
