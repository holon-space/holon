//! Slice-agnostic test doubles for the composed-invariant catalog, plus a small
//! prelude so a per-invariant test module needs only
//! `use crate::pbt::composed::fixtures::*`.
//!
//! The SUT doubles ([`FixtureBackend`], [`BuggyEditor`], [`FixtureLoroLog`],
//! [`FixtureSqlProjection`], …) hand-craft states the real validating APIs can't
//! produce, to drive invariants to *failure*. They don't touch a real backend —
//! the catch tests run without Turso/Loro/MemoryBackend.
//!
//! The **reference** side is no longer faked here: slices and catch tests seed
//! the real [`ReferenceState`](crate::pbt::reference_state::ReferenceState) oracle
//! via [`seed_ref`](crate::pbt::composed::subsystem_seed::seed_ref) /
//! [`seed_ref_with_editor`](crate::pbt::composed::subsystem_seed::seed_ref_with_editor)
//! (F2 Stage 2 — the hand-rolled `FixtureRef`/`FixtureEditorRef`/`EditorModel`
//! ref models were retired). The text primitive both sides share lives in
//! `holon_frontend::editor_caret`.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub use holon_api::{Block, BlockContent, ContentType, EntityUri};
use holon_pbt_core::capabilities::{
    SutBackend, SutEditorMirrorRead, SutErrorLog, SutLoroLog, SutLoroTaskState, SutSqlProjection,
    SutViewSelection,
};
pub use holon_pbt_core::composition::{CapMap, run_selected};
use holon_pbt_core::composition::{CapProvider, Config};

// Prelude: the catalog a test drives, re-exported so `fixtures::*` is a one-line
// import. (Builders are slice-specific and imported from each slice directly.)
pub use super::catalog::composed_invariant_catalog;

pub fn uri(s: &str) -> EntityUri {
    EntityUri::parse(s).expect("valid test EntityUri")
}

// ─── Block-tree SUT/Ref doubles ──────────────────────────────────────────

/// A fixture SUT that returns a hand-crafted block list — used to drive the
/// structural invariants to *failure*, which the real `MemoryBackend` API
/// (which enforces the domain rules at write time) cannot construct. Same
/// `SutBackend` cap, so it composes through the identical selection path.
pub struct FixtureBackend {
    pub blocks: Vec<Block>,
}

#[async_trait::async_trait(?Send)]
impl SutBackend for FixtureBackend {
    async fn live_block_snapshot(&self) -> Vec<Block> {
        self.blocks.clone()
    }
    async fn block_raw_snapshot(&self) -> Vec<Block> {
        self.blocks.clone()
    }
    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

impl CapProvider for FixtureBackend {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutBackend>);
    }
}

pub fn fixture_slice(blocks: Vec<Block>) -> CapMap {
    Config::new().with(FixtureBackend { blocks }).build()
}

// ─── Editor (second SUT component) doubles ───────────────────────────────

/// A SUT editor with hand-set, deliberately-wrong live text/caret — drives
/// the editor invariants to *failure*, which a correct `InMemEditorComponent`
/// cannot produce. Same `SutEditorMirrorRead` cap, identical selection path.
pub struct BuggyEditor {
    pub block: EntityUri,
    pub text: String,
    pub caret: usize,
}

impl SutEditorMirrorRead for BuggyEditor {
    fn editor_caret_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String> {
        if block_id == &self.block {
            Ok(Some(self.caret))
        } else {
            Ok(None)
        }
    }
    fn editor_live_text(&self, block_id: &EntityUri) -> Result<String, String> {
        if block_id == &self.block {
            Ok(self.text.clone())
        } else {
            Err(format!("no editor for {block_id}"))
        }
    }
}

impl CapProvider for BuggyEditor {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutEditorMirrorRead>);
    }
}

pub fn buggy_editor_map(editor: BuggyEditor) -> CapMap {
    Config::new().with(editor).build()
}

// ─── Loro-log SUT double ─────────────────────────────────────────────────

/// A hand-crafted [`SutLoroLog`] SUT — a Loro store that can be told to *report
/// an error* (`had_errors`) or to hand back a *deliberately mis-ordered* child
/// list (`children`), neither of which the real `LoroBackendComponent` (a valid
/// CRDT) can produce. Same `SutLoroLog` cap, identical selection path, so the
/// Loro invariants' catch tests run with no real Loro tree.
#[derive(Default)]
pub struct FixtureLoroLog {
    pub had_errors: bool,
    /// `parent stable-id → ordered child stable-ids`, as `loro_children_of`
    /// reports them. A parent absent from the map yields `None` (the body skips
    /// it), matching a parent not represented in the tree.
    pub children: std::collections::HashMap<String, Vec<String>>,
}

#[async_trait::async_trait(?Send)]
impl SutLoroLog for FixtureLoroLog {
    async fn loro_had_errors(&self) -> bool {
        self.had_errors
    }
    async fn loro_children_of(&self, parent: &str) -> Option<Vec<String>> {
        self.children.get(parent).cloned()
    }
    /// Unused by the wired Loro invariants (`loro_no_errors`,
    /// `loro_children_match_ref` read only the two methods above); honest `None`.
    async fn loro_block_snapshot(&self) -> Option<Vec<Block>> {
        None
    }
}

impl CapProvider for FixtureLoroLog {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutLoroLog>);
    }
}

pub fn loro_log_map(log: FixtureLoroLog) -> CapMap {
    Config::new().with(log).build()
}

// ─── Error-log SUT double ─────────────────────────────────────────────────

/// A hand-crafted [`SutErrorLog`] SUT — its `error_count` can be set non-zero to
/// inject what a clean frontend session can't, so the `inv-no-errors` catch test
/// runs without a real `FrontendSession`. Same `SutErrorLog` cap, identical
/// selection path.
#[derive(Default)]
pub struct FixtureErrorLog {
    pub error_count: usize,
    pub context: Vec<String>,
}

#[async_trait::async_trait(?Send)]
impl SutErrorLog for FixtureErrorLog {
    async fn app_error_count(&self) -> usize {
        self.error_count
    }
    async fn app_error_context(&self) -> Vec<String> {
        self.context.clone()
    }
}

impl CapProvider for FixtureErrorLog {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutErrorLog>);
    }
}

pub fn error_log_map(log: FixtureErrorLog) -> CapMap {
    Config::new().with(log).build()
}

// ─── SQL-projection SUT double ────────────────────────────────────────────

/// A hand-crafted [`SutSqlProjection`] SUT — its `block_content` map can be set
/// to *diverge* from the reference, which the real `SqlProjectionComponent`
/// (writing through the production block operations) cannot produce. Same
/// `SutSqlProjection` cap, identical selection path, so the SQL-projection
/// invariants' catch tests run with no real Turso engine. Only the methods the
/// wired SQL invariants read carry data; the rest are honest empties.
#[derive(Default)]
pub struct FixtureSqlProjection {
    /// `id → content` as the SQL `block_raw.content` projection reports it.
    pub content: HashMap<EntityUri, String>,
    /// `id → task_state` as the SQL `json_extract(properties,'$.task_state')`
    /// projection reports it — set on the catch path to *diverge* from the Loro
    /// projection, which a synced store can't produce.
    pub task_state: HashMap<EntityUri, String>,
}

#[async_trait::async_trait(?Send)]
impl SutSqlProjection for FixtureSqlProjection {
    async fn block_content(&self, id: &EntityUri) -> Option<String> {
        self.content.get(id).cloned()
    }

    async fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        // Union of both projections so `inv-task-state-storage-coherence`
        // (which iterates `all_block_ids`) visits the task_state-only ids too.
        self.content
            .keys()
            .chain(self.task_state.keys())
            .cloned()
            .collect()
    }

    // ─── honest empties (no wired invariant reads these on the catch path) ──
    async fn block_row(&self, _: &EntityUri) -> Option<Vec<String>> {
        None
    }
    async fn sorted_children(&self, _: &EntityUri) -> Vec<EntityUri> {
        Vec::new()
    }
    async fn watch_row_count(&self, _: &str) -> Option<usize> {
        None
    }
    async fn block_raw_row(&self, _: &EntityUri) -> Option<Vec<String>> {
        None
    }
    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri> {
        BTreeSet::new()
    }
    async fn block_task_state(&self, id: &EntityUri) -> Option<String> {
        self.task_state.get(id).cloned()
    }
}

impl CapProvider for FixtureSqlProjection {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutSqlProjection>);
    }
}

/// Build a SUT `CapMap` exposing only `SutSqlProjection` over a hand-set
/// `id → content` map.
pub fn sql_projection_map(content: Vec<(EntityUri, &str)>) -> CapMap {
    let content = content
        .into_iter()
        .map(|(id, c)| (id, c.to_string()))
        .collect();
    Config::new()
        .with(FixtureSqlProjection {
            content,
            task_state: HashMap::new(),
        })
        .build()
}

// ─── Loro task_state SUT double ───────────────────────────────────────────

/// A hand-crafted [`SutLoroTaskState`] SUT — its `id → task_state` map is the
/// Loro projection side of `inv-task-state-storage-coherence`. Paired with a
/// [`FixtureSqlProjection`] whose `task_state` map *disagrees*, it drives the
/// coherence invariant to failure — a Loro↔SQL desync the synced
/// `LoroBackendComponent`/`SqlProjectionComponent` pair can't produce.
#[derive(Default)]
pub struct FixtureLoroTaskState {
    /// `id → task_state` as the Loro `properties["task_state"]` projection
    /// reports it. An id absent from the map yields `None`.
    pub task_state: HashMap<EntityUri, String>,
}

#[async_trait::async_trait(?Send)]
impl SutLoroTaskState for FixtureLoroTaskState {
    async fn loro_task_state_of(&self, block_id: &str) -> Option<String> {
        self.task_state
            .iter()
            .find(|(id, _)| id.as_str() == block_id)
            .map(|(_, state)| state.clone())
    }
}

impl CapProvider for FixtureLoroTaskState {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutLoroTaskState>);
    }
}

/// Build a SUT `CapMap` hosting **both** `SutSqlProjection` and
/// `SutLoroTaskState` over hand-set `id → task_state` maps — the two-cap SUT
/// `inv-task-state-storage-coherence` needs. The SQL projection's `all_block_ids`
/// is driven by the SQL `task_state` map (the invariant iterates it), and the
/// per-side maps can be set to agree (positive) or diverge (catch).
pub fn task_state_maps(sql: Vec<(EntityUri, &str)>, loro: Vec<(EntityUri, &str)>) -> CapMap {
    let to_map = |v: Vec<(EntityUri, &str)>| {
        v.into_iter()
            .map(|(id, s)| (id, s.to_string()))
            .collect::<HashMap<_, _>>()
    };
    Config::new()
        .with(FixtureSqlProjection {
            content: HashMap::new(),
            task_state: to_map(sql),
        })
        .with(FixtureLoroTaskState {
            task_state: to_map(loro),
        })
        .build()
}

// ─── ViewModel SUT double ─────────────────────────────────────────────────

/// A hand-crafted [`SutViewSelection`] SUT — its `headless_error_node_count` can be
/// set to a non-zero count, which a real `HeadlessFrontendComponent` (rendering
/// a valid tree) never produces. Same `SutViewSelection` cap, identical selection
/// path, so the frontend invariants' catch tests run with no real engine. Only
/// `headless_error_node_count` carries data; the rest are honest defaults.
pub struct FixtureViewModel {
    /// What `headless_error_node_count` reports: `None` = tree not ready (Skip),
    /// `Some(0)` = clean, `Some(n)` = `n` error widgets.
    pub error_count: Option<usize>,
}

#[async_trait::async_trait(?Send)]
impl SutViewSelection for FixtureViewModel {
    async fn headless_error_node_count(&self) -> Option<usize> {
        self.error_count
    }
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        Vec::new()
    }
    async fn current_view(&self) -> String {
        "all".to_string()
    }
}

impl CapProvider for FixtureViewModel {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutViewSelection>);
    }
}

/// Build a SUT `CapMap` exposing only `SutViewSelection` with a fixed error count.
pub fn viewmodel_map(error_count: Option<usize>) -> CapMap {
    Config::new().with(FixtureViewModel { error_count }).build()
}

/// A hand-crafted [`ComposedBudget`] SUT — returns a canned [`SqlBudgetReport`],
/// so the `inv-sql-budget` catch test can inject an *enforced* violation without a
/// real span collector / `MetricsSut` lifecycle (the production
/// [`ComposedSpanMetrics`] needs `note_transition_start`/`freeze_for_check` driven
/// by the harness, which the teeth don't run). Same `ComposedBudget` cap,
/// identical selection path.
///
/// [`ComposedBudget`]: crate::pbt::composed::span_metrics::ComposedBudget
/// [`ComposedSpanMetrics`]: crate::pbt::composed::span_metrics::ComposedSpanMetrics
#[cfg(feature = "otel-testing")]
#[derive(Default)]
pub struct FixtureBudget {
    pub enforce: bool,
    pub errors: Vec<String>,
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::composed::span_metrics::ComposedBudget for FixtureBudget {
    fn budget_report(&self) -> crate::pbt::invariants::bodies::sql_budget::SqlBudgetReport {
        crate::pbt::invariants::bodies::sql_budget::SqlBudgetReport {
            enforce: self.enforce,
            errors: self.errors.clone(),
        }
    }
}

#[cfg(feature = "otel-testing")]
impl CapProvider for FixtureBudget {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn crate::pbt::composed::span_metrics::ComposedBudget>);
    }
}

/// Build a SUT `CapMap` exposing only `ComposedBudget` over a canned report.
#[cfg(feature = "otel-testing")]
pub fn budget_map(b: FixtureBudget) -> CapMap {
    Config::new().with(b).build()
}
