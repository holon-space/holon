//! `RichTextEditor` — GPUI view that paints `(text, marks)` with styled
//! runs, a caret, and click-to-place / drag-to-select selection.
//!
//! Builds on Phase 0.2's bare-GPUI spike (`frontends/gpui/examples/rich_input_spike.rs`),
//! Phase 2's mark vocabulary, and Phase 3 primitives:
//! - `crate::render::rich_text_runs::marks_to_text_runs` for the paint pass
//! - `holon_frontend::rich_text_selection::RichTextSelection` for caret/selection state
//! - Phase 3.1a Loro cursor helpers (consumed by the embedding view, not directly here)
//!
//! # Scope of this scaffold
//!
//! This file delivers the **paint + selection** half of the editor:
//! - `RichTextEditor` entity with text/marks/selection/focus
//! - `Element` impl that shapes runs and paints the line + caret
//! - Mouse click → `RichTextSelection::move_to(scalar)` via
//!   `WrappedLine::closest_index_for_position`
//! - `EntityInputHandler` stubs sufficient for IME registration
//!
//! Explicitly **NOT** in this scaffold (subsequent chunks):
//! - Keyboard editing — needs async dispatch through
//!   `MarkOperations::apply_mark` and `LoroBackend::insert_text`
//! - Selection highlight rendering (visual polish on top of the caret)
//! - Replacing the existing `EditorView` — that's a separate integration
//!   that wires the new editor into `editable_text` builder paths

use std::ops::Range;

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    fill, px, rgb, size, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, MouseDownEvent, ParentElement, Pixels, Point, SharedString, TextAlign,
    UTF16Selection, Window, WrappedLine, WrappedLineLayout,
};

use holon::core::datasource::{__operations_mark_operations, __operations_text_operations};
use holon_api::{InlineMark, MarkSpan};
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::rich_text_selection::RichTextSelection;

use crate::render::rich_text_runs::{marks_to_text_runs, scalar_range_to_bytes, RichTextStyle};

/// State backing the rich-text editor view.
///
/// Mutations go through the operation pipeline (`TextOperations`,
/// `MarkOperations`); the editor builds `OperationIntent`s and dispatches
/// them via the optional `BuilderServices` handle. This separation keeps
/// the view free of direct backend access and lets it be unit-tested by
/// inspecting the produced intents (no `BuilderServices` needed in tests).
pub struct RichTextEditor {
    /// Block id for downstream wiring (operation intents target this).
    block_id: String,
    text: SharedString,
    marks: Vec<MarkSpan>,
    selection: RichTextSelection,
    style: RichTextStyle,
    line_height: Pixels,
    focus: FocusHandle,
    /// Optional dispatch handle. `None` means intents are built but not
    /// dispatched (tests, headless contexts). `Some` means real keystrokes
    /// flow through the standard operation pipeline.
    services: Option<Arc<dyn BuilderServices>>,
}

impl RichTextEditor {
    pub fn new(
        block_id: String,
        text: impl Into<SharedString>,
        marks: Vec<MarkSpan>,
        style: RichTextStyle,
        focus: FocusHandle,
    ) -> Self {
        Self {
            block_id,
            text: text.into(),
            marks,
            selection: RichTextSelection::default(),
            style,
            line_height: px(20.0),
            focus,
            services: None,
        }
    }

    /// Wire dispatch — supplied once after construction by the embedding view.
    pub fn set_services(&mut self, services: Arc<dyn BuilderServices>) {
        self.services = Some(services);
    }

    /// Build (and optionally dispatch) the `apply_mark` intent for the
    /// current selection. Returns the intent so tests can inspect it; the
    /// real flow side-effects via `services.dispatch_intent`.
    pub fn apply_mark_to_selection(&self, mark: &InlineMark) -> OperationIntent {
        let range = self.selection.range();
        let mark_json = serde_json::to_string(mark).expect("InlineMark serialization is total");
        let intent: OperationIntent = __operations_mark_operations::apply_mark_op(
            "block",
            &self.block_id,
            range.start as i64,
            range.end as i64,
            mark_json,
        )
        .into();
        self.maybe_dispatch(&intent);
        intent
    }

    /// Companion to `apply_mark_to_selection` for unmarking.
    pub fn remove_mark_from_selection(&self, mark: &InlineMark) -> OperationIntent {
        let range = self.selection.range();
        let intent: OperationIntent = __operations_mark_operations::remove_mark_op(
            "block",
            &self.block_id,
            range.start as i64,
            range.end as i64,
            mark.loro_key().to_string(),
        )
        .into();
        self.maybe_dispatch(&intent);
        intent
    }

    /// Insert `text` at the caret. If the selection is non-empty, the
    /// covered range is deleted first (the standard "type-replaces-selection"
    /// editor contract). Returns a `Vec<OperationIntent>` because a
    /// non-empty selection produces both a `delete_text` and an
    /// `insert_text` intent — callers / tests inspect the full sequence.
    pub fn insert_at_caret(&self, text: &str) -> Vec<OperationIntent> {
        let range = self.selection.range();
        let mut intents = Vec::with_capacity(2);
        if !range.is_empty() {
            intents.push(self.delete_text_intent(range.start, range.end - range.start));
        }
        intents.push(self.insert_text_intent(range.start, text));
        for intent in &intents {
            self.maybe_dispatch(intent);
        }
        intents
    }

    /// Backspace at caret — delete one scalar to the left. With a non-empty
    /// selection, deletes the selection instead. Returns the intent (`None`
    /// if there is nothing to delete: caret at position 0 with empty
    /// selection).
    pub fn delete_backward(&self) -> Option<OperationIntent> {
        let range = self.selection.range();
        let intent = if !range.is_empty() {
            Some(self.delete_text_intent(range.start, range.end - range.start))
        } else if range.start > 0 {
            Some(self.delete_text_intent(range.start - 1, 1))
        } else {
            None
        };
        if let Some(intent) = intent.as_ref() {
            self.maybe_dispatch(intent);
        }
        intent
    }

    fn maybe_dispatch(&self, intent: &OperationIntent) {
        if let Some(services) = self.services.as_ref() {
            services.dispatch_intent(intent.clone());
        }
    }

    fn insert_text_intent(&self, pos: usize, text: &str) -> OperationIntent {
        __operations_text_operations::insert_text_op(
            "block",
            &self.block_id,
            pos as i64,
            text.to_string(),
        )
        .into()
    }

    fn delete_text_intent(&self, pos: usize, len: usize) -> OperationIntent {
        __operations_text_operations::delete_text_op(
            "block",
            &self.block_id,
            pos as i64,
            len as i64,
        )
        .into()
    }

    /// Replace the text + marks payload (typically called when the underlying
    /// block updates via CDC). Selection is clamped to the new text length
    /// so an end-of-text caret survives a buffer shrink.
    pub fn set_payload(&mut self, text: impl Into<SharedString>, marks: Vec<MarkSpan>) {
        self.text = text.into();
        self.marks = marks;
        self.selection.clamp_to(scalar_count(&self.text));
    }

    pub fn block_id(&self) -> &str {
        &self.block_id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn marks(&self) -> &[MarkSpan] {
        &self.marks
    }

    pub fn selection(&self) -> &RichTextSelection {
        &self.selection
    }

    pub fn set_selection(&mut self, sel: RichTextSelection) {
        self.selection = sel;
    }

    /// Translate a byte offset (from gpui hit-testing) into a Unicode-scalar
    /// offset, the unit used for `RichTextSelection`. gpui's
    /// `closest_index_for_position` returns byte indices on UTF-8 boundaries,
    /// so this is well-defined.
    pub fn byte_to_scalar(&self, byte_offset: usize) -> usize {
        let text: &str = &self.text;
        if byte_offset >= text.len() {
            return scalar_count(&self.text);
        }
        text[..byte_offset].chars().count()
    }

    /// Inverse of `byte_to_scalar`.
    pub fn scalar_to_byte(&self, scalar_offset: usize) -> usize {
        let total = scalar_count(&self.text);
        if scalar_offset >= total {
            return self.text.len();
        }
        self.text
            .char_indices()
            .nth(scalar_offset)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.text.len())
    }

    /// Build the `Vec<TextRun>` for the current text + marks.
    pub fn build_runs(&self) -> Vec<gpui::TextRun> {
        marks_to_text_runs(&self.text, &self.marks, &self.style)
    }

    /// Selection range as **byte** offsets — the form gpui's text APIs want.
    pub fn selection_byte_range(&self) -> Range<usize> {
        let chars = self.selection.range();
        scalar_range_to_bytes(&self.text, chars)
    }
}

fn scalar_count(s: &str) -> usize {
    s.chars().count()
}

impl Focusable for RichTextEditor {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// IME / text-input integration point. Method bodies are stubs in this
/// scaffold — the editor view registers as the input target so platform
/// IME plumbing is wired, but actual edits go through the operation
/// pipeline in a subsequent chunk.
impl EntityInputHandler for RichTextEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        self.text.get(range).map(|s| s.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let bytes = self.selection_byte_range();
        Some(UTF16Selection {
            range: bytes,
            reversed: self.selection.head < self.selection.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // gpui hands us a UTF-8 byte range. If `None`, the IME wants us to
        // replace the current selection. Translate to Unicode-scalar offsets
        // and dispatch through the operation pipeline.
        let (scalar_start, scalar_end) = match range {
            Some(r) => (self.byte_to_scalar(r.start), self.byte_to_scalar(r.end)),
            None => {
                let r = self.selection.range();
                (r.start, r.end)
            }
        };
        // Equivalent of insert_at_caret with explicit range — bypasses the
        // selection field so IME can target arbitrary spans (composition
        // edits, marked-text replacement) without first mutating selection.
        if scalar_end > scalar_start {
            let intent = self.delete_text_intent(scalar_start, scalar_end - scalar_start);
            self.maybe_dispatch(&intent);
        }
        if !new_text.is_empty() {
            let intent = self.insert_text_intent(scalar_start, new_text);
            self.maybe_dispatch(&intent);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.selection.head)
    }
}

impl Render for RichTextEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        gpui::div()
            .key_context("RichTextEditor")
            .track_focus(&self.focus)
            .size_full()
            .child(EditorPaint { state: entity })
    }
}

/// The painted Element that hosts shape_text + paint + mouse handling.
/// Cloned per render pass; cheap because `Entity` is reference-counted.
struct EditorPaint {
    state: Entity<RichTextEditor>,
}

struct PrepaintData {
    line: Option<WrappedLine>,
}

impl IntoElement for EditorPaint {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorPaint {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintData;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspect_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let line_height = self.state.read(cx).line_height;
        let style = gpui::Style {
            size: size(
                gpui::relative(1.0).into(),
                gpui::DefiniteLength::from(line_height).into(),
            ),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspect_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Self::PrepaintState {
        let state = self.state.read(cx);
        let text = state.text.clone();
        let runs = state.build_runs();
        if text.is_empty() {
            return PrepaintData { line: None };
        }
        let mut wrapped = window
            .text_system()
            .shape_text(text, px(14.0), &runs, None, None)
            .expect("shape_text failed");
        let line = wrapped.drain(..).next();
        PrepaintData { line }
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspect_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        let line_height = self.state.read(cx).line_height;
        let focus = self.state.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );

        // Take the line out of prepaint by value. This avoids the
        // `&WrappedLine` ↔ `Clone-via-Deref` ambiguity that bites when the
        // closure below captures a clone — on owned `WrappedLine`, method
        // resolution auto-derefs to `Arc::clone` (returning
        // `Arc<WrappedLineLayout>`), which is what the closure needs.
        let Some(line) = prepaint.line.take() else {
            return;
        };
        line.paint(
            bounds.origin,
            line_height,
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .expect("paint line");

        // Caret. Translate the selection's head from scalar→byte to feed
        // gpui's byte-based position_for_index.
        let (head_scalar, has_focus) = {
            let s = self.state.read(cx);
            (s.selection.head, s.focus.is_focused(window))
        };
        if has_focus {
            let head_byte = self.state.read(cx).scalar_to_byte(head_scalar);
            if let Some(caret_pos) = line.position_for_index(head_byte, line_height) {
                let caret_origin = bounds.origin + caret_pos;
                let caret_bounds = Bounds::new(caret_origin, size(px(2.0), line_height));
                window.paint_quad(fill(caret_bounds, rgb(0xffffff)));
            }
        }

        // Mouse: click → set caret to byte-offset under cursor → translate
        // to scalar offset → update selection state.
        let layout_for_mouse: Arc<WrappedLineLayout> = line.clone();
        let state_handle = self.state.clone();
        let bounds_for_mouse = bounds;
        window.on_mouse_event(move |event: &MouseDownEvent, _phase, window, cx| {
            if !bounds_for_mouse.contains(&event.position) {
                return;
            }
            let local = event.position - bounds_for_mouse.origin;
            let byte_idx = layout_for_mouse
                .closest_index_for_position(local, line_height)
                .unwrap_or_else(|i| i);
            state_handle.update(cx, |editor, cx| {
                let scalar = editor.byte_to_scalar(byte_idx);
                editor.selection.move_to(scalar);
                window.focus(&editor.focus.clone(), cx);
                cx.notify();
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::InlineMark;

    fn test_style() -> RichTextStyle {
        RichTextStyle {
            default_font: gpui::font(".SystemUIFont"),
            default_color: gpui::Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.9,
                a: 1.0,
            },
            muted_bg: gpui::Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.2,
                a: 1.0,
            },
            code_color: gpui::Hsla {
                h: 0.4,
                s: 0.5,
                l: 0.7,
                a: 1.0,
            },
            link_color: gpui::Hsla {
                h: 0.6,
                s: 0.7,
                l: 0.6,
                a: 1.0,
            },
        }
    }

    /// Build an editor state for tests. We can't construct a real
    /// `FocusHandle` outside a gpui App, but the helpers under test only
    /// touch text/marks/selection — the focus field is dead-weight for them.
    /// Skipping focus-dependent paths in test by using a Drop'able ManuallyDrop
    /// shim isn't worth it; we test only the `pub fn`s that don't read focus.
    /// The caret/byte-scalar helpers ARE reachable via a custom helper that
    /// sidesteps `RichTextEditor::new`'s focus arg requirement — see below.
    fn editor_no_focus(text: &'static str, marks: Vec<MarkSpan>) -> EditorTestProbe {
        EditorTestProbe {
            text: text.into(),
            marks,
            selection: RichTextSelection::default(),
            style: test_style(),
        }
    }

    /// Mirror of `RichTextEditor`'s focus-free fields, exposing the same
    /// scalar/byte conversion + run-build helpers for tests.
    struct EditorTestProbe {
        text: SharedString,
        marks: Vec<MarkSpan>,
        selection: RichTextSelection,
        style: RichTextStyle,
    }

    impl EditorTestProbe {
        fn byte_to_scalar(&self, byte_offset: usize) -> usize {
            let text: &str = &self.text;
            if byte_offset >= text.len() {
                return text.chars().count();
            }
            text[..byte_offset].chars().count()
        }
        fn scalar_to_byte(&self, scalar_offset: usize) -> usize {
            let total = self.text.chars().count();
            if scalar_offset >= total {
                return self.text.len();
            }
            self.text
                .char_indices()
                .nth(scalar_offset)
                .map(|(i, _)| i)
                .unwrap_or_else(|| self.text.len())
        }
        fn build_runs(&self) -> Vec<gpui::TextRun> {
            marks_to_text_runs(&self.text, &self.marks, &self.style)
        }
        fn selection_byte_range(&self) -> Range<usize> {
            scalar_range_to_bytes(&self.text, self.selection.range())
        }
    }

    #[test]
    fn byte_scalar_round_trip_ascii() {
        let e = editor_no_focus("hello", vec![]);
        for i in 0..=5 {
            assert_eq!(e.byte_to_scalar(e.scalar_to_byte(i)), i);
        }
    }

    #[test]
    fn byte_scalar_round_trip_multibyte() {
        // "a你好b" — scalars [0..4], bytes [0..8]
        let e = editor_no_focus("a你好b", vec![]);
        // Scalar 0 = byte 0, scalar 1 = byte 1, scalar 2 = byte 4,
        // scalar 3 = byte 7, scalar 4 = byte 8 (end).
        assert_eq!(e.scalar_to_byte(0), 0);
        assert_eq!(e.scalar_to_byte(1), 1);
        assert_eq!(e.scalar_to_byte(2), 4);
        assert_eq!(e.scalar_to_byte(3), 7);
        assert_eq!(e.scalar_to_byte(4), 8);
        assert_eq!(e.byte_to_scalar(0), 0);
        assert_eq!(e.byte_to_scalar(1), 1);
        assert_eq!(e.byte_to_scalar(4), 2);
        assert_eq!(e.byte_to_scalar(7), 3);
        assert_eq!(e.byte_to_scalar(8), 4);
    }

    #[test]
    fn build_runs_partition_text() {
        let e = editor_no_focus("hello world", vec![MarkSpan::new(0, 5, InlineMark::Bold)]);
        let runs = e.build_runs();
        assert_eq!(runs.len(), 2);
        let total: usize = runs.iter().map(|r| r.len).sum();
        assert_eq!(total, "hello world".len());
    }

    #[test]
    fn selection_byte_range_caret_default() {
        let e = editor_no_focus("hello", vec![]);
        assert_eq!(e.selection_byte_range(), 0..0);
    }

    #[test]
    fn selection_byte_range_with_multibyte() {
        // "a你好b" — selection [1..3) chars covers "你好" → bytes [1..7)
        let mut e = editor_no_focus("a你好b", vec![]);
        e.selection = RichTextSelection::span(1, 3);
        assert_eq!(e.selection_byte_range(), 1..7);
    }

    #[test]
    fn selection_byte_range_backward_sorts() {
        // Backward selection (head < anchor): range() sorts.
        let mut e = editor_no_focus("hello", vec![]);
        e.selection = RichTextSelection::span(4, 1);
        assert_eq!(e.selection_byte_range(), 1..4);
    }

    #[test]
    fn byte_to_scalar_at_boundaries() {
        let e = editor_no_focus("ab", vec![]);
        assert_eq!(e.byte_to_scalar(0), 0);
        assert_eq!(e.byte_to_scalar(1), 1);
        assert_eq!(e.byte_to_scalar(2), 2);
        assert_eq!(e.byte_to_scalar(99), 2, "past-end clamps to scalar count");
    }

    use holon_api::types::EntityName;
    use holon_api::Value;

    /// Smoke-test that the macro-generated `*_op()` constructors produce
    /// the right `(entity_name, op_name, params)` for the editor's
    /// IME / shortcut paths. The conversion `Operation -> OperationIntent`
    /// just drops `display_name`; everything else is identity.
    #[test]
    fn apply_mark_op_round_trips_through_intent() {
        let mark_json = serde_json::to_string(&InlineMark::Bold).unwrap();
        let op = __operations_mark_operations::apply_mark_op(
            "block",
            "block:abc",
            2,
            5,
            mark_json.clone(),
        );
        let intent: OperationIntent = op.into();
        assert_eq!(intent.entity_name, EntityName::new("block"));
        assert_eq!(intent.op_name, "apply_mark");
        assert_eq!(intent.params["id"], Value::String("block:abc".into()));
        assert_eq!(intent.params["range_start"], Value::Integer(2));
        assert_eq!(intent.params["range_end"], Value::Integer(5));
        let parsed: InlineMark =
            serde_json::from_str(intent.params["mark_json"].as_string().unwrap()).unwrap();
        assert_eq!(parsed, InlineMark::Bold);
    }

    #[test]
    fn remove_mark_op_uses_loro_key() {
        let op = __operations_mark_operations::remove_mark_op(
            "block",
            "block:abc",
            2,
            5,
            InlineMark::Italic.loro_key().to_string(),
        );
        let intent: OperationIntent = op.into();
        assert_eq!(intent.op_name, "remove_mark");
        assert_eq!(intent.params["key"], Value::String("italic".into()));
        assert!(!intent.params.contains_key("mark_json"));
    }

    #[test]
    fn insert_text_op_carries_pos_and_text() {
        let op = __operations_text_operations::insert_text_op(
            "block",
            "block:xyz",
            7,
            "hello".to_string(),
        );
        let intent: OperationIntent = op.into();
        assert_eq!(intent.op_name, "insert_text");
        assert_eq!(intent.params["id"], Value::String("block:xyz".into()));
        assert_eq!(intent.params["pos"], Value::Integer(7));
        assert_eq!(intent.params["text"], Value::String("hello".into()));
    }

    #[test]
    fn delete_text_op_carries_pos_and_len() {
        let op = __operations_text_operations::delete_text_op("block", "block:xyz", 3, 4);
        let intent: OperationIntent = op.into();
        assert_eq!(intent.op_name, "delete_text");
        assert_eq!(intent.params["pos"], Value::Integer(3));
        assert_eq!(intent.params["len"], Value::Integer(4));
    }
}
