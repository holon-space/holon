//! Premise-check + env-gap closure for BugFunnel row 28 ("Live-authored
//! `holon_rule` source block never activates").
//!
//! Dogfood #6 authored a `holon_rule` block at runtime over MCP: it landed in
//! `block_raw` with the right `content_type`/`source_language` and a one-shot
//! discovery SELECT matched it, yet no operate watcher started and the rule
//! never fired until app restart — despite discovery being a live CDC watch
//! (`holon_rule_watcher::start_holon_rule_watchers` → `query_and_watch` over
//! `holon_rule_discovery.sql`).
//!
//! This test drives that exact path through the PROD session wiring
//! (`TestEnvironment::start_app` = the DI graph GPUI resolves, which spawns the
//! discovery watcher via `wiring.rs:407` → `start_action_watchers` →
//! `start_holon_rule_watchers`). It authors a clock-subject operate rule at
//! runtime and asserts the discovery watcher picks it up — proven by the rule's
//! `RuleStatus` becoming `Active` (`holon_rule_watcher::start_rule` sets it and
//! logs "starting operate watcher" on the exact success path the dogfood found
//! silent).
//!
//! If the status never appears the discovery CDC → watcher-spawn path is not
//! driven at runtime (row 28's claim); if it does, the runtime-author path is
//! wired and the row narrows to the restart-only observation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityName;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

/// A clock-subject operate rule (the `daily_journal` shape) — carries an
/// `emit`, so `start_rule` treats it as an operate rule and (on the success
/// path) sets `RuleStatus::Active`. Mirrors
/// `holon_rule_watcher::tests::JOURNAL_RULE`.
const RUNTIME_RULE_YAML: &str = r#"
name: runtime_authored_probe
when: 'not block_exists("Journals/{today}")'
emit:
  place: page(journals)
  name: "{today}"
"#;

#[test]
fn runtime_authored_holon_rule_is_discovered_by_the_live_watcher() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");

        let session = env.session_arc();
        let engine = env.engine().clone();

        // A real seeded block to graft the rule host under (create requires a
        // valid parent_id).
        let grandparent = {
            let snap = session
                .block_query()
                .snapshot()
                .await
                .expect("block snapshot");
            snap.iter_blocks()
                .map(|b| b.id.as_str().to_string())
                .find(|id| id.starts_with("block:"))
                .expect("at least one seeded block to parent under")
        };

        // Author the rule under a fresh host so it has no sibling
        // `holon_sql`/`holon_prql`/`holon_gql` trigger — that keeps it a
        // single-block rule the `holon_rule_watcher` owns (an unpaired block),
        // not a query+action pair the legacy `action_watcher` claims.
        let parent_id = "block:row28-rule-parent".to_string();
        let mut parent_params = HashMap::new();
        parent_params.insert("id".to_string(), Value::String(parent_id.clone()));
        parent_params.insert("parent_id".to_string(), Value::String(grandparent.clone()));
        parent_params.insert(
            "content".to_string(),
            Value::String("Row28 rule host".to_string()),
        );
        parent_params.insert("block_type".to_string(), Value::String("text".to_string()));
        session
            .execute_operation(&EntityName::new("block"), "create", parent_params)
            .await
            .expect("create parent block");

        let rule_id = "block:row28-runtime-rule".to_string();
        let mut rule_params = HashMap::new();
        rule_params.insert("id".to_string(), Value::String(rule_id.clone()));
        rule_params.insert("parent_id".to_string(), Value::String(parent_id.clone()));
        rule_params.insert(
            "content".to_string(),
            Value::String(RUNTIME_RULE_YAML.to_string()),
        );
        rule_params.insert(
            "content_type".to_string(),
            Value::String("source".to_string()),
        );
        rule_params.insert(
            "source_language".to_string(),
            Value::String("holon_rule".to_string()),
        );
        rule_params.insert(
            "source_name".to_string(),
            Value::String("runtime_authored_probe".to_string()),
        );
        session
            .execute_operation(&EntityName::new("block"), "create", rule_params)
            .await
            .expect("create holon_rule block at runtime");

        // Sanity: the one-shot discovery SELECT matches it (the dogfood confirmed
        // this held even though the live watcher stayed silent).
        let discovery = engine
            .db_handle()
            .query(
                "SELECT id FROM block WHERE content_type = 'source' AND source_language = \
                 'holon_rule'",
                HashMap::new(),
            )
            .await
            .expect("discovery SELECT");
        let matched: Vec<String> = discovery
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .collect();
        assert!(
            matched.contains(&rule_id),
            "the runtime-authored rule must match the discovery query (block matview lagging?); \
             matched={matched:?}"
        );

        // The claim under test: the LIVE discovery watcher spawns an operate
        // watcher for the runtime-authored rule → its RuleStatus becomes Active.
        let status = engine.rule_status().clone();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = None;
        while Instant::now() < deadline {
            if let Some(s) = status.get(&rule_id) {
                seen = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            seen.is_some(),
            "ROW 28: the live holon_rule discovery watcher never picked up the runtime-authored \
             rule {rule_id} — no RuleStatus was ever set (CDC discovery → watcher-spawn path not \
             driven at runtime). It matches the one-shot SELECT: {matched:?}"
        );
        let seen = seen.unwrap();
        assert!(
            seen.is_active(),
            "ROW 28: the runtime-authored rule was discovered but did not go Active (status: \
             {seen:?}) — discovery fired but the operate watcher did not start on the success path"
        );
    });
}
