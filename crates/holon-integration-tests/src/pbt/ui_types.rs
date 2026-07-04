//! UI-fragment types of the PBT reference model: cursor, active editor, and
//! navigation history. Extracted from `reference_state.rs`.

use holon_api::entity_uri::EntityUri;
use holon_frontend::editor_caret;

/// Cursor position within a focused block. Tracks line and column to predict
/// whether arrow keys cause cross-block navigation or intra-block movement.
#[derive(Debug, Clone, Copy)]
pub struct CursorPosition {
    pub line: usize,
    pub column: usize,
}

impl CursorPosition {
    pub fn start() -> Self {
        Self { line: 0, column: 0 }
    }
}

/// Mirror of the GPUI editor's live `InputState`: the in-memory text of the
/// currently focused EditableText, plus the cursor offset within that text.
/// Diverges from `block.content` whenever the user has typed/deleted without
/// blurring — exactly the divergence that surfaces split-with-pending-edit
/// (and similar) bugs.
#[derive(Debug, Clone)]
pub struct ActiveEditor {
    pub block_id: EntityUri,
    /// What the GPUI `InputState.text()` currently shows.
    pub in_memory_content: String,
    /// Byte offset of the caret within `in_memory_content`.
    pub cursor_byte: usize,
    /// True once modeled typing/deleting touched `in_memory_content` since
    /// the editor opened (or since the last commit). Mirrors what prod's
    /// commit paths observe: a DIRTY editor's text is user-authored and
    /// commits on blur / at a structural commit point; a clean editor whose
    /// text merely diverged from `block.content` is STALE against an
    /// external change (prod's data subscription refreshes idle editors) —
    /// committing it would write old text into the ref.
    pub dirty: bool,
}

impl ActiveEditor {
    /// Insert text at the cursor and advance. Delegates caret/text math to the
    /// **shared** `editor_caret` primitive — the SAME one the SUT's
    /// `InMemEditorComponent` drives — so ref and SUT cannot diverge on the
    /// text primitive itself (multibyte-safe).
    pub fn type_chars(&mut self, text: &str) {
        debug_assert!(self.cursor_byte <= self.in_memory_content.len());
        self.cursor_byte =
            editor_caret::insert_at(&mut self.in_memory_content, self.cursor_byte, text);
        self.dirty = true;
    }

    /// Delete `count` chars before the cursor (Backspace ×count). Stops at
    /// start.
    pub fn delete_backward(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.cursor_byte =
            editor_caret::delete_back(&mut self.in_memory_content, self.cursor_byte, count);
        self.dirty = true;
    }

    /// Move the caret to a clamped byte position. Snaps to the nearest char
    /// boundary at or before the target (a raw byte target — home/end/a click —
    /// may land mid-codepoint); `clamp_boundary` keeps the caret legal.
    pub fn move_cursor(&mut self, position: usize) {
        self.cursor_byte = editor_caret::clamp_boundary(&self.in_memory_content, position);
    }
}

/// Navigation history for a region (for back/forward navigation)
#[derive(Debug, Clone)]
pub struct NavigationHistory {
    /// History entries: None = home view, Some(id) = focused on block
    pub entries: Vec<Option<EntityUri>>,
    /// Current cursor position in history
    pub cursor: usize,
}

/// One open `navigation_history` row (`closed_at IS NULL`). Mirrors the
/// open-rows projection that drives the `focus_roots` matview.
///
/// `block_id = None` represents a home row (block_id NULL in SQL); home
/// rows are kept here because they bump `next_history_id` and contribute
/// to move-to-top dedup, but they are excluded from `expected_focus_root_ids`
/// (they're filtered out by the consumer GQL JOIN on `root.id = fr.root_id`).
#[derive(Debug, Clone)]
pub struct OpenPinEntry {
    pub history_id: i64,
    pub block_id: Option<EntityUri>,
    pub added_ts_logical: u64,
}

impl Default for NavigationHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigationHistory {
    pub fn new() -> Self {
        Self {
            entries: vec![None],
            cursor: 0,
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor < self.entries.len().saturating_sub(1)
    }

    pub fn current_focus(&self) -> Option<EntityUri> {
        self.entries.get(self.cursor).cloned().flatten()
    }
}
