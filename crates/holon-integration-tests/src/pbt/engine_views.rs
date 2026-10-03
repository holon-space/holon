//! The view engine a Loro projection feeds, and the views recomputed from
//! blocks, so a test can compare the two.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::TryRecvError;

use holon_api::block::SnapshotBlock;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::Stamp;
use holon_views::batch;
use holon_views::batch::Multiset;
use holon_views::batch::add;
use holon_views::engine::Field;
use holon_views::engine::ViewBatch;
use holon_views::engine::ViewEngine;
use holon_views::engine::fields;
use holon_views::error::EngineError;
use holon_views::intern::Interner;
use holon_views::plan::check_all;
use holon_views::row::DynRow;
use holon_views::views::View;
use holon_views::views::block_row;
use holon_views::views::catalog;
use holon_views::views::views;

/// One view's rows, decoded.
pub type Released = Multiset<Vec<Field>>;

/// A fresh clock and an engine on it; a stop is logged as an error and makes
/// every later snapshot return it.
pub fn start_views_engine() -> (Arc<CommitClock>, Arc<ViewEngine>) {
    let clock = Arc::new(CommitClock::new());
    let engine = ViewEngine::start(clock.clone(), |error| {
        tracing::error!("the view engine stopped: {error}")
    });
    (clock, Arc::new(engine))
}

/// Returns once `engine` has handled every feed posted before the call, so
/// the clock holds what those feeds cover.
pub fn engine_caught_up(engine: &ViewEngine) -> Result<(), EngineError> {
    engine.snapshot_and_subscribe(View::Row).map(drop)
}

/// The views over `blocks` by the batch backend, in the order of
/// [`View::ALL`].
pub fn recompute<'a>(
    blocks: impl IntoIterator<Item = &'a SnapshotBlock>,
) -> Result<Vec<Released>, EngineError> {
    let mut interner = Interner::default();
    let mut rows = Multiset::<DynRow>::new();
    for block in blocks {
        add(&mut rows, block_row(&mut interner, block)?, 1);
    }
    check_all(&views().plans(), &catalog())
        .expect("Holon's views are well typed")
        .iter()
        .map(|plan| {
            Ok(batch::run(plan, std::slice::from_ref(&rows))?
                .into_iter()
                .map(|(row, n)| (fields(&row, plan.schema(), &interner), n))
                .collect())
        })
        .collect()
}

/// Every view of an engine, folded version by version from its subscription.
pub struct FoldedViews {
    subscriptions: Vec<Receiver<ViewBatch>>,
    /// Per view, the `below` of the version `states` holds.
    pub below: Vec<Stamp>,
    pub states: Vec<Released>,
}

impl FoldedViews {
    pub fn subscribe(engine: &ViewEngine) -> Result<FoldedViews, EngineError> {
        let mut folded = FoldedViews {
            subscriptions: Vec::new(),
            below: Vec::new(),
            states: Vec::new(),
        };
        for view in View::ALL {
            let (snapshot, subscription) = engine.snapshot_and_subscribe(view)?;
            let mut state = Released::new();
            for (row, n) in snapshot.deltas {
                add(&mut state, row, n);
            }
            folded.subscriptions.push(subscription);
            folded.below.push(snapshot.below);
            folded.states.push(state);
        }
        Ok(folded)
    }

    /// Folds in the next version of view `i` released so far, if any.
    pub fn next(&mut self, i: usize) -> Option<Stamp> {
        let batch = match self.subscriptions[i].try_recv() {
            Ok(batch) => batch,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => panic!("the view engine thread is gone"),
        };
        for (row, n) in batch.deltas {
            add(&mut self.states[i], row, n);
        }
        self.below[i] = batch.below;
        Some(batch.below)
    }
}

/// Each view as its name and its rows rendered `"{row:?} x{count}"`, sorted,
/// in the order of [`View::ALL`].
pub fn view_rows(views: &[Released]) -> Vec<(String, Vec<String>)> {
    assert_eq!(views.len(), View::ALL.len());
    View::ALL
        .iter()
        .zip(views)
        .map(|(view, rows)| {
            let mut rows: Vec<String> = rows
                .iter()
                .map(|(row, n)| format!("{row:?} x{n}"))
                .collect();
            rows.sort();
            (format!("{view:?}"), rows)
        })
        .collect()
}

/// The rows `a` holds with a count `b` does not, at most `cap` of them.
pub fn only_in(a: &Released, b: &Released, cap: usize) -> Vec<(Vec<Field>, isize)> {
    a.iter()
        .filter(|(row, n)| b.get(*row) != Some(*n))
        .take(cap)
        .map(|(row, n)| (row.clone(), *n))
        .collect()
}
