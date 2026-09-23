#![cfg(feature = "pbt")]
//! A `source_text` write judges whether it demotes a task by the block's
//! stored keyword. That keyword must come from the write authority: read from
//! a projection that has not caught up on the previous keystroke's demotion, a
//! task already demoted looks like a task still, and every later keystroke
//! writes the clear again — one more operation and one more undo step each.
//!
//! ## Why this binary holds exactly ONE test
//!
//! Same reason as `projector_lag_lock.rs`: the lag is a process-global
//! environment variable read by the projector on every pass, so a second test
//! here would silently run under it.
//!
//! @pbt kind harness
//! @pbt covers task-keyword-read-authority — a keystroke after a demotion
//! reads the demoted state from the write authority under projection lag

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;

/// Far wider than the race needs; the point is determinism, not calibration.
const LAG_MS: &str = "600";

const TARGET: &str = "block:kw-lag-target";

const TASK_STATE_WRITES: &str = "SELECT id FROM operation WHERE operation LIKE '%task_state%' AND operation LIKE '%kw-lag-target%'";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

fn source_text(value: &str) -> HashMap<String, Value> {
    HashMap::from([
        ("id".to_string(), Value::String(TARGET.to_string())),
        (
            "field".to_string(),
            Value::String(holon_api::SOURCE_TEXT_FIELD.to_string()),
        ),
        ("value".to_string(), Value::String(value.to_string())),
    ])
}

#[test]
fn a_keystroke_after_a_demotion_does_not_clear_the_task_state_again() {
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test (module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
    }

    let rt = runtime();
    rt.clone().block_on(async move {
        let world = TestEnvironmentBuilder::new()
            .with_org_file(
                "KwLag.org".to_string(),
                "#+TITLE: Kw Lag\n* TODO Target\n:PROPERTIES:\n:ID: kw-lag-target\n:END:\n"
                    .to_string(),
            )
            .build(rt.clone())
            .await
            .expect("boot the vault");
        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;
        let before = world
            .query_sql(TASK_STATE_WRITES)
            .await
            .expect("count task-state writes")
            .len();

        world
            .execute_operation("block", "set_field", source_text("Target x"))
            .await
            .expect("demote the task by typing past its keyword");
        let projected = world
            .query_sql(&format!(
                "SELECT json_extract(properties, '$.task_state') AS task_state FROM block_raw \
                 WHERE id = '{TARGET}'"
            ))
            .await
            .expect("query the projection");
        assert_eq!(
            projected
                .first()
                .and_then(|r| r.get("task_state"))
                .and_then(|v| v.as_string()),
            Some("TODO"),
            "the projection already reflects the demotion, so this run does not exercise the lag \
             window at all — raise HOLON_TEST_PROJECTOR_LAG_MS"
        );
        world
            .execute_operation("block", "set_field", source_text("Target xy"))
            .await
            .expect("type one more character");

        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;
        let writes = world
            .query_sql(TASK_STATE_WRITES)
            .await
            .expect("count task-state writes")
            .len()
            - before;
        assert_eq!(
            writes, 1,
            "the demotion clears task_state once; a later keystroke that re-clears it read a \
             stale keyword"
        );
    });
}
