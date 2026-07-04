//! Action-engine actor fragment of the PBT reference model (ADR 0004 / 0006).
//!
//! `ActionActorState` holds the state owned by the action-engine actor:
//! lifecycle flag, the doc-id allocator, the last-applied transition tag, and
//! the undo/redo snapshot stacks. Per ADR 0004 this state must vanish when the
//! action engine isn't wired — isolating it here lets a reduced wiring drop
//! the fragment instead of carrying dead state.
//!
//! @pbt kind ref
//! @pbt covers action-actor-state — lifecycle flag, `next_doc_id` allocator,
//!   last-transition tag (Markov weighting), and the undo/redo `BlockState`
//!   snapshot stacks. FIDELITY: `next_doc_id`/`next_id` are monotonic across
//!   undo/redo by design (prod never reuses a burned id, see
//! `pop_undo_to_redo`).

use holon_api::entity_uri::EntityUri;

use super::block_state::BlockState;

/// Action-engine actor state extracted from `ReferenceState` (ADR 0004 Phase
/// 4).
#[derive(Debug, Clone)]
pub struct ActionActorState {
    /// Whether the application has been started.
    pub app_started: bool,

    /// ID counter for generating unique document IDs.
    pub next_doc_id: usize,

    /// Variant tag of the most recently applied transition. Drives Markov-
    /// style adaptive weighting in `transitions()`.
    pub last_transition_kind: Option<&'static str>,

    /// Undo stack: snapshots of `BlockState` before each UI mutation.
    pub undo_stack: Vec<BlockState>,

    /// Redo stack: snapshots of `BlockState` before each undo.
    pub redo_stack: Vec<BlockState>,
}

impl ActionActorState {
    pub fn new() -> Self {
        Self {
            app_started: false,
            next_doc_id: 0,
            last_transition_kind: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Generate a synthetic `block:ref-doc-N` URI for a new document and bump
    /// the counter.
    pub fn next_synthetic_doc_uri(&mut self) -> EntityUri {
        let uri = EntityUri::block(&format!("ref-doc-{}", self.next_doc_id));
        self.next_doc_id += 1;
        uri
    }
}

impl Default for ActionActorState {
    fn default() -> Self {
        Self::new()
    }
}
