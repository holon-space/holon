//! The MIDDLE hop of the ordering-spec chain: `ensure_query_watching` derives
//! a watched query's sort order from its compiled `ORDER BY` and installs it on
//! the `ReactiveRenderedRows` the collection renders.
//!
//! The two ENDS are covered elsewhere and cannot see this wiring: the compile
//! side by `ordering_spec_carries_a_prql_sort_to_the_render_layer` (crate
//! `holon`), the render side by the `reactive_view` tests — which HAND-SET the
//! spec via `set_ordering_spec`. Here the spec is derived by production code
//! only; the test never sets it.
//!
//! A full keystone rung for this seam is blocked on #42's QueryTable widening;
//! this integration-level test stands in until then.
//!
//! @pbt kind harness
//! @pbt covers watch-query-ordering-spec-install — the engine derives and
//! installs a watched query's declared row order

use std::sync::Arc;

use holon_api::QueryLanguage;
use holon_api::ReactiveRowProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::table_expr;
use holon_integration_tests::TestEnvironment;

#[test]
fn ensure_query_watching_installs_the_querys_declared_order() {
    // SUT owns its runtime; plain #[test] + block_on keeps the runtime Drop on
    // the main thread (same shape as watch_guard_raii.rs).
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    env.start_app(false).await.expect("start_app");

    let reactive = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: Arc<dyn BuilderServices> = reactive.clone();

    let descending = spec_of(
        &reactive,
        &services,
        "from block | select {id, content} | sort {-content}",
    );
    assert_eq!(
        descending.as_deref(),
        Some("-content"),
        "watch_query_live must install the descending order its query declares"
    );

    let ascending = spec_of(
        &reactive,
        &services,
        "from block | select {id, content} | sort content",
    );
    assert_eq!(
        ascending.as_deref(),
        Some("content"),
        "watch_query_live must install the ascending order its query declares"
    );

    let unordered = spec_of(&reactive, &services, "from block | select {id, content}");
    assert_eq!(
        unordered, None,
        "a query that declares no order must leave the collection unsorted"
    );
}

/// Watch `query` through the production `watch_query_live` →
/// `ensure_query_watching` path and read back the spec the engine installed.
/// The `LiveBlock` is held until after the read so its `WatchGuard` cannot
/// release the watcher first.
fn spec_of(
    reactive: &Arc<holon_frontend::reactive::ReactiveEngine>,
    services: &Arc<dyn BuilderServices>,
    query: &str,
) -> Option<String> {
    let (key, live) = reactive.watch_query_live(
        query.to_string(),
        QueryLanguage::HolonPrql,
        table_expr(),
        None,
        services.clone(),
    );
    let spec = reactive.ensure_watching(&key).ordering_spec();
    drop(live);
    spec
}
