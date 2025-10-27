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
use holon_api::render_eval::{CORE_VALUE_FN_LOOKUP, resolve_args_with};
use holon_api::render_types::Arg;
use holon_api::{EntityName, Value};
use rhai::{Dynamic, Engine as RhaiEngine, Map as RhaiMap, Scope};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tracing::info;

use crate::api::backend_engine::BackendEngine;
use crate::render_dsl::{create_render_engine, dynamic_to_render_expr};
use crate::storage::types::StorageEntity;
use holon_api::streaming::Change;

const DISCOVERY_SQL: &str = include_str!("../../../../assets/queries/action_discovery.sql");

struct ParsedAction {
    entity: String,
    operation: String,
    params: Vec<Arg>,
}

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

    let mut row_stream = engine
        .query_and_watch(sql, HashMap::new(), None)
        .await
        .with_context(|| format!("Failed to subscribe to query for action {action_id}"))?;

    while let Some(batch) = row_stream.next().await {
        for item in batch.inner.items {
            if let Change::Created { data, .. } = item.change {
                let resolved =
                    resolve_args_with(&parsed_action.params, &data, &CORE_VALUE_FN_LOOKUP);

                let params: StorageEntity = resolved.named;

                info!(
                    "[action_watcher] executing {}.{} with params={params:?}",
                    parsed_action.entity, parsed_action.operation
                );

                if let Err(e) = engine
                    .execute_operation(&entity_name, &parsed_action.operation, params)
                    .await
                {
                    tracing::error!(
                        "[action_watcher] execute_operation failed for action {action_id}: {e:#}"
                    );
                }
            }
        }
    }

    Ok(())
}

fn parse_action_dsl(source: &str) -> Result<ParsedAction> {
    let trimmed = source.trim();

    let engine = build_action_engine();
    let mut scope = Scope::new();
    scope.push("block", EntityRef("block".to_string()));
    let result = engine
        .eval_expression_with_scope::<Dynamic>(&mut scope, trimmed)
        .map_err(|e| anyhow::anyhow!("Rhai eval failed for action DSL '{trimmed}': {e}"))?;

    let map = result
        .clone()
        .try_cast::<RhaiMap>()
        .ok_or_else(|| anyhow::anyhow!("Action DSL did not return a map, got: {result:?}"))?;

    let entity = map
        .get("_action_entity")
        .and_then(|v| v.clone().into_string().ok())
        .ok_or_else(|| anyhow::anyhow!("Action DSL result missing _action_entity"))?;
    let operation = map
        .get("_action_op")
        .and_then(|v| v.clone().into_string().ok())
        .ok_or_else(|| anyhow::anyhow!("Action DSL result missing _action_op"))?;

    let params_map = map
        .get("_action_params")
        .and_then(|v| v.clone().try_cast::<RhaiMap>())
        .unwrap_or_default();

    let mut params: Vec<Arg> = Vec::new();
    for (k, v) in &params_map {
        let expr = dynamic_to_render_expr(v)
            .with_context(|| format!("Failed to convert param '{k}' to RenderExpr"))?;
        params.push(Arg {
            name: Some(k.to_string()),
            value: expr,
        });
    }

    Ok(ParsedAction {
        entity,
        operation,
        params,
    })
}

fn build_action_engine() -> RhaiEngine {
    let mut engine = create_render_engine();

    engine.register_type_with_name::<EntityRef>("EntityRef");

    for op in &[
        "create",
        "set_field",
        "update",
        "delete",
        "cycle_task_state",
    ] {
        let op_str = op.to_string();
        engine.register_fn(
            *op,
            move |entity: &mut EntityRef, params: Dynamic| -> Dynamic {
                make_action_node(&entity.0, &op_str, params)
            },
        );
    }

    engine
}

fn make_action_node(entity: &str, operation: &str, params: Dynamic) -> Dynamic {
    let params_map = if params.is_map() {
        params.cast::<RhaiMap>()
    } else {
        RhaiMap::new()
    };

    let mut map = RhaiMap::new();
    map.insert("_action_entity".into(), Dynamic::from(entity.to_string()));
    map.insert("_action_op".into(), Dynamic::from(operation.to_string()));
    map.insert("_action_params".into(), Dynamic::from(params_map));
    Dynamic::from(map)
}

fn extract_string(row: &StorageEntity, key: &str) -> Option<String> {
    match row.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        other => Some(format!("{other:?}")),
    }
}

// Rhai custom type for dot-notation: block.create(#{...})
#[derive(Clone, Debug)]
struct EntityRef(String);

impl std::fmt::Display for EntityRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityRef({})", self.0)
    }
}
