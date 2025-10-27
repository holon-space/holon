//! E2E transition types for the PBT state machine.

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
    NavigateFocus { region: Region, block_id: String },

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
        doc_uri: String,
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
}
