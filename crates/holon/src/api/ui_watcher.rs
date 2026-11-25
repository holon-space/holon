use std::sync::Arc;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::live_data::DeliveryCursor;
use holon_api::live_data::OwedChild;
use holon_api::live_data::reporting_cursor;
use holon_api::live_data::reporting_to;
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
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use super::backend_engine::BackendEngine;
use super::layout_seed::LayoutSeedPhase;
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
    /// The block became, or stopped being, a region's focus root, which
    /// decides whether it renders as its subtree or as a leaf — increment
    /// generation.
    FocusRootChange,
    /// The boot seed finished writing the default layout, which decides
    /// whether a perspective without displayable panels is pending or an
    /// error — increment generation.
    LayoutSeedSettled,
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

/// The delivery cursors of one [`watch_ui`]. `data` is the consumer's cursor
/// from the enclosing [`reporting_to`]. The trigger subscriptions report to
/// `triggers` and each render's data subscription to its own cursor, all owed
/// children of `data`: a superseded render's batches stop counting once it
/// ends, since its events are never applied.
#[derive(Clone)]
struct Delivery {
    data: Arc<DeliveryCursor>,
    triggers: Arc<OwedChild>,
    reported: bool,
}

impl Delivery {
    fn enclosing() -> Self {
        let reporting = reporting_cursor();
        let reported = reporting.is_some();
        let data = reporting.unwrap_or_default();
        let triggers = Arc::new(data.owed_child());
        Self {
            data,
            triggers,
            reported,
        }
    }

    async fn subscribe<F: std::future::Future>(
        &self,
        cursor: &Arc<DeliveryCursor>,
        subscribe: F,
    ) -> F::Output {
        if self.reported {
            reporting_to(cursor.clone(), subscribe).await
        } else {
            subscribe.await
        }
    }

    /// The seq a render that reads the store after this call reflects: every
    /// batch routed so far was committed before it. The render owes it on the
    /// returned child of `data`, so a render superseded before its successor
    /// begins takes the claim with it when it ends.
    fn begin_render(&self) -> (u64, OwedChild) {
        let seq = self.data.sent_with_children();
        let claim = self.data.owed_child();
        claim.cursor().routed(seq);
        (seq, claim)
    }

    /// The seq a render that `begun` at that seq reflects once its own
    /// subscription is registered: the predecessor's batches routed so far are
    /// in its read, unless batches routed to the render itself already follow
    /// that read. The render owes it on its own cursor, so the predecessor can
    /// end: a successor that takes over later reads the claim from there, and a
    /// render superseded before any successor read it takes the claim with it.
    fn take_over(
        &self,
        begun: u64,
        render: &DeliveryCursor,
        predecessor: Option<&DeliveryCursor>,
    ) -> u64 {
        let handed = predecessor.map_or(0, DeliveryCursor::sent);
        // shortcut: a batch the router hands to both cursors between these two
        // loads is claimed one apply early; close it with a seq from the
        // router.
        if render.sent() > 0 || handed <= begun {
            return begun;
        }
        render.routed(handed);
        handed
    }
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

    let delivery = Delivery::enclosing();
    let struct_stream = delivery
        .subscribe(
            delivery.triggers.cursor(),
            engine.subscribe_sql(&structural_sql),
        )
        .await?;
    let focus_stream = delivery
        .subscribe(
            delivery.triggers.cursor(),
            engine.subscribe_sql_keyed(
                crate::api::block_domain::FOCUS_ROOT_MEMBERSHIP_SQL,
                block_id.as_str(),
            ),
        )
        .await?;

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
    let layout_seed =
        (block_id == holon_api::root_layout_block_uri()).then(|| engine.layout_seed().subscribe());
    let trigger_stream = merge_triggers(
        struct_stream,
        focus_stream,
        layout_seed,
        command_rx,
        profile_signal,
        &mut aborts,
    );

    let watcher_handle = tokio::spawn(run_reactive_watcher(
        trigger_stream,
        engine,
        profile_resolver,
        block_id,
        output_tx,
        delivery,
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

/// Merge structural CDC events, focus-root membership CDC, WatcherCommands, and
/// profile cache changes into a single `RenderTrigger` stream, prepended with
/// an `Initial` trigger.
///
/// Every input `render_entity` reads must be one of these triggers: an input
/// that changes without one leaves the watcher rendering a stale decision.
///
/// All spawned actors register their abort handles with `aborts` so that
/// dropping the owning `WatchHandle` cancels them immediately rather than
/// waiting for their (possibly long-lived) source streams to terminate.
fn merge_triggers(
    struct_stream: RowChangeStream,
    focus_stream: RowChangeStream,
    layout_seed: Option<tokio::sync::watch::Receiver<LayoutSeedPhase>>,
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

    let tx_focus = tx.clone();
    let focus_handle = tokio::spawn(async move {
        tokio::pin!(focus_stream);
        while let Some(_batch) = focus_stream.next().await {
            if tx_focus.send(RenderTrigger::FocusRootChange).await.is_err() {
                break;
            }
        }
    });
    aborts.push(focus_handle.abort_handle());

    if let Some(mut phase) = layout_seed {
        let tx_seed = tx.clone();
        let seed_handle = tokio::spawn(async move {
            while phase.changed().await.is_ok() {
                if *phase.borrow_and_update() == LayoutSeedPhase::Settled
                    && tx_seed
                        .send(RenderTrigger::LayoutSeedSettled)
                        .await
                        .is_err()
                {
                    break;
                }
            }
        });
        aborts.push(seed_handle.abort_handle());
    }

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
    delivery: Delivery,
) {
    let mut generation: u64 = 0;
    let mut variant = None;
    // Dropped by the next generation once its watch is registered, which is
    // what lets the current one go.
    let mut release_current: Option<oneshot::Sender<std::convert::Infallible>> = None;
    let mut current_render: Option<Arc<DeliveryCursor>> = None;

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
            RenderTrigger::FocusRootChange => {
                generation += 1;
                tracing::info!(
                    "[UiWatcher] Focus-root membership of block '{block_id}' changed — re-rendering"
                );
            }
            RenderTrigger::LayoutSeedSettled => {
                generation += 1;
                tracing::info!("[UiWatcher] Layout seed settled — re-rendering '{block_id}'");
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
        let delivery = delivery.clone();

        // Inner stream: Structure event followed by Data events.
        // Spawned as a task so render_entity can be awaited.
        let (inner_tx, inner_rx) = mpsc::channel::<UiEvent>(64);
        let (release_tx, release_rx) = oneshot::channel();
        let release_predecessor = release_current.replace(release_tx);
        let render_cursor = delivery.data.owed_child();
        let predecessor = current_render.replace(render_cursor.cursor().clone());
        let block_id_clone = block_id.clone();
        crate::util::spawn_actor(async move {
            render_and_forward(
                inner_tx,
                engine,
                resolver,
                &block_id_clone,
                &current_var,
                current_gen,
                Handover {
                    release_predecessor,
                    released: release_rx,
                    predecessor,
                },
                render_cursor,
                &delivery,
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

/// How one render generation hands its place to the next: make before break.
///
/// A generation's data stream owns its claim on the watched place, so it keeps
/// that stream until the generation after it has registered its own claim. A
/// re-render of the same place then never leaves the place unwatched, and so
/// never deletes its membership row only to insert it again; a re-render onto
/// another place releases the old one exactly once, in the action that moved.
struct Handover {
    /// Held until this generation's own claim is registered.
    release_predecessor: Option<oneshot::Sender<std::convert::Infallible>>,
    /// Resolves when the next generation holds its claim, or when no next
    /// generation can come because the watcher ended.
    released: oneshot::Receiver<std::convert::Infallible>,
    /// The data cursor of the generation this one supersedes.
    predecessor: Option<Arc<DeliveryCursor>>,
}

/// Render a block and forward the result + data stream into the inner channel.
///
/// On success: emits Structure event, then forwards data CDC as Data events.
/// On failure: emits Structure event with error WidgetSpec (stream stays open).
#[allow(clippy::too_many_arguments)] // one generation's inputs; each arg is a distinct concern
async fn render_and_forward(
    tx: mpsc::Sender<UiEvent>,
    engine: Arc<BackendEngine>,
    profile_resolver: Arc<dyn ProfileResolving>,
    block_id: &EntityUri,
    variant: &Option<String>,
    generation: u64,
    handover: Handover,
    render_cursor: OwedChild,
    delivery: &Delivery,
) {
    let Handover {
        mut release_predecessor,
        released,
        predecessor,
    } = handover;
    let (rendered, begin_claim) = delivery.begin_render();
    let mut held_stream = None;
    'rendered: {
        // Advice-rule status surface (ADR 0022 "rule blocks render their own
        // status"): a rule block whose id carries a NON-Active status has its
        // render replaced by the error surface, so parse errors and async DDL
        // failures are visible in place (v1 replaces, like the existing
        // render-failure path; over-cap banner comes later).
        if let Some(status) = engine.advice_status().get(block_id.as_str())
            && !status.is_active()
        {
            let _ = tx
                .send(UiEvent::Structure {
                    render_expr: error_render_expr(&format!("advice rule: {status}")),
                    candidates: Vec::new(),
                    generation,
                    seq: rendered,
                })
                .await;
            break 'rendered;
        }

        match delivery
            .subscribe(
                render_cursor.cursor(),
                engine.blocks().render_entity(block_id, variant),
            )
            .await
        {
            Ok((render_expr, mut data_stream)) => {
                let rendered =
                    delivery.take_over(rendered, render_cursor.cursor(), predecessor.as_deref());
                drop(release_predecessor.take());
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
                        seq: 0,
                    })
                    .await
                    .is_ok()
                {
                    forward_data_stream(
                        &mut data_stream,
                        &tx,
                        profile_resolver,
                        generation,
                        rendered,
                    )
                    .await;
                }
                held_stream = Some(data_stream);
            }
            Err(e) => {
                if e.downcast_ref::<crate::api::block_domain::NoBlockRow>()
                    .is_some()
                {
                    // WARN, not ERROR: a block with no row is a watch STATE,
                    // not a failed render — the watcher discloses it with the
                    // same visible error widget and recovers on its own when a
                    // row appears.
                    tracing::warn!(
                        block_id = %block_id,
                        "[UiWatcher] render_entity found no block row — nothing to render until one \
                         appears"
                    );
                    let _ = tx
                        .send(UiEvent::Structure {
                            render_expr: error_render_expr(&format!("{e:#}")),
                            candidates: Vec::new(),
                            generation,
                            seq: rendered,
                        })
                        .await;
                    break 'rendered;
                }
                if let Some(unparseable) =
                    e.downcast_ref::<crate::api::block_domain::UnparseableRenderSource>()
                {
                    // WARN, not ERROR: the author's render source is wrong, and
                    // the error widget below discloses that on the block
                    // itself.
                    tracing::warn!(block_id = %block_id, "[UiWatcher] {unparseable}");
                    let _ = tx
                        .send(UiEvent::Structure {
                            render_expr: error_render_expr(&format!("{e:#}")),
                            candidates: Vec::new(),
                            generation,
                            seq: rendered,
                        })
                        .await;
                    break 'rendered;
                }
                let verdict = missing_table_verdict(engine.integration_attribution(), &e);
                if let Some(verdict) = verdict.as_ref().filter(|v| v.is_fully_explained()) {
                    // WARN, not ERROR: every one of these tables belongs to an
                    // integration whose failure was already disclosed at boot
                    // with its real cause. Raising it again per block is the
                    // flood, not the signal.
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
                            seq: rendered,
                        })
                        .await;
                    break 'rendered;
                }

                // Anything the inert half does not fully explain stays loud,
                // and carries BOTH the unexplained owners and any inert
                // integrations that co-occurred — dropping either would hide
                // the half the user can act on.
                let notes = verdict.map(|v| v.notes()).unwrap_or_default();
                let attribution = if notes.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", notes.join("; "))
                };
                // ERROR, not WARN: WARN is the disclosed-degradation tier and
                // is deliberately not read by `inv-no-observed-errors`. A block
                // that failed to render is a real failure, so it must reach the
                // error-capture oracles. The error widget below stays: it is
                // the visible, disclosed surface for the same failure.
                //
                // `{:#}`, not `{}`: the matview error chains its cause rather
                // than spelling it in, and plain Display prints only the
                // outermost context — which would drop the missing table names
                // entirely.
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
                        seq: rendered,
                    })
                    .await;
            }
        }
    }
    drop(release_predecessor);
    // The data stream and the begin claim end only once a successor holds its
    // claim or the watcher ended.
    let Err(oneshot::error::RecvError { .. }) = released.await;
    drop((held_stream, begin_claim));
}

/// Forward a data CDC stream as UiEvent::Data events.
async fn forward_data_stream(
    stream: &mut RowChangeStream,
    output_tx: &mpsc::Sender<UiEvent>,
    profile_resolver: Arc<dyn ProfileResolving>,
    generation: u64,
    rendered: u64,
) {
    loop {
        // A superseded generation's output closes while its data stream is
        // quiet; waiting only for the next batch would hold its watch until
        // some later, unrelated write.
        let batch_with_metadata = tokio::select! {
            () = output_tx.closed() => return,
            next = stream.next() => match next {
                Some(batch) => batch,
                None => return,
            },
        };
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
        // Convert Change<EnrichedRow> → Change<DataRow> at the UiEvent
        // boundary. UiEvent::Data uses MapChange (= Change<DataRow>) — the FFI
        // boundary requires this shape.
        let map_changes: Vec<holon_api::MapChange> = enriched
            .into_iter()
            .map(|c| c.map(EnrichedRow::into_inner))
            .collect();

        let seq = metadata.seq.max(rendered);
        let batch = BatchMapChangeWithMetadata {
            inner: BatchMapChange { items: map_changes },
            metadata,
        };

        if output_tx
            .send(UiEvent::Data {
                batch,
                generation,
                seq,
            })
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
        // variant conditions against this raw storage row (no UI-state
        // bindings), emitting spurious `eval_bool_source` errors. See
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

/// The claims one `watch_ui` puts on its consumer's data cursor, under every
/// interleaving of re-render triggers, router batches and hand-overs between
/// render generations. The model only orders the calls; the cursors and the
/// claims are the real `Delivery` and `DeliveryCursor`.
#[cfg(test)]
mod handover {
    use std::collections::VecDeque;

    use proptest::prelude::*;
    use proptest::test_runner::Config;
    use proptest::test_runner::TestCaseError;
    use proptest::test_runner::TestRunner;

    use super::*;

    #[derive(Clone, Debug)]
    enum Op {
        /// A new generation supersedes the current one. A structural trigger
        /// first routes its batch to the trigger cursor.
        Trigger {
            structural: bool,
        },
        Begin(usize),
        /// The router hands its next batch to the subscribed generations
        /// among `to`.
        Route {
            to: Vec<usize>,
            gap: u64,
        },
        TakeOver(usize),
        /// The render fails: the generation sends its error structure and
        /// ends, which releases its predecessor.
        Fail(usize),
        /// The consumer applies the next event of the current generation.
        Deliver,
        /// A superseded generation that took over or failed ends once its
        /// successor released it.
        End(usize),
    }

    struct Generation {
        render: Option<OwedChild>,
        begin_claim: Option<OwedChild>,
        cursor: Arc<DeliveryCursor>,
        predecessor: Option<Arc<DeliveryCursor>>,
        begun: Option<u64>,
        rendered: Option<u64>,
        /// Batches routed before the hand-over, forwarded after it.
        backlog: Vec<u64>,
        events: VecDeque<u64>,
        handed_over: bool,
        released: bool,
    }

    struct Watcher {
        delivery: Delivery,
        generations: Vec<Generation>,
        last_seq: u64,
    }

    impl Watcher {
        fn new() -> Self {
            Self {
                delivery: Delivery::enclosing(),
                generations: Vec::new(),
                last_seq: 0,
            }
        }

        fn pick(&self, i: usize) -> Option<usize> {
            (!self.generations.is_empty()).then(|| i % self.generations.len())
        }

        fn apply(&mut self, op: &Op) {
            match op {
                Op::Trigger { structural } => self.trigger(*structural),
                Op::Begin(i) => {
                    if let Some(g) = self.pick(*i) {
                        self.begin(g);
                    }
                }
                Op::Route { to, gap } => self.route(to, *gap),
                Op::TakeOver(i) => {
                    if let Some(g) = self.pick(*i) {
                        self.take_over(g);
                    }
                }
                Op::Fail(i) => {
                    if let Some(g) = self.pick(*i) {
                        self.fail(g);
                    }
                }
                Op::Deliver => self.deliver(),
                Op::End(i) => {
                    if let Some(g) = self.pick(*i) {
                        self.end(g);
                    }
                }
            }
        }

        fn trigger(&mut self, structural: bool) {
            if structural {
                self.last_seq += 1;
                self.delivery.triggers.cursor().routed(self.last_seq);
            }
            let render = self.delivery.data.owed_child();
            let cursor = render.cursor().clone();
            let predecessor = self.generations.last().map(|g| g.cursor.clone());
            self.generations.push(Generation {
                render: Some(render),
                begin_claim: None,
                cursor,
                predecessor,
                begun: None,
                rendered: None,
                backlog: Vec::new(),
                events: VecDeque::new(),
                handed_over: false,
                released: false,
            });
        }

        fn begin(&mut self, g: usize) {
            if self.generations[g].begun.is_none() {
                let (begun, claim) = self.delivery.begin_render();
                let generation = &mut self.generations[g];
                generation.begun = Some(begun);
                generation.begin_claim = Some(claim);
            }
        }

        fn route(&mut self, to: &[usize], gap: u64) {
            let mut targets: Vec<usize> = to
                .iter()
                .filter_map(|i| self.pick(*i))
                .filter(|g| {
                    let generation = &self.generations[*g];
                    generation.begun.is_some() && generation.render.is_some()
                })
                .collect();
            targets.sort_unstable();
            targets.dedup();
            if targets.is_empty() {
                return;
            }
            self.last_seq += 1 + gap;
            let seq = self.last_seq;
            for g in targets {
                let generation = &mut self.generations[g];
                generation.cursor.routed(seq);
                match generation.rendered {
                    Some(rendered) => generation.events.push_back(seq.max(rendered)),
                    None => generation.backlog.push(seq),
                }
            }
        }

        fn take_over(&mut self, g: usize) {
            let generation = &mut self.generations[g];
            let Some(begun) = generation.begun else {
                return;
            };
            if generation.handed_over || generation.render.is_none() {
                return;
            }
            let rendered = self.delivery.take_over(
                begun,
                &generation.cursor,
                generation.predecessor.as_deref(),
            );
            generation.rendered = Some(rendered);
            generation.handed_over = true;
            generation.events.push_back(rendered);
            let backlog = std::mem::take(&mut generation.backlog);
            generation
                .events
                .extend(backlog.into_iter().map(|seq| seq.max(rendered)));
            self.release_predecessor_of(g);
        }

        fn fail(&mut self, g: usize) {
            let generation = &mut self.generations[g];
            let Some(begun) = generation.begun else {
                return;
            };
            if generation.handed_over || generation.render.is_none() {
                return;
            }
            generation.events = VecDeque::from([begun]);
            generation.backlog.clear();
            generation.render = None;
            self.release_predecessor_of(g);
        }

        fn release_predecessor_of(&mut self, g: usize) {
            if g > 0 {
                self.generations[g - 1].released = true;
            }
        }

        fn deliver(&mut self) {
            let Some(current) = self.generations.last_mut() else {
                return;
            };
            if let Some(seq) = current.events.pop_front() {
                self.delivery.data.applied_through(seq);
            }
        }

        fn end(&mut self, g: usize) {
            let superseded = g + 1 < self.generations.len();
            let generation = &mut self.generations[g];
            let finished = generation.handed_over || generation.render.is_none();
            if superseded && finished && generation.released {
                generation.render = None;
                generation.begin_claim = None;
            }
        }

        /// Every pending render completes, oldest first, and the consumer
        /// applies everything the current generation sends.
        fn drain(&mut self) {
            for g in 0..self.generations.len() {
                self.begin(g);
            }
            for g in 0..self.generations.len() {
                self.take_over(g);
            }
            for g in 0..self.generations.len() {
                self.end(g);
            }
            while self
                .generations
                .last()
                .is_some_and(|current| !current.events.is_empty())
            {
                self.deliver();
            }
        }
    }

    fn settles(ops: &[Op]) -> Result<(), String> {
        let mut watcher = Watcher::new();
        for op in ops {
            watcher.apply(op);
            owes_what_the_current_generation_still_sends(&watcher)?;
        }
        watcher.drain();
        let data = &watcher.delivery.data;
        if data.is_lagging() {
            return Err(format!(
                "data owed: applied {} sent {}",
                data.applied(),
                data.sent_with_children()
            ));
        }
        Ok(())
    }

    /// Every seq the current generation still sends keeps `data` owed, so a
    /// settle cannot accept a frame before it. A begun render that has not
    /// handed over sends at least `begun`.
    fn owes_what_the_current_generation_still_sends(watcher: &Watcher) -> Result<(), String> {
        let Some(current) = watcher.generations.last() else {
            return Ok(());
        };
        let begun = current
            .begun
            .filter(|_| !current.handed_over && current.render.is_some());
        let Some(pending) = current
            .events
            .iter()
            .chain(&current.backlog)
            .copied()
            .chain(begun)
            .max()
        else {
            return Ok(());
        };
        let data = &watcher.delivery.data;
        if pending > data.applied() && !data.is_lagging() {
            return Err(format!(
                "data settled early: applied {} while the current generation still sends {pending}",
                data.applied()
            ));
        }
        Ok(())
    }

    fn op() -> impl Strategy<Value = Op> {
        let generation = 0..6usize;
        prop_oneof![
            2 => any::<bool>().prop_map(|structural| Op::Trigger { structural }),
            3 => generation.clone().prop_map(Op::Begin),
            3 => (prop::collection::vec(generation.clone(), 1..3), 0..3u64)
                .prop_map(|(to, gap)| Op::Route { to, gap }),
            3 => generation.clone().prop_map(Op::TakeOver),
            1 => generation.clone().prop_map(Op::Fail),
            2 => Just(Op::Deliver),
            2 => generation.prop_map(Op::End),
        ]
    }

    /// Interleavings stored as explicit schedules.
    fn found() -> Vec<(&'static str, Vec<Op>)> {
        use Op::*;
        let first_generation_shows_68 = [
            Trigger { structural: false },
            Begin(0),
            TakeOver(0),
            Route {
                to: vec![0],
                gap: 67,
            },
            Deliver,
            Deliver,
        ];
        vec![
            (
                "superseded before its own hand-over",
                [
                    &first_generation_shows_68[..],
                    &[
                        Trigger { structural: false },
                        Begin(1),
                        Trigger { structural: false },
                        Begin(2),
                        Route {
                            to: vec![0],
                            gap: 0,
                        },
                        TakeOver(1),
                        TakeOver(2),
                    ],
                ]
                .concat(),
            ),
            (
                "superseded, hands over after its successor did",
                [
                    &first_generation_shows_68[..],
                    &[
                        Trigger { structural: false },
                        Begin(1),
                        Trigger { structural: false },
                        Begin(2),
                        TakeOver(2),
                        Route {
                            to: vec![0],
                            gap: 0,
                        },
                        TakeOver(1),
                    ],
                ]
                .concat(),
            ),
            (
                "superseded generation fails while its successor still owes its begin seq",
                vec![
                    Trigger { structural: false },
                    Begin(0),
                    Route {
                        to: vec![0],
                        gap: 0,
                    },
                    Trigger { structural: false },
                    Begin(1),
                    Fail(0),
                    Fail(1),
                ],
            ),
        ]
    }

    #[test]
    fn every_claim_on_the_data_cursor_is_applied_once_the_watcher_settles() {
        let mut failures = Vec::new();
        for (name, ops) in found() {
            if let Err(e) = settles(&ops) {
                failures.push(format!("{name}: {e}"));
            }
        }
        // An under-claim shows only in a few schedules: 256 cases missed it.
        let mut runner = TestRunner::new(Config {
            cases: 2048,
            failure_persistence: None,
            ..Config::default()
        });
        if let Err(e) = runner.run(&prop::collection::vec(op(), 0..40), |ops| {
            settles(&ops).map_err(TestCaseError::fail)
        }) {
            failures.push(format!("generated: {e}"));
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
