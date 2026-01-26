//! Renders a `ReactiveViewModel` tree into r3bl render ops.
//!
//! Counterpart of `frontends/gpui/src/render/builders/mod.rs`. The shadow
//! interpreter (in `holon-frontend`) has already turned the `RenderExpr` into
//! a `ReactiveViewModel`; this module walks that tree, reading the reactive
//! `expr`, `props`, `data` mutables and the `collection` `MutableVec` snapshot,
//! and emits r3bl `RenderOpIRVec` operations.

use holon_api::EntityUri;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::LayoutHint;
use holon_frontend::ReactiveViewModel;
use r3bl_tui::{
    col, new_style, render_tui_styled_texts_into, row, tui_color, tui_styled_text,
    tui_styled_texts, Pos, RenderOpCommon, RenderOpIRVec,
};
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

/// A single keyboard-navigable region discovered while walking the reactive
/// view model. Populated by [`render_selectable`] (for explicit
/// `selectable(action: ...)` widgets) and [`render_live_block`] (for any
/// block-resolving slot — main-panel blocks, sidebar items resolved through
/// `live_block`). The keyboard handler in `app_main` reads it to move the
/// focus marker (↑ / ↓) and to dispatch the bound action on Enter when
/// `intent` is `Some`.
///
/// `kind` lets `reconcile_focus` distinguish "first sidebar entry" from
/// "first main-panel block" so we can auto-jump focus to a new doc's first
/// block after the user activates a sidebar selectable.
///
/// `region` identifies which top-level `columns(...)` slot the entry lives
/// under (sidebar = 0, main panel = 1, drawer = 2 in the default layout) so
/// the keyboard handler can scope ↑/↓ to the active region and let Tab hop
/// between regions instead of cycling through every selectable on screen.
#[derive(Debug, Clone)]
pub struct SelectableRegion {
    /// Entity ID this region targets. Used as a stable key to keep focus
    /// pinned on the same row across re-renders, even when the registry's
    /// index order shifts.
    pub entity_id: String,
    /// Click intent baked at shadow-build time — same value GPUI dispatches
    /// on mouse-down. `None` for block regions that have no bound action
    /// (Enter on those is a no-op for now).
    pub intent: Option<OperationIntent>,
    pub kind: SelectableKind,
    /// Top-level columns slot index — set by the OUTERMOST `render_columns`
    /// only. Defaults to 0 if the tree never goes through a columns layout.
    pub region: usize,
    /// Editable text target inside this region's subtree (the first
    /// `editable_text` we find while walking the row). When the user presses
    /// Enter on a `Block` region, the keyboard handler reads this to seed
    /// an edit buffer. `None` means there's nothing to edit (e.g. a sidebar
    /// `selectable` whose action is a navigation_focus, not a text field).
    pub editable: Option<EditableTarget>,
    /// Origin (cell coords) where this region was painted. Combined with
    /// `rows`/`cols` and a fixed cell size, this gives the
    /// [`crate::geometry::TuiGeometry`] adapter the rectangle it needs to
    /// translate into pixel-space `ElementInfo`. Reserved slots have `rows = 0`
    /// until the inner render returns and the caller back-fills it.
    pub start_row: usize,
    pub start_col: usize,
    pub rows: usize,
    pub cols: usize,
    /// Widget kind at the registration site — `"selectable"` for explicit
    /// `selectable(action: ...)` widgets, `"live_block"` for tree/outline rows
    /// auto-registered by `render_collection_vertical`. Mirrors GPUI's
    /// `ElementInfo::widget_type` so PBT histograms read the same way for both
    /// frontends.
    pub widget_type: String,
    /// Live text the region puts on screen, when it has any. Pulled from the
    /// in-edit `EditableTarget` when present, else from the node's `content`
    /// prop. Drives the `inv-displayed-text` invariant + the
    /// `wait_for_geometry_ready` content gate.
    pub displayed_text: Option<String>,
}

/// Captures the `editable_text` props needed to enter edit mode for a Block:
/// which block to write back to, which field, and the current value to seed
/// the buffer with. Resolved at register time so the input handler doesn't
/// have to walk the tree on every Enter.
#[derive(Debug, Clone)]
pub struct EditableTarget {
    pub block_id: String,
    pub field: String,
    pub current_content: String,
}

/// Walk a row's view-model subtree looking for the first `editable_text`
/// widget. `row_block_id` is the block ID from the parent row — used as
/// fallback when the leaf `editable_text`'s own data doesn't carry an `id`
/// (column-bound widgets only see the column value, not the full row).
fn find_editable_target(
    node: &ReactiveViewModel,
    row_block_id: Option<&str>,
) -> Option<EditableTarget> {
    if node.widget_name().as_deref() == Some("editable_text") {
        let from_row = node.row_id();
        let from_entity = node.entity_id();
        let block_id = from_row
            .clone()
            .or_else(|| from_entity.clone())
            .or_else(|| row_block_id.map(|s| s.to_string()))?;
        tracing::trace!(
            "[find_editable_target] row_id={from_row:?} entity_id={from_entity:?} \
             fallback={row_block_id:?} → {block_id}"
        );
        let field = node
            .prop_str("field")
            .unwrap_or_else(|| "content".to_string());
        let current_content = node.prop_str("content").unwrap_or_default();
        return Some(EditableTarget {
            block_id,
            field,
            current_content,
        });
    }
    if let Some(view) = node.collection.as_ref() {
        for item in view.items.lock_ref().iter() {
            if let Some(t) = find_editable_target(item.as_ref(), row_block_id) {
                return Some(t);
            }
        }
    }
    for child in &node.children {
        if let Some(t) = find_editable_target(child.as_ref(), row_block_id) {
            return Some(t);
        }
    }
    if let Some(slot) = node.slot.as_ref() {
        let inner = slot.content.lock_ref().clone();
        if let Some(t) = find_editable_target(inner.as_ref(), row_block_id) {
            return Some(t);
        }
    }
    tracing::trace!(
        "[find_editable_target] not found under '{}' (children={} collection={} slot={})",
        node.widget_name().unwrap_or_default(),
        node.children.len(),
        node.collection
            .as_ref()
            .map_or(0, |c| c.items.lock_ref().len()),
        node.slot.is_some(),
    );
    None
}

/// Differentiates explicit user-clickable selectables from block-row
/// regions auto-registered by `render_live_block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectableKind {
    /// `selectable(action: ...)` widget — has a click intent.
    Selectable,
    /// `live_block` slot — a navigable doc-block row; no click intent today.
    Block,
}

/// Registry of selectable regions discovered during a render pass.
#[derive(Debug, Default, Clone)]
pub struct RenderRegistry {
    pub selectables: Vec<SelectableRegion>,
}

/// Per-render context bundling the engine handle, the registry the walk
/// populates, and the keyboard focus index. Threaded through every render
/// helper so `selectable` registrations and focus highlighting stay in sync.
pub struct RenderCtx<'a> {
    pub engine: &'a Arc<ReactiveEngine>,
    pub registry: &'a mut RenderRegistry,
    /// Index of the currently keyboard-focused selectable, if any. Compared
    /// against `registry.selectables.len()` _before_ pushing the current
    /// selectable, so it indexes the order in which `render_selectable` is
    /// called.
    pub focus_index: Option<usize>,
    /// Region index assigned by the outermost `render_columns`. `None` until
    /// the walk enters the first columns layout; once set, nested
    /// `render_columns` calls leave it alone so a nested grid inside one
    /// region still tags its descendants with the parent region.
    pub region: Option<usize>,
    /// Live snapshot of the inline edit buffer, when active. The renderer
    /// looks at it inside `editable_text` to swap the prop-derived content
    /// for the user's in-progress text + cursor highlight.
    pub edit: Option<EditView>,
    /// Recursion depth through `render_live_block`. The reactive engine's
    /// `snapshot_reactive` resolves the inner block; if that snapshot is
    /// itself a `live_block` referencing the same id (or a chain that
    /// loops back), we recurse forever. Bumped on entry, decremented on
    /// exit, hard-capped by [`MAX_LIVE_BLOCK_DEPTH`] which paints a
    /// "Recursive block" placeholder and unwinds.
    pub live_block_depth: usize,
}

/// Max recursion through `render_live_block`. Real layouts go at most
/// 3-4 levels deep (root → sidebar wrapper → live_block → content); 8
/// leaves room for unusual nesting without permitting accidental infinite
/// loops.
pub const MAX_LIVE_BLOCK_DEPTH: usize = 8;

/// Read-only view of the in-progress edit buffer for the renderer. We
/// deliberatelly don't expose mutability here — input handling lives in
/// `app_main` and this struct is just so the renderer can paint what the
/// user is typing.
#[derive(Debug, Clone)]
pub struct EditView {
    pub block_id: String,
    pub field: String,
    pub buffer: String,
    pub cursor: usize,
}

impl<'a> RenderCtx<'a> {
    pub fn new(
        engine: &'a Arc<ReactiveEngine>,
        registry: &'a mut RenderRegistry,
        focus_index: Option<usize>,
    ) -> Self {
        Self {
            engine,
            registry,
            focus_index,
            region: None,
            edit: None,
            live_block_depth: 0,
        }
    }

    pub fn with_edit(mut self, edit: Option<EditView>) -> Self {
        self.edit = edit;
        self
    }

    fn current_region(&self) -> usize {
        self.region.unwrap_or(0)
    }
}

/// Pixel-to-cell heuristic for translating GPUI logical pixels into terminal
/// columns when honouring `LayoutHint::Fixed { px }`. Roughly the width of one
/// monospace glyph at 14px in GPUI; tweak if drawers look too narrow/wide.
const PX_PER_CELL: f32 = 10.0;

/// Convert a logical-pixel gap (as authored for GPUI) into a terminal cell
/// count. Any non-zero gap rounds up to at least 1 cell, so a `gap: 4` in the
/// DSL still produces a visible column separator instead of vanishing under
/// the px-to-cell ratio.
fn gap_px_to_cells(gap_px: f32) -> usize {
    if gap_px <= 0.0 {
        0
    } else {
        ((gap_px / PX_PER_CELL).ceil() as usize).max(1)
    }
}

/// Names of widgets the TUI knows how to render.
///
/// Used by `render_supported_widgets()` to participate in profile-variant
/// filtering on the backend. Anything outside this set will still render
/// (we fall back to drawing children/collection), but the backend may strip
/// it before we ever see it.
pub const TUI_SUPPORTED_WIDGETS: &[&str] = &[
    "text",
    "row",
    "column",
    "columns",
    "list",
    "section",
    "tree",
    "table",
    "outline",
    "checkbox",
    "badge",
    "icon",
    "spacer",
    "block_ref",
    "block",
    "live_block",
    "live_query",
    "render_block",
    "render_entity",
    "focusable",
    "selectable",
    "editable_text",
    "pref_field",
    "error",
    "loading",
    "state_toggle",
    "bullet",
    "drop_zone",
    "draggable",
    "pie_menu",
];

/// Walks `node` and emits render ops, returning the number of rows consumed.
///
/// `start_row` / `start_col` are absolute terminal coordinates. `max_width`
/// is the cell budget for the current row context (used for clipping).
///
/// `engine` is used to resolve `live_block` / `render_entity` slots by calling
/// `snapshot_reactive(block_id)`, which both kicks off a background watcher
/// (if not already running) and returns the current snapshot. The 100 Hz
/// ticker in `app_main` re-renders, so progressive data fills in over time.
pub fn render_view_model(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let name = node.widget_name().unwrap_or_default();
    match name.as_str() {
        "text" => render_text(node, ops, start_row, start_col, max_width),
        "editable_text" => render_editable_text(node, ctx, ops, start_row, start_col, max_width),
        "checkbox" => render_checkbox(node, ops, start_row, start_col),
        "badge" => render_badge(node, ops, start_row, start_col),
        "icon" => render_icon(node, ops, start_row, start_col),
        "spacer" => 1,
        "row" => render_row(node, ctx, ops, start_row, start_col, max_width),
        "column" => render_column(node, ctx, ops, start_row, start_col, max_width),
        "columns" => render_columns(node, ctx, ops, start_row, start_col, max_width),
        "list" | "section" | "tree" | "table" | "outline" => render_collection_vertical(
            node,
            ctx,
            ops,
            start_row,
            start_col,
            max_width,
            name.as_str(),
        ),
        "error" => render_error(node, ops, start_row, start_col, max_width),
        "pref_field" => render_pref_field(node, ops, start_row, start_col, max_width),
        "loading" => render_plain(node, ops, start_row, start_col, "Loading…"),
        "state_toggle" => render_state_toggle(node, ops, start_row, start_col, max_width),
        "bullet" => render_plain(node, ops, start_row, start_col, "•"),
        "drop_zone" => 0,
        // live_block / render_entity carry their content in a slot that the
        // frontend has to fill via a sub-watch. Resolve the inner block via
        // `snapshot_reactive` (which also ensures the watcher is running).
        "live_block" | "render_entity" | "block_ref" | "block" | "render_block" => {
            render_live_block(node, ctx, ops, start_row, start_col, max_width)
        }
        "selectable" => render_selectable(node, ctx, ops, start_row, start_col, max_width),
        // Other transparent wrappers — recurse into existing children/collection/slot.
        "live_query" | "focusable" => {
            render_passthrough(node, ctx, ops, start_row, start_col, max_width)
        }
        // Anything else: render children + collection if present, otherwise text.
        _ => render_passthrough(node, ctx, ops, start_row, start_col, max_width),
    }
}

fn render_live_block(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let Some(block_id_str) = node.prop_str("block_id") else {
        return render_passthrough(node, ctx, ops, start_row, start_col, max_width);
    };
    if ctx.live_block_depth >= MAX_LIVE_BLOCK_DEPTH {
        tracing::warn!(
            "live_block recursion exceeded depth {} at {} — likely cycle",
            MAX_LIVE_BLOCK_DEPTH,
            block_id_str
        );
        return render_plain(node, ops, start_row, start_col, "Recursive block");
    }
    let uri = EntityUri::parse(&block_id_str).unwrap_or_else(|_| EntityUri::block(&block_id_str));
    let inner = ctx.engine.snapshot_reactive(&uri);
    let inner_name = inner.widget_name().unwrap_or_default();
    if inner_name == "empty" || inner_name == "loading" {
        return render_plain(node, ops, start_row, start_col, "Loading…");
    }

    // Per-row block registration happens at the `tree`/`outline` collection
    // boundary (see `render_collection_vertical`); per-row `render_entity()`
    // is inlined and no `live_block` wrapper survives. The 3 layout
    // wrappers (`block:default-left-sidebar`, `…-main-panel`, `…-right-sidebar`)
    // that DO hit this path are containers, not user-facing rows — skip
    // registration.
    let _ = block_id_str;
    ctx.live_block_depth += 1;
    let rows = render_view_model(&inner, ctx, ops, start_row, start_col, max_width);
    ctx.live_block_depth -= 1;
    rows
}

/// Renders a `selectable` wrapper. Captures the click intent (for keyboard
/// activation) and registers an entry in `ctx.registry`. If the registered
/// index matches `ctx.focus_index`, paints a `►` marker before the child
/// and shifts the child two cells to the right.
///
/// Falls through to `render_passthrough` when the node has no click intent —
/// `selectable(...)` without an `action:` arg is a no-op visually.
fn render_selectable(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let Some(intent) = node.click_intent() else {
        return render_passthrough(node, ctx, ops, start_row, start_col, max_width);
    };
    let entity_id = node
        .row_id()
        .or_else(|| node.entity_id())
        .unwrap_or_default();

    let idx = ctx.registry.selectables.len();
    let is_focused = ctx.focus_index == Some(idx);
    let region = ctx.current_region();
    // Sidebar `selectable(action: ...)` widgets dispatch a navigation_focus
    // intent on Enter — we don't try to edit them inline.
    let displayed_text = node.prop_str("content");
    ctx.registry.selectables.push(SelectableRegion {
        entity_id,
        intent: Some(intent),
        kind: SelectableKind::Selectable,
        region,
        editable: None,
        start_row,
        start_col,
        rows: 0,
        cols: max_width,
        widget_type: "selectable".into(),
        displayed_text,
    });

    let rows_consumed = if is_focused {
        let prefix_width = 2.min(max_width);
        if prefix_width > 0 {
            *ops +=
                RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
            let texts = tui_styled_texts! {
                tui_styled_text! {
                    @style: new_style!(bold color_fg: {tui_color!(hex "#FFCC00")} color_bg: {tui_color!(hex "#333333")}),
                    @text: "►"
                },
            };
            render_tui_styled_texts_into(&texts, ops);
        }
        let child_col = start_col + prefix_width;
        let child_width = max_width.saturating_sub(prefix_width);
        if child_width == 0 {
            ctx.registry.selectables[idx].rows = 1;
            return 1;
        }
        render_passthrough(node, ctx, ops, start_row, child_col, child_width)
    } else {
        render_passthrough(node, ctx, ops, start_row, start_col, max_width)
    };
    ctx.registry.selectables[idx].rows = rows_consumed;
    rows_consumed
}

/// Renders an `editable_text` widget. When `ctx.edit` is `Some` and points
/// at this row, paints the in-progress edit buffer with a cursor highlight
/// instead of the prop-derived `content`. Otherwise behaves identically to
/// `render_text`.
fn render_editable_text(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let row_id = node.row_id().or_else(|| node.entity_id());
    let edit = ctx
        .edit
        .as_ref()
        .filter(|e| row_id.as_deref() == Some(e.block_id.as_str()));

    if let Some(edit) = edit {
        return render_edit_buffer(edit, ops, start_row, start_col, max_width);
    }
    render_text(node, ops, start_row, start_col, max_width)
}

/// Paints the edit buffer and a single-cell cursor highlight at the buffer's
/// `cursor` byte offset. Multibyte-safe: we walk graphemes and stop on the
/// one whose start-byte offset matches the cursor.
///
/// When the buffer is longer than `max_width`, scrolls horizontally so the
/// cursor stays inside the viewport. A trailing `›` indicator paints when
/// content is hidden to the right; a leading `‹` (eating the first column)
/// signals content scrolled off the left.
fn render_edit_buffer(
    edit: &EditView,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    if max_width == 0 {
        return 1;
    }

    // Walk graphemes once to record (byte_offset, char_count) so we can map
    // the byte cursor to a grapheme column and back.
    let graphemes: Vec<(usize, &str)> = edit
        .buffer
        .grapheme_indices(true)
        .map(|(b, g)| (b, g))
        .collect();
    let total_cols = graphemes.len();
    let cursor_col = graphemes
        .iter()
        .position(|(b, _)| *b >= edit.cursor)
        .unwrap_or(total_cols);

    // Choose a scroll offset that keeps the cursor visible. We reserve one
    // cell for the right-side `›` when there's hidden content, and one cell
    // for the left-side `‹` when scrolled past the start. With max_width
    // ≤ 2 we skip indicators since there's no room for any content.
    let mut scroll_start = 0usize;
    if cursor_col < scroll_start {
        scroll_start = cursor_col;
    }
    // Recompute `viewport_width` reflecting indicator reservations. Done
    // iteratively because the indicators only appear once we've scrolled.
    let mut viewport_width = max_width;
    let mut left_indicator;
    let mut right_indicator;
    loop {
        left_indicator = scroll_start > 0;
        let visible_end = scroll_start + viewport_width;
        right_indicator = visible_end < total_cols.saturating_add(1); // +1 for end-cursor
        let lhs = if left_indicator && max_width > 1 {
            1
        } else {
            0
        };
        let rhs = if right_indicator && max_width > 1 {
            1
        } else {
            0
        };
        let new_viewport = max_width.saturating_sub(lhs + rhs);
        if new_viewport == viewport_width {
            break;
        }
        viewport_width = new_viewport;
        // After shrinking the viewport, the cursor may now fall outside; nudge
        // scroll_start to bring it back.
        if cursor_col >= scroll_start + viewport_width {
            scroll_start = cursor_col + 1 - viewport_width;
        } else if cursor_col < scroll_start {
            scroll_start = cursor_col;
        }
    }
    // Ensure cursor is in window after the loop (covers initial cursor past
    // the right edge before any scrolling was decided).
    if cursor_col >= scroll_start + viewport_width {
        scroll_start = (cursor_col + 1).saturating_sub(viewport_width);
    }

    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let fg = tui_color!(hex "#FFFFFF");
    let cursor_bg = tui_color!(hex "#FFCC00");
    let cursor_fg = tui_color!(hex "#000000");
    let dim_fg = tui_color!(hex "#888888");

    if left_indicator && max_width > 1 {
        let texts = tui_styled_texts! {
            tui_styled_text! { @style: new_style!(color_fg: {dim_fg}), @text: "‹" },
        };
        render_tui_styled_texts_into(&texts, ops);
    }

    let mut painted_cells = 0usize;
    let mut painted_cursor = false;
    for (col_idx, (byte_pos, g)) in graphemes.iter().enumerate() {
        if col_idx < scroll_start {
            continue;
        }
        if painted_cells >= viewport_width {
            break;
        }
        if *byte_pos == edit.cursor {
            let texts = tui_styled_texts! {
                tui_styled_text! {
                    @style: new_style!(bold color_fg: {cursor_fg} color_bg: {cursor_bg}),
                    @text: *g
                },
            };
            render_tui_styled_texts_into(&texts, ops);
            painted_cursor = true;
        } else {
            let texts = tui_styled_texts! {
                tui_styled_text! { @style: new_style!(color_fg: {fg}), @text: *g },
            };
            render_tui_styled_texts_into(&texts, ops);
        }
        painted_cells += 1;
    }
    // Cursor at end-of-buffer — render a trailing block so the user can see
    // where the next char will go (only when the end position is inside
    // the viewport).
    if !painted_cursor
        && painted_cells < viewport_width
        && cursor_col >= scroll_start
        && cursor_col == total_cols
    {
        let texts = tui_styled_texts! {
            tui_styled_text! {
                @style: new_style!(bold color_fg: {cursor_fg} color_bg: {cursor_bg}),
                @text: " "
            },
        };
        render_tui_styled_texts_into(&texts, ops);
    }

    if right_indicator && max_width > 1 {
        let texts = tui_styled_texts! {
            tui_styled_text! { @style: new_style!(color_fg: {dim_fg}), @text: "›" },
        };
        render_tui_styled_texts_into(&texts, ops);
    }
    1
}

fn render_text(
    node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let content = node.prop_str("content").unwrap_or_default();
    let bold = node.prop_bool("bold").unwrap_or(false);
    if content.is_empty() {
        return 1;
    }
    let lines: Vec<&str> = content.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let clipped = clip_to_width(line, max_width);
        *ops +=
            RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row + i))));
        let fg = tui_color!(hex "#CCCCCC");
        let style = if bold {
            new_style!(bold color_fg: {fg})
        } else {
            new_style!(color_fg: {fg})
        };
        let texts = tui_styled_texts! {
            tui_styled_text! { @style: style, @text: clipped.as_str() },
        };
        render_tui_styled_texts_into(&texts, ops);
    }
    lines.len().max(1)
}

fn render_plain(
    _node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    text: &str,
) -> usize {
    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg: {tui_color!(hex "#888888")} dim),
            @text: text
        },
    };
    render_tui_styled_texts_into(&texts, ops);
    1
}

fn render_checkbox(
    node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
) -> usize {
    let checked = node.prop_bool("checked").unwrap_or(false);
    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let text = if checked { "[✓] " } else { "[ ] " };
    let fg = if checked {
        tui_color!(hex "#00FF00")
    } else {
        tui_color!(hex "#888888")
    };
    let texts = tui_styled_texts! {
        tui_styled_text! { @style: new_style!(color_fg: {fg}), @text: text },
    };
    render_tui_styled_texts_into(&texts, ops);
    1
}

fn render_badge(
    node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
) -> usize {
    let content = node.prop_str("content").unwrap_or_default();
    let label = node.prop_str("label").unwrap_or_default();
    let display_inner = if !content.is_empty() { content } else { label };
    let display = format!(" [{}] ", display_inner);
    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg: {tui_color!(hex "#FFFF00")} bold),
            @text: &display
        },
    };
    render_tui_styled_texts_into(&texts, ops);
    1
}

fn render_icon(
    node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
) -> usize {
    let symbol = node.prop_str("symbol").unwrap_or_else(|| "●".to_string());
    let display = format!("{} ", symbol);
    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg: {tui_color!(hex "#CCCCCC")}),
            @text: &display
        },
    };
    render_tui_styled_texts_into(&texts, ops);
    1
}

fn render_error(
    node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let message = node.prop_str("message").unwrap_or_default();
    let display = format!("⚠ {}", message);
    let clipped = clip_to_width(&display, max_width);
    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg: {tui_color!(hex "#FF6666")} bold),
            @text: clipped.as_str()
        },
    };
    render_tui_styled_texts_into(&texts, ops);
    1
}

fn render_state_toggle(
    node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let label = node.prop_str("label").unwrap_or_default();
    if label.is_empty() {
        return 0;
    }
    let clipped = clip_to_width(&label, max_width);
    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg: {tui_color!(hex "#FFCC00")} bold),
            @text: clipped.as_str()
        },
    };
    render_tui_styled_texts_into(&texts, ops);
    1
}

fn render_pref_field(
    node: &ReactiveViewModel,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let label = node.prop_str("label").unwrap_or_default();
    let current = node.prop_str("current").unwrap_or_default();
    let line = format!("{}: {}", label, current);
    let clipped = clip_to_width(&line, max_width);
    *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
    let texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg: {tui_color!(hex "#CCCCCC")}),
            @text: clipped.as_str()
        },
    };
    render_tui_styled_texts_into(&texts, ops);
    1
}

fn render_row(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let mut current_col = start_col;
    let mut max_rows = 1;
    let bound = start_col + max_width;
    let gap = node
        .prop_f64("gap")
        .map(|g| gap_px_to_cells(g as f32))
        .unwrap_or(0);

    let mut items: Vec<Arc<ReactiveViewModel>> = Vec::new();
    if let Some(view) = node.collection.as_ref() {
        items.extend(view.items.lock_ref().iter().cloned());
    }
    items.extend(node.children.iter().cloned());

    let mut rendered_any = false;
    for child in &items {
        if current_col >= bound {
            break;
        }
        if rendered_any && gap > 0 {
            current_col = (current_col + gap).min(bound);
            if current_col >= bound {
                break;
            }
        }
        let remaining = bound - current_col;
        let rows = render_view_model(child.as_ref(), ctx, ops, start_row, current_col, remaining);
        max_rows = max_rows.max(rows);
        current_col += estimate_width(child.as_ref()).min(remaining);
        rendered_any = true;
    }
    max_rows
}

fn render_column(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let mut current_row = start_row;
    // Vertical gap honoured between every advancing child. We use cells/2
    // for column gaps because terminal rows are roughly twice as tall as
    // they are wide — a `gap: 8` should feel similar vertically to
    // horizontally instead of blowing out the layout.
    let gap = node
        .prop_f64("gap")
        .map(|g| {
            if g <= 0.0 {
                0
            } else {
                ((g as f32 / (PX_PER_CELL * 2.0)).ceil() as usize).max(1)
            }
        })
        .unwrap_or(0);

    // Collection items always advance at least one row — they're the
    // visible rows the user sees. Static children (drop_zone, slots,
    // wrappers) are allowed to consume 0 so they don't introduce blank
    // gaps between collection items.
    let collection_items: Vec<Arc<ReactiveViewModel>> = node
        .collection
        .as_ref()
        .map(|view| view.items.lock_ref().iter().cloned().collect())
        .unwrap_or_default();
    let mut emitted_any = false;
    for item in &collection_items {
        if emitted_any && gap > 0 {
            current_row += gap;
        }
        let consumed =
            render_view_model(item.as_ref(), ctx, ops, current_row, start_col, max_width);
        current_row += consumed.max(1);
        emitted_any = true;
    }
    // Static children: drop_zones / wrappers / empty placeholders dominate
    // here, and they typically consume 0 rows. Skipping the gap for them
    // avoids introducing visual breaks before invisible items and matches
    // the original "0-consumed children take 0 rows" contract.
    let children: Vec<Arc<ReactiveViewModel>> = node.children.iter().cloned().collect();
    for child in &children {
        let consumed =
            render_view_model(child.as_ref(), ctx, ops, current_row, start_col, max_width);
        current_row += consumed;
    }
    current_row.saturating_sub(start_row)
}

/// Horizontal layout — slice `max_width` across children honouring their
/// `LayoutHint`. `Fixed { px }` children get `(px / PX_PER_CELL)` cells; the
/// remainder is shared by `Flex { weight }` children proportional to weight.
/// Items are taken from `collection` first, then static `children`.
///
/// `CollectionVariant::Columns { gap }` is honoured: between every pair of
/// non-empty slots we leave `gap_px_to_cells(gap)` blank cells so the panels
/// don't visually touch.
fn render_columns(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    // Snapshot all items as Arc<ReactiveViewModel> so we can release the
    // collection's lock_ref and own the slice for the rest of the function.
    let mut items: Vec<Arc<ReactiveViewModel>> = Vec::new();
    if let Some(view) = node.collection.as_ref() {
        items.extend(view.items.lock_ref().iter().cloned());
    }
    items.extend(node.children.iter().cloned());

    if items.is_empty() {
        return 1;
    }

    // Upstream `CollectionVariant` switched from a closed enum (`Columns { gap }`)
    // to an open struct keyed by `LayoutSpec::name`. Match the layout by name
    // so we honour `gap` only on `columns(...)` collections; other shapes
    // (list, table, …) carry their own `gap` semantics that this site shouldn't
    // claim.
    let gap = node
        .collection
        .as_ref()
        .and_then(|v| {
            let layout = v.layout()?;
            (layout.name() == "columns").then_some(layout.gap)
        })
        .map(gap_px_to_cells)
        .unwrap_or(0);
    let total_gap = gap.saturating_mul(items.len().saturating_sub(1));
    let usable_width = max_width.saturating_sub(total_gap);

    let mut widths = vec![0usize; items.len()];
    let mut flex_total: f32 = 0.0;
    let mut fixed_used: usize = 0;
    for (i, item) in items.iter().enumerate() {
        match item.layout_hint {
            LayoutHint::Fixed { px } => {
                let cells = (px / PX_PER_CELL).round().max(0.0) as usize;
                widths[i] = cells.min(usable_width.saturating_sub(fixed_used));
                fixed_used = (fixed_used + widths[i]).min(usable_width);
            }
            LayoutHint::Flex { weight } => {
                flex_total += weight.max(0.0);
            }
        }
    }

    let remaining = usable_width.saturating_sub(fixed_used);
    if flex_total > 0.0 && remaining > 0 {
        let mut allocated: usize = 0;
        let last_flex_idx = items
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, it)| matches!(it.layout_hint, LayoutHint::Flex { .. }).then_some(i));
        for (i, item) in items.iter().enumerate() {
            if let LayoutHint::Flex { weight } = item.layout_hint {
                let share = if Some(i) == last_flex_idx {
                    remaining.saturating_sub(allocated)
                } else {
                    ((weight.max(0.0) / flex_total) * remaining as f32).floor() as usize
                };
                widths[i] = share;
                allocated += share;
            }
        }
    }

    let mut current_col = start_col;
    let bound = start_col + max_width;
    let mut max_rows = 1usize;
    let mut rendered_any = false;
    // Only the OUTERMOST `render_columns` assigns regions. Once any ancestor
    // has tagged the walk with a region, nested columns inherit it so a grid
    // inside one panel doesn't compete with the top-level sidebar/main/drawer
    // split for Tab navigation.
    let assigns_regions = ctx.region.is_none();
    for (i, item) in items.iter().enumerate() {
        if current_col >= bound || widths[i] == 0 {
            continue;
        }
        if rendered_any && gap > 0 {
            current_col = (current_col + gap).min(bound);
            if current_col >= bound {
                break;
            }
        }
        let slot = widths[i].min(bound - current_col);
        let rows = if assigns_regions {
            let prev = ctx.region;
            ctx.region = Some(i);
            let r = render_view_model(item.as_ref(), ctx, ops, start_row, current_col, slot);
            ctx.region = prev;
            r
        } else {
            render_view_model(item.as_ref(), ctx, ops, start_row, current_col, slot)
        };
        max_rows = max_rows.max(rows);
        current_col += slot;
        rendered_any = true;
    }
    max_rows
}

fn render_collection_vertical(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
    kind: &str,
) -> usize {
    let title = node.prop_str("title").unwrap_or_default();
    let mut consumed = 0;
    if (kind == "section" || kind == "list") && !title.is_empty() {
        let header = format!("── {} ──", title);
        let clipped = clip_to_width(&header, max_width);
        *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
        let texts = tui_styled_texts! {
            tui_styled_text! {
                @style: new_style!(bold color_fg: {tui_color!(hex "#00AAFF")}),
                @text: clipped.as_str()
            },
        };
        render_tui_styled_texts_into(&texts, ops);
        consumed += 1;
    }

    let items: Vec<Arc<ReactiveViewModel>> = node
        .collection
        .as_ref()
        .map(|view| view.items.lock_ref().iter().cloned().collect())
        .unwrap_or_default();
    let indent = if kind == "tree" { 2 } else { 0 };
    // `tree` / `table` / `outline` items are doc-block rows (per-row
    // `render_entity` resolves inline, so no `live_block` wrapper survives
    // — register here off the row's `data.id`). `list` / `section` items
    // are sidebar entries already wrapped in `selectable(...)`; their
    // selectable registration takes care of cursor navigation.
    let register_as_blocks = matches!(kind, "tree" | "table" | "outline");
    for item in &items {
        let depth = item.prop_f64("depth").unwrap_or(0.0) as usize;
        let item_col = start_col + indent + depth.saturating_mul(2);
        let inner_width = max_width.saturating_sub(item_col - start_col);

        let mut registered_idx: Option<usize> = None;
        let block_focused = if register_as_blocks {
            // tree/table items typically wrap the row's data deeper — their
            // top-level node is e.g. a `column` whose first child carries the
            // data. Walk via `find_node_by_id` style: scan for any
            // descendant that has an entity_id.
            let row_id = holon_frontend::focus_path::resolve_entity_id(item.as_ref())
                .or_else(|| descendant_entity_id(item.as_ref()));
            if let Some(row_id) = row_id {
                let idx = ctx.registry.selectables.len();
                let region = ctx.current_region();
                let editable = find_editable_target(item.as_ref(), Some(row_id.as_str()));
                let displayed_text = editable
                    .as_ref()
                    .map(|e| e.current_content.clone())
                    .or_else(|| item.prop_str("content"));
                ctx.registry.selectables.push(SelectableRegion {
                    entity_id: row_id,
                    intent: None,
                    kind: SelectableKind::Block,
                    region,
                    editable,
                    start_row: start_row + consumed,
                    start_col: item_col,
                    rows: 0,
                    cols: inner_width,
                    widget_type: "live_block".into(),
                    displayed_text,
                });
                registered_idx = Some(idx);
                ctx.focus_index == Some(idx)
            } else {
                false
            }
        } else {
            false
        };

        let (child_col, child_width) = if block_focused {
            let prefix_width = 2.min(inner_width);
            if prefix_width > 0 {
                *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((
                    col(item_col),
                    row(start_row + consumed),
                )));
                let texts = tui_styled_texts! {
                    tui_styled_text! {
                        @style: new_style!(bold color_fg: {tui_color!(hex "#FFCC00")} color_bg: {tui_color!(hex "#333333")}),
                        @text: "►"
                    },
                };
                render_tui_styled_texts_into(&texts, ops);
            }
            (
                item_col + prefix_width,
                inner_width.saturating_sub(prefix_width),
            )
        } else {
            (item_col, inner_width)
        };

        let rows = render_view_model(
            item.as_ref(),
            ctx,
            ops,
            start_row + consumed,
            child_col,
            child_width,
        );
        if let Some(idx) = registered_idx {
            ctx.registry.selectables[idx].rows = rows.max(1);
        }
        consumed += rows.max(1);
    }

    for child in &node.children {
        let rows = render_view_model(
            child.as_ref(),
            ctx,
            ops,
            start_row + consumed,
            start_col,
            max_width,
        );
        consumed += rows;
    }

    consumed.max(1)
}

/// Render a node by its children/collection without adding any chrome.
fn render_passthrough(
    node: &ReactiveViewModel,
    ctx: &mut RenderCtx<'_>,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
    max_width: usize,
) -> usize {
    let mut consumed = 0;

    let collection_items: Vec<Arc<ReactiveViewModel>> = node
        .collection
        .as_ref()
        .map(|view| view.items.lock_ref().iter().cloned().collect())
        .unwrap_or_default();
    for item in &collection_items {
        let rows = render_view_model(
            item.as_ref(),
            ctx,
            ops,
            start_row + consumed,
            start_col,
            max_width,
        );
        consumed += rows.max(1);
    }

    for child in &node.children {
        let rows = render_view_model(
            child.as_ref(),
            ctx,
            ops,
            start_row + consumed,
            start_col,
            max_width,
        );
        consumed += rows;
    }

    if let Some(slot) = node.slot.as_ref() {
        let inner = slot.content.lock_ref().clone();
        let rows = render_view_model(
            inner.as_ref(),
            ctx,
            ops,
            start_row + consumed,
            start_col,
            max_width,
        );
        consumed += rows;
    }

    // Unknown widgets with no children/collection/slot render nothing.
    // Showing the widget name for debugging clutters the screen with stray
    // wrapper labels (`draggable`, `pie_menu`, …) that overdraw siblings.
    consumed
}

/// Conservative width estimate for advancing the cursor between row siblings.
fn estimate_width(node: &ReactiveViewModel) -> usize {
    let name = node.widget_name().unwrap_or_default();
    match name.as_str() {
        "text" | "editable_text" => node
            .prop_str("content")
            .map(|s| display_width(&s) + 1)
            .unwrap_or(0),
        "checkbox" => 4,
        "badge" => {
            let inner = node
                .prop_str("content")
                .or_else(|| node.prop_str("label"))
                .unwrap_or_default();
            display_width(&inner) + 4
        }
        "icon" => node
            .prop_str("symbol")
            .map(|s| display_width(&s) + 1)
            .unwrap_or(2),
        // spacer(N) — N defaults to logical pixels in GPUI; treat as cells / 4
        // for terminal-scale spacing. Floor at 1 so a bare `spacer()` still
        // separates siblings.
        "spacer" => node
            .prop_f64("size")
            .or_else(|| node.prop_f64("width"))
            .map(|px| ((px / 4.0).round() as usize).max(1))
            .unwrap_or(1),
        "state_toggle" => {
            let label = node.prop_str("label").unwrap_or_default();
            if label.is_empty() {
                0
            } else {
                display_width(&label) + 1
            }
        }
        "bullet" => 2,
        // Transparent wrappers — width comes from the inner widget the
        // wrapper recursed into. We can't easily inspect that here, so we
        // fall back to 0 and the renderer overdraws or relies on enclosing
        // layout (`render_columns`) to pre-allocate the slot width.
        _ => 0,
    }
}

/// Recursively scan for an entity id inside a node's children/collection/slot.
/// Used by `render_collection_vertical` to extract a stable id for tree/table
/// rows whose top-level wrapper (a `column`) has no `data` of its own — the
/// id lives on a descendant `selectable` / `editable_text` / `live_block`.
fn descendant_entity_id(node: &ReactiveViewModel) -> Option<String> {
    if let Some(id) = holon_frontend::focus_path::resolve_entity_id(node) {
        return Some(id);
    }
    if let Some(view) = node.collection.as_ref() {
        for item in view.items.lock_ref().iter() {
            if let Some(id) = descendant_entity_id(item.as_ref()) {
                return Some(id);
            }
        }
    }
    for child in &node.children {
        if let Some(id) = descendant_entity_id(child.as_ref()) {
            return Some(id);
        }
    }
    if let Some(slot) = node.slot.as_ref() {
        let inner = slot.content.lock_ref().clone();
        if let Some(id) = descendant_entity_id(inner.as_ref()) {
            return Some(id);
        }
    }
    None
}

fn display_width(s: &str) -> usize {
    s.graphemes(true).count()
}

fn clip_to_width(line: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(line.len());
    let mut count = 0usize;
    for g in line.graphemes(true) {
        if count >= max_width {
            break;
        }
        out.push_str(g);
        count += 1;
    }
    out
}

/// Public for `lib.rs` so `render_supported_widgets` can build a HashSet.
pub fn supported_widget_names() -> Vec<&'static str> {
    TUI_SUPPORTED_WIDGETS.to_vec()
}
