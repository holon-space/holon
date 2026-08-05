//! Per-transition expected-SQL budgets, hoisted to `holon-pbt-core`
//! (Phase 1a Step 1) so transition SqlBudget impls can be generic over the
//! reference type (`R: RefSqlCardinality`) and later move into companion
//! `*-testing` crates without depending on `holon-integration-tests`.
//!
//! These are pure counting formulas (no OTEL deps), so they live here
//! unconditionally; the `holon-integration-tests` dispatch + metric machinery
//! that consumes them stays `#[cfg(feature = "otel-testing")]` on that side.

use crate::capabilities::RefSqlCardinality;

pub const REACTIVE_BASE: usize = 5;
pub const JOURNAL_READS: usize = 2;
pub const NAV_DML_READS: usize = 5;

/// The post-navigation re-render's projection + block lookups — the part of a
/// navigate that the DML/journal/reactive constants do not name.
///
/// A measured constant, not a formula: across 63 `NavigateFocus` samples
/// spanning b=27..50 and r=4..6 the total sat flat at 26 reads, so the fan is
/// bounded by the focus root's own scope rather than by vault size. A breach
/// means the navigate render started scaling with the vault, which is the
/// regression this budget exists to catch.
pub const NAV_RENDER_FAN_READS: usize = 14;
pub const CACHE_EVENT_READS: usize = 3;
pub const READS_PER_WATCH: usize = 2;

// ── Click-driven navigation: PINNED, per-transition SQL read ceilings ──
//
// `PinBlock` and `OpenTabViaModifierClick` both reach navigation *through the
// rendered widget tree*, so each resolves the clicked row against a
// resolved-tree snapshot before navigating — reads the nav-drive constants
// (`REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS` = 12) do not model.
//
// The two constants below are that extra cost, PINNED AT THE MEASURED CEILING
// (2026-08-03, ~290 samples over `hand-authored` + keystone at
// 32/64/64/64/64/128/128 cases). They are upper limits taken from observation,
// NOT derived formulas: a breach means the click path grew, and the number must
// be re-measured deliberately, never nudged to make a run pass.
//
// They are SEPARATE because the two transitions measurably differ — one shared
// constant would have to sit at the larger, leaving the cheaper transition
// 5 reads of dead slack in which a regression could hide.
//
// Neither carries state-dependent terms, because every candidate term measured
// ZERO (each was implemented, run, and refuted by telemetry):
// - document count — reads never tracked `docs_tolerance` (which ranged 8–12
//   across the corpus); a docs-scaled tolerance only hides breaches, and did:
//   it swallowed the first teeth run.
// - active user watches — identical reads at watches 0, 1 and 2, because
//   neither transition mutates a block, so no CDC fires and no watch
//   re-evaluates. (Contrast the document-mutating siblings, which correctly pay
//   `watches * READS_PER_WATCH`.) Held sampled forever by the
//   `watch-bearing-click-nav-sql-budget` hand-authored case.
// - first visit to a root — first-visit opens measured the same reads and 0 DDL
//   as revisits, so adding the term would have opened a hole in the ceiling.
//   `NavigateFocus` has since been measured the same way and no longer carries
//   a first-visit term either: matview creation is not on the navigate path.

/// `PinBlock`'s click-resolve cost. Ceiling = 12 + 5 = **17 reads**, measured
/// over 133 shallow-state samples.
///
/// A breach into the ~90–141 read regime is NOT a cost-model gap and must not
/// be absorbed by widening this constant. Measured cause (2026-08-04,
/// known-reds entry 14): reads elevate iff the target's NESTING DEPTH exceeds
/// the main-panel query's depth-20 recursion cap. Past that cap the panel
/// renders no row for the target, so `click_entity_with_modifiers` spins its 2s
/// poll deadline (~41 redundant re-snapshots of the same two `watch_view`
/// SELECTs) and the pin never dispatches.
///
/// Reads track the poll deadline, not state size. Depth 12 → 17 reads; depth
/// 21 → 89; depth 22 → 101. Panel WIDTH is irrelevant: a 40-block FLAT panel
/// renders all 40 rows and pins the 40th for 17 reads.
pub const PIN_BLOCK_CLICK_RESOLVE_READS: usize = 5;

/// `OpenTabViaModifierClick`'s click-resolve cost. Ceiling = 12 + 10 =
/// **22 reads**.
///
/// Sampling caveat worth heeding: a first pass over 39 samples saw a maximum of
/// 21 and the ceiling was set there; the very next 64-case run produced 22 in
/// 56 of 75 draws. A few dozen samples do NOT characterize this mode.
pub const OPEN_TAB_CLICK_RESOLVE_READS: usize = 10;

/// The reactive render coalesces nondeterministically, so a click-nav
/// transition re-reads `focus_roots` / `current_focus` either 3 or 4 times for
/// the same work — the 21-vs-22 bimodality in the corpus is exactly this one
/// redundant read, and the N+1 report calls it out as "identical bindings —
/// redundant".
///
/// A *fixed* one-read pad for that jitter, shared by both click transitions
/// because it models the same coalescing mechanism. Deliberately NOT
/// `docs_tolerance`: each ceiling stays hard and state-independent
/// (`<transition>_CLICK_RESOLVE_READS` + 1), it just does not pretend a
/// coalescing-dependent counter is exact. Widening this is not the fix for a
/// breach — a breach means the click path grew.
pub const CLICK_JITTER_TOLERANCE: usize = 1;

/// Per-variant budget files share this tolerance helper. Base jitter (4)
/// plus extra matview checks (~2 reads per extra doc) for restarts reusing
/// matviews via `ensure_view`.
pub fn docs_tolerance<R: RefSqlCardinality>(state: &R) -> usize {
    let docs = state.document_count();
    4 + if docs > 1 { (docs - 1) * 2 } else { 0 }
}

/// Expected SQL counts for a transition, computed from current state.
#[derive(Debug)]
pub struct ExpectedSql {
    /// Expected number of SQL reads (via turso query())
    pub reads: usize,
    /// Expected number of SQL writes (via turso execute())
    pub writes: usize,
    /// Expected number of DDL statements
    pub ddl: usize,
    /// Tolerance: actual may exceed expected by this many (for async race
    /// margins)
    pub tolerance: usize,
}

/// Per-transition SQL budget. Separated from the behaviour trait
/// (`holon_pbt_core::TransitionImpl`) because the budget is an
/// integration-test concern that has no meaning for the layout /
/// editor-pure PBTs — those slices never touch SQL. Each transition
/// variant implements this; the `E2ETransition` enum dispatches it.
///
/// Generic over the reference type via [`RefSqlCardinality`] (Phase 1a Step 1):
/// impls read state only through the cardinality accessors, so they no longer
/// bind the concrete `ReferenceState`.
pub trait SqlBudget {
    fn expected_sql<R: RefSqlCardinality>(&self, state: &R) -> ExpectedSql;
}

/// Mutation kind discriminant — avoids constructing dummy Mutation values.
/// (`from_mutation` stays in `holon-integration-tests`, next to the concrete
/// `Mutation` type it maps from.)
pub enum MutationKind {
    Create,
    Update,
    Delete,
    Move,
    RestartApp,
}

/// CDC cascade tolerance. Multi-doc amplifies heavily: org sync re-writes ALL
/// documents, each triggering CDC events with name IS NULL polls + property
/// lookups. The cross-doc cost scales with blocks × (docs-1).
pub fn cdc_tolerance(blocks: usize, docs: usize) -> usize {
    if docs > 1 {
        4 + blocks / 2 + (docs - 1) * blocks / 3
    } else {
        4 + blocks / 3
    }
}

/// Expected SQL for a specific mutation type. Pure counting formula derived
/// from HOLON_PERF_DETAIL span analysis.
pub fn expected_sql_for_kind(
    kind: MutationKind,
    watches: usize,
    blocks: usize,
    docs: usize,
) -> ExpectedSql {
    let tol = cdc_tolerance(blocks, docs);
    match kind {
        MutationKind::Create => ExpectedSql {
            reads: REACTIVE_BASE
                + CACHE_EVENT_READS
                + 2
                + 2
                + 1
                + 2
                + CACHE_EVENT_READS
                + 1
                + watches * (READS_PER_WATCH + 2),
            writes: 2 + watches.min(2),
            ddl: 0,
            tolerance: tol,
        },
        MutationKind::Update => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + 2 + 1 + 1 + 1 + 2 + watches * READS_PER_WATCH,
            writes: 3,
            ddl: 0,
            tolerance: tol,
        },
        MutationKind::Delete => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + 3 + 1 + 1 + 1 + 2 + watches * READS_PER_WATCH,
            writes: 3,
            ddl: 0,
            tolerance: tol,
        },
        MutationKind::Move => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + 2 + 1 + 1 + 1 + 2 + watches * READS_PER_WATCH,
            writes: 3,
            ddl: 0,
            tolerance: tol,
        },
        MutationKind::RestartApp => ExpectedSql {
            reads: REACTIVE_BASE + 4,
            writes: 2,
            ddl: 0,
            tolerance: 3,
        },
    }
}

// ── SqlBudget for the shared interaction transitions ──────────────
// These transition *types* live in `holon-pbt-core::interactions`, so their
// `SqlBudget` impls must live here too (orphan rule): both trait and type are
// this crate's. (Phase 1a Step 1 — moved out of holon-integration-tests.)
use crate::interactions::DeliverBlockContent;
use crate::interactions::SwitchViewMode;
use crate::interactions::ToggleCollapse;
use crate::interactions::ToggleDrawer;

macro_rules! reactive_view_budget {
    ($ty:ty) => {
        impl SqlBudget for $ty {
            fn expected_sql<R: RefSqlCardinality>(&self, state: &R) -> ExpectedSql {
                ExpectedSql {
                    reads: REACTIVE_BASE + 10,
                    writes: 0,
                    ddl: 0,
                    tolerance: docs_tolerance(state) + 5,
                }
            }
        }
    };
}
reactive_view_budget!(DeliverBlockContent);
reactive_view_budget!(SwitchViewMode);
reactive_view_budget!(ToggleDrawer);

// ToggleCollapse is no longer a pure reactive-view flip: collapse is document
// state (2026-07-11 ruling), so the chevron click also dispatches
// `set_field(collapsed)` — one block-update's worth of SQL on top of the
// reactive base.
impl SqlBudget for ToggleCollapse {
    fn expected_sql<R: RefSqlCardinality>(&self, state: &R) -> ExpectedSql {
        let update = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        ExpectedSql {
            reads: REACTIVE_BASE + 10 + update.reads,
            writes: update.writes,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5 + update.tolerance,
        }
    }
}
