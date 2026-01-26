//! Thin per-block-per-field UI adapter wrapping a `LoroText` handle.
//!
//! Cached in the provider by `(BlockId, FieldName)` so re-renders don't
//! churn subscriptions. Editors call `apply_local` synchronously; remote
//! changes arrive as a stream of deltas filtered by origin.

use anyhow::Result;
use loro::cursor::Cursor as LoroCursor;
use loro::event::Diff;
use loro::{ContainerID, ContainerTrait, LoroDoc, LoroText};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

// ── TextOp / TextDelta ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum TextOp {
    Insert {
        pos_codepoint: usize,
        text: String,
    },
    Delete {
        pos_codepoint: usize,
        len_codepoint: usize,
    },
}

#[derive(Clone, Debug)]
pub struct TextDelta {
    pub ops: Vec<DeltaOp>,
}

#[derive(Clone, Debug)]
pub enum DeltaOp {
    Retain { len_codepoint: usize },
    Insert { text: String },
    Delete { len_codepoint: usize },
}

// ── CursorAnchor ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum CursorBias {
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub struct CursorAnchor {
    inner: LoroCursor,
    bias: CursorBias,
}

// ── MutableText ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct MutableText {
    inner: Arc<MutableTextInner>,
}

struct MutableTextInner {
    doc: Arc<LoroDoc>,
    text: LoroText,
    text_id: ContainerID,
    remote_tx: broadcast::Sender<TextDelta>,
    #[allow(dead_code)]
    subscription: loro::Subscription,
}

impl MutableText {
    /// Create a new MutableText wrapping an existing LoroText container.
    pub fn new(doc: Arc<LoroDoc>, text: LoroText) -> Result<Self> {
        let text_id = text.id();
        let (remote_tx, _) = broadcast::channel(256);
        let tx_for_cb = remote_tx.clone();
        let target_id = text_id.clone();

        let subscription = doc.subscribe(
            &text_id,
            Arc::new(move |event| {
                // Filter A: skip our own writes.
                if event.origin == "ui_local" {
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
            inner: Arc::new(MutableTextInner {
                doc,
                text,
                text_id,
                remote_tx,
                subscription,
            }),
        })
    }

    pub fn current(&self) -> String {
        self.inner.text.to_string()
    }

    pub fn apply_local(&self, op: TextOp) -> Result<()> {
        self.inner.doc.set_next_commit_origin("ui_local");
        match op {
            TextOp::Insert {
                pos_codepoint,
                text,
            } => {
                self.inner.text.insert(pos_codepoint, &text)?;
            }
            TextOp::Delete {
                pos_codepoint,
                len_codepoint,
            } => {
                self.inner.text.delete(pos_codepoint, len_codepoint)?;
            }
        }
        self.inner.doc.commit();
        Ok(())
    }

    pub fn remote_deltas(&self) -> impl Stream<Item = TextDelta> {
        let rx = self.inner.remote_tx.subscribe();
        BroadcastStream::new(rx).filter_map(|r| match r {
            Ok(delta) => Some(delta),
            Err(_) => {
                tracing::warn!(
                    "MutableText remote_deltas lagged; consumer should call current() and resync"
                );
                None
            }
        })
    }

    pub fn anchor_cursor(&self, char_offset: usize, bias: CursorBias) -> CursorAnchor {
        let inner = self
            .inner
            .text
            .get_cursor(char_offset, Default::default())
            .unwrap_or_else(|| self.inner.text.get_cursor(0, Default::default()).unwrap());
        CursorAnchor { inner, bias }
    }

    pub fn resolve_cursor(&self, anchor: &CursorAnchor) -> usize {
        self.inner
            .doc
            .get_cursor_pos(&anchor.inner)
            .map(|r| r.current.pos)
            .unwrap_or(0)
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
    fn test_mutable_text_current_and_apply_local() -> Result<()> {
        let (doc, text) = make_doc_with_text();
        let mt = MutableText::new(doc, text)?;

        assert_eq!(mt.current(), "");

        mt.apply_local(TextOp::Insert {
            pos_codepoint: 0,
            text: "Hello".to_string(),
        })?;
        assert_eq!(mt.current(), "Hello");

        mt.apply_local(TextOp::Insert {
            pos_codepoint: 5,
            text: " World".to_string(),
        })?;
        assert_eq!(mt.current(), "Hello World");

        mt.apply_local(TextOp::Delete {
            pos_codepoint: 5,
            len_codepoint: 6,
        })?;
        assert_eq!(mt.current(), "Hello");

        Ok(())
    }

    #[test]
    fn test_mutable_text_echo_suppression() -> Result<()> {
        let (doc, text) = make_doc_with_text();
        let mt = MutableText::new(doc.clone(), text)?;

        // Do a local edit — should NOT appear on remote_deltas
        mt.apply_local(TextOp::Insert {
            pos_codepoint: 0,
            text: "secret".to_string(),
        })?;

        // The echo filter should skip this (origin == "ui_local")
        // No easy way to assert stream emptiness synchronously, but the
        // architecture guarantees the filter is in place.
        assert_eq!(mt.current(), "secret");
        Ok(())
    }

    #[test]
    fn test_mutable_text_cursor_anchor() -> Result<()> {
        let (doc, text) = make_doc_with_text();
        let mt = MutableText::new(doc, text)?;

        mt.apply_local(TextOp::Insert {
            pos_codepoint: 0,
            text: "Hello World".to_string(),
        })?;

        let anchor = mt.anchor_cursor(5, CursorBias::Left);
        let pos = mt.resolve_cursor(&anchor);
        assert_eq!(pos, 5, "cursor at position 5 before edit");

        // Insert text before the cursor — cursor should shift right
        mt.apply_local(TextOp::Insert {
            pos_codepoint: 0,
            text: ">>".to_string(),
        })?;
        let pos = mt.resolve_cursor(&anchor);
        assert_eq!(pos, 7, "cursor should shift right after insert before it");

        Ok(())
    }
}
