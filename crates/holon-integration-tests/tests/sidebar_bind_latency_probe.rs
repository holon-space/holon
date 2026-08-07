#![cfg(feature = "pbt")]
//! **Does a newly-created page's LeftSidebar row bind its `navigation.focus`
//! click-intent, and how long does that take?**
//!
//! `await_sidebar_intent` (the keystone's sidebar-click barrier) polls for
//! exactly this binding and fails the whole corpus after 5s — the
//! `sidebar-focus-bind` known-red family. The barrier cannot tell a SLOW bind
//! from one that never lands, and it observes only the one page the drawn
//! transition happens to name.
//!
//! This probe separates the two and turns the rate into a measurement:
//! creating pages back to back, it records each row's time-to-bind and, at the
//! end, compares the sidebar watch's row set against the sidebar's OWN SQL.
//! A page the query returns but the watch never holds is a lost update in the
//! watch's row-set maintenance (arrival-order sensitivity); a page that binds
//! only slowly is a budget question. The two verdicts point at different code.
//!
//! @pbt kind harness
//! @pbt covers sidebar-focus-bind — sidebar row-set convergence + bind latency

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::ClickModifiers;
use holon_api::EntityUri;
use holon_frontend::reactive::BuilderServices;
use holon_integration_tests::TestEnvironment;

/// Pages created back to back. Enough draws that a per-create race shows up as
/// a rate, few enough that the probe stays well inside a normal test budget.
const PAGES: usize = 12;

/// Per-row bind budget. Generous against the 5s the keystone barrier allows —
/// this probe is here to say WHICH failure happened, not to re-litigate the
/// barrier's budget.
const BIND_BUDGET: Duration = Duration::from_secs(15);

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    )
}

const SIDEBAR: &str = "block:default-left-sidebar";

/// The sidebar's own query, verbatim from `assets/default/index.org`. Kept in
/// sync by the assertion below: if the asset's query changes and this one does
/// not, the two row sets diverge and the probe fails loudly rather than
/// comparing against a query nothing renders.
const SIDEBAR_SQL: &str = "SELECT b.id FROM block b JOIN block_tags bt ON bt.block_id = b.id \
                           WHERE bt.tag = 'Page' AND b.id != 'block:__default__' \
                           ORDER BY b.content ASC";

#[test]
fn every_created_page_binds_its_sidebar_focus_intent() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironment::new_running(rt.clone())
            .await
            .expect("start a running Turso environment");
        let root_uri = holon_api::root_layout_block_uri();
        let reactive = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();

        let mut created: Vec<EntityUri> = Vec::new();
        let mut latencies: Vec<(EntityUri, Duration)> = Vec::new();

        for i in 0..PAGES {
            let page = env
                .create_document(&format!("bind_probe_{i}.org"))
                .await
                .unwrap_or_else(|e| panic!("create bind_probe_{i}.org: {e:#}"));
            created.push(page.clone());

            let started = Instant::now();
            let deadline = started + BIND_BUDGET;
            loop {
                let resolved = reactive.snapshot_resolved(&root_uri);
                if holon_frontend::focus_path::find_click_intent_in_region(
                    &resolved,
                    &page,
                    "left_sidebar",
                    ClickModifiers::none(),
                )
                .is_some()
                {
                    latencies.push((page.clone(), started.elapsed()));
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "page {i} ({page}) never bound a navigation.focus sidebar intent within \
                     {BIND_BUDGET:?}.\n  MISS REASON: {}\n  {}\n  bind latencies so far: {:?}",
                    holon_frontend::focus_path::click_intent_miss_reason(
                        &resolved,
                        &page,
                        "left_sidebar",
                        ClickModifiers::none(),
                    ),
                    holon_frontend::reactive::generation_drops::report(),
                    latencies
                        .iter()
                        .map(|(_, d)| d.as_millis())
                        .collect::<Vec<_>>(),
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        // Convergence contract: once every create has settled, the watch's row
        // set and the query's result set must be the SAME set. A page in the
        // query but not the watch is the lost update the barrier's timeout
        // would report only as "never bound".
        let sidebar = EntityUri::parse(SIDEBAR).expect("static sidebar key");
        let mut queried: Vec<String> = env
            .query_sql(SIDEBAR_SQL)
            .await
            .expect("sidebar query")
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
            .collect();
        queried.sort();

        let (_, rows) = reactive.ensure_watching(&sidebar).snapshot();
        let mut watched: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
            .collect();
        watched.sort();
        watched.dedup();

        for page in &created {
            assert!(
                queried.contains(&page.to_string()),
                "the sidebar's own SQL does not return the created page {page} — the probe's \
                 SIDEBAR_SQL has drifted from assets/default/index.org, or page creation stopped \
                 tagging `Page`. Query returned: {queried:?}"
            );
        }
        assert_eq!(
            watched,
            queried,
            "the sidebar watch's row set diverges from the sidebar's own query after every create \
             settled — a lost update in the watch's row-set maintenance, which is what a \
             `sidebar-focus-bind` timeout looks like from the barrier.\n  {}",
            holon_frontend::reactive::generation_drops::report(),
        );

        let millis: Vec<u128> = latencies.iter().map(|(_, d)| d.as_millis()).collect();
        let worst = millis.iter().copied().max().unwrap_or(0);
        println!("[sidebar-bind] per-page bind latency (ms): {millis:?}; worst={worst}ms");
    });
}
