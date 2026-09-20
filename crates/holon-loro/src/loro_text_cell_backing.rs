//! [`TextCellBacking`] backed by a Loro `LoroText` container.
//!
//! Direct port of the previous `MutableTextInner` (`mutable_text.rs`)
//! adapted to the [`CellBacking`] / [`TextCellBacking`] traits. Reads the
//! current string from the `LoroText`, applies local edits as Loro
//! `insert`/`delete` ops under a [`WriteOrigin`] so the outbound projector can
//! distinguish self-originated writes, and re-publishes peer-originated deltas
//! through a broadcast channel.

use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use holon_core::cell::CellBacking;
use holon_core::cell::CursorAnchor;
use holon_core::cell::CursorBias;
use holon_core::cell::DeltaOp;
use holon_core::cell::TextCellBacking;
use holon_core::cell::TextDelta;
use holon_core::cell::TextOp;
use loro::ContainerID;
use loro::ContainerTrait;
use loro::LoroDoc;
use loro::LoroText;
use loro::cursor::Cursor as LoroCursor;
use loro::event::Diff;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::doc_lock::DocLock;
use crate::doc_lock::mutate_guarded;
use crate::write_origin::WriteOrigin;

/// Commit origin stamped on the editor's *own keystroke* writes
/// (`apply_text_op`), i.e. [`WriteOrigin::UiEditorKeystroke`]. It is the
/// **only** origin the subscribe filter suppresses: the editor already holds
/// the value it just typed, and re-delivering its own echo would yank the caret
/// to end via the absolute `set_value` convergence path
/// (`editor_view::converge_input`).
///
/// Every *other* writer — structural ops / `set_field` (which reach Loro via
/// `update_block_text` → [`WriteOrigin::BlockOps`]), `apply_replace`
/// ([`WriteOrigin::UiValueSet`]), and remote peer imports — is an
/// **authoritative** write
/// the editor must converge to, so those events pass the filter. Authority
/// rationale: `docs/Architecture/UI.md` §"Field authority and intent capture".
///
/// CROSS-FRONTEND COUPLING: `apply_text_op` is called by every keystroke path
/// (gpui, TUI, headless mirror). Stamping this origin is safe only because
/// none of those consumers observe their *own* keystrokes via the
/// `remote_deltas`/`signal` stream (TUI's stream loop only re-reads
/// `current()`; headless never subscribes; gpui suppresses its own echo by
/// design). Any future consumer that needs to *see* a local editor's
/// keystrokes on the stream would silently miss them.
pub(crate) fn editor_echo_origin() -> std::borrow::Cow<'static, str> {
    WriteOrigin::UiEditorKeystroke.as_origin()
}

pub struct LoroTextCellBacking {
    doc: Arc<LoroDoc>,
    text: LoroText,
    #[allow(dead_code)]
    text_id: ContainerID,
    /// The document's write lock, shared with every `LoroDocument` over the
    /// same `Arc`. The pending loro transaction is per-document: a commit
    /// taken without it flushes another writer's pending ops too, and the
    /// merged commit reaches subscribers under one origin — misattributing
    /// somebody else's write to the editor.
    lock: DocLock,
    /// Diagnostic label for [`DocLock`]'s timeout and upgrade errors.
    lock_label: String,
    /// Where a lost keystroke is disclosed. A caller that passes `None` gets
    /// the `Err` and owes its user the disclosure itself.
    bus: Option<Arc<holon_api::ConditionBus>>,
    /// The block whose text this cell holds, named as the condition's subject.
    subject: String,
    remote_tx: broadcast::Sender<TextDelta>,
    #[allow(dead_code)]
    subscription: loro::Subscription,
}

impl LoroTextCellBacking {
    /// Wrap an existing `LoroText` container into a text-rich cell
    /// backing. Subscribes to the Loro doc on construction so peer-
    /// originated deltas land on the broadcast channel.
    pub fn new(doc: Arc<LoroDoc>, text: LoroText) -> Result<Self> {
        Self::disclosing(doc, text, None, String::new())
    }

    /// [`Self::new`] wired to the condition bus, so a keystroke the write lock
    /// refuses is disclosed rather than only returned.
    pub fn disclosing(
        doc: Arc<LoroDoc>,
        text: LoroText,
        bus: Option<Arc<holon_api::ConditionBus>>,
        subject: String,
    ) -> Result<Self> {
        let text_id = text.id();
        let (remote_tx, _) = broadcast::channel(256);
        let tx_for_cb = remote_tx.clone();
        let target_id = text_id.clone();

        let subscription = doc.subscribe(
            &text_id,
            Arc::new(move |event| {
                // Filter A: suppress ONLY the editor's own keystroke echo.
                // Every other writer (structural `with_write`/`set_field`,
                // `apply_replace`, peer imports) is authoritative and must
                // reach the editor so it converges. See `editor_echo_origin`.
                if *event.origin == *editor_echo_origin() {
                    return;
                }
                // Filter B: only this container.
                for diff in &event.events {
                    if diff.target.clone() != target_id {
                        continue;
                    }
                    if let Diff::Text(text_deltas) = &diff.diff {
                        let translated = translate_text_delta(text_deltas);
                        let _ = tx_for_cb.send(translated);
                    }
                }
            }),
        );

        let lock = DocLock::for_doc(&doc);
        let lock_label = format!("text cell {text_id:?}");
        Ok(Self {
            doc,
            text,
            text_id,
            lock,
            lock_label,
            bus,
            subject,
            remote_tx,
            subscription,
        })
    }
}

/// A write the doc lock refused means the keystroke never reached the store
/// while the editor kept the text — the one failure mode this project ranks
/// last if it stays silent. Disclose it, then hand the error on.
fn disclose_refused_write<R>(
    bus: &Option<Arc<holon_api::ConditionBus>>,
    subject: &str,
    outcome: Result<R>,
) -> Result<R> {
    outcome.inspect_err(|e| {
        if let Some(bus) = bus {
            bus.emit(holon_api::Condition {
                subject: subject.to_string(),
                reason: holon_api::ConditionKind::LocalEditNotApplied {
                    detail: e.to_string(),
                },
            });
        }
    })
}

impl LoroTextCellBacking {
    fn write_disclosing<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        disclose_refused_write(
            &self.bus,
            &self.subject,
            self.lock.write(&self.lock_label, f),
        )
    }
}

/// The reads below take no doc lock, by choice.
///
/// They run on the frontend's render and caret paths, where a blocking
/// acquire would put a snapshot save's hold in front of every frame. What
/// they give up is isolation, not soundness: loro serialises state access
/// internally, so an unlocked read returns a whole string — but one that may
/// hold a multi-op write's interior, a value no writer intended (measured by
/// `an_unlocked_read_during_another_writers_transaction_is_dirty_not_torn` and
/// `an_unlocked_read_inside_a_multi_op_write_sees_an_intermediate_state`). The
/// editor converges on the commit event that follows.
impl CellBacking<String> for LoroTextCellBacking {
    fn current(&self) -> String {
        self.text.to_string()
    }

    fn signal(&self) -> BoxStream<'static, String> {
        // Phase 1 wires the remote-deltas stream as the structural change
        // signal — every peer-originated delta produces an emission with
        // the post-delta full string. Self-originated changes are NOT
        // emitted here (the editor already has the value); consumers that
        // want every value can subscribe to `Cell::signal()` AND keep
        // their own write echoes (the existing pattern in
        // `editor_view_model.rs`). Mirrors the historical
        // `MutableText::remote_deltas` contract — initial value, then one
        // emission per peer delta.
        let deltas = self.remote_deltas();
        let text = self.text.clone();
        let initial = text.to_string();
        let text_for_map = text;
        let tail = deltas.map(move |_delta| text_for_map.to_string());
        Box::pin(futures::stream::once(async move { initial }).chain(tail))
    }

    fn apply_replace(&self, v: String) -> BoxFuture<'static, Result<()>> {
        let doc = self.doc.clone();
        let text = self.text.clone();
        let lock = self.lock.clone();
        let label = self.lock_label.clone();
        let bus = self.bus.clone();
        let subject = self.subject.clone();
        Box::pin(async move {
            disclose_refused_write(
                &bus,
                &subject,
                lock.write(&label, || {
                    doc.set_next_commit_origin(&WriteOrigin::UiValueSet.as_origin());
                    mutate_guarded(&doc, "a text cell value-set", || {
                        text.update(&v, loro::UpdateOptions::default())
                            .map_err(|e| anyhow!("LoroText::update failed: {e:?}"))
                    })?;
                    doc.commit();
                    Ok(())
                }),
            )
        })
    }

    fn as_text_backing(&self) -> Option<&dyn TextCellBacking> {
        Some(self)
    }
}

impl TextCellBacking for LoroTextCellBacking {
    fn apply_text_op(&self, op: TextOp) -> Result<()> {
        // The editor's own keystroke — stamp the echo origin so the subscribe
        // filter drops it (the editor already holds this value; converging it
        // back would yank the caret to end). All non-keystroke writers carry
        // another origin and therefore pass the filter.
        self.write_disclosing(|| {
            self.doc
                .set_next_commit_origin(&WriteOrigin::UiEditorKeystroke.as_origin());
            mutate_guarded(&self.doc, "a text cell keystroke", || {
                match op {
                    TextOp::Insert {
                        pos_codepoint,
                        text,
                    } => {
                        self.text.insert(pos_codepoint, &text)?;
                    }
                    TextOp::Delete {
                        pos_codepoint,
                        len_codepoint,
                    } => {
                        self.text.delete(pos_codepoint, len_codepoint)?;
                    }
                }
                Ok(())
            })?;
            self.doc.commit();
            Ok(())
        })
    }

    fn anchor_cursor(&self, char_offset: usize, bias: CursorBias) -> CursorAnchor {
        let inner = self
            .text
            .get_cursor(char_offset, Default::default())
            .unwrap_or_else(|| self.text.get_cursor(0, Default::default()).unwrap());
        CursorAnchor::new(Box::new(inner), bias)
    }

    fn resolve_cursor(&self, anchor: &CursorAnchor) -> usize {
        let Some(inner) = anchor.inner.downcast_ref::<LoroCursor>() else {
            tracing::warn!(
                "LoroTextCellBacking::resolve_cursor received an anchor whose inner is not a \
                 loro::Cursor. Returning 0; caller likely created the anchor on a different \
                 backing."
            );
            return 0;
        };
        self.doc
            .get_cursor_pos(inner)
            .map(|r| r.current.pos)
            .unwrap_or(0)
    }

    fn remote_deltas(&self) -> BoxStream<'static, TextDelta> {
        let rx = self.remote_tx.subscribe();
        Box::pin(BroadcastStream::new(rx).filter_map(|r| async move {
            match r {
                Ok(delta) => Some(delta),
                Err(_) => {
                    tracing::warn!(
                        "LoroTextCellBacking remote_deltas lagged; consumer should call current() \
                         and resync"
                    );
                    None
                }
            }
        }))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn translate_text_delta(deltas: &[loro::TextDelta]) -> TextDelta {
    let mut ops = Vec::new();
    for delta in deltas {
        match delta {
            loro::TextDelta::Retain { retain, .. } => {
                ops.push(DeltaOp::Retain {
                    len_codepoint: *retain,
                });
            }
            loro::TextDelta::Insert { insert, .. } => {
                ops.push(DeltaOp::Insert {
                    text: insert.clone(),
                });
            }
            loro::TextDelta::Delete { delete } => {
                ops.push(DeltaOp::Delete {
                    len_codepoint: *delete,
                });
            }
        }
    }
    TextDelta { ops }
}

#[cfg(test)]
mod tests {
    use anyhow::bail;
    use loro::LoroDoc;

    use super::*;

    fn make_doc_with_text() -> (Arc<LoroDoc>, LoroText) {
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree("test_tree");
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        let text: LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
        (doc, text)
    }

    /// One row per commit: its origin, and whether the committing thread held
    /// the doc's write guard when it fired.
    type CommitGuardLog = Arc<std::sync::Mutex<Vec<(String, bool)>>>;

    /// For every commit the doc emits: its origin, and whether the thread that
    /// fired it held this doc's write guard at that moment.
    ///
    /// The guard flag is meaningful only when loro runs the callback ON the
    /// committing thread. It usually does, but an emission raised while a
    /// subscriber is already running is queued
    /// (`loro-internal/src/subscription.rs:107-118`) and delivered by whichever
    /// thread next drains it — and that thread's guard state says nothing about
    /// the committer's. `expected` is the thread the caller is about to commit
    /// on; a callback that arrives anywhere else panics rather than record a
    /// flag that could be read as either a false green or a false red.
    fn guard_at_each_commit(doc: &Arc<LoroDoc>) -> (loro::Subscription, CommitGuardLog) {
        let lock = crate::doc_lock::DocLock::for_doc(doc);
        let expected = std::thread::current().id();
        let seen: CommitGuardLog = Arc::new(std::sync::Mutex::new(Vec::new()));
        let out = seen.clone();
        let sub = doc.subscribe_root(Arc::new(move |event| {
            assert_eq!(
                std::thread::current().id(),
                expected,
                "loro delivered this commit's event on a thread other than the \
                 committer's, so the write-guard flag would describe the wrong \
                 thread"
            );
            out.lock()
                .unwrap()
                .push((event.origin.to_string(), lock.this_thread_holds_write()));
        }));
        (sub, seen)
    }

    /// A guarded batch that FAILS must not leave its ops for the next writer.
    ///
    /// `with_write`'s closure can return `Err` after it has already applied
    /// ops. Loro's pending transaction is per-document, so ops left behind are
    /// flushed by whoever commits next and reach subscribers under THAT
    /// writer's origin: here the failed block batch would arrive labelled as
    /// the user's keystroke and then be dropped by the editor's echo filter.
    /// Under D154 it is worse — a failed batch's partial ops become durable
    /// under another writer's name, so a rollback cannot attribute them.
    ///
    /// The scope therefore flushes its own ops however it ends. This test does
    /// not assert that the failed batch is UNDONE: `with_write` is isolation,
    /// not rollback (`tests/with_write_is_isolation_not_rollback.rs`), and the
    /// pinned loro exposes no way to abort a transaction.
    #[test]
    fn a_failed_batch_does_not_leave_its_ops_for_the_next_writer() {
        use std::sync::Mutex;

        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let document = crate::LoroDocument::from_existing(doc.clone(), "failed-batch");
        let tree = doc.get_tree("test_tree");
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let typed: LoroText = tree
            .get_meta(node)
            .unwrap()
            .ensure_mergeable_text("content_raw")
            .unwrap();
        let written: LoroText = tree
            .get_meta(node)
            .unwrap()
            .ensure_mergeable_text("other")
            .unwrap();
        let typed_id = typed.id();
        let written_id = written.id();
        doc.commit();

        let commits: Arc<Mutex<Vec<(String, bool, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = commits.clone();
        let _sub = doc.subscribe_root(Arc::new(move |event| {
            let touched = |id: &ContainerID| event.events.iter().any(|e| *e.target == *id);
            seen.lock().unwrap().push((
                event.origin.to_string(),
                touched(&typed_id),
                touched(&written_id),
            ));
        }));

        let backing = LoroTextCellBacking::new(doc.clone(), typed).unwrap();

        let failed = document.with_write(WriteOrigin::BlockOps, |_txn| -> Result<()> {
            written.insert(0, "block")?;
            bail!("a later step of this batch failed")
        });
        assert!(failed.is_err(), "the batch must report its failure");
        assert_eq!(
            doc.get_pending_txn_len(),
            0,
            "the failed batch dropped its write guard with ops still pending"
        );

        backing
            .apply_text_op(TextOp::Insert {
                pos_codepoint: 0,
                text: "k".to_string(),
            })
            .unwrap();

        let commits = commits.lock().unwrap();
        let echo = editor_echo_origin().to_string();
        assert!(
            commits
                .iter()
                .all(|(origin, _, written)| !(*origin == echo && *written)),
            "the keystroke's commit carried the failed batch's ops under the \
             keystroke's own origin: {commits:?}"
        );
        assert!(
            commits.iter().any(
                |(origin, _, written)| *written && *origin == WriteOrigin::BlockOps.as_origin()
            ),
            "the failed batch's op must reach subscribers under the batch's own \
             origin: {commits:?}"
        );
    }

    /// The keystroke's COMMIT — not merely its op — must run under the write
    /// guard.
    ///
    /// `commit()` flushes the document's whole pending transaction, so a
    /// commit taken outside the guard carries away whatever any other writer
    /// left pending, under the keystroke's origin. Holding the guard for the
    /// op alone and releasing it before the commit reopens exactly the window
    /// the guard exists to close, and does so without any interleaving the
    /// acquire can be made to lose.
    #[test]
    fn a_keystrokes_commit_fires_under_the_write_guard() {
        let (doc, text) = make_doc_with_text();
        let (_sub, commits) = guard_at_each_commit(&doc);
        let backing = LoroTextCellBacking::new(doc, text).unwrap();

        backing
            .apply_text_op(TextOp::Insert {
                pos_codepoint: 0,
                text: "k".to_string(),
            })
            .unwrap();

        let commits = commits.lock().unwrap();
        assert_eq!(commits.len(), 1, "one op, one commit: {commits:?}");
        assert!(
            commits[0].1,
            "the keystroke flushed the document's shared pending transaction \
             without exclusive access: {commits:?}"
        );
    }

    /// The authoritative value-set path owes the same guarantee.
    ///
    /// `apply_replace` is a second writer on the same document (`UiValueSet`,
    /// the cell's non-keystroke write). Its op and its commit must be one
    /// guarded unit for the same reason the keystroke's must.
    #[tokio::test]
    async fn a_value_sets_commit_fires_under_the_write_guard() {
        let (doc, text) = make_doc_with_text();
        let (_sub, commits) = guard_at_each_commit(&doc);
        let backing = LoroTextCellBacking::new(doc, text).unwrap();

        backing.apply_replace("value".to_string()).await.unwrap();

        let commits = commits.lock().unwrap();
        assert_eq!(commits.len(), 1, "one replace, one commit: {commits:?}");
        assert!(
            commits[0].1,
            "the value-set flushed the document's shared pending transaction \
             without exclusive access: {commits:?}"
        );
    }

    /// A commit must carry exactly one writer's ops.
    ///
    /// The pending loro transaction is per-document and shared: a writer that
    /// commits without the document's write lock flushes whatever another
    /// writer has left pending along with its own. The merged commit then
    /// reaches subscribers under the LAST origin armed, so a block operation
    /// arrives labelled as the user's keystroke — suppressed by the editor's
    /// echo filter and offered to the text-undo manager as user text.
    #[test]
    fn a_keystroke_does_not_flush_another_writer_into_its_own_commit() {
        use std::sync::Mutex;
        use std::sync::mpsc;

        // The wrapper is built AROUND this `Arc`, not asked for it back:
        // `DocLock` keys on the `Arc`, so both writers contend for one lock.
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let document = crate::LoroDocument::from_existing(doc.clone(), "attribution");
        let tree = doc.get_tree("test_tree");
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let typed: LoroText = tree
            .get_meta(node)
            .unwrap()
            .ensure_mergeable_text("content_raw")
            .unwrap();
        let written: LoroText = tree
            .get_meta(node)
            .unwrap()
            .ensure_mergeable_text("other")
            .unwrap();
        let typed_id = typed.id();
        let written_id = written.id();

        // One entry per commit: its origin, and whether it carried each
        // writer's container.
        let commits: Arc<Mutex<Vec<(String, bool, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = commits.clone();
        let _sub = doc.subscribe_root(Arc::new(move |event| {
            let touched = |id: &ContainerID| event.events.iter().any(|e| *e.target == *id);
            seen.lock().unwrap().push((
                event.origin.to_string(),
                touched(&typed_id),
                touched(&written_id),
            ));
        }));

        let backing = LoroTextCellBacking::new(doc.clone(), typed).unwrap();
        let observed_lock = crate::doc_lock::DocLock::for_doc(&doc);

        let (inside_tx, inside_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished_in_writer = finished.clone();

        let writer = std::thread::spawn(move || {
            document
                .with_write(WriteOrigin::BlockOps, |_txn| {
                    written.insert(0, "block")?;
                    // Only now may the keystroke start, so it cannot win the
                    // lock before the writer ever takes it.
                    inside_tx.send(()).unwrap();
                    // Release only once the keystroke thread is PARKED on this
                    // doc's write lock: the window is then closed by
                    // construction rather than by a sleep the scheduler may
                    // spend elsewhere. A build without the lock parks nobody,
                    // so the deadline expires and the unguarded commit lands
                    // inside the window — which is the failure this asserts.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while observed_lock.writers_waiting() == 0
                        && std::time::Instant::now() < deadline
                    {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    let committed_inside =
                        finished_in_writer.load(std::sync::atomic::Ordering::SeqCst);
                    release_tx.send(committed_inside).unwrap();
                    Ok(())
                })
                .unwrap();
        });

        inside_rx.recv().unwrap();
        let keystroke = std::thread::spawn(move || {
            backing
                .apply_text_op(TextOp::Insert {
                    pos_codepoint: 0,
                    text: "k".to_string(),
                })
                .unwrap();
            finished.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let committed_inside = release_rx.recv().unwrap();
        writer.join().unwrap();
        keystroke.join().unwrap();
        assert!(
            !committed_inside,
            "the keystroke committed while another writer held the document"
        );

        let commits = commits.lock().unwrap();
        let merged: Vec<&(String, bool, bool)> = commits
            .iter()
            .filter(|(_, typed, written)| *typed && *written)
            .collect();
        assert!(
            merged.is_empty(),
            "a commit carried two writers' ops, so its origin names only one of them: \
             {merged:?} (all commits: {commits:?})"
        );
    }

    /// A keystroke the write lock refuses is DISCLOSED, not just returned.
    ///
    /// The frontends log the `Err` and carry on, so an undisclosed refusal is
    /// a keystroke the user watched land in the editor and never reached the
    /// store. The read-upgrade bail is the fast path into that branch; the
    /// 30s timeout reaches the same disclosure.
    #[test]
    fn a_keystroke_the_lock_refuses_reaches_the_condition_bus() {
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let document = crate::LoroDocument::from_existing(doc.clone(), "refusal");
        let tree = doc.get_tree("test_tree");
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let text: LoroText = tree
            .get_meta(node)
            .unwrap()
            .ensure_mergeable_text("content_raw")
            .unwrap();

        let bus = Arc::new(holon_api::ConditionBus::new());
        let backing = LoroTextCellBacking::disclosing(
            doc,
            text,
            Some(bus.clone()),
            "block:typed-into".to_string(),
        )
        .unwrap();

        // A write requested while this thread holds the read guard cannot be
        // satisfied, so the lock refuses immediately instead of blocking.
        let refused = document.with_read(|_| {
            Ok(backing.apply_text_op(TextOp::Insert {
                pos_codepoint: 0,
                text: "k".to_string(),
            }))
        });
        assert!(
            refused.unwrap().is_err(),
            "the lock must refuse a write requested under a read guard"
        );

        let raised = bus.current();
        let disclosed: Vec<_> = raised
            .iter()
            .filter(|c| c.condition_key().kind == holon_api::ConditionKind::LOCAL_EDIT_NOT_APPLIED)
            .collect();
        assert_eq!(
            disclosed.len(),
            1,
            "the refused keystroke must be disclosed, not only returned: {raised:?}"
        );
        assert_eq!(disclosed[0].subject, "block:typed-into");
    }

    #[test]
    fn current_and_apply_text_op_round_trip() -> Result<()> {
        let (doc, text) = make_doc_with_text();
        let backing = LoroTextCellBacking::new(doc, text)?;
        backing.apply_text_op(TextOp::Insert {
            pos_codepoint: 0,
            text: "hello".to_string(),
        })?;
        assert_eq!(backing.current(), "hello");
        backing.apply_text_op(TextOp::Insert {
            pos_codepoint: 5,
            text: " world".to_string(),
        })?;
        assert_eq!(backing.current(), "hello world");
        backing.apply_text_op(TextOp::Delete {
            pos_codepoint: 5,
            len_codepoint: 6,
        })?;
        assert_eq!(backing.current(), "hello");
        Ok(())
    }

    #[tokio::test]
    async fn apply_replace_round_trip() -> Result<()> {
        let (doc, text) = make_doc_with_text();
        let backing: Arc<dyn CellBacking<String>> = Arc::new(LoroTextCellBacking::new(doc, text)?);
        assert_eq!(backing.current(), "");
        backing.apply_replace("first".to_string()).await?;
        assert_eq!(backing.current(), "first");
        backing.apply_replace("second".to_string()).await?;
        assert_eq!(backing.current(), "second");
        Ok(())
    }

    #[test]
    fn echo_suppression_origin_filter() -> Result<()> {
        let (doc, text) = make_doc_with_text();
        let backing = LoroTextCellBacking::new(doc, text)?;
        let mut rx = backing.remote_tx.subscribe();
        backing.apply_text_op(TextOp::Insert {
            pos_codepoint: 0,
            text: "x".into(),
        })?;
        // The editor's own keystroke is stamped the keystroke origin and must
        // be dropped by the subscribe filter — it should NOT reach remote_tx.
        assert!(rx.try_recv().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn authoritative_replace_reaches_remote_tx() -> Result<()> {
        // The fix: a NON-keystroke write (`apply_replace`, origin
        // `WriteOrigin::UiValueSet` — the same origin structural
        // `set_field`/`update_block_text` writes use) is authoritative and MUST
        // pass the filter so a subscribed editor converges to it. This is what
        // was previously (wrongly) swallowed, causing the join's merged content
        // to be lost.
        let (doc, text) = make_doc_with_text();
        let backing = LoroTextCellBacking::new(doc, text)?;
        let mut rx = backing.remote_tx.subscribe();
        backing.apply_replace("8".to_string()).await?;
        assert!(
            rx.try_recv().is_ok(),
            "authoritative apply_replace must reach remote_tx (editor convergence channel)"
        );
        Ok(())
    }

    /// Increment G finding — echo suppression is GLOBAL to the backing, not
    /// scoped to the originating editor. Two occurrences of the SAME block
    /// share ONE `LoroTextCellBacking` (same `EntityUri` → one `CellCache`
    /// entry → one `remote_tx`, since `Cell` clones share the backing `Arc`).
    /// A keystroke via `apply_text_op` is stamped the keystroke origin and
    /// dropped at the doc-subscribe callback (Filter A) BEFORE it reaches
    /// `remote_tx`, so it reaches NEITHER subscriber — a non-typing sibling
    /// occurrence is starved and never converges via `remote_deltas`. This is
    /// why "type in one occurrence, the other updates live" is NOT true "by
    /// construction" on the shared-cell path: pre-Increment-G it was the
    /// per-row `_data_subscription` (CDC) and the render backstop that
    /// carried sibling liveness — exactly the paths Increment G retires. A
    /// non-echo write (`apply_replace`, origin `WriteOrigin::UiValueSet` — the
    /// same origin class as structural `set_field` / peer imports; the
    /// filter is a pure origin-string check, so this is representative)
    /// DOES reach both subscribers, so the filter's legitimate job (never
    /// echoing an editor's OWN keystroke back to itself) stays proven.
    #[tokio::test]
    async fn keystroke_echo_starves_sibling_subscriber() -> Result<()> {
        let (doc, text) = make_doc_with_text();
        let backing = LoroTextCellBacking::new(doc, text)?;
        // Occurrence 1 (the typist) and occurrence 2 (the sibling) both
        // subscribe to the one shared remote-delta channel.
        let mut occ1 = backing.remote_tx.subscribe();
        let mut occ2 = backing.remote_tx.subscribe();

        // (a) A keystroke reaches NEITHER subscriber — the sibling is starved.
        backing.apply_text_op(TextOp::Insert {
            pos_codepoint: 0,
            text: "x".into(),
        })?;
        assert!(
            occ1.try_recv().is_err(),
            "keystroke echo must not reach the originating occurrence"
        );
        assert!(
            occ2.try_recv().is_err(),
            "keystroke echo is filtered GLOBALLY, so a sibling occurrence sharing the backing \
             never receives the edit via remote_deltas"
        );

        // (b) A non-echo authoritative write reaches BOTH subscribers — the
        // filter's legitimate pass-through is intact.
        backing.apply_replace("y".to_string()).await?;
        assert!(
            occ1.try_recv().is_ok(),
            "non-echo write must reach the first subscriber"
        );
        assert!(
            occ2.try_recv().is_ok(),
            "non-echo write must reach the sibling subscriber"
        );
        Ok(())
    }
}
