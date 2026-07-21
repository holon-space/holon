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

use crate::editor_caret;
use crate::editor_view_model::EditorKey;
use crate::editor_view_model::EditorViewModel;
use crate::editor_view_model::structural_block_action;
use crate::operations::OperationIntent;
use crate::reactive::BuilderServices;
use crate::reactive::ReactiveEngine;

/// Per-block byte cursor + helpers for routing one keystroke at a time.
///
/// SPIKE (Phase 1b): the cursor map is keyed by `(block_id, occurrence)`, not
/// bare `block_id`. `occurrence = None` is the block's canonical occurrence
/// (every existing caller resolves to it, unchanged); `Some(n)` is a
/// display-placed occurrence. This is the crux of the display-placement
/// de-risk: two occurrences of one block get INDEPENDENT carets here, while the
/// text write still resolves through `editable_text(canonical_uri)` — caret per
/// occurrence, edit to canonical home.
type CursorKey = (String, Option<u32>);

/// Construct a cell-free editor [`EditorViewModel`] for `block_id` seeded with
/// `seed` content. No Loro cell is attached, so `apply_local_edit` takes the
/// SqlOnly `set_field("content")`+`write_seq` branch — the production GPUI
/// SqlOnly editor the headless keystone models (Inc 4).
fn new_editor_vm(block_id: &str, seed: &str) -> EditorViewModel {
    let mut ctx = HashMap::new();
    ctx.insert("id".to_string(), Value::String(block_id.to_string()));
    EditorViewModel::new(
        Vec::new(),
        Vec::new(),
        ctx,
        "content".to_string(),
        seed.to_string(),
    )
}

pub struct HeadlessEditorMirror {
    cursors: Mutex<HashMap<CursorKey, usize>>,
    /// Per-block authoritative editor buffers (Inc 4). Each focused/typed block
    /// gets a cell-free [`EditorViewModel`] that OWNS the visible buffer,
    /// `last_local_seq`, and the echo/convergence decision — the headless
    /// analogue of production GPUI's SqlOnly editor (`InputState` buffer + the
    /// `set_field`/`write_seq` write path + the data-sync echo guard). Keyed by
    /// the canonical block id string (occurrence-independent: the write always
    /// targets the canonical home). Deliberately NO Loro cell is attached so
    /// `apply_local_edit` takes the `set_field`+`write_seq` branch and the
    /// echo composition (`evaluate_data_sync_echo`) is the live headless typing
    /// path — the composition the keystone was structurally blind to before.
    editors: Mutex<HashMap<String, EditorViewModel>>,
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
            editors: Mutex::new(HashMap::new()),
        }
    }

    /// Reset the tracked cursor for one occurrence of a block (Escape / focus
    /// loss / after a structural op). Idempotent.
    pub fn forget(&self, block_id: &str, occ: Option<u32>) {
        self.cursors
            .lock()
            .unwrap()
            .remove(&(block_id.to_string(), occ));
        // The editor buffer is occurrence-independent (canonical write home), so
        // any focus-loss / structural-op forget on this block retires its VM.
        self.editors.lock().unwrap().remove(block_id);
    }

    /// Read-only view of the tracked cursor for a block's CANONICAL occurrence.
    /// `None` when no keystroke has touched it yet. Never initializes.
    /// (Back-compat: existing callers want the canonical caret.)
    pub fn tracked_cursor(&self, block_id: &str) -> Option<usize> {
        self.tracked_cursor_at(block_id, None)
    }

    /// SPIKE: read the tracked cursor for a specific occurrence.
    pub fn tracked_cursor_at(&self, block_id: &str, occ: Option<u32>) -> Option<usize> {
        self.cursors
            .lock()
            .unwrap()
            .get(&(block_id.to_string(), occ))
            .copied()
    }

    fn set_cursor(&self, block_id: &str, occ: Option<u32>, byte: usize) {
        self.cursors
            .lock()
            .unwrap()
            .insert((block_id.to_string(), occ), byte);
    }

    /// The live authoritative editor buffer for `block_id` (Inc 4): the VM's
    /// owned buffer — the pre-commit value keystrokes mutate, which after a
    /// same-seq trailing-whitespace echo can legitimately diverge from the
    /// SQL-stored (trimmed) `block.content`. `None` when no editor VM is open
    /// for the block. This is the exact source `SutEditorMirrorRead::
    /// editor_live_text` reads for the headless frontend.
    pub fn live_text(&self, block_id: &str) -> Option<String> {
        self.editors
            .lock()
            .unwrap()
            .get(block_id)
            .map(|vm| vm.buffer().to_string())
    }

    /// Open (idempotently) a cell-free editor VM for `block_id` seeded with
    /// `seed` (the block's current authority content). Called on focus so a
    /// focused-but-not-yet-typed editor already mirrors the block content.
    pub fn ensure_editor(&self, block_id: &str, seed: &str) {
        self.editors
            .lock()
            .unwrap()
            .entry(block_id.to_string())
            .or_insert_with(|| new_editor_vm(block_id, seed));
    }

    /// (Re)seed a block's editor VM from the CURRENT backend authority — the
    /// fresh-mount seed prod applies when an editor OPENS on a block
    /// (`open_active_editor`: a clean editor whose buffer == the block's stored
    /// content). OVERWRITES any prior (possibly stale post-`AdoptBaseline`) VM
    /// so a re-focus reads authority, matching the reference's fresh
    /// `open_active_editor`. Reads the SAME source the production editor seeds
    /// from — the Loro cell when present, else the SQL-projected content — so a
    /// focused-but-not-yet-typed block already exercises the VM read+converge
    /// path (Inc 4). Idempotent-`ensure_editor` would NOT do here: a lingering
    /// VM that adopted a trailing-ws baseline would keep the trailing space and
    /// diverge from the ref's freshly-opened (trimmed) buffer.
    pub async fn reset_editor_from_authority(
        &self,
        engine: &Arc<ReactiveEngine>,
        block_uri: &holon_api::EntityUri,
    ) -> Result<()> {
        let block_id = block_uri.to_string();
        let services: &dyn BuilderServices = engine.as_ref();
        let content = match services.editable_text(block_uri, "content") {
            Ok(mt) => mt.current(),
            Err(_) => self.sql_block_content(engine, &block_id).await?,
        };
        self.editors
            .lock()
            .unwrap()
            .insert(block_id.clone(), new_editor_vm(&block_id, &content));
        Ok(())
    }

    /// Apply one user-typed buffer edit through the VM (the single keystroke
    /// sink): mutate the owned buffer to `new_text`, stamp `write_seq`, and
    /// dispatch the resulting `set_field("content")` intent through the real op
    /// pipeline (`dispatch_intent_sync`). The lock is released BEFORE the await
    /// so no VM guard is held across the dispatch.
    async fn vm_commit_edit(
        &self,
        engine: &Arc<ReactiveEngine>,
        block_id: &str,
        seed: &str,
        new_text: &str,
    ) -> Result<()> {
        self.ensure_editor(block_id, seed);
        let intent = {
            let mut eds = self.editors.lock().unwrap();
            let vm = eds
                .get_mut(block_id)
                .expect("ensure_editor just guaranteed a VM for this block");
            vm.apply_local_edit(new_text)?
        };
        if let Some(intent) = intent {
            engine.dispatch_intent_sync(intent).await?;
        }
        Ok(())
    }

    /// Converge one editor's buffer against the settled SQL authority (Inc 4 —
    /// the headless data-sync loop). Reads the block's stored (trimmed)
    /// `content` and runs the VM's `converge_from_data_sync` against its own
    /// `last_local_seq`; a `Converge` directive re-seeds the buffer, while the
    /// own trailing-whitespace echo (`AdoptBaseline`) and in-sync/stale cases
    /// leave the typed buffer intact. `echo_seq` is the VM's high-water: no
    /// non-editor writer bumps the `write_seq` column, so the CDC row an echo
    /// carries always holds exactly this value.
    pub async fn converge_editor(
        &self,
        engine: &Arc<ReactiveEngine>,
        block_id: &str,
    ) -> Result<()> {
        let content = self.sql_block_content(engine, block_id).await?;
        let mut eds = self.editors.lock().unwrap();
        if let Some(vm) = eds.get_mut(block_id) {
            let seq = vm.last_local_seq();
            if let Some(directive) = vm.converge_from_data_sync(&content, Some(seq)) {
                vm.set_buffer_from_authority(&directive.target, directive.seq);
            }
        }
        Ok(())
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
        let occ = engine.focused_occurrence();
        let services: &dyn BuilderServices = engine.as_ref();
        let current_text = match services.editable_text(block_uri, "content") {
            Ok(mt) => mt.current(),
            Err(_) => self.sql_block_content(engine, &block_id).await?,
        };
        self.set_cursor(&block_id, occ, current_text.len());
        // A click re-mounts the editor: (re)seed its buffer VM from the current
        // authority content, discarding any stale buffer from an earlier editor
        // session on this block (mirrors GPUI re-seeding `InputState` on
        // focus-gain). Keyed by canonical id (occurrence-independent write home).
        self.editors
            .lock()
            .unwrap()
            .insert(block_id.clone(), new_editor_vm(&block_id, &current_text));
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
        // SPIKE (Phase 1b): the focused OCCURRENCE keys the caret; the block id
        // still keys the write (`editable_text(block_uri)` below is unchanged).
        let occ = engine.focused_occurrence();

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
        // Char keystrokes and mid-line backspace route through the block's
        // cell-free editor VM (Inc 4): the VM owns the buffer and dispatches a
        // `set_field("content")`+`write_seq` write through the real op pipeline.
        // No `MutableText` is required — the SqlOnly editor path the composed
        // keystone models has no Loro cell. A block whose create intent has not
        // landed yet surfaces as a loud `dispatch_intent_sync` error, not a
        // silent no-op.
        // SqlOnly variant has no `MutableText` — read the block's
        // SQL-projected `content` directly so the headless cursor walks the
        // same byte string a production GPUI editor would after
        // `set_value(content)`. Without this, `current_text` was `""`,
        // `cursor_or_init` pinned the cursor at 0, every "right" keystroke
        // no-op'd (`"".chars().next() == None`), and Enter fired
        // `split_block(.., position=0)` — the SqlOnly SplitBlock content-
        // routing divergence first surfaced by `split_block_content_pbt`
        // (commit aa636444).
        let current_text = if let Some(buffered) = self.live_text(&block_id) {
            // An editor VM already owns this block's buffer (a prior keystroke
            // this focus session, or a focus-time `ensure_editor` seed) — it is
            // the authority, not the possibly-mid-settle SQL/cell snapshot.
            buffered
        } else if let Some(ref m) = mt {
            m.current()
        } else {
            self.sql_block_content(engine, &block_id).await?
        };
        // First keystroke since focus: adopt the armed caret seed (split → 0,
        // join → boundary) exactly like a mounting GPUI editor does via
        // `peek_caret_seed`; without a seed, default to end-of-text
        // (`set_value` behaviour). A tracked cursor always wins — the seed is
        // only the mount-time initial position.
        let cursor_byte = match self.tracked_cursor_at(&block_id, occ) {
            Some(c) => c,
            None => {
                let init = engine
                    .peek_caret_seed(&block_uri)
                    .filter(|&o| current_text.is_char_boundary(o.min(current_text.len())))
                    .map(|o| o.min(current_text.len()))
                    .unwrap_or(current_text.len());
                self.set_cursor(&block_id, occ, init);
                init
            }
        };

        let has_shift = modifiers.iter().any(|m| *m == "shift");
        let has_ctrl_alt_cmd = modifiers
            .iter()
            .any(|m| matches!(*m, "ctrl" | "alt" | "cmd"));

        match keystroke {
            "home" if !has_ctrl_alt_cmd => {
                self.set_cursor(&block_id, occ, 0);
            }
            "end" if !has_ctrl_alt_cmd => {
                self.set_cursor(&block_id, occ, current_text.len());
            }
            "right" if !has_ctrl_alt_cmd => {
                self.set_cursor(
                    &block_id,
                    occ,
                    editor_caret::move_right(&current_text, cursor_byte),
                );
            }
            "left" if !has_ctrl_alt_cmd => {
                self.set_cursor(
                    &block_id,
                    occ,
                    editor_caret::move_left(&current_text, cursor_byte),
                );
            }
            "backspace" if cursor_byte == 0 && !has_ctrl_alt_cmd && !has_shift => {
                let intent = structural_block_action(EditorKey::Backspace, &block_id, 0)
                    .expect("Backspace at caret 0 is the structural join_block");
                engine.dispatch_intent_sync(intent).await?;
                self.forget(&block_id, occ);
            }
            "backspace" if cursor_byte > 0 && !has_ctrl_alt_cmd && !has_shift => {
                // `cursor_byte > 0` guarantees a preceding char, so `move_left`
                // always retreats by exactly one codepoint here.
                let new_cursor_byte = editor_caret::move_left(&current_text, cursor_byte);
                let mut new_text = current_text.clone();
                new_text.replace_range(new_cursor_byte..cursor_byte, "");
                self.vm_commit_edit(engine, &block_id, &current_text, &new_text)
                    .await?;
                self.set_cursor(&block_id, occ, new_cursor_byte);
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
                self.forget(&block_id, occ);
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
                self.forget(&block_id, occ);
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
                let mut new_text = current_text.clone();
                new_text.insert_str(cursor_byte, &inserted);
                self.vm_commit_edit(engine, &block_id, &current_text, &new_text)
                    .await?;
                self.set_cursor(&block_id, occ, cursor_byte + inserted.len());
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

#[cfg(test)]
mod spike_phase_1b_tests {
    //! SPIKE (Phase 1b — display-placement de-risk): proves the mirror keys the
    //! caret by `(block_id, occurrence)`, so two occurrences of ONE block hold
    //! INDEPENDENT carets. Combined with `handle_keystroke` resolving
    //! `editable_text(&block_uri)` by the canonical id (unchanged by this
    //! spike), this is the "caret per occurrence, write to canonical" property
    //! the display-placement contract needs. Pure cursor-map test — no engine.
    use super::*;

    #[test]
    fn occurrence_keyed_cursors_are_independent() {
        let m = HeadlessEditorMirror::new();
        let block = "block:abc";

        // Canonical occurrence (None) and a display-placed occurrence Some(1).
        m.set_cursor(block, None, 3);
        m.set_cursor(block, Some(1), 7);

        // Independent, and `tracked_cursor` (back-compat) resolves canonical.
        assert_eq!(m.tracked_cursor(block), Some(3));
        assert_eq!(m.tracked_cursor_at(block, None), Some(3));
        assert_eq!(m.tracked_cursor_at(block, Some(1)), Some(7));

        // Moving the display occurrence's caret does NOT touch the canonical.
        m.set_cursor(block, Some(1), 9);
        assert_eq!(m.tracked_cursor_at(block, Some(1)), Some(9));
        assert_eq!(m.tracked_cursor_at(block, None), Some(3));

        // Forgetting one occurrence leaves the other intact.
        m.forget(block, Some(1));
        assert_eq!(m.tracked_cursor_at(block, Some(1)), None);
        assert_eq!(m.tracked_cursor_at(block, None), Some(3));
    }
}
