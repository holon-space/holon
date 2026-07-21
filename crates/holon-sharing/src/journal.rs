//! Migration journal (ADR 0028 H3, review A2).
//!
//! A crossing's migration (delete-in-source + create-in-target across two
//! container docs) is **journaled**: the journal is the **unit of atomicity**,
//! and it is **idempotent and resumable**. Replays of already-applied steps are
//! no-ops; a crash mid-window resumes from the persisted journal and completes.
//!
//! ## Leak-direction contract (Inc 2)
//! The step order is fixed by `widens_audience` so the container audience is
//! never, at any instant, wider than the policy audience:
//! - **widen** (`widens_audience = true`): **create-in-target (shared) FIRST**,
//!   then delete-in-source. The block is briefly in both containers (union =
//!   the wider, intended target — no leak beyond the target).
//! - **narrow** (`widens_audience = false`): **delete-in-source (shared)
//!   FIRST**, then create-in-target. The block is briefly in NEITHER =
//!   disclosed transient invisibility (never a leak, never a silent orphan —
//!   see [`crate::projection`]).

use serde::Deserialize;
use serde::Serialize;

use crate::types::Crossing;
use crate::types::CrossingId;

/// One side of the delete+create pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationOp {
    /// Create the block in the target container (project its SQL rows there).
    CreateInTarget,
    /// Delete the block from the source container (retract its SQL rows).
    DeleteFromSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalStep {
    pub crossing_id: CrossingId,
    pub op: MigrationOp,
    pub status: StepStatus,
}

/// A resumable, idempotent migration plan for one crossing. Serialize it to
/// persist across a crash; `resume` after reload replays only `Pending` steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationJournal {
    pub crossing_id: CrossingId,
    pub steps: Vec<JournalStep>,
}

/// Error surfaced by an `apply` closure (fail-loud; never swallowed).
#[derive(Debug, thiserror::Error)]
#[error("migration step {op:?} for crossing `{crossing_id}` failed: {reason}")]
pub struct MigrationError {
    pub crossing_id: String,
    pub op: MigrationOp,
    pub reason: String,
}

impl MigrationJournal {
    /// Build the ordered step list for a crossing, honoring the leak-direction
    /// contract above.
    pub fn plan(crossing: &Crossing) -> Self {
        let ops = if crossing.widens_audience {
            [MigrationOp::CreateInTarget, MigrationOp::DeleteFromSource]
        } else {
            [MigrationOp::DeleteFromSource, MigrationOp::CreateInTarget]
        };
        let steps = ops
            .into_iter()
            .map(|op| JournalStep {
                crossing_id: crossing.crossing_id.clone(),
                op,
                status: StepStatus::Pending,
            })
            .collect();
        Self {
            crossing_id: crossing.crossing_id.clone(),
            steps,
        }
    }

    /// Apply every `Pending` step in order, marking each `Done` on success.
    /// Idempotent: `Done` steps are skipped, so calling `resume` on a complete
    /// (or partially complete) journal never re-applies a finished step. If a
    /// step's `apply` returns `Err`, that step stays `Pending` (it never
    /// mutated) and the error propagates — resume it later.
    pub fn resume<F>(&mut self, mut apply: F) -> Result<(), MigrationError>
    where
        F: FnMut(&JournalStep) -> Result<(), MigrationError>,
    {
        for step in &mut self.steps {
            if step.status == StepStatus::Done {
                continue;
            }
            apply(step)?;
            step.status = StepStatus::Done;
        }
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| s.status == StepStatus::Done)
    }
}
