//! Completion boundaries a masked transition can wait on, and what waiting for
//! one produced.
//!
//! A yield gives the runtime microseconds; an intent needs a SQL write plus a
//! CDC round trip, which takes milliseconds. Waiting on a boundary the system
//! actually crosses is therefore the only way a schedule other than
//! dispatch-all-then-settle becomes reachable.

use std::time::Duration;

use holon_frontend::dispatch_journal::DispatchJournal;

/// What the scheduler waits for before releasing the next dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Resume {
    /// Release the next dispatch at once.
    Immediate,
    Wait(Boundary),
}

/// A boundary the system crosses on its own. Separate from [`Resume`] so
/// "wait for nothing" cannot be handed to a function whose whole job is to
/// wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Boundary {
    /// `n` more of the in-flight intents settled.
    AfterIntents(u8),
    /// The Turso CDC watermark advanced.
    AfterCdcBatch,
    /// Every projection reached a combined fixed point.
    ///
    /// Alone among the boundaries this one cannot report a failure through
    /// [`BoundaryOutcome`]: the convergence wait fails loud on its own
    /// (`converge_projections` panics when the projections do not settle), so
    /// reaching this arm's return at all means it converged. Its wait is also
    /// floored at `CONVERGE_BUDGET` — a caller deadline below that floor does
    /// not shorten it.
    AfterQuiescence,
}

/// The dispatch window a boundary is measured against — the journal mark taken
/// before the transition dispatched anything.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoundaryWindow {
    pub journal_mark: Option<u64>,
}

impl BoundaryWindow {
    pub fn open(journal: Option<&DispatchJournal>) -> Self {
        Self {
            journal_mark: journal.map(|j| j.mark()),
        }
    }
}

/// Waiting for a boundary ends one of exactly three ways: refused up front,
/// observed, or the deadline burned — and burning the deadline is always a
/// wedge. A fourth "finished but never crossed" answer would be a degrade path
/// wide enough for a real wedge to hide in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoundaryOutcome {
    Observed(BoundaryEvidence),
    /// Nothing in flight could ever cross this boundary — decided BEFORE
    /// waiting. Carries the reason, which callers disclose.
    Unobservable(String),
    /// The deadline was consumed with work that should have crossed the
    /// boundary. A wedge, not a degrade.
    TimedOutWithPendingWork {
        pending: usize,
        waited: Duration,
    },
}

impl BoundaryOutcome {
    /// The wedge report, worded for what the deadline actually found: work
    /// still in flight reads differently from work that finished without ever
    /// producing the projection the boundary names, and a reader has to be able
    /// to tell which one they are looking at.
    pub fn wedge_report(boundary: Boundary, pending: usize, waited: Duration) -> String {
        match pending {
            0 => format!(
                "waited {waited:?} for {boundary:?} with NOTHING left in flight — the dispatch \
                 completed without ever producing it, so this boundary does not follow from this \
                 work"
            ),
            n => format!(
                "waited {waited:?} for {boundary:?} with {n} intent(s) STILL in flight — the \
                 dispatch or its projection is not progressing"
            ),
        }
    }

    pub fn evidence(&self) -> Option<&BoundaryEvidence> {
        match self {
            Self::Observed(e) => Some(e),
            _ => None,
        }
    }
}

/// What moved, for the conformance oracle and the run log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundaryEvidence {
    pub boundary: Boundary,
    pub waited: Duration,
    /// The observable's before→after reading.
    pub detail: String,
}

/// Journal entries in the window that have settled, and those still pending.
pub fn settled_and_pending(journal: &DispatchJournal, mark: u64) -> (usize, usize) {
    let entries = journal
        .since(mark)
        .unwrap_or_else(|e| panic!("[boundary] dispatch journal readback: {e:#}"));
    let pending = entries.iter().filter(|e| e.outcome.is_pending()).count();
    (entries.len() - pending, pending)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::EntityName;
    use holon_frontend::operations::OperationIntent;

    use super::*;

    fn intent() -> OperationIntent {
        OperationIntent::new(
            EntityName::new("block"),
            "set_field".to_string(),
            HashMap::new(),
        )
    }

    #[test]
    fn the_window_counts_only_what_followed_its_mark() {
        let journal = DispatchJournal::new();
        journal.settle(journal.record(&intent()), Ok(()));
        let window = BoundaryWindow::open(Some(&journal));
        let seq = journal.record(&intent());

        let mark = window.journal_mark.expect("journal implies a mark");
        assert_eq!(settled_and_pending(&journal, mark), (0, 1));
        journal.settle(seq, Ok(()));
        assert_eq!(settled_and_pending(&journal, mark), (1, 0));
    }
}
