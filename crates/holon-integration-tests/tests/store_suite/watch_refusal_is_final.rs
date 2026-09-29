//! Contract: a watch the engine refuses for good is disclosed once and never
//! attempted again, and a query that merely mentions the word `clock` is not
//! refused.
//!
//! @pbt kind harness
//! @pbt covers watch-refusal-is-final — a refused query watcher discloses and
//! stops; a query that reads no clock row is served

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::QueryLanguage;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::ReactiveRenderedRows;
use holon_frontend::reactive::table_expr;
use holon_integration_tests::TestEnvironment;

const MENTIONS_CLOCK: &str = "SELECT id, content FROM block WHERE content LIKE '%clock%'";
const NAMES_NO_GRAIN: &str =
    "SELECT b.id, c.today FROM block b JOIN clock c ON c.grain = b.content";

#[test]
fn a_refused_watch_is_disclosed_once_and_a_clock_mention_is_served() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

fn watch(
    reactive: &Arc<ReactiveEngine>,
    sql: &str,
) -> (
    holon_frontend::reactive::LiveBlock,
    Arc<ReactiveRenderedRows>,
) {
    let services: Arc<dyn BuilderServices> = reactive.clone();
    let (key, live) = reactive.watch_query_live(
        sql.to_string(),
        QueryLanguage::HolonSql,
        table_expr(),
        None,
        services,
    );
    (live, reactive.ensure_watching(&key))
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    env.start_app(false).await.expect("start_app");
    let reactive: Arc<ReactiveEngine> = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();

    let (_mentions_live, mentions) = watch(&reactive, MENTIONS_CLOCK);
    let (_refused_live, refused) = watch(&reactive, NAMES_NO_GRAIN);

    let deadline = Instant::now() + Duration::from_secs(20);
    while (mentions.is_loading() && mentions.error().is_none()) || refused.error().is_none() {
        assert!(
            Instant::now() < deadline,
            "the watches did not settle: mentions loading={} error={:?}; refused error={:?}",
            mentions.is_loading(),
            mentions.error(),
            refused.error()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        mentions.error(),
        None,
        "a query that reads no clock row must be served: {MENTIONS_CLOCK}"
    );
    let disclosed = refused.error().expect("checked above");
    assert!(
        disclosed.contains("names no grain"),
        "the refusal must say why: {disclosed}"
    );

    refused.clear_error();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        refused.error(),
        None,
        "a refused watch was attempted again; a refusal cannot succeed on a retry"
    );
}
