//! GPUI UI for subtree sharing: share/accept modals + degraded-event surfacing.
//!
//! Three user-visible entry points:
//! - **Share:** context menu on a block (wired in `lib.rs`) → "Share subtree"
//!   calls `execute_operation("tree", "share_subtree", ...)` and opens a modal
//!   with the returned ticket + a bearer-capability warning quoted from
//!   `docs/Reference/SUBTREE_SHARING.md` + a reserved area for degraded events.
//! - **Accept:** title-bar button "🔗" opens a modal; the current flow uses
//!   "Paste from clipboard + use focused block as parent" because wiring a full
//!   in-modal text-editing form requires gpui_component::input focus plumbing
//!   that's orthogonal to this pass.
//! - **Degraded signals:** a background task drains
//!   `LoroShareBackend::degraded_bus()` and renders toasts / a red modal for
//!   `SnapshotSaveFailed`, `SnapshotLoadFailed`, `RehydrationFailed`.
//!
//! Bridge from the tokio broadcast into GPUI's reactive model:
//! `rt_handle.spawn` runs the `recv().await` loop inside the tokio runtime
//! and forwards events through a `futures::channel::mpsc::unbounded` channel
//! to a pump running on GPUI's executor (`cx.spawn`). The pump calls
//! `cx.update_window` to mutate the `ShareUiState` entity, which emits a
//! `NotifyShareUi` event that triggers the main `HolonApp`'s re-render.

use std::sync::Arc;

use gpui::AnyElement;
use gpui::AnyWindowHandle;
use gpui::AsyncApp;
use gpui::ClipboardItem;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::Hsla;
use gpui::IntoElement;
use gpui::MouseButton;
use gpui::SharedString;
use gpui::Stateful;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use holon::sync::DegradedChange;
use holon::sync::DegradedConditionKey;
use holon::sync::ShareDegraded;
use holon::sync::ShareDegradedReason;
use holon_api::EntityName;
use holon_api::Value;
use holon_app::PendingState;
use holon_app::PendingWriteEvent;
use holon_app::PendingWriteEventKind;
use holon_app::PendingWriteStore;
use holon_app::PendingWriteView;
use holon_frontend::FrontendSession;
use holon_frontend::dispatch_journal::DispatchJournal;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;

/// Threat-model sentences from `docs/Reference/SUBTREE_SHARING.md` (lines
/// 34–35). Quoted verbatim — users of the share UI must see the exact wording
/// so there's no doubt this is a bearer capability.
pub const BEARER_CAPABILITY_WARNING: &str = "A ticket is a bearer capability. Anyone who obtains it can read and write the shared subtree \
     until the share is dropped. There is no authn/authz layer inside iroh — peer identity is the \
     only gate, and the initial handshake does not verify \"who you are\" beyond a cryptographic \
     node id.";

/// Parsed response from `share_subtree` — the op returns a JSON string in
/// `OperationResult::response` with `ticket`, `shared_tree_id`,
/// `mount_block_id`.
#[derive(Clone, Debug)]
pub struct ShareTicket {
    pub ticket: String,
    pub shared_tree_id: String,
    pub mount_block_id: String,
}

impl ShareTicket {
    pub fn from_value(v: &Value) -> anyhow::Result<Self> {
        let Value::String(s) = v else {
            anyhow::bail!("share_subtree response is not a String: {v:?}");
        };
        let parsed: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| anyhow::anyhow!("share_subtree response not valid JSON: {e}; raw={s}"))?;
        let ticket = parsed
            .get("ticket")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("share_subtree response missing `ticket`"))?
            .to_string();
        let shared_tree_id = parsed
            .get("shared_tree_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("share_subtree response missing `shared_tree_id`"))?
            .to_string();
        let mount_block_id = parsed
            .get("mount_block_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("share_subtree response missing `mount_block_id`"))?
            .to_string();
        Ok(ShareTicket {
            ticket,
            shared_tree_id,
            mount_block_id,
        })
    }
}

/// A degraded-mode notification to render as a yellow toast.
#[derive(Clone, Debug)]
pub struct DegradedToast {
    pub kind: DegradedKind,
    pub shared_tree_id: String,
    pub detail: String,
    /// Set for toasts sourced from the degraded bus, where every degradation is
    /// a sticky condition — upserted on re-raise, removed on clear. `None` for
    /// UI-local toasts (undo/command/preference failures, info) that have no
    /// bus condition behind them.
    pub condition: Option<DegradedConditionKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradedKind {
    /// Yellow — save will retry on next commit.
    SnapshotSaveFailed,
    /// Yellow — rehydration hiccup at startup, share may lag.
    RehydrationFailed,
    /// Yellow — a shared doc edit failed to project into SQL; the UI (which
    /// reads SQL) is stale until the next successful projection.
    SqlProjectionFailed,
    /// Red — a shared doc tried to shadow a LOCAL block id; the projection was
    /// refused to protect the recipient's own content.
    ForeignIdCollision,
    /// Red — OrgMode initial-scan failed to ingest one or more vault files.
    /// Other files keep syncing; the failed file(s) need fixing. Surfaced so a
    /// bad file is visible instead of silently killing file sync.
    OrgIngestFailed,
    /// Red — an undo/redo request reached the engine but failed (e.g. no
    /// operation engine wired, or the underlying apply errored). Fail-loud:
    /// undo/redo must never look like a silent no-op when it actually blew
    /// up, so this is always surfaced instead of just logged.
    UndoFailed,
    /// Red — a history entry no longer matched the state it was recorded
    /// against, so it was DROPPED: that edit can never be undone again. This is
    /// a data-trust event, not a failed request, so it says what was lost
    /// rather than that the press did not work.
    UndoStepDropped,
    /// Red — a slash-menu command was selected but failed (e.g. a template
    /// insert whose target block couldn't be resolved, or an empty page-root
    /// placement). Fail-loud: the selection consumed the key, so it must never
    /// look like a silent no-op or a stray block-split.
    CommandFailed,
    /// Red — a preference write reached the config layer but could not be
    /// persisted (e.g. the config dir is on a read-only filesystem). Fail-loud:
    /// the in-memory value applied for this session, but it will NOT survive a
    /// restart, and the process must stay alive — never SIGABRT on a failed
    /// settings write.
    PreferenceSaveFailed,
    /// Yellow — a `once_only` connector write was queued and needs human
    /// confirmation (leases/read-write ruling, increment 4). Disclosed on
    /// enqueue; the write never fires unattended. Approve it in the
    /// pending-writes panel.
    ConnectorWritePending,
    /// Red — a dispatched `once_only` connector write's outcome is unknown
    /// (post-dispatch failure / lost ack). Fail-loud: it is NOT auto-retried;
    /// the human must verify on the remote before resending.
    ConnectorWriteOutcomeUnknown,
    /// Yellow — a write inside a shared/mounted subtree reached Loro+SQL but
    /// its org materialization is pending (mount not yet a page on disk).
    /// Disclosed degrade per the share write-back track (inc 1): the edit is
    /// NOT lost, only the file projection lags.
    SharedSubtreeNotMaterialized,
    /// Red — the org write-back stream died and its supervisor could not keep
    /// it alive. Edits still reach Loro + SQL, but they stop reaching disk, so
    /// the vault on disk silently falls behind the app until this clears.
    WritebackDegraded,
    /// Red — an MCP integration provider failed to connect at boot. Its cache
    /// tables were never created, so dependent pages render blank; this names
    /// the integration and the connect error so the blankness is attributable.
    IntegrationConnectFailed,
    /// Red — an MCP integration provider is waiting on an OAuth grant. Same
    /// blank-page consequence, but the user can fix it via the carried URL.
    IntegrationNeedsAuth,
    /// Yellow — an installed sidecar was not honored and the copy bundled with
    /// this build was used instead. The integration works; the file the user
    /// installed does not, so the detail names both paths and the mismatch.
    IntegrationSidecarSuperseded,
    /// Yellow — a sidecar file is installed for an integration that is not
    /// switched on, so it runs nothing. Names the state file to write.
    IntegrationNotEnabled,
    /// Yellow — a sidecar file names an integration this build does not ship.
    /// Nothing on disk can introduce one, so the file does nothing.
    IntegrationSidecarNotBundled,
    /// A plain info-style toast (used for "ticket copied").
    Info,
}

/// A red-modal quarantine event. Separate from `DegradedToast` because it
/// needs a distinct, persistent, full-screen treatment.
#[derive(Clone, Debug)]
pub struct QuarantineEvent {
    pub shared_tree_id: String,
    pub quarantine_path: String,
}

/// Per-window share-UI state. Lives as a GPUI `Entity` on the main thread.
pub struct ShareUiState {
    pub share_modal: Option<ShareTicket>,
    pub show_accept_modal: bool,
    pub toasts: Vec<DegradedToast>,
    pub quarantines: Vec<QuarantineEvent>,
    pub share_error: Option<String>,
    pub accept_error: Option<String>,
}

impl ShareUiState {
    pub fn new() -> Self {
        Self {
            share_modal: None,
            show_accept_modal: false,
            toasts: Vec::new(),
            quarantines: Vec::new(),
            share_error: None,
            accept_error: None,
        }
    }

    pub fn open_share(&mut self, ticket: ShareTicket) {
        self.share_error = None;
        self.share_modal = Some(ticket);
    }

    pub fn open_accept(&mut self) {
        self.accept_error = None;
        self.show_accept_modal = true;
    }

    pub fn close_share(&mut self) {
        self.share_modal = None;
        self.share_error = None;
    }

    pub fn close_accept(&mut self) {
        self.show_accept_modal = false;
        self.accept_error = None;
    }

    pub fn dismiss_toast(&mut self, index: usize) {
        if index < self.toasts.len() {
            self.toasts.remove(index);
        }
    }

    pub fn dismiss_quarantine(&mut self, index: usize) {
        if index < self.quarantines.len() {
            self.quarantines.remove(index);
        }
    }

    /// Route a broadcast event from the degraded bus into the right field.
    pub fn apply_degraded(&mut self, event: ShareDegraded) {
        let condition = event.condition_key();
        match event.reason {
            ShareDegradedReason::SnapshotSaveFailed(detail) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::SnapshotSaveFailed,
                    shared_tree_id: event.shared_tree_id,
                    detail,
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::RehydrationFailed(detail) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::RehydrationFailed,
                    shared_tree_id: event.shared_tree_id,
                    detail,
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::SqlProjectionFailed(detail) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::SqlProjectionFailed,
                    shared_tree_id: event.shared_tree_id,
                    detail,
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::ForeignIdCollision(block_id) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::ForeignIdCollision,
                    shared_tree_id: event.shared_tree_id,
                    detail: block_id,
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::SnapshotLoadFailed(path) => {
                // Upsert, like `push_toast`: a sticky condition can arrive
                // twice (once replayed in `current`, once live), and two
                // identical full-screen quarantine modals for one share is a
                // dismissal treadmill.
                let quarantine = QuarantineEvent {
                    shared_tree_id: event.shared_tree_id,
                    quarantine_path: path,
                };
                match self
                    .quarantines
                    .iter_mut()
                    .find(|q| q.shared_tree_id == quarantine.shared_tree_id)
                {
                    Some(existing) => *existing = quarantine,
                    None => self.quarantines.push(quarantine),
                }
            }
            ShareDegradedReason::OrgIngestFailed(summary) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::OrgIngestFailed,
                    shared_tree_id: event.shared_tree_id,
                    detail: summary,
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::WritebackDegraded(detail) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::WritebackDegraded,
                    shared_tree_id: event.shared_tree_id,
                    detail,
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::SharedSubtreeNotMaterialized {
                block_id,
                owning_page,
            } => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::SharedSubtreeNotMaterialized,
                    shared_tree_id: event.shared_tree_id,
                    // Name the page the user can open. With no page in the
                    // walk there is nothing better to show than the block id.
                    detail: owning_page.unwrap_or(block_id),
                    condition: Some(condition.clone()),
                });
            }
            // The toast body truncates `detail` at 80 chars, so both of these
            // lead with the integration name.
            ShareDegradedReason::IntegrationConnectFailed { integration, error } => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::IntegrationConnectFailed,
                    shared_tree_id: event.shared_tree_id,
                    detail: format!("{integration}: {error}"),
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::IntegrationNeedsAuth {
                integration,
                auth_url,
            } => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::IntegrationNeedsAuth,
                    shared_tree_id: event.shared_tree_id,
                    detail: format!("{integration}: authorize at {auth_url}"),
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::IntegrationSidecarSuperseded {
                integration,
                installed_path,
                bundled_source,
                incompatibility,
            } => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::IntegrationSidecarSuperseded,
                    shared_tree_id: event.shared_tree_id,
                    detail: format!(
                        "{integration}: {installed_path} was ignored ({incompatibility}); running \
                         the bundled {bundled_source}"
                    ),
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::IntegrationNotEnabled {
                integration,
                installed_path,
                state_path,
                remedy,
            } => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::IntegrationNotEnabled,
                    shared_tree_id: event.shared_tree_id,
                    // Remedy first: it is the only clause the user acts on, so
                    // it must survive both the cap and a hurried read.
                    detail: format!(
                        "{integration}: run `{remedy}` to write {state_path} — until then \
                         {installed_path} runs nothing"
                    ),
                    condition: Some(condition.clone()),
                });
            }
            ShareDegradedReason::IntegrationSidecarNotBundled {
                provider,
                installed_path,
            } => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::IntegrationSidecarNotBundled,
                    shared_tree_id: event.shared_tree_id,
                    detail: format!(
                        "{provider}: {installed_path} names an integration this build does not \
                         ship — it runs nothing"
                    ),
                    condition: Some(condition.clone()),
                });
            }
        }
    }

    /// Drop the toast for a condition the bus reports as no longer in effect.
    pub fn apply_degraded_cleared(&mut self, key: &DegradedConditionKey) {
        self.toasts.retain(|t| t.condition.as_ref() != Some(key));
    }

    pub fn push_toast(&mut self, toast: DegradedToast) {
        const MAX_TOASTS: usize = 5;
        // A condition can arrive twice — once in a subscription's replayed
        // `current`, once as a live `Raised` — so it upserts rather than stacks.
        if let Some(key) = toast.condition.clone() {
            if let Some(existing) = self
                .toasts
                .iter_mut()
                .find(|t| t.condition.as_ref() == Some(&key))
            {
                *existing = toast;
                return;
            }
        }
        if self.toasts.len() >= MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.toasts.push(toast);
    }
}

impl Default for ShareUiState {
    fn default() -> Self {
        Self::new()
    }
}

/// Marker event — consumers call `cx.notify()` when they see it.
pub struct NotifyShareUi;
impl EventEmitter<NotifyShareUi> for ShareUiState {}

/// GPUI global that routes a right-click-share event from a block view back
/// into the window-level share-UI wiring. Any GPUI view that knows a row_id
/// and receives a right-click dispatches `ShareTrigger::trigger(block_id, cx)`.
///
/// Set in `launch_holon_window_impl` after the share_backend is wired.
#[derive(Clone)]
pub struct ShareTrigger(Arc<dyn Fn(String, &mut gpui::App) + Send + Sync>);

impl ShareTrigger {
    pub fn new(f: impl Fn(String, &mut gpui::App) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn trigger(block_id: String, cx: &mut gpui::App) {
        if let Some(t) = cx.try_global::<ShareTrigger>().cloned() {
            (t.0)(block_id, cx);
        } else {
            tracing::warn!(
                "[share-ui] ShareTrigger global missing; share context menu is inert (iroh-sync \
                 disabled?)"
            );
        }
    }
}

impl gpui::Global for ShareTrigger {}

/// GPUI global that lets any view surface a [`DegradedToast`] without plumbing
/// the `ShareUiState` entity through every intermediate builder — mirrors
/// [`ShareTrigger`]. Installed in `launch_holon_window_impl`.
#[derive(Clone)]
pub struct DegradedToastSink(Arc<dyn Fn(DegradedToast, &mut gpui::App) + Send + Sync>);

impl DegradedToastSink {
    pub fn new(f: impl Fn(DegradedToast, &mut gpui::App) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Surface `toast`. If the sink global is missing (a wiring bug), fail loud
    /// in the log rather than silently dropping the failure notice.
    pub fn push(toast: DegradedToast, cx: &mut gpui::App) {
        if let Some(sink) = cx.try_global::<DegradedToastSink>().cloned() {
            (sink.0)(toast, cx);
        } else {
            tracing::error!(
                "[degraded-toast] sink global missing; toast dropped: {}",
                toast.detail
            );
        }
    }
}

impl gpui::Global for DegradedToastSink {}

// ─── Degraded bus bridge ────────────────────────────────────────────────────

/// Spawn the tokio-broadcast → GPUI-entity bridge.
///
/// The `recv()` loop runs inside the tokio runtime (`rt_handle.spawn`). Each
/// received `ShareDegraded` is forwarded through an unbounded `mpsc` channel
/// to a pump running on GPUI's executor, which calls `cx.update_window` to
/// mutate the `ShareUiState`.
/// Takes the bus itself, NOT the share backend: degraded conditions are raised
/// by MCP integrations and org ingest as well as by shares, so the subscriber
/// must exist in every consolidator mode — keying it off a Loro-only handle
/// left the shipped SqlOnly build raising conditions nobody listened to.
pub fn spawn_degraded_bus_bridge(
    bus: Arc<holon::sync::DegradedSignalBus>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<DegradedChange>();

    // Subscribe SYNCHRONOUSLY, before the pump task is scheduled: a condition
    // raised between this call and the task's first poll would otherwise be
    // lost, and "is anyone listening?" must be true the moment wiring returns.
    let subscription = bus.subscribe();

    // Tokio side: replay the conditions already in effect (they may have been
    // raised during boot DI, long before this window existed), then pump live
    // changes.
    rt_handle.spawn(async move {
        let mut bus_rx = subscription.changes;
        for event in subscription.current {
            if tx.unbounded_send(DegradedChange::Raised(event)).is_err() {
                return;
            }
        }
        loop {
            match bus_rx.recv().await {
                Ok(change) => {
                    if tx.unbounded_send(change).is_err() {
                        return; // pump gone, exit
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[share-ui] degraded bus lagged by {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!("[share-ui] degraded bus closed; bridge exiting");
                    return;
                }
            }
        }
    });

    // GPUI side: drain mpsc, mutate state entity.
    async_cx
        .spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(change) = rx.next().await {
                let _ = cx.update_window(window_handle, |_, _window, cx| {
                    share_state.update(cx, |s, cx| {
                        match change.clone() {
                            DegradedChange::Raised(event) => s.apply_degraded(event),
                            DegradedChange::Cleared(key) => s.apply_degraded_cleared(&key),
                        }
                        cx.emit(NotifyShareUi);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
}

/// Bridge fire-and-forget op-execution failures (from `dispatch_intent`, which
/// has no awaiting caller) to visible `CommandFailed` toasts. The
/// frontend-agnostic engine holds the returned sink on its `UiState` and calls
/// it — from a spawned tokio task, off the main thread — on every dropped op
/// error; this drains the messages on GPUI's executor and renders a red toast
/// carrying the op's VERBATIM error (e.g. the fail-closed delete's
/// `delete_subtree` / `delete_keep_children` guidance). Mirrors
/// [`spawn_degraded_bus_bridge`]; additive to the engine's `error_tracker` +
/// `tracing::error!` monitoring seams.
pub fn spawn_op_failure_toast_bridge(
    toast_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) -> Arc<dyn Fn(String) + Send + Sync> {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();

    // GPUI side: drain mpsc on the main thread, push a CommandFailed toast.
    async_cx
        .spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(detail) = rx.next().await {
                let _ = cx.update_window(window_handle, |_, _window, cx| {
                    toast_state.update(cx, |s, cx| {
                        s.push_toast(DegradedToast {
                            kind: DegradedKind::CommandFailed,
                            shared_tree_id: "command".into(),
                            detail,
                            condition: None,
                        });
                        cx.emit(NotifyShareUi);
                        cx.notify();
                    });
                });
            }
        })
        .detach();

    // The sink the engine calls off-thread: forward into the mpsc channel.
    Arc::new(move |detail: String| {
        let _ = tx.unbounded_send(detail);
    })
}

// ─── Pending connector-write approval (leases/read-write ruling, inc 4c) ────

/// GPUI global holding the shared [`PendingWriteStore`] so the render pass and
/// the approve dispatcher can reach it without threading it through every
/// window-launch signature — mirrors [`DegradedToastSink`]/[`ShareTrigger`].
/// Installed in `main.rs` from the DI-resolved handle when MCP integrations are
/// configured.
#[derive(Clone)]
pub struct PendingWritesGlobal(pub Arc<PendingWriteStore>);

impl gpui::Global for PendingWritesGlobal {}

/// Spawn the pending-write bus → GPUI bridge (mirror of
/// [`spawn_degraded_bus_bridge`]). Each [`PendingWriteEvent`] pushes a
/// disclosure toast and triggers a re-render; the panel itself reads live state
/// via [`PendingWriteStore::list`], so the event only needs to nudge the UI.
pub fn spawn_pending_writes_bridge(
    store: Arc<PendingWriteStore>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<PendingWriteEvent>();

    rt_handle.spawn(async move {
        let mut bus_rx = store.subscribe();
        loop {
            match bus_rx.recv().await {
                Ok(event) => {
                    if tx.unbounded_send(event).is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[pending-writes] bus lagged by {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!("[pending-writes] bus closed; bridge exiting");
                    return;
                }
            }
        }
    });

    async_cx
        .spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(event) = rx.next().await {
                let toast = pending_event_toast(&event);
                let _ = cx.update_window(window_handle, |_, _window, cx| {
                    share_state.update(cx, |s, cx| {
                        s.push_toast(toast.clone());
                        cx.emit(NotifyShareUi);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
}

/// Build the disclosure toast for a pending-write event.
fn pending_event_toast(event: &PendingWriteEvent) -> DegradedToast {
    match event.kind {
        PendingWriteEventKind::AwaitingConfirmation => DegradedToast {
            kind: DegradedKind::ConnectorWritePending,
            shared_tree_id: event.connector.clone(),
            detail: format!(
                "{} ({}) — approve in the pending panel",
                event.display, event.tool
            ),
            condition: None,
        },
        PendingWriteEventKind::OutcomeUnknown => DegradedToast {
            kind: DegradedKind::ConnectorWriteOutcomeUnknown,
            shared_tree_id: event.connector.clone(),
            detail: format!(
                "{} ({}) — {}; verify on the remote",
                event.display, event.tool, event.detail
            ),
            condition: None,
        },
    }
}

/// Dispatch approval of a queued `once_only` connector write (increment 4c).
/// Compare-and-take on the shared store, then re-dispatch through
/// `session.execute_operation` — the SAME chokepoint — with the stored call.
/// Only the single winning approval re-dispatches (the store's `confirm` is a
/// one-shot); a failed re-dispatch surfaces a loud toast.
pub fn dispatch_approve(
    session: Arc<FrontendSession>,
    store: Arc<PendingWriteStore>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
    intent_key: String,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<(), String>>();
    rt_handle.spawn(async move {
        let outcome = if !store.confirm(&intent_key) {
            Err(format!(
                "no once_only write awaiting confirmation for intent '{intent_key}' (already \
                 approved, dispatched, or unknown-outcome)"
            ))
        } else {
            match store.stored_call(&intent_key) {
                Some((entity_name, op_name, params)) => {
                    // StorageEntity keys are `Arc<str>`; the session API takes
                    // `HashMap<String, Value>`. The key/value strings survive the
                    // round-trip, so the chokepoint re-mints the SAME intent key.
                    let params: std::collections::HashMap<String, Value> = params
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect();
                    session
                        .execute_operation(&entity_name, &op_name, params)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("{e:#}"))
                }
                None => Err(format!(
                    "confirmed intent '{intent_key}' has no stored call — cannot re-dispatch"
                )),
            }
        };
        let _ = tx.send(outcome);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                share_state.update(cx, |s, cx| {
                    if let Ok(Err(e)) = outcome {
                        s.push_toast(DegradedToast {
                            kind: DegradedKind::ConnectorWriteOutcomeUnknown,
                            shared_tree_id: "connector-write".into(),
                            detail: format!("approve failed: {e}"),
                            condition: None,
                        });
                    }
                    // Success is silent here; the panel re-reads store state
                    // (the row leaves AwaitingConfirmation) and disappears.
                    cx.emit(NotifyShareUi);
                    cx.notify();
                });
            });
        })
        .detach();
}

// ─── Op dispatchers (tokio-side + GPUI-side result routing) ─────────────────

pub fn dispatch_share(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
    block_id: String,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<ShareTicket, String>>();
    rt_handle.spawn(async move {
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), Value::String(block_id));
        // State-only sharing: "full" retention would ship the whole forked
        // oplog (including pruned sibling subtrees' content) to the accepter —
        // a whole-vault history leak. "none" exports current state only.
        // See docs/Reference/SUBTREE_SHARING.md B1.
        params.insert("retention".to_string(), Value::String("none".to_string()));
        let result = session
            .execute_operation(&EntityName::new("tree"), "share_subtree", params)
            .await;
        let outcome = match result.map(|out| out.response) {
            Ok(Some(v)) => ShareTicket::from_value(&v).map_err(|e| format!("{e:#}")),
            Ok(None) => Err("share_subtree returned no response".to_string()),
            Err(e) => Err(format!("{e:#}")),
        };
        let _ = tx.send(outcome);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                share_state.update(cx, |s, cx| {
                    match outcome {
                        Ok(Ok(ticket)) => s.open_share(ticket),
                        Ok(Err(e)) => {
                            s.share_modal = None;
                            s.share_error = Some(e);
                        }
                        Err(_cancelled) => {
                            s.share_error =
                                Some("share_subtree task dropped before responding".into());
                        }
                    }
                    cx.emit(NotifyShareUi);
                    cx.notify();
                });
            });
        })
        .detach();
}

pub fn dispatch_accept(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
    parent_id: String,
    ticket: String,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<(), String>>();
    rt_handle.spawn(async move {
        let mut params = std::collections::HashMap::new();
        params.insert("parent_id".to_string(), Value::String(parent_id));
        params.insert("ticket".to_string(), Value::String(ticket));
        let result = session
            .execute_operation(&EntityName::new("tree"), "accept_shared_subtree", params)
            .await;
        let outcome = match result {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("{e:#}")),
        };
        let _ = tx.send(outcome);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                share_state.update(cx, |s, cx| {
                    match outcome {
                        Ok(Ok(())) => s.close_accept(),
                        Ok(Err(e)) => s.accept_error = Some(e),
                        Err(_) => {
                            s.accept_error = Some("accept_shared_subtree task dropped".into());
                        }
                    }
                    cx.emit(NotifyShareUi);
                    cx.notify();
                });
            });
        })
        .detach();
}

/// What one undo/redo press owes the user, if anything.
struct UndoDisclosure {
    kind: DegradedKind,
    detail: String,
    /// A consumed no-op entry is a degraded press; a DROPPED step and a failed
    /// request are errors.
    warn_only: bool,
}

/// Map an undo/redo outcome to its disclosure. `None` = nothing to say: the
/// press did its job, and for `Applied` the projection update is the feedback.
///
/// Extracted from the dispatch closure so the ROUTING — which outcome earns
/// which toast kind and which words — is pinned by a test instead of living
/// only inside a GPUI window callback. `StaleDropped` must NOT share a kind
/// with the failure arms: a step that is gone forever and a press that did not
/// work call for different actions from the reader.
fn undo_disclosure(
    label: &str,
    outcome: &Result<Result<holon_api::UndoOutcome, String>, futures::channel::oneshot::Canceled>,
) -> Option<UndoDisclosure> {
    match outcome {
        Ok(Ok(holon_api::UndoOutcome::Applied | holon_api::UndoOutcome::Empty)) => None,
        Ok(Ok(holon_api::UndoOutcome::NoChange)) => Some(UndoDisclosure {
            kind: DegradedKind::UndoFailed,
            detail: format!("{label}: entry made no change (no-op)"),
            warn_only: true,
        }),
        Ok(Ok(holon_api::UndoOutcome::StaleDropped { reason })) => Some(UndoDisclosure {
            kind: DegradedKind::UndoStepDropped,
            detail: holon_api::undo_step_dropped_detail(label, reason),
            warn_only: false,
        }),
        Ok(Err(e)) => Some(UndoDisclosure {
            kind: DegradedKind::UndoFailed,
            detail: format!("{label}: {e}"),
            warn_only: false,
        }),
        Err(_cancelled) => Some(UndoDisclosure {
            kind: DegradedKind::UndoFailed,
            detail: format!("{label}: task dropped before responding"),
            warn_only: false,
        }),
    }
}

/// Undo/redo dispatch, cmd-z / cmd-shift-z.
///
/// Same tokio-side-compute + oneshot + GPUI-side-toast shape as
/// `dispatch_share`/`dispatch_accept` above: the engine call runs on the
/// tokio runtime (`rt_handle`), the result crosses to the GPUI executor via
/// a oneshot channel, and only a genuine `Err` produces a user-visible
/// toast — `Applied` is silent (the projection update is the feedback) and
/// `Empty` only logs at debug level.
fn dispatch_undo_redo(
    is_redo: bool,
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    journal: Arc<DispatchJournal>,
    reseed: holon_frontend::reactive::AuthorityReseedHandle,
    async_cx: &AsyncApp,
) {
    // Undo/redo run `FrontendSession::undo` directly — they never reach
    // `dispatch_intent`, so without this the dispatch journal (and every
    // reply reading it) would report a working cmd+z as "nothing ran".
    let journal_seq = journal.record_window_action(if is_redo { "redo" } else { "undo" });
    // Taken HERE, on the press, because the round trip below is a real DB call:
    // the row that holds focus and the buffer state at resolution time are not
    // the ones the replay acted on.
    let reseed_gesture = reseed.capture();
    let (tx, rx) = futures::channel::oneshot::channel::<Result<holon_api::UndoOutcome, String>>();
    rt_handle.spawn(async move {
        let result = if is_redo {
            session.redo().await
        } else {
            session.undo().await
        };
        let _ = tx.send(result.map_err(|e| format!("{e:#}")));
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let label = if is_redo { "redo" } else { "undo" };
            // The replay rewrote the store under the row that held focus at the
            // press. Its open editor is skipped by every convergence channel
            // while focused, so without this its stale buffer survives — and the
            // next keystroke commits it over the restored content.
            //
            // The outcome is BOUND, not dropped: `ReseedArm` is `#[must_use]`
            // precisely because discarding it is how a skipped re-seed becomes
            // invisible. `arm` discloses it (it holds the target row); this
            // scope adds it to the press's own line so one log entry answers
            // "did the gesture run, and did the row re-seed".
            let reseed = matches!(outcome, Ok(Ok(holon_api::UndoOutcome::Applied)))
                .then(|| reseed_gesture.arm(label));
            let disclosure = undo_disclosure(label, &outcome);
            match &disclosure {
                None => tracing::debug!("[{label}] ran; re-seed {reseed:?}"),
                Some(d) if d.warn_only => tracing::warn!("[{label}] {}", d.detail),
                Some(d) => tracing::error!("[{label}] {}", d.detail),
            }
            // Every press settles the journal entry: no disclosure = the action
            // ran (an empty stack is a legitimate outcome of running it), a
            // disclosure = it did not do its job, verbatim reason attached.
            journal.settle(
                journal_seq,
                match &disclosure {
                    None => Ok(()),
                    Some(d) => Err(d.detail.clone()),
                },
            );
            if let Some(d) = disclosure {
                let _ = cx.update_window(window_handle, |_, _window, cx| {
                    share_state.update(cx, |s, cx| {
                        s.push_toast(DegradedToast {
                            kind: d.kind,
                            shared_tree_id: "undo".into(),
                            detail: d.detail,
                            condition: None,
                        });
                        cx.emit(NotifyShareUi);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
}

pub fn dispatch_undo(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    journal: Arc<DispatchJournal>,
    reseed: holon_frontend::reactive::AuthorityReseedHandle,
    async_cx: &AsyncApp,
) {
    dispatch_undo_redo(
        false,
        session,
        rt_handle,
        share_state,
        window_handle,
        journal,
        reseed,
        async_cx,
    );
}

pub fn dispatch_redo(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    journal: Arc<DispatchJournal>,
    reseed: holon_frontend::reactive::AuthorityReseedHandle,
    async_cx: &AsyncApp,
) {
    dispatch_undo_redo(
        true,
        session,
        rt_handle,
        share_state,
        window_handle,
        journal,
        reseed,
        async_cx,
    );
}

// ─── Rendering ──────────────────────────────────────────────────────────────

/// Theme values needed by the overlays.
#[derive(Clone, Copy)]
pub struct OverlayTheme {
    pub bg: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub muted_fg: Hsla,
}

/// Render every overlay (share/accept/quarantine modals + toast stack) for
/// the current state. Caller stacks these on top of the main content.
#[allow(clippy::too_many_arguments)]
pub fn render_overlays(
    state: &ShareUiState,
    share_state: Entity<ShareUiState>,
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    window_handle: AnyWindowHandle,
    async_cx: AsyncApp,
    pending_store: Option<Arc<PendingWriteStore>>,
    theme: OverlayTheme,
) -> Vec<AnyElement> {
    let mut overlays: Vec<AnyElement> = Vec::new();

    // Pending connector-write approval panel (leases/read-write ruling, inc 4c).
    // Built first, from clones, so the modal branches below can still consume
    // `session`/`async_cx`/`share_state` by value. Rendered last (pushed at the
    // end) so it sits above content. Shows writes awaiting confirmation and
    // disclosed outcome-unknown entries; both must be visible.
    let pending_panel: Option<AnyElement> = pending_store.as_ref().and_then(|store| {
        let rows: Vec<PendingWriteView> = store
            .list()
            .into_iter()
            .filter(|r| {
                matches!(
                    r.state,
                    PendingState::AwaitingConfirmation | PendingState::OutcomeUnknown { .. }
                )
            })
            .collect();
        if rows.is_empty() {
            None
        } else {
            Some(render_pending_writes_panel(
                rows,
                session.clone(),
                store.clone(),
                rt_handle.clone(),
                window_handle,
                async_cx.clone(),
                share_state.clone(),
                theme,
            ))
        }
    });

    if let Some(ticket) = &state.share_modal {
        overlays.push(render_share_modal(ticket, share_state.clone(), theme));
    } else if let Some(e) = &state.share_error {
        overlays.push(render_error_modal(
            "Share failed",
            e,
            share_state.clone(),
            |s| s.share_error = None,
            theme,
        ));
    }

    if state.show_accept_modal {
        overlays.push(render_accept_modal(
            state.accept_error.as_deref(),
            share_state.clone(),
            session,
            engine,
            rt_handle,
            window_handle,
            async_cx,
            theme,
        ));
    }

    for (idx, q) in state.quarantines.iter().enumerate() {
        overlays.push(render_quarantine_modal(idx, q, share_state.clone(), theme));
    }

    if !state.toasts.is_empty() {
        overlays.push(render_toast_stack(&state.toasts, share_state, theme));
    }

    if let Some(panel) = pending_panel {
        overlays.push(panel);
    }

    overlays
}

/// Render the pending connector-write approval panel (leases/read-write ruling,
/// increment 4c). One card per `AwaitingConfirmation` intent (with an Approve
/// button that re-dispatches through the chokepoint) and per `OutcomeUnknown`
/// intent (disclosed, no auto-retry — the human verifies on the remote).
#[allow(clippy::too_many_arguments)]
fn render_pending_writes_panel(
    rows: Vec<PendingWriteView>,
    session: Arc<FrontendSession>,
    store: Arc<PendingWriteStore>,
    rt_handle: tokio::runtime::Handle,
    window_handle: AnyWindowHandle,
    async_cx: AsyncApp,
    share_state: Entity<ShareUiState>,
    theme: OverlayTheme,
) -> AnyElement {
    let mut panel = div()
        .id("pending-writes-panel")
        .absolute()
        .top(px(16.0))
        .right(px(16.0))
        .flex()
        .flex_col()
        .gap_2()
        .min_w(px(300.0))
        .max_w(px(420.0));

    panel = panel.child(
        div()
            .text_size(px(12.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.fg)
            .child(format!("Connector writes ({})", rows.len())),
    );

    for row in rows {
        let awaiting = matches!(row.state, PendingState::AwaitingConfirmation);
        let (bar, title) = if awaiting {
            (gpui::rgba(0xfbbf24ff), "Awaiting approval")
        } else {
            (gpui::rgba(0xef4444ff), "Outcome unknown — verify on remote")
        };
        let detail = match &row.state {
            PendingState::OutcomeUnknown { detail } => {
                format!("{} · {} · {}", row.display, row.tool, detail)
            }
            _ => format!("{} · {} · {}", row.display, row.tool, row.connector),
        };

        let mut card = div()
            .id(SharedString::from(format!("pending-{}", row.intent_key)))
            .px_3()
            .py_2()
            .rounded(px(6.0))
            .bg(theme.bg)
            .border_l_4()
            .border_1()
            .border_color(bar)
            .text_color(theme.fg)
            .text_size(px(12.0))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(bar)
                    .child(title),
            )
            .child(div().text_color(theme.muted_fg).child(detail));

        if awaiting {
            let session = session.clone();
            let store = store.clone();
            let rt_handle = rt_handle.clone();
            let share_state = share_state.clone();
            let async_cx = async_cx.clone();
            let key = row.intent_key.clone();
            card = card.child(
                div()
                    .id(SharedString::from(format!("approve-{}", row.intent_key)))
                    .mt_1()
                    .px_3()
                    .py_1()
                    .rounded(px(6.0))
                    .bg(gpui::rgba(0x22c55eff))
                    .text_color(gpui::rgba(0x000000cc))
                    .cursor_pointer()
                    .w(px(96.0))
                    .child("Approve")
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        dispatch_approve(
                            session.clone(),
                            store.clone(),
                            rt_handle.clone(),
                            share_state.clone(),
                            window_handle,
                            &async_cx,
                            key.clone(),
                        );
                    }),
            );
        }

        panel = panel.child(card);
    }

    panel.into_any_element()
}

fn overlay_backdrop(id: &str) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("{id}-backdrop")))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(gpui::rgba(0x00000088))
        .flex()
        .items_center()
        .justify_center()
        // Inset so the capped-width panel keeps a margin on narrow (phone)
        // viewports instead of running off both screen edges.
        .p(px(16.0))
}

fn modal_panel(id: &str, width: f32, theme: OverlayTheme) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("{id}-panel")))
        // `width` is a MAX, not a demand: on a phone (~402pt) the panel becomes
        // a full-width card; on desktop it caps at `width`. Fixes the accept/
        // share/quarantine/error dialogs overflowing the mobile viewport.
        .w_full()
        .max_w(px(width))
        .max_h(px(720.0))
        .overflow_y_scroll()
        .bg(theme.bg)
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .p(px(24.0))
        .flex()
        .flex_col()
        .gap_3()
}

fn render_share_modal(
    ticket: &ShareTicket,
    share_state: Entity<ShareUiState>,
    theme: OverlayTheme,
) -> AnyElement {
    let ticket_text = ticket.ticket.clone();
    let ticket_for_copy = ticket_text.clone();
    let shared_tree_id = ticket.shared_tree_id.clone();
    let mount_block_id = ticket.mount_block_id.clone();

    let close_a = share_state.clone();
    let close_b = share_state.clone();
    let copy_state = share_state.clone();

    overlay_backdrop("share-modal")
        .child(
            modal_panel("share-modal", 640.0, theme)
                .text_color(theme.fg)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .pb_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Subtree shared"),
                        )
                        .child(
                            div()
                                .id("share-modal-close")
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .hover(|s| s.bg(gpui::rgba(0xffffff18)))
                                .child("✕")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_a.update(cx, |s, cx| {
                                        s.close_share();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .child(
                    div()
                        .p_3()
                        .rounded(px(6.0))
                        .bg(gpui::rgba(0x80000020))
                        .border_1()
                        .border_color(gpui::rgba(0xa02020ff))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .child(BEARER_CAPABILITY_WARNING.to_string()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_fg)
                        .child(format!(
                            "shared_tree_id: {shared_tree_id}   mount_block_id: {mount_block_id}"
                        )),
                )
                .child(
                    div()
                        .id("share-ticket-box")
                        .p_3()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(gpui::rgba(0x0000001a))
                        .text_size(px(11.0))
                        .text_color(theme.fg)
                        .child(ticket_text),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .id("share-copy-button")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0x2563ebff))
                                .text_color(gpui::rgba(0xffffffff))
                                .text_size(px(13.0))
                                .hover(|s| s.bg(gpui::rgba(0x1d4ed8ff)))
                                .child("Copy ticket")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        ticket_for_copy.clone(),
                                    ));
                                    copy_state.update(cx, |s, cx| {
                                        s.push_toast(DegradedToast {
                                            kind: DegradedKind::Info,
                                            shared_tree_id: "ui".into(),
                                            detail: "Ticket copied to clipboard".into(),
                                            condition: None,
                                        });
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            div()
                                .id("share-modal-dismiss")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(13.0))
                                .child("Close")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_b.update(cx, |s, cx| {
                                        s.close_share();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                // Reserved space for degraded events that fire before dismissal.
                .child(
                    div()
                        .min_h(px(24.0))
                        .mt_2()
                        .text_size(px(12.0))
                        .text_color(gpui::rgba(0xd97706ff)),
                ),
        )
        .into_any_element()
}

fn render_accept_modal(
    inline_error: Option<&str>,
    share_state: Entity<ShareUiState>,
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    window_handle: AnyWindowHandle,
    async_cx: AsyncApp,
    theme: OverlayTheme,
) -> AnyElement {
    let close_a = share_state.clone();
    let close_b = share_state.clone();
    let paste_state = share_state.clone();
    let inline_error_owned = inline_error.map(|s| s.to_string());

    overlay_backdrop("accept-modal")
        .child(
            modal_panel("accept-modal", 640.0, theme)
                .text_color(theme.fg)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .pb_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Accept share ticket"),
                        )
                        .child(
                            div()
                                .id("accept-modal-close")
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .hover(|s| s.bg(gpui::rgba(0xffffff18)))
                                .child("✕")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_a.update(cx, |s, cx| {
                                        s.close_accept();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .child(div().text_size(px(13.0)).text_color(theme.muted_fg).child(
                    "Click 'Paste & accept' to read a ticket from the clipboard and attach the \
                     shared subtree under the currently focused block.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .id("accept-paste-button")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0x2563ebff))
                                .text_color(gpui::rgba(0xffffffff))
                                .text_size(px(13.0))
                                .hover(|s| s.bg(gpui::rgba(0x1d4ed8ff)))
                                .child("Paste & accept")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    let clip = cx
                                        .read_from_clipboard()
                                        .and_then(|c| c.text().map(|s| s.to_string()))
                                        .unwrap_or_default();
                                    let focused = engine
                                        .focused_block()
                                        .map(|u| u.as_str().to_string())
                                        .unwrap_or_default();
                                    if clip.trim().is_empty() {
                                        paste_state.update(cx, |s, cx| {
                                            s.accept_error = Some(
                                                "clipboard is empty; copy a ticket first".into(),
                                            );
                                            cx.emit(NotifyShareUi);
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    if focused.is_empty() {
                                        paste_state.update(cx, |s, cx| {
                                            s.accept_error = Some(
                                                "no focused block; click a parent block first"
                                                    .into(),
                                            );
                                            cx.emit(NotifyShareUi);
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    dispatch_accept(
                                        session.clone(),
                                        rt_handle.clone(),
                                        paste_state.clone(),
                                        window_handle,
                                        &async_cx,
                                        focused,
                                        clip.trim().to_string(),
                                    );
                                }),
                        )
                        .child(
                            div()
                                .id("accept-modal-dismiss")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(13.0))
                                .child("Close")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_b.update(cx, |s, cx| {
                                        s.close_accept();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .when_some(inline_error_owned, |this, e| {
                    this.child(
                        div()
                            .p_2()
                            .rounded(px(4.0))
                            .bg(gpui::rgba(0x80000030))
                            .text_size(px(12.0))
                            .text_color(gpui::rgba(0xfca5a5ff))
                            .child(format!("Error: {e}")),
                    )
                }),
        )
        .into_any_element()
}

fn render_quarantine_modal(
    idx: usize,
    q: &QuarantineEvent,
    share_state: Entity<ShareUiState>,
    theme: OverlayTheme,
) -> AnyElement {
    let shared_tree_id = q.shared_tree_id.clone();
    let quarantine_path = q.quarantine_path.clone();
    let quarantine_path_copy = quarantine_path.clone();

    let red_bg: Hsla = gpui::rgba(0x7f1d1dff).into();
    let red_border: Hsla = gpui::rgba(0xef4444ff).into();
    let red_theme = OverlayTheme {
        bg: red_bg,
        border: red_border,
        fg: gpui::rgba(0xffffffff).into(),
        muted_fg: gpui::rgba(0xfecacaff).into(),
    };

    let close_state = share_state.clone();

    overlay_backdrop(&format!("quarantine-{idx}"))
        .child(
            modal_panel(&format!("quarantine-{idx}"), 600.0, red_theme)
                .text_color(red_theme.fg)
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Share snapshot could not be restored"),
                )
                .child(div().text_size(px(13.0)).child(format!(
                    "Share `{shared_tree_id}` could not be restored. Your edits before the \
                     corruption are quarantined at `{quarantine_path}`. Re-accept the ticket from \
                     the other peer to restore."
                )))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .id(SharedString::from(format!("quarantine-dismiss-{idx}")))
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0xffffff1a))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(13.0))
                                .child("Dismiss")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_state.update(cx, |s, cx| {
                                        s.dismiss_quarantine(idx);
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("quarantine-copy-path-{idx}")))
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0xffffff1a))
                                .text_size(px(13.0))
                                .child("Copy path")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        quarantine_path_copy.clone(),
                                    ));
                                }),
                        ),
                ),
        )
        .into_any_element()
}

/// The single line a toast renders — what the user actually reads.
///
/// `detail` is capped so one degradation cannot fill the window, but the cap
/// counts CHARACTERS, not bytes: `detail` carries paths and error text, and
/// cutting those mid-character panics the render. The budget is wide enough to
/// hold a disclosure's two absolute paths plus its remedy, because a toast that
/// truncates away the one actionable clause reads as complete while telling the
/// user nothing they can act on.
fn toast_message(toast: &DegradedToast) -> String {
    const MAX_DETAIL_CHARS: usize = 320;
    let (_, icon, label) = toast_style(toast.kind);
    let detail = match toast.detail.char_indices().nth(MAX_DETAIL_CHARS) {
        Some((cut, _)) => format!("{}…", &toast.detail[..cut]),
        None => toast.detail.clone(),
    };
    format!("{icon}  {label} — {detail}")
}

/// Background, icon and headline for a toast kind. Split from the render so
/// [`toast_message`] — the string the user actually reads — is testable.
fn toast_style(kind: DegradedKind) -> (gpui::Rgba, &'static str, &'static str) {
    match kind {
        DegradedKind::SnapshotSaveFailed => (gpui::rgba(0xfbbf24ff), "⚠", "Snapshot save failed"),
        DegradedKind::RehydrationFailed => (gpui::rgba(0xfbbf24ff), "↻", "Rehydration failed"),
        DegradedKind::SqlProjectionFailed => (gpui::rgba(0xfbbf24ff), "⚠", "Shared edit not shown"),
        DegradedKind::ForeignIdCollision => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "Blocked shared write (id collision)",
        ),
        DegradedKind::OrgIngestFailed => (
            gpui::rgba(0xef4444ff),
            "⚠",
            "File sync degraded (bad org file)",
        ),
        DegradedKind::UndoFailed => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "Undo/redo failed",
        ),
        DegradedKind::UndoStepDropped => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "History step dropped — that edit can no longer be undone",
        ),
        DegradedKind::CommandFailed => {
            (gpui::rgba(0xef4444ff), crate::icon("⛔"), "Command failed")
        }
        DegradedKind::PreferenceSaveFailed => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "Preference not saved",
        ),
        DegradedKind::ConnectorWritePending => (
            gpui::rgba(0xfbbf24ff),
            "⚠",
            "Connector write needs approval",
        ),
        DegradedKind::ConnectorWriteOutcomeUnknown => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "Connector write outcome unknown",
        ),
        DegradedKind::SharedSubtreeNotMaterialized => (
            gpui::rgba(0xfbbf24ff),
            "⚠",
            "Shared subtree not materialized",
        ),
        DegradedKind::WritebackDegraded => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "Edits are not reaching disk",
        ),
        DegradedKind::IntegrationConnectFailed => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "Integration unavailable",
        ),
        DegradedKind::IntegrationNeedsAuth => (
            gpui::rgba(0xef4444ff),
            crate::icon("⛔"),
            "Integration needs authorization",
        ),
        DegradedKind::IntegrationSidecarSuperseded => (
            gpui::rgba(0xfbbf24ff),
            "⚠",
            "Installed integration file ignored — using the bundled one",
        ),
        DegradedKind::IntegrationNotEnabled => (
            gpui::rgba(0xfbbf24ff),
            "⚠",
            "Integration is not switched on",
        ),
        DegradedKind::IntegrationSidecarNotBundled => (
            gpui::rgba(0xfbbf24ff),
            "⚠",
            "Integration file for a provider this build does not ship",
        ),
        DegradedKind::Info => (gpui::rgba(0x60a5faff), "i", "Info"),
    }
}

fn render_toast_stack(
    toasts: &[DegradedToast],
    share_state: Entity<ShareUiState>,
    theme: OverlayTheme,
) -> AnyElement {
    let mut stack = div()
        .absolute()
        .bottom(px(16.0))
        .right(px(16.0))
        .flex()
        .flex_col()
        .gap_2();

    for (idx, toast) in toasts.iter().enumerate() {
        let (bg_color, _, _) = toast_style(toast.kind);
        let msg = toast_message(toast);
        let close_state = share_state.clone();
        stack = stack.child(
            div()
                .id(SharedString::from(format!("toast-{idx}")))
                .px_3()
                .py_2()
                .rounded(px(6.0))
                .bg(bg_color)
                .border_1()
                .border_color(theme.border)
                .text_color(gpui::rgba(0x000000cc))
                .text_size(px(12.0))
                .min_w(px(280.0))
                .max_w(px(420.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(div().child(msg))
                .child(
                    div()
                        .id(SharedString::from(format!("toast-close-{idx}")))
                        .cursor_pointer()
                        .pl_2()
                        .child("✕")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            close_state.update(cx, |s, cx| {
                                s.dismiss_toast(idx);
                                cx.emit(NotifyShareUi);
                                cx.notify();
                            });
                        }),
                ),
        );
    }

    stack.into_any_element()
}

fn render_error_modal(
    title: &str,
    message: &str,
    share_state: Entity<ShareUiState>,
    clear: fn(&mut ShareUiState),
    theme: OverlayTheme,
) -> AnyElement {
    let title = title.to_string();
    let message = message.to_string();
    let close_state = share_state.clone();

    overlay_backdrop("share-error-modal")
        .child(
            modal_panel("share-error", 520.0, theme)
                .text_color(theme.fg)
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .p_2()
                        .rounded(px(4.0))
                        .bg(gpui::rgba(0x80000030))
                        .text_color(gpui::rgba(0xfca5a5ff))
                        .text_size(px(12.0))
                        .child(message),
                )
                .child(
                    div()
                        .id("share-error-close")
                        .cursor_pointer()
                        .px_3()
                        .py_2()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(13.0))
                        .child("Close")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            close_state.update(cx, |s, cx| {
                                clear(s);
                                cx.emit(NotifyShareUi);
                                cx.notify();
                            });
                        }),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use futures::channel::oneshot;
    use holon::sync::ShareDegraded;
    use holon::sync::ShareDegradedReason;

    use super::*;

    #[test]
    fn apply_degraded_routes_save_failed_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "abc".into(),
            reason: ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::SnapshotSaveFailed);
        assert_eq!(s.toasts[0].shared_tree_id, "abc");
        assert!(s.quarantines.is_empty());
    }

    #[test]
    fn apply_degraded_routes_load_failed_to_quarantine() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "xyz".into(),
            reason: ShareDegradedReason::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
        });
        assert!(s.toasts.is_empty());
        assert_eq!(s.quarantines.len(), 1);
        assert_eq!(s.quarantines[0].quarantine_path, "/tmp/x.corrupt-1");
    }

    /// Every degradation is now a sticky condition, so the bridge can deliver
    /// one twice — replayed in `subscription.current` and again as the live
    /// `Raised`. The quarantine modal must not stack.
    #[test]
    fn a_double_delivered_quarantine_stays_one_modal() {
        let mut s = ShareUiState::new();
        let event = ShareDegraded {
            shared_tree_id: "xyz".into(),
            reason: ShareDegradedReason::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
        };
        s.apply_degraded(event.clone());
        s.apply_degraded(event);
        assert_eq!(s.quarantines.len(), 1);
    }

    /// A boot-race degradation that a late window learns about via replay must
    /// still render — and must be clearable by key like any other condition.
    #[test]
    fn org_ingest_failed_is_a_clearable_condition() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "org-initial-scan".into(),
            reason: ShareDegradedReason::OrgIngestFailed("notes.org: unparseable".into()),
        });
        assert_eq!(s.toasts.len(), 1);
        let key = s.toasts[0]
            .condition
            .clone()
            .expect("every bus-sourced toast carries its condition key");
        s.apply_degraded_cleared(&key);
        assert!(s.toasts.is_empty());
    }

    #[test]
    fn apply_degraded_routes_rehydration_failed_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "r".into(),
            reason: ShareDegradedReason::RehydrationFailed("endpoint".into()),
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::RehydrationFailed);
    }

    /// `dispatch_undo`/`dispatch_redo` (below) route a genuine engine `Err`
    /// through exactly this `push_toast` call — this pins the toast-kind
    /// plumbing on its own (undo/redo never gets a `ShareDegraded` broadcast
    /// event, so it can't go through `apply_degraded` like the other kinds).
    #[test]
    fn undo_failed_toast_is_pushed_and_bounded_like_other_kinds() {
        let mut s = ShareUiState::new();
        s.push_toast(DegradedToast {
            kind: DegradedKind::UndoFailed,
            shared_tree_id: "undo".into(),
            detail: "undo: this operation requires an operation engine, which is not wired in \
                     this (no-Turso) session"
                .into(),
            condition: None,
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::UndoFailed);
        assert!(s.toasts[0].detail.contains("operation engine"));
    }

    /// A dropped history entry is a data-trust event: the step is gone for
    /// good. Its disclosure must lead with the loss, carry the engine's
    /// verbatim reason, and must NOT reuse the generic "undo/redo failed" kind
    /// — a user who reads that assumes the press simply did not work and
    /// presses again. This drives the ROUTING the dispatcher uses, so changing
    /// the kind at the routing site (not just the words) reds here.
    #[test]
    fn stale_dropped_entry_routes_to_its_own_kind_and_discloses_the_lost_step() {
        let reason = "state changed under undo: block:5260462b.content expected String(\"second \
                      lin\") but found Some(String(\"second li\"))";
        let outcome: Result<Result<holon_api::UndoOutcome, String>, oneshot::Canceled> =
            Ok(Ok(holon_api::UndoOutcome::StaleDropped {
                reason: reason.to_string(),
            }));

        let d = super::undo_disclosure("undo", &outcome).expect("a dropped step must disclose");
        assert_eq!(
            d.kind,
            DegradedKind::UndoStepDropped,
            "a lost step must not share a toast kind with a failed press"
        );
        assert!(
            d.detail.contains("can no longer be undone"),
            "the disclosure must name the loss, got {:?}",
            d.detail
        );
        assert!(
            d.detail.contains(reason),
            "the disclosure must carry the engine's reason verbatim, got {:?}",
            d.detail
        );
        assert!(!d.warn_only, "a lost step is an error, not a warning");

        // …and the routed disclosure is what reaches the toast stack.
        let mut s = ShareUiState::new();
        s.push_toast(DegradedToast {
            kind: d.kind,
            shared_tree_id: "undo".into(),
            detail: d.detail,
            condition: None,
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::UndoStepDropped);
    }

    /// The arms that must stay silent, and the arms that must not: a press that
    /// did its job says nothing, everything else discloses. Without this,
    /// moving an outcome into the silent arm would go unnoticed.
    #[test]
    fn only_a_press_that_did_its_job_stays_silent() {
        let silent: Vec<Result<Result<holon_api::UndoOutcome, String>, oneshot::Canceled>> = vec![
            Ok(Ok(holon_api::UndoOutcome::Applied)),
            Ok(Ok(holon_api::UndoOutcome::Empty)),
        ];
        for outcome in &silent {
            assert!(
                super::undo_disclosure("undo", outcome).is_none(),
                "{outcome:?} must not toast"
            );
        }

        let loud: Vec<Result<Result<holon_api::UndoOutcome, String>, oneshot::Canceled>> = vec![
            Ok(Ok(holon_api::UndoOutcome::NoChange)),
            Ok(Err("no operation engine wired".to_string())),
            Err(oneshot::Canceled),
        ];
        for outcome in &loud {
            let d = super::undo_disclosure("undo", outcome)
                .unwrap_or_else(|| panic!("{outcome:?} must disclose"));
            assert_eq!(
                d.kind,
                DegradedKind::UndoFailed,
                "{outcome:?} is a failed press, not a lost step"
            );
        }
    }

    #[test]
    fn apply_degraded_routes_integration_connect_failed_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "todoist".into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: "todoist".into(),
                error: "No such file or directory (os error 2)".into(),
            },
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::IntegrationConnectFailed);
        assert!(
            s.toasts[0].detail.contains("todoist"),
            "detail must name the integration: {}",
            s.toasts[0].detail
        );
        assert!(
            s.toasts[0].detail.contains("os error 2"),
            "detail must carry the connect error: {}",
            s.toasts[0].detail
        );
        assert!(s.quarantines.is_empty());
    }

    /// The boot seam: the bus raises integration conditions during boot DI, so
    /// the window's bridge learns them from the subscription's replayed
    /// `current` — that replay must render a toast just like a live event.
    #[test]
    fn replayed_boot_condition_renders_a_toast() {
        let bus = holon::sync::DegradedSignalBus::new();
        bus.emit(ShareDegraded {
            shared_tree_id: "todoist".into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: "todoist".into(),
                error: "No such file or directory (os error 2)".into(),
            },
        });

        let mut s = ShareUiState::new();
        for event in bus.subscribe().current {
            s.apply_degraded(event);
        }

        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::IntegrationConnectFailed);
        assert!(s.toasts[0].detail.contains("todoist"));
    }

    /// `subscribe` may deliver a condition twice (replayed `current` + a live
    /// `Raised` racing it). That must upsert, not stack two banners.
    #[test]
    fn replayed_then_live_duplicate_yields_one_toast() {
        let event = ShareDegraded {
            shared_tree_id: "todoist".into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: "todoist".into(),
                error: "os error 2".into(),
            },
        };
        let mut s = ShareUiState::new();
        s.apply_degraded(event.clone());
        s.apply_degraded(event);
        assert_eq!(s.toasts.len(), 1);
    }

    #[test]
    fn cleared_condition_removes_its_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "todoist".into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: "todoist".into(),
                error: "os error 2".into(),
            },
        });
        // A transient toast alongside it must survive the clear.
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "share".into(),
            reason: ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
        });
        assert_eq!(s.toasts.len(), 2);

        s.apply_degraded_cleared(&DegradedConditionKey {
            subject: "todoist".into(),
            kind: "integration-connect-failed",
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::SnapshotSaveFailed);
    }

    /// The toast body truncates `detail` at 80 chars, so the integration name
    /// must come first or a long error hides it.
    #[test]
    fn integration_connect_failed_detail_leads_with_the_integration_name() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "todoist".into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: "todoist".into(),
                error: "x".repeat(200),
            },
        });
        assert!(s.toasts[0].detail[..80].contains("todoist"));
    }

    #[test]
    fn apply_degraded_routes_integration_needs_auth_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "linear".into(),
            reason: ShareDegradedReason::IntegrationNeedsAuth {
                integration: "linear".into(),
                auth_url: "https://linear.app/oauth/authorize?x=1".into(),
            },
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::IntegrationNeedsAuth);
        assert!(s.toasts[0].detail.contains("linear"));
        assert!(s.toasts[0].detail.contains("https://linear.app/oauth"));
    }

    /// The supersede toast is the ONLY place a user learns that the file they
    /// installed is not the one running, so its detail must carry all four
    /// facts needed to act: which provider, which file was ignored, why, and
    /// what ran instead.
    #[test]
    fn apply_degraded_routes_sidecar_superseded_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "claude-history".into(),
            reason: ShareDegradedReason::IntegrationSidecarSuperseded {
                integration: "claude-history".into(),
                installed_path: "/home/u/.config/holon/integrations/claude-history.yaml".into(),
                bundled_source: "assets/integrations/claude-history.yaml".into(),
                incompatibility: "it declares schema_version none but this build's sidecar format \
                                  is schema_version 1"
                    .into(),
            },
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::IntegrationSidecarSuperseded);
        let detail = &s.toasts[0].detail;
        assert!(detail.contains("claude-history"), "provider: {detail}");
        assert!(
            detail.contains("/home/u/.config/holon/integrations/claude-history.yaml"),
            "installed path: {detail}"
        );
        assert!(
            detail.contains("schema_version"),
            "incompatibility: {detail}"
        );
        assert!(
            detail.contains("assets/integrations/claude-history.yaml"),
            "bundled source: {detail}"
        );
        assert!(s.quarantines.is_empty());
    }

    /// A pre-cutover setup — sidecar copied into the integrations directory, no
    /// state file — is the case where the user is most certain the integration
    /// is on. The toast is the only thing that says otherwise, so it must name
    /// the file to write, not just report that something is off.
    #[test]
    fn apply_degraded_routes_integration_not_enabled_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "gcal".into(),
            reason: ShareDegradedReason::IntegrationNotEnabled {
                integration: "gcal".into(),
                installed_path: "/home/u/.config/holon/integrations/gcal.yaml".into(),
                state_path: "/home/u/.config/holon/integrations/gcal.state.toml".into(),
                remedy: "scripts/holon-integration-enable.sh gcal".into(),
            },
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::IntegrationNotEnabled);
        let detail = &s.toasts[0].detail;
        assert!(detail.contains("gcal"), "provider: {detail}");
        assert!(
            detail.contains("/home/u/.config/holon/integrations/gcal.state.toml"),
            "the file to write: {detail}"
        );
        assert!(
            detail.contains("scripts/holon-integration-enable.sh gcal"),
            "the remedy: {detail}"
        );
    }

    /// D1. The toast the user SEES is the truncated one. Martin's own paths are
    /// long enough that a cap applied to the whole detail eats the remedy,
    /// which is the only part he has to act on.
    #[test]
    fn the_rendered_not_enabled_toast_still_carries_the_remedy() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "gcal".into(),
            reason: ShareDegradedReason::IntegrationNotEnabled {
                integration: "gcal".into(),
                installed_path: "/Users/martin/.config/holon/integrations/gcal.yaml".into(),
                state_path: "/Users/martin/.config/holon/integrations/gcal.state.toml".into(),
                remedy: "scripts/holon-integration-enable.sh gcal".into(),
            },
        });
        let rendered = toast_message(&s.toasts[0]);
        assert!(
            rendered.contains("/Users/martin/.config/holon/integrations/gcal.state.toml"),
            "the rendered toast must name the file to write: {rendered}"
        );
    }

    /// D2. A remedy that does not work is worse than none: it reads as
    /// complete. The state file needs `schema_version` and `configuration`
    /// too, so the toast must point at the command that writes a whole one.
    #[test]
    fn the_rendered_remedy_is_one_that_actually_works() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "gcal".into(),
            reason: ShareDegradedReason::IntegrationNotEnabled {
                integration: "gcal".into(),
                installed_path: "/Users/martin/.config/holon/integrations/gcal.yaml".into(),
                state_path: "/Users/martin/.config/holon/integrations/gcal.state.toml".into(),
                remedy: "scripts/holon-integration-enable.sh gcal".into(),
            },
        });
        let rendered = toast_message(&s.toasts[0]);
        assert!(
            !rendered.contains("write `enabled = true`"),
            "a bare `enabled = true` file is REJECTED by the state parser, so this \
             advice produces a broken integration: {rendered}"
        );
        assert!(
            rendered.contains("scripts/holon-integration-enable.sh gcal"),
            "the remedy the bus carries must reach the user verbatim: {rendered}"
        );
    }

    /// D6. `detail` is user data — paths, error text, an em dash. Truncating it
    /// by BYTES splits a multi-byte character and panics the render.
    #[test]
    fn no_detail_length_can_panic_the_render() {
        // The sweep has to move the cut ACROSS a multi-byte character, which
        // means varying a field the format places BEFORE one — varying a
        // trailing field slides only ASCII past the boundary and the sweep
        // proves nothing. Two shapes: a multi-byte char the format itself
        // contributes (the em dash after `state_path`), and multi-byte content
        // inside the varied field, which is the general case since these fields
        // are user data. The range spans the cap from both sides.
        for len in 1..400 {
            for state_path in [&"s".repeat(len), &"é".repeat(len)] {
                let mut s = ShareUiState::new();
                s.apply_degraded(ShareDegraded {
                    shared_tree_id: "gcal".into(),
                    reason: ShareDegradedReason::IntegrationNotEnabled {
                        integration: "gcal".into(),
                        installed_path: "/p/gcal.yaml".into(),
                        state_path: state_path.clone(),
                        remedy: "scripts/holon-integration-enable.sh gcal".into(),
                    },
                });
                let rendered = toast_message(&s.toasts[0]);
                assert!(!rendered.is_empty(), "empty render at state length {len}");
            }
        }
    }

    /// A sidecar for a provider the build does not ship must not look like a
    /// broken integration — it is not an integration at all.
    #[test]
    fn apply_degraded_routes_sidecar_not_bundled_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "my-own-thing".into(),
            reason: ShareDegradedReason::IntegrationSidecarNotBundled {
                provider: "my-own-thing".into(),
                installed_path: "/home/u/.config/holon/integrations/my-own-thing.yaml".into(),
            },
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::IntegrationSidecarNotBundled);
        let detail = &s.toasts[0].detail;
        assert!(
            detail.contains("/home/u/.config/holon/integrations/my-own-thing.yaml"),
            "the file that does nothing: {detail}"
        );
    }

    #[test]
    fn toast_stack_bounded_to_five() {
        let mut s = ShareUiState::new();
        for i in 0..8 {
            s.apply_degraded(ShareDegraded {
                shared_tree_id: format!("s{i}"),
                reason: ShareDegradedReason::SnapshotSaveFailed(format!("err{i}")),
            });
        }
        assert_eq!(s.toasts.len(), 5);
        // FIFO eviction: the first three were dropped.
        assert_eq!(s.toasts[0].shared_tree_id, "s3");
        assert_eq!(s.toasts[4].shared_tree_id, "s7");
    }

    #[test]
    fn ticket_parses_from_json_response() {
        let json = serde_json::json!({
            "ticket": "base64-ticket",
            "shared_tree_id": "share-1",
            "mount_block_id": "block:mount-1",
            "shared_root": "42:7",
        });
        let v = Value::String(json.to_string());
        let t = ShareTicket::from_value(&v).unwrap();
        assert_eq!(t.ticket, "base64-ticket");
        assert_eq!(t.shared_tree_id, "share-1");
        assert_eq!(t.mount_block_id, "block:mount-1");
    }

    #[test]
    fn ticket_parse_reports_missing_field() {
        let v = Value::String(r#"{"ticket":"x"}"#.to_string());
        let err = ShareTicket::from_value(&v).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("shared_tree_id"), "msg={msg}");
    }

    #[test]
    fn ticket_parse_rejects_non_string() {
        let v = Value::Integer(42);
        let err = ShareTicket::from_value(&v).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not a String"), "msg={msg}");
    }
}
