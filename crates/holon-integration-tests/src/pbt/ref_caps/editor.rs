//! `RefEditorMirror` / `RefEditorMirrorMut`.
//!
//! @pbt kind ref
//! @pbt covers editor-mirror — headless mirror of the live GPUI/TUI
//!   `InputState` (active editor block + dirty/clean buffer text). FIDELITY:
//!   the clean-buffer refresh-from-content-cell (dirty buffers keep pending
//!   user text) is modeled in `apply_content_mutation`, not here.

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;

use super::super::reference_state::ReferenceState;
use super::cap_id;

impl RefEditorMirror for ReferenceState {
    fn active_editor_block(&self) -> Option<EntityUri> {
        self.ui
            .tab
            .active_editor
            .as_ref()
            .map(|e| cap_id(&e.block_id))
    }

    fn active_editor_text(&self) -> Option<&str> {
        self.ui
            .tab
            .active_editor
            .as_ref()
            .map(|e| e.in_memory_content.as_str())
    }

    fn active_editor_cursor(&self) -> Option<usize> {
        self.ui.tab.active_editor.as_ref().map(|e| e.cursor_byte)
    }

    fn active_editor_dirty(&self) -> bool {
        self.ui.tab.active_editor.as_ref().is_some_and(|e| e.dirty)
    }
}

impl RefEditorMirrorMut for ReferenceState {
    fn type_chars(&mut self, text: &str) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.type_chars(text);
        }
    }

    fn delete_backward(&mut self, count: usize) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.delete_backward(count);
        }
    }

    fn move_cursor(&mut self, byte_position: usize) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.move_cursor(byte_position);
        }
    }

    fn reseed_active_editor(&mut self, text: &str, cursor: usize) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.in_memory_content = text.to_string();
            editor.move_cursor(cursor);
        }
    }

    fn mark_active_editor_committed(&mut self) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.dirty = false;
        }
    }
}
