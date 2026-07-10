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
            if let Change::Created { data, .. } = item.change {
                fire_action(&engine, &entity_name, &parsed_action, &action_id, &data).await;
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
    action_id: &str,
    data: &StorageEntity,
) {
    // Action params are all named, plain values (never render templates or
    // row-producing collections), so evaluate each directly. Routing through the
    // render-oriented `resolve_args_with` would divert any param whose name
    // collides with a render-template key (`parent_id`, `sortkey`, `action`, …
    // see `is_template_arg`) into an unevaluated `templates` bucket the op never
    // sees — which silently dropped `block.create`'s `parent_id` and broke
    // journal auto-create ("parent_id is required for block creation").
    let params: StorageEntity = parsed_action
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

    info!(
        "[action_watcher] executing {}.{} with params={params:?}",
        parsed_action.entity, parsed_action.operation
    );

    if let Err(e) = engine
        .execute_operation(entity_name, &parsed_action.operation, params)
        .await
    {
        tracing::error!("[action_watcher] execute_operation failed for action {action_id}: {e:#}");
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
