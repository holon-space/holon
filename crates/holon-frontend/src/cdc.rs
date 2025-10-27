use holon_api::reactive::{UiEventResult, UiState};
use holon_api::widget_spec::WidgetSpec;
use holon_api::{UiEvent, WatchHandle};
use tokio::sync::watch;

// ── AppState: watch-channel based reactive state ────────────────────────────

/// Reactive frontend state backed by a `tokio::sync::watch` channel.
///
/// The CDC listener (tokio task) owns `UiState` exclusively and sends
/// `WidgetSpec` snapshots through a `watch` channel. The render thread
/// reads via `AppState` — no `RwLock`, no dirty flag, no contention.
///
/// # Usage patterns
///
/// **Polling frameworks** (macroquad, r3bl, blinc):
/// ```ignore
/// loop {
///     let spec = app_state.widget_spec();  // always latest, never blocks
///     render(spec);
/// }
/// ```
///
/// **Callback frameworks** (GPUI, WaterUI):
/// ```ignore
/// let mut state = app_state.clone();
/// tokio::spawn(async move {
///     while state.changed().await {
///         refresh(cx);  // signal framework to re-render
///     }
/// });
/// ```
#[derive(Clone)]
pub struct AppState {
    rx: watch::Receiver<WidgetSpec>,
}

impl AppState {
    /// Get the latest WidgetSpec snapshot (non-blocking).
    pub fn widget_spec(&self) -> WidgetSpec {
        self.rx.borrow().clone()
    }

    /// Wait until the state changes. Returns `false` if the stream closed.
    ///
    /// After this returns `true`, call `widget_spec()` to get the new state.
    pub async fn changed(&mut self) -> bool {
        self.rx.changed().await.is_ok()
    }
}

/// Spawn a CDC listener that consumes `UiEvent`s from a `WatchHandle` and
/// produces `WidgetSpec` snapshots via a `watch` channel.
///
/// Returns an `AppState` handle for the render thread. The background task
/// owns `UiState` exclusively — no shared mutable state.
///
/// The `WatchHandle` is moved into the spawned task, keeping both the event
/// receiver and command sender alive for the lifetime of the listener.
pub fn spawn_ui_listener(watch_handle: WatchHandle) -> AppState {
    let initial = WidgetSpec::from_rows(vec![]);
    let (tx, rx) = watch::channel(initial);

    tokio::spawn(async move {
        ui_event_loop(watch_handle, tx).await;
    });

    AppState { rx }
}

/// Core event loop: receive UiEvents, apply to UiState, send snapshots.
///
/// Separated from `spawn_ui_listener` so it can be called directly by
/// frameworks that manage their own task spawning (e.g., GPUI's `cx.spawn`).
pub async fn ui_event_loop(mut watch: WatchHandle, tx: watch::Sender<WidgetSpec>) {
    let mut ui_state = UiState::new(WidgetSpec::from_rows(vec![]));

    while let Some(event) = watch.recv().await {
        match &event {
            UiEvent::Structure {
                generation,
                widget_spec,
            } => {
                tracing::info!(
                    generation,
                    rows = widget_spec.data.len(),
                    "Structural update received"
                );
            }
            UiEvent::Data { .. } | UiEvent::CollectionUpdate { .. } => {}
        }

        match ui_state.apply_event(event) {
            UiEventResult::StructureChanged(spec) => {
                let _ = tx.send(spec);
            }
            UiEventResult::DataChanged => {
                let _ = tx.send(ui_state.to_widget_spec());
            }
            UiEventResult::NoChange => {}
        }
    }
    tracing::info!("UiEvent stream ended");
}

// ── CdcState: callback-based variant (for WaterUI, Dioxus) ─────────────────

/// CDC state with a callback notification mechanism.
///
/// Used by WaterUI (mailbox callback) and Dioxus (watch channel sender).
/// For new frontends, prefer `spawn_ui_listener` / `AppState` instead.
pub struct CdcState {
    ui_state: UiState,
    notify: Box<dyn Fn(WidgetSpec) + Send>,
}

impl CdcState {
    pub fn new(initial: WidgetSpec, notify: impl Fn(WidgetSpec) + Send + 'static) -> Self {
        Self {
            ui_state: UiState::new(initial),
            notify: Box::new(notify),
        }
    }

    pub fn generation(&self) -> u64 {
        self.ui_state.generation()
    }

    fn emit(&self) {
        (self.notify)(self.ui_state.to_widget_spec());
    }

    pub fn replace_widget_spec(&mut self, widget_spec: WidgetSpec, generation: u64) {
        self.ui_state.apply_event(UiEvent::Structure {
            widget_spec,
            generation,
        });
        self.emit();
    }

    pub fn apply_data_batch(
        &mut self,
        changes: impl IntoIterator<Item = holon_api::streaming::Change<holon_api::widget_spec::DataRow>>,
    ) {
        use holon_api::streaming::{Batch, BatchMetadata, WithMetadata};
        let batch = WithMetadata {
            inner: Batch {
                items: changes.into_iter().collect(),
            },
            metadata: BatchMetadata {
                relation_name: String::new(),
                trace_context: None,
                sync_token: None,
            },
        };
        let result = self.ui_state.apply_event(UiEvent::Data {
            batch,
            generation: self.ui_state.generation(),
        });
        if matches!(result, UiEventResult::DataChanged) {
            self.emit();
        }
    }
}
