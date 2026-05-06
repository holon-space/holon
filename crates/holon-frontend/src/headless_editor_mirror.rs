//! Headless `InputState` mirror used by `ReactiveEngineDriver` to simulate
//! per-keystroke typing through `MutableText`/Loro the same way
//! `gpui-component`'s `InputState` does in production GPUI.
//!
//! `general_e2e_pbt.rs` runs against a headless driver, so without this
//! module its `send_raw_keystroke` would bail and the keystroke-driven
//! atomic editor primitives (`TypeChars`, `PressKey`, `DeleteBackward`,
//! `MoveCursor`) couldn't fire. With this module the headless path
//! exercises the exact same MutableText writes per character, dispatches
//! `split_block` / `join_block` at the live cursor for Enter /
//! Backspace-at-0, and surfaces the sync-race bug class that production
//! GPUI hits on `editor_view.rs:548-572`.
//!
//! Cursor state is tracked as a byte offset per block, lazily initialized
//! from `MutableText::current()` on first keystroke after focus changes.
//! When no `MutableText` is configured (SqlOnly variant), char keystrokes
//! still update the cursor mirror so structural chord ops still operate at
//! a sensible position; the text itself simply doesn't land anywhere.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::cell::TextOp;
use anyhow::{Context, Result};
use holon_api::{EntityName, Value};

use crate::operations::OperationIntent;
use crate::reactive::{BuilderServices, ReactiveEngine};

/// Per-block byte cursor + helpers for routing one keystroke at a time.
pub struct HeadlessEditorMirror {
    cursors: Mutex<HashMap<String, usize>>,
}

impl Default for HeadlessEditorMirror {
    fn default() -> Self {
        Self::new()
    }
}

impl HeadlessEditorMirror {
    pub fn new() -> Self {
        Self {
            cursors: Mutex::new(HashMap::new()),
        }
    }

    /// Reset the tracked cursor for a block (e.g. on Escape or focus loss).
    /// Idempotent.
    pub fn forget(&self, block_id: &str) {
        self.cursors.lock().unwrap().remove(block_id);
    }

    /// Look up the cursor for `block_id`, lazily initializing to the byte
    /// length of `current_text` (matching gpui-component InputState's
    /// behaviour after `set_value(text)` — caret lands at the end).
    fn cursor_or_init(&self, block_id: &str, current_text: &str) -> usize {
        let mut guard = self.cursors.lock().unwrap();
        *guard
            .entry(block_id.to_string())
            .or_insert_with(|| current_text.len())
    }

    fn set_cursor(&self, block_id: &str, byte: usize) {
        self.cursors
            .lock()
            .unwrap()
            .insert(block_id.to_string(), byte);
    }

    /// Route a single keystroke through the same logical pipeline GPUI's
    /// `editor_view.rs` runs in capture phase: char keys mutate
    /// `MutableText` directly, Enter / Backspace-at-0 / Tab / Shift+Tab
    /// dispatch their structural intents at the live cursor.
    ///
    /// Errors propagate from `dispatch_intent_sync`. Char keystrokes with
    /// no `MutableText` attached (SqlOnly variant) are no-ops on the text
    /// side but still advance the cursor mirror so subsequent structural
    /// ops have a position to dispatch against.
    pub async fn handle_keystroke(
        &self,
        engine: &Arc<ReactiveEngine>,
        keystroke: &str,
        modifiers: &[&str],
    ) -> Result<()> {
        let block_uri = engine.focused_block().with_context(|| {
            format!(
                "headless send_raw_keystroke({keystroke:?}) — no focused block; \
                     did the test FocusEditableText / click_entity before typing?"
            )
        })?;
        let block_id = block_uri.to_string();

        let services: &dyn BuilderServices = engine.as_ref();
        // `editable_text` returns Err in SqlOnly mode (no Loro provider
        // configured) and in Full mode if the block isn't in the Loro
        // tree yet. Both states are legitimate during PBT runs — the
        // mirror still tracks the cursor so structural ops can dispatch.
        // Trace the error so a real Loro misconfiguration is visible.
        let mt = match services.editable_text(&block_id, "content") {
            Ok(mt) => Some(mt),
            Err(e) => {
                tracing::trace!("[HeadlessEditorMirror] no MutableText for {block_id}: {e:#}");
                None
            }
        };
        // Char keystrokes and mid-line backspace need a `MutableText` to land
        // anywhere. Without one, silently no-op'ing the keystroke makes the
        // ref→prod divergence look like a CDC race when it's really the loro
        // consumer not having applied the block's create event yet. Fail
        // loud so the runner barrier (`pre_inv16_settle`) surfaces the real
        // gap. SqlOnly variants don't trigger char-typing transitions
        // (`TypeChars`/`PressKey` gate on `enable_loro`), so this only fires
        // on real races in Full.
        let needs_mt = !modifiers
            .iter()
            .any(|m| matches!(*m, "ctrl" | "alt" | "cmd"))
            && ((keystroke == "backspace"
                && self
                    .cursors
                    .lock()
                    .unwrap()
                    .get(&block_id)
                    .copied()
                    .unwrap_or(0)
                    > 0)
                || (keystroke.chars().count() == 1
                    && !matches!(keystroke, "home" | "end" | "left" | "right" | "tab")));
        if needs_mt && mt.is_none() {
            anyhow::bail!(
                "headless send_raw_keystroke({keystroke:?}) — no MutableText for focused block \
                 {block_id}. The loro consumer hasn't applied the block's create event yet. \
                 The PBT runner's `pre_inv16_settle` barrier should have waited for it; \
                 increase the consumer timeout or check why the loro consumer is stuck."
            );
        }
        let current_text = mt.as_ref().map(|m| m.current()).unwrap_or_default();
        let cursor_byte = self.cursor_or_init(&block_id, &current_text);

        let has_shift = modifiers.iter().any(|m| *m == "shift");
        let has_ctrl_alt_cmd = modifiers
            .iter()
            .any(|m| matches!(*m, "ctrl" | "alt" | "cmd"));

        match keystroke {
            "home" if !has_ctrl_alt_cmd => {
                self.set_cursor(&block_id, 0);
            }
            "end" if !has_ctrl_alt_cmd => {
                self.set_cursor(&block_id, current_text.len());
            }
            "right" if !has_ctrl_alt_cmd => {
                let advance = current_text[cursor_byte..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(0);
                self.set_cursor(&block_id, cursor_byte + advance);
            }
            "left" if !has_ctrl_alt_cmd => {
                let retreat = current_text[..cursor_byte]
                    .chars()
                    .next_back()
                    .map(char::len_utf8)
                    .unwrap_or(0);
                self.set_cursor(&block_id, cursor_byte - retreat);
            }
            "backspace" if cursor_byte == 0 && !has_ctrl_alt_cmd && !has_shift => {
                let intent = make_block_intent("join_block", &block_id, Some(0));
                engine.dispatch_intent_sync(intent).await?;
                self.forget(&block_id);
            }
            "backspace" if cursor_byte > 0 && !has_ctrl_alt_cmd && !has_shift => {
                let prev_char_byte_len = current_text[..cursor_byte]
                    .chars()
                    .next_back()
                    .map(char::len_utf8)
                    .with_context(|| {
                        format!(
                            "backspace at cursor_byte={cursor_byte} in text of \
                             len={} but no preceding char",
                            current_text.len()
                        )
                    })?;
                let new_cursor_byte = cursor_byte - prev_char_byte_len;
                if let Some(cell) = mt.as_ref() {
                    let pos_codepoint = current_text[..new_cursor_byte].chars().count();
                    let len_codepoint = current_text[new_cursor_byte..cursor_byte].chars().count();
                    cell.apply_text_op(TextOp::Delete {
                        pos_codepoint,
                        len_codepoint,
                    })?;
                }
                self.set_cursor(&block_id, new_cursor_byte);
            }
            "enter" if !has_ctrl_alt_cmd && !has_shift => {
                let intent = make_block_intent("split_block", &block_id, Some(cursor_byte as i64));
                engine.dispatch_intent_sync(intent).await?;
                self.forget(&block_id);
            }
            "tab" if !has_shift && !has_ctrl_alt_cmd => {
                let intent = make_block_intent("indent", &block_id, None);
                engine.dispatch_intent_sync(intent).await?;
            }
            "tab" if has_shift && !has_ctrl_alt_cmd => {
                let intent = make_block_intent("outdent", &block_id, None);
                engine.dispatch_intent_sync(intent).await?;
            }
            "escape" => {
                self.forget(&block_id);
            }
            single if !has_ctrl_alt_cmd && single.chars().count() == 1 => {
                let raw = single
                    .chars()
                    .next()
                    .expect("char count == 1 so first char exists");
                let ch = if has_shift {
                    raw.to_uppercase().next().unwrap_or(raw)
                } else {
                    raw
                };
                let inserted = ch.to_string();
                if let Some(cell) = mt.as_ref() {
                    let pos_codepoint = current_text[..cursor_byte].chars().count();
                    cell.apply_text_op(TextOp::Insert {
                        pos_codepoint,
                        text: inserted.clone(),
                    })?;
                }
                self.set_cursor(&block_id, cursor_byte + inserted.len());
            }
            _ => {
                tracing::trace!(
                    "[HeadlessEditorMirror] unhandled keystroke: {keystroke:?} mods={modifiers:?}"
                );
            }
        }
        Ok(())
    }
}

fn make_block_intent(op: &str, block_id: &str, position: Option<i64>) -> OperationIntent {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(block_id.to_string()));
    if let Some(p) = position {
        params.insert("position".to_string(), Value::Integer(p));
    }
    OperationIntent::new(EntityName::new("block"), op.to_string(), params)
}
