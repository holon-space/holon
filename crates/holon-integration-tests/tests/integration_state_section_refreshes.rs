//! Contract: the seeded Integrations section REFRESHES when integration state
//! changes after its watch is established.
//!
//! Dogfood escape `2026-08-18-integrations-section-shows-one-stale-row`: the
//! live app showed exactly ONE integration (`claude-history`, the first
//! provider in the bundle) with all four enabled, and it never changed over
//! 5+ minutes. The section's watch had latched a snapshot taken partway
//! through the projector's row-by-row population and never saw the rest.
//!
//! `watch_recovers_when_table_appears.rs` is the closest existing rung and it
//! does NOT cover this: it writes its rows BEFORE the watch first succeeds, so
//! it proves recovery-by-retry, not ongoing delivery. Nothing asserted that a
//! write landing AFTER a successful watch reaches the section — which is
//! exactly the window this bug lives in.
//!
//! The GPUI seeded-sidebar test cannot cover it either: `TestServices` fakes
//! `watch_query_live` with canned static rows
//! (`frontends/gpui/tests/support/mod.rs`), so it proves the section RENDERS
//! rows, never that it RECEIVES new ones.
//!
//! @pbt kind harness
//! @pbt covers integration-state-section-refresh — a write to
//! `integration_state` after the section's watch is live reaches the section
//! @pbt overlaps watch_recovers_when_table_appears — kept: that rung covers the
//! table-absent-then-appears recovery, this one covers steady-state delivery

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::di::DbHandleProvider;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_app::integrations_section::SIDEBAR_SQL;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::table_expr;
use holon_integration_tests::TestEnvironment;

fn insert_sql() -> &'static str {
    "INSERT INTO integration_state \
     (id, provider_name, enabled, status, config_status, updated_at) \
     VALUES (?, ?, 1, 'Pending', 'unconfigured', '2026-08-18 00:00:00')"
}

#[test]
fn a_row_written_after_the_watch_is_live_reaches_the_section() {
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
    db.transition_to_ready()
        .await
        .expect("transition the actor to Ready");

    let reactive: Arc<ReactiveEngine> = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: Arc<dyn BuilderServices> = reactive.clone();

    // Start from an empty mirror so the staging below is the only source of
    // rows. `TestEnvironment` boots the real wiring, so the projector has
    // already populated it.
    db.execute_values("DELETE FROM integration_state", vec![])
        .await
        .expect("clear the mirror");

    // The projector's FIRST provider lands before the section is watched —
    // the live app's ordering, where boot populates row 1 and the sidebar
    // renders while the rest are still being written.
    db.execute_values(
        insert_sql(),
        vec![
            Value::String("integration:claude-history".to_string()),
            Value::String("claude-history".to_string()),
        ],
    )
    .await
    .expect("insert the first provider");

    let (key, live) = reactive.watch_query_live(
        SIDEBAR_SQL.to_string(),
        QueryLanguage::HolonSql,
        table_expr(),
        None,
        services.clone(),
    );
    let rows = reactive.ensure_watching(&key);

    // The watch settles on the one row that exists.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_expr, snapshot) = rows.snapshot();
        if snapshot.len() == 1 && rows.error().is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "sanity: the section must first deliver the one row that exists — rows={} error={:?}",
            snapshot.len(),
            rows.error()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // THE RED. The remaining providers land AFTER the watch is established —
    // the exact window the dogfood bug lives in.
    for provider in ["gcal", "gmail", "todoist"] {
        db.execute_values(
            insert_sql(),
            vec![
                Value::String(format!("integration:{provider}")),
                Value::String(provider.to_string()),
            ],
        )
        .await
        .expect("insert a later provider");
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (_expr, snapshot) = rows.snapshot();
        if snapshot.len() == 4 && rows.error().is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the Integrations section NEVER saw the rows written after its watch went live: \
             rows={} error={:?} — a section that latches one snapshot and stops is how the live \
             app showed a single stale integration for minutes",
            snapshot.len(),
            rows.error()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    drop(live);
}

/// The live app's actual sequence: the sidebar's watch goes live while the
/// mirror is still EMPTY (the projector runs late in the boot — see
/// `integration_state_boot_population.rs`), and every row arrives afterwards.
///
/// Separate from the case above because an empty first result is a different
/// state from a non-empty one: a watcher that caches "no rows" and stops is
/// indistinguishable from a correct empty section until rows appear.
#[test]
fn a_watch_that_starts_empty_still_receives_every_later_row() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run_from_empty(runtime.clone()));
}

async fn run_from_empty(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    env.start_app(false).await.expect("start_app");

    let db = env
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();
    db.transition_to_ready()
        .await
        .expect("transition the actor to Ready");

    db.execute_values("DELETE FROM integration_state", vec![])
        .await
        .expect("clear the mirror");

    let reactive: Arc<ReactiveEngine> = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: Arc<dyn BuilderServices> = reactive.clone();

    let (key, live) = reactive.watch_query_live(
        SIDEBAR_SQL.to_string(),
        QueryLanguage::HolonSql,
        table_expr(),
        None,
        services.clone(),
    );
    let rows = reactive.ensure_watching(&key);

    // Let the empty result settle, so the rows below genuinely arrive after the
    // watch has produced a snapshot rather than racing its first read.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_expr, snapshot) = rows.snapshot();
    assert_eq!(
        snapshot.len(),
        0,
        "sanity: the section starts empty in this rung"
    );

    for provider in ["claude-history", "gcal", "gmail", "todoist"] {
        db.execute_values(
            insert_sql(),
            vec![
                Value::String(format!("integration:{provider}")),
                Value::String(provider.to_string()),
            ],
        )
        .await
        .expect("insert a provider");
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (_expr, snapshot) = rows.snapshot();
        if snapshot.len() == 4 && rows.error().is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a section whose watch went live on an EMPTY mirror never received the rows the \
             projector wrote afterwards: rows={} error={:?}",
            snapshot.len(),
            rows.error()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    drop(live);
}
