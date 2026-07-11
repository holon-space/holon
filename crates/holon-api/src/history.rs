//! History & provenance as a queryable relation (VisionGapAnalysis C2b, ADR
//! 0024 P8).
//!
//! Provenance stamping ([`crate::provenance`]) records the *latest* authorship
//! on a block. This module is the complementary **stream**: every op/effect the
//! engine executes, appended in order, so "postponed 7 times", the supervision
//! view, and Guide's over-time queries become plain queries over one relation.
//!
//! # Fidelity & disclosure (Martin's ruling, 2026-07-11)
//!
//! The relation is a **disclosed ephemeral cache** — Turso-projected, Layer-3,
//! rebuildable from the substrate's own history, **never authoritative**. The
//! [`HistoryStore`] trait is the typed convenience surface; the Turso impl also
//! keeps the data in a plain SQL table so matviews / PRQL can join it directly
//! (the ruling allows direct SQL exposure — this trait is not an indirection
//! wall, it is the thin typed accessor).
//!
//! Org-standalone vaults have no Turso query substrate, so they get a
//! **degraded** impl whose reads fail loud with a disclosed reason and whose
//! fidelity reports [`HistoryFidelity::None`] — mirroring the existing
//! CRDT-vs-LWW capability split (a `LoroMemory` container "deliberately gives
//! up SQL/GQL/PRQL queries").

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

/// How completely the history relation can be **rebuilt** from the substrate's
/// own durable history — the ADR 0024 P8 ladder (`Loro op history ≻ jj/git ≻
/// none`). Orthogonal to whether the relation is *currently* populated; it is a
/// disclosed statement of rebuild guarantee, not of live contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryFidelity {
    /// Loro op history present — the relation is fully rebuildable op-by-op.
    Loro,
    /// Only jj/git commit history — rebuild is coarse (commit granularity).
    Jj,
    /// No durable history source and/or no query substrate — the relation holds
    /// only what accrued this session, or is absent (org-standalone degraded).
    None,
}

impl HistoryFidelity {
    /// Short tag for logs / disclosure.
    pub fn tag(self) -> &'static str {
        match self {
            HistoryFidelity::Loro => "loro",
            HistoryFidelity::Jj => "jj",
            HistoryFidelity::None => "none",
        }
    }
}

/// One recorded op/effect in the history stream. Append-only; the store assigns
/// a monotonic sequence so `query` results are totally ordered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEvent {
    /// The block the op affected (a `FieldDelta::entity_id`).
    pub block_id: String,
    /// The op that ran (`create` / `update` / `set_field` / `delete` / …).
    pub op_name: String,
    /// Provenance origin kind (the [`crate::OpOrigin::tag`]).
    pub origin: String,
    /// Firing transition id — set for rule-origin ops.
    pub transition_id: Option<String>,
    /// Driving agent session id — set for agent-origin ops.
    pub session_id: Option<String>,
    /// Driving agent tool-call id — set for agent-origin ops.
    pub tool_call_id: Option<String>,
    /// The field that changed (for field-level deltas), enabling
    /// state-transition counts like "moved to `postponed` N times".
    pub field: Option<String>,
    /// String rendering of the new field value.
    pub new_value: Option<String>,
    /// Wall-clock time (ms since Unix epoch) from the injected clock seam.
    pub at_millis: i64,
}

/// Filter for stream queries. Every `Some` field is an AND condition; `None`
/// fields are unconstrained. `since`/`until` are inclusive/exclusive on
/// `at_millis` (`since <= at < until`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    pub block_id: Option<String>,
    pub origin: Option<String>,
    pub session_id: Option<String>,
    pub field: Option<String>,
    pub new_value: Option<String>,
    pub since_millis: Option<i64>,
    pub until_millis: Option<i64>,
}

impl HistoryQuery {
    /// Everything about one block, oldest→newest.
    pub fn for_block(block_id: impl Into<String>) -> Self {
        Self {
            block_id: Some(block_id.into()),
            ..Self::default()
        }
    }

    /// Everything from one agent session (supervision view).
    pub fn for_session(session_id: impl Into<String>) -> Self {
        Self {
            session_id: Some(session_id.into()),
            ..Self::default()
        }
    }

    /// A block moved to a given field value — the "postponed N times" shape.
    pub fn transitions_to(
        block_id: impl Into<String>,
        field: impl Into<String>,
        new_value: impl Into<String>,
    ) -> Self {
        Self {
            block_id: Some(block_id.into()),
            field: Some(field.into()),
            new_value: Some(new_value.into()),
            ..Self::default()
        }
    }
}

/// The queryable op/effect history relation (C2b).
///
/// A disclosed ephemeral cache; see the module docs. Implementations:
/// - a Turso-backed store (full: the relation is a real SQL table, joinable);
/// - a degraded store for org-standalone vaults (reads fail loud, disclosed).
#[async_trait]
pub trait HistoryStore: Send + Sync {
    /// The rebuild fidelity of this store (disclosed, mode-dependent).
    fn fidelity(&self) -> HistoryFidelity;

    /// Append one event to the stream. Fed from the dispatch chokepoint. Fails
    /// loud on a write error (never silently drops history).
    async fn record(&self, event: HistoryEvent) -> anyhow::Result<()>;

    /// Events matching `filter`, in append (`seq`) order.
    async fn query(&self, filter: &HistoryQuery) -> anyhow::Result<Vec<HistoryEvent>>;

    /// Count events matching `filter` — the "postponed N times" primitive
    /// (`count(&HistoryQuery::transitions_to(block, "status", "postponed"))`).
    async fn count(&self, filter: &HistoryQuery) -> anyhow::Result<u64>;
}
