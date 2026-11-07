//! Test seam: hold one row's bridges between "the engine changed the row" and
//! "the window's list took the change", until the test opens the hold.
//!
//! While a hold is shut on an inserted row the engine has the row but the
//! window's list does not, so a settle that returns in that state returns on a
//! stale frame. A content write is no such case: a row's text reads the shared
//! row cell at paint, so any repaint shows it.
//!
//! An engaged hold is a bridge in flight: the epoch relay publishes no new
//! watermark while one is engaged, as a bridge that awaits before its apply
//! would need.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use futures_signals::signal::Mutable;
use futures_signals::signal::SignalExt;
use futures_signals::signal_vec::VecDiff;
use holon_api::EntityUri;
use holon_frontend::ReactiveViewModel;

#[derive(Clone)]
struct Gate {
    opened: Mutable<bool>,
    engaged: Arc<AtomicBool>,
}

static HOLDS: LazyLock<Mutex<HashMap<EntityUri, Gate>>> = LazyLock::new(Default::default);
static RELEASES: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);

/// Shut until [`BridgeHold::open`] or drop.
#[must_use = "dropping the hold opens it"]
pub struct BridgeHold {
    row: EntityUri,
    gate: Gate,
}

/// Hold `row`'s data bridge from its next data change on.
pub fn hold_row_bridge(row: EntityUri) -> BridgeHold {
    let gate = Gate {
        opened: Mutable::new(false),
        engaged: Arc::default(),
    };
    let previous = HOLDS.lock().unwrap().insert(row.clone(), gate.clone());
    assert!(previous.is_none(), "{row} already has a bridge hold");
    BridgeHold { row, gate }
}

impl BridgeHold {
    /// Whether the row's bridge reached the hold, i.e. a data change of the
    /// row is waiting on it.
    pub fn engaged(&self) -> bool {
        self.gate.engaged.load(Ordering::SeqCst)
    }

    pub fn open(self) {}
}

impl Drop for BridgeHold {
    fn drop(&mut self) {
        HOLDS.lock().unwrap().remove(&self.row);
        self.gate.opened.set(true);
        RELEASES.send_modify(|n| *n += 1);
    }
}

/// The rows whose bridge waits on a shut hold.
pub fn engaged_rows() -> Vec<EntityUri> {
    HOLDS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, gate)| gate.engaged.load(Ordering::SeqCst))
        .map(|(row, _)| row.clone())
        .collect()
}

/// Resolves after the next hold opens.
pub(crate) fn released() -> impl std::future::Future<Output = ()> + 'static {
    let mut rx = RELEASES.subscribe();
    async move {
        rx.changed().await.expect("the release sender is static");
    }
}

/// Waits while `row` has a shut hold.
pub(crate) async fn wait_while_held(row: &EntityUri) {
    let gate = HOLDS.lock().unwrap().get(row).cloned();
    if let Some(gate) = gate {
        gate.engaged.store(true, Ordering::SeqCst);
        gate.opened.signal().wait_for(true).await;
    }
}

/// Waits while a row that `diff` writes has a shut hold.
pub(crate) async fn wait_while_diff_held(diff: &VecDiff<Arc<ReactiveViewModel>>) {
    let rows: Vec<EntityUri> = match diff {
        VecDiff::UpdateAt { value, .. }
        | VecDiff::InsertAt { value, .. }
        | VecDiff::Push { value } => value.row_id().into_iter().collect(),
        VecDiff::Replace { values } => values.iter().filter_map(|v| v.row_id()).collect(),
        _ => Vec::new(),
    };
    for row in &rows {
        wait_while_held(row).await;
    }
}
