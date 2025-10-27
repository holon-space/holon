use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::sync::{Mutex, mpsc};
use tokio::task::AbortHandle;
use tokio_stream::StreamExt;

use holon_api::Value;
use holon_api::render_types::{Arg, RenderExpr, RowProfile};
use holon_api::streaming::{
    BatchMapChange, BatchMapChangeWithMetadata, Change, UiEvent, WatchHandle, WatcherCommand,
};
use holon_api::widget_spec::ResolvedRow;

use super::backend_engine::BackendEngine;
use crate::entity_profile::{ProfileContext, ProfileResolving};
use crate::storage::turso::RowChangeStream;

/// Manages a long-lived UI stream for a single block.
///
/// Watches for structural changes (block edits, query source changes) via a
/// structural matview CDC, re-renders on each change, and hot-swaps the data
/// CDC forwarder. Errors become `Structure` events with error WidgetSpecs
/// instead of killing the stream.
struct UiWatcher {
    block_id: String,
    is_root: bool,
    preferred_variant: Mutex<Option<String>>,
    engine: Arc<BackendEngine>,
    profile_resolver: Arc<dyn ProfileResolving>,
    output_tx: mpsc::Sender<UiEvent>,
    generation: AtomicU64,
    data_forwarder_abort: Mutex<Option<AbortHandle>>,
}

impl UiWatcher {
    /// Main run loop. Selects between structural CDC events and commands.
    async fn run(
        self: Arc<Self>,
        mut struct_stream: RowChangeStream,
        mut command_rx: mpsc::Receiver<WatcherCommand>,
    ) {
        tracing::info!(
            "[UiWatcher] Starting initial render for block '{}'",
            self.block_id
        );
        self.do_render(true).await;

        loop {
            tokio::select! {
                maybe_batch = struct_stream.next() => {
                    match maybe_batch {
                        Some(_) => {
                            tracing::info!("[UiWatcher] Structural CDC for block '{}' — re-rendering", self.block_id);
                            self.do_render(true).await;
                        }
                        None => {
                            tracing::warn!("[UiWatcher] Structural CDC stream ended for block '{}'", self.block_id);
                            break;
                        }
                    }
                }
                maybe_cmd = command_rx.recv() => {
                    match maybe_cmd {
                        Some(WatcherCommand::SetVariant(variant)) => {
                            tracing::info!("[UiWatcher] SetVariant('{}') for block '{}'", variant, self.block_id);
                            *self.preferred_variant.lock().await = Some(variant);
                            self.do_render(false).await;
                        }
                        None => {
                            tracing::error!(
                                "[UiWatcher] Command channel closed unexpectedly for block '{}' — \
                                 the command sender was dropped by the frontend. This kills structural \
                                 CDC and prevents re-renders.",
                                self.block_id
                            );
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Re-render the block. On structural changes, increments generation and
    /// hot-swaps the data forwarder. On variant switches, only emits a new
    /// Structure event without touching the data forwarder.
    #[tracing::instrument(skip(self), fields(block_id = %self.block_id, increment_generation))]
    async fn do_render(&self, increment_generation: bool) {
        if increment_generation {
            // Cancel old data forwarder
            if let Some(handle) = self.data_forwarder_abort.lock().await.take() {
                handle.abort();
            }
        }

        let generation = if increment_generation {
            self.generation.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.generation.load(Ordering::SeqCst)
        };

        let variant = self.preferred_variant.lock().await.clone();

        match self
            .engine
            .blocks()
            .render_block(&self.block_id, variant, self.is_root)
            .await
        {
            Ok((widget_spec, data_stream)) => {
                tracing::info!(
                    "[UiWatcher] render_block('{}') OK: gen={}, rows={}, render={:?}",
                    self.block_id,
                    generation,
                    widget_spec.data.len(),
                    match &widget_spec.render_expr {
                        holon_api::render_types::RenderExpr::FunctionCall { name, .. } =>
                            name.as_str(),
                        _ => "non-function",
                    },
                );
                // Emit Structure event
                let _ = self
                    .output_tx
                    .send(UiEvent::Structure {
                        widget_spec: widget_spec.clone(),
                        generation,
                    })
                    .await;

                if increment_generation {
                    self.spawn_data_forwarder(data_stream, generation).await;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[UiWatcher] render_block('{}') failed: {}",
                    self.block_id,
                    e
                );
                let error_spec = error_widget_spec(&format!("{e}"));
                let _ = self
                    .output_tx
                    .send(UiEvent::Structure {
                        widget_spec: error_spec,
                        generation,
                    })
                    .await;
                // No data forwarder on error — stream stays open for next structural event
            }
        }
    }

    /// Spawn a background task that converts RowChange CDC events to
    /// UiEvent::Data and sends them to the output channel.
    async fn spawn_data_forwarder(&self, stream: RowChangeStream, generation: u64) {
        let output_tx = self.output_tx.clone();
        let profile_resolver = self.profile_resolver.clone();
        let profile_ctx = ProfileContext {
            preferred_variant: self.preferred_variant.lock().await.clone(),
            view_width: None,
        };

        let handle = tokio::spawn(async move {
            forward_data_stream(stream, output_tx, profile_resolver, profile_ctx, generation).await;
        });

        *self.data_forwarder_abort.lock().await = Some(handle.abort_handle());
    }
}

/// Forward a data CDC stream as UiEvent::Data events.
async fn forward_data_stream(
    mut stream: RowChangeStream,
    output_tx: mpsc::Sender<UiEvent>,
    profile_resolver: Arc<dyn ProfileResolving>,
    profile_ctx: ProfileContext,
    generation: u64,
) {
    while let Some(batch_with_metadata) = stream.next().await {
        let metadata = batch_with_metadata.metadata.clone();
        let map_changes = resolve_batch_profiles(
            batch_with_metadata.inner.items,
            &profile_resolver,
            &profile_ctx,
        );

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

/// Convert a batch of RowChange items to MapChange items, resolving entity profiles.
///
/// This is the shared profile resolution logic used by both UiWatcher and
/// the legacy spawn_stream_forwarder in FFI.
pub fn resolve_batch_profiles(
    items: Vec<crate::storage::turso::RowChange>,
    profile_resolver: &Arc<dyn ProfileResolving>,
    profile_ctx: &ProfileContext,
) -> Vec<holon_api::MapChange> {
    items
        .into_iter()
        .map(|row_change| match row_change.change {
            Change::Created { data, origin } => {
                let profile = resolve_profile_for_row(profile_resolver, &data, profile_ctx);
                let data = flatten_properties(data);
                Change::Created {
                    data: ResolvedRow { data, profile },
                    origin,
                }
            }
            Change::Updated { id, data, origin } => {
                let profile = resolve_profile_for_row(profile_resolver, &data, profile_ctx);
                let data = flatten_properties(data);
                Change::Updated {
                    id,
                    data: ResolvedRow { data, profile },
                    origin,
                }
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

/// Resolve a single row's EntityProfile, returning None for fallback profiles.
fn resolve_profile_for_row(
    resolver: &Arc<dyn ProfileResolving>,
    data: &HashMap<String, Value>,
    ctx: &ProfileContext,
) -> Option<RowProfile> {
    let profile = ProfileResolving::resolve(resolver.as_ref(), data, ctx);
    if profile.name == "fallback" {
        return None;
    }
    Some(RowProfile {
        name: profile.name.clone(),
        render: profile.render.clone(),
        operations: profile.operations.clone(),
    })
}

/// Promote fields from the `properties` JSON object to top-level row keys.
/// This makes org-mode properties (task_state, priority, tags, etc.) accessible
/// to render widgets like `state_toggle(task_state)` via `context.getColumn()`.
fn flatten_properties(mut data: HashMap<String, Value>) -> HashMap<String, Value> {
    if let Some(Value::Object(props)) = data.get("properties") {
        for (key, value) in props.clone() {
            if !data.contains_key(&key) {
                data.insert(key, value);
            }
        }
    }
    data
}

/// Create an error WidgetSpec for render failures.
///
/// Uses `RenderExpr::FunctionCall { name: "error", args: [message] }` so the
/// frontend can render an inline error message.
fn error_widget_spec(message: &str) -> holon_api::WidgetSpec {
    holon_api::WidgetSpec {
        render_expr: RenderExpr::FunctionCall {
            name: "error".to_string(),
            args: vec![Arg {
                name: Some("message".to_string()),
                value: RenderExpr::Literal {
                    value: holon_api::Value::String(message.to_string()),
                },
            }],
            operations: Vec::new(),
        },
        data: Vec::new(),
        actions: Vec::new(),
    }
}

/// Start watching a block's UI, returning a stream of UiEvents and a command channel.
///
/// Creates a structural matview that detects when the block or its children change,
/// then re-renders via `BlockDomain::render_block()`. The output stream carries both
/// structural updates (new WidgetSpec) and data deltas (CDC batches).
#[tracing::instrument(skip(engine), fields(block_id = %block_id, is_root))]
pub async fn watch_ui(
    engine: Arc<BackendEngine>,
    block_id: String,
    preferred_variant: Option<String>,
    is_root: bool,
) -> Result<WatchHandle> {
    // Create structural matview: watches the block itself and its direct children
    // (which include query source and render source blocks).
    let structural_sql = format!(
        "SELECT id, content, content_type, source_language, parent_id \
         FROM block WHERE id = '{}' OR parent_id = '{}'",
        block_id, block_id
    );

    let view_name = engine
        .matview_manager()
        .ensure_view(&structural_sql)
        .await?;
    let struct_stream = engine.matview_manager().subscribe_cdc(&view_name);

    let (output_tx, output_rx) = mpsc::channel(64);
    let (command_tx, command_rx) = mpsc::channel(16);

    let watcher = Arc::new(UiWatcher {
        block_id,
        is_root,
        preferred_variant: Mutex::new(preferred_variant),
        engine: engine.clone(),
        profile_resolver: engine.profile_resolver().clone(),
        output_tx,
        generation: AtomicU64::new(0),
        data_forwarder_abort: Mutex::new(None),
    });

    tokio::spawn(async move {
        watcher.run(struct_stream, command_rx).await;
    });

    Ok(WatchHandle::new(output_rx, command_tx))
}
