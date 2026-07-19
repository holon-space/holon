//! Writer designation for `once_only` connector effects (leases/read-write
//! ruling, increment 4).
//!
//! ADR 0024 P4: external once-only effects (send email, create-without-key)
//! cannot be made exactly-once by any CRDT — they require *asymmetry*, an
//! explicit gated mechanism. Martin's 2026-07-19 ruling: a configurable rule
//! system, behind a trait, deciding *which device may do what*. First iteration
//! ships exactly two policies — [`ConfirmManually`] and [`AlwaysRun`] —
//! selected per-connector by the sidecar's `once_only:` field
//! ([`crate::mcp_sidecar::OnceOnlyAuthorization`]). Future impls (vault-block
//! roles, TTL leases) slot in behind the same trait without touching the
//! dispatch chokepoint.
//!
//! The trait is deliberately **pure**: it answers "does this device run this
//! connector's once_only writes freely, or require confirmation?" It does NOT
//! consult ledger/confirmation state. The confirmation *state machine* lives in
//! [`PendingWriteStore`] + the chokepoint, so a future policy impl only has to
//! answer the same pure question.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::EntityName;
use holon_core::storage::types::StorageEntity;

use crate::mcp_sidecar::OnceOnlyAuthorization;
use crate::mcp_sidecar::ToolEffect;

/// A pending external write, presented to a [`WriteAuthorizationPolicy`] for a
/// decision. Parse-don't-validate: no stringly control fields — the effect is
/// the typed [`ToolEffect`], the key is the deterministic intent key minted at
/// the chokepoint (increment 3's naming discipline, ADR 0024 P4).
pub struct WriteIntent<'a> {
    pub connector: &'a str,
    pub tool: &'a str,
    pub effect: ToolEffect,
    pub intent_key: &'a str,
}

/// The typed decision a policy returns for a [`WriteIntent`]. Never a bare
/// bool: `RequireConfirmation` is a first-class disclosed outcome (ADR 0024 P4
/// makes manual override first-class), distinct from an outright `Deny`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteDecision {
    /// Dispatch immediately (the device holds authority for this connector).
    Allow,
    /// Pause for human confirmation — enqueue as pending, never auto-fire.
    RequireConfirmation,
    /// Refuse with a disclosed reason. Reserved; unused by the two
    /// iteration-1 policies, but the chokepoint honours it.
    Deny { reason: String },
}

/// Decides authority for `once_only` effects on one connector. Pure: no ledger
/// access, no I/O.
pub trait WriteAuthorizationPolicy: Send + Sync {
    fn authorize(&self, intent: &WriteIntent<'_>) -> WriteDecision;
}

/// This device holds write authority: once_only effects dispatch immediately.
pub struct AlwaysRun;

impl WriteAuthorizationPolicy for AlwaysRun {
    fn authorize(&self, _: &WriteIntent<'_>) -> WriteDecision {
        WriteDecision::Allow
    }
}

/// Safe default: every once_only effect pauses for explicit human confirmation
/// and never fires unattended (no TTL, no auto-approve).
pub struct ConfirmManually;

impl WriteAuthorizationPolicy for ConfirmManually {
    fn authorize(&self, _: &WriteIntent<'_>) -> WriteDecision {
        WriteDecision::RequireConfirmation
    }
}

/// Map the sidecar config value to a policy impl. New config variants (TTL
/// lease, vault role) add new arms here and a new impl above — the chokepoint
/// is untouched.
pub fn policy_for(cfg: OnceOnlyAuthorization) -> Box<dyn WriteAuthorizationPolicy> {
    match cfg {
        OnceOnlyAuthorization::ConfirmManually => Box::new(ConfirmManually),
        OnceOnlyAuthorization::AlwaysRun => Box::new(AlwaysRun),
    }
}

/// Lifecycle of one once_only intent in the [`PendingWriteStore`]. The
/// transitions encode at-most-once: a dispatch-owning state
/// ([`PendingState::Dispatching`]) is taken *before* the remote call, and a
/// post-dispatch failure lands in [`PendingState::OutcomeUnknown`] which is
/// NEVER auto-retried — it is surfaced for the human to verify on the remote
/// (fail-loud, no fake success, no silent retry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingState {
    /// Queued under `confirm_manually`, awaiting explicit approval.
    AwaitingConfirmation,
    /// Approval granted a one-shot dispatch token; not yet taken for dispatch.
    Confirmed,
    /// Dispatch owned (taken before the remote call). At most one per key.
    Dispatching,
    /// Remote acked.
    Sent,
    /// Dispatch attempted, no positive ack (crash/timeout/remote error).
    /// Disclosed; never auto-retried.
    OutcomeUnknown { detail: String },
}

/// One tracked once_only intent: enough to re-dispatch on approval and to
/// display in the pending-writes UI.
#[derive(Debug, Clone)]
pub struct PendingWrite {
    pub entity_name: EntityName,
    pub op_name: String,
    pub params: StorageEntity,
    pub connector: String,
    pub tool: String,
    pub display: String,
    pub state: PendingState,
    pub dispatch_count: u32,
}

/// A read-only snapshot of a pending intent for UI/tests.
#[derive(Debug, Clone)]
pub struct PendingWriteView {
    pub intent_key: String,
    pub connector: String,
    pub tool: String,
    pub display: String,
    pub state: PendingState,
    pub dispatch_count: u32,
}

/// The at-most-once state machine for once_only writes. All transitions mutate
/// under one mutex — the store is the correctness surface, so the compare-and-
/// take methods ([`Self::confirm`], [`Self::take_for_dispatch`],
/// [`Self::begin_dispatch`]) return a single-winner bool. In-memory per
/// process (proposal Q3: the ledger is retry bookkeeping, not a cross-replica
/// dedup mechanism; durability is out of scope for iteration-1 and disclosed).
#[derive(Default)]
pub struct PendingWriteStore {
    entries: Mutex<HashMap<String, PendingWrite>>,
}

impl PendingWriteStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a once_only intent awaiting confirmation. Idempotent: a repeated
    /// enqueue of the same intent key (e.g. a reactive `triggered_by` re-fire)
    /// coalesces to the single existing entry — no duplicate confirmations.
    pub fn enqueue_pending(&self, key: &str, mut write: PendingWrite) {
        let mut entries = self.entries.lock().unwrap();
        entries.entry(key.to_string()).or_insert_with(|| {
            write.state = PendingState::AwaitingConfirmation;
            write.dispatch_count = 0;
            write
        });
    }

    /// Compare-and-take approval (single-winner). `AwaitingConfirmation ->
    /// Confirmed` returns `true` exactly once; any later or racing call returns
    /// `false`. The `true` is the one-shot dispatch token — only the winner
    /// re-dispatches.
    pub fn confirm(&self, key: &str) -> bool {
        let mut entries = self.entries.lock().unwrap();
        match entries.get_mut(key) {
            Some(w) if w.state == PendingState::AwaitingConfirmation => {
                w.state = PendingState::Confirmed;
                true
            }
            _ => false,
        }
    }

    /// Take dispatch ownership on the *confirmed* path. `Confirmed ->
    /// Dispatching` returns `true` exactly once; else `false`. The remote call
    /// happens only after `true`, so at most one dispatch is ever in flight.
    pub fn take_for_dispatch(&self, key: &str) -> bool {
        let mut entries = self.entries.lock().unwrap();
        match entries.get_mut(key) {
            Some(w) if w.state == PendingState::Confirmed => {
                w.state = PendingState::Dispatching;
                w.dispatch_count += 1;
                true
            }
            _ => false,
        }
    }

    /// Take dispatch ownership on the *always_run* path: insert straight into
    /// `Dispatching` iff the key is absent. If an entry already exists (any
    /// state — in flight, sent, or outcome-unknown) returns `false`, so a
    /// re-attempt never fires the effect twice.
    pub fn begin_dispatch(&self, key: &str, mut write: PendingWrite) -> bool {
        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(key) {
            return false;
        }
        write.state = PendingState::Dispatching;
        write.dispatch_count = 1;
        entries.insert(key.to_string(), write);
        true
    }

    /// Record a positive ack: `-> Sent`.
    pub fn mark_sent(&self, key: &str) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(w) = entries.get_mut(key) {
            w.state = PendingState::Sent;
        }
    }

    /// Record a dispatch whose outcome is unknown (post-dispatch failure).
    /// Terminal and disclosed; never auto-retried.
    pub fn mark_outcome_unknown(&self, key: &str, detail: String) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(w) = entries.get_mut(key) {
            w.state = PendingState::OutcomeUnknown { detail };
        }
    }

    /// The stored call for a tracked intent, for re-dispatch on approval.
    pub fn stored_call(&self, key: &str) -> Option<(EntityName, String, StorageEntity)> {
        let entries = self.entries.lock().unwrap();
        entries
            .get(key)
            .map(|w| (w.entity_name.clone(), w.op_name.clone(), w.params.clone()))
    }

    /// Current state of a tracked intent (test/UI introspection).
    pub fn state_of(&self, key: &str) -> Option<PendingState> {
        let entries = self.entries.lock().unwrap();
        entries.get(key).map(|w| w.state.clone())
    }

    /// Snapshot of all tracked intents for the pending-writes UI.
    pub fn list(&self) -> Vec<PendingWriteView> {
        let entries = self.entries.lock().unwrap();
        entries
            .iter()
            .map(|(k, w)| PendingWriteView {
                intent_key: k.clone(),
                connector: w.connector.clone(),
                tool: w.tool.clone(),
                display: w.display.clone(),
                state: w.state.clone(),
                dispatch_count: w.dispatch_count,
            })
            .collect()
    }
}

/// Shared handle to the pending store (provider holds one; the frontend gets a
/// clone for the approve UI in increment 4c).
pub type SharedPendingWrites = Arc<PendingWriteStore>;

#[cfg(test)]
mod tests {
    use super::*;

    fn write(state: PendingState) -> PendingWrite {
        PendingWrite {
            entity_name: EntityName::from("items"),
            op_name: "write_item".to_string(),
            params: StorageEntity::new(),
            connector: "mock".to_string(),
            tool: "write-item".to_string(),
            display: "Write item".to_string(),
            state,
            dispatch_count: 0,
        }
    }

    #[test]
    fn confirm_is_single_winner() {
        let store = PendingWriteStore::new();
        store.enqueue_pending("k", write(PendingState::AwaitingConfirmation));
        assert!(store.confirm("k"), "first confirm wins");
        assert!(!store.confirm("k"), "second confirm is a no-op");
        assert_eq!(store.state_of("k"), Some(PendingState::Confirmed));
    }

    #[test]
    fn take_for_dispatch_only_after_confirm() {
        let store = PendingWriteStore::new();
        store.enqueue_pending("k", write(PendingState::AwaitingConfirmation));
        assert!(
            !store.take_for_dispatch("k"),
            "cannot dispatch before confirm"
        );
        assert!(store.confirm("k"));
        assert!(
            store.take_for_dispatch("k"),
            "confirmed -> dispatching once"
        );
        assert!(!store.take_for_dispatch("k"), "dispatch token is one-shot");
        assert_eq!(store.state_of("k"), Some(PendingState::Dispatching));
    }

    #[test]
    fn begin_dispatch_is_at_most_once() {
        let store = PendingWriteStore::new();
        assert!(store.begin_dispatch("k", write(PendingState::AwaitingConfirmation)));
        assert!(
            !store.begin_dispatch("k", write(PendingState::AwaitingConfirmation)),
            "a second always-run attempt must not re-fire"
        );
        store.mark_outcome_unknown("k", "boom".to_string());
        assert!(
            !store.begin_dispatch("k", write(PendingState::AwaitingConfirmation)),
            "outcome-unknown is terminal — never auto-retried"
        );
    }

    #[test]
    fn enqueue_coalesces_refire() {
        let store = PendingWriteStore::new();
        store.enqueue_pending("k", write(PendingState::AwaitingConfirmation));
        store.confirm("k");
        // A reactive re-fire of the same intent must not resurrect it to awaiting.
        store.enqueue_pending("k", write(PendingState::AwaitingConfirmation));
        assert_eq!(store.state_of("k"), Some(PendingState::Confirmed));
        assert_eq!(store.list().len(), 1);
    }
}
