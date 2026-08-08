//! What the app actually dispatched, as opposed to what a keymap said it
//! would.
//!
//! A driver that reports the keymap MATCH as the outcome cannot tell a working
//! chord from a chord whose action was shadowed on the way to the dispatcher —
//! both read as success. This journal is the observation seam that makes the
//! difference expressible: a caller takes a [`mark`](DispatchJournal::mark)
//! before a gesture and reads back exactly what that gesture produced.
//!
//! Two kinds of entry, because Holon has two chord registries and a reply that
//! covers only one is wrong for the other:
//!
//! * [`DispatchJournal::record`] — an [`OperationIntent`] going to the
//!   dispatcher (the structural registry: split_block, indent, …).
//! * [`DispatchJournal::record_window_action`] — a window-registry action whose
//!   handler does NOT go through `dispatch_intent` (undo/redo run
//!   `FrontendSession::undo` directly, open_search only opens a panel). Named
//!   with the registry's own action name so a reply can match it.
//!
//! Every entry starts [`DispatchOutcome::Pending`] and is settled by whoever
//! learns the result. An entry that is still pending is reported as pending —
//! never as success, and never as "nothing ran".

use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::Result;

use crate::operations::OperationIntent;

/// Entries kept for readback. A gesture produces one or a few entries, so this
/// is several hundred gestures of history.
const RETAINED_ENTRIES: usize = 512;

/// Where a dispatched action got to. `Pending` is a real answer, not a
/// placeholder: fire-and-forget dispatch means the outcome can genuinely be
/// unknown at read time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    Pending,
    Succeeded,
    Failed(String),
}

impl DispatchOutcome {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

/// One dispatched action, reduced to what an observer needs to decide whether
/// the right thing happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchedIntent {
    pub entity_name: String,
    pub op_name: String,
    /// The `id` param, when the action carries one — the block it targeted.
    pub target: Option<String>,
    pub outcome: DispatchOutcome,
    /// Position in the dispatch stream; the handle [`DispatchJournal::settle`]
    /// takes.
    pub seq: u64,
}

/// The entity name window-registry actions are recorded under. Not a real
/// entity — these actions have no `execute_operation` form.
pub const WINDOW_REGISTRY: &str = "window";

/// Ordered record of dispatched actions with a monotonic mark.
#[derive(Debug, Default)]
pub struct DispatchJournal {
    /// `(entries, total_recorded)` under one lock so a mark taken between a
    /// push and a counter bump can never miss an entry.
    state: Mutex<(VecDeque<DispatchedIntent>, u64)>,
}

impl DispatchJournal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current position in the dispatch stream. Pass to [`Self::since`].
    pub fn mark(&self) -> u64 {
        self.state.lock().unwrap().1
    }

    pub fn record(&self, intent: &OperationIntent) -> u64 {
        self.push(
            intent.entity_name.as_str().to_string(),
            intent.op_name.clone(),
            intent
                .params
                .get("id")
                .and_then(|v| v.as_string())
                .map(str::to_string),
        )
    }

    /// Record that a window-registry chord's handler ran. `action` must be the
    /// name that registry publishes (`undo`, `cycle_tab_next`, …) — a reply
    /// matches on it.
    pub fn record_window_action(&self, action: &str) -> u64 {
        self.push(WINDOW_REGISTRY.to_string(), action.to_string(), None)
    }

    fn push(&self, entity_name: String, op_name: String, target: Option<String>) -> u64 {
        let mut state = self.state.lock().unwrap();
        let seq = state.1;
        state.0.push_back(DispatchedIntent {
            entity_name,
            op_name,
            target,
            outcome: DispatchOutcome::Pending,
            seq,
        });
        while state.0.len() > RETAINED_ENTRIES {
            state.0.pop_front();
        }
        state.1 += 1;
        seq
    }

    /// Attach the outcome to the entry `record*` returned `seq` for. A `seq`
    /// that has aged out of the retained window is silently dropped — the
    /// entry it would have settled is already unreadable.
    pub fn settle(&self, seq: u64, outcome: Result<(), String>) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.0.iter_mut().rev().find(|e| e.seq == seq) {
            entry.outcome = match outcome {
                Ok(()) => DispatchOutcome::Succeeded,
                Err(e) => DispatchOutcome::Failed(e),
            };
        }
    }

    /// Everything dispatched after `mark`.
    ///
    /// Errors when `mark` predates the retained window: a silently truncated
    /// answer would read as "fewer things happened than did", which is the
    /// exact lie this journal exists to prevent.
    pub fn since(&self, mark: u64) -> Result<Vec<DispatchedIntent>> {
        let state = self.state.lock().unwrap();
        let (entries, recorded) = (&state.0, state.1);
        anyhow::ensure!(
            mark <= recorded,
            "dispatch journal mark {mark} is ahead of the {recorded} entries recorded — the mark \
             came from a different journal"
        );
        let wanted = (recorded - mark) as usize;
        anyhow::ensure!(
            wanted <= entries.len(),
            "dispatch journal retains {} entries but {wanted} actions were dispatched since mark \
             {mark} — the answer would be truncated",
            entries.len()
        );
        Ok(entries
            .iter()
            .skip(entries.len() - wanted)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::EntityName;

    use super::*;

    fn intent(op: &str, id: &str) -> OperationIntent {
        let mut params = HashMap::new();
        params.insert("id".into(), holon_api::Value::String(id.into()));
        OperationIntent::new(EntityName::new("block"), op.into(), params)
    }

    #[test]
    fn since_returns_only_what_followed_the_mark() {
        let journal = DispatchJournal::new();
        journal.record(&intent("split_block", "block:a"));
        let mark = journal.mark();
        journal.record(&intent("cycle_task_state", "block:b"));
        let seen = journal.since(mark).unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].op_name, "cycle_task_state");
        assert_eq!(seen[0].target.as_deref(), Some("block:b"));
        assert_eq!(seen[0].entity_name, "block");
    }

    #[test]
    fn an_unsettled_entry_reads_pending_not_succeeded() {
        let journal = DispatchJournal::new();
        let mark = journal.mark();
        journal.record(&intent("indent", "block:a"));
        assert_eq!(
            journal.since(mark).unwrap()[0].outcome,
            DispatchOutcome::Pending
        );
    }

    #[test]
    fn settle_attaches_the_outcome_to_its_own_entry() {
        let journal = DispatchJournal::new();
        let mark = journal.mark();
        let ok = journal.record(&intent("indent", "block:a"));
        let bad = journal.record(&intent("outdent", "block:b"));
        journal.settle(ok, Ok(()));
        journal.settle(bad, Err("no parent to outdent to".into()));
        let seen = journal.since(mark).unwrap();
        assert_eq!(seen[0].outcome, DispatchOutcome::Succeeded);
        assert_eq!(
            seen[1].outcome,
            DispatchOutcome::Failed("no parent to outdent to".into())
        );
    }

    #[test]
    fn a_window_action_is_recorded_under_its_registry_name() {
        let journal = DispatchJournal::new();
        let mark = journal.mark();
        let seq = journal.record_window_action("undo");
        journal.settle(seq, Ok(()));
        let seen = journal.since(mark).unwrap();
        assert_eq!(seen[0].entity_name, WINDOW_REGISTRY);
        assert_eq!(seen[0].op_name, "undo");
        assert_eq!(seen[0].outcome, DispatchOutcome::Succeeded);
    }

    #[test]
    fn a_mark_older_than_the_retained_window_errors_instead_of_truncating() {
        let journal = DispatchJournal::new();
        let mark = journal.mark();
        for _ in 0..(RETAINED_ENTRIES + 1) {
            journal.record(&intent("indent", "block:a"));
        }
        let err = journal.since(mark).unwrap_err().to_string();
        assert!(err.contains("truncated"), "unexpected error: {err}");
    }
}
