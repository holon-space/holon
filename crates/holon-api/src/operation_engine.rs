//! The operation-execution capability trait (ADR 0004 — "Turso is one of
//! four").
//!
//! `OperationEngine` is the seam the frontend's mutation/operation path depends
//! on instead of a concrete backend. It covers dispatching operations,
//! discovering which operations an entity supports, and undo/redo. Operations
//! are *not* fundamentally Turso-bound — implementations live in
//! `holon::api::operation_engine` (`BackendEngine` and the dispatcher-backed
//! `DispatchingOperationEngine`); the frontend holds it as
//! `Option<Arc<dyn OperationEngine>>` so a session without the capability
//! reports its absence as a typed fact rather than panicking behind `engine()`.

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::EntityName;
use crate::OperationDescriptor;
use crate::StorageEntity;
use crate::Value;

/// Provenance of an operation as it enters the engine — the "who caused this"
/// axis of ADR 0024's `fired-by: <transition-id>` slot.
///
/// This is the undo-provenance dimension, distinct from `EventOrigin`
/// (CDC/event routing). It decides whether an operation is user-authored
/// (and therefore belongs on the user undo stack) or system-authored
/// (rule firings, sync merges, ingest) which must NOT enter the user stack.
///
/// There is deliberately no `Default`: every dispatch site names its origin so
/// the "who caused this" question is answered by the compiler, not guessed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpOrigin {
    /// A direct human gesture in a frontend (UI edit, web-worker session). The
    /// only origin that pushes undo entries.
    User,
    /// Fired by a rule/transition. `transition_id` is ADR 0024's `fired-by`
    /// provenance slot. Never enters the user undo stack.
    Rule { transition_id: String },
    /// Executed on behalf of an AI agent driving the system over the embedded
    /// MCP server. `session_id`/`tool_call_id` are the supervision-view /
    /// revert-whole-call slots (ADR 0024 P8, VisionGapAnalysis C2a). Agent
    /// actions are reverted through the supervision surface, so they do NOT
    /// enter the human undo stack.
    Agent {
        session_id: String,
        tool_call_id: String,
    },
    /// Applied by a CRDT sync merge from a peer replica.
    Sync,
    /// Applied by org-file / external ingest at the boundary.
    Ingest,
}

impl OpOrigin {
    /// Whether an operation with this origin should push a user undo entry.
    pub fn is_user(&self) -> bool {
        matches!(self, OpOrigin::User)
    }

    /// Short tag for logs / persistence / the provenance stamp discriminator.
    pub fn tag(&self) -> &str {
        match self {
            OpOrigin::User => "user",
            OpOrigin::Rule { .. } => "rule",
            OpOrigin::Agent { .. } => "agent",
            OpOrigin::Sync => "sync",
            OpOrigin::Ingest => "ingest",
        }
    }
}

/// Whether an operation's effect is PROVEN to have taken hold.
///
/// Local operations are proven by construction: they committed, or they
/// returned `Err`. An external connector write is proven only when the remote
/// vouches for it — a provider that acks an effect it could not confirm
/// (`{"outcome":"unconfirmed"}`) has told us the truth, and reporting that as
/// delivery would turn its honesty into a fabricated record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Proven,
    /// Dispatched, delivery not proven. `detail` is the provider's own wording
    /// and is surfaced verbatim; never auto-retried.
    Unproven {
        detail: String,
    },
}

/// What an engine dispatch reports: the operation's response payload and
/// whether its effect is proven to have landed. The two travel together so a
/// caller cannot read the response and silently assume delivery.
#[derive(Debug, Clone)]
pub struct OpOutcome {
    pub response: Option<Value>,
    pub delivery: Delivery,
}

impl OpOutcome {
    /// An outcome from a path whose effect is proven by construction.
    pub fn proven(response: Option<Value>) -> Self {
        Self {
            response,
            delivery: Delivery::Proven,
        }
    }
}

/// Execute, discover, and undo/redo operations.
#[async_trait]
pub trait OperationEngine: Send + Sync {
    /// Dispatch an operation, returning its optional result value.
    ///
    /// `origin` states who caused the operation (ADR 0024 `fired-by`
    /// provenance). Only [`OpOrigin::User`] operations push undo entries.
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        origin: OpOrigin,
    ) -> Result<OpOutcome>;

    /// The operations registered for `entity_name`.
    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor>;

    /// Whether `op_name` is registered for `entity_name`.
    async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool;

    /// Undo the last operation. See [`UndoOutcome`] — a stale entry is dropped
    /// loudly rather than silently skipped.
    async fn undo(&self) -> Result<UndoOutcome>;

    /// Redo the last undone operation. See [`UndoOutcome`].
    async fn redo(&self) -> Result<UndoOutcome>;

    /// Whether an undo is available.
    async fn can_undo(&self) -> bool;

    /// Whether a redo is available.
    async fn can_redo(&self) -> bool;
}

/// Result of an undo/redo request.
///
/// A stale entry (the state the inverse was computed against has since changed)
/// is DROPPED with `StaleDropped` — never silently skipped to the next entry —
/// so the frontend can surface it (banner) and the user knows the history point
/// was abandoned rather than applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoOutcome {
    /// An entry was successfully undone/redone.
    Applied,
    /// Nothing to undo/redo (the stack was empty).
    Empty,
    /// The top entry's precondition no longer matches current state; the entry
    /// was dropped. `reason` is user-surfaceable.
    StaleDropped { reason: String },
    /// The entry was consumed but its inverse (or forward, on redo) replay
    /// produced NO observable state change — every reported field delta was
    /// vacuous (`old_value == new_value`). Distinguished from [`Self::Applied`]
    /// so the caller never claims "undone" for a press that changed nothing
    /// (fail-loud contract, CLAUDE.md): the entry is dropped and the outcome is
    /// surfaced to the user (MCP result + UI toast) as a no-op, not success.
    /// This closes the BugFunnel 2026-07-13 gap where consecutive undo calls
    /// reported success while a poison identical-content entry sat on top and
    /// the targeted delete stayed unreachable.
    NoChange,
}

/// The disclosure text for [`UndoOutcome::StaleDropped`]: what the user LOST
/// first, the state divergence that caused it second. `label` is `"undo"` or
/// `"redo"`.
///
/// Lives here, beside the outcome, because every surface must say the same
/// thing — the GPUI toast, the dispatch journal a test or an agent reads, and
/// the MCP reply. A dropped entry is permanent, so a surface that says only
/// "skipped" or "failed" invites the user to press again for a step that can
/// never come back.
pub fn undo_step_dropped_detail(label: &str, reason: &str) -> String {
    format!("{label}: history step dropped — this edit can no longer be undone ({reason})")
}

impl UndoOutcome {
    /// Back-compat helper: did an entry actually apply?
    pub fn applied(&self) -> bool {
        matches!(self, UndoOutcome::Applied)
    }
}
