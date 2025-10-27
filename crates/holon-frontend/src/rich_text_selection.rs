//! Framework-agnostic caret + selection state for the rich-text editor.
//!
//! Holds two resolved Unicode-scalar positions (`anchor`, `head`) and the
//! mutation API a controller invokes from keyboard / mouse / IME events:
//! click → `move_to`, shift-click / arrow-with-shift → `extend_to`, etc.
//!
//! # What this is NOT
//!
//! - **Not a Loro cursor.** Cursors are framework-coupled (`loro::cursor::Cursor`
//!   needs the `loro` crate). The GPUI integration layer owns the cursor
//!   handles and re-resolves them to scalar positions after every backend
//!   mutation, then pushes the resolved values into this struct via
//!   `set_resolved`. That keeps `holon-frontend` Loro-free.
//! - **Not a buffer.** The buffer lives in Loro; reads come from
//!   `Block.content` / `Block.marks` projections.
//!
//! # Why anchor + head, not start + end
//!
//! The distinction matters for shift-arrow extension and for IME range
//! reporting — `selected_text_range` must report the head's *side*, not just
//! a sorted range. `range()` produces `[min, max)` for callers that want the
//! covered region.

use std::ops::Range;

/// A caret or non-empty selection over a block's text.
///
/// Positions are Unicode-scalar offsets (matching `MarkSpan::start`/`end`
/// and Loro's default `mark` flavor). `anchor == head` is a caret;
/// `anchor != head` is a non-empty selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RichTextSelection {
    /// The selection's fixed end — set by click, doesn't move under
    /// shift-arrow extension.
    pub anchor: usize,
    /// The selection's moving end — where the visible caret renders.
    /// Shift-arrow / drag-select moves only this.
    pub head: usize,
}

impl RichTextSelection {
    /// Empty selection (caret) at `pos`.
    pub fn caret_at(pos: usize) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }

    /// Non-empty selection between `anchor` and `head`. The two may be in
    /// either order — `range()` always returns the sorted half-open range.
    pub fn span(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// True when this is a caret (no characters covered).
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.head
    }

    /// Half-open range of covered scalars, `[min(anchor, head), max(...))`.
    /// Returns an empty range for a caret.
    pub fn range(&self) -> Range<usize> {
        if self.anchor <= self.head {
            self.anchor..self.head
        } else {
            self.head..self.anchor
        }
    }

    /// Collapse the selection to the head (the visible caret end).
    /// Right-arrow without shift uses this to drop the trailing selection.
    pub fn collapse_to_head(&mut self) {
        self.anchor = self.head;
    }

    /// Collapse the selection to the anchor.
    pub fn collapse_to_anchor(&mut self) {
        self.head = self.anchor;
    }

    /// Move both endpoints to `pos` — a click or a non-shift arrow key.
    pub fn move_to(&mut self, pos: usize) {
        self.anchor = pos;
        self.head = pos;
    }

    /// Move only the head — shift-arrow / drag-select extending the
    /// selection from a fixed anchor.
    pub fn extend_to(&mut self, pos: usize) {
        self.head = pos;
    }

    /// Select the full text `[0, text_len)`. Anchor at start so subsequent
    /// shift-extensions feel natural ("select all then shift-left to
    /// shrink from the end").
    pub fn select_all(&mut self, text_len: usize) {
        self.anchor = 0;
        self.head = text_len;
    }

    /// Clamp both endpoints into `[0, text_len]`. Call after the buffer
    /// shrinks (delete, remote concurrent edit) to keep the selection
    /// inside the document. The bound is inclusive of `text_len` so a
    /// caret can sit at end-of-text.
    pub fn clamp_to(&mut self, text_len: usize) {
        self.anchor = self.anchor.min(text_len);
        self.head = self.head.min(text_len);
    }
}

impl Default for RichTextSelection {
    fn default() -> Self {
        Self::caret_at(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_is_collapsed() {
        let sel = RichTextSelection::caret_at(5);
        assert_eq!(sel.anchor, 5);
        assert_eq!(sel.head, 5);
        assert!(sel.is_collapsed());
        assert_eq!(sel.range(), 5..5);
    }

    #[test]
    fn span_forward_range() {
        let sel = RichTextSelection::span(2, 7);
        assert!(!sel.is_collapsed());
        assert_eq!(sel.range(), 2..7);
    }

    #[test]
    fn span_backward_range_sorts() {
        // anchor=7, head=2 — selecting backward from a click. range() sorts.
        let sel = RichTextSelection::span(7, 2);
        assert_eq!(sel.range(), 2..7);
        assert_eq!(sel.head, 2, "head reports backward direction");
        assert_eq!(sel.anchor, 7);
    }

    #[test]
    fn collapse_to_head_drops_anchor() {
        let mut sel = RichTextSelection::span(2, 7);
        sel.collapse_to_head();
        assert_eq!(sel, RichTextSelection::caret_at(7));
    }

    #[test]
    fn collapse_to_anchor_drops_head() {
        let mut sel = RichTextSelection::span(2, 7);
        sel.collapse_to_anchor();
        assert_eq!(sel, RichTextSelection::caret_at(2));
    }

    #[test]
    fn move_to_resets_both_endpoints() {
        let mut sel = RichTextSelection::span(2, 7);
        sel.move_to(10);
        assert_eq!(sel, RichTextSelection::caret_at(10));
    }

    #[test]
    fn extend_to_moves_only_head() {
        let mut sel = RichTextSelection::caret_at(5);
        sel.extend_to(10);
        // anchor stays at 5; head moves to 10.
        assert_eq!(sel.anchor, 5);
        assert_eq!(sel.head, 10);
        assert_eq!(sel.range(), 5..10);
    }

    #[test]
    fn extend_to_can_invert_direction() {
        // Start with a forward selection; shift-arrow past the anchor
        // flips direction. range() still sorts.
        let mut sel = RichTextSelection::span(5, 10);
        sel.extend_to(2);
        assert_eq!(sel.anchor, 5);
        assert_eq!(sel.head, 2);
        assert_eq!(sel.range(), 2..5);
    }

    #[test]
    fn select_all_anchors_at_start() {
        let mut sel = RichTextSelection::caret_at(3);
        sel.select_all(20);
        assert_eq!(sel.anchor, 0);
        assert_eq!(sel.head, 20);
        assert_eq!(sel.range(), 0..20);
    }

    #[test]
    fn clamp_keeps_caret_at_end_of_text() {
        // text_len=5 means the caret can sit at pos 5 (one-past-the-last
        // scalar) — that's where typing appends.
        let mut sel = RichTextSelection::caret_at(20);
        sel.clamp_to(5);
        assert_eq!(sel, RichTextSelection::caret_at(5));
    }

    #[test]
    fn clamp_handles_partial_overflow() {
        // Selection [3..15) over text that shrank to 8 chars.
        let mut sel = RichTextSelection::span(3, 15);
        sel.clamp_to(8);
        assert_eq!(sel.anchor, 3);
        assert_eq!(sel.head, 8);
        assert_eq!(sel.range(), 3..8);
    }

    #[test]
    fn default_is_caret_at_origin() {
        let sel = RichTextSelection::default();
        assert!(sel.is_collapsed());
        assert_eq!(sel.anchor, 0);
        assert_eq!(sel.head, 0);
    }
}
