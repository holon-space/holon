//! E2E transition types for the PBT state machine.

use std::hash::{Hash, Hasher};

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::{QueryLanguage, Region};

use super::query::TestQuery;
use super::types::MutationEvent;
use crate::LoroCorruptionType;

/// Generate a deterministic, UUID-like stable ID from inputs.
/// Both the reference model and SUT use this to produce identical block IDs
/// for peer-created blocks.
pub fn deterministic_peer_block_id(
    peer_idx: usize,
    parent_stable_id: Option<&str>,
    content: &str,
    seq: usize,
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    peer_idx.hash(&mut hasher);
    parent_stable_id.hash(&mut hasher);
    content.hash(&mut hasher);
    seq.hash(&mut hasher);
    let h = hasher.finish();
    // Format as 8-4-4-4-12 UUID-like string from the 64-bit hash + a fixed suffix
    let hi = (h >> 32) as u32;
    let lo = h as u32;
    format!(
        "peer-{hi:08x}-{lo:08x}-{peer_idx:04x}-{seq:04x}",
        hi = hi,
        lo = lo,
        peer_idx = peer_idx,
        seq = seq,
    )
}

#[derive(Debug, Clone)]
pub enum E2ETransition {
    Nothing,
    // === Pre-startup transitions ===
    /// Write an org file to temp directory (before app starts)
    WriteOrgFile {
        filename: String,
        content: String,
    },

    /// Create a directory (possibly nested) before app starts
    CreateDirectory {
        path: String,
    },

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
    CreateDocument {
        file_name: String,
    },

    /// Apply a mutation from any source (UI, external file, Loro sync)
    ApplyMutation(MutationEvent),

    /// Set up a CDC watch for a query (language-neutral)
    SetupWatch {
        query_id: String,
        query: TestQuery,
        language: QueryLanguage,
    },

    /// Remove a watch
    RemoveWatch {
        query_id: String,
    },

    /// Switch the active view filter
    SwitchView {
        view_name: String,
    },

    /// Navigate to focus on a specific block in a region
    NavigateFocus {
        region: Region,
        block_id: EntityUri,
    },

    /// Navigate back in history for a region
    NavigateBack {
        region: Region,
    },

    /// Navigate forward in history for a region
    NavigateForward {
        region: Region,
    },

    /// Navigate to home (root view) for a region
    NavigateHome {
        region: Region,
    },

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

    /// Edit content via the EditorController's on_blur path.
    /// Simulates what the GPUI blur handler does through the shared ViewModel layer:
    ///   1. Render block → shadow interpret → ViewModel
    ///   2. Build EditorController from EditableText node
    ///   3. Verify normal text doesn't fire triggers (on_text_changed → None)
    ///   4. Call on_blur(new_content) → EditorAction::Execute with set_field params
    ///   5. Dispatch the operation
    EditViaViewModel {
        block_id: EntityUri,
        new_content: String,
    },

    /// Simulate the full slash command trigger → EditorController → CommandMenu
    /// → operation execution pipeline. Tests the three-tier input model:
    ///   1. on_text_changed("/", 1) activates popup (PopupActivated)
    ///   2. CommandProvider finds satisfiable operations
    ///   3. on_key(Down) navigates, on_key(Enter) selects "delete"
    ///   4. EditorAction::Execute dispatches the operation
    ///
    /// This validates that EditorController correctly routes triggers to the
    /// popup menu and that operations dispatched through the menu path execute.
    TriggerSlashCommand {
        block_id: EntityUri,
    },

    /// Simulate the `[[` doc link trigger → EditorController → InsertText pipeline.
    /// Tests trigger detection (on_text_changed with `[[` mid-line) and the
    /// PopupProvider's on_select producing correct `[[id][label]]` syntax.
    ///
    /// This is a read-only transition — no block state changes.
    /// The async search is bypassed by manually populating items.
    TriggerDocLink {
        block_id: EntityUri,
        target_block_id: EntityUri,
    },

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

    /// Indent block via Tab keybinding — make child of previous sibling.
    /// Exercises: keybinding registry → shadow index → bubble_input → indent operation.
    Indent {
        block_id: EntityUri,
    },

    /// Outdent block via Shift+Tab keybinding — move to grandparent level.
    Outdent {
        block_id: EntityUri,
    },

    /// Move block up via Alt+Up keybinding — swap with previous sibling.
    MoveUp {
        block_id: EntityUri,
    },

    /// Move block down via Alt+Down keybinding — swap with next sibling.
    MoveDown {
        block_id: EntityUri,
    },

    /// Drag `source` and drop it onto `target` so that `source` becomes a
    /// child of `target`. Headless: walks the shadow tree to assert that a
    /// Draggable covers source and a DropZone covers target, then dispatches
    /// the `move_block` intent the drop_zone closure would build.
    /// GPUI: pushes real `MouseDown` → `MouseMove(pressed=Left)` … →
    /// `MouseUp` events through the interaction channel so `cx.active_drag`
    /// engages and `on_drop` fires.
    DragDropBlock {
        source: EntityUri,
        target: EntityUri,
    },

    /// Split block at cursor position via Enter keybinding.
    /// Creates a new sibling block after the original with content after the cursor.
    /// The original block keeps content before the cursor.
    SplitBlock {
        block_id: EntityUri,
        position: usize,
    },

    /// Join block into its previous sibling via Backspace at position 0.
    /// Symmetric inverse of `SplitBlock`: appends `block_id`'s content to the
    /// end of the previous sibling, re-parents `block_id`'s children under
    /// the previous sibling, then deletes `block_id`. The cursor lands at
    /// the join boundary (= old previous-sibling content length).
    ///
    /// Precondition: `block_id` must have a previous sibling at the same
    /// level — otherwise there is no block to merge into. (Backspace at
    /// position 0 in a first-child is a no-op and is not generated.)
    JoinBlock {
        block_id: EntityUri,
    },

    /// Click on a block to focus it. The only way to get initial editor focus.
    /// GPUI: enigo click at element center. Headless: navigate_focus teleport.
    /// Hard-asserts that the correct element receives focus.
    ClickBlock {
        region: Region,
        block_id: EntityUri,
    },

    // === Atomic editor primitives (GPUI-only — gated by PBT_ATOMIC_EDITOR=1) ===
    //
    // Decompose the bundled `SplitBlock`/`JoinBlock`/`EditViaViewModel`
    // transitions into orthogonal pieces so the generator can compose
    // sequences like `Focus → Type → DeleteBackward → PressKey(Enter)`.
    // The reference model maintains an `ActiveEditor` mirror of GPUI's
    // `InputState`, which exposes the in-memory-vs-DB divergence that
    // surfaces split-with-pending-edit and similar contract violations.
    /// Click an EditableText to take editor focus. Reference seeds
    /// `ActiveEditor.in_memory_content` from the block's saved content
    /// and lands the cursor at the end of line.
    FocusEditableText {
        block_id: EntityUri,
    },

    /// Move the caret to a byte offset within `ActiveEditor.in_memory_content`.
    /// SUT: `Home` + N×`Right` keystrokes through `PlatformInput`.
    MoveCursor {
        byte_position: usize,
    },

    /// Type ASCII characters at the current cursor offset. Modifies
    /// `InputState.text()` only — does NOT commit to DB.
    TypeChars {
        text: String,
    },

    /// Press Backspace `count` times. Modifies `InputState.text()` only.
    /// (Cursor at zero with the editor focused is the join_block path —
    /// dispatch via `PressKey` for that, not here.)
    DeleteBackward {
        count: usize,
    },

    /// Dispatch a chord while an editor is focused. Reference model encodes
    /// the *intended* contract: any structural chord (Enter/Backspace at 0/
    /// Tab/Shift+Tab/Alt+Up/Alt+Down) must commit `in_memory_content` to
    /// the block first, then mutate structure. Production today bypasses
    /// the commit — `assert_blocks_equivalent` catches the divergence.
    PressKey {
        chord: holon_api::KeyChord,
    },

    /// Click outside the editor or otherwise drop focus, committing any
    /// pending in-memory edit via `set_field`. Clears `ActiveEditor`.
    Blur,

    /// Arrow-key navigation from the currently focused block.
    /// Only valid when a block is focused (must ClickBlock first).
    /// GPUI: enigo arrow keys. Headless: shadow index bubble_input walk.
    /// Hard-asserts that actual focus matches reference model prediction.
    ArrowNavigate {
        region: Region,
        direction: holon_frontend::navigation::NavDirection,
        steps: u8,
    },

    /// Undo the last UI mutation via BackendEngine::undo().
    /// Only UI mutations (not external file edits) are undoable.
    UndoLastMutation,

    /// Redo the last undone mutation via BackendEngine::redo().
    Redo,

    /// Emit a fake MCP data update to trigger Turso IVM re-evaluation of all
    /// materialized views. Used to detect CDC re-emission bugs like cursor-jump-back
    /// where stale matview data re-fires signals.
    EmitMcpData,

    // === Multi-instance sync transitions ===
    /// Add a Loro-only peer instance that shares the primary's current state.
    AddPeer,

    /// Edit a block on a peer's LoroDoc directly (no SQL, no BackendEngine).
    PeerEdit {
        peer_idx: usize,
        op: PeerEditOp,
    },

    /// Bidirectional sync between primary's LoroDoc and a peer via DirectSync.
    SyncWithPeer {
        peer_idx: usize,
    },

    /// One-directional merge: peer's changes → primary.
    MergeFromPeer {
        peer_idx: usize,
    },

    // === MutableText transitions (Phase 3) ===
    /// Edit a block's LoroText container on a peer at the character level.
    PeerCharEdit {
        peer_idx: usize,
        block_id: String,
        op: TextOp,
    },
}

/// Character-level text operations on a peer's LoroText container.
#[derive(Debug, Clone)]
pub enum TextOp {
    Insert {
        pos_codepoint: usize,
        text: String,
    },
    Delete {
        pos_codepoint: usize,
        len_codepoint: usize,
    },
}

/// Operations that can be performed on a peer's Loro tree.
#[derive(Debug, Clone)]
pub enum PeerEditOp {
    Create {
        parent_stable_id: Option<String>,
        content: String,
        /// Deterministic stable ID derived from a hash of (peer_idx, parent, content, seq).
        /// Both the ref model and SUT use this same ID.
        stable_id: String,
    },
    Update {
        stable_id: String,
        content: String,
    },
    Delete {
        stable_id: String,
    },
}

impl E2ETransition {
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::Nothing => "Nothing",
            Self::WriteOrgFile { .. } => "WriteOrgFile",
            Self::CreateDirectory { .. } => "CreateDirectory",
            Self::GitInit => "GitInit",
            Self::JjGitInit => "JjGitInit",
            Self::CreateStaleLoro { .. } => "CreateStaleLoro",
            Self::StartApp { .. } => "StartApp",
            Self::CreateDocument { .. } => "CreateDocument",
            Self::ApplyMutation(_) => "ApplyMutation",
            Self::SetupWatch { .. } => "SetupWatch",
            Self::RemoveWatch { .. } => "RemoveWatch",
            Self::SwitchView { .. } => "SwitchView",
            Self::NavigateFocus { .. } => "NavigateFocus",
            Self::NavigateBack { .. } => "NavigateBack",
            Self::NavigateForward { .. } => "NavigateForward",
            Self::NavigateHome { .. } => "NavigateHome",
            Self::SimulateRestart => "SimulateRestart",
            Self::BulkExternalAdd { .. } => "BulkExternalAdd",
            Self::ConcurrentSchemaInit => "ConcurrentSchemaInit",
            Self::ConcurrentMutations { .. } => "ConcurrentMutations",
            Self::EditViaDisplayTree { .. } => "EditViaDisplayTree",
            Self::EditViaViewModel { .. } => "EditViaViewModel",
            Self::TriggerSlashCommand { .. } => "TriggerSlashCommand",
            Self::TriggerDocLink { .. } => "TriggerDocLink",
            Self::ToggleState { .. } => "ToggleState",
            Self::Indent { .. } => "Indent",
            Self::Outdent { .. } => "Outdent",
            Self::MoveUp { .. } => "MoveUp",
            Self::MoveDown { .. } => "MoveDown",
            Self::DragDropBlock { .. } => "DragDropBlock",
            Self::SplitBlock { .. } => "SplitBlock",
            Self::JoinBlock { .. } => "JoinBlock",
            Self::ClickBlock { .. } => "ClickBlock",
            Self::FocusEditableText { .. } => "FocusEditableText",
            Self::MoveCursor { .. } => "MoveCursor",
            Self::TypeChars { .. } => "TypeChars",
            Self::DeleteBackward { .. } => "DeleteBackward",
            Self::PressKey { .. } => "PressKey",
            Self::Blur => "Blur",
            Self::ArrowNavigate { .. } => "ArrowNavigate",
            Self::UndoLastMutation => "UndoLastMutation",
            Self::Redo => "Redo",
            Self::EmitMcpData => "EmitMcpData",
            Self::AddPeer => "AddPeer",
            Self::PeerEdit { .. } => "PeerEdit",
            Self::SyncWithPeer { .. } => "SyncWithPeer",
            Self::MergeFromPeer { .. } => "MergeFromPeer",
            Self::PeerCharEdit { .. } => "PeerCharEdit",
        }
    }
}
