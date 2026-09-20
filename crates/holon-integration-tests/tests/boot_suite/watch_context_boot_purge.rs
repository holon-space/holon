//! A membership row left by a previous process must not survive the next boot.
//!
//! `watch_context` rows are owned by live watches. A process that dies — or
//! shuts down with panels open, where `Drop` finds the database actor already
//! gone — leaves rows nobody owns, and every one of them keeps a subtree
//! incrementally maintained in the shared view for a watch that no longer
//! exists. Boot truncates the table before this session registers anything,
//! and THAT is what makes "counted as a shutdown row, not a leak" honest.
//!
//! @pbt kind harness
//! @pbt covers boot-purge(watch_context) — a crashed process's membership rows
//! are gone after restart

use std::sync::Arc;

use holon_integration_tests::TestEnvironment;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn membership_rows(env: &TestEnvironment) -> usize {
    env.query_sql("SELECT watch_key FROM watch_context")
        .await
        .expect("query watch_context")
        .len()
}

#[test]
fn a_crashed_processs_membership_rows_are_purged_at_the_next_boot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.set_enable_loro(false);

        env.start_app(true).await.expect("boot-1 start_app");

        // What a crashed process leaves behind: a row with no owner in this
        // or any later process. Written directly, because the only other way
        // to produce one is to kill the process mid-watch.
        env.engine()
            .db_handle()
            .execute_values(
                "INSERT INTO watch_context (watch_key, context_id, kind, nonce) VALUES (?, ?, ?, \
                 ?)",
                vec![
                    holon_api::Value::String("root:block:from-a-dead-process".to_string()),
                    holon_api::Value::String("block:journals".to_string()),
                    holon_api::Value::String("root".to_string()),
                    holon_api::Value::String("nonce-of-a-process-that-is-gone".to_string()),
                ],
            )
            .await
            .expect("insert the orphan membership row");

        assert_eq!(
            membership_rows(&env).await,
            1,
            "the orphan row must exist before the restart, or this test proves nothing"
        );

        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2 over the SAME database ──────────────────────────────
        env.start_app(true).await.expect("boot-2 start_app");

        let survivors = env
            .query_sql("SELECT watch_key FROM watch_context")
            .await
            .expect("query watch_context after restart");
        assert!(
            survivors.is_empty(),
            "a previous process's membership rows survived the boot truncate: {survivors:?} — \
             each keeps a subtree incrementally maintained for a watch that no longer exists"
        );

        env.stop_app().await.expect("stop_app after boot-2");
    });
}
