//! [`TextCellBacking`] backed by a Loro `LoroText` container.
//!
//! Direct port of the previous `MutableTextInner` (`mutable_text.rs`)
//! adapted to the [`CellBacking`] / [`TextCellBacking`] traits. Reads the
//! current string from the `LoroText`, applies local edits as Loro
//! `insert`/`delete` ops with `set_next_commit_origin("ui_local")` so the
//! outbound projector can distinguish self-originated writes, and
//! re-publishes peer-originated deltas through a broadcast channel.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use loro::cursor::Cursor as LoroCursor;
use loro::event::Diff;
use loro::{ContainerID, ContainerTrait, LoroDoc, LoroText};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use holon_core::cell::{
    CellBacking, CursorAnchor, CursorBias, DeltaOp, TextCellBacking, TextDelta, TextOp,
};

/// Commit origin stamped on the editor's *own keystroke* writes
/// (`apply_text_op`). It is the **only** origin the subscribe filter
/// suppresses: the editor already holds the value it just typed, and
/// re-delivering its own echo would yank the caret to end via the absolute
/// `set_value` convergence path (`editor_view::converge_input`).
///
/// Every *other* writer — structural ops / `set_field` (which reach Loro via
/// `update_block_text` → `LoroDocument::with_write_origin("ui_local")`),
/// `apply_replace`, and remote peer imports — is an **authoritative** write
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
pub(crate) const EDITOR_ECHO_ORIGIN: &str = "ui_editor_echo";

pub struct LoroTextCellBacking {
    doc: Arc<LoroDoc>,
    text: LoroText,
    #[allow(dead_code)]
    text_id: ContainerID,
    remote_tx: broadcast::Sender<TextDelta>,
    #[allow(dead_code)]
    subscription: loro::Subscription,
}

impl LoroTextCellBacking {
    /// Wrap an existing `LoroText` container into a text-rich cell
    /// backing. Subscribes to the Loro doc on construction so peer-
    /// originated deltas land on the broadcast channel.
    pub fn new(doc: Arc<LoroDoc>, text: LoroText) -> Result<Self> {
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
                // reach the editor so it converges. See `EDITOR_ECHO_ORIGIN`.
                if event.origin == EDITOR_ECHO_ORIGIN {
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

        Ok(Self {
            doc,
            text,
            text_id,
            remote_tx,
            subscription,
        })
    }
}

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
        Box::pin(async move {
            doc.set_next_commit_origin("ui_local");
            text.update(&v, loro::UpdateOptions::default())
                .map_err(|e| anyhow!("LoroText::update failed: {e:?}"))?;
            doc.commit();
            Ok(())
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
        // back would yank the caret to end). All non-keystroke writers keep
        // "ui_local" and therefore pass the filter.
        self.doc.set_next_commit_origin(EDITOR_ECHO_ORIGIN);
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
        self.doc.commit();
        Ok(())
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
                "LoroTextCellBacking::resolve_cursor received an anchor whose inner is not \
                 a loro::Cursor. Returning 0; caller likely created the anchor on a different \
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
                        "LoroTextCellBacking remote_deltas lagged; \
                         consumer should call current() and resync"
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
    use super::*;
    use loro::LoroDoc;

    fn make_doc_with_text() -> (Arc<LoroDoc>, LoroText) {
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree("test_tree");
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        let text: LoroText = meta
            .insert_container("content_raw", LoroText::new())
            .unwrap();
        (doc, text)
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
        // The editor's own keystroke is stamped EDITOR_ECHO_ORIGIN and must
        // be dropped by the subscribe filter — it should NOT reach remote_tx.
        assert!(rx.try_recv().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn authoritative_replace_reaches_remote_tx() -> Result<()> {
        // The fix: a NON-keystroke write (`apply_replace`, origin "ui_local" —
        // the same origin structural `set_field`/`update_block_text` writes
        // use) is authoritative and MUST pass the filter so a subscribed
        // editor converges to it. This is what was previously (wrongly)
        // swallowed, causing the join's merged content to be lost.
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
}
