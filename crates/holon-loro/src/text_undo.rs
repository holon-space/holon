//! The vault document's text-undo manager.
//!
//! Loro's `UndoManager` is the only thing that can take back the characters
//! THIS peer typed while leaving a concurrent peer's characters in place: it
//! transforms its undo spans against remote changes, which a value-replaying
//! inverse journal cannot do. So it owns text, and the operation journal keeps
//! owning everything else (D115.A, Option A).
//!
//! Nothing here is wired to cmd-z yet. This increment installs the manager and
//! proves what it records; the journal delegation is increment 2.

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use loro::PeerID;
use parking_lot::Mutex;

use crate::loro_document::LoroDocument;
use crate::write_origin::WriteOrigin;

/// How much typing one undo step takes back.
///
/// This is the ONLY thing that closes an undo group. Loro decides merging at
/// write time from the elapsed interval alone (`loro-internal/src/undo.rs`
/// `in_merge_interval`), and its `record_new_checkpoint` does not reset that
/// clock — measured in `tests/text_undo_contract.rs`. So there is no way to
/// force a split, and the journal never assumes one: it derives its marker
/// count from the manager's own step count.
///
/// Loro merges consecutive local edits that land within this window into one
/// undo group. Zero would make every keystroke its own step, which is not what
/// a person means by cmd-z; 500 ms takes back a burst of typing (design
/// question 2).
pub const MERGE_INTERVAL_MS: i64 = 500;

/// The peer id Loro is ACTUALLY writing under.
///
/// Not [`LoroDocument::peer_id`], which returns a value cached on the wrapper
/// when it was built: a write straight to the inner doc moves Loro's id without
/// touching that cache, and that is precisely the path the guard exists to
/// catch. Reading the live value is what makes the check able to fail.
fn live_peer_id(doc: &LoroDocument) -> PeerID {
    // ALLOW(loro_doc_escape): reads an identity field, not tree or text state,
    // so it observes no write batch's interior.
    doc.doc().peer_id()
}

/// A Loro `UndoManager` bound to one document, with the invariants Holon needs
/// around it.
pub struct TextUndo {
    /// `&mut self` on every mutating call, so the manager needs a lock of its
    /// own even though the document has one.
    inner: Mutex<loro::UndoManager>,
    doc: Arc<LoroDocument>,
    /// The peer id the manager was built under. See [`Self::check_peer`].
    peer_at_install: PeerID,
}

impl TextUndo {
    /// Build the manager over `doc`.
    ///
    /// Two policies are set here and nowhere else:
    ///
    /// - **One exclusion.** Everything that is not the user's own keystroke
    ///   carries [`WriteOrigin::SYSTEM_PREFIX`], so a single
    ///   `add_exclude_origin_prefix` keeps org ingest, dispatcher operations,
    ///   sharing, pairing and peer merges out of the stack. A seam added later
    ///   is excluded unless it deliberately claims to be a keystroke.
    /// - **A merge interval**, so one cmd-z is a burst of typing.
    pub fn install(doc: Arc<LoroDocument>) -> Self {
        let peer_at_install = live_peer_id(&doc);
        // ALLOW(loro_doc_escape): the manager is a long-lived observer that
        // registers subscriptions on the doc, the blessed transport shape.
        let raw = doc.doc();
        let mut inner = loro::UndoManager::new(&raw);
        inner.add_exclude_origin_prefix(WriteOrigin::SYSTEM_PREFIX);
        inner.set_merge_interval(MERGE_INTERVAL_MS);
        // The fork defaults to `usize::MAX`. Unbounded, the manager outgrows
        // the journal's marker count and the two stacks drift; capped, both
        // sides evict their oldest by the same rule (`pop_front` in
        // `loro-internal/src/undo.rs:595`, `drop_oldest_text_epoch` here).
        inner.set_max_undo_steps(holon_core::TEXT_UNDO_MAX_GROUPS);
        Self {
            inner: Mutex::new(inner),
            doc,
            peer_at_install,
        }
    }

    /// Refuse loudly if the document's peer id moved.
    ///
    /// Measured against the pinned fork: `UndoManager` subscribes to peer-id
    /// changes and CLEARS both stacks when one arrives, with no signal to the
    /// caller. Silently reporting "nothing to undo" after the user's history
    /// evaporated is the one outcome the error-handling policy forbids, so the
    /// mismatch becomes an error instead. No production path changes the vault
    /// document's peer id after boot; this exists so that a future one fails
    /// visibly rather than eating the history.
    fn check_peer(&self) -> Result<()> {
        let now = live_peer_id(&self.doc);
        anyhow::ensure!(
            now == self.peer_at_install,
            "the vault document's peer id changed from {} to {} while the text-undo manager was \
             alive; Loro cleared the undo and redo stacks when that happened, so the user's typing \
             history is gone rather than merely unavailable",
            self.peer_at_install,
            now
        );
        Ok(())
    }

    pub fn can_undo(&self) -> Result<bool> {
        self.check_peer()?;
        Ok(self.inner.lock().can_undo())
    }

    pub fn can_redo(&self) -> Result<bool> {
        self.check_peer()?;
        Ok(self.inner.lock().can_redo())
    }

    /// How many undo steps the manager holds. The journal's text-epoch markers
    /// must stay 1:1 with this (increment 2).
    pub fn undo_count(&self) -> Result<usize> {
        self.check_peer()?;
        Ok(self.inner.lock().undo_count())
    }

    /// Take back one undo step. Returns whether anything was undone.
    ///
    /// The manager writes and commits, so it runs inside the document's write
    /// scope under [`WriteOrigin::UiUndo`] — otherwise the undo's own ops would
    /// commit under whatever Loro happened to have armed.
    pub fn undo(&self) -> Result<bool> {
        self.check_peer()?;
        self.doc.with_write(WriteOrigin::UiUndo, |_txn| {
            self.inner.lock().undo().context("text undo")
        })
    }

    /// Reapply one undone step. Returns whether anything was redone.
    pub fn redo(&self) -> Result<bool> {
        self.check_peer()?;
        self.doc.with_write(WriteOrigin::UiUndo, |_txn| {
            self.inner.lock().redo().context("text redo")
        })
    }
}

impl holon_core::TextUndoDelegate for TextUndo {
    fn undo_depth(&self) -> Result<usize> {
        self.undo_count()
    }

    fn undo_one(&self) -> Result<bool> {
        self.undo()
    }

    fn redo_one(&self) -> Result<bool> {
        self.redo()
    }
}

impl std::fmt::Debug for TextUndo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextUndo")
            .field("doc_id", &self.doc.doc_id())
            .field("peer_at_install", &self.peer_at_install)
            .finish()
    }
}

/// The engine's handle on a manager that may not be armed yet.
///
/// The journal asks for the text side on every undo and every journalled
/// operation, but the manager is only built when an editor cell goes live. An
/// unarmed session simply has no text steps — depth 0, nothing to undo — which
/// is exactly true: no editor has been opened, so nothing was typed.
pub struct LazyTextUndo {
    store: crate::loro_document_store::LoroDocumentStore,
}

impl LazyTextUndo {
    pub fn new(store: crate::loro_document_store::LoroDocumentStore) -> Self {
        Self { store }
    }

    /// The manager if it is armed. Never arms it: arming is the editor's
    /// signal, and doing it here would put the subscriber back on the ingest
    /// path through the journal's own bookkeeping.
    fn armed(&self) -> Option<Arc<TextUndo>> {
        self.store.text_undo()
    }
}

impl holon_core::TextUndoDelegate for LazyTextUndo {
    fn undo_depth(&self) -> Result<usize> {
        match self.armed() {
            Some(undo) => undo.undo_count(),
            None => Ok(0),
        }
    }

    fn undo_one(&self) -> Result<bool> {
        match self.armed() {
            Some(undo) => undo.undo(),
            None => Ok(false),
        }
    }

    fn redo_one(&self) -> Result<bool> {
        match self.armed() {
            Some(undo) => undo.redo(),
            None => Ok(false),
        }
    }
}
