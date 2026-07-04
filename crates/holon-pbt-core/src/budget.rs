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
pub const CACHE_EVENT_READS: usize = 3;
pub const READS_PER_WATCH: usize = 2;

/// First navigation to a root renders it for the first time: each watch
/// matview takes `ensure_view`'s create path (2 sqlite_master existence
/// checks + the initial view SELECT). Measured on the minimal
/// WriteOrgFile×2 → StartApp → NavigateFocus capture: 6 new views ⇒ ~18
/// reads + 6 CREATEs.
pub const FIRST_VISIT_VIEW_READS: usize = 18;
pub const FIRST_VISIT_VIEW_DDL: usize = 6;

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
reactive_view_budget!(ToggleCollapse);
reactive_view_budget!(ToggleDrawer);
