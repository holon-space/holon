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
//!
//! # Rebuild contract (honest partial — C2 INC 4, fork F2b)
//!
//! [`HistoryStore::rebuild`] truncates the relation and repopulates it from
//! what the substrate can **prove**, disclosed exactly by [`HistoryFidelity`]:
//!
//! - The block store durably keeps each block's `_provenance` stamp (its
//!   *latest* authorship — origin, ids, timestamp). Rebuild recovers, per
//!   extant stamped block, **one `create` event** carrying that stamp. This is
//!   the provable subset.
//! - It does **not** recover the intermediate field-delta stream (a block's
//!   current row holds current values, not the historical deltas, and prior
//!   `old_value`s are unknowable), so state-transition counts like "postponed N
//!   times" are NOT rebuildable. Rather than fabricate provenance for ops that
//!   left no trace, rebuild omits them — and the store reports
//!   [`HistoryFidelity::Partial`], never [`HistoryFidelity::Loro`].
//!
//! Full (Loro) fidelity — riding provenance on Loro commit metadata so the
//! whole op stream is losslessly recoverable — is PARKED (frontier work; C2
//! fork F2a, awaiting Martin's ruling). Rebuild is deterministic: ordering the
//! recovered stamps by `(at_millis, block_id)` and assigning `op_group`s
//! sequentially, two rebuilds of the same substrate produce byte-identical
//! rows.
//!
//! # Schema evolution
//!
//! Because the relation is ephemeral and rebuildable, its migration story is
//! **drop + recreate** — the schema module detects a stale table shape and
//! replaces it; no migration machinery ever. Losing accrued rows is
//! contractually fine (and disclosed by [`HistoryFidelity`]).

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

/// How completely the history relation can be **rebuilt** from the substrate's
/// own durable history — the ADR 0024 P8 ladder (`Loro op history ≻ jj/git ≻
/// block stamps ≻ none`). Orthogonal to whether the relation is *currently*
/// populated; it is a disclosed statement of rebuild guarantee, not of live
/// contents. The reported value is **computed from what the store's `rebuild`
/// can actually reproduce today**, never asserted by the caller — reporting a
/// stronger value than the implemented rebuilder can deliver is an undisclosed
/// over-claim the fail-loud philosophy forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryFidelity {
    /// Loro op history present — the relation is fully rebuildable op-by-op.
    /// PARKED: no rebuilder recovers the full op stream yet (needs provenance
    /// ridden on Loro commit metadata — C2 fork F2a, awaiting ruling). No store
    /// reports this today; a store must NOT claim it without the rebuilder.
    Loro,
    /// Only jj/git commit history — rebuild is coarse (commit granularity).
    Jj,
    /// Partial: rebuild recovers only what the block substrate durably
    /// preserves — one `create` event per extant block, carrying that
    /// block's stamped `_provenance` (its latest authorship: origin + ids +
    /// timestamp). Intermediate field-delta events (the "postponed N times"
    /// stream) left no recoverable trace and are NOT reproduced. The honest
    /// guarantee of the Turso store until full (Loro) fidelity exists.
    Partial,
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
            HistoryFidelity::Partial => "partial",
            HistoryFidelity::None => "none",
        }
    }
}

/// One recorded op/effect in the history stream. Append-only; the store assigns
/// a monotonic sequence so `query` results are totally ordered.
///
/// `op_group` + `old_value` deliberately give this shape the information an
/// "undo entries as data" future (undo ruling: A-shaped-for-C, ADR 0024 later)
/// needs — one op group = one undo entry, `old_value` = the inverse payload.
/// The undo stack is untouched by C2; the door is merely kept open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEvent {
    /// The entity type the op ran on (`block` for block ops).
    pub entity_name: String,
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
    /// Deterministic ADR 0024 effect id, when the op was an effect firing.
    /// Reserved: the engine records `None` until effects flow through the
    /// dispatch chokepoint; the column lands now so the schema is final.
    pub effect_id: Option<String>,
    /// The field that changed (for field-level deltas), enabling
    /// state-transition counts like "moved to `postponed` N times".
    pub field: Option<String>,
    /// String rendering of the prior field value (forensics; inverse payload
    /// for a future undo-entries-as-data consumer).
    pub old_value: Option<String>,
    /// String rendering of the new field value.
    pub new_value: Option<String>,
    /// Wall-clock time (ms since Unix epoch) from the injected clock seam.
    pub at_millis: i64,
    /// Groups the N field-delta events of ONE op. Assigned by the store at
    /// insert (a deterministic monotonic sequence — pure function of table
    /// state and call order, NEVER random, so PBT replay and
    /// rebuild-from-stream stay deterministic). `None` only on a not-yet
    /// recorded event; always `Some` when read back.
    pub op_group: Option<i64>,
}

impl HistoryEvent {
    /// Build the single create-event for a genuinely-new block, in the SAME
    /// shape the engine's `history_events_for` emits for a `create` field-delta
    /// (`field="id"`, `old_value="null"`, `new_value=block_id`). Used by the
    /// doc-ingest path, which mints doc/day PAGE blocks through the ordering
    /// authority (never the engine), so its creates would otherwise leave no
    /// history trace.
    pub fn create_event(
        entity_name: impl Into<String>,
        block_id: impl Into<String>,
        origin: &crate::OpOrigin,
        at_millis: i64,
    ) -> Self {
        let stamp = crate::ProvenanceStamp::from_origin(origin, at_millis);
        let block_id = block_id.into();
        Self {
            entity_name: entity_name.into(),
            block_id: block_id.clone(),
            op_name: "create".to_string(),
            origin: stamp.origin,
            transition_id: stamp.transition_id,
            session_id: stamp.session_id,
            tool_call_id: stamp.tool_call_id,
            effect_id: None,
            field: Some("id".to_string()),
            old_value: Some("null".to_string()),
            new_value: Some(block_id),
            at_millis: stamp.at_millis,
            op_group: None,
        }
    }
}

/// The UTC calendar day (`YYYY-MM-DD`) of an `at_millis` timestamp — the
/// denormalized `day` column the store derives at insert, so day-grouped
/// queries (the Automations journal) never depend on the IVM maintaining an
/// `at_millis`→day expression. Disclosed: UTC, not local time.
pub fn utc_day(at_millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_millis)
        .unwrap_or_else(|| panic!("at_millis {at_millis} outside chrono's datetime range"))
        .date_naive()
        .to_string()
}

/// Filter for stream queries. Every `Some` field is an AND condition; `None`
/// fields are unconstrained. `since`/`until` are inclusive/exclusive on
/// `at_millis` (`since <= at < until`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    pub entity_name: Option<String>,
    pub block_id: Option<String>,
    pub origin: Option<String>,
    pub session_id: Option<String>,
    pub field: Option<String>,
    pub new_value: Option<String>,
    /// UTC calendar day (`YYYY-MM-DD`) — matches the store-derived `day`
    /// column (see [`utc_day`]).
    pub day: Option<String>,
    /// All events of one op (the group the store assigned at insert).
    pub op_group: Option<i64>,
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

/// The parse-don't-validate boundary shape for an external `query_history`
/// request (MCP tool / worker). Every field mirrors [`HistoryQuery`]; `count`
/// selects the count primitive over the row list. `deny_unknown_fields` makes
/// an unknown/misspelled filter key a LOUD deserialization error at the
/// boundary — never a silently-ignored filter that returns the wrong rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQueryArgs {
    #[serde(default)]
    pub entity_name: Option<String>,
    #[serde(default)]
    pub block_id: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub new_value: Option<String>,
    #[serde(default)]
    pub day: Option<String>,
    #[serde(default)]
    pub op_group: Option<i64>,
    #[serde(default)]
    pub since_millis: Option<i64>,
    #[serde(default)]
    pub until_millis: Option<i64>,
    /// Return the match count instead of the event rows.
    #[serde(default)]
    pub count: bool,
}

impl HistoryQueryArgs {
    /// Project the parsed args into the store's [`HistoryQuery`] filter (drops
    /// the `count` flag, which the caller reads separately).
    pub fn into_query(&self) -> HistoryQuery {
        HistoryQuery {
            entity_name: self.entity_name.clone(),
            block_id: self.block_id.clone(),
            origin: self.origin.clone(),
            session_id: self.session_id.clone(),
            field: self.field.clone(),
            new_value: self.new_value.clone(),
            day: self.day.clone(),
            op_group: self.op_group,
            since_millis: self.since_millis,
            until_millis: self.until_millis,
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

    /// Append the field-delta events of ONE completed op as a single group,
    /// in one transaction. The store assigns the batch a fresh `op_group`
    /// (deterministic monotonic sequence). Fed from the dispatch chokepoint.
    /// Fails loud on a write error (never silently drops history).
    async fn record_batch(&self, events: Vec<HistoryEvent>) -> anyhow::Result<()>;

    /// Append one event as its own single-event group. Convenience over
    /// [`Self::record_batch`].
    async fn record(&self, event: HistoryEvent) -> anyhow::Result<()> {
        self.record_batch(vec![event]).await
    }

    /// Events matching `filter`, in append (`seq`) order.
    async fn query(&self, filter: &HistoryQuery) -> anyhow::Result<Vec<HistoryEvent>>;

    /// Count events matching `filter` — the "postponed N times" primitive
    /// (`count(&HistoryQuery::transitions_to(block, "status", "postponed"))`).
    async fn count(&self, filter: &HistoryQuery) -> anyhow::Result<u64>;

    /// Truncate the relation and repopulate it from what the substrate can
    /// prove — the disclosed **partial** rebuild (see the module docs' rebuild
    /// contract and [`HistoryFidelity::Partial`]): one `create` event per
    /// extant block carrying its stamped `_provenance`; the field-delta
    /// stream is not recovered. Deterministic (two rebuilds → identical
    /// rows). Fails loud on a substrate read/write error (never silently
    /// produces a partial relation).
    async fn rebuild(&self) -> anyhow::Result<()>;
}
