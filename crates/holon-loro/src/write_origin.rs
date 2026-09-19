//! Which seam committed a batch into a Loro document.
//!
//! Loro stamps every commit with a free-form origin string and hands it to
//! subscribers. Holon reads that string to decide two things: whether the
//! editor should converge to a write (it must not converge to its own echo),
//! and — from the cell-undo lane onwards — whether a text-undo manager may
//! take the write back.
//!
//! # Seam, not actor
//!
//! This is deliberately NOT [`holon_api::OpOrigin`]. `OpOrigin` names who
//! caused an operation (a person, a rule, an agent, ingest); a Loro origin
//! names which code path committed. One seam serves several actors: every
//! block operation, whether a person moved a block or an org file was
//! ingested, reaches Loro through [`BlockOps`](WriteOrigin::BlockOps). Asking
//! the seam to name the actor would need `OpOrigin` threaded through the whole
//! `BlockOperations` surface, and would make the CRDT a second home for
//! provenance the undo journal already records.
//!
//! What the seam CAN answer is the question undo asks: is this the user's own
//! keystroke? Exactly one variant says yes.

use std::borrow::Cow;

/// The seam that produced a Loro commit.
///
/// [`Self::as_origin`] is the single boundary where a variant becomes the
/// string Loro carries. Every non-keystroke origin is prefixed with
/// [`Self::SYSTEM_PREFIX`] there, so excluding non-user writes from an undo
/// manager is one `add_exclude_origin_prefix` call and a seam added later is
/// excluded unless it deliberately claims to be a keystroke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOrigin {
    /// The user's own keystroke in an editor cell
    /// (`LoroTextCellBacking::apply_text_op`). The only origin an undo manager
    /// may take back, and the only one the editor's subscribe filter
    /// suppresses — the editor already holds the value it just typed.
    UiEditorKeystroke,
    /// An absolute value set through a cell (`CellBacking::apply_replace`)
    /// rather than typed. Authoritative for the editor, which must converge to
    /// it.
    UiValueSet,
    /// A block operation through `LoroBackend` — create, move, delete, field
    /// and property writes. Serves the dispatcher, org ingest and rule firings
    /// alike; the actor is recorded by the undo journal, not here.
    BlockOps,
    /// First-boot tree/container creation.
    SchemaInit,
    /// A local update re-applied onto our own document.
    Reconcile,
    /// A delta imported from a peer replica.
    SyncImport,
    /// Sharing lifecycle: share, accept, or unshare a subtree.
    ShareLifecycle,
    /// Device pairing: adopting a staged document, wiping the tree.
    DevicePairing,
    /// The text-undo manager taking a step back or forward.
    ///
    /// System-prefixed, so the manager does not re-record its own undo as a
    /// fresh undoable action. Redo is unaffected: Loro moves the step onto the
    /// redo stack internally rather than through the origin-recording path
    /// (pinned by `an_undo_is_not_recorded_as_a_new_undo_step`).
    UiUndo,
    /// Building the text-undo manager.
    ///
    /// Registers subscriptions and commits nothing. It takes the write scope
    /// only for mutual exclusion: Loro panics if a subscriber is registered
    /// while the document is emitting, and emission happens only inside a
    /// write scope.
    UndoArm,
    /// The flush a snapshot export performs before reading the frontier.
    ///
    /// It flushes whatever was pending, which by the write-scope contract is
    /// nothing. The label exists so that if a stray batch ever DOES reach it,
    /// those ops land excluded from undo rather than under the empty origin.
    SnapshotFlush,
    /// Carrying the document back to the version a failed multi-op batch
    /// started from.
    BatchRollback,
    /// A test or probe write. Production code never produces this; the label
    /// is what the probe calls itself, so a stray origin in a log names its
    /// test.
    Probe(&'static str),
}

impl WriteOrigin {
    /// The prefix every origin carries except [`Self::UiEditorKeystroke`].
    pub const SYSTEM_PREFIX: &'static str = "sys.";

    /// The string Loro stamps on the commit.
    pub fn as_origin(&self) -> Cow<'static, str> {
        match self {
            // Load-bearing spelling: the editor's subscribe filter compares
            // against it, and changing it would silently re-deliver every
            // keystroke to the editor that typed it.
            Self::UiEditorKeystroke => Cow::Borrowed("ui_editor_echo"),
            Self::Probe(label) => Cow::Owned(format!("{}probe.{label}", Self::SYSTEM_PREFIX)),
            other => Cow::Owned(format!("{}{}", Self::SYSTEM_PREFIX, other.seam_tag())),
        }
    }

    /// Whether a text-undo manager may take this write back.
    pub fn is_user_text(&self) -> bool {
        matches!(self, Self::UiEditorKeystroke)
    }

    fn seam_tag(&self) -> &'static str {
        match self {
            Self::UiEditorKeystroke => "ui_editor_keystroke",
            Self::UiValueSet => "ui_value_set",
            Self::UiUndo => "ui_undo",
            Self::SnapshotFlush => "snapshot_flush",
            Self::UndoArm => "undo_arm",
            Self::BlockOps => "block_ops",
            Self::SchemaInit => "schema_init",
            Self::Reconcile => "reconcile",
            Self::SyncImport => "sync_import",
            Self::ShareLifecycle => "share_lifecycle",
            Self::DevicePairing => "device_pairing",
            Self::BatchRollback => "batch_rollback",
            Self::Probe(label) => label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_origin_escapes_the_system_prefix() {
        let every = [
            WriteOrigin::UiEditorKeystroke,
            WriteOrigin::UiValueSet,
            WriteOrigin::UiUndo,
            WriteOrigin::SnapshotFlush,
            WriteOrigin::UndoArm,
            WriteOrigin::BlockOps,
            WriteOrigin::SchemaInit,
            WriteOrigin::Reconcile,
            WriteOrigin::SyncImport,
            WriteOrigin::ShareLifecycle,
            WriteOrigin::DevicePairing,
            WriteOrigin::BatchRollback,
            WriteOrigin::Probe("some_test"),
        ];
        let undoable: Vec<_> = every
            .iter()
            .filter(|o| !o.as_origin().starts_with(WriteOrigin::SYSTEM_PREFIX))
            .collect();
        assert_eq!(undoable, vec![&WriteOrigin::UiEditorKeystroke]);
        assert!(every.iter().all(|o| o.is_user_text() == !o
            .as_origin()
            .starts_with(WriteOrigin::SYSTEM_PREFIX)));
    }

    #[test]
    fn distinct_seams_get_distinct_origins() {
        let every = [
            WriteOrigin::UiEditorKeystroke,
            WriteOrigin::UiValueSet,
            WriteOrigin::UiUndo,
            WriteOrigin::SnapshotFlush,
            WriteOrigin::UndoArm,
            WriteOrigin::BlockOps,
            WriteOrigin::SchemaInit,
            WriteOrigin::Reconcile,
            WriteOrigin::SyncImport,
            WriteOrigin::ShareLifecycle,
            WriteOrigin::DevicePairing,
            WriteOrigin::BatchRollback,
        ];
        let mut strings: Vec<String> = every.iter().map(|o| o.as_origin().to_string()).collect();
        strings.sort();
        let count = strings.len();
        strings.dedup();
        assert_eq!(
            strings.len(),
            count,
            "two seams share an origin: {strings:?}"
        );
    }
}
