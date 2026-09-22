#![cfg(feature = "pbt")]
//! `instantiate_template` dispatched right after its template was written must
//! find the template although the SQL projection has not caught up: the
//! definition is read from the write authority, never from the projection.
//!
//! ## Why this binary holds exactly ONE test
//!
//! Same reason as `projector_lag_lock.rs`: the lag is a process-global
//! environment variable read by the projector on every pass, so a second test
//! here would silently run under it.
//!
//! @pbt kind harness
//! @pbt covers template-read-authority — instantiate_template reads its
//! definition from the write authority under projection lag

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_integration_tests::template_fixture;

/// Far wider than the race needs; the point is determinism, not calibration.
const LAG_MS: &str = "600";

const TARGET: &str = "block:tpl-lag-target";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

fn params(pairs: Vec<(&str, Value)>) -> HashMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

#[test]
fn instantiate_template_finds_a_template_the_projection_has_not_reached() {
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test (module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
    }

    let rt = runtime();
    rt.clone().block_on(async move {
        let world = TestEnvironmentBuilder::new()
            .with_org_file(
                "TplLag.org".to_string(),
                "#+TITLE: Tpl Lag\n* Target\n:PROPERTIES:\n:ID: tpl-lag-target\n:END:\n"
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
        assert_eq!(
            world
                .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{TARGET}'"))
                .await
                .expect("query the projection")
                .len(),
            1,
            "the instantiation target must be settled before the lag window opens"
        );

        world
            .execute_operation(
                "block",
                "create",
                params(vec![
                    ("id", Value::String(template_fixture::TPL_ROOT.to_string())),
                    (
                        "parent_id",
                        Value::String(EntityUri::no_parent().to_string()),
                    ),
                    (
                        "content",
                        Value::String(template_fixture::TPL_ROOT_CONTENT.to_string()),
                    ),
                    ("template", Value::String("t".to_string())),
                    (
                        "template_vars",
                        Value::String(template_fixture::TPL_VARS.to_string()),
                    ),
                ]),
            )
            .await
            .expect("create the template root");
        world
            .execute_operation(
                "block",
                "create",
                params(vec![
                    ("id", Value::String(template_fixture::TPL_CHILD.to_string())),
                    (
                        "parent_id",
                        Value::String(template_fixture::TPL_ROOT.to_string()),
                    ),
                    (
                        "content",
                        Value::String(template_fixture::TPL_CHILD_CONTENT.to_string()),
                    ),
                    (
                        "marks",
                        Value::String(template_fixture::tpl_child_marks_json()),
                    ),
                ]),
            )
            .await
            .expect("create the template child");

        let projected = world
            .query_sql(&format!(
                "SELECT id FROM block_raw WHERE id = '{}'",
                template_fixture::TPL_ROOT
            ))
            .await
            .expect("query the projection");
        assert!(
            projected.is_empty(),
            "the projection already holds the template, so this run does not exercise the lag \
             window at all — raise HOLON_TEST_PROJECTOR_LAG_MS"
        );

        let bindings = [("date".to_string(), "xyz".to_string())];
        world
            .execute_operation(
                "block",
                "instantiate_template",
                params(vec![
                    (
                        "template_id",
                        Value::String(template_fixture::TPL_ROOT.to_string()),
                    ),
                    ("target_parent", Value::String(TARGET.to_string())),
                    ("context_key", Value::String("lag".to_string())),
                    (
                        "bindings",
                        Value::Object(
                            bindings
                                .iter()
                                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                                .collect(),
                        ),
                    ),
                ]),
            )
            .await
            .expect("instantiate a template the projection has not reached yet");

        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;

        let with_mood: Vec<(String, String)> = bindings
            .iter()
            .cloned()
            .chain([("mood".to_string(), "neutral".to_string())])
            .collect();
        let roots = world
            .query_sql(&format!(
                "SELECT id, content FROM block_raw WHERE parent_id = '{TARGET}'"
            ))
            .await
            .expect("query the instance root");
        assert_eq!(roots.len(), 1, "exactly one instance root, got {roots:?}");
        let root = &roots[0];
        assert_eq!(
            root.get("content").and_then(|v| v.as_string()),
            Some(
                template_fixture::instantiated_root(&with_mood)
                    .content
                    .as_str()
            ),
            "instance root content"
        );
        let root_id = root
            .get("id")
            .and_then(|v| v.as_string())
            .expect("instance root id")
            .to_string();
        let children = world
            .query_sql(&format!(
                "SELECT content FROM block_raw WHERE parent_id = '{root_id}'"
            ))
            .await
            .expect("query the instance child");
        assert_eq!(
            children
                .iter()
                .map(|r| r
                    .get("content")
                    .and_then(|v| v.as_string())
                    .map(str::to_string))
                .collect::<Vec<_>>(),
            vec![Some(
                template_fixture::instantiated_child(&with_mood).content
            )],
            "instance child content"
        );
    });
}
