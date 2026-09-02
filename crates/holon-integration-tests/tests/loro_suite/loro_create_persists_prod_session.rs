//! Regression lock for the 2026-07-21 dogfood "silent create no-persist" bug
//! (ORACLE / fail-loud violation).
//!
//! Live symptom: over the MCP server, `execute_operation(block, create, …)`
//! returned "executed successfully" but persisted NOTHING to `block_raw`
//! (reproduced under both a top-level sentinel parent and an existing block),
//! while `insert_text` / `delete` / `add_tag` (which touch already-projected
//! rows) DID persist.
//!
//! This test drives `block.create` through the EXACT live-MCP path and proves
//! it DOES persist on current `main`, so it locks the invariant and pins the
//! root cause: the drop was NOT in create's routing/authority/origin. The
//! reproduction ruled out, in order:
//!   * routing — create routes to the Loro `LoroBlockOperations` CRUD authority
//!     (`TestEnvironment::new` defaults to Loro-enabled, the GPUI wiring), not
//!     a no-op provider;
//!   * origin — dispatched through the SAME `HolonService` facade the MCP
//!     server builds, with `OpOrigin::Agent` provenance (server.rs
//!     `service()`), NOT the User-origin `FrontendSession::execute_operation`;
//!   * timing — the row is present once the projection quiesces within the
//!     budget, so the write is not lost, only deferred.
//!
//! With CRDT enabled the Loro commit IS the persist; the `block_raw` row is a
//! projection of it, produced by the spawned reconcile loop in
//! `holon_loro::loro_sync_controller` and therefore NOT promised at the moment
//! `create` returns. What this test locks is that the projection delivers the
//! row within the quiescence budget — a wedged loop exhausts it and fails
//! loudly, which is exactly the escape above.
//!
//! The live drop was therefore ENVIRONMENT-class: the co-landed backlinks
//! matview-recreation storm (fixed in `main` e9bcb287) wedged the Loro→SQL
//! projection loop, so new-block INSERTs never landed while boot-seeded rows
//! (the targets of insert_text/add_tag/delete) stayed visible. This lock guards
//! against the create path itself ever silently dropping the row again.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

/// Count `block_raw` rows for a given block id.
async fn block_rows(engine: &holon::api::BackendEngine, id: &str) -> usize {
    let sql = format!(
        "SELECT id FROM block_raw WHERE id = '{}'",
        id.replace('\'', "''"),
    );
    engine
        .db_handle()
        .query(&sql, HashMap::new())
        .await
        .expect("query block_raw")
        .len()
}

/// Pick one seeded `block:` id to serve as an existing parent.
async fn pick_block(session: &holon_frontend::FrontendSession) -> String {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    snap.iter_blocks()
        .map(|b| b.id.as_str().to_string())
        .find(|id| id.starts_with("block:"))
        .expect("need at least one seeded block: block")
}

/// A create dispatched through the prod session must land a real `block_raw`
/// row once the Loro→SQL projection quiesces — under BOTH parent shapes the
/// dogfood hit (existing block and the top-level sentinel).
#[test]
fn prod_session_create_block_persists_to_block_raw() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        let session = env.session_arc();
        let engine = env.engine().clone();

        // The MCP server dispatches through `HolonService` with Agent
        // provenance (server.rs `service()`), NOT through the User-origin
        // `FrontendSession::execute_operation`. Reproduce that exactly.
        let agent_service = holon::api::holon_service::HolonService::new_with_origin(
            engine.clone(),
            holon_api::OpOrigin::Agent {
                session_id: "mcp-session:test".to_string(),
                tool_call_id: "tool-call:test".to_string(),
            },
        );

        let parent_id = pick_block(&session).await;

        for (label, new_id, parent) in [
            (
                "under an existing block",
                "block:create-persist-probe-child",
                parent_id.clone(),
            ),
            (
                "under the top-level sentinel",
                "block:create-persist-probe-top",
                "sentinel:no_parent".to_string(),
            ),
        ] {
            assert_eq!(
                block_rows(&engine, new_id).await,
                0,
                "{label}: fresh id must not already exist"
            );

            let mut params: HashMap<std::sync::Arc<str>, Value> = HashMap::new();
            params.insert("id".into(), Value::String(new_id.to_string()));
            params.insert("parent_id".into(), Value::String(parent));
            params.insert("content".into(), Value::String(format!("probe {label}")));

            agent_service
                .execute_operation(&EntityName::new("block"), "create", params)
                .await
                .unwrap_or_else(|e| panic!("{label}: block.create dispatch failed: {e:#}"));

            // The row is a projection of the Loro commit, so it lands once the
            // reconcile loop has run — a wedged loop exhausts this budget.
            env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
            assert_eq!(
                block_rows(&engine, new_id).await,
                1,
                "{label}: block.create returned success but NO row reached block_raw \
                 within the projection budget — success-before-persist (fail-loud \
                 violation)"
            );
        }
    });
}
