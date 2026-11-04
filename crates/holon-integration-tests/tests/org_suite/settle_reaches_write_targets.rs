//! `TestEnvironment::settle` returns only when every write made before it
//! reached its target: an external org edit is in the store, a UI edit is in
//! its file, and a slow subscriber applied its CDC batch. Each check after the
//! settle is ONE read, so a settle that returns on a quiet period alone (the
//! subscriber applies far later than any quiet window, the ingest emits no CDC
//! until it parsed the file) is red.
//!
//! @pbt kind harness
//! @pbt covers settle-reaches-write-targets — the env settle waits for each
//! write's target, never for quiet

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use holon::api::BackendEngine;
use holon_api::QueryLanguage;
use holon_api::live_data::DeliveryCursor;
use holon_api::live_data::reporting_to;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use proptest::prelude::*;
use tokio_stream::StreamExt;

const SLOW_APPLY: Duration = Duration::from_millis(300);

#[derive(Clone, Debug)]
enum Write {
    /// Rewrite `ext.org` on disk, as an editor outside the app would.
    External(String),
    /// Edit the block of `ui.org` through the app.
    Ui(String),
    /// Insert a row the slow subscriber watches.
    Sql,
}

fn write() -> impl Strategy<Value = Write> {
    prop_oneof![
        "[a-z]{3,10}".prop_map(Write::External),
        "[a-z]{3,10}".prop_map(Write::Ui),
        Just(Write::Sql),
    ]
}

fn org_file(id: &str, title: &str) -> String {
    format!("* {title}\n:PROPERTIES:\n:ID: {id}\n:END:\n")
}

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
        .execute_ddl("CREATE TABLE settle_probe (id TEXT PRIMARY KEY, owner TEXT NOT NULL)")
        .await
        .expect("create the probe table");
    let cursor = Arc::new(DeliveryCursor::default());
    let (_view, mut stream) = reporting_to(
        cursor.clone(),
        engine.matview_manager().ensure_and_subscribe(
            "SELECT id, owner AS watch_key FROM settle_probe",
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

async fn stored_content(env: &TestEnvironment, id: &str) -> String {
    let rows = env
        .query(
            &format!("from block | filter id == \"block:{id}\" | select {{content}}"),
            QueryLanguage::HolonPrql,
        )
        .await
        .expect("query the block");
    assert_eq!(rows.len(), 1, "block {id} is not in the store: {rows:?}");
    rows[0]
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| panic!("block {id} has no string content: {rows:?}"))
        .to_string()
}

/// Every check is one read after one settle; the failures name each write
/// whose target the settle did not wait for.
async fn writes_reach_their_targets(rt: Arc<tokio::runtime::Runtime>, writes: &[Write]) {
    let env = TestEnvironmentBuilder::new()
        .with_org_file("ext.org", org_file("ext-1", "start"))
        .with_org_file("ui.org", org_file("ui-1", "start"))
        .build(rt)
        .await
        .expect("build the environment");
    env.settle().await;
    let applied = slow_subscriber(env.engine()).await;
    env.settle().await;

    let mut sql_rows = 0u64;
    for (i, w) in writes.iter().enumerate() {
        match w {
            Write::External(title) => {
                FileSystem::write(
                    env.org_fs.as_ref(),
                    &env.org_file_path("ext.org"),
                    org_file("ext-1", title).as_bytes(),
                )
                .await
                .expect("rewrite ext.org");
            }
            Write::Ui(title) => env
                .update_block_content("ui-1", title)
                .await
                .expect("edit the ui block"),
            Write::Sql => {
                env.engine()
                    .db_handle()
                    .execute(
                        &format!("INSERT INTO settle_probe (id, owner) VALUES ('r{i}', 'mine')"),
                        vec![],
                    )
                    .await
                    .expect("insert a probe row");
                sql_rows += 1;
            }
        }
        // Writes made while an earlier one is still on its way also count.
        if i % 2 == 1 {
            env.settle().await;
        }
    }
    env.settle().await;

    let mut failures = Vec::new();
    if let Some(title) = writes.iter().rev().find_map(|w| match w {
        Write::External(t) => Some(t),
        _ => None,
    }) {
        let stored = stored_content(&env, "ext-1").await;
        if &stored != title {
            failures.push(format!(
                "org-ingest: the store holds {stored:?}, the file {title:?}"
            ));
        }
    }
    if let Some(title) = writes.iter().rev().find_map(|w| match w {
        Write::Ui(t) => Some(t),
        _ => None,
    }) {
        let file = env
            .org_fs
            .read_to_string(&env.org_file_path("ui.org"))
            .await
            .expect("read ui.org");
        if !file.contains(&format!("* {title}\n")) {
            failures.push(format!("org-writeback: ui.org lacks {title:?}:\n{file}"));
        }
    }
    let applied_rows = applied.load(Ordering::SeqCst);
    if applied_rows != sql_rows {
        failures.push(format!(
            "cdc-applied: {sql_rows} rows inserted, the subscriber applied {applied_rows}"
        ));
    }
    assert!(failures.is_empty(), "{writes:?}\n{}", failures.join("\n"));
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 6,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_settle_returns_only_when_each_write_reached_its_target(
        writes in proptest::collection::vec(write(), 1..5)
    ) {
        let rt = runtime();
        rt.block_on(writes_reach_their_targets(rt.clone(), &writes));
    }
}
