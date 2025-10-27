//! Reactive stream operators and CDC diff types.
//!
//! Provides composable stream operators (`scan_state`, `switch_map`,
//! `combine_latest`, `coalesce`) built on tokio channels, plus collection
//! diff types (`MapDiff`) inspired by futures-signals.
//!
//! # Design
//!
//! - **Transport**: tokio `mpsc` channels (backpressure via bounded channels)
//! - **Operators**: extension trait on `Stream` — compose with `.scan_state()`,
//!   `.switch_map()`, etc. Each operator spawns a tokio task internally.
//! - **Diff types**: `MapDiff<K, V>` for keyed collection deltas (CDC → UI)
//! - **WASM note**: tokio channels would need replacing for WASM targets;
//!   the diff types and operator signatures are runtime-agnostic.

use std::collections::HashMap;
use std::hash::Hash;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// Fire-and-forget task spawn. On native and wasi-threads (holon-worker)
/// uses `tokio::spawn`. On wasm32-unknown-unknown (dioxus-web's browser
/// build) uses `wasm_bindgen_futures::spawn_local` because there is no
/// tokio runtime — only Dioxus's `spawn_local` driver.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn spawn_actor<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(future);
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn spawn_actor<F>(future: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
}

// Re-export Stream from tokio_stream (which re-exports futures_core::Stream)
pub use tokio_stream::Stream;

use crate::RenderExpr;

// ── Diff types ──────────────────────────────────────────────────────────────

/// Incremental diff for a keyed collection (HashMap-like).
///
/// Directly maps to CDC semantics: Insert/Update/Remove correspond to
/// Created/Updated/Deleted change events. `Replace` is used for initial
/// snapshot delivery.
#[derive(Debug, Clone)]
pub enum MapDiff<K, V> {
    /// Replace entire collection (initial snapshot).
    Replace(HashMap<K, V>),
    /// Insert a new entry.
    Insert { key: K, value: V },
    /// Update an existing entry.
    Update { key: K, value: V },
    /// Remove an entry by key.
    Remove { key: K },
    /// Clear all entries.
    Clear,
}

/// Apply a `MapDiff` to a `HashMap`.
///
/// This is the pure reduction function used by `CdcAccumulator` and frontends.
pub fn apply_map_diff<K, V>(state: &mut HashMap<K, V>, diff: MapDiff<K, V>)
where
    K: Eq + Hash,
{
    match diff {
        MapDiff::Replace(new_state) => *state = new_state,
        MapDiff::Insert { key, value } | MapDiff::Update { key, value } => {
            state.insert(key, value);
        }
        MapDiff::Remove { key } => {
            state.remove(&key);
        }
        MapDiff::Clear => state.clear(),
    }
}

// ── Stream operator receivers ───────────────────────────────────────────────

/// Stream wrapper around an `mpsc::Receiver`. Used as the return type for all
/// spawn-based operators (`scan_state`, `switch_map`, `combine_latest`, `coalesce`).
pub struct OperatorStream<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> Stream for OperatorStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.rx.poll_recv(cx)
    }
}

// ── Extension trait ─────────────────────────────────────────────────────────

/// Extension trait adding reactive operators to any `Stream`.
pub trait ReactiveStreamExt: Stream + Sized {
    /// Accumulate state from a stream of events (like Redux `reduce` / Rx `scan`).
    ///
    /// Emits a clone of the state after each event. For large state, wrap in
    /// `Arc` or emit diffs downstream instead of snapshots.
    fn scan_state<S, F>(self, initial: S, f: F) -> OperatorStream<S>
    where
        Self: Send + Unpin + 'static,
        Self::Item: Send + 'static,
        S: Clone + Send + 'static,
        F: FnMut(&mut S, Self::Item) + Send + 'static,
    {
        scan_state_spawn(self, initial, f)
    }

    /// When the outer stream emits, subscribe to a new inner stream and drop
    /// the previous one. Equivalent to RxJS `switchMap`.
    ///
    /// Automatically aborts the previous inner stream's forwarder task when a
    /// new outer event arrives — no manual `AbortHandle` management needed.
    fn switch_map<U, F, Inner>(self, f: F) -> OperatorStream<U>
    where
        Self: Send + 'static,
        Self::Item: Send + 'static,
        U: Send + 'static,
        F: FnMut(Self::Item) -> Inner + Send + 'static,
        Inner: Stream<Item = U> + Send + Unpin + 'static,
    {
        switch_map_spawn(self, f)
    }
}

impl<T: Stream + Sized> ReactiveStreamExt for T {}

// ── scan_state (spawn-based) ────────────────────────────────────────────────

fn scan_state_spawn<S, T, State, F>(source: S, initial: State, mut f: F) -> OperatorStream<State>
where
    S: Stream<Item = T> + Send + Unpin + 'static,
    T: Send + 'static,
    State: Clone + Send + 'static,
    F: FnMut(&mut State, T) + Send + 'static,
{
    let (tx, rx) = mpsc::channel(64);

    spawn_actor(async move {
        tokio::pin!(source);
        let mut state = initial;
        while let Some(item) = source.next().await {
            f(&mut state, item);
            if tx.send(state.clone()).await.is_err() {
                break;
            }
        }
    });

    OperatorStream { rx }
}

// ── switch_map ──────────────────────────────────────────────────────────────

fn switch_map_spawn<S, U, F, Inner>(source: S, mut f: F) -> OperatorStream<U>
where
    S: Stream + Send + 'static,
    S::Item: Send + 'static,
    U: Send + 'static,
    F: FnMut(S::Item) -> Inner + Send + 'static,
    Inner: Stream<Item = U> + Send + Unpin + 'static,
{
    use futures::future::{AbortHandle, Abortable};

    let (tx, rx) = mpsc::channel(64);

    spawn_actor(async move {
        tokio::pin!(source);
        let mut current_abort: Option<AbortHandle> = None;

        while let Some(outer_item) = source.next().await {
            if let Some(handle) = current_abort.take() {
                handle.abort();
            }

            let inner = f(outer_item);
            let tx = tx.clone();
            let (abort_handle, abort_reg) = AbortHandle::new_pair();
            current_abort = Some(abort_handle);
            spawn_actor(async move {
                let _ = Abortable::new(
                    async move {
                        tokio::pin!(inner);
                        while let Some(inner_item) = inner.next().await {
                            if tx.send(inner_item).await.is_err() {
                                break;
                            }
                        }
                    },
                    abort_reg,
                )
                .await;
            });
        }

        if let Some(handle) = current_abort.take() {
            handle.abort();
        }
    });

    OperatorStream { rx }
}

// ── combine_latest ──────────────────────────────────────────────────────────

/// Combine two streams, emitting whenever either updates, using the latest
/// value from both. Equivalent to RxJS `combineLatest`.
///
/// Does not emit until both streams have produced at least one value.
pub fn combine_latest<A, B, C, SA, SB, F>(stream_a: SA, stream_b: SB, f: F) -> OperatorStream<C>
where
    A: Send + Clone + 'static,
    B: Send + Clone + 'static,
    C: Send + 'static,
    SA: Stream<Item = A> + Send + Unpin + 'static,
    SB: Stream<Item = B> + Send + Unpin + 'static,
    F: Fn(&A, &B) -> C + Send + 'static,
{
    let (tx, rx) = mpsc::channel(64);

    spawn_actor(async move {
        tokio::pin!(stream_a);
        tokio::pin!(stream_b);

        let mut latest_a: Option<A> = None;
        let mut latest_b: Option<B> = None;

        loop {
            tokio::select! {
                maybe_a = stream_a.next() => {
                    match maybe_a {
                        Some(a) => {
                            latest_a = Some(a);
                            if let (Some(a), Some(b)) = (&latest_a, &latest_b) {
                                if tx.send(f(a, b)).await.is_err() { break; }
                            }
                        }
                        None => break,
                    }
                }
                maybe_b = stream_b.next() => {
                    match maybe_b {
                        Some(b) => {
                            latest_b = Some(b);
                            if let (Some(a), Some(b)) = (&latest_a, &latest_b) {
                                if tx.send(f(a, b)).await.is_err() { break; }
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    OperatorStream { rx }
}

// ── coalesce ────────────────────────────────────────────────────────────────

/// Buffer items for up to `window` duration, then apply a coalescing function
/// to the batch before emitting.
///
/// Useful for CDC pipelines where DELETE+INSERT pairs should be merged into
/// UPDATE before forwarding to the UI.
pub fn coalesce<T, S, F>(source: S, window: Duration, mut merge: F) -> OperatorStream<T>
where
    T: Send + 'static,
    S: Stream<Item = T> + Send + Unpin + 'static,
    F: FnMut(Vec<T>) -> Vec<T> + Send + 'static,
{
    let (tx, rx) = mpsc::channel(64);

    spawn_actor(async move {
        tokio::pin!(source);
        let mut buffer: Vec<T> = Vec::new();

        loop {
            if buffer.is_empty() {
                match source.next().await {
                    Some(item) => buffer.push(item),
                    None => break,
                }
            }

            let deadline = tokio::time::Instant::now() + window;
            loop {
                tokio::select! {
                    maybe_item = source.next() => {
                        match maybe_item {
                            Some(item) => buffer.push(item),
                            None => {
                                if !buffer.is_empty() {
                                    let coalesced = merge(std::mem::take(&mut buffer));
                                    for item in coalesced {
                                        if tx.send(item).await.is_err() { return; }
                                    }
                                }
                                return;
                            }
                        }
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        break;
                    }
                }
            }

            let coalesced = merge(std::mem::take(&mut buffer));
            for item in coalesced {
                if tx.send(item).await.is_err() {
                    return;
                }
            }
        }
    });

    OperatorStream { rx }
}

// ── CDC accumulator ─────────────────────────────────────────────────────────

/// Unified CDC state accumulator for frontends.
///
/// Replaces the duplicated match-on-Change logic across frontend `cdc.rs` files.
/// Maintains a `HashMap<String, V>` keyed by entity ID and applies diffs
/// incrementally.
pub struct CdcAccumulator<V> {
    state: HashMap<String, V>,
}

impl<V> CdcAccumulator<V> {
    pub fn new() -> Self {
        Self {
            state: HashMap::new(),
        }
    }

    pub fn from_initial(items: impl IntoIterator<Item = (String, V)>) -> Self {
        Self {
            state: items.into_iter().collect(),
        }
    }

    pub fn state(&self) -> &HashMap<String, V> {
        &self.state
    }

    pub fn into_state(self) -> HashMap<String, V> {
        self.state
    }

    pub fn apply_diff(&mut self, diff: MapDiff<String, V>) {
        apply_map_diff(&mut self.state, diff);
    }
}

impl CdcAccumulator<crate::widget_spec::DataRow> {
    /// Build from initial query result rows (extracts "id" column as key).
    pub fn from_rows(rows: Vec<crate::widget_spec::DataRow>) -> Self {
        let mut state = HashMap::with_capacity(rows.len());
        for row in rows {
            if let Some(id) = row.get("id").and_then(|v| v.as_string()) {
                state.insert(id.to_string(), row);
            }
        }
        Self { state }
    }

    /// Convert a `Change<DataRow>` into a `MapDiff<String, DataRow>`.
    ///
    /// Single source of truth for CDC-to-diff conversion, replacing duplicated
    /// match arms across 5 frontend cdc.rs files.
    pub fn change_to_diff(
        change: crate::Change<crate::widget_spec::DataRow>,
    ) -> MapDiff<String, crate::widget_spec::DataRow> {
        match change {
            crate::Change::Created { data, .. } => {
                let key = data
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("Created event must have 'id' column")
                    .to_string();
                MapDiff::Insert { key, value: data }
            }
            crate::Change::Updated { id, data, .. } => MapDiff::Update {
                key: id,
                value: data,
            },
            crate::Change::Deleted { id, .. } => MapDiff::Remove { key: id },
            crate::Change::FieldsChanged {
                entity_id, fields, ..
            } => {
                let mut patch = crate::widget_spec::DataRow::new();
                patch.insert("id".to_string(), crate::Value::String(entity_id.clone()));
                for (name, _old, new) in fields {
                    patch.insert(name, new);
                }
                MapDiff::Update {
                    key: entity_id,
                    value: patch,
                }
            }
        }
    }

    /// Apply a `Change<DataRow>` directly.
    pub fn apply_change(&mut self, change: crate::Change<crate::widget_spec::DataRow>) {
        let diff = Self::change_to_diff(change);
        // FieldsChanged produces a partial Update — patch in-place if row exists
        if let MapDiff::Update { ref key, ref value } = diff {
            if let Some(existing) = self.state.get_mut(key) {
                for (k, v) in value {
                    existing.insert(k.clone(), v.clone());
                }
                return;
            }
        }
        self.apply_diff(diff);
    }

    /// Apply a batch of changes.
    pub fn apply_batch(
        &mut self,
        changes: impl IntoIterator<Item = crate::Change<crate::widget_spec::DataRow>>,
    ) {
        for change in changes {
            self.apply_change(change);
        }
    }

    /// Export current state as a Vec<DataRow> (for WidgetSpec construction).
    pub fn to_vec(&self) -> Vec<crate::widget_spec::DataRow> {
        self.state.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.state.len()
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.state.contains_key(key)
    }
}

// ── UiEvent application ─────────────────────────────────────────────────────

/// Unified state container for frontends that receive `UiEvent` streams.
///
/// Replaces the duplicated `cdc.rs` + `state.rs` pattern across 5 frontends.
/// Manages render expression, accumulated data rows, and generation tracking.
pub struct UiState {
    render_expr: RenderExpr,
    data: CdcAccumulator<crate::widget_spec::DataRow>,
    generation: u64,
}

/// Result of applying a UiEvent to UiState.
pub enum UiEventResult {
    /// Structure changed — frontend should re-render entirely with the new RenderExpr.
    StructureChanged(RenderExpr),
    /// Data changed — frontend should update rows in place.
    DataChanged,
    /// Event was stale or irrelevant — no action needed.
    NoChange,
}

impl UiState {
    pub fn new(initial: RenderExpr) -> Self {
        Self {
            render_expr: initial,
            data: CdcAccumulator::from_rows(vec![]),
            generation: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Current render expression.
    pub fn render_expr(&self) -> RenderExpr {
        self.render_expr.clone()
    }

    /// Current data rows (for rendering).
    pub fn data(&self) -> &HashMap<String, crate::widget_spec::DataRow> {
        self.data.state()
    }

    /// Apply a UiEvent. Returns what changed so the frontend can decide
    /// how to update its UI framework.
    pub fn apply_event(&mut self, event: crate::UiEvent) -> UiEventResult {
        match event {
            crate::UiEvent::Structure {
                render_expr,
                candidates: _,
                generation,
            } => {
                self.render_expr = render_expr.clone();
                self.data = CdcAccumulator::from_rows(vec![]);
                self.generation = generation;
                UiEventResult::StructureChanged(render_expr)
            }
            crate::UiEvent::Data { batch, generation } => {
                if self.generation != generation {
                    return UiEventResult::NoChange;
                }
                self.data.apply_batch(batch.inner.items);
                UiEventResult::DataChanged
            }
        }
    }
}

// ── Convenience: materialize a MapDiff stream ───────────────────────────────

/// Materialize a stream of `MapDiff`s into a stream of full snapshots.
///
/// Each emission is the complete `HashMap` after applying the latest diff.
/// Equivalent to `source.scan_state(HashMap::new(), apply_map_diff)`.
pub fn materialize_map<K, V, S>(source: S) -> OperatorStream<HashMap<K, V>>
where
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Send + 'static,
    S: Stream<Item = MapDiff<K, V>> + Send + Unpin + 'static,
{
    source.scan_state(HashMap::new(), |state, diff| apply_map_diff(state, diff))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::wrappers::ReceiverStream;

    #[test]
    fn test_map_diff_apply() {
        let mut state = HashMap::new();
        apply_map_diff(&mut state, MapDiff::Insert { key: "a", value: 1 });
        assert_eq!(state.get("a"), Some(&1));

        apply_map_diff(&mut state, MapDiff::Update { key: "a", value: 2 });
        assert_eq!(state.get("a"), Some(&2));

        apply_map_diff(&mut state, MapDiff::Remove { key: "a" });
        assert!(state.is_empty());
    }

    #[test]
    fn test_map_diff_replace() {
        let mut state = HashMap::from([("old", 1)]);
        let new = HashMap::from([("new", 2)]);
        apply_map_diff(&mut state, MapDiff::Replace(new.clone()));
        assert_eq!(state, new);
    }

    #[tokio::test]
    async fn test_scan_state() {
        let (tx, rx) = mpsc::channel(16);
        let stream = ReceiverStream::new(rx);

        let mut scanned = stream.scan_state(0i32, |state, item: i32| *state += item);

        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap();
        drop(tx);

        assert_eq!(scanned.next().await, Some(1));
        assert_eq!(scanned.next().await, Some(3));
        assert_eq!(scanned.next().await, Some(6));
        assert_eq!(scanned.next().await, None);
    }

    #[tokio::test]
    async fn test_switch_map_uses_latest_inner() {
        let (outer_tx, outer_rx) = mpsc::channel::<u32>(16);
        let outer = ReceiverStream::new(outer_rx);

        // Inner streams: each sends a single tagged value after a short delay
        let mut switched = outer.switch_map(|n| {
            let (tx, rx) = mpsc::channel(16);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = tx.send(n * 100).await;
            });
            ReceiverStream::new(rx)
        });

        // Send one outer value and wait for its inner result
        outer_tx.send(1).await.unwrap();
        let first = switched.next().await;
        assert_eq!(first, Some(100));

        // Send another — should get result from the new inner
        outer_tx.send(2).await.unwrap();
        let second = switched.next().await;
        assert_eq!(second, Some(200));

        drop(outer_tx);
        assert_eq!(switched.next().await, None);
    }

    #[tokio::test]
    async fn test_combine_latest_waits_for_both() {
        let (tx_a, rx_a) = mpsc::channel(16);
        let (tx_b, rx_b) = mpsc::channel(16);

        let mut combined = combine_latest(
            ReceiverStream::new(rx_a),
            ReceiverStream::new(rx_b),
            |a: &i32, b: &i32| (*a, *b),
        );

        tx_a.send(1).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // B arrives — now both have values
        tx_b.send(10).await.unwrap();
        assert_eq!(combined.next().await, Some((1, 10)));

        // Update A — uses latest B
        tx_a.send(2).await.unwrap();
        assert_eq!(combined.next().await, Some((2, 10)));

        drop(tx_a);
        drop(tx_b);
    }

    #[tokio::test]
    async fn test_coalesce_batching() {
        let (tx, rx) = mpsc::channel(16);
        let stream = ReceiverStream::new(rx);

        let mut coalesced = coalesce(stream, Duration::from_millis(50), |batch: Vec<i32>| {
            vec![batch.into_iter().sum()]
        });

        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap();
        drop(tx);

        assert_eq!(coalesced.next().await, Some(6));
    }

    #[test]
    fn test_cdc_accumulator_from_rows() {
        use crate::Value;

        let rows = vec![
            HashMap::from([
                ("id".to_string(), Value::String("a".to_string())),
                ("content".to_string(), Value::String("hello".to_string())),
            ]),
            HashMap::from([
                ("id".to_string(), Value::String("b".to_string())),
                ("content".to_string(), Value::String("world".to_string())),
            ]),
        ];

        let acc = CdcAccumulator::from_rows(rows);
        assert_eq!(acc.state().len(), 2);
        assert!(acc.state().contains_key("a"));
        assert!(acc.state().contains_key("b"));
    }

    #[test]
    fn test_cdc_accumulator_apply_change() {
        use crate::{ChangeOrigin, Value};

        let mut acc = CdcAccumulator::<crate::widget_spec::DataRow>::from_rows(vec![]);

        let row = HashMap::from([
            ("id".to_string(), Value::String("x".to_string())),
            ("content".to_string(), Value::String("test".to_string())),
        ]);

        acc.apply_change(crate::Change::Created {
            data: row,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        assert_eq!(acc.state().len(), 1);

        acc.apply_change(crate::Change::Deleted {
            id: "x".to_string(),
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        assert_eq!(acc.state().len(), 0);
    }

    #[test]
    fn test_cdc_accumulator_fields_changed_patches() {
        use crate::{ChangeOrigin, Value};

        let mut acc = CdcAccumulator::from_rows(vec![HashMap::from([
            ("id".to_string(), Value::String("x".to_string())),
            ("content".to_string(), Value::String("old".to_string())),
            ("tags".to_string(), Value::String("a,b".to_string())),
        ])]);

        acc.apply_change(crate::Change::FieldsChanged {
            entity_id: "x".to_string(),
            fields: vec![(
                "content".to_string(),
                Value::String("old".to_string()),
                Value::String("new".to_string()),
            )],
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });

        let row = acc.state().get("x").unwrap();
        // content should be patched
        assert_eq!(row.get("content").unwrap().as_string(), Some("new"));
        // tags should be preserved
        assert_eq!(row.get("tags").unwrap().as_string(), Some("a,b"));
    }
}
