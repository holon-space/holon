use std::sync::Arc;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::reactive::ReactiveStreamExt;
use holon_api::render_requirements::RenderRequirements;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_api::streaming::ActorAbortGuard;
use holon_api::streaming::BatchMapChange;
use holon_api::streaming::BatchMapChangeWithMetadata;
use holon_api::streaming::Change;
use holon_api::streaming::UiEvent;
use holon_api::streaming::WatchHandle;
use holon_api::streaming::WatcherCommand;
use holon_api::widget_spec::EnrichedRow;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use super::backend_engine::BackendEngine;
use crate::entity_profile::ProfileResolving;
use crate::storage::turso::RowChangeStream;

/// Internal trigger that causes a re-render.
///
/// Merging structural CDC, variant commands, and profile changes into a single
/// stream lets us use `switch_map` to manage the data forwarder lifecycle
/// automatically — no manual `AbortHandle`, no `Mutex<Option<AbortHandle>>`,
/// no generation drift race conditions.
enum RenderTrigger {
    /// Block or children changed in the database — increment generation.
    StructuralChange,
    /// User requested a different entity profile variant — same generation,
    /// but the data forwarder restarts with the updated profile context
    /// (fixing the stale-context bug in the previous implementation).
    VariantChange(String),
    /// Entity profile blocks changed — re-render with new profiles without
    /// incrementing generation (data matview is unchanged, only rendering
    /// and computed fields may differ).
    ProfileChange,
    /// Initial render on startup — treated like a structural change.
    Initial,
}

/// Start watching a block's UI, returning a stream of UiEvents and a command
/// channel.
///
/// Creates a structural matview that detects when the block or its children
/// change, then re-renders via `BlockDomain::render_entity()`. The output
/// stream carries both structural updates (new WidgetSpec) and data deltas (CDC
/// batches).
#[tracing::instrument(skip(engine), fields(block_id = %block_id, is_root))]
pub async fn watch_ui(engine: Arc<BackendEngine>, block_id: EntityUri) -> Result<WatchHandle> {
    let structural_sql = format!(
        "SELECT * FROM {table} WHERE id = '{id}' OR parent_id = '{id}'",
        table = crate::storage::BLOCK_READ_TABLE,
        id = block_id,
    );

    let struct_stream = engine.subscribe_sql(&structural_sql).await?;

    let (output_tx, output_rx) = mpsc::channel(64);
    let (command_tx, command_rx) = mpsc::channel(16);

    // Track every spawned actor so dropping the WatchHandle aborts the whole
    // pipeline immediately. Without this the merge_triggers struct/profile
    // actors stay parked on engine-scoped streams until the engine itself
    // drops, leaking work onto a shared runtime — the historical source of
    // PBT non-determinism with `general_e2e_pbt`.
    let mut aborts = ActorAbortGuard::new();

    // Merge structural CDC + commands into a single trigger stream.
    let profile_resolver = engine.profile_resolver().clone();
    let profile_signal = profile_resolver.profile_signal();
    let trigger_stream = merge_triggers(struct_stream, command_rx, profile_signal, &mut aborts);

    let watcher_handle = tokio::spawn(run_reactive_watcher(
        trigger_stream,
        engine,
        profile_resolver,
        block_id,
        output_tx,
    ));
    aborts.push(watcher_handle.abort_handle());

    Ok(WatchHandle::with_aborts(output_rx, command_tx, aborts))
}

pub use holon_api::ui_watcher::UiWatcher;

#[async_trait::async_trait]
impl UiWatcher for BackendEngine {
    async fn watch_ui(self: Arc<Self>, block_id: EntityUri) -> Result<WatchHandle> {
        // Call the free function (path resolution → not the trait method).
        self::watch_ui(self, block_id).await
    }
}

/// Merge structural CDC events, WatcherCommands, and profile cache changes
/// into a single `RenderTrigger` stream, prepended with an `Initial` trigger.
///
/// All four spawned actors register their abort handles with `aborts` so that
/// dropping the owning `WatchHandle` cancels them immediately rather than
/// waiting for their (possibly long-lived) source streams to terminate.
fn merge_triggers(
    struct_stream: RowChangeStream,
    command_rx: mpsc::Receiver<WatcherCommand>,
    profile_signal: futures_signals::signal::Mutable<Arc<crate::entity_profile::ProfileCache>>,
    aborts: &mut ActorAbortGuard,
) -> ReceiverStream<RenderTrigger> {
    let (tx, rx) = mpsc::channel(64);

    // Initial trigger
    let tx_init = tx.clone();
    let init_handle = tokio::spawn(async move {
        let _ = tx_init.send(RenderTrigger::Initial).await;
    });
    aborts.push(init_handle.abort_handle());

    // Structural CDC → RenderTrigger::StructuralChange
    let tx_struct = tx.clone();
    let struct_handle = tokio::spawn(async move {
        tokio::pin!(struct_stream);
        while let Some(_batch) = struct_stream.next().await {
            if tx_struct
                .send(RenderTrigger::StructuralChange)
                .await
                .is_err()
            {
                break;
            }
        }
    });
    aborts.push(struct_handle.abort_handle());

    // Commands → RenderTrigger::VariantChange
    let tx_cmd = tx.clone();
    let cmd_handle = tokio::spawn(async move {
        let mut command_rx = command_rx;
        while let Some(cmd) = command_rx.recv().await {
            let trigger = match cmd {
                WatcherCommand::SetVariant(v) => RenderTrigger::VariantChange(v),
            };
            if tx_cmd.send(trigger).await.is_err() {
                break;
            }
        }
    });
    aborts.push(cmd_handle.abort_handle());

    // Profile cache changes → RenderTrigger::ProfileChange
    let tx_profile = tx;
    let profile_handle = tokio::spawn(async move {
        use futures_signals::signal::SignalExt;
        // signal_cloned fires the initial value first; skip it so we only
        // react to subsequent profile rebuilds.
        let mut stream = profile_signal.signal_cloned().to_stream();
        let _initial = futures::StreamExt::next(&mut stream).await;
        while futures::StreamExt::next(&mut stream).await.is_some() {
            if tx_profile.send(RenderTrigger::ProfileChange).await.is_err() {
                break;
            }
        }
    });
    aborts.push(profile_handle.abort_handle());

    ReceiverStream::new(rx)
}

/// The reactive watcher core. Uses `switch_map` to automatically manage the
/// data forwarder lifecycle: when any trigger arrives, the previous inner
/// stream (data forwarder) is aborted and a new one is spawned.
async fn run_reactive_watcher(
    trigger_stream: ReceiverStream<RenderTrigger>,
    engine: Arc<BackendEngine>,
    profile_resolver: Arc<dyn ProfileResolving>,
    block_id: EntityUri,
    output_tx: mpsc::Sender<UiEvent>,
) {
    let mut generation: u64 = 0;
    let mut variant = None;

    // switch_map: each trigger produces an inner stream of UiEvents.
    // When a new trigger arrives, the previous inner stream is aborted.
    let mut ui_events = trigger_stream.switch_map(move |trigger| {
        match &trigger {
            RenderTrigger::Initial => {
                generation += 1;
                tracing::info!("[UiWatcher] Initial render for block '{block_id}'");
            }
            RenderTrigger::StructuralChange => {
                generation += 1;
                tracing::info!("[UiWatcher] Structural CDC for block '{block_id}' — re-rendering");
            }
            RenderTrigger::VariantChange(v) => {
                variant = Some(v.clone());
                tracing::info!("[UiWatcher] SetVariant('{v}') for block '{block_id}'");
            }
            RenderTrigger::ProfileChange => {
                tracing::info!("[UiWatcher] Profile changed — re-rendering block '{block_id}'");
            }
        }

        let current_gen = generation;
        let current_var = variant.clone();
        let engine = engine.clone();
        let resolver = profile_resolver.clone();

        // Inner stream: Structure event followed by Data events.
        // Spawned as a task so render_entity can be awaited.
        let (inner_tx, inner_rx) = mpsc::channel::<UiEvent>(64);
        let block_id_clone = block_id.clone();
        crate::util::spawn_actor(async move {
            render_and_forward(
                inner_tx,
                engine,
                resolver,
                &block_id_clone,
                &current_var,
                current_gen,
            )
            .await;
        });
        ReceiverStream::new(inner_rx)
    });

    // Forward switch_map output to the WatchHandle's output channel
    while let Some(event) = ui_events.next().await {
        if output_tx.send(event).await.is_err() {
            tracing::debug!("[UiWatcher] Output channel closed, stopping");
            break;
        }
    }
}

/// Render a block and forward the result + data stream into the inner channel.
///
/// On success: emits Structure event, then forwards data CDC as Data events.
/// On failure: emits Structure event with error WidgetSpec (stream stays open).
async fn render_and_forward(
    tx: mpsc::Sender<UiEvent>,
    engine: Arc<BackendEngine>,
    profile_resolver: Arc<dyn ProfileResolving>,
    block_id: &EntityUri,
    variant: &Option<String>,
    generation: u64,
) {
    // Advice-rule status surface (ADR 0022 "rule blocks render their own status"):
    // a rule block whose id carries a NON-Active status has its render replaced
    // by the error surface, so parse errors and async DDL failures are visible
    // in place (v1 replaces, like the existing render-failure path; over-cap
    // banner comes later).
    if let Some(status) = engine.advice_status().get(block_id.as_str())
        && !status.is_active()
    {
        let _ = tx
            .send(UiEvent::Structure {
                render_expr: error_render_expr(&format!("advice rule: {status}")),
                candidates: Vec::new(),
                generation,
            })
            .await;
        return;
    }

    match engine.blocks().render_entity(block_id, variant).await {
        Ok((render_expr, data_stream)) => {
            tracing::info!(
                "[UiWatcher] render_entity('{}') OK: gen={}, render={:?}",
                block_id,
                generation,
                match &render_expr {
                    RenderExpr::FunctionCall { name, .. } => name.as_str(),
                    _ => "non-function",
                },
            );

            if tx
                .send(UiEvent::Structure {
                    render_expr,
                    candidates: Vec::new(),
                    generation,
                })
                .await
                .is_err()
            {
                return;
            }

            forward_data_stream(data_stream, tx, profile_resolver, generation).await;
        }
        Err(e) => {
            let verdict = missing_table_verdict(engine.integration_attribution(), &e);
            if let Some(verdict) = verdict.as_ref().filter(|v| v.is_fully_explained()) {
                // WARN, not ERROR: every one of these tables belongs to an
                // integration whose failure was already disclosed at boot with
                // its real cause. Raising it again per block is the flood, not
                // the signal.
                tracing::warn!(
                    "[UiWatcher] render_entity('{}') reads tables of integrations that are not \
                     connected: {}",
                    block_id,
                    verdict.disclosures().join(" · ")
                );
                let _ = tx
                    .send(UiEvent::Structure {
                        render_expr: disclosed_render_expr(verdict),
                        candidates: Vec::new(),
                        generation,
                    })
                    .await;
                return;
            }

            // Anything the inert half does not fully explain stays loud, and
            // carries BOTH the unexplained owners and any inert integrations
            // that co-occurred — dropping either would hide the half the user
            // can act on.
            let notes = verdict.map(|v| v.notes()).unwrap_or_default();
            let attribution = if notes.is_empty() {
                String::new()
            } else {
                format!(" — {}", notes.join("; "))
            };
            // ERROR, not WARN: WARN is the disclosed-degradation tier and is
            // deliberately not read by `inv-no-observed-errors`. A block that
            // failed to render is a real failure, so it must reach the
            // error-capture oracles. The error widget below stays: it is the
            // visible, disclosed surface for the same failure.
            //
            // `{:#}`, not `{}`: the matview error chains its cause rather than
            // spelling it in, and plain Display prints only the outermost
            // context — which would drop the missing table names entirely.
            tracing::error!(
                "[UiWatcher] render_entity('{}') failed: {:#}{}",
                block_id,
                e,
                attribution
            );
            let _ = tx
                .send(UiEvent::Structure {
                    render_expr: error_render_expr(&format!("{e:#}{attribution}")),
                    candidates: Vec::new(),
                    generation,
                })
                .await;
        }
    }
}

/// Forward a data CDC stream as UiEvent::Data events.
async fn forward_data_stream(
    mut stream: RowChangeStream,
    output_tx: mpsc::Sender<UiEvent>,
    profile_resolver: Arc<dyn ProfileResolving>,
    generation: u64,
) {
    while let Some(batch_with_metadata) = stream.next().await {
        tracing::info!(
            "[UiWatcher] forward_data_stream: received batch with {} items for gen={}",
            batch_with_metadata.inner.items.len(),
            generation
        );
        let metadata = batch_with_metadata.metadata.clone();
        // `watch_ui` renders each row through its own entity profile
        // (`BlockDomain::render_entity`), so the profile answers for itself.
        let enriched = enrich_batch(
            batch_with_metadata.inner.items,
            &profile_resolver,
            &RenderRequirements::entity_dispatch(),
        );
        // Convert Change<EnrichedRow> → Change<DataRow> at the UiEvent boundary.
        // UiEvent::Data uses MapChange (= Change<DataRow>) — the FFI boundary requires
        // this shape.
        let map_changes: Vec<holon_api::MapChange> = enriched
            .into_iter()
            .map(|c| c.map(EnrichedRow::into_inner))
            .collect();

        let batch = BatchMapChangeWithMetadata {
            inner: BatchMapChange { items: map_changes },
            metadata,
        };

        if output_tx
            .send(UiEvent::Data { batch, generation })
            .await
            .is_err()
        {
            tracing::debug!("[UiWatcher] Output channel closed, stopping data forwarder");
            break;
        }
    }
}

/// Convert a batch of RowChange items to enriched changes.
///
/// Returns `Change<EnrichedRow>` — the type-safe proof that enrichment
/// happened. Used by `enrich_stream` and `forward_data_stream`.
pub fn enrich_batch(
    items: Vec<crate::storage::turso::RowChange>,
    profile_resolver: &Arc<dyn ProfileResolving>,
    renderer: &RenderRequirements,
) -> Vec<Change<EnrichedRow>> {
    items
        .into_iter()
        .map(|row_change| match row_change.change {
            Change::Created { data, origin } => {
                let data = enrich_row(data, profile_resolver, renderer);
                Change::Created { data, origin }
            }
            Change::Updated { id, data, origin } => {
                let data = enrich_row(data, profile_resolver, renderer);
                Change::Updated { id, data, origin }
            }
            Change::Deleted { id, origin } => Change::Deleted { id, origin },
            Change::FieldsChanged {
                entity_id,
                fields,
                origin,
            } => Change::FieldsChanged {
                entity_id,
                fields,
                origin,
            },
        })
        .collect()
}

/// Enrich a raw storage row into an `EnrichedRow`.
///
/// Delegates to `EnrichedRow::from_storage` — the enrichment constructor.
pub fn enrich_row(
    data: holon_api::StorageEntity,
    resolver: &Arc<dyn ProfileResolving>,
    renderer: &RenderRequirements,
) -> EnrichedRow {
    let resolver = resolver.clone();
    let renderer = renderer.clone();
    EnrichedRow::from_storage(data, |row| {
        // Computed fields only — do NOT resolve the render profile here. The
        // profile was discarded anyway, and resolving it evaluated UI-bearing
        // variant conditions against this raw storage row (no UI-state bindings),
        // emitting spurious `eval_bool_source` errors. See
        // `ProfileResolving::resolve_computed_only`.
        resolver.resolve_computed_only(row, &renderer)
    })
}

use holon_api::EnrichedChangeStream;

/// Wrap a raw `RowChangeStream` with enrichment (flatten_properties + computed
/// fields).
///
/// Spawns a forwarding task that enriches each batch before delivering it.
/// Returns an `EnrichedChangeStream` carrying `Change<EnrichedRow>` — the type
/// proves enrichment happened.  No trust boundaries needed downstream.
///
/// This is the canonical enrichment boundary — call this once at the point
/// where raw storage data enters the frontend.
pub fn enrich_stream(
    raw: RowChangeStream,
    profile_resolver: Arc<dyn ProfileResolving>,
    renderer: RenderRequirements,
) -> EnrichedChangeStream {
    use holon_api::streaming::Batch;
    use holon_api::streaming::WithMetadata;
    use tokio::sync::mpsc;
    use tokio_stream::StreamExt;

    let (tx, rx) = mpsc::channel(64);
    crate::util::spawn_actor(async move {
        tokio::pin!(raw);
        while let Some(batch) = raw.next().await {
            let enriched = enrich_batch(batch.inner.items, &profile_resolver, &renderer);
            let enriched_batch = WithMetadata {
                inner: Batch { items: enriched },
                metadata: batch.metadata,
            };
            if tx.send(enriched_batch).await.is_err() {
                break;
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

// Tests for the old flatten_properties function were removed — the behavior
// is now part of EnrichedRow::from_raw and tested at that level.

/// How a render failure's missing tables split across integration ownership,
/// or `None` when the failure names no tables at all.
fn missing_table_verdict(
    attribution: &holon_core::integration_attribution::IntegrationAttribution,
    error: &anyhow::Error,
) -> Option<holon_core::integration_attribution::MissingTableVerdict> {
    let holon_core::storage::types::StorageError::MissingDependencies { missing, .. } =
        error.downcast_ref::<holon_core::storage::types::StorageError>()?
    else {
        return None;
    };
    Some(attribution.classify_missing(missing.iter().map(String::as_str)))
}

/// The disclosure for a failure fully explained by integrations that are not
/// running.
///
/// Deliberately the `error` widget, not a new widget kind: every frontend
/// already renders `error`, and a kind none of them knows falls through to
/// `ViewKind::Empty` — which paints NOTHING on dioxus-web and is invisible to
/// every headless oracle. The `degraded_disclosure` prop follows
/// `annotate_degraded`'s convention so a renderer can style it calmly, and the
/// message carries the same sentence for the ones that do not.
fn disclosed_render_expr(
    verdict: &holon_core::integration_attribution::MissingTableVerdict,
) -> holon_api::RenderExpr {
    let disclosure = verdict.disclosures().join(" · ");
    let integrations = verdict
        .inert
        .iter()
        .map(|o| o.integration.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    RenderExpr::FunctionCall {
        name: "error".to_string(),
        args: vec![
            string_arg("message", &disclosure),
            string_arg("degraded_disclosure", &disclosure),
            string_arg("integration", &integrations),
        ],
    }
}

fn string_arg(name: &str, value: &str) -> Arg {
    Arg {
        name: Some(name.to_string()),
        value: RenderExpr::Literal {
            value: holon_api::Value::String(value.to_string()),
        },
    }
}

/// Create an error RenderExpr for render failures.
fn error_render_expr(message: &str) -> holon_api::RenderExpr {
    RenderExpr::FunctionCall {
        name: "error".to_string(),
        args: vec![Arg {
            name: Some("message".to_string()),
            value: RenderExpr::Literal {
                value: holon_api::Value::String(message.to_string()),
            },
        }],
    }
}

#[cfg(test)]
mod tests {
    use holon_core::integration_attribution::IntegrationAttribution;
    use holon_core::integration_attribution::IntegrationStatus;
    use holon_core::integration_attribution::TableOwner;
    use holon_core::storage::types::StorageError;

    use super::*;

    fn owner(integration: &str, display: &str, status: IntegrationStatus) -> TableOwner {
        TableOwner {
            integration: integration.to_string(),
            display_name: display.to_string(),
            status,
            cause: "binary 'claude-history-mcp' not found on PATH".to_string(),
        }
    }

    fn claude_history(status: IntegrationStatus) -> IntegrationAttribution {
        let attribution = IntegrationAttribution::new();
        for table in ["cc_session", "cc_message"] {
            attribution.declare(table, owner("claude-history", "Claude History", status));
        }
        attribution
    }

    /// The real shape: `MatviewManager::ensure_view` wraps the actor's typed
    /// error in a context layer before the watcher ever sees it.
    fn missing_tables(tables: &[&str]) -> anyhow::Error {
        anyhow::Error::new(StorageError::MissingDependencies {
            sql_preview: "CREATE MATERIALIZED VIEW watch_view_2ed15df4 AS SELECT …".to_string(),
            missing: tables.iter().map(|t| t.to_string()).collect(),
        })
        .context("Failed to create materialized view watch_view_2ed15df4")
    }

    fn arg_str(expr: &RenderExpr, name: &str) -> Option<String> {
        let RenderExpr::FunctionCall { args, .. } = expr else {
            panic!("expected a function call, got {expr:?}");
        };
        let arg = args.iter().find(|a| a.name.as_deref() == Some(name))?;
        let RenderExpr::Literal {
            value: holon_api::Value::String(s),
        } = &arg.value
        else {
            panic!("`{name}` is not a string literal in {expr:?}");
        };
        Some(s.clone())
    }

    fn widget_name(expr: &RenderExpr) -> &str {
        let RenderExpr::FunctionCall { name, .. } = expr else {
            panic!("expected a function call");
        };
        name
    }

    #[test]
    fn a_view_over_a_dead_integrations_tables_discloses_it_on_a_node_every_frontend_renders() {
        let attribution = claude_history(IntegrationStatus::Unavailable);
        let verdict =
            missing_table_verdict(&attribution, &missing_tables(&["cc_session", "cc_message"]))
                .expect("a MissingDependencies failure must be classified");
        assert!(verdict.is_fully_explained());

        let expr = disclosed_render_expr(&verdict);

        // `error`, not a bespoke kind: an unknown widget name falls through to
        // ViewKind::Empty, which paints nothing on dioxus-web and is invisible
        // to every headless oracle.
        assert_eq!(widget_name(&expr), "error");
        assert_eq!(
            arg_str(&expr, "integration").as_deref(),
            Some("claude-history")
        );
        let expected = "Claude History is not connected — status: Unavailable (binary \
                        'claude-history-mcp' not found on PATH)";
        assert_eq!(arg_str(&expr, "message").as_deref(), Some(expected));
        assert_eq!(
            arg_str(&expr, "degraded_disclosure").as_deref(),
            Some(expected),
            "the disclosure prop is what lets a renderer style this calmly"
        );
    }

    /// The masking bug: a dead integration sharing the failure with a genuine
    /// one must not turn the genuine one into a calm banner.
    #[test]
    fn a_mixed_failure_stays_loud_and_names_every_owner() {
        let attribution = claude_history(IntegrationStatus::Unavailable);
        attribution.declare(
            "td_task",
            owner("todoist", "Todoist", IntegrationStatus::Connected),
        );

        let error = missing_tables(&["cc_session", "td_task", "blocks"]);
        let verdict = missing_table_verdict(&attribution, &error).expect("classified");

        assert!(
            !verdict.is_fully_explained(),
            "a connected owner and an unowned table must keep this loud"
        );
        assert_eq!(
            verdict.notes(),
            vec![
                "td_task belongs to integration 'todoist' (status: Connected)".to_string(),
                "blocks belongs to no integration".to_string(),
                "Claude History is not connected — status: Unavailable (binary \
                 'claude-history-mcp' not found on PATH)"
                    .to_string(),
            ],
            "the loud error must name the connected owner, the unowned table, AND still carry \
             the inert disclosure"
        );
    }

    #[test]
    fn a_failure_that_names_no_tables_is_left_alone() {
        let attribution = claude_history(IntegrationStatus::Unavailable);
        let unrelated = anyhow::anyhow!("block 'x' has no query source child");
        assert!(missing_table_verdict(&attribution, &unrelated).is_none());
    }

    /// The regression the chained-source fix introduced: plain `Display` on an
    /// anyhow chain prints only the outermost context, dropping the missing
    /// table names that are the whole point of the message.
    #[test]
    fn the_alternate_format_is_required_to_keep_the_cause() {
        let error = missing_tables(&["cc_session"]);
        assert!(
            !format!("{error}").contains("cc_session"),
            "plain Display drops the chained cause — the log MUST use {{:#}}"
        );
        assert!(format!("{error:#}").contains("cc_session"));
    }
}
