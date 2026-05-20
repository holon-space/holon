//! `Cell<T>` — the universal reactive read primitive.
//!
//! A `Cell<T>` is a reactive container identified by `(EntityUri, FieldPath)`
//! exposing synchronous `current()`, reactive `signal()`, and an async `set()`
//! that dispatches via a typed writer chosen at construction. Backings
//! (Loro, LWW-via-`set_field`, external-system) implement [`CellBacking`];
//! `Cell<T>` is just the consumer-facing handle.
//!
//! Text-rich operations (insert/delete/cursor anchors) live on
//! [`Cell<String>`] and dispatch through the optional
//! [`CellBacking::as_text_backing`] accessor — Loro backings supply the rich
//! behaviour, LWW backings return `None` and `apply_text_op` fails loudly.
//! There is intentionally NO sync compute-replace path for the LWW backing
//! because that backing is unreachable in production (SqlOnly mode has no
//! editor); failing fast matches the project's "Fail Loud, Never Fake"
//! contract from `CLAUDE.md`.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use futures::{future::BoxFuture, stream::BoxStream};

/// Consumer-facing handle for a reactive field of type `T`.
///
/// Cheap to clone (just clones the `Arc`); two clones of the same `Cell`
/// observe the same backing.
pub struct Cell<T: 'static + Send + Sync + Clone> {
    inner: Arc<dyn CellBacking<T>>,
}

impl<T: 'static + Send + Sync + Clone> Clone for Cell<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: 'static + Send + Sync + Clone> Cell<T> {
    /// Wrap a backing into a `Cell`. Registry-internal API; consumers obtain
    /// `Cell`s through an [`crate::cell_registry::EntityCellRegistry`].
    pub fn from_backing(backing: Arc<dyn CellBacking<T>>) -> Self {
        Self { inner: backing }
    }

    /// Synchronous snapshot of the current value.
    pub fn current(&self) -> T {
        self.inner.current()
    }

    /// Reactive stream of values; first item is the current value, then
    /// every change.
    pub fn signal(&self) -> BoxStream<'static, T> {
        self.inner.signal()
    }

    /// Replace the value via the backing's typed writer.
    pub async fn set(&self, v: T) -> Result<()> {
        self.inner.apply_replace(v).await
    }

    /// Internal: access the underlying backing as `Arc<dyn Any>` for the
    /// registry's type-erased cache. Not part of the public API.
    pub fn inner_arc(&self) -> &Arc<dyn CellBacking<T>> {
        &self.inner
    }
}

/// Backing trait for [`Cell<T>`]. Implementations choose the storage and
/// write path (Loro container, SQL `set_field`, external API call, etc.).
///
/// Object-safe: `Cell<T>` holds `Arc<dyn CellBacking<T>>`.
pub trait CellBacking<T: 'static + Send + Sync + Clone>: Send + Sync + 'static {
    fn current(&self) -> T;
    fn signal(&self) -> BoxStream<'static, T>;
    fn apply_replace(&self, v: T) -> BoxFuture<'static, Result<()>>;

    /// Optional accessor for text-rich operations. Defaults to `None`;
    /// override on text-rich backings (e.g. `LoroTextCellBacking`).
    /// `Cell<String>::apply_text_op` calls this to decide whether to
    /// dispatch the rich op or fail loudly.
    fn as_text_backing(&self) -> Option<&dyn TextCellBacking> {
        None
    }
}

/// Text-rich operations on a `Cell<String>` backing.
///
/// Implemented by Loro-backed `Cell<String>`s. Lets editor keystrokes
/// produce minimal CRDT ops (`insert`/`delete` at codepoint positions)
/// rather than full-string replacements that would lose RGA causal
/// history under concurrent peer edits.
pub trait TextCellBacking: CellBacking<String> {
    fn apply_text_op(&self, op: TextOp) -> Result<()>;
    fn anchor_cursor(&self, char_offset: usize, bias: CursorBias) -> CursorAnchor;
    fn resolve_cursor(&self, anchor: &CursorAnchor) -> usize;
    fn remote_deltas(&self) -> BoxStream<'static, TextDelta>;
}

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

/// Compute the [`TextOp`]s needed to transform `old` into `new`.
///
/// Uses a common-prefix / common-suffix diff over **codepoints** (the unit
/// `TextOp` positions are expressed in) to isolate the single contiguous
/// region that changed. A mid-edit that both removes and adds text (e.g.
/// typing over a selection, autocomplete) yields a `Delete` followed by an
/// `Insert`, both anchored at the prefix boundary; a pure insert or pure
/// delete yields a single op. Returns an empty `Vec` when `old == new`.
pub fn compute_text_delta(old: &str, new: &str) -> Vec<TextOp> {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();

    let prefix = old_chars
        .iter()
        .zip(new_chars.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Common suffix length, clamped so it never overlaps the prefix on the
    // shorter string. `prefix <= min(old, new)` always holds, so this never
    // underflows.
    let max_suffix = old_chars.len().min(new_chars.len()) - prefix;
    let suffix = old_chars
        .iter()
        .rev()
        .zip(new_chars.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(max_suffix);

    let old_mid_len = old_chars.len() - prefix - suffix;
    let new_mid: String = new_chars[prefix..new_chars.len() - suffix].iter().collect();

    let mut ops = Vec::new();
    if old_mid_len > 0 {
        ops.push(TextOp::Delete {
            pos_codepoint: prefix,
            len_codepoint: old_mid_len,
        });
    }
    if !new_mid.is_empty() {
        ops.push(TextOp::Insert {
            pos_codepoint: prefix,
            text: new_mid,
        });
    }
    ops
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

#[derive(Clone, Debug)]
pub enum CursorBias {
    Left,
    Right,
}

/// Opaque cursor anchor. The `inner` payload is backing-specific
/// (e.g. `loro::cursor::Cursor`); kept type-erased so `holon-core` doesn't
/// drag the `loro` dependency.
pub struct CursorAnchor {
    pub inner: Box<dyn std::any::Any + Send + Sync>,
    pub bias: CursorBias,
}

impl CursorAnchor {
    pub fn new(inner: Box<dyn std::any::Any + Send + Sync>, bias: CursorBias) -> Self {
        Self { inner, bias }
    }
}

// ── Cell<String> rich-text inherent methods ────────────────────────────

impl Cell<String> {
    /// Apply a text op via the backing's [`TextCellBacking`] surface, if
    /// available. Errors loudly when the backing has no rich-text support
    /// (e.g. an LWW backing in SqlOnly mode); callers in editor paths
    /// should only ever reach this with a Loro backing.
    pub fn apply_text_op(&self, op: TextOp) -> Result<()> {
        match self.inner.as_text_backing() {
            Some(text) => text.apply_text_op(op),
            None => Err(anyhow!(
                "Cell<String>::apply_text_op called on a backing without text-rich support \
                 (as_text_backing() returned None). This is unreachable in production: \
                 SqlOnly mode + no editor. Callers in editor paths should be on a Loro \
                 backing. Use cell.set(new_string) for non-text-editor writes."
            )),
        }
    }

    pub fn anchor_cursor(&self, char_offset: usize, bias: CursorBias) -> Result<CursorAnchor> {
        match self.inner.as_text_backing() {
            Some(text) => Ok(text.anchor_cursor(char_offset, bias)),
            None => Err(anyhow!(
                "Cell<String>::anchor_cursor on non-text-rich backing (unreachable in production)"
            )),
        }
    }

    pub fn resolve_cursor(&self, anchor: &CursorAnchor) -> Result<usize> {
        match self.inner.as_text_backing() {
            Some(text) => Ok(text.resolve_cursor(anchor)),
            None => Err(anyhow!(
                "Cell<String>::resolve_cursor on non-text-rich backing (unreachable in production)"
            )),
        }
    }

    /// Stream of remote deltas (peer-originated edits, with our own writes
    /// filtered out). Empty stream on backings without text-rich support.
    pub fn remote_deltas(&self) -> BoxStream<'static, TextDelta> {
        match self.inner.as_text_backing() {
            Some(text) => text.remote_deltas(),
            None => Box::pin(futures::stream::empty()),
        }
    }
}

// ── LwwTextCellBacking ──────────────────────────────────────────────────

/// LWW-only text cell backing for SqlOnly mode and synthetic test stores.
///
/// Reads from the `read` callback; writes via the `write` callback (which
/// the registry wires to `CrudOperations::set_field` underneath). No
/// rich-text support — `as_text_backing` returns `None`, so
/// `Cell<String>::apply_text_op` fails loudly. This is intentional: LWW
/// text editing in production would be a bug (per the project's
/// authority-first design), and SqlOnly mode has no editor.
pub struct LwwTextCellBacking {
    pub(crate) read: Arc<dyn Fn() -> String + Send + Sync>,
    pub(crate) write: Arc<dyn Fn(String) -> BoxFuture<'static, Result<()>> + Send + Sync>,
    pub(crate) signal_factory: Arc<dyn Fn() -> BoxStream<'static, String> + Send + Sync>,
}

impl LwwTextCellBacking {
    pub fn new(
        read: Arc<dyn Fn() -> String + Send + Sync>,
        write: Arc<dyn Fn(String) -> BoxFuture<'static, Result<()>> + Send + Sync>,
        signal_factory: Arc<dyn Fn() -> BoxStream<'static, String> + Send + Sync>,
    ) -> Self {
        Self {
            read,
            write,
            signal_factory,
        }
    }
}

impl CellBacking<String> for LwwTextCellBacking {
    fn current(&self) -> String {
        (self.read)()
    }

    fn signal(&self) -> BoxStream<'static, String> {
        (self.signal_factory)()
    }

    fn apply_replace(&self, v: String) -> BoxFuture<'static, Result<()>> {
        (self.write)(v)
    }

    fn as_text_backing(&self) -> Option<&dyn TextCellBacking> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::Mutex;

    #[tokio::test]
    async fn lww_text_backing_replace_and_signal_round_trip() {
        let value = Arc::new(Mutex::new(String::from("hello")));
        let value_for_read = value.clone();
        let value_for_write = value.clone();
        let backing = Arc::new(LwwTextCellBacking::new(
            Arc::new(move || value_for_read.lock().unwrap().clone()),
            Arc::new(move |new_v: String| {
                let v = value_for_write.clone();
                Box::pin(async move {
                    *v.lock().unwrap() = new_v;
                    Ok(())
                })
            }),
            Arc::new(|| Box::pin(futures::stream::empty())),
        ));
        let cell = Cell::from_backing(backing as Arc<dyn CellBacking<String>>);
        assert_eq!(cell.current(), "hello");
        cell.set("world".to_string()).await.unwrap();
        assert_eq!(cell.current(), "world");
    }

    #[tokio::test]
    async fn cell_string_apply_text_op_errs_on_lww_backing() {
        let backing = Arc::new(LwwTextCellBacking::new(
            Arc::new(String::new),
            Arc::new(|_| Box::pin(async { Ok(()) })),
            Arc::new(|| Box::pin(futures::stream::empty())),
        ));
        let cell = Cell::from_backing(backing as Arc<dyn CellBacking<String>>);
        let res = cell.apply_text_op(TextOp::Insert {
            pos_codepoint: 0,
            text: "x".into(),
        });
        assert!(res.is_err());
        let msg = format!("{:#}", res.unwrap_err());
        assert!(msg.contains("apply_text_op"));
    }

    #[tokio::test]
    async fn cell_string_remote_deltas_empty_on_lww_backing() {
        let backing = Arc::new(LwwTextCellBacking::new(
            Arc::new(String::new),
            Arc::new(|_| Box::pin(async { Ok(()) })),
            Arc::new(|| Box::pin(futures::stream::empty())),
        ));
        let cell = Cell::from_backing(backing as Arc<dyn CellBacking<String>>);
        let mut stream = cell.remote_deltas();
        // empty stream completes immediately
        assert!(stream.next().await.is_none());
    }

    /// Apply computed ops to `old` (in codepoint space) and assert we
    /// reconstruct `new` — the property `compute_text_delta` must hold.
    fn apply_delta(old: &str, ops: &[TextOp]) -> String {
        let mut chars: Vec<char> = old.chars().collect();
        for op in ops {
            match op {
                TextOp::Delete {
                    pos_codepoint,
                    len_codepoint,
                } => {
                    chars.drain(*pos_codepoint..*pos_codepoint + *len_codepoint);
                }
                TextOp::Insert {
                    pos_codepoint,
                    text,
                } => {
                    chars.splice(*pos_codepoint..*pos_codepoint, text.chars());
                }
            }
        }
        chars.into_iter().collect()
    }

    fn check_delta(old: &str, new: &str) {
        let ops = compute_text_delta(old, new);
        assert_eq!(
            apply_delta(old, &ops),
            new,
            "old={old:?} new={new:?} ops={ops:?}"
        );
    }

    #[test]
    fn delta_insert_only() {
        check_delta("abc", "abXc");
        check_delta("", "hello");
        check_delta("abc", "abcdef");
        check_delta("abc", "Xabc");
    }

    #[test]
    fn delta_delete_only() {
        check_delta("abXc", "abc");
        check_delta("hello", "");
        check_delta("abcdef", "abc");
        check_delta("Xabc", "abc");
    }

    #[test]
    fn delta_replace_in_middle() {
        check_delta("abcdef", "abXYZef");
        check_delta("hello world", "hello there");
        // A replace yields both a Delete and an Insert.
        let ops = compute_text_delta("abcdef", "abXYZef");
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], TextOp::Delete { .. }));
        assert!(matches!(ops[1], TextOp::Insert { .. }));
    }

    #[test]
    fn delta_overlapping_prefix_and_suffix() {
        // The crash case: prefix and suffix regions would overlap.
        check_delta("aaa", "aa");
        check_delta("aa", "aaa");
        check_delta("ababab", "abab");
        check_delta("xxxx", "xx");
    }

    #[test]
    fn delta_empty_and_identical() {
        assert!(compute_text_delta("same", "same").is_empty());
        check_delta("", "");
    }

    #[test]
    fn delta_multibyte_and_emoji() {
        check_delta("café", "cafés");
        check_delta("héllo", "hllo");
        check_delta("a😀b", "a😀😀b");
        check_delta("foo😀bar", "foobar");
        check_delta("naïve", "native");
    }
}
