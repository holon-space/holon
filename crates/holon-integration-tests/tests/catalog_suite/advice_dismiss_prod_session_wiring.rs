//! Env-gap closure for BugFunnel row 26 ("`dismiss_advice` is NOT executable
//! in the prod GPUI session").
//!
//! The advice live-MCP gate (`advice_live_mcp_gate.rs`) proves `dismiss_advice`
//! dispatchable over the wire, but it drives the COMPOSED `full_headless`
//! session, whose block CRUD authority is the Loro `LoroBlockOperations`
//! provider — the only provider that advertised the `dismiss_advice` op. The
//! SqlOnly wiring (`crdt.enabled = false`, `wiring.rs`) instead routes block
//! CRUD to `SqlOperationProvider`. That provider did NOT
//! advertise or handle `dismiss_advice`, so the woven advice row's own
//! `dismiss_advice` op_button dispatched into the dispatcher and hit
//! "No provider registered for entity: 'block'" — the dismiss gesture was dead
//! in prod even though the weave rendered.
//!
//! This test closes that gap by booting the real DI container via
//! `TestEnvironment::start_app` — the SAME wiring the desktop app resolves —
//! and dispatching `block.dismiss_advice` through the prod `FrontendSession`.
//! It asserts:
//!
//!   1. The op is dispatchable in THIS (SqlOnly) session — it returns Ok, NOT
//!      "No provider registered for entity: block".
//!   2. The dismissal PERSISTS: a row `(anchor_id, lesson_id)` lands in the
//!      `advice_suppressed` junction (the authored exclusion set the weave's
//!      anti-join reads).
//!   3. It is IDEMPOTENT: re-dismissing the same lesson leaves exactly one row.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

/// Pick two distinct seeded `block:` blocks to play anchor and lesson. Both
/// must be real rows in `block_raw` because `advice_suppressed.lesson_id` has
/// an immediate FK into it.
async fn pick_two_blocks(session: &holon_frontend::FrontendSession) -> (String, String) {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    let ids: Vec<String> = snap
        .iter_blocks()
        .map(|b| b.id.as_str().to_string())
        .filter(|id| id.starts_with("block:"))
        .collect();
    assert!(
        ids.len() >= 2,
        "need at least two seeded block: blocks; got {ids:?}"
    );
    (ids[0].clone(), ids[1].clone())
}

/// Count `advice_suppressed` rows for a given (anchor, lesson) pair.
async fn suppressed_rows(engine: &holon::api::BackendEngine, anchor: &str, lesson: &str) -> usize {
    let sql = format!(
        "SELECT anchor_id, lesson_id FROM advice_suppressed WHERE anchor_id = '{}' AND lesson_id \
         = '{}'",
        anchor.replace('\'', "''"),
        lesson.replace('\'', "''"),
    );
    engine
        .db_handle()
        .query(&sql, HashMap::new())
        .await
        .expect("query advice_suppressed")
        .len()
}

/// Whether `id` is a row of `block_raw`: the FK target of
/// `advice_suppressed.anchor_id`.
async fn in_block_raw(engine: &holon::api::BackendEngine, id: &str) -> bool {
    let sql = format!(
        "SELECT id FROM block_raw WHERE id = '{}'",
        id.replace('\'', "''")
    );
    !engine
        .db_handle()
        .query(&sql, HashMap::new())
        .await
        .expect("query block_raw")
        .is_empty()
}

/// Whether the row shows up within two seconds. Separates a read that lags the
/// write from a write that never landed.
async fn row_lands_late(engine: &holon::api::BackendEngine, anchor: &str, lesson: &str) -> bool {
    for _ in 0..20 {
        if suppressed_rows(engine, anchor, lesson).await == 1 {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

#[test]
fn prod_session_dispatches_dismiss_advice() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");

        let session = env.session_arc();
        let engine = env.engine().clone();

        let (anchor_id, lesson_id) = pick_two_blocks(&session).await;
        assert_eq!(
            suppressed_rows(&engine, &anchor_id, &lesson_id).await,
            0,
            "fresh session: the anchor must not already suppress the lesson"
        );

        let mut params = HashMap::new();
        params.insert("anchor_id".to_string(), Value::String(anchor_id.clone()));
        params.insert("lesson_id".to_string(), Value::String(lesson_id.clone()));

        // (1) The op must dispatch in the SqlOnly prod session. Pre-fix this
        //     returned Err("No provider registered for entity: block").
        session
            .execute_operation(&EntityName::new("block"), "dismiss_advice", params.clone())
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "PROD SESSION GAP (row 26): block.dismiss_advice is not dispatchable in the \
                     SqlOnly app session: {e:#}"
                )
            });

        // (2) The dismissal persisted in the authored exclusion set.
        let rows = suppressed_rows(&engine, &anchor_id, &lesson_id).await;
        assert!(
            rows == 1,
            "dismiss_advice must append exactly one (anchor, lesson) row to advice_suppressed; \
             rows={rows} anchor_in_block_raw={} lesson_in_block_raw={} \
             row_lands_within_2s={}",
            in_block_raw(&engine, &anchor_id).await,
            in_block_raw(&engine, &lesson_id).await,
            row_lands_late(&engine, &anchor_id, &lesson_id).await,
        );

        // (3) Idempotent — re-dismissing the same lesson is a no-op.
        session
            .execute_operation(&EntityName::new("block"), "dismiss_advice", params)
            .await
            .expect("second dismiss_advice dispatch");
        assert_eq!(
            suppressed_rows(&engine, &anchor_id, &lesson_id).await,
            1,
            "dismiss_advice must be idempotent — a re-dismiss must not duplicate the junction row"
        );
    });
}
