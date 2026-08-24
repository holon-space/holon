//! Framework-agnostic editor view model.
//!
//! Bundles the per-editable-field state every frontend needs:
//! - `ViewEventHandler` + input triggers (slash commands, wikilinks, blur sync)
//! - the [`Cell<String>`] handle (when attached) for collaborative text edits
//!
//! Frontends create one per editable field and feed it platform events. The
//! view model returns `EditorAction` values that the frontend executes using
//! its platform-specific APIs, and exposes pass-through methods for CRDT
//! reads/writes so frontends never reach for the cell backing directly.

use std::collections::HashMap;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;
use futures::stream::BoxStream;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_api::render_types::OperationWiring;
use holon_api::types::EntityName;
use holon_core::cell::Cell;
use holon_core::cell::CursorAnchor;
use holon_core::cell::CursorBias;
use holon_core::cell::TextDelta;
use holon_core::cell::TextOp;

use crate::echo::EchoDecision;
use crate::echo::evaluate_data_sync_echo;
use crate::input_trigger::InputTrigger;
use crate::input_trigger::ViewEvent;
use crate::input_trigger::{self};
use crate::operations::OperationIntent;
use crate::popup_menu::MenuKey;
use crate::popup_menu::PopupItem;
use crate::popup_menu::PopupResult;
use crate::popup_menu::PopupState;
use crate::reactive::BuilderServices;
use crate::view_event_handler::HandleResult;
use crate::view_event_handler::ViewEventHandler;

/// Actions the frontend should execute after calling EditorViewModel methods.
///
/// The controller decides *what* to do; the frontend decides *how* to do it
/// using platform-specific APIs.
pub enum EditorAction {
    /// Nothing to do.
    None,

    /// Re-render the popup overlay (items or selection changed).
    UpdatePopup,

    /// A popup was just activated. The frontend must watch this signal
    /// and call `notify_items_changed()` on each emission to keep the
    /// popup state in sync.
    PopupActivated {
        signal: Pin<Box<dyn futures_signals::signal::Signal<Item = Vec<PopupItem>> + Send>>,
    },

    /// The popup was dismissed. Frontend should hide the overlay.
    PopupDismissed,

    /// Dispatch an operation (slash command selected, text synced on blur,
    /// etc.).
    Execute(OperationIntent),

    /// Dispatch an operation AND strip the typed slash-command text first.
    /// `strip_prefix_start` is the line-relative column of the "/" trigger;
    /// the frontend must remove `line_start + strip_prefix_start .. cursor`
    /// from the editor text BEFORE dispatching, otherwise "/delete" remains
    /// in the block content and gets committed at the next commit point.
    ExecuteAndStripCommand {
        intent: OperationIntent,
        strip_prefix_start: usize,
    },

    /// Insert text at a position (wiki-link selected).
    /// `prefix_start` is the column where the trigger prefix started (e.g.,
    /// `[[`). Frontend should replace text from `line_start + prefix_start`
    /// to `cursor` with `replacement`.
    InsertText {
        replacement: String,
        prefix_start: usize,
    },

    /// Let the parent handle this key (popup is not active).
    /// E.g., MoveUp/MoveDown should propagate to cross-block navigation.
    Propagate,

    /// A popup selection was handled but FAILED. The frontend must surface
    /// `message` visibly (toast/banner), strip the typed command text if
    /// `strip_prefix_start` is `Some`, and consume the key WITHOUT falling
    /// through to a structural op (split_block). Fail-loud: a failed command
    /// must never read as a silent no-op or a stray split.
    CommandFailed {
        message: String,
        strip_prefix_start: Option<usize>,
    },

    /// A picker phase of a slash command opened. The frontend must remove
    /// `line_start + prefix_start .. line_start + prefix_start + len` from the
    /// visible editor and report what it removed back through
    /// [`EditorViewModel::command_text_hidden`], which holds it until the phase
    /// ends (ruling D1.b — the typed command must not sit in the block while
    /// its picker is open).
    ///
    /// `len` is the span the MENU matched on, not one derived from the caret:
    /// a caret moved back into the middle of `/emb` would otherwise leave the
    /// tail visible.
    HideCommandText { prefix_start: usize, len: usize },

    /// A picker phase whose command text was hidden was CANCELLED. The frontend
    /// must put `text` back verbatim, directly after `line_prefix`, and leave
    /// the caret after it — the user's typing must survive the cancel exactly
    /// as they left it.
    ///
    /// The anchor travels as the PREFIX BYTES rather than an offset so the
    /// frontend re-derives it from the live line. An offset captured at hide
    /// time silently stops addressing the same place the moment anything on
    /// the line changes.
    RestoreCommandText { line_prefix: String, text: String },
}

impl std::fmt::Debug for EditorAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::UpdatePopup => write!(f, "UpdatePopup"),
            Self::PopupActivated { .. } => write!(f, "PopupActivated {{ signal: ... }}"),
            Self::PopupDismissed => write!(f, "PopupDismissed"),
            Self::Execute(intent) => write!(f, "Execute({:?})", intent),
            Self::ExecuteAndStripCommand {
                intent,
                strip_prefix_start,
            } => write!(
                f,
                "ExecuteAndStripCommand({:?}, strip_prefix_start={})",
                intent, strip_prefix_start
            ),
            Self::InsertText {
                replacement,
                prefix_start,
            } => {
                write!(
                    f,
                    "InsertText {{ replacement: {:?}, prefix_start: {} }}",
                    replacement, prefix_start
                )
            }
            Self::Propagate => write!(f, "Propagate"),
            Self::CommandFailed {
                message,
                strip_prefix_start,
            } => write!(
                f,
                "CommandFailed {{ message: {:?}, strip_prefix_start: {:?} }}",
                message, strip_prefix_start
            ),
            Self::HideCommandText { prefix_start, len } => {
                write!(
                    f,
                    "HideCommandText {{ prefix_start: {prefix_start}, len: {len} }}"
                )
            }
            Self::RestoreCommandText { line_prefix, text } => write!(
                f,
                "RestoreCommandText {{ line_prefix: {line_prefix:?}, text: {text:?} }}"
            ),
        }
    }
}

/// Slash-command text held out of the visible editor while a picker phase of
/// that command is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenCommandText {
    /// The text that stood BEFORE the hidden command on its line at hide time.
    ///
    /// The restore anchor is this string's length, but it can only be used
    /// after proving the live line still starts with exactly these bytes —
    /// which is what [`Self::anchor_in`] does. Holding the bytes instead of the
    /// offset is what makes a stale anchor unrepresentable: an offset captured
    /// at hide time keeps looking valid after the line under it has changed,
    /// and slicing with it panics or reinserts the command in the wrong place.
    pub line_prefix: String,
    /// The removed text, including the trigger char.
    pub text: String,
}

impl HiddenCommandText {
    /// Byte offset within `current_line` where the hidden text belongs, or
    /// `None` when the line no longer starts with the recorded prefix — the
    /// user edited across the anchor and there is no such place any more.
    pub fn anchor_in(&self, current_line: &str) -> Option<usize> {
        current_line
            .starts_with(self.line_prefix.as_str())
            .then_some(self.line_prefix.len())
    }
}

/// Instruction the convergence decision hands back to the adapter: set the
/// visible editor buffer (`InputState`) to the backend authority.
///
/// `target` is the SqlOnly authority text, and it is also what the adapter's
/// `converge_input` applies when no Loro cell is attached; a
/// cell-attached editor re-reads the *live* cell authority at apply time, so a
/// composition committed during an IME deferral is merged, never reverted.
/// `seq` is the write-ordering high-water this convergence accepted — it lets a
/// directive deferred past an IME composition be discarded when a newer local
/// write (the committed composition) superseded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvergeDirective {
    pub target: String,
    pub seq: i64,
}

/// Framework-agnostic controller for an editable text field.
///
/// Each editable text node in the ViewModel gets one controller.
/// The frontend creates it during reconciliation and calls its methods
/// from platform event handlers.
///
/// # Buffer ownership (buffer-ownership inversion, Increment 1)
///
/// The view model owns the authoritative editable-text buffer (`buffer`) and
/// the write-ordering high-water mark (`last_local_seq`). RAW-text coordinates:
/// `buffer` holds raw Unicode scalars so the RAW `[[…]]` edit-mode feature can
/// seed/commit through this same seam without a second migration.
///
/// ONE-DIRECTIONAL SHADOW INVARIANT (Increment 1 only): while GPUI's
/// `InputState` is still the visible authority, this buffer is a shadow kept in
/// lockstep by the adapter — `InputState` writes flow INTO the buffer
/// (`apply_local_edit`), never yet the reverse. Increment 2 flips the buffer to
/// the write authority; Increment 3 makes it the convergence authority via
/// `set_buffer_from_authority`.
pub struct EditorViewModel {
    handler: ViewEventHandler,
    triggers: Vec<InputTrigger>,
    cell: Option<Cell<String>>,
    /// Authoritative editable-text buffer in RAW-text coordinates. See the
    /// struct-level "Buffer ownership" note for the shadow invariant.
    buffer: String,
    /// Highest [`holon_api::write_seq::WriteSeq`] this editor has authored (via
    /// a content keystroke) or accepted (from a converged external write). The
    /// data-sync convergence guard drops any echo whose `write_seq` is strictly
    /// less than this. Starts at `WriteSeq::ZERO`: before the user types, every
    /// echo converges (correct seeding).
    last_local_seq: i64,
    /// A convergence directive deferred because an IME composition was in
    /// progress at converge time. Stashed by `set_pending_directive`,
    /// superseded by any newer directive, and replayed by the adapter on the
    /// composition-end / focus edge via `take_pending_directive`.
    pending_directive: Option<ConvergeDirective>,
    /// Handle to the reaper's registry, when this editor was mounted by a
    /// frontend that births blocks from creation affordances. `None` for
    /// headless/unit editors, which have no reaper to race.
    ephemeral_newborns: Option<Arc<crate::creation_slot::EphemeralNewborns>>,
    /// The typed slash-command text the frontend removed from the visible
    /// editor for the duration of a picker phase (ruling D1.b), with the
    /// line-relative offset it was removed from. `Some` exactly while a phase
    /// is hiding it; the text goes back verbatim when the phase is cancelled.
    hidden_command: Option<HiddenCommandText>,
    /// The exact line a [`EditorAction::RestoreCommandText`] is about to
    /// produce. Re-inserting the command text is a programmatic edit that fires
    /// the same change event a keystroke does, and the re-inserted `/` would
    /// instantly re-open the menu the user just escaped out of.
    ///
    /// Matching on the CONTENT rather than arming a bare one-shot keeps this
    /// self-limiting: if the programmatic change event never arrives, the next
    /// real keystroke produces a different line and is handled normally instead
    /// of being swallowed.
    restoring_to: Option<String>,
    /// The owning document's `#+TODO:` vocabulary, read once per focus. It
    /// governs what the editable surface SHOWS (the source projection and its
    /// refusals) and, through [`Self::surface`], which channel its commits
    /// take.
    ///
    /// `None` until the read returns — deliberately NOT the parser's defaults.
    /// See [`crate::editor_source::Surface::Pending`]: the defaults declare
    /// `TODO`, so a fabricated vocabulary here silently loses task state.
    vocabulary: Option<holon_org_format::TaskKeywordVocabulary>,
    /// What this editor's buffer MEANS, decided at the seed under the
    /// vocabulary above. The commit router reads it rather than re-deriving:
    /// the cheap shape rule it would otherwise use is vocabulary-free, and a
    /// refused buffer whose content merely starts with an uppercase token would
    /// be admitted to the source channel and lose its task state there.
    surface: crate::editor_source::Surface,
    /// Bytes the surface prepends to the content column — the task keyword and
    /// its separating space, `0` whenever the two coincide. Recorded at the
    /// seed so an offset arriving in CONTENT coordinates (a structural op's
    /// `cursor_offset`) can be carried into the buffer that consumes it.
    surface_prefix: usize,
}

impl EditorViewModel {
    pub fn new(
        operations: Vec<OperationWiring>,
        triggers: Vec<InputTrigger>,
        context_params: HashMap<String, Value>,
        field: String,
        original_value: String,
    ) -> Self {
        let handler =
            ViewEventHandler::new(operations, context_params, field, original_value.clone());
        Self {
            handler,
            triggers,
            cell: None,
            buffer: original_value,
            last_local_seq: holon_api::write_seq::WriteSeq::ZERO.get(),
            pending_directive: None,
            ephemeral_newborns: None,
            hidden_command: None,
            restoring_to: None,
            vocabulary: None,
            surface: crate::editor_source::Surface::Pending,
            surface_prefix: 0,
        }
    }

    /// Wire the reaper's registry so the first non-empty keystroke can retire
    /// this editor's block from it. See [`Self::retire_from_the_reaper`].
    pub fn set_ephemeral_newborns(
        &mut self,
        newborns: Arc<crate::creation_slot::EphemeralNewborns>,
    ) {
        self.ephemeral_newborns = Some(newborns);
    }

    /// The block this editor edits stops being reapable the moment it carries
    /// content. Called from the single keystroke sink BEFORE the write is
    /// dispatched, which is what makes the reaper unable to race typing: a
    /// blur handler reading the registry after any processed keystroke already
    /// sees the block retired.
    fn retire_from_the_reaper(&self, new_text: &str) {
        if new_text.is_empty() {
            return;
        }
        let (Some(newborns), Some(id)) = (&self.ephemeral_newborns, self.handler.context_id())
        else {
            return;
        };
        if let Ok(uri) = holon_api::EntityUri::parse(id) {
            newborns.retire(&uri);
        }
    }

    /// Announce that a local edit reached this buffer, so an async gesture
    /// holding a pre-round-trip snapshot of this row (the undo/redo re-seed)
    /// refuses to overwrite it.
    ///
    /// Called from the keystroke sink UPSTREAM of the cell/no-cell fork: the
    /// buffer became the authority in either mode, and a signal taken from one
    /// mode's write path would be silent in the other.
    fn note_local_edit(&self) {
        let Some(id) = self.handler.context_id() else {
            return;
        };
        // ALLOW(entity_uri_from_raw): boundary — the context id is the render
        // spec's row id string; the gesture side keys on the same parse.
        crate::local_edit_epoch::note(&holon_api::EntityUri::from_raw(id));
    }

    /// Attach a [`Cell<String>`] handle for the editable field. Call once,
    /// after construction, when the production frontend resolves the cell
    /// from `BuilderServices::editable_text` (which routes through the
    /// `BlockCellRegistry`). Tests and headless paths leave it unattached
    /// — CRDT pass-throughs return `None` / `Err` in that case.
    pub fn attach_cell(&mut self, cell: Cell<String>) {
        // Seed the authoritative buffer from the cell's current text so it
        // matches the visible InputState (the CRDT text is the mount seed in
        // cell mode). Without this the first keystroke would diff against the
        // stale construction-time content.
        self.buffer = cell.current();
        self.cell = Some(cell);
        // A Loro `Cell` is now the per-keystroke content writer, so the
        // handler must drop the redundant on-blur `set_field("content")` to
        // avoid racing the Loro projection. Without a cell the flag stays
        // `false` and the on-blur write remains the sole content writer.
        self.handler.set_loro_content_writer(true);
    }

    /// Whether a [`Cell<String>`] is attached. Frontends should check
    /// before driving the remote-delta loop or computing local diffs.
    pub fn has_cell(&self) -> bool {
        self.cell.is_some()
    }

    /// Re-baseline the blur-commit change tracking to an authority re-seed.
    ///
    /// Pass-through to [`ViewEventHandler::set_baseline`]. Called by the
    /// frontend right after it absolutely re-seeds the visible buffer from the
    /// backend authority (`converge_input`) so an unmodified, re-seeded editor
    /// does not diff as dirty and fire a spurious identical-content
    /// `set_field` on the next blur. Not a local write — advances no write-seq.
    pub fn rebaseline(&mut self, text: &str) {
        self.handler.set_baseline(text.to_string());
    }

    /// Borrow the attached [`Cell<String>`]. Returns `None` if unattached.
    /// Most frontends should prefer the pass-through methods below; this
    /// is the escape hatch for spawning long-lived async consumers (e.g.
    /// `cx.spawn` on GPUI) that need to outlive a lock-guard scope.
    pub fn cell(&self) -> Option<&Cell<String>> {
        self.cell.as_ref()
    }

    /// Current CRDT text snapshot. `None` when unattached.
    pub fn current_text(&self) -> Option<String> {
        self.cell.as_ref().map(|c| c.current())
    }

    /// Borrow the authoritative editable-text buffer (RAW-text coordinates).
    /// See the struct-level "Buffer ownership" note.
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Highest write-ordering sequence this editor has authored or accepted.
    /// The convergence guard drops echoes strictly older than this.
    pub fn last_local_seq(&self) -> i64 {
        self.last_local_seq
    }

    /// Advance the write-ordering high-water mark to at least `seq` (never
    /// regressing it). The convergence loop calls this when it accepts an
    /// authority state (Converge) or confirms its own echo (InSync /
    /// AdoptBaseline).
    pub fn advance_local_seq(&mut self, seq: i64) {
        self.last_local_seq = self.last_local_seq.max(seq);
    }

    /// Converge the authoritative buffer to a backend/authority `text` carrying
    /// ordering token `seq` (the convergence sink). Advances the high-water
    /// mark (never regressing it) and re-baselines the blur-commit change
    /// tracking to the re-seeded text so an unmodified, re-seeded editor
    /// does not diff as dirty and fire a spurious identical-content
    /// `set_field` on the next blur. Folds in the former `rebaseline`
    /// contract: not a local write — it does not stamp a new write-seq,
    /// only adopts `seq` as the accepted high-water.
    ///
    /// RAW-seam hook: `text` MAY be a raw reconstruction
    /// (`render_inline_marks`) once raw-edit mode lands; today callers pass
    /// stored (stripped) content and the signature does not change when raw
    /// seeding arrives.
    pub fn set_buffer_from_authority(&mut self, text: &str, seq: i64) {
        self.buffer = text.to_string();
        self.last_local_seq = self.last_local_seq.max(seq);
        self.handler.set_baseline(text.to_string());
    }

    /// The vault syntax this block's stored state shows in the editable
    /// surface, or its stored content with the refusal disclosed. See
    /// [`crate::editor_source::project_or_disclose`]; the vocabulary is the one
    /// this editor was seeded with on focus.
    ///
    /// `marks` are the spans over `content`. The editable surface is a SOURCE
    /// projection (F2-arm=d, ratified 2026-08-11), so they are rendered BACK to
    /// their org delimiters here: entering a block whose content carries
    /// `~code~` must show the `~`, not markup-free text that is nonetheless
    /// styled (`2026-08-18-editor-seeded-from-stripped-content-not-source`).
    /// The keyword prefix goes on top of that source, never on the label.
    pub fn project_authority(
        &mut self,
        content: &str,
        marks: &[holon_api::MarkSpan],
        task_state: Option<&str>,
    ) -> String {
        let source = holon_org_format::render_inline_marks(content, marks);
        let Some(vocabulary) = self.vocabulary.as_ref() else {
            // UNRESOLVED, not "declares nothing". The surface stays unprefixed
            // and the router stays on the safe channel until the real
            // vocabulary arrives and this runs again.
            self.surface = crate::editor_source::Surface::Pending;
            self.surface_prefix = 0;
            return source;
        };
        let seed = crate::editor_source::project_or_disclose(
            self.handler.context_id().unwrap_or("<unmounted>"),
            &source,
            task_state,
            vocabulary,
        );
        self.surface = seed.surface;
        // The KEYWORD prefix only. The markup delta between source and content
        // is not a prefix at all and is crossed by the offset map in
        // `surface_caret_to_content`.
        self.surface_prefix = seed.text.len().saturating_sub(source.len());
        seed.text
    }

    /// Carry an offset expressed in CONTENT coordinates into the SURFACE the
    /// editor shows. Structural ops report their caret that way (`join_block`
    /// returns the merge boundary into the target's content, `split_block`
    /// returns 0), and the buffer that consumes the seed is the projection — so
    /// without this the caret lands inside the keyword on any tasked target and
    /// the next keystroke corrupts it (task #93), or before the inline markup
    /// the buffer carries and the content column does not.
    pub fn content_offset_to_surface(&self, offset: usize) -> anyhow::Result<usize> {
        Ok(self.surface_prefix
            + holon_org_format::source_content_offsets(surface_body(
                &self.buffer,
                self.surface_prefix,
            )?)
            .source_offset(offset)?)
    }

    /// This editor's caret in both coordinate systems, for
    /// [`structural_block_action`]. The ONE place a surface caret becomes a
    /// content caret in production.
    ///
    /// `surface` is the text the caret was measured on, passed in rather than
    /// read off `self.buffer` for the same reason
    /// [`Self::chord_commit_intent`] takes it: the widget's buffer can be one
    /// keystroke ahead of this VM's, and the caret belongs to the widget's.
    pub fn structural_caret(
        &self,
        surface: &str,
        surface_byte: usize,
    ) -> anyhow::Result<StructuralCaret> {
        Ok(StructuralCaret {
            surface_byte,
            content_byte: self.surface_offset_to_content(surface, surface_byte)?,
        })
    }

    /// The CONTENT byte a caret in this editor's buffer sits at — the seam
    /// every structural op crosses in the other direction.
    ///
    /// The buffer is vault syntax: a keyword the content column does not carry
    /// heads it, and `~code~` / `*bold*` / `[[u][Label]]` inside it are stored
    /// as a stripped label plus a mark set. A caret byte measured on it is
    /// therefore not a content byte, and handing one to `split_block` cut the
    /// wrong place or was refused outright
    /// (`2026-08-18-split-position-measured-on-the-editor-surface`).
    pub fn surface_offset_to_content(
        &self,
        surface: &str,
        surface_byte: usize,
    ) -> anyhow::Result<usize> {
        surface_caret_to_content(surface, self.surface_prefix, surface_byte)
    }

    /// What this editor's buffer means — see [`crate::editor_source::Surface`].
    pub fn surface(&self) -> crate::editor_source::Surface {
        self.surface
    }

    /// Adopt the owning document's `#+TODO:` vocabulary for this editing
    /// session. Called once per focus, before the first authority seed — it
    /// decides only what the surface may SHOW; the commit re-resolves the
    /// vocabulary at the store, so a stale copy here can never write a wrong
    /// task state.
    pub fn set_task_vocabulary(&mut self, vocabulary: holon_org_format::TaskKeywordVocabulary) {
        self.vocabulary = Some(vocabulary);
    }

    /// Apply a local (user-typed) edit to the authoritative buffer — the single
    /// keystroke sink. Mutates `buffer` to `new_text` and returns the
    /// persistence intent the frontend must dispatch (its sole commit funnel):
    ///
    /// - **Source-channel commit** (either mode): the buffer is — or has just
    ///   stopped being — vault syntax for a task, so the whole raw text goes to
    ///   `set_field("source_text")` and the STORE re-derives `content` and
    ///   `task_state` from it. In cell mode the CRDT delta is suppressed for
    ///   exactly these writes: the cell holds the content column, which is not
    ///   what the buffer says. See [`holon_org_format::source_channel_commit`].
    /// - **Cell mode** (Loro attached), ordinary text: computes the delta from
    ///   the previous buffer and applies it through the CRDT (`apply_local`);
    ///   returns `Ok(None)` — the Loro projection is the content writer.
    /// - **No-cell mode** (SqlOnly), real block: stamps a monotonic
    ///   `write_seq`, records it as `last_local_seq`, and returns
    ///   `Ok(Some(set_field intent))` so the typed text lands in the backend
    ///   before the next transition.
    /// - **Unchanged text**: returns `Ok(None)`.
    ///
    /// This is also where an empty-born block stops being reapable: the first
    /// non-empty text retires it from
    /// [`crate::creation_slot::EphemeralNewborns`] synchronously, BEFORE
    /// the write is dispatched, so a blur racing this keystroke can never
    /// find the block still reapable. Retirement is permanent — a block the
    /// user clears again is theirs, not the reaper's.
    ///
    /// RAW-seam hook: the `set_field` intent path is the single point where the
    /// dispatcher re-extracts inline marks on commit; keep it the sole commit
    /// funnel so raw→stripped extraction has exactly one home.
    /// Which channel this buffer's commit takes. The judgment is the SEED's,
    /// not the keystroke's, because only the seed saw the document's
    /// vocabulary:
    ///
    /// * `Refused` — the surface could not show the block's keyword, so it must
    ///   not be able to REMOVE it. Pinned to the content channel for the whole
    ///   session; the store never re-derives a task state from that channel.
    /// * `Projected` — the buffer IS vault syntax, so every commit re-derives
    ///   both columns, deletion of the keyword included.
    /// * `Untasked` — nothing to lose, so the cheap vocabulary-free shape rule
    ///   is enough: it decides only whether the STORE is asked to parse, and
    ///   the store's parse is itself vocabulary-aware and writes no task state
    ///   for text that names none. Reachable only AFTER a real vocabulary
    ///   classified the seed, so the router is never less vocabulary-aware than
    ///   the parse it feeds.
    /// * `Pending` — the vocabulary is not known, so nothing has been judged at
    ///   all. Same safe channel as `Refused`.
    fn commits_as_source(&self, new_text: &str) -> bool {
        match self.surface {
            // Unclassified and refused both mean "this surface may not change a
            // task state", for different reasons that both make the source
            // channel wrong.
            crate::editor_source::Surface::Pending | crate::editor_source::Surface::Refused => {
                false
            }
            crate::editor_source::Surface::Projected => true,
            crate::editor_source::Surface::Untasked => {
                holon_org_format::source_channel_commit(&self.buffer, new_text)
            }
        }
    }

    pub fn apply_local_edit(&mut self, new_text: &str) -> Result<Option<OperationIntent>> {
        if new_text == self.buffer {
            return Ok(None);
        }
        self.retire_from_the_reaper(new_text);
        self.note_local_edit();
        let source_channel = self.commits_as_source(new_text);
        if self.cell.is_some() && !source_channel {
            // Cell mode: apply the delta through the CRDT unless the cell
            // already holds this text (our own echo).
            if self.current_text().as_deref() != Some(new_text) {
                for op in crate::cell::compute_text_delta(&self.buffer, new_text) {
                    self.apply_local(op)?;
                }
            }
            self.buffer = new_text.to_string();
            return Ok(None);
        }
        let id = self.handler.context_id().map(str::to_string);
        self.buffer = new_text.to_string();
        let Some(id) = id else {
            return Ok(None);
        };
        // Stamp a monotonic ordering token on this write and record it as our
        // last local sequence BEFORE the caller dispatches, so a fast CDC echo
        // cannot race a not-yet-recorded seq.
        let seq = holon_api::write_seq::next();
        self.last_local_seq = seq.get();
        // This text is now persisted, so it is no longer pending: re-baseline
        // the commit funnels ([`Self::pending_commit_intent`]) off it. Without
        // this they re-dispatch it at every focus leave as a second, unordered
        // `set_field` with no `write_seq`.
        self.handler.set_baseline(new_text.to_string());
        let field = if source_channel {
            holon_api::SOURCE_TEXT_FIELD
        } else {
            "content"
        };
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(id));
        params.insert("field".into(), Value::String(field.to_string()));
        params.insert("value".into(), Value::String(new_text.to_string()));
        params.insert("write_seq".into(), Value::Integer(seq.get()));
        Ok(Some(OperationIntent::new(
            "block".into(),
            "set_field".into(),
            params,
        )))
    }

    /// Decide how to react to a SqlOnly data-sync echo (`new_value` carrying
    /// ordering token `echo_seq`), running the op-versioned echo-suppression
    /// rule against the VM's own authoritative `buffer`. Performs every
    /// VM-state mutation that is safe mid-IME-composition inline (high-water
    /// advance for InSync/Converge, baseline adopt for AdoptBaseline) and
    /// returns `Some(directive)` ONLY for the Converge case — the adapter must
    /// then set the visible InputState to the authority (immediately, or
    /// deferred past an IME composition). Returns `None` for InSync / DropStale
    /// / DropNoSeq / AdoptBaseline: the visible buffer is left untouched.
    pub fn converge_from_data_sync(
        &mut self,
        new_value: &str,
        echo_seq: Option<i64>,
    ) -> Option<ConvergeDirective> {
        match evaluate_data_sync_echo(&self.buffer, new_value, echo_seq, self.last_local_seq) {
            EchoDecision::InSync { advance_to } => {
                if let Some(seq) = advance_to {
                    self.advance_local_seq(seq);
                }
                None
            }
            EchoDecision::DropStale => None,
            EchoDecision::DropNoSeq => {
                // Content changed but the row carries no `write_seq` token — a
                // schema/projection regression. Fail LOUD and DROP: converging
                // blindly here is exactly the stale-echo data loss we prevent.
                tracing::error!(
                    target: "editor.data_sync",
                    row_id = ?self.handler.context_id(),
                    new = %new_value,
                    "data-sync echo has no write_seq column; dropping \
                     (schema/projection regression)"
                );
                None
            }
            EchoDecision::AdoptBaseline { seq } => {
                // The echo is the SQL-canonicalized form of our OWN in-flight
                // write (trailing whitespace trimmed on store). Keep the typed
                // buffer; re-baseline change tracking to the canonical authority
                // so a later blur diffs against SQL truth, not a stale baseline.
                self.advance_local_seq(seq);
                self.rebaseline(new_value);
                None
            }
            EchoDecision::Converge { seq } => {
                self.advance_local_seq(seq);
                Some(ConvergeDirective {
                    target: new_value.to_string(),
                    seq: self.last_local_seq,
                })
            }
        }
    }

    /// Build the convergence directive for a cell-mode remote-delta wakeup:
    /// converge the visible InputState to the live Loro authority
    /// (`current_text`). `None` when no cell is attached (the remote-delta loop
    /// only runs cell-attached). `seq` is the current high-water; cell-mode
    /// local edits do not stamp a write-seq, so a deferred remote directive
    /// always replays and the adapter re-reads the live (merged) cell.
    pub fn remote_converge_directive(&self) -> Option<ConvergeDirective> {
        let target = self.current_text()?;
        Some(ConvergeDirective {
            target,
            seq: self.last_local_seq,
        })
    }

    /// Stash a convergence directive the adapter deferred because an IME
    /// composition is in progress (`ime_marked_range().is_some()`). A newer
    /// directive supersedes an older pending one. The buffer is NOT committed
    /// while deferred, so a mid-composition converge never overwrites the
    /// in-flight composed text.
    pub fn set_pending_directive(&mut self, directive: ConvergeDirective) {
        self.pending_directive = Some(directive);
    }

    /// Take the deferred directive for replay on a composition-end / focus
    /// edge, clearing the pending slot. Returns `None` — discarding it — when a
    /// newer local write (a committed composition, which stamped a higher
    /// `write_seq`) superseded it since it was deferred.
    pub fn take_pending_directive(&mut self) -> Option<ConvergeDirective> {
        let directive = self.pending_directive.take()?;
        if self.last_local_seq > directive.seq {
            return None;
        }
        Some(directive)
    }

    /// Apply a local edit to the CRDT (origin-tagged so the remote-delta
    /// stream filters it out). Errors when no cell is attached —
    /// callers in headless contexts should gate on [`Self::has_cell`].
    pub fn apply_local(&self, op: TextOp) -> Result<()> {
        let cell = self
            .cell
            .as_ref()
            .ok_or_else(|| anyhow!("EditorViewModel has no Cell<String> attached"))?;
        cell.apply_text_op(op)
    }

    /// Stream of remote text deltas (peer edits, file reloads, CDC echoes
    /// of writes from elsewhere). Returns `None` when no cell is
    /// attached. Each frontend drives its own consumer loop on the
    /// returned stream — see GPUI `editor_view.rs` and TUI `app_main.rs`.
    pub fn remote_deltas(&self) -> Option<BoxStream<'static, TextDelta>> {
        self.cell.as_ref().map(|c| c.remote_deltas())
    }

    /// Anchor a cursor position so it can be re-resolved after a remote
    /// delta splices into the CRDT. `None` when no cell is attached or
    /// the backing has no text-rich support.
    pub fn anchor_cursor(&self, char_offset: usize, bias: CursorBias) -> Option<CursorAnchor> {
        self.cell
            .as_ref()
            .and_then(|c| c.anchor_cursor(char_offset, bias).ok()) // ALLOW(ok):
        // backings without
        // text-rich support
        // degrade to
        // None
    }

    /// Resolve a previously-anchored cursor against the current CRDT state.
    /// `None` when no cell is attached or the backing can't resolve the
    /// anchor (different backing, no text-rich support).
    pub fn resolve_cursor(&self, anchor: &CursorAnchor) -> Option<usize> {
        self.cell
            .as_ref()
            .and_then(|c| c.resolve_cursor(anchor).ok()) // ALLOW(ok): backings
        // without text-rich
        // support degrade to
        // None
    }

    /// Build an EditorViewModel from an EditableText ViewModel node.
    ///
    /// Extracts field, content, operations, triggers, and context params from
    /// the node. Panics if the node is not an EditableText.
    pub fn from_view_model(node: &crate::ViewModel) -> Self {
        let (field, content) = match &node.kind {
            crate::view_model::ViewKind::EditableText { field, content } => {
                (field.clone(), content.clone())
            }
            _ => panic!(
                "EditorViewModel::from_view_model called on non-EditableText node: {:?}",
                node.widget_name()
            ),
        };
        let context_params: HashMap<String, Value> = node
            .entity
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self::new(
            node.operations.clone(),
            node.triggers.clone(),
            context_params,
            field,
            content,
        )
    }

    /// Directly populate popup items (for tests / sync providers).
    pub fn set_popup_items(&mut self, items: Vec<PopupItem>) {
        self.handler.popup.set_items(items);
    }

    /// Enable async providers. `LinkProvider` needs a `BuilderServices`
    /// handle to run doc-link SQL queries against a real backend.
    pub fn set_async_context(&mut self, services: Arc<dyn BuilderServices>) {
        self.handler.set_async_context(services);
    }

    /// Called when the text content changes (every keystroke).
    ///
    /// `current_line` is the line the cursor is on.
    /// `cursor_byte` is the cursor BYTE offset within that line (GPUI callers
    /// convert their character column first — see `check_triggers`).
    pub fn on_text_changed(&mut self, current_line: &str, cursor_byte: usize) -> EditorAction {
        if let Some(expected) = &self.restoring_to {
            let is_the_restore = expected == current_line;
            self.restoring_to = None;
            if is_the_restore {
                // Our own re-insertion coming back as a change event. Letting it
                // reach `check_triggers` would re-open the menu on the `/` we
                // just put back, undoing the user's Escape.
                return EditorAction::None;
            }
        }
        if let Some(hidden) = &self.hidden_command {
            // While a picker phase hides its command text there is no trigger
            // char left on the line, so `check_triggers` would find nothing and
            // read the phase as dismissed on the very next keystroke. The phase
            // owns the input instead: what the user has typed since it opened
            // IS the search term.
            match hidden.anchor_in(current_line) {
                Some(anchor) if cursor_byte >= anchor => {
                    let term = current_line[anchor..cursor_byte].to_string();
                    self.handler.popup.on_text_changed(&term);
                    return EditorAction::UpdatePopup;
                }
                _ => {
                    // The edit crossed the anchor, so there is no longer a place
                    // on this line where the command text belongs. End the phase
                    // and say so, rather than reinserting it somewhere the user
                    // never typed it. Backspace at the anchor does NOT land here
                    // — the frontend intercepts that and cancels the phase with
                    // a proper restore (`cancel_hidden_phase_at_anchor`).
                    tracing::warn!(
                        hidden = %hidden.text,
                        line = %current_line,
                        "an edit crossed the hidden slash-command anchor; ending the picker \
                         phase and dropping the command text"
                    );
                    self.handler.popup.dismiss();
                    self.hidden_command = None;
                    return EditorAction::PopupDismissed;
                }
            }
        }

        let view_event = input_trigger::check_triggers(&self.triggers, current_line, cursor_byte);

        let result = if let Some(event) = view_event {
            self.handler.handle(event)
        } else if self.handler.is_overlay_active() {
            self.handler.handle(ViewEvent::TriggerDismissed)
        } else {
            return EditorAction::None;
        };

        self.handle_result_to_action(result)
    }

    /// Route a text-sync commit through the SAME channel decision the keystroke
    /// sink makes. [`ViewEventHandler::handle_text_sync`] builds its
    /// `set_field` from the editable node's own field, which is `content` —
    /// and for a surface that IS vault syntax those bytes are the
    /// projection, not the content column, so committing them there folds
    /// the keyword into the title. `Refused`/`Pending` surfaces keep the
    /// content channel, because `commits_as_source` is the same match the
    /// keystroke sink uses.
    fn route_commit_channel(&self, params: &mut HashMap<String, Value>) {
        let value = params
            .get("value")
            .and_then(|v| v.as_string())
            .expect("a text-sync commit always carries its value")
            .to_string();
        let field = params
            .get("field")
            .and_then(|v| v.as_string())
            .expect("a text-sync commit always carries its field");
        if field == "content" && self.commits_as_source(&value) {
            params.insert(
                "field".into(),
                Value::String(holon_api::SOURCE_TEXT_FIELD.to_string()),
            );
        }
    }

    /// Called when the editor loses focus (blur).
    ///
    /// If the text changed, returns `Execute` with a set_field operation.
    pub fn on_blur(&mut self, current_value: &str) -> EditorAction {
        let mut result = self.handler.handle(ViewEvent::TextSync {
            value: current_value.to_string(),
        });
        if let HandleResult::PopupResult(PopupResult::Execute { params, .. }) = &mut result {
            self.route_commit_channel(params);
        }
        self.handle_result_to_action(result)
    }

    /// Pending-text commit for a STRUCTURAL CHORD ("structural ops are commit
    /// points", docs/Architecture/UI.md): `Some(intent)` iff `live_text` is
    /// text this editor has NOT already put through its keystroke sink. The
    /// returned intent MUST be dispatched ordered BEFORE the structural op, or
    /// that text is lost.
    ///
    /// Narrower than the focus-leave funnel ([`Self::pending_commit_intent`])
    /// because the editor STAYS focused across a chord — and a focused editor
    /// receives no data-sync echo, so after a non-editor origin moves its row
    /// (a `split_block` from MCP or a peer) the buffer is silently STALE.
    /// Re-committing a buffer the keystroke sink already persisted then lands
    /// as a REVERT that resurrects the pre-split text beside the split's
    /// surviving tail, duplicating it (task #94). Text the sink never saw — an
    /// IME composition, a programmatic `set_value` — is genuinely pending and
    /// still flushes.
    pub fn chord_commit_intent(&mut self, live_text: &str) -> Option<OperationIntent> {
        if live_text == self.buffer {
            return None;
        }
        tracing::debug!(
            target: "editor.pending_commit",
            row_id = ?self.handler.context_id(),
            live = %live_text,
            buffer = %self.buffer,
            "chord flushes text the keystroke sink never saw"
        );
        self.pending_commit_intent(live_text)
    }

    /// Pending-text commit for the FOCUS-LEAVE boundary. Same decision as
    /// `on_blur`'s TextSync path — `Some(intent)` iff the live text diverged
    /// from the authority's view AND this editor is the content writer
    /// (SqlOnly); `None` when unchanged or when Loro's per-keystroke pipeline
    /// already writes through. Like a blur, this re-baselines the change
    /// tracking: the returned intent MUST be dispatched, ordered BEFORE any
    /// structural op, or the pending text is lost.
    pub fn pending_commit_intent(&mut self, live_text: &str) -> Option<OperationIntent> {
        let result = self.handler.handle(ViewEvent::TextSync {
            value: live_text.to_string(),
        });
        match result {
            HandleResult::PopupResult(PopupResult::Execute {
                entity_name,
                op_name,
                mut params,
                ..
            }) => {
                self.route_commit_channel(&mut params);
                Some(OperationIntent::new(entity_name, op_name, params))
            }
            _ => None,
        }
    }

    /// Called when a navigation key is pressed (Up/Down/Enter/Escape).
    ///
    /// If the popup is active, the key is routed to the popup.
    /// Otherwise returns `Propagate` so the frontend can handle
    /// cross-block navigation or other default behavior.
    pub fn on_key(&mut self, key: EditorKey) -> EditorAction {
        if !self.handler.is_overlay_active() {
            return match key {
                EditorKey::Enter => EditorAction::None, // let Input handle newline
                EditorKey::Escape => EditorAction::Propagate,
                EditorKey::Up | EditorKey::Down => EditorAction::Propagate,
                // Structural keys are routed through `structural_block_action`
                // by the frontend, not the popup controller — propagate so the
                // medium's own handler runs.
                EditorKey::Backspace | EditorKey::Tab | EditorKey::BackTab => {
                    EditorAction::Propagate
                }
            };
        }

        let menu_key = match key {
            EditorKey::Up => MenuKey::Up,
            EditorKey::Down => MenuKey::Down,
            EditorKey::Enter => MenuKey::Enter,
            EditorKey::Escape => MenuKey::Escape,
            // Not popup-navigation keys; let the frontend's structural/char
            // handling run instead of consuming them in the menu.
            EditorKey::Backspace | EditorKey::Tab | EditorKey::BackTab => {
                return EditorAction::Propagate;
            }
        };

        let result = self.handler.on_key(menu_key);
        self.popup_result_to_action(result)
    }

    /// Called when the user clicks the popup row at `index`.
    ///
    /// Deliberately shares `PopupMenu::select_current` with the Enter key
    /// rather than dispatching itself: a pointer pick and a keyboard pick must
    /// produce the same `PopupResult` for the same row, and a second dispatch
    /// path would drift (task #45 — the rows had no handler at all, so every
    /// command was mouse-dead).
    pub fn on_popup_item_clicked(&mut self, index: usize, expected_id: &str) -> EditorAction {
        let result = self.handler.on_item_clicked(index, expected_id);
        self.popup_result_to_action(result)
    }

    /// Report what the frontend removed from the visible editor in response to
    /// [`EditorAction::HideCommandText`]: the command `text` and the
    /// `line_prefix` that stood before it. The controller holds both until the
    /// picker phase ends, so a cancel can put the text back verbatim.
    pub fn command_text_hidden(&mut self, line_prefix: String, text: String) {
        self.hidden_command = Some(HiddenCommandText { line_prefix, text });
    }

    /// Report the line a [`EditorAction::RestoreCommandText`] just produced, so
    /// the change event that re-insertion fires is not mistaken for the user
    /// typing the trigger again.
    pub fn command_text_restored(&mut self, restored_line: String) {
        self.restoring_to = Some(restored_line);
    }

    /// The slash-command text currently held out of the visible editor.
    pub fn hidden_command_text(&self) -> Option<&HiddenCommandText> {
        self.hidden_command.as_ref()
    }

    /// Backspace pressed exactly at an open picker phase's hide anchor: cancel
    /// the phase and put its command text back, consuming the keystroke.
    ///
    /// Without this, the delete runs first and eats into the very region the
    /// anchor addresses — which is how a hide-time offset ends up indexing past
    /// the end of the buffer. Intercepting keeps the only reachable way out of
    /// a phase on the restoring path.
    ///
    /// `None` when no phase is hiding text or the caret is elsewhere, so the
    /// frontend's normal delete runs.
    pub fn cancel_hidden_phase_at_anchor(
        &mut self,
        current_line: &str,
        cursor_byte: usize,
    ) -> Option<EditorAction> {
        if self.hidden_command.as_ref()?.anchor_in(current_line)? != cursor_byte {
            return None;
        }
        let hidden = self.hidden_command.take().expect("checked just above");
        self.handler.popup.dismiss();
        Some(EditorAction::RestoreCommandText {
            line_prefix: hidden.line_prefix,
            text: hidden.text,
        })
    }

    /// Whether the popup overlay is currently visible.
    pub fn is_popup_active(&self) -> bool {
        self.handler.is_overlay_active()
    }

    /// Current popup state for rendering. Returns `None` if no popup is active.
    pub fn popup_state(&self) -> Option<PopupState> {
        self.handler.popup.popup_state()
    }

    /// Apply an inline mark over a range of the block's text.
    ///
    /// Range is in Unicode-scalar offsets, half-open `[range.start,
    /// range.end)`. Returns an `Execute(OperationIntent)` for the
    /// `apply_mark` operation on the `block` entity; the frontend
    /// dispatches it through its standard operation pipeline. This is
    /// incremental — pre-existing marks of other keys, or same-key spans on
    /// disjoint ranges, are preserved.
    ///
    /// Returns `EditorAction::None` if the controller's context has no `id`
    /// (a programming error in the wiring; logged by callers if needed).
    pub fn apply_mark(&self, range: Range<usize>, mark: &InlineMark) -> EditorAction {
        let Some(id) = self.handler.context_id() else {
            return EditorAction::None;
        };
        let mark_json = serde_json::to_string(mark).expect("InlineMark serialization is total");
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("range_start".into(), Value::Integer(range.start as i64));
        params.insert("range_end".into(), Value::Integer(range.end as i64));
        params.insert("mark_json".into(), Value::String(mark_json));
        EditorAction::Execute(OperationIntent::new(
            EntityName::new("block"),
            "apply_mark".into(),
            params,
        ))
    }

    /// Remove an inline mark over a range of the block's text.
    ///
    /// The `mark` argument's `loro_key()` selects which key to unmark. The
    /// mark's value (e.g. a Link's target) is ignored — `unmark` is a
    /// range-based operation that doesn't need the original value to
    /// identify what to remove.
    pub fn remove_mark(&self, range: Range<usize>, mark: &InlineMark) -> EditorAction {
        let Some(id) = self.handler.context_id() else {
            return EditorAction::None;
        };
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("range_start".into(), Value::Integer(range.start as i64));
        params.insert("range_end".into(), Value::Integer(range.end as i64));
        params.insert("key".into(), Value::String(mark.loro_key().into()));
        EditorAction::Execute(OperationIntent::new(
            EntityName::new("block"),
            "remove_mark".into(),
            params,
        ))
    }

    fn handle_result_to_action(&mut self, result: HandleResult) -> EditorAction {
        match result {
            HandleResult::Activated { signal } => EditorAction::PopupActivated { signal },
            HandleResult::PopupResult(pr) => self.popup_result_to_action(pr),
        }
    }

    fn popup_result_to_action(&mut self, result: PopupResult) -> EditorAction {
        // Every terminal result consumes the command: the frontend's own
        // `strip_prefix_start` handling removes what is left on the line, so
        // the hidden text must NOT come back on top of it.
        if matches!(
            result,
            PopupResult::Execute { .. }
                | PopupResult::InsertText { .. }
                | PopupResult::Failed { .. }
        ) {
            self.hidden_command = None;
        }
        match result {
            PopupResult::NotActive => EditorAction::None,
            PopupResult::Updated => EditorAction::UpdatePopup,
            PopupResult::PhaseAdvanced { hide: Some(span) } => EditorAction::HideCommandText {
                prefix_start: span.prefix_start,
                len: span.len,
            },
            // A caller that manages its own editor text (the headless mirror)
            // has nothing to hide — the phase change is just a re-render.
            PopupResult::PhaseAdvanced { hide: None } => EditorAction::UpdatePopup,
            PopupResult::Dismissed => match self.hidden_command.take() {
                Some(hidden) => EditorAction::RestoreCommandText {
                    line_prefix: hidden.line_prefix,
                    text: hidden.text,
                },
                None => EditorAction::PopupDismissed,
            },
            PopupResult::Execute {
                entity_name,
                op_name,
                params,
                strip_prefix_start,
            } => {
                let intent = OperationIntent::new(entity_name, op_name, params);
                match strip_prefix_start {
                    Some(strip_prefix_start) => EditorAction::ExecuteAndStripCommand {
                        intent,
                        strip_prefix_start,
                    },
                    None => EditorAction::Execute(intent),
                }
            }
            PopupResult::InsertText {
                replacement,
                prefix_start,
            } => EditorAction::InsertText {
                replacement,
                prefix_start,
            },
            PopupResult::Failed {
                message,
                strip_prefix_start,
            } => EditorAction::CommandFailed {
                message,
                strip_prefix_start,
            },
        }
    }
}

/// Abstract keyboard keys that the editor controller handles.
///
/// Frontends map their platform-specific key types to this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKey {
    Up,
    Down,
    Enter,
    Escape,
    /// Backspace. Structural only at caret 0 (join with previous block);
    /// elsewhere it's a per-medium char delete (see
    /// [`structural_block_action`]).
    Backspace,
    /// Tab → indent.
    Tab,
    /// Shift+Tab → outdent.
    BackTab,
}

/// `surface` past the task keyword it shows — the org source the content
/// column is parsed from.
fn surface_body(surface: &str, surface_prefix: usize) -> anyhow::Result<&str> {
    surface.get(surface_prefix..).ok_or_else(|| {
        anyhow::anyhow!(
            "surface prefix {surface_prefix} is not a boundary of the {}-byte surface {surface:?}",
            surface.len(),
        )
    })
}

/// The CONTENT byte a caret measured on an editable SURFACE sits at.
///
/// The seam between what the user edits and what the store keeps, in ONE
/// place: production's [`EditorViewModel::surface_offset_to_content`] and the
/// keystone's reference model both call this, so the model cannot describe a
/// caret production does not compute.
///
/// `surface_prefix` is the task keyword the surface shows and the content
/// column does not carry; the rest of the surface is org source, whose
/// delimiters (`~code~`, `[[u][Label]]`) the store strips into marks.
pub fn surface_caret_to_content(
    surface: &str,
    surface_prefix: usize,
    surface_byte: usize,
) -> anyhow::Result<usize> {
    if surface_byte > surface.len() {
        anyhow::bail!(
            "caret {surface_byte} is past the {}-byte surface {surface:?}",
            surface.len(),
        );
    }
    // A caret inside the keyword precedes every content byte: the keyword is
    // the surface's, not the content's.
    let Some(body_byte) = surface_byte.checked_sub(surface_prefix) else {
        return Ok(0);
    };
    holon_org_format::source_content_offsets(surface_body(surface, surface_prefix)?)
        .content_offset(body_byte)
}

/// The structural block operation a key triggers when no popup/completion is
/// active, given the caret byte offset. This is the single source of truth for
/// the Enter→split / Backspace-at-0→join / Tab→indent / Shift+Tab→outdent
/// decision, shared by every frontend so the real UI (GPUI `editor_view`
/// capture handlers) and the test harness (`HeadlessEditorMirror`) can't drift.
///
/// Returns `None` for keys that don't map to a structural op at this caret
/// (plain char input, cursor moves, mid-line backspace) — the caller applies
/// those in its own medium: GPUI lets `InputState` consume them, the headless
/// mirror mutates its `MutableText` + cursor model directly.
///
/// `target_id` is the block the op acts on (the focused leaf, which a GPUI
/// Page-level editor resolves via `services.focused_block()`); `caret` carries
/// the cursor in BOTH coordinate systems, because the two decisions here need
/// different ones — see [`StructuralCaret`].
pub fn structural_block_action(
    key: EditorKey,
    target_id: &str,
    caret: StructuralCaret,
) -> Option<OperationIntent> {
    // A creation affordance is a rendered row, not a block: it mounts no editor
    // and focus on it is intercepted into a birth, so no keystroke can be aimed
    // at one. An affordance id reaching this table therefore means the birth
    // interception was bypassed — a frontend routing bug, which must panic here
    // rather than travel one layer down and surface as a backend "Block not
    // found" against an id that never existed.
    assert!(
        !crate::row_origin::RowOrigin::from_id(target_id).is_creation_placeholder(),
        "structural {key:?} dispatched against creation-affordance id {target_id:?} — an \
         affordance is not a block; focus on it must be intercepted into a birth \
         (`ReactiveEngine::birth_creation_affordance`) before any structural op can be aimed at it"
    );
    let intent = |op: &str, position: Option<i64>| {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(target_id.to_string()));
        if let Some(p) = position {
            params.insert("position".to_string(), Value::Integer(p));
        }
        OperationIntent::new(EntityName::new("block"), op.to_string(), params)
    };
    match key {
        EditorKey::Enter => Some(intent("split_block", Some(caret.content_byte as i64))),
        EditorKey::Backspace if caret.surface_byte == 0 => Some(intent("join_block", Some(0))),
        EditorKey::Tab => Some(intent("indent", None)),
        EditorKey::BackTab => Some(intent("outdent", None)),
        EditorKey::Backspace | EditorKey::Up | EditorKey::Down | EditorKey::Escape => None,
    }
}

/// One caret, in the two coordinate systems a structural key needs.
///
/// The buffer the user's caret is measured on is VAULT SYNTAX; the block's
/// content column is its parse (keyword stripped, `~code~` stored as a label
/// plus a mark). The two decisions here read different ones and must not be
/// collapsed: a split CUTS the content, so it needs the content byte, while
/// "is the caret at the very start" is a question about what the user sees —
/// on `TODO milk` the surface byte right after the keyword is content byte 0,
/// and answering Backspace there with a join would swallow the block instead
/// of deleting a space.
///
/// Built by [`EditorViewModel::structural_caret`], which owns the projection
/// the two coordinates differ by — there is no constructor that lets a caller
/// pass one number for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuralCaret {
    surface_byte: usize,
    content_byte: usize,
}

impl StructuralCaret {
    /// A caret on a buffer that IS its own content — no keyword, no markup.
    /// For callers with no editor projection to cross (the reference model's
    /// plain-text mirrors, tests seeding a bare block).
    pub fn on_plain_text(byte: usize) -> Self {
        Self {
            surface_byte: byte,
            content_byte: byte,
        }
    }

    /// A caret whose two coordinates a caller already resolved itself — for a
    /// frontend with no [`EditorViewModel`] to own the projection.
    pub fn at(surface_byte: usize, content_byte: usize) -> Self {
        Self {
            surface_byte,
            content_byte,
        }
    }

    pub fn surface_byte(&self) -> usize {
        self.surface_byte
    }

    pub fn content_byte(&self) -> usize {
        self.content_byte
    }
}

/// Marks that fully cover `[range.start, range.end)`.
///
/// Used by toolbars to drive the "this button is ON" state — Bold appears
/// pressed when every scalar in the selection is Bold. A selection where
/// only part is Bold returns no Bold entry; the toolbar should treat that
/// as "mixed" / "off" depending on its UX choice (this helper deliberately
/// doesn't compute a tri-state since callers' definitions of "mixed" vary).
///
/// Empty selections (`range.start == range.end`) treat the position as a
/// caret: a mark covers the caret iff `mark.start <= pos && mark.end >= pos`.
/// (At the right boundary, `ExpandType::After` marks are reported as active
/// so toolbar state matches what typing-at-the-boundary would inherit.)
pub fn selection_marks(marks: &[MarkSpan], range: Range<usize>) -> Vec<InlineMark> {
    marks
        .iter()
        .filter(|m| m.start <= range.start && m.end >= range.end)
        .map(|m| m.mark.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use holon_api::render_types::OperationDescriptor;
    use holon_api::render_types::OperationParam;
    use holon_api::render_types::TypeHint;
    use holon_api::types::EntityName;

    use super::*;
    use crate::input_trigger::InputTrigger;

    fn make_op(name: &str, fields: &[&str], params: Vec<OperationParam>) -> OperationWiring {
        OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                name: name.into(),
                display_name: name.into(),
                required_params: params,
                affected_fields: fields.iter().map(|s| s.to_string()).collect(),
                id_column: "id".to_string(),
                description: String::new(),
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Test,
                },
                trigger: None,
                bound_params: Default::default(),
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            },
        }
    }

    fn param(name: &str) -> OperationParam {
        OperationParam {
            name: name.into(),
            type_hint: TypeHint::String,
            description: String::new(),
        }
    }

    fn test_controller() -> EditorViewModel {
        let ops = vec![
            make_op(
                "set_field",
                &["content"],
                vec![param("id"), param("field"), param("value")],
            ),
            make_op("delete", &["parent_id"], vec![param("id")]),
        ];
        let triggers = vec![
            InputTrigger::TextPrefix {
                prefix: "/".to_string(),
                action: "command_menu".to_string(),
                at_line_start: true,
                word_boundary: false,
            },
            InputTrigger::TextPrefix {
                prefix: "[[".to_string(),
                action: "doc_link".to_string(),
                at_line_start: false,
                word_boundary: false,
            },
        ];
        let context = HashMap::from([("id".into(), Value::String("block-1".into()))]);
        EditorViewModel::new(ops, triggers, context, "content".into(), "original".into())
    }

    #[test]
    fn normal_text_returns_none() {
        let mut ctrl = test_controller();
        let action = ctrl.on_text_changed("hello world", 11);
        assert!(matches!(action, EditorAction::None));
    }

    #[test]
    fn slash_at_start_activates_popup() {
        let mut ctrl = test_controller();
        let action = ctrl.on_text_changed("/", 1);
        assert!(matches!(action, EditorAction::PopupActivated { .. }));
        assert!(ctrl.is_popup_active());
    }

    /// Controller wired with the PRODUCTION triggers a rendered editable block
    /// gets (`default_triggers_for_operations`: mid-line `/` command_menu with
    /// the word-boundary gate + always-on `[[`). `test_controller`'s slash
    /// trigger is `at_line_start: true`, which does NOT exercise the mid-line
    /// URL path — this one does.
    fn prod_controller() -> EditorViewModel {
        let ops = vec![
            make_op(
                "set_field",
                &["content"],
                vec![param("id"), param("field"), param("value")],
            ),
            make_op("delete", &["parent_id"], vec![param("id")]),
        ];
        let triggers = crate::input_trigger::default_triggers_for_operations(&ops);
        let context = HashMap::from([("id".into(), Value::String("block-1".into()))]);
        EditorViewModel::new(ops, triggers, context, "content".into(), "original".into())
    }

    /// Regression (BugFunnel): a block whose content is a URL must NOT open the
    /// command menu. Before the word-boundary gate, the trailing `/path` fired
    /// `command_menu`; the URL tail became a filter matching nothing → the
    /// permanent "Type to search…" popup that never dismissed. This drives the
    /// exact per-keystroke entry the GPUI editor view calls (`on_text_changed`)
    /// and observes the real popup state (`is_popup_active`).
    #[test]
    fn url_content_does_not_activate_command_menu() {
        let mut ctrl = prod_controller();
        let url = "https://example.com/path";
        let action = ctrl.on_text_changed(url, url.len());
        assert!(
            matches!(action, EditorAction::None),
            "URL content must not activate any popup, got {action:?}"
        );
        assert!(
            !ctrl.is_popup_active(),
            "command menu must stay closed for URL content"
        );
    }

    /// Logseq-style `text /cmd`: a `/` right after whitespace still opens the
    /// menu (the boundary gate must not over-reject).
    #[test]
    fn slash_after_space_activates_command_menu() {
        let mut ctrl = prod_controller();
        let line = "foo /de";
        let action = ctrl.on_text_changed(line, line.len());
        assert!(
            matches!(action, EditorAction::PopupActivated { .. }),
            "slash after a space must open the command menu, got {action:?}"
        );
        assert!(ctrl.is_popup_active());
    }

    /// Dismissal-side consistency: once the menu is open, typing on into a URL
    /// (so `check_triggers` now returns `None`) must DISMISS it — the fix keeps
    /// the open/close sides symmetric, so a popup can never get stuck open on a
    /// URL block.
    #[test]
    fn url_after_open_menu_dismisses_it() {
        let mut ctrl = prod_controller();
        ctrl.on_text_changed("/", 1);
        assert!(ctrl.is_popup_active(), "precondition: menu opened on '/'");
        let url = "https://example.com/path";
        let action = ctrl.on_text_changed(url, url.len());
        assert!(
            matches!(action, EditorAction::PopupDismissed),
            "typing into a URL must dismiss the menu, got {action:?}"
        );
        assert!(!ctrl.is_popup_active());
    }

    #[test]
    fn key_up_propagates_when_no_popup() {
        let mut ctrl = test_controller();
        let action = ctrl.on_key(EditorKey::Up);
        assert!(matches!(action, EditorAction::Propagate));
    }

    #[test]
    fn key_up_updates_popup_when_active() {
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/", 1);
        // Manually set items so popup has something to navigate
        ctrl.handler.popup.set_items(vec![
            PopupItem {
                id: "a".into(),
                label: "A".into(),
                icon: None,
            },
            PopupItem {
                id: "b".into(),
                label: "B".into(),
                icon: None,
            },
        ]);
        let action = ctrl.on_key(EditorKey::Down);
        assert!(matches!(action, EditorAction::UpdatePopup));
    }

    #[test]
    fn escape_dismisses_active_popup() {
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/", 1);
        let action = ctrl.on_key(EditorKey::Escape);
        assert!(matches!(action, EditorAction::PopupDismissed));
        assert!(!ctrl.is_popup_active());
    }

    #[test]
    fn escape_propagates_when_no_popup() {
        let mut ctrl = test_controller();
        let action = ctrl.on_key(EditorKey::Escape);
        assert!(matches!(action, EditorAction::Propagate));
    }

    #[test]
    fn enter_executes_selected_command() {
        // Selecting a slash-command from the popup must strip the typed
        // "/delete" text from the editor BEFORE dispatching the op — see
        // `EditorAction::ExecuteAndStripCommand`'s doc comment — otherwise
        // the trigger text remains in the block content and gets committed
        // at the next commit point. Plain `Execute` (no strip) is only for
        // non-popup paths (blur set_field etc.).
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/", 1);
        ctrl.handler.popup.set_items(vec![PopupItem {
            id: "delete".into(),
            label: "Delete".into(),
            icon: None,
        }]);
        let action = ctrl.on_key(EditorKey::Enter);
        match action {
            EditorAction::ExecuteAndStripCommand {
                intent,
                strip_prefix_start,
            } => {
                assert_eq!(intent.op_name, "delete");
                assert_eq!(intent.params["id"], Value::String("block-1".into()));
                assert_eq!(strip_prefix_start, 0);
            }
            other => panic!("Expected ExecuteAndStripCommand, got {:?}", other),
        }
    }

    #[test]
    fn blur_with_changed_text_executes() {
        let mut ctrl = test_controller();
        let action = ctrl.on_blur("new text");
        match action {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.op_name, "set_field");
                assert_eq!(intent.params["value"], Value::String("new text".into()));
                assert_eq!(intent.params["field"], Value::String("content".into()));
            }
            other => panic!("Expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn blur_with_same_text_returns_none() {
        let mut ctrl = test_controller();
        let action = ctrl.on_blur("original");
        assert!(matches!(action, EditorAction::None));
    }

    #[test]
    fn blur_with_changed_text_drops_content_when_loro_cell_attached() {
        // When a Loro content writer (`Cell`) is attached, the per-keystroke
        // pipeline owns content persistence; the on-blur `set_field("content")`
        // must be dropped to avoid racing the Loro projection. Without a cell
        // (SqlOnly mode) it MUST fire — that case is covered by
        // `blur_with_changed_text_executes`.
        use holon_core::cell::CellBacking;
        use holon_core::cell::LwwTextCellBacking;
        let mut ctrl = test_controller();
        let backing = std::sync::Arc::new(LwwTextCellBacking::new(
            std::sync::Arc::new(|| "original".to_string()),
            std::sync::Arc::new(|_| Box::pin(async { Ok(()) })),
            std::sync::Arc::new(|| Box::pin(futures::stream::empty())),
        ));
        ctrl.attach_cell(Cell::from_backing(
            backing as std::sync::Arc<dyn CellBacking<String>>,
        ));
        let action = ctrl.on_blur("new text");
        assert!(
            matches!(action, EditorAction::None),
            "content set_field must be dropped when a Loro cell is the writer, got {action:?}"
        );
    }

    #[test]
    fn cell_authority_reflects_merged_content_after_external_join() {
        // Regression (2026-07-10): after a `join_block`, the surviving block's
        // content is merged in the backend, and the editor's content authority
        // — the attached `Cell` read via `current_text()` — MUST reflect that
        // merged value. The GPUI editor's focus-gain reload converges its
        // `InputState` to exactly this authority; if the authority itself were
        // stale, the reload could not cure the stale buffer. This pins the
        // authority contract the fix depends on: `current_text()` is a live
        // read of the cell backing, never a snapshot taken at attach time.
        use std::sync::Arc;
        use std::sync::Mutex;

        use holon_core::cell::CellBacking;
        use holon_core::cell::LwwTextCellBacking;

        // Shared backing store the "backend" (join_block's set_field) writes to.
        let store = Arc::new(Mutex::new("First manual block".to_string())); // pre-split (18)
        let read_store = store.clone();
        let backing = Arc::new(LwwTextCellBacking::new(
            Arc::new(move || read_store.lock().unwrap().clone()),
            Arc::new(|_| Box::pin(async { Ok(()) })),
            Arc::new(|| Box::pin(futures::stream::empty())),
        ));
        let mut ctrl = test_controller();
        ctrl.attach_cell(Cell::from_backing(backing as Arc<dyn CellBacking<String>>));

        assert_eq!(ctrl.current_text().as_deref(), Some("First manual block"));

        // Backend join merges the two blocks into the survivor (17 chars),
        // dropping the space, exactly as prod `join_block`'s `set_field` does.
        *store.lock().unwrap() = "First manualblock".to_string();

        assert_eq!(
            ctrl.current_text().as_deref(),
            Some("First manualblock"),
            "content authority must be a live read of the cell — a focus reload converges \
             InputState to this, curing the stale pre-join buffer"
        );
    }

    #[test]
    fn double_bracket_fires_doc_link() {
        let mut ctrl = test_controller();
        // Without async context, doc_link returns None (no LinkProvider)
        let action = ctrl.on_text_changed("see [[proj", 10);
        assert!(matches!(action, EditorAction::None));
    }

    #[test]
    fn text_change_dismisses_stale_popup() {
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/del", 4); // activates popup
        assert!(ctrl.is_popup_active());
        // Type normal text (no trigger match) → should dismiss
        let action = ctrl.on_text_changed("hello", 5);
        assert!(matches!(action, EditorAction::PopupDismissed));
        assert!(!ctrl.is_popup_active());
    }

    #[test]
    fn apply_mark_emits_apply_mark_intent() {
        let ctrl = test_controller();
        let action = ctrl.apply_mark(0..5, &InlineMark::Bold);
        match action {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.entity_name, EntityName::new("block"));
                assert_eq!(intent.op_name, "apply_mark");
                assert_eq!(intent.params["id"], Value::String("block-1".into()));
                assert_eq!(intent.params["range_start"], Value::Integer(0));
                assert_eq!(intent.params["range_end"], Value::Integer(5));
                let mark_json = intent.params["mark_json"]
                    .as_string()
                    .expect("mark_json string");
                let mark: InlineMark =
                    serde_json::from_str(mark_json).expect("mark_json round-trips");
                assert_eq!(mark, InlineMark::Bold);
            }
            other => panic!("expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn apply_mark_round_trips_link_target() {
        // Link variants carry data (target + label); intent payload must
        // preserve them so the backend reconstitutes the full InlineMark.
        use holon_api::EntityRef;
        use holon_api::EntityUri;

        let ctrl = test_controller();
        let mark = InlineMark::Link {
            target: EntityRef::from_uri(&EntityUri::block("abc-123")),
            label: "see also".into(),
        };
        let action = ctrl.apply_mark(2..10, &mark);
        let EditorAction::Execute(intent) = action else {
            panic!("expected Execute");
        };
        let mark_json = intent.params["mark_json"].as_string().unwrap();
        let parsed: InlineMark = serde_json::from_str(mark_json).unwrap();
        assert_eq!(parsed, mark);
    }

    #[test]
    fn remove_mark_emits_remove_mark_intent_with_key() {
        let ctrl = test_controller();
        let action = ctrl.remove_mark(3..7, &InlineMark::Italic);
        match action {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.op_name, "remove_mark");
                assert_eq!(intent.params["range_start"], Value::Integer(3));
                assert_eq!(intent.params["range_end"], Value::Integer(7));
                assert_eq!(intent.params["key"], Value::String("italic".into()));
                // No mark_json — remove only needs the key.
                assert!(!intent.params.contains_key("mark_json"));
            }
            other => panic!("expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn selection_marks_returns_marks_fully_covering_range() {
        let marks = vec![
            MarkSpan::new(0, 10, InlineMark::Bold),     // covers selection
            MarkSpan::new(5, 7, InlineMark::Italic),    // doesn't cover selection
            MarkSpan::new(0, 8, InlineMark::Underline), // covers selection
        ];
        let active = selection_marks(&marks, 2..8);
        assert!(active.contains(&InlineMark::Bold));
        assert!(active.contains(&InlineMark::Underline));
        assert!(!active.contains(&InlineMark::Italic));
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn selection_marks_partial_cover_excludes() {
        // Mark must cover the ENTIRE range — half-overlap doesn't count.
        let marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
        let active = selection_marks(&marks, 3..7);
        assert!(active.is_empty(), "Bold over [0,5) does NOT cover [3,7)");
    }

    #[test]
    fn selection_marks_caret_at_mark_boundary() {
        // Empty range = caret. Caret at position 5 inside Bold([0,5)) — the
        // mark's right boundary. Per ExpandType::After, typing here inherits
        // Bold, so the toolbar should show Bold ON.
        let marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
        let active = selection_marks(&marks, 5..5);
        assert!(active.contains(&InlineMark::Bold));
    }

    fn block_vm(id: &str) -> EditorViewModel {
        let context_params = HashMap::from([("id".to_string(), Value::String(id.to_string()))]);
        EditorViewModel::new(
            Vec::new(),
            Vec::new(),
            context_params,
            "content".to_string(),
            String::new(),
        )
    }

    /// Care (6): a block born empty by a creation affordance takes `indent`
    /// IMMEDIATELY. Before ruling (C) the caret sat in a virtual slot and this
    /// call site asserted itself dead — the user's Tab did nothing at all and
    /// said nothing about it.
    #[test]
    fn an_empty_born_block_indents_immediately() {
        let intent = structural_block_action(
            EditorKey::Tab,
            "block:newborn-1",
            StructuralCaret::on_plain_text(0),
        )
        .expect("Tab in an empty-born block must dispatch indent");
        assert_eq!(intent.op_name, "indent");
        assert_eq!(intent.params["id"], Value::String("block:newborn-1".into()));
    }

    /// Care (7): Enter in an empty-born block is an ordinary `split_block` at
    /// offset 0 — it opens a fresh empty sibling below, the least surprising
    /// outliner behaviour and the SAME gesture as Enter anywhere else. There is
    /// no longer a "commit the slot" special case to be surprised by.
    #[test]
    fn enter_in_an_empty_born_block_splits_like_any_other_block() {
        let intent = structural_block_action(
            EditorKey::Enter,
            "block:newborn-1",
            StructuralCaret::on_plain_text(0),
        )
        .expect("Enter in an empty-born block must dispatch split_block");
        assert_eq!(intent.op_name, "split_block");
        assert_eq!(intent.params["position"], Value::Integer(0));
    }

    /// An affordance id in the structural table means the birth interception
    /// was bypassed. Fail loud HERE, where the routing bug is, rather than one
    /// layer down as a backend "Block not found" against an id that never
    /// existed.
    #[test]
    #[should_panic(expected = "creation-affordance id")]
    fn a_structural_op_against_an_affordance_id_is_a_loud_routing_bug() {
        structural_block_action(
            EditorKey::Enter,
            "block:__virtual:page-1",
            StructuralCaret::on_plain_text(0),
        );
    }

    /// The keystroke sink retires an empty-born block from the reaper BEFORE it
    /// dispatches the write, which is what makes a racing blur unable to reap a
    /// block the user has typed into (care 2).
    #[test]
    fn the_first_non_empty_keystroke_retires_the_block_from_the_reaper() {
        let newborns = std::sync::Arc::new(crate::creation_slot::EphemeralNewborns::new());
        let id = holon_api::EntityUri::parse("block:newborn-1").unwrap();
        newborns.record("block:__virtual:p", id.clone());

        let mut vm = block_vm("block:newborn-1");
        vm.set_ephemeral_newborns(newborns.clone());
        vm.apply_local_edit("h").expect("keystroke");

        assert!(
            !newborns.is_ephemeral(&id),
            "a block that has been typed into must no longer be reapable"
        );
    }

    /// Care (1): clearing the block again does NOT re-arm the reaper — the user
    /// deliberately emptied their own block and it must survive the blur.
    #[test]
    fn clearing_a_typed_block_does_not_make_it_reapable_again() {
        let newborns = std::sync::Arc::new(crate::creation_slot::EphemeralNewborns::new());
        let id = holon_api::EntityUri::parse("block:newborn-1").unwrap();
        newborns.record("block:__virtual:p", id.clone());

        let mut vm = block_vm("block:newborn-1");
        vm.set_ephemeral_newborns(newborns.clone());
        vm.apply_local_edit("h").expect("typed");
        vm.apply_local_edit("").expect("cleared");

        assert!(!newborns.is_ephemeral(&id));
    }

    /// BugFunnel 2026-07-13 defect (a) — spurious identical-content blur commit
    /// after refocus. When `converge_input` absolutely re-seeds the visible
    /// buffer from the STORED (stripped) content it re-baselines change
    /// tracking via [`EditorViewModel::rebaseline`]; the next blur of that
    /// unmodified, re-seeded editor must NOT commit. Before the fix the
    /// baseline still held the raw typed markup (`[[Some Page]]`), so the
    /// blur diffed the stripped buffer (`Some Page`) as "changed" and fired
    /// a `set_field("content")` with text identical to storage — which
    /// nulled live link marks and polluted the undo stack.
    #[test]
    fn reseed_rebaseline_suppresses_spurious_blur_commit() {
        let mut vm = test_controller();
        // User typed `[[Some Page]]`; the first blur commits it once and
        // re-baselines the handler to the raw typed markup.
        assert!(
            matches!(vm.on_blur("[[Some Page]]"), EditorAction::Execute(_)),
            "first blur after a genuine edit must commit once"
        );

        // Refocus: converge_input re-seeds the buffer to the stored STRIPPED
        // content and re-baselines to it.
        vm.rebaseline("Some Page");

        // Blur of the re-seeded, unmodified editor: NO commit.
        assert!(
            matches!(vm.on_blur("Some Page"), EditorAction::None),
            "blur of a re-seeded, unmodified editor must NOT commit (spurious identical-content \
             set_field wipes marks / poisons undo)"
        );
    }

    /// Text the keystroke sink has already persisted is not pending: the
    /// focus-leave funnel must see nothing to commit. A second, unordered
    /// `set_field` of the same text carries no `write_seq` (so its own echo is
    /// dropped) and lands AFTER whatever structural op moved the focus — when
    /// that op was the `join_block` that consumed this very block, the write
    /// hits a block that no longer exists.
    #[test]
    fn the_focus_leave_funnel_does_not_recommit_what_the_keystroke_sink_wrote() {
        let mut vm = test_controller();
        let typed = vm
            .apply_local_edit("")
            .expect("keystroke")
            .expect("SqlOnly keystroke sink writes content");
        assert!(
            typed.params.contains_key("write_seq"),
            "the keystroke sink stamps an ordering token"
        );

        assert!(
            vm.chord_commit_intent("").is_none(),
            "the structural commit point has nothing left to flush"
        );
        assert!(
            vm.pending_commit_intent("").is_none(),
            "focus leave re-committed already-persisted text"
        );
    }

    /// The reseed re-baseline must not swallow a GENUINE post-reseed edit:
    /// typing after a reseed still commits exactly once, and a follow-up blur
    /// with the same text is idempotent.
    #[test]
    fn typed_change_after_reseed_commits_once() {
        let mut vm = test_controller();
        vm.rebaseline("Some Page");
        match vm.on_blur("Some Page edited") {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.op_name, "set_field");
                assert_eq!(
                    intent.params["value"],
                    Value::String("Some Page edited".into())
                );
            }
            other => panic!("a real edit after reseed must commit once, got {:?}", other),
        }
        assert!(
            matches!(vm.on_blur("Some Page edited"), EditorAction::None),
            "second blur with unchanged text must not re-commit"
        );
    }

    /// Inc 3: the convergence DECISION lives in the VM. A genuinely newer
    /// external authority write yields a directive the adapter must apply; the
    /// non-converging echo outcomes yield `None` and only mutate VM state.
    #[test]
    fn data_sync_decision_maps_echo_outcomes_to_directive() {
        let mut vm = test_controller();
        vm.advance_local_seq(10);

        // Stale echo (seq < high-water) → dropped, no directive, no advance.
        assert!(vm.converge_from_data_sync("older", Some(5)).is_none());
        assert_eq!(vm.last_local_seq(), 10);

        // In-sync echo of the current buffer → no directive, advances high-water.
        assert!(vm.converge_from_data_sync("original", Some(12)).is_none());
        assert_eq!(vm.last_local_seq(), 12);

        // Genuinely newer external write → Converge directive for the adapter.
        let directive = vm
            .converge_from_data_sync("peer edit", Some(20))
            .expect("newer external write must yield a converge directive");
        assert_eq!(directive.target, "peer edit");
        assert_eq!(directive.seq, 20);
        assert_eq!(vm.last_local_seq(), 20);
    }

    /// The SQL-canonicalized echo of the editor's OWN in-flight write (same
    /// seq, trailing whitespace trimmed) adopts the baseline WITHOUT a
    /// directive — the typed buffer, trailing space and all, is kept.
    #[test]
    fn data_sync_own_canonicalized_echo_adopts_baseline_without_directive() {
        let mut vm = test_controller();
        // Type a trailing space through the buffer sink; records a fresh seq.
        vm.apply_local_edit("foo ").unwrap();
        let seq = vm.last_local_seq();
        assert!(
            vm.converge_from_data_sync("foo", Some(seq)).is_none(),
            "own canonicalized echo must not converge (would delete the space)"
        );
        assert_eq!(vm.buffer(), "foo ", "typed buffer kept as-is");
    }

    /// Arm (d): the editable surface IS the source projection, so the keystroke
    /// that makes the buffer keyword-headed keeps the keyword ON SCREEN and
    /// commits the whole raw text through the SOURCE channel. The store's parse
    /// — not the editor — decides what that text means.
    #[test]
    fn the_committing_space_commits_the_raw_source() {
        let mut vm = test_controller();
        // This document declares no `#+TODO:` line, so the parser's defaults ARE
        // its vocabulary — stated explicitly, because an editor that has not
        // resolved one classifies nothing (`Surface::Pending`).
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let seed = vm.project_authority("TODO", &[], Some("TODO"));
        vm.set_buffer_from_authority(&seed, 0);
        let intent = vm
            .apply_local_edit("TODO ")
            .expect("keystroke")
            .expect("a source commit");
        assert_eq!(intent.op_name, "set_field");
        assert_eq!(
            intent.params["field"],
            Value::String(holon_api::SOURCE_TEXT_FIELD.into())
        );
        assert_eq!(intent.params["value"], Value::String("TODO ".into()));
        assert_eq!(
            vm.buffer(),
            "TODO ",
            "the keyword stays in the editable surface — it is vault syntax, not a gesture"
        );
    }

    /// DELETING the keyword is the demotion gesture, and it must reach the only
    /// channel that can clear a task state. A buffer that merely STOPPED being
    /// keyword-headed still commits as source.
    #[test]
    fn deleting_the_keyword_still_takes_the_source_channel() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let seed = vm.project_authority("milk", &[], Some("TODO"));
        assert_eq!(seed, "TODO milk");
        vm.set_buffer_from_authority(&seed, 0);
        let intent = vm
            .apply_local_edit("milk")
            .expect("keystroke")
            .expect("a source commit");
        assert_eq!(
            intent.params["field"],
            Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
            "a content write would leave the block a task with the keyword gone from view"
        );
        assert_eq!(intent.params["value"], Value::String("milk".into()));
    }

    /// Ordinary prose never pays for the source channel: it commits `content`,
    /// which by contract never re-derives the task state (the #64 lock).
    #[test]
    fn ordinary_prose_commits_the_content_channel() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let seed = vm.project_authority("buy", &[], None);
        vm.set_buffer_from_authority(&seed, 0);
        let intent = vm
            .apply_local_edit("buy milk")
            .expect("keystroke")
            .expect("a content commit");
        assert_eq!(intent.params["field"], Value::String("content".into()));
    }

    /// The seed is the source projection, and a keyword the document does not
    /// declare is REFUSED rather than shown: projecting it would read back as
    /// prose and demote the task on the next commit.
    #[test]
    fn an_undeclared_keyword_is_not_projected_into_the_surface() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        assert_eq!(
            vm.project_authority("milk", &[], Some("TODO")),
            "milk",
            "an undeclared keyword must not reach the surface"
        );
        assert_eq!(
            vm.project_authority("milk", &[], Some("NEXT")),
            "NEXT milk",
            "a declared keyword projects to vault syntax"
        );
        assert_eq!(vm.project_authority("milk", &[], None), "milk");
    }

    /// THE VOCABULARY WINDOW, encoded — and made DETERMINISTIC by construction
    /// rather than by racing a DB round trip: "the vocabulary has not arrived"
    /// is simply "nobody called `set_task_vocabulary` yet", which is exactly
    /// the state a freshly mounted GPUI editor is in until its
    /// page-ancestor read returns.
    ///
    /// Classifying under the parser's DEFAULT vocabulary in that window is a
    /// fake-data bug: the defaults declare `TODO`, so a block the REAL
    /// vocabulary would refuse gets projected and pinned to the source channel,
    /// and the still-unclassified arm falls to the vocabulary-free shape rule.
    /// Both demote silently on the next keystroke.
    #[test]
    fn an_unresolved_vocabulary_never_classifies_the_surface() {
        // Arm A — a block the real vocabulary would REFUSE. Under the defaults
        // it projects to `TODO ASAP call Bob` and routes source.
        let mut vm = test_controller();
        let seed = vm.project_authority("ASAP call Bob", &[], Some("TODO"));
        assert_eq!(
            seed, "ASAP call Bob",
            "an unresolved vocabulary must not FABRICATE vault syntax — the content \
             column is the only thing known to be true here"
        );
        vm.set_buffer_from_authority(&seed, 0);
        let intent = vm
            .apply_local_edit("ASAP call Bob!")
            .expect("keystroke")
            .expect("a commit");
        assert_eq!(
            intent.params["field"],
            Value::String("content".into()),
            "arm A: a keystroke inside the vocabulary window must take the channel that \
             cannot touch a task state"
        );

        // Arm B — an UNTASKED block whose text merely has keyword SHAPE. The
        // shape rule is vocabulary-free, so it may not be consulted before the
        // vocabulary that would adjudicate it has arrived.
        let mut untasked = test_controller();
        let seed = untasked.project_authority("call Bob", &[], None);
        untasked.set_buffer_from_authority(&seed, 0);
        let intent = untasked
            .apply_local_edit("TODO call Bob")
            .expect("keystroke")
            .expect("a commit");
        assert_eq!(
            intent.params["field"],
            Value::String("content".into()),
            "arm B: the router must not be less vocabulary-aware than the parse it feeds"
        );
    }

    /// The window CLOSES: once the real vocabulary arrives and the surface is
    /// re-projected, both arms classify and route as they should. Without this
    /// the fix above could pass by pinning every editor to the content channel
    /// forever, which would silently disable the whole feature.
    #[test]
    fn the_resolved_vocabulary_classifies_and_reopens_the_source_channel() {
        let declared = holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        );

        // Refused stays on content — for the right reason now, not by accident.
        let mut refused = test_controller();
        refused.set_task_vocabulary(declared.clone());
        let seed = refused.project_authority("ASAP call Bob", &[], Some("TODO"));
        refused.set_buffer_from_authority(&seed, 0);
        assert_eq!(
            refused
                .apply_local_edit("ASAP call Bob!")
                .unwrap()
                .unwrap()
                .params["field"],
            Value::String("content".into())
        );

        // A DECLARED keyword projects and routes source.
        let mut projected = test_controller();
        projected.set_task_vocabulary(declared.clone());
        let seed = projected.project_authority("call bank", &[], Some("NEXT"));
        assert_eq!(seed, "NEXT call bank");
        projected.set_buffer_from_authority(&seed, 0);
        assert_eq!(
            projected
                .apply_local_edit("NEXT call banks")
                .unwrap()
                .unwrap()
                .params["field"],
            Value::String(holon_api::SOURCE_TEXT_FIELD.into())
        );

        // And an untasked block can still be PROMOTED by typing, which is the
        // feature this must not disable.
        let mut untasked = test_controller();
        untasked.set_task_vocabulary(declared);
        let seed = untasked.project_authority("call bank", &[], None);
        untasked.set_buffer_from_authority(&seed, 0);
        assert_eq!(
            untasked
                .apply_local_edit("NEXT call bank")
                .unwrap()
                .unwrap()
                .params["field"],
            Value::String(holon_api::SOURCE_TEXT_FIELD.into())
        );
    }

    /// TASK #93. A structural op reports its caret in CONTENT coordinates
    /// (`join_block` returns the merge boundary, `split_block` returns 0), but
    /// the buffer that consumes the seed is the SURFACE. On a merge target that
    /// is a task the seed therefore lands `keyword.len() + 1` bytes short —
    /// INSIDE the keyword — and the next keystroke corrupts it (`TODOX
    /// milkbread`), which the store then reads as naming no keyword at all: the
    /// same silent-demotion class as the vocabulary hole.
    #[test]
    fn a_caret_seed_in_content_coordinates_crosses_the_keyword_prefix() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let seed = vm.project_authority("milk", &[], Some("TODO"));
        assert_eq!(seed, "TODO milk");
        vm.set_buffer_from_authority(&seed, 0);

        // `join_block` reports the merge boundary as an offset into `milk`.
        assert_eq!(
            vm.content_offset_to_surface(4).unwrap(),
            9,
            "end-of-content is end-of-surface, not four bytes into the keyword"
        );
        assert_eq!(
            vm.content_offset_to_surface(0).unwrap(),
            5,
            "offset 0 is the START OF THE CONTENT, which sits after the keyword"
        );
    }

    /// THE DOGFOOD ESCAPE (2026-08-18,
    /// `2026-08-18-split-position-measured-on-the-editor-surface`). The user
    /// typed `~installation_source~`; the store kept the stripped label plus a
    /// `Code` mark, so the caret parked at end of line was two bytes — the two
    /// `~` — past the content `split_block` cuts. Enter answered "Split
    /// position 68 exceeds content length 66" and did nothing.
    #[test]
    fn a_caret_after_typed_inline_markup_lands_on_the_stripped_content() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let surface = "Add column ~installation_source~ which can contain Ansible reference";
        let seed = vm.project_authority(surface, &[], None);
        vm.set_buffer_from_authority(&seed, 0);

        assert_eq!(surface.len(), 68);
        let caret = vm.structural_caret(surface, surface.len()).unwrap();
        assert_eq!(caret.surface_byte(), 68);
        assert_eq!(
            caret.content_byte(),
            66,
            "the two `~` delimiters are surface, not content"
        );
    }

    /// The two coordinates a structural key needs stay distinct: on a task the
    /// surface byte right after the keyword is content byte 0, and Backspace
    /// there must delete a space rather than join the block away.
    #[test]
    fn the_structural_caret_keeps_both_coordinates() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let seed = vm.project_authority("milk", &[], Some("TODO"));
        assert_eq!(seed, "TODO milk");
        vm.set_buffer_from_authority(&seed, 0);

        let end = vm.structural_caret(&seed, seed.len()).unwrap();
        assert_eq!((end.surface_byte(), end.content_byte()), (9, 4));
        let after_keyword = vm.structural_caret(&seed, 5).unwrap();
        assert_eq!(
            (after_keyword.surface_byte(), after_keyword.content_byte()),
            (5, 0)
        );

        assert_eq!(
            structural_block_action(EditorKey::Enter, "block:t", end)
                .unwrap()
                .params["position"],
            Value::Integer(4),
            "Enter at end of `TODO milk` splits at the end of `milk`, not four bytes past it"
        );
        assert!(
            structural_block_action(EditorKey::Backspace, "block:t", after_keyword).is_none(),
            "Backspace after the keyword deletes a character; only surface byte 0 joins"
        );
    }

    /// A caret that does not land on the surface it was measured on is a
    /// routing bug and must be an Err naming both numbers — never a clamp that
    /// silently splits somewhere else.
    #[test]
    fn a_caret_past_the_surface_is_refused() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let seed = vm.project_authority("milk", &[], None);
        vm.set_buffer_from_authority(&seed, 0);

        let err = vm.structural_caret(&seed, 99).unwrap_err().to_string();
        assert!(err.contains("99") && err.contains('4'), "{err}");
    }

    /// The mapping is a no-op wherever the surface is not a projection — a
    /// plain block, and a REFUSED one (whose surface IS the content column).
    #[test]
    fn a_caret_seed_is_untouched_when_the_surface_shows_the_content_column() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::default());
        let seed = vm.project_authority("milk", &[], None);
        vm.set_buffer_from_authority(&seed, 0);
        assert_eq!(vm.content_offset_to_surface(2).unwrap(), 2);

        let mut refused = test_controller();
        refused.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = refused.project_authority("API rewrite", &[], Some("TODO"));
        refused.set_buffer_from_authority(&seed, 0);
        assert_eq!(
            refused.content_offset_to_surface(3).unwrap(),
            3,
            "a refused surface shows the content column, so the two coordinate \
             spaces are the same one"
        );
    }

    /// THE REFUTED CLAIM, encoded. The seed refusal is judged under the
    /// DOCUMENT's vocabulary; the commit router was judged by the
    /// vocabulary-FREE shape rule. A refused block whose stored CONTENT happens
    /// to start with an uppercase token was therefore admitted to the source
    /// channel, where the store found no declared keyword and cleared
    /// `task_state` — silently, because the only WARN fires at the seed.
    ///
    /// Reachable by adding a `#+TODO:` line to a page whose blocks were already
    /// marked TODO under the defaults: every one of them is refused, and any
    /// whose text starts with `API`/`PR`/`ASAP`/… loses its task on the next
    /// keystroke.
    #[test]
    fn a_refused_surface_never_commits_through_the_source_channel() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string(), "WAITING".to_string()],
            &["DONE".to_string()],
        ));
        // The block carries TODO, which THIS document does not declare, so the
        // projection is refused and the surface shows stored content.
        let seed = vm.project_authority("ASAP call Bob", &[], Some("TODO"));
        assert_eq!(seed, "ASAP call Bob", "precondition: the seed was refused");
        vm.set_buffer_from_authority(&seed, 0);

        let intent = vm
            .apply_local_edit("ASAP call Bob!")
            .expect("keystroke")
            .expect("a commit");
        assert_eq!(
            intent.params["field"],
            Value::String("content".into()),
            "a surface that could not show the keyword must not be allowed to REMOVE it — \
             the source channel would re-derive `task_state` from text that never carried it"
        );
    }

    /// The refusal is pinned for the whole session, not just the first
    /// keystroke: the block is still refused after any number of edits, so
    /// every one of them stays on the content channel.
    #[test]
    fn the_refusal_pins_the_channel_for_the_whole_session() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("API rewrite", &[], Some("TODO"));
        vm.set_buffer_from_authority(&seed, 0);
        for typed in ["API rewrite ", "API rewrite n", "NEXT rewrite"] {
            let intent = vm
                .apply_local_edit(typed)
                .expect("keystroke")
                .expect("a commit");
            assert_eq!(
                intent.params["field"],
                Value::String("content".into()),
                "keystroke {typed:?} escaped the pinned content channel"
            );
        }
    }

    /// The pin is NOT a blanket disable, and the guard cannot pass by refusing
    /// everything: a block whose keyword the document DOES declare projects,
    /// and its surface commits as SOURCE — including the demoting edit that
    /// deletes the keyword out of it.
    #[test]
    fn a_projected_surface_still_commits_as_source() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("call bank", &[], Some("NEXT"));
        assert_eq!(seed, "NEXT call bank", "precondition: the seed projected");
        vm.set_buffer_from_authority(&seed, 0);

        for typed in ["NEXT call banks", "call banks"] {
            let intent = vm
                .apply_local_edit(typed)
                .expect("keystroke")
                .expect("a commit");
            assert_eq!(
                intent.params["field"],
                Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
                "keystroke {typed:?} must re-derive both columns — the second one is the \
                 demotion gesture, which only this channel can perform"
            );
        }
    }

    /// The BLUR funnel takes the same channel as the keystroke funnel. It
    /// builds its `set_field` from the editable node's own field (`content`),
    /// so without the routing it commits the SURFACE — vault syntax — into the
    /// content column and folds the keyword into the title (task #99).
    #[test]
    fn a_blur_commits_a_projected_surface_as_source() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("call bank", &[], Some("NEXT"));
        vm.set_buffer_from_authority(&seed, 0);

        let intent = vm
            .pending_commit_intent("NEXT call banks")
            .expect("a changed buffer must commit on blur");
        assert_eq!(
            intent.params["field"],
            Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
            "the blur commit carries the surface, so the store must re-derive both columns"
        );
    }

    /// And the ruled `Refused` session-pin survives the routing, because it is
    /// the same `commits_as_source` match: a surface that could not SHOW the
    /// keyword must not be able to remove it on the way out either.
    #[test]
    fn a_blur_keeps_a_refused_surface_on_the_content_channel() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("API rewrite", &[], Some("TODO"));
        vm.set_buffer_from_authority(&seed, 0);

        let intent = vm
            .pending_commit_intent("NEXT rewrite")
            .expect("a changed buffer must commit on blur");
        assert_eq!(
            intent.params["field"],
            Value::String("content".into()),
            "the blur escaped the pinned content channel"
        );
    }

    /// Task #94: the structural-chord funnel must not re-commit text the
    /// keystroke sink already dispatched. A focused editor receives no
    /// data-sync echo, so after a non-editor origin splits its row the buffer
    /// is stale — and this re-commit would land as a revert that resurrects the
    /// pre-split text beside the split's surviving tail.
    #[test]
    fn a_chord_does_not_re_commit_what_the_keystroke_sink_already_wrote() {
        let mut vm = test_controller();
        vm.set_buffer_from_authority("alpha two", 0);
        vm.apply_local_edit("alpha twoZZZZ")
            .expect("the keystroke sink accepts the edit")
            .expect("SqlOnly dispatches the typed text itself");

        assert!(
            vm.chord_commit_intent("alpha twoZZZZ").is_none(),
            "the chord flushed a buffer the keystroke sink had already committed"
        );
    }

    /// The same guard on a PROJECTED (tasked) surface, so the #99 fold cannot
    /// re-enter through this third funnel: nothing is re-committed at all, and
    /// when the funnel legitimately fires (see the blur tests above) it routes
    /// through `commits_as_source`.
    #[test]
    fn a_chord_on_a_tasked_row_re_commits_nothing_after_typing() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("call bank", &[], Some("NEXT"));
        vm.set_buffer_from_authority(&seed, 0);
        let typed = format!("{seed}s");
        vm.apply_local_edit(&typed)
            .expect("the keystroke sink accepts the edit")
            .expect("a projected surface commits through the source channel");

        assert!(
            vm.chord_commit_intent(&typed).is_none(),
            "the chord re-committed an already-dispatched tasked surface"
        );
    }

    /// The chord funnel takes the SAME channel decision as the blur funnel it
    /// delegates to. Without this the #99 fold re-enters through the third
    /// commit funnel: a chord flush of a PROJECTED surface would commit vault
    /// syntax into the content column.
    #[test]
    fn a_chord_flush_of_a_projected_surface_takes_the_source_channel() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("call bank", &[], Some("NEXT"));
        vm.set_buffer_from_authority(&seed, 0);

        let intent = vm
            .chord_commit_intent("NEXT call banks")
            .expect("text the sink never saw must flush");
        assert_eq!(
            intent.params["field"],
            Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
            "the chord flush carries the surface, so the store must re-derive both columns"
        );
    }

    /// And the ruled `Refused` session-pin survives the chord funnel too: a
    /// surface that could not SHOW the keyword must not be able to remove it.
    #[test]
    fn a_chord_flush_of_a_refused_surface_stays_on_the_content_channel() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("API rewrite", &[], Some("TODO"));
        vm.set_buffer_from_authority(&seed, 0);

        let intent = vm
            .chord_commit_intent("NEXT rewrite")
            .expect("text the sink never saw must flush");
        assert_eq!(
            intent.params["field"],
            Value::String("content".into()),
            "the chord flush escaped the pinned content channel"
        );
    }

    /// The funnel still exists for text the sink never saw — an IME
    /// composition or a programmatic `set_value` moves the visible field
    /// without reaching `apply_local_edit`, and that text must not be lost to
    /// the structural op.
    #[test]
    fn a_chord_still_flushes_text_the_keystroke_sink_never_saw() {
        let mut vm = test_controller();
        vm.set_buffer_from_authority("alpha two", 0);

        let intent = vm
            .chord_commit_intent("alpha two composed")
            .expect("text the sink never saw is genuinely pending and must be flushed");
        assert_eq!(
            intent.params["value"],
            Value::String("alpha two composed".into())
        );
    }

    /// An UNTASKED block whose text merely has the SHAPE of a keyword the
    /// document does not declare must not gain a blank `task_state`. The store
    /// is what guarantees this (it skips the task-state constituent when there
    /// is nothing to clear); the routing here is what puts it in front of that
    /// guarantee, so both halves are pinned — see
    /// `a_source_write_that_declares_nothing_leaves_a_plain_block_plain` in
    /// `crates/holon/tests/promote_task_keyword_compound.rs`.
    #[test]
    fn an_untasked_block_with_an_undeclared_uppercase_token_still_commits() {
        let mut vm = test_controller();
        vm.set_task_vocabulary(holon_org_format::TaskKeywordVocabulary::for_document(
            &["NEXT".to_string()],
            &["DONE".to_string()],
        ));
        let seed = vm.project_authority("ASAP call Bob", &[], None);
        assert_eq!(seed, "ASAP call Bob");
        vm.set_buffer_from_authority(&seed, 0);

        let intent = vm
            .apply_local_edit("ASAP call Bob!")
            .expect("keystroke")
            .expect("a commit");
        assert_eq!(
            intent.params["value"],
            Value::String("ASAP call Bob!".into()),
            "the keystroke is never lost, whichever channel carries it"
        );
    }

    /// Amendment 3: a directive deferred during an IME composition replays
    /// until a newer local write supersedes it, and a newer deferred
    /// directive supersedes an older pending one.
    #[test]
    fn pending_directive_supersede_and_stale_discard() {
        let mut vm = test_controller();

        // Newer pending directive supersedes an older one.
        vm.set_pending_directive(ConvergeDirective {
            target: "old".into(),
            seq: 2,
        });
        vm.set_pending_directive(ConvergeDirective {
            target: "new".into(),
            seq: 7,
        });
        let taken = vm
            .take_pending_directive()
            .expect("newest directive replays");
        assert_eq!(taken.target, "new");
        assert!(vm.take_pending_directive().is_none(), "cleared after take");

        // A directive whose seq is behind a newer local write is discarded.
        vm.set_pending_directive(ConvergeDirective {
            target: "stale".into(),
            seq: 5,
        });
        vm.advance_local_seq(9);
        assert!(
            vm.take_pending_directive().is_none(),
            "a directive superseded by a newer local write is discarded on replay"
        );
    }
}
