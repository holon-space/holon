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
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use holon_api::Value;

use crate::cell::TextOp;
use crate::editor_caret;
use crate::editor_view_model::EditorKey;
use crate::editor_view_model::structural_block_action;
use crate::operations::OperationIntent;
use crate::reactive::BuilderServices;
use crate::reactive::ReactiveEngine;

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

    /// Read-only view of the tracked cursor for `block_id`. `None` when no
    /// keystroke has touched the block yet (the lazy seed-or-end init in
    /// `handle_keystroke` hasn't run). Never initializes — observation must
    /// not mutate the mirror.
    pub fn tracked_cursor(&self, block_id: &str) -> Option<usize> {
        self.cursors.lock().unwrap().get(block_id).copied()
    }

    fn set_cursor(&self, block_id: &str, byte: usize) {
        self.cursors
            .lock()
            .unwrap()
            .insert(block_id.to_string(), byte);
    }

    /// Mirror a user click on a block: seed the tracked caret for
    /// `block_uri` to end-of-text. Chord dispatch clicks the entity before
    /// pressing the chord, which re-opens its editor at the click position
    /// — modeled as end-of-text (`model_chord_click_focus` in the PBT ref).
    /// Deliberately ignores any armed caret seed: that seed belongs to the
    /// op-followup mount (split → 0, join → boundary), which the op's own
    /// focus sync already adopted; a later click overrides it just like a
    /// real mouse click re-places a GPUI caret. A cursor tracked during an
    /// earlier editor session on this block is stale and is overwritten.
    pub async fn seed_for_click(
        &self,
        engine: &Arc<ReactiveEngine>,
        block_uri: &holon_api::EntityUri,
    ) -> Result<()> {
        let block_id = block_uri.to_string();
        let services: &dyn BuilderServices = engine.as_ref();
        let current_text = match services.editable_text(block_uri, "content") {
            Ok(mt) => mt.current(),
            Err(_) => self.sql_block_content(engine, &block_id).await?,
        };
        self.set_cursor(&block_id, current_text.len());
        Ok(())
    }

    /// Read the block's `content` via the `QueryEngine` capability's
    /// non-settling `block_raw` read. Returns `""` when the block isn't
    /// present yet (e.g. the just-clicked block's create event hasn't
    /// projected) — caller's subsequent keystrokes then no-op until the
    /// row materialises, matching production GPUI's "editor mounts after
    /// CDC settles" behaviour. Query errors propagate (fail loud).
    async fn sql_block_content(
        &self,
        engine: &Arc<ReactiveEngine>,
        block_id: &str,
    ) -> Result<String> {
        let uri = holon_api::EntityUri::parse(block_id)
            .with_context(|| format!("headless mirror got a non-URI block id {block_id:?}"))?;
        let query_engine = engine.session().query_engine().with_context(|| {
            format!("headless mirror content read for {block_id} needs the Turso query engine")
        })?;
        Ok(query_engine
            .block_content_by_id(&uri)
            .await?
            .unwrap_or_default())
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
                "headless send_raw_keystroke({keystroke:?}) — no focused block; did the test \
                 FocusEditableText / click_entity before typing?"
            )
        })?;
        let block_id = block_uri.to_string();

        let services: &dyn BuilderServices = engine.as_ref();
        // `editable_text` returns Err in SqlOnly mode (no Loro provider
        // configured) and in Full mode if the block isn't in the Loro
        // tree yet. Both states are legitimate during PBT runs — the
        // mirror still tracks the cursor so structural ops can dispatch.
        // Trace the error so a real Loro misconfiguration is visible.
        let mt = match services.editable_text(&block_uri, "content") {
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
                 {block_id}. Its Loro `content_raw` text container isn't resolvable yet (the \
                 block's create intent hasn't landed in the Loro tree). The PBT runner's \
                 `pre_inv16_settle` barrier should have waited for it; increase that timeout or \
                 check why the block create is stuck."
            );
        }
        // SqlOnly variant has no `MutableText` — read the block's
        // SQL-projected `content` directly so the headless cursor walks the
        // same byte string a production GPUI editor would after
        // `set_value(content)`. Without this, `current_text` was `""`,
        // `cursor_or_init` pinned the cursor at 0, every "right" keystroke
        // no-op'd (`"".chars().next() == None`), and Enter fired
        // `split_block(.., position=0)` — the SqlOnly SplitBlock content-
        // routing divergence first surfaced by `split_block_content_pbt`
        // (commit aa636444).
        let current_text = if let Some(ref m) = mt {
            m.current()
        } else {
            // We can use Loro CRDT even when Loro as storage / P2P transport is disabled,
            // so MutableText should be available
            self.sql_block_content(engine, &block_id).await?
        };
        // First keystroke since focus: adopt the armed caret seed (split → 0,
        // join → boundary) exactly like a mounting GPUI editor does via
        // `peek_caret_seed`; without a seed, default to end-of-text
        // (`set_value` behaviour). A tracked cursor always wins — the seed is
        // only the mount-time initial position.
        let cursor_byte = match self.tracked_cursor(&block_id) {
            Some(c) => c,
            None => {
                let init = engine
                    .peek_caret_seed(&block_uri)
                    .filter(|&o| current_text.is_char_boundary(o.min(current_text.len())))
                    .map(|o| o.min(current_text.len()))
                    .unwrap_or(current_text.len());
                self.set_cursor(&block_id, init);
                init
            }
        };

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
                self.set_cursor(
                    &block_id,
                    editor_caret::move_right(&current_text, cursor_byte),
                );
            }
            "left" if !has_ctrl_alt_cmd => {
                self.set_cursor(
                    &block_id,
                    editor_caret::move_left(&current_text, cursor_byte),
                );
            }
            "backspace" if cursor_byte == 0 && !has_ctrl_alt_cmd && !has_shift => {
                let intent = structural_block_action(EditorKey::Backspace, &block_id, 0)
                    .expect("Backspace at caret 0 is the structural join_block");
                engine.dispatch_intent_sync(intent).await?;
                self.forget(&block_id);
            }
            "backspace" if cursor_byte > 0 && !has_ctrl_alt_cmd && !has_shift => {
                // `cursor_byte > 0` guarantees a preceding char, so `move_left`
                // always retreats by exactly one codepoint here.
                let new_cursor_byte = editor_caret::move_left(&current_text, cursor_byte);
                if let Some(cell) = mt.as_ref() {
                    let pos_codepoint =
                        editor_caret::byte_to_codepoint(&current_text, new_cursor_byte);
                    let len_codepoint =
                        editor_caret::codepoint_len(&current_text, new_cursor_byte, cursor_byte);
                    cell.apply_text_op(TextOp::Delete {
                        pos_codepoint,
                        len_codepoint,
                    })?;
                }
                self.set_cursor(&block_id, new_cursor_byte);
            }
            "enter" if !has_ctrl_alt_cmd && !has_shift => {
                // LogSeq parity: if the cursor sits after a `/cmd` that matches a
                // slash command, Enter executes it instead of splitting — the
                // same routing GPUI does via `EditorViewModel`/popup
                // (`editor_view.rs:578-616`). Otherwise split at the cursor.
                if let Some(intent) =
                    self.slash_command_on_enter(engine, &block_uri, &current_text, cursor_byte)
                {
                    engine.dispatch_intent_sync(intent).await?;
                } else {
                    let intent = structural_block_action(EditorKey::Enter, &block_id, cursor_byte)
                        .expect("Enter is the structural split_block");
                    engine.dispatch_intent_sync(intent).await?;
                }
                self.forget(&block_id);
            }
            "tab" if !has_shift && !has_ctrl_alt_cmd => {
                let intent = structural_block_action(EditorKey::Tab, &block_id, cursor_byte)
                    .expect("Tab is the structural indent");
                engine.dispatch_intent_sync(intent).await?;
            }
            "tab" if has_shift && !has_ctrl_alt_cmd => {
                let intent = structural_block_action(EditorKey::BackTab, &block_id, cursor_byte)
                    .expect("Shift+Tab is the structural outdent");
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
                    let pos_codepoint = editor_caret::byte_to_codepoint(&current_text, cursor_byte);
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

    /// Replicate GPUI's Enter→slash-command routing for the headless path: if
    /// the text before the cursor ends in a `/cmd` matching an available block
    /// operation, return that command's intent to dispatch. Returns `None` when
    /// no command matches (the caller then falls through to `split_block`).
    ///
    /// Uses the same pure logic GPUI's `EditorViewModel`/`CommandProvider`
    /// drive (`check_triggers` + `build_command_items` + `on_select`), but
    /// sources the block's operations from its entity profile rather than a
    /// rendered node — leaf-block rendering streams in async and is unreliable
    /// to snapshot mid-keystroke in headless.
    fn slash_command_on_enter(
        &self,
        engine: &Arc<ReactiveEngine>,
        block_uri: &holon_api::EntityUri,
        current_text: &str,
        cursor_byte: usize,
    ) -> Option<OperationIntent> {
        use holon_api::render_types::OperationWiring;

        use crate::command_provider::CommandProvider;
        use crate::input_trigger::ViewEvent;
        use crate::input_trigger::check_triggers;
        use crate::input_trigger::default_triggers_for_operations;
        use crate::popup_menu::PopupProvider;
        use crate::popup_menu::PopupResult;

        // The block's available operations are entity-level (keyed by id
        // scheme), identical to what the renderer attaches to the block's
        // editable node — so source them from the profile cache rather than a
        // rendered node (leaf-block render data streams in async and is
        // unreliable to read mid-keystroke in headless).
        let services: &dyn BuilderServices = engine.as_ref();
        let descriptors = services.entity_operations(block_uri.scheme());
        if descriptors.is_empty() {
            return None;
        }
        let wirings: Vec<OperationWiring> = descriptors
            .into_iter()
            .map(|descriptor| OperationWiring {
                modified_param: String::new(),
                descriptor,
            })
            .collect();

        // Does the text before the cursor end in a `/cmd`? `check_triggers`
        // slices `current_line[..cursor_column]` as bytes, so pass the byte
        // offset.
        let triggers = default_triggers_for_operations(&wirings);
        let event = check_triggers(&triggers, current_text, cursor_byte)?;
        let ViewEvent::TriggerFired {
            action,
            filter_text,
            ..
        } = event
        else {
            return None;
        };
        if action != "command_menu" {
            return None;
        }

        let mut context = HashMap::new();
        context.insert(
            "id".to_string(),
            Value::String(block_uri.as_str().to_string()),
        );
        let items = CommandProvider::build_command_items(&wirings, &context, &filter_text);
        let first = items.first()?;
        let provider = CommandProvider::new(wirings, context);
        match provider.on_select(first, &filter_text) {
            PopupResult::Execute {
                entity_name,
                op_name,
                params,
                ..
            } => Some(OperationIntent::new(entity_name, op_name, params)),
            _ => None,
        }
    }
}
