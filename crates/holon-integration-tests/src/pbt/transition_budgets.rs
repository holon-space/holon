//! Per-transition performance budgets.
//!
//! SQL counts are **deterministic** — they depend on the number of active watches,
//! documents, blocks, etc. They are computed from `ReferenceState`, not recorded.
//!
//! Timing is **non-deterministic** — wall-clock and query durations are checked
//! against generous hard limits only.

use std::time::Duration;

use super::reference_state::ReferenceState;
use super::transitions::E2ETransition;
use super::types::Mutation;
use crate::test_tracing::TransitionMetrics;

// ── SQL count model ───────────────────────────────────────────────
//
// Every post-startup transition has a "base" read overhead from the reactive
// engine checking what to re-render:
//
//   REACTIVE_BASE = 5:
//     1× SELECT ... FROM block (full block for render source)
//     1× SELECT region, block_id FROM current_focus
//     3× SELECT root_id AS id FROM focus_roots WHERE region = '{region}'
//
// UI mutations go through the operation journal (undo/redo tracking):
//
//   JOURNAL_READS = 2:
//     1× UPDATE operation SET status = ...     (clear redo stack)
//     1× INSERT INTO operation (...) RETURNING id  (insert + get ID in one query)
//   (COUNT(*) for trim is amortized to every 10th operation)
//
// Navigation operations execute DML tracked as "query" spans:
//
//   NAV_DML_READS = 5:
//     1× DELETE FROM navigation_history WHERE region = ... AND id > ...
//     1× INSERT INTO navigation_history (region, block_id) VALUES (...)
//     1× INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES (...)
//     1× SELECT MAX(id) FROM navigation_history WHERE region = ...
//     1× SELECT history_id FROM navigation_cursor WHERE region = ...
//
// Org sync CDC events trigger cache subscriber reads:
//
//   CACHE_EVENT_READS = 3:
//     2× SELECT id FROM block WHERE name IS NULL     (one per CDC event)
//     1× SELECT id, properties FROM block WHERE properties IS NOT NULL
//
// User watches (from SetupWatch) add matview existence checks:
//
//   READS_PER_WATCH = 2:
//     1× SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_...'
//     1× SELECT * FROM watch_view_...
//
// NOTE: Internal watches (region watches, all-blocks watch, structural watch_ui)
// use subscribe_sql → matview CDC broadcast and do NOT generate "query" spans
// during post-startup transitions. Only user watches from SetupWatch contribute.

const REACTIVE_BASE: usize = 5;
const JOURNAL_READS: usize = 2;
const NAV_DML_READS: usize = 5;
const CACHE_EVENT_READS: usize = 3;
const READS_PER_WATCH: usize = 2;

/// Expected SQL counts for a transition, computed from current state.
#[derive(Debug)]
pub struct ExpectedSql {
    /// Expected number of SQL reads (via turso query())
    pub reads: usize,
    /// Expected number of SQL writes (via turso execute())
    pub writes: usize,
    /// Expected number of DDL statements
    pub ddl: usize,
    /// Tolerance: actual may exceed expected by this many (for async race margins)
    pub tolerance: usize,
}

/// Compute expected SQL counts for a transition given the current reference state.
///
/// The formulas are derived from SQL span analysis (HOLON_PERF_DETAIL=1, 2026-04-05).
/// When a formula doesn't match reality, it means either:
/// 1. The code changed (update the formula), or
/// 2. There's an N+1 bug (fix the code).
///
/// **Tolerance** accounts for CDC-driven re-render cascades: when a mutation
/// triggers org sync (file re-write → file watcher → re-parse → CDC events),
/// each cascade cycle adds parent chain walks and property lookups proportional
/// to the number of blocks in the affected document.
pub fn expected_sql(transition: &E2ETransition, ref_state: &ReferenceState) -> ExpectedSql {
    let watches = ref_state.active_watches.len();
    let blocks = ref_state.block_state.blocks.len();
    let docs = ref_state.documents.len();
    // Base jitter tolerance (4) + extra docs add matview checks (~2 reads per extra doc).
    // The base of 4 accounts for view_exists checks when matviews are reused across
    // restarts (ensure_view checks sqlite_master instead of CREATE'ing fresh).
    let docs_tolerance = 4 + if docs > 1 { (docs - 1) * 2 } else { 0 };

    use E2ETransition::*;
    match transition {
        E2ETransition::Nothing => ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        },

        // Pre-startup: filesystem only, no SQL
        WriteOrgFile { .. }
        | CreateDirectory { .. }
        | GitInit
        | JjGitInit
        | CreateStaleLoro { .. } => ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        },

        // StartApp: highly variable — schema DDL, file sync, matview creation.
        // Too many async phases to model precisely. Use generous bounds.
        StartApp { .. } => ExpectedSql {
            reads: 200,
            writes: 60,
            ddl: 300,
            tolerance: 80,
        },

        // SwitchView: reactive base reads. Usually exact, rare +1-2 from timing jitter.
        SwitchView { .. } => ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance,
        },

        // RemoveWatch: reactive base only (Turso keeps matview alive).
        RemoveWatch { .. } => ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance,
        },

        // SetupWatch: reactive base (5) + view existence check (2) + turso internal check (1)
        //   + initial matview data read (1) = 9 reads, 0 writes, 1 DDL.
        // Pending CDC events from prior transitions drain during SetupWatch,
        // adding reactive cycles proportional to the number of dirtied blocks.
        // Pathological cases (CreateStaleLoro disrupting StartApp init) defer
        // matview creation here, requiring large ddl/write tolerance.
        SetupWatch { .. } => ExpectedSql {
            reads: REACTIVE_BASE + 2 + 1 + 1,
            writes: 0,
            ddl: 1,
            tolerance: docs_tolerance + blocks * 6,
        },

        // NavigateFocus: reactive base (5) + journal (4) + nav DML (5) = 14 reads.
        //   Observed: 14 with 1 doc, 16 with 2 docs (extra matview checks for second doc).
        NavigateFocus { .. } => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance,
        },

        // NavigateBack/Forward/Home: same journal + nav DML path as NavigateFocus.
        //   NavigateBack/Forward omit DELETE+INSERT history (2 fewer nav reads).
        NavigateBack { .. } | NavigateForward { .. } => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS - 2,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance,
        },
        NavigateHome { .. } => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance,
        },

        // ClickBlock: navigate_focus + shadow index build from current_view_model().
        // The snapshot() call queries block data to build the ViewModel tree.
        ClickBlock { .. } => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance + 5,
        },

        // ArrowNavigate: shadow index walk (pure Rust, no SQL) + navigate_focus at end.
        // Extra reads from content queries during boundary prediction.
        ArrowNavigate { steps, .. } => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + (*steps as usize * 2),
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance + (*steps as usize * 2),
        },

        // SimulateRestart: clears last_projection → re-sync → file write.
        //   reactive base (5) + name IS NULL (2) + properties (1) + doc/block existence (1)
        //   = 9 reads, 2 writes. Variable: 7 when no re-sync needed, 11 with double CDC cycle.
        SimulateRestart => ExpectedSql {
            reads: REACTIVE_BASE + 4,
            writes: 2,
            ddl: 0,
            tolerance: 3 + docs_tolerance,
        },

        // CreateDocument: file write → org sync → block upserts.
        //   reactive base (5) + cache events (3) + doc name poll (2) + block existence (2)
        //   + per-watch: matview existence + data read (READS_PER_WATCH)
        //   = 12 + watches × 2 reads, 4 writes.
        //   Observed: 12 (0w/1d), 16-18 (1w/1d), up to 26 (1w/2d — CDC cascades across docs).
        CreateDocument { .. } => ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 4 + watches * READS_PER_WATCH,
            writes: 4,
            ddl: 0,
            // Use blocks + 5 since CreateDocument itself adds blocks to the DB.
            tolerance: cdc_tolerance(blocks + 5, docs + 1) + watches * 4,
        },

        // ApplyMutation: depends on mutation type and active watches
        ApplyMutation(event) => expected_mutation_sql(&event.mutation, watches, blocks, docs),

        // BulkExternalAdd: N blocks via org file write → org sync → block upserts.
        //   Reads: reactive base (5) + cache events (3) + doc lookup (1)
        //     + block existence/resolve (~1 per new block)
        //     + per-watch: matview checks + data read (READS_PER_WATCH).
        //   Writes: N× UPDATE events processed + INSERT event + INSERT block.
        //   DDL: 0-watches (matview creation for newly dirtied watches).
        //   Observed: reads=13-21, writes=10-22 (scales with N).
        BulkExternalAdd {
            blocks: new_blocks, ..
        } => {
            let n = new_blocks.len();
            ExpectedSql {
                reads: REACTIVE_BASE + CACHE_EVENT_READS + 1 + n + watches * READS_PER_WATCH,
                writes: n + 2,
                ddl: watches,
                // Each new block triggers org sync CDC; reactive base fires per CDC cycle.
                tolerance: cdc_tolerance(blocks + n, docs) + n * 3,
            }
        }

        // ConcurrentSchemaInit: DDL re-init (similar to StartApp DDL phase).
        ConcurrentSchemaInit => ExpectedSql {
            reads: 100,
            writes: 30,
            ddl: 250,
            tolerance: 50,
        },

        // ConcurrentMutations: two mutations without sync.
        // Worst case: 2× mutation reads/writes.
        ConcurrentMutations {
            ui_mutation,
            external_mutation,
        } => {
            let a = expected_mutation_sql(&ui_mutation.mutation, watches, blocks, docs);
            let b = expected_mutation_sql(&external_mutation.mutation, watches, blocks, docs);
            ExpectedSql {
                reads: a.reads + b.reads,
                writes: a.writes + b.writes,
                ddl: a.ddl + b.ddl,
                tolerance: a.tolerance + b.tolerance,
            }
        }

        // EditViaDisplayTree / EditViaViewModel: render → dispatch set_field.
        // SQL-wise same as ApplyMutation::Update (render is CPU-only, no extra SQL).
        EditViaDisplayTree { .. } | EditViaViewModel { .. } | ToggleState { .. } => {
            expected_sql_for_kind(MutationKind::Update, watches, blocks, docs)
        }

        // TriggerSlashCommand: render → trigger → delete operation.
        TriggerSlashCommand { .. } => {
            expected_sql_for_kind(MutationKind::Delete, watches, blocks, docs)
        }

        // TriggerDocLink: read-only (no block state change).
        TriggerDocLink { .. } => ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        },

        // Structural operations: indent/outdent/move touch 1-2 blocks.
        // SplitBlock creates a new block + updates original.
        Indent { .. } | Outdent { .. } | MoveUp { .. } | MoveDown { .. } | DragDropBlock { .. } => {
            let mut sql = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
            sql.tolerance += 5; // extra margin for ordering operations
            sql
        }
        SplitBlock { .. } => {
            let update = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
            let create = expected_sql_for_kind(MutationKind::Create, watches, blocks, docs);
            ExpectedSql {
                reads: update.reads + create.reads - REACTIVE_BASE, // shared base
                writes: update.writes + create.writes,
                ddl: 0,
                tolerance: update.tolerance + create.tolerance,
            }
        }
        // JoinBlock is the inverse: 1 update (prev's content) + 1 delete (current).
        JoinBlock { .. } => {
            let update = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
            let delete = expected_sql_for_kind(MutationKind::Delete, watches, blocks, docs);
            ExpectedSql {
                reads: update.reads + delete.reads - REACTIVE_BASE,
                writes: update.writes + delete.writes,
                ddl: 0,
                tolerance: update.tolerance + delete.tolerance,
            }
        }

        // EmitMcpData: real MCP pipeline — resource fetch → sync engine diff →
        // cache apply_batch → Turso write. Reads: cache get_all_ids + get_all
        // for full-sync diff. Writes: cache INSERT for new entity.
        // CDC cascade from previous transitions drains here too. Pathological
        // cases (CreateStaleLoro disrupting StartApp init) defer matview
        // creation into this transition, so tolerance scales with blocks.
        EmitMcpData => ExpectedSql {
            reads: 4,
            writes: 2,
            ddl: 0,
            tolerance: docs_tolerance + blocks * 6,
        },

        // Undo/Redo: replays an operation — similar to the original mutation.
        UndoLastMutation | Redo => {
            let mut sql = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
            sql.tolerance += 5; // undo journal adds a few extra reads
            sql
        }

        // Peer transitions are Loro-only in PBT (LoroModule not wired).
        // AddPeer: export_snapshot triggers ~5 SQL reads (store persistence).
        // Others: async CDC drain from previous transitions can land here.
        // In production, SyncWithPeer / MergeFromPeer fire Loro's
        // `subscribe_root` callback, which wakes `LoroSyncController` to
        // reconcile the diff into the command/event bus.
        AddPeer
        | PeerEdit { .. }
        | PeerCharEdit { .. }
        | SyncWithPeer { .. }
        | MergeFromPeer { .. } => ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        },

        // Atomic editor primitives: pure InputState mutations (Focus,
        // MoveCursor, TypeChars, DeleteBackward, Blur) issue no SQL on
        // their own — the commit happens later via PressKey or Blur.
        // PressKey is variable: dispatching Enter→split fires create+update,
        // Tab→indent fires update, Escape fires nothing. Use the most
        // permissive bound and let real chord handlers settle.
        FocusEditableText { .. }
        | MoveCursor { .. }
        | TypeChars { .. }
        | DeleteBackward { .. }
        | Blur => ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        },
        PressKey { .. } => {
            let update = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
            let create = expected_sql_for_kind(MutationKind::Create, watches, blocks, docs);
            ExpectedSql {
                reads: update.reads + create.reads - REACTIVE_BASE,
                writes: update.writes + create.writes,
                ddl: 0,
                tolerance: update.tolerance + create.tolerance,
            }
        }
    }
}

/// Mutation kind discriminant — avoids constructing dummy Mutation values.
enum MutationKind {
    Create,
    Update,
    Delete,
    Move,
    RestartApp,
}

impl MutationKind {
    fn from_mutation(m: &Mutation) -> Self {
        match m {
            Mutation::Create { .. } => Self::Create,
            Mutation::Update { .. } => Self::Update,
            Mutation::Delete { .. } => Self::Delete,
            Mutation::Move { .. } => Self::Move,
            Mutation::RestartApp => Self::RestartApp,
        }
    }
}

/// Expected SQL for a specific mutation type.
///
/// ## Read breakdown (from HOLON_PERF_DETAIL=1 analysis, 2026-04-05):
///
/// **Create** (external, via org file write — no operation journal):
///   reactive base (5) + cache events (3) = 8 reads
///   + per-watch: matview existence + data read (READS_PER_WATCH)
///   + per-watch: block_with_path + render source load (2)
///   Observed: 8 (0 watches), 14 (1 watch)
///
/// **Update** (UI dispatch — has operation journal):
///   reactive base (5) + journal (4) + parent chain walk (4)
///   + block content fetch (1) + name IS NULL (1) + properties IS NOT NULL (1)
///   = 16 reads
///   + per-watch: matview existence + data read (READS_PER_WATCH)
///   + per-watch: block_with_path + render source load (2)
///   Observed: 16 (0 watches), 18 (1 watch)
///
/// **Delete**: like Update + doc_uri lookup (1)
///   = 17 + watches × (READS_PER_WATCH + 2)
///
/// ## CDC cascade tolerance
///
/// When a mutation triggers org sync, the org file gets re-written, which triggers
/// file watcher → re-parse → CDC events. Each cascade cycle adds:
///   - name IS NULL checks (1-2 per event)
///   - property lookups (1-2 per affected block)
///
/// After the recursive CTE fix for find_document_uri, parent chain walks no longer
/// scale with block count (O(1) instead of O(depth)). The remaining CDC overhead
/// is mostly constant per mutation.
fn cdc_tolerance(blocks: usize, docs: usize) -> usize {
    // Empirical after CTE fix: CDC overhead is much flatter for single-doc.
    // Multi-doc amplifies heavily: org sync re-writes ALL documents, each
    // triggering CDC events with name IS NULL polls + property lookups.
    // The cross-doc cost scales with blocks × (docs-1).
    if docs > 1 {
        4 + blocks / 2 + (docs - 1) * blocks / 3
    } else {
        4 + blocks / 3
    }
}

fn expected_sql_for_kind(
    kind: MutationKind,
    watches: usize,
    blocks: usize,
    docs: usize,
) -> ExpectedSql {
    let tol = cdc_tolerance(blocks, docs);
    match kind {
        // Create goes through external mutation (org file write), no operation journal.
        // reactive base (5) + cache events (3) + find_document_uri CTE (2) + properties (2)
        // = 12 reads observed.
        // Loro outbound reconcile adds: post-update row read (1) + find_document_uri (2)
        //   + cache event reads (3) + properties merge (1) = 7.
        // Per-watch: matview existence + data read + block_with_path + render source load = 4.
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
        // Update (external path): reactive base (5) + find_document_uri CTE (2)
        //   + name IS NULL (1) + properties IS NOT NULL (1) + per-block properties (2)
        //   = 11 reads observed.
        // Update (UI path): adds journal (4) + block content fetch (1) = 16 reads.
        // Per-watch: matview existence + data read = 2.
        MutationKind::Update => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + 2 + 1 + 1 + 1 + 2 + watches * READS_PER_WATCH,
            writes: 3,
            ddl: 0,
            tolerance: tol,
        },
        // Delete: like Update + doc_uri extra CTE (1).
        MutationKind::Delete => ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + 3 + 1 + 1 + 1 + 2 + watches * READS_PER_WATCH,
            writes: 3,
            ddl: 0,
            tolerance: tol,
        },
        // Move: 2× find_document_uri CTE (2) + content fetch (1) + name IS NULL (1)
        //   + properties IS NOT NULL (1) + per-block properties (2).
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

/// Expected SQL for a mutation via its `Mutation` value.
fn expected_mutation_sql(
    mutation: &Mutation,
    watches: usize,
    blocks: usize,
    docs: usize,
) -> ExpectedSql {
    expected_sql_for_kind(MutationKind::from_mutation(mutation), watches, blocks, docs)
}

// ── Transition key ────────────────────────────────────────────────

/// Human-readable name for a transition variant (for log output).
pub fn transition_key(transition: &E2ETransition) -> String {
    use E2ETransition::*;
    match transition {
        E2ETransition::Nothing => "Nothing".into(),
        WriteOrgFile { .. } => "WriteOrgFile".into(),

        CreateDirectory { .. } => "CreateDirectory".into(),
        GitInit => "GitInit".into(),
        JjGitInit => "JjGitInit".into(),
        CreateStaleLoro { .. } => "CreateStaleLoro".into(),
        StartApp { .. } => "StartApp".into(),
        CreateDocument { .. } => "CreateDocument".into(),
        ApplyMutation(event) => match &event.mutation {
            Mutation::Create { .. } => "ApplyMutation::Create".into(),
            Mutation::Update { .. } => "ApplyMutation::Update".into(),
            Mutation::Delete { .. } => "ApplyMutation::Delete".into(),
            Mutation::Move { .. } => "ApplyMutation::Move".into(),
            Mutation::RestartApp => "ApplyMutation::RestartApp".into(),
        },
        SetupWatch { .. } => "SetupWatch".into(),
        RemoveWatch { .. } => "RemoveWatch".into(),
        SwitchView { .. } => "SwitchView".into(),
        NavigateFocus { .. } => "NavigateFocus".into(),
        NavigateBack { .. } => "NavigateBack".into(),
        NavigateForward { .. } => "NavigateForward".into(),
        NavigateHome { .. } => "NavigateHome".into(),
        ClickBlock { .. } => "ClickBlock".into(),
        ArrowNavigate { .. } => "ArrowNavigate".into(),
        SimulateRestart => "SimulateRestart".into(),
        BulkExternalAdd { .. } => "BulkExternalAdd".into(),
        ConcurrentSchemaInit => "ConcurrentSchemaInit".into(),
        ConcurrentMutations { .. } => "ConcurrentMutations".into(),
        EditViaDisplayTree { .. } => "EditViaDisplayTree".into(),
        EditViaViewModel { .. } => "EditViaViewModel".into(),
        TriggerSlashCommand { .. } => "TriggerSlashCommand".into(),
        TriggerDocLink { .. } => "TriggerDocLink".into(),
        ToggleState { .. } => "ToggleState".into(),
        Indent { .. } => "Indent".into(),
        Outdent { .. } => "Outdent".into(),
        MoveUp { .. } => "MoveUp".into(),
        MoveDown { .. } => "MoveDown".into(),
        DragDropBlock { .. } => "DragDropBlock".into(),
        SplitBlock { .. } => "SplitBlock".into(),
        JoinBlock { .. } => "JoinBlock".into(),
        UndoLastMutation => "UndoLastMutation".into(),
        Redo => "Redo".into(),
        EmitMcpData => "EmitMcpData".into(),
        AddPeer => "AddPeer".into(),
        PeerEdit { .. } => "PeerEdit".into(),
        SyncWithPeer { .. } => "SyncWithPeer".into(),
        MergeFromPeer { .. } => "MergeFromPeer".into(),
        PeerCharEdit { .. } => "PeerCharEdit".into(),
        FocusEditableText { .. } => "FocusEditableText".into(),
        MoveCursor { .. } => "MoveCursor".into(),
        TypeChars { .. } => "TypeChars".into(),
        DeleteBackward { .. } => "DeleteBackward".into(),
        PressKey { .. } => "PressKey".into(),
        Blur => "Blur".into(),
    }
}

// ── Render budget model ──────────────────────────────────────────
//
// Render counts are NON-DETERMINISTIC — they depend on GPUI's frame scheduling,
// signal coalescing, and CDC timing. Budgets are generous upper bounds.
//
// Start as Violation::Warning to collect calibration data. Promote to Error
// once the model is validated across ~50 PBT runs.

/// Expected render span counts for a transition.
#[derive(Debug)]
pub struct ExpectedRenders {
    /// Maximum total `frontend.render` spans expected
    pub max_total: usize,
    /// Maximum `frontend.render` spans with `component = "root"` expected
    pub max_root: usize,
}

/// Compute expected render budget for a transition.
///
/// Returns `None` for transitions where render budgets don't apply
/// (pre-startup, StartApp which is too variable).
pub fn expected_renders(
    transition: &E2ETransition,
    ref_state: &ReferenceState,
) -> Option<ExpectedRenders> {
    let blocks = ref_state.block_state.blocks.len();

    use E2ETransition::*;
    match transition {
        E2ETransition::Nothing => None,
        // Pre-startup: no UI
        WriteOrgFile { .. }
        | CreateDirectory { .. }
        | GitInit
        | JjGitInit
        | CreateStaleLoro { .. } => None,

        // StartApp: too variable (initial render + schema + matview cascade)
        StartApp { .. } | ConcurrentSchemaInit => None,

        // Navigation/view switches: focus change re-renders affected blocks
        NavigateFocus { .. }
        | NavigateBack { .. }
        | NavigateForward { .. }
        | NavigateHome { .. }
        | ClickBlock { .. }
        | ArrowNavigate { .. }
        | SwitchView { .. } => Some(ExpectedRenders {
            max_total: 20 + blocks * 3,
            max_root: 5,
        }),

        // Mutations: CDC cascade triggers re-renders proportional to block count
        ApplyMutation(_)
        | EditViaDisplayTree { .. }
        | EditViaViewModel { .. }
        | ToggleState { .. }
        | TriggerSlashCommand { .. }
        | Indent { .. }
        | Outdent { .. }
        | MoveUp { .. }
        | MoveDown { .. }
        | DragDropBlock { .. }
        | SplitBlock { .. }
        | JoinBlock { .. }
        | UndoLastMutation
        | Redo => Some(ExpectedRenders {
            max_total: 30 + blocks * 3,
            max_root: 10,
        }),

        // Bulk operations: more blocks, more renders
        BulkExternalAdd {
            blocks: new_blocks, ..
        } => {
            let n = new_blocks.len();
            Some(ExpectedRenders {
                max_total: 30 + (blocks + n) * 5,
                max_root: 10,
            })
        }

        CreateDocument { .. } => Some(ExpectedRenders {
            max_total: 30 + blocks * 5,
            max_root: 10,
        }),

        // Watch lifecycle
        SetupWatch { .. } | RemoveWatch { .. } => Some(ExpectedRenders {
            max_total: 15 + blocks * 2,
            max_root: 5,
        }),

        // SimulateRestart: re-sync triggers renders
        SimulateRestart => Some(ExpectedRenders {
            max_total: 30 + blocks * 3,
            max_root: 10,
        }),

        // Concurrent mutations: two mutations worth of renders
        ConcurrentMutations { .. } => Some(ExpectedRenders {
            max_total: 50 + blocks * 5,
            max_root: 15,
        }),

        // Read-only / no-render transitions
        TriggerDocLink { .. } | EmitMcpData => Some(ExpectedRenders {
            max_total: 10,
            max_root: 3,
        }),

        // Peer transitions: Loro-only in PBT, minimal UI impact
        AddPeer
        | PeerEdit { .. }
        | PeerCharEdit { .. }
        | SyncWithPeer { .. }
        | MergeFromPeer { .. } => Some(ExpectedRenders {
            max_total: 10 + blocks * 2,
            max_root: 5,
        }),

        // Atomic editor primitives: Focus rebuilds the editor,
        // MoveCursor/TypeChars/DeleteBackward only update InputState
        // (no widget re-render), Blur dispatches set_field. PressKey
        // is variable like a chord — covered by the most generous bound.
        FocusEditableText { .. } | Blur => Some(ExpectedRenders {
            max_total: 20 + blocks,
            max_root: 5,
        }),
        MoveCursor { .. } | DeleteBackward { .. } => Some(ExpectedRenders {
            max_total: 10,
            max_root: 3,
        }),
        TypeChars { text } => {
            let keystrokes = text.len().max(1);
            Some(ExpectedRenders {
                max_total: keystrokes * (3 + blocks / 4),
                max_root: keystrokes * 2,
            })
        }
        PressKey { .. } => Some(ExpectedRenders {
            max_total: 30 + blocks * 3,
            max_root: 10,
        }),
    }
}

// ── Checking ──────────────────────────────────────────────────────

pub enum Violation {
    Warning(String),
    Error(String),
}

/// Check observed metrics against computed expected SQL counts + timing + memory limits.
pub fn check_budget(
    transition: &E2ETransition,
    ref_state: &ReferenceState,
    metrics: &TransitionMetrics,
    wall_time: Duration,
    memory: Option<&MemoryMetrics>,
) -> Vec<Violation> {
    let key = transition_key(transition);
    let expected = expected_sql(transition, ref_state);
    let mut violations = Vec::new();

    // SQL reads: must be within expected + tolerance
    let reads_limit = expected.reads + expected.tolerance;
    if metrics.sql_read_count > reads_limit {
        violations.push(Violation::Error(format!(
            "{key}.sql_reads: {actual} exceeds expected {expected} + tolerance {tol} = {limit} \
             (watches={w}, docs={d})",
            actual = metrics.sql_read_count,
            expected = expected.reads,
            tol = expected.tolerance,
            limit = reads_limit,
            w = ref_state.active_watches.len(),
            d = ref_state.documents.len(),
        )));
    }

    // SQL writes
    let writes_limit = expected.writes + expected.tolerance;
    if metrics.sql_write_count > writes_limit {
        violations.push(Violation::Error(format!(
            "{key}.sql_writes: {actual} exceeds expected {expected} + tolerance {tol} = {limit}",
            actual = metrics.sql_write_count,
            expected = expected.writes,
            tol = expected.tolerance,
            limit = writes_limit,
        )));
    }

    // SQL DDL
    let ddl_limit = expected.ddl + expected.tolerance;
    if metrics.sql_ddl_count > ddl_limit {
        violations.push(Violation::Error(format!(
            "{key}.sql_ddl: {actual} exceeds expected {expected} + tolerance {tol} = {limit}",
            actual = metrics.sql_ddl_count,
            expected = expected.ddl,
            tol = expected.tolerance,
            limit = ddl_limit,
        )));
    }

    // Timing limits — generous, non-deterministic
    let max_single_query = Duration::from_secs(2);
    if metrics.max_query_duration > max_single_query {
        violations.push(Violation::Error(format!(
            "{key}.single_query: {}ms exceeds limit {}ms",
            metrics.max_query_duration.as_millis(),
            max_single_query.as_millis(),
        )));
    }

    let max_wall = Duration::from_secs(30);
    if wall_time > max_wall {
        violations.push(Violation::Error(format!(
            "{key}.wall_time: {}ms exceeds limit {}ms",
            wall_time.as_millis(),
            max_wall.as_millis(),
        )));
    }

    // ── Memory limits ────────────────────────────────────────────
    if let Some(mem) = memory {
        let delta = mem.rss_delta_bytes();
        let limit = (max_rss_delta_bytes(transition) as f64 * memory_multiplier()) as isize;

        if delta > limit {
            violations.push(Violation::Error(format!(
                "{key}.rss_delta: {delta_mb:+.1}MB exceeds limit {limit_mb:.0}MB \
                 (before={before_mb:.0}MB, after={after_mb:.0}MB)",
                delta_mb = mem.rss_delta_mb(),
                limit_mb = limit as f64 / (1024.0 * 1024.0),
                before_mb = mem.rss_before as f64 / (1024.0 * 1024.0),
                after_mb = mem.rss_after as f64 / (1024.0 * 1024.0),
            )));
        }

        let cumulative = mem.cumulative_growth_bytes();
        let cumulative_limit = (MAX_CUMULATIVE_RSS_GROWTH as f64 * memory_multiplier()) as isize;
        if cumulative > cumulative_limit {
            violations.push(Violation::Error(format!(
                "{key}.rss_cumulative: {cum_mb:+.1}MB total growth exceeds limit {limit_mb:.0}MB \
                 (baseline={base_mb:.0}MB, current={cur_mb:.0}MB)",
                cum_mb = mem.cumulative_growth_mb(),
                limit_mb = MAX_CUMULATIVE_RSS_GROWTH as f64 / (1024.0 * 1024.0),
                base_mb = mem.rss_baseline as f64 / (1024.0 * 1024.0),
                cur_mb = mem.rss_after as f64 / (1024.0 * 1024.0),
            )));
        }
    }

    // ── Render limits (Warning — non-deterministic, calibrating) ─
    if let Some(expected) = expected_renders(transition, ref_state) {
        if metrics.render_count > expected.max_total {
            violations.push(Violation::Warning(format!(
                "{key}.render_total: {actual} exceeds limit {limit} (blocks={b})",
                actual = metrics.render_count,
                limit = expected.max_total,
                b = ref_state.block_state.blocks.len(),
            )));
        }

        let root_count = metrics
            .render_by_component
            .iter()
            .find(|(c, _)| c == "root")
            .map(|(_, n)| *n)
            .unwrap_or(0);
        if root_count > expected.max_root {
            violations.push(Violation::Warning(format!(
                "{key}.render_root: {root_count} exceeds limit {limit}",
                limit = expected.max_root,
            )));
        }
    }

    violations
}

// ── Memory budget model ─────────────────────────────────────────
//
// RSS (Resident Set Size) is the OS-visible memory footprint. It's
// non-deterministic (page-granular, affected by OS reclaim) but it's
// what users see when memory bloats from 250MB to 4GB.
//
// Per-transition limits are generous hard caps. The cumulative limit
// catches slow leaks that stay under per-transition thresholds.
//
// When a limit is breached, sut.rs dumps system-level allocation stats
// to help identify the bloated subsystem.

const MB: usize = 1024 * 1024;

/// Linear multiplier for all memory budgets.
/// Set `PBT_MEMORY_MULTIPLIER=1.5` to relax limits by 50% (e.g. for
/// debug builds with full debug info, extra tracing subscribers, etc.).
/// Defaults to 1.0.
fn memory_multiplier() -> f64 {
    static MUL: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *MUL.get_or_init(|| {
        std::env::var("PBT_MEMORY_MULTIPLIER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0)
    })
}

/// Cumulative RSS growth limit across the entire PBT run.
/// If the process grows by more than this from the first transition,
/// something is leaking.
pub const MAX_CUMULATIVE_RSS_GROWTH: usize = 2000 * MB;

/// Per-transition RSS delta limit in bytes.
///
/// Calibrated from PBT runs (2026-04-06, sql_only variant, 2 cases):
///   StartApp (1st): +613MB (226K OTel spans + Turso schema + matviews)
///   StartApp (2nd): +59MB  (schema already cached, fewer spans)
///   BulkExternalAdd: +32MB (org sync + CDC cascades)
///   ApplyMutation:   +9MB  (single block mutation)
///   Navigation/View: <1MB
///
/// Limits are ~2x observed max to avoid flaky failures from OS page reclaim jitter.
pub fn max_rss_delta_bytes(transition: &E2ETransition) -> usize {
    use E2ETransition::*;
    match transition {
        // StartApp: schema DDL, file sync, matview creation, 200K+ OTel spans retained
        // in InMemorySpanExporter until next transition's reset().
        // First invocation in a process is ~600MB; subsequent ~60MB. Post-refactor
        // outliers reach ~1.2GB when CDC cascades multiply spans, so 1500MB headroom.
        StartApp { .. } => 1500 * MB,

        // ConcurrentSchemaInit re-initializes DDL — similar to StartApp.
        ConcurrentSchemaInit => 1500 * MB,

        // Pre-startup: filesystem only, negligible memory.
        WriteOrgFile { .. }
        | CreateDirectory { .. }
        | GitInit
        | JjGitInit
        | CreateStaleLoro { .. } => 5 * MB,

        // BulkExternalAdd: N blocks via org sync + CDC cascades. Observed: +32MB
        // (SQL-only), up to ~180MB with Loro outbound reconcile fork_at.
        BulkExternalAdd { .. } => 200 * MB,

        // CreateDocument: parse + upsert. Similar to BulkExternalAdd.
        CreateDocument { .. } => 200 * MB,

        // ConcurrentMutations: two mutations at once.
        ConcurrentMutations { .. } => 50 * MB,

        // SimulateRestart: clears and re-syncs.
        SimulateRestart => 80 * MB,

        // ApplyMutation: single block mutation. Observed: <10MB (SQL-only),
        // up to ~40MB with Loro outbound reconcile (fork_at doc copy).
        ApplyMutation(_) => 50 * MB,

        // SetupWatch: matview creation. Observed: +4MB.
        SetupWatch { .. } => 15 * MB,

        // Navigation, view switches, watch removal, undo/redo: <1MB observed.
        _ => 10 * MB,
    }
}

/// Memory metrics for a single transition.
#[derive(Debug, Clone)]
pub struct MemoryMetrics {
    /// RSS before the transition (bytes).
    pub rss_before: usize,
    /// RSS after the transition (bytes).
    pub rss_after: usize,
    /// RSS at the very start of the PBT run (first transition), for cumulative tracking.
    pub rss_baseline: usize,
}

impl MemoryMetrics {
    pub fn rss_delta_bytes(&self) -> isize {
        self.rss_after as isize - self.rss_before as isize
    }

    pub fn rss_delta_mb(&self) -> f64 {
        self.rss_delta_bytes() as f64 / (1024.0 * 1024.0)
    }

    pub fn cumulative_growth_bytes(&self) -> isize {
        self.rss_after as isize - self.rss_baseline as isize
    }

    pub fn cumulative_growth_mb(&self) -> f64 {
        self.cumulative_growth_bytes() as f64 / (1024.0 * 1024.0)
    }
}

/// Dump system-level memory diagnostics to stderr.
/// Called when an RSS budget is breached to help identify what's consuming memory.
#[cfg(target_os = "macos")]
pub fn diagnose_memory(key: &str) {
    eprintln!("[MEMORY DIAG] {key}: dumping macOS memory stats...");

    if let Ok(output) = std::process::Command::new("footprint")
        .arg("-j")
        .arg(std::process::id().to_string())
        .output()
    {
        if output.status.success() {
            let json = String::from_utf8_lossy(&output.stdout);
            // Extract top-level categories from footprint JSON
            eprintln!(
                "[MEMORY DIAG] {key}: footprint output ({} bytes):",
                json.len()
            );
            // Print first 2000 chars — enough to see the major categories
            for line in json.lines().take(60) {
                eprintln!("  {line}");
            }
        } else {
            eprintln!(
                "[MEMORY DIAG] {key}: footprint failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    // Also dump RSS breakdown via ps
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "pid,rss,vsz,command", "-p"])
        .arg(std::process::id().to_string())
        .output()
        && output.status.success()
    {
        eprintln!(
            "[MEMORY DIAG] {key}: ps:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn diagnose_memory(key: &str) {
    eprintln!("[MEMORY DIAG] {key}: dumping /proc/self/status memory info...");
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("Vm") || line.starts_with("Rss") || line.starts_with("Hugetlb") {
                eprintln!("  {line}");
            }
        }
    }
    if let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps_rollup") {
        eprintln!("[MEMORY DIAG] {key}: smaps_rollup:");
        for line in smaps.lines() {
            eprintln!("  {line}");
        }
    }
}
