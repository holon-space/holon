// # Action Watcher — query-triggered operations
//
// Mirrors UI rendering: query produces rows, sibling block processes them.
// For render blocks the output is a widget tree; for action blocks it's an
// execute_operation call routed through the command bus (traces, undo, events).
//
// ## Security & Sync (see Projects/Holon.org "Query-Triggered Actions")
//
// V1 only supports Local-scope actions (block CRUD, idempotent via INSERT OR
// IGNORE). Every peer executes independently and converges. Once-scope actions
// (external side effects like email/webhook) require a shared execution log for
// deduplication and are NOT yet supported. The execution gate belongs in the
// execute_operation pipeline, not here — when Once-scope is added, this module
// stays unchanged; only the dispatcher learns to check the dedup log.
//
// Action definitions sync via Loro like any block. Triggers fire locally per
// peer. A malicious collaborator in a shared sub-tree can inject Local-scope
// actions but can only create blocks they could also create manually.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use holon_api::InterpValue;
use holon_api::action_dsl::parse_action_dsl;
use holon_api::effect_id::{FiringKey, OutputSlot, RuleId, deterministic_block_id};
use holon_api::render_eval::{CORE_VALUE_FN_LOOKUP, eval_to_interp};
use holon_api::{EntityName, Value};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tracing::info;

use crate::api::backend_engine::BackendEngine;
use holon_api::streaming::Change;
use holon_core::storage::types::StorageEntity;

const DISCOVERY_SQL: &str = include_str!("../../../../assets/queries/action_discovery.sql");

#[tracing::instrument(skip(engine), name = "action_watcher.start")]
pub async fn start_action_watchers(engine: Arc<BackendEngine>) -> Result<()> {
    let discovery_stream = engine
        .query_and_watch(DISCOVERY_SQL.to_string(), HashMap::new(), None)
        .await
        .context("Failed to subscribe to action discovery matview")?;

    crate::util::spawn_actor(run_discovery_loop(engine, discovery_stream));
    Ok(())
}

async fn run_discovery_loop(
    engine: Arc<BackendEngine>,
    mut discovery_stream: crate::storage::turso::RowChangeStream,
) {
    let mut active: HashMap<String, JoinHandle<()>> = HashMap::new();

    while let Some(batch) = discovery_stream.next().await {
        for item in batch.inner.items {
            match item.change {
                Change::Created { data, .. } => {
                    let action_id = match extract_string(&data, "action_id") {
                        Some(id) => id,
                        None => {
                            tracing::warn!("[action_watcher] discovery row missing action_id");
                            continue;
                        }
                    };
                    let query_source = match extract_string(&data, "query_source") {
                        Some(s) => s,
                        None => {
                            tracing::warn!("[action_watcher] {action_id} missing query_source");
                            continue;
                        }
                    };
                    let query_language = match extract_string(&data, "query_language") {
                        Some(s) => s,
                        None => {
                            tracing::warn!("[action_watcher] {action_id} missing query_language");
                            continue;
                        }
                    };
                    let action_source = match extract_string(&data, "action_source") {
                        Some(s) => s,
                        None => {
                            tracing::warn!("[action_watcher] {action_id} missing action_source");
                            continue;
                        }
                    };

                    info!("[action_watcher] starting watcher for {action_id}");
                    let handle = tokio::spawn(run_pair_watcher(
                        engine.clone(),
                        action_id.clone(),
                        query_source,
                        query_language,
                        action_source,
                    ));
                    active.insert(action_id, handle);
                }
                Change::Deleted { id, .. } => {
                    if let Some(handle) = active.remove(&id) {
                        handle.abort();
                        info!("[action_watcher] aborted watcher for {id}");
                    }
                }
                Change::Updated { id, data, .. } => {
                    if let Some(handle) = active.remove(&id) {
                        handle.abort();
                    }
                    let query_source = match extract_string(&data, "query_source") {
                        Some(s) => s,
                        None => continue,
                    };
                    let query_language = match extract_string(&data, "query_language") {
                        Some(s) => s,
                        None => continue,
                    };
                    let action_source = match extract_string(&data, "action_source") {
                        Some(s) => s,
                        None => continue,
                    };
                    info!("[action_watcher] restarting watcher for {id}");
                    let handle = tokio::spawn(run_pair_watcher(
                        engine.clone(),
                        id.clone(),
                        query_source,
                        query_language,
                        action_source,
                    ));
                    active.insert(id, handle);
                }
                _ => {}
            }
        }
    }
}

async fn run_pair_watcher(
    engine: Arc<BackendEngine>,
    action_id: String,
    query_source: String,
    query_language: String,
    action_source: String,
) {
    if let Err(e) = run_pair_watcher_inner(
        engine,
        action_id.clone(),
        query_source,
        query_language,
        action_source,
    )
    .await
    {
        tracing::error!("[action_watcher] pair watcher for {action_id} failed: {e:#}");
    }
}

async fn run_pair_watcher_inner(
    engine: Arc<BackendEngine>,
    action_id: String,
    query_source: String,
    query_language: String,
    action_source: String,
) -> Result<()> {
    let language = holon_api::QueryLanguage::from_str(&query_language).with_context(|| {
        format!("Unknown query language '{query_language}' for action {action_id}")
    })?;

    let sql = engine
        .compile_to_sql(&query_source, language)
        .with_context(|| {
            format!("Failed to compile query for action {action_id}: {query_source}")
        })?;

    let parsed_action = parse_action_dsl(&action_source)
        .with_context(|| format!("Failed to parse action DSL for {action_id}: {action_source}"))?;

    let entity_name = EntityName::new(&parsed_action.entity);
    // Parse the discovery id into a typed rule identity once, at the watcher
    // boundary; every firing derives its deterministic effect id from it.
    let rule = RuleId::new(action_id.clone());

    // Every trigger is a data-reactive matview watch. Temporal triggers (the
    // journal auto-create) read the `clock` relation's materialized `today`
    // value instead of `date('now')`, so they too back a CDC matview: the
    // scheduler's day-rollover UPDATE re-fires the watch (ADR 0024 P5). The old
    // boot-one-shot `is_tableless` branch is gone — it never re-fired and was
    // the "temporal triggers never re-fire" defect (BugFunnel F4).
    let mut row_stream = engine
        .query_and_watch(sql, HashMap::new(), None)
        .await
        .with_context(|| format!("Failed to subscribe to query for action {action_id}"))?;

    while let Some(batch) = row_stream.next().await {
        for item in batch.inner.items {
            match item.change {
                Change::Created { data, .. } => {
                    fire_action(&engine, &entity_name, &parsed_action, &rule, &data).await;
                }
                // Re-fire on Updated ONLY for create-effect rules. A rowid-stable
                // UPDATE (the clock relation's day-rollover write, WP1) re-fires
                // the journal create; the deterministic effect id (WP2) makes the
                // re-create converge to a no-op upsert. Non-create effects
                // (set_field/update/delete) must NOT re-fire on Updated — they
                // would re-execute a side effect with no dedup (a pre-existing
                // re-fire hazard deferred to Phase-2 inhibitor guards). Gate
                // strictly on the operation kind.
                Change::Updated { data, .. } if parsed_action.operation == "create" => {
                    fire_action(&engine, &entity_name, &parsed_action, &rule, &data).await;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Resolve an action's params against a produced row and dispatch the operation.
/// Fired for each `Created` row the reactive `query_and_watch` matview delivers.
/// Execution failures are logged loudly (fail-loud) but do not abort the
/// watcher — one bad row must not tear down the whole action.
async fn fire_action(
    engine: &BackendEngine,
    entity_name: &EntityName,
    parsed_action: &holon_api::action_dsl::ParsedAction,
    rule: &RuleId,
    data: &StorageEntity,
) {
    // Action params are all named, plain values (never render templates or
    // row-producing collections), so evaluate each directly. Routing through the
    // render-oriented `resolve_args_with` would divert any param whose name
    // collides with a render-template key (`parent_id`, `sortkey`, `action`, …
    // see `is_template_arg`) into an unevaluated `templates` bucket the op never
    // sees — which silently dropped `block.create`'s `parent_id` and broke
    // journal auto-create ("parent_id is required for block creation").
    let mut params: StorageEntity = parsed_action
        .params
        .iter()
        .filter_map(|arg| {
            let name = arg.name.as_ref()?;
            match eval_to_interp(&arg.value, data, &CORE_VALUE_FN_LOOKUP) {
                InterpValue::Value(v) => Some((name.clone().into(), v)),
                InterpValue::Rows(_) => None,
            }
        })
        .collect();

    // Deterministic effect id (ADR 0024 P4): a rule-fired create mints a
    // name-based UUIDv5 of (rule-id, firing-key, slot) so every replica firing
    // this rule for this row produces the SAME block id; the CRDT merge then
    // collapses concurrent creates into one node, and a re-fire (boot re-reg,
    // day rollover) upserts the same id (Ok in both providers — SqlOnly ON
    // CONFLICT DO UPDATE, Loro get-then-update). We supply it explicitly so the
    // provider's random-v4 id-less path never runs for rules. An author-supplied
    // literal id is already replica-stable, so it wins.
    if parsed_action.operation == "create" && !params.contains_key("id") {
        let key = FiringKey::from_row(data);
        let id = deterministic_block_id(rule, &key, &OutputSlot::first());
        params.insert("id".into(), Value::String(id.as_str().to_string()));
    }

    info!(
        "[action_watcher] executing {}.{} with params={params:?}",
        parsed_action.entity, parsed_action.operation
    );

    // Duplicate-id create is a convergent upsert (Ok) in both providers, so a
    // re-fire is idempotent, not an error. Any *other* execute failure is a real
    // fault — logged loudly (fail-loud); it must not tear down the watcher.
    if let Err(e) = engine
        .execute_operation(entity_name, &parsed_action.operation, params)
        .await
    {
        tracing::error!(
            "[action_watcher] execute_operation failed for action {}: {e:#}",
            rule.as_str()
        );
    }
}

fn extract_string(row: &StorageEntity, key: &str) -> Option<String> {
    match row.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        other => Some(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::di::test_helpers::create_test_engine_with_providers;
    use crate::storage::BLOCK_WRITE_TABLE;
    use std::time::Duration;

    const JOURNAL_ACTION: &str =
        "block.create(#{parent_id: \"block:journals\", name: col(\"name\")})";

    /// A test engine with the `block` SQL operation provider registered (writes
    /// to `block_raw`), mirroring the production wiring in `loro_module.rs`.
    async fn block_engine() -> Arc<BackendEngine> {
        create_test_engine_with_providers(":memory:".into(), |module| {
            module.with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                ))
            })
        })
        .await
        .unwrap()
    }

    async fn seed_journals_parent(engine: &BackendEngine) {
        // Route through the sanctioned block writer (SqlBlockOperations), not a
        // raw INSERT, so the FK parent exists for the rule-fired children.
        let mut parent = StorageEntity::new();
        parent.insert("id".into(), Value::String("block:journals".to_string()));
        parent.insert("content".into(), Value::String("Journals".to_string()));
        engine
            .execute_operation(&EntityName::new("block"), "create", parent)
            .await
            .unwrap();
    }

    async fn count_journal_children(engine: &BackendEngine) -> usize {
        engine
            .db_handle()
            .query(
                "SELECT id FROM block_raw WHERE parent_id = 'block:journals'",
                HashMap::new(),
            )
            .await
            .unwrap()
            .len()
    }

    fn name_row(day: &str) -> StorageEntity {
        let mut row = StorageEntity::new();
        row.insert("name".into(), Value::String(day.to_string()));
        row
    }

    /// WP2 core: firing the same create rule for the same day twice yields the
    /// same deterministic id, so the second create upserts the same row — no
    /// duplicate. A different day is a different firing key, so a new block.
    #[tokio::test(flavor = "multi_thread")]
    async fn fire_action_dedups_same_day_and_distinguishes_days() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;

        let parsed = parse_action_dsl(JOURNAL_ACTION).unwrap();
        let entity = EntityName::new(&parsed.entity);
        let rule = RuleId::new("journals::action::0");

        fire_action(&engine, &entity, &parsed, &rule, &name_row("2026-07-10")).await;
        fire_action(&engine, &entity, &parsed, &rule, &name_row("2026-07-10")).await;
        assert_eq!(
            count_journal_children(&engine).await,
            1,
            "same-day re-fire must converge to one journal block"
        );

        fire_action(&engine, &entity, &parsed, &rule, &name_row("2026-07-11")).await;
        assert_eq!(
            count_journal_children(&engine).await,
            2,
            "a new day must mint a distinct journal block"
        );
    }

    async fn wait_for_children(engine: &BackendEngine, expected: usize, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let n = count_journal_children(engine).await;
            if n == expected {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "expected {expected} journal children, still {n} after timeout"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The rollover extension end-to-end: the clock-backed journal rule fires on
    /// the initial `Created` row, then re-fires on the day-rollover `Updated`
    /// (rowid-stable UPDATE of the `clock` row) to create the next day's journal.
    /// The deterministic id keeps each day at exactly one block.
    #[tokio::test(flavor = "multi_thread")]
    async fn rollover_update_refires_journal_create() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;
        // Pin the clock `day` row to a known date (a boot-seeded row may already
        // exist), so the watcher's initial `Created` fires for a deterministic
        // day.
        engine
            .db_handle()
            .execute(
                "INSERT INTO clock (grain, today, epoch_day, updated_at) \
                 VALUES ('day', '2026-07-10', 20679, '2026-07-10T00:00:00Z') \
                 ON CONFLICT(grain) DO UPDATE SET today = excluded.today, \
                 epoch_day = excluded.epoch_day, updated_at = excluded.updated_at",
                vec![],
            )
            .await
            .unwrap();

        let watcher = tokio::spawn(run_pair_watcher(
            engine.clone(),
            "journals::action::0".to_string(),
            "SELECT today as name FROM clock WHERE grain = 'day'".to_string(),
            "holon_sql".to_string(),
            JOURNAL_ACTION.to_string(),
        ));

        // Initial Created row → day-1 journal.
        wait_for_children(&engine, 1, Duration::from_secs(15)).await;

        // Simulate day-rollover: a rowid-stable UPDATE emits CDC `Updated`, which
        // the create-effect rule now re-fires on.
        engine
            .db_handle()
            .execute(
                "UPDATE clock SET today = '2026-07-11', epoch_day = 20680, \
                 updated_at = '2026-07-11T00:00:00Z' WHERE grain = 'day'",
                vec![],
            )
            .await
            .unwrap();

        wait_for_children(&engine, 2, Duration::from_secs(15)).await;

        watcher.abort();
    }
}
