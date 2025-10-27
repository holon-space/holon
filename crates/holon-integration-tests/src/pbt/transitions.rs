//! E2E transition types for the PBT state machine.

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::{QueryLanguage, Region};

use super::query::TestQuery;
use super::types::MutationEvent;
use crate::LoroCorruptionType;

#[derive(Debug, Clone)]
pub enum E2ETransition {
    // === Pre-startup transitions ===
    /// Write an org file to temp directory (before app starts)
    WriteOrgFile { filename: String, content: String },

    /// Create a directory (possibly nested) before app starts
    CreateDirectory { path: String },

    /// Initialize git repository (runs `git init`)
    GitInit,

    /// Initialize jj repository (runs `jj git init`)
    JjGitInit,

    /// Create a stale/corrupted .loro file BEFORE the system starts.
    CreateStaleLoro {
        /// The org filename this .loro file corresponds to (e.g., "test.org")
        org_filename: String,
        /// Type of corruption to simulate
        corruption_type: LoroCorruptionType,
    },

    /// Start the application (triggers sync, may race with DDL)
    StartApp {
        wait_for_ready: bool,
        /// Enable Todoist fake mode (adds concurrent DDL during startup)
        enable_todoist: bool,
        /// Enable Loro CRDT layer (false = SQL-only, matching Flutter default)
        enable_loro: bool,
    },

    // === Post-startup transitions ===
    /// Create a new document (Org file)
    CreateDocument { file_name: String },

    /// Apply a mutation from any source (UI, external file, Loro sync)
    ApplyMutation(MutationEvent),

    /// Set up a CDC watch for a query (language-neutral)
    SetupWatch {
        query_id: String,
        query: TestQuery,
        language: QueryLanguage,
    },

    /// Remove a watch
    RemoveWatch { query_id: String },

    /// Switch the active view filter
    SwitchView { view_name: String },

    /// Navigate to focus on a specific block in a region
    NavigateFocus { region: Region, block_id: EntityUri },

    /// Navigate back in history for a region
    NavigateBack { region: Region },

    /// Navigate forward in history for a region
    NavigateForward { region: Region },

    /// Navigate to home (root view) for a region
    NavigateHome { region: Region },

    /// Simulate app restart: clears OrgSyncController's last_projection.
    SimulateRestart,

    /// Bulk external add: adds multiple blocks at once via external file modification.
    BulkExternalAdd {
        /// Target document URI
        doc_uri: EntityUri,
        /// Blocks to add (fully specified for deterministic state)
        blocks: Vec<Block>,
    },

    /// Concurrent schema init: triggers schema re-initialization while other operations are running.
    ConcurrentSchemaInit,

    /// Concurrent mutations: fires a UI mutation and an External mutation without waiting
    /// for sync between them.
    ConcurrentMutations {
        ui_mutation: MutationEvent,
        external_mutation: MutationEvent,
    },

    /// Edit content via the display tree's EditableText operations.
    /// Simulates what the GPUI blur handler does: extracts entity_name and
    /// set_field operation from the node's operations vec, then executes it.
    /// Fails if the EditableText node lacks entity or operations (the bug).
    EditViaDisplayTree {
        block_id: EntityUri,
        new_content: String,
    },

    /// Edit content via the ViewEventHandler's TextSync path.
    /// Simulates what the GPUI blur handler does through the shared ViewModel layer:
    ///   1. Render block → shadow interpret → ViewModel
    ///   2. Verify triggers are present on EditableText
    ///   3. Verify normal text doesn't fire triggers
    ///   4. Feed `ViewEvent::TextSync { value }` to ViewEventHandler
    ///   5. ViewEventHandler returns `MenuAction::Execute` with set_field params
    ///   6. Dispatch the operation
    EditViaViewModel {
        block_id: EntityUri,
        new_content: String,
    },

    /// Simulate the full slash command trigger → ViewEventHandler → CommandMenu
    /// → operation execution pipeline. Tests the three-tier input model:
    ///   1. check_triggers() matches "/" at line start
    ///   2. ViewEventHandler routes to CommandMenuController
    ///   3. CommandMenuController finds satisfiable operations
    ///   4. Selects "delete" (fully satisfied with just `id`) and executes
    ///
    /// This validates that triggers are present on EditableText nodes,
    /// that the shared ViewEventHandler logic works, and that operations
    /// dispatched through the menu path execute correctly.
    TriggerSlashCommand { block_id: EntityUri },

    /// Set a block's task state via the StateToggle widget path.
    /// Simulates what real frontends do:
    ///   1. Render block → shadow interpret → ViewModel
    ///   2. Find StateToggle node, verify current state matches
    ///   3. Dispatch set_field(task_state, new_state) — matching real frontend behavior
    ///
    /// `new_state` is any valid state from the toggle's states list (not just "next").
    ToggleState {
        block_id: EntityUri,
        new_state: String,
    },

    /// Undo the last UI mutation via BackendEngine::undo().
    /// Only UI mutations (not external file edits) are undoable.
    UndoLastMutation,

    /// Redo the last undone mutation via BackendEngine::redo().
    Redo,
}
