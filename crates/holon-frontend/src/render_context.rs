use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::{Mutable, ReadOnlyMutable};
use holon_api::render_types::OperationWiring;
use holon_api::widget_spec::DataRow;

use crate::reactive::BuilderServices;

/// Viewport or container allocation in logical + physical pixels.
///
/// Refined per subtree during render interpretation: each layout container
/// that declares a `child_space` clause (e.g. `columns`) partitions its
/// incoming `available_space` among its children, and the refined value
/// flows into `pick_active_variant` so profile variants can select different
/// renders based on how much room they have — CSS container queries.
///
/// Logical pixels are the primary input (already DPI-normalized by the UI
/// framework); physical pixels and `scale_factor` are secondary signals for
/// density-aware decisions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AvailableSpace {
    pub width_px: f32,
    pub height_px: f32,
    pub width_physical_px: f32,
    pub height_physical_px: f32,
    pub scale_factor: f32,
}

/// Hint from a widget to its parent layout container about how much space it needs.
///
/// Maps directly to CSS flex properties:
/// - `Flex { weight }` → `flex: weight` (proportional share of remaining space)
/// - `Fixed { px }` → `flex: 0 0 Npx` (exact allocation, no grow/shrink)
///
/// Overlay drawers use `Fixed { px: 0.0 }` because they don't participate in
/// flow layout — they float above siblings without consuming horizontal space.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayoutHint {
    /// Takes a proportional share of remaining space after Fixed children.
    Flex { weight: f32 },
    /// Takes exactly this many pixels. Does not grow or shrink.
    Fixed { px: f32 },
}

impl Default for LayoutHint {
    fn default() -> Self {
        Self::Flex { weight: 1.0 }
    }
}

static EMPTY_ROW: std::sync::LazyLock<Arc<DataRow>> =
    std::sync::LazyLock::new(|| Arc::new(HashMap::new()));

/// Pure data context passed through the render tree during interpretation.
///
/// Two levels of row binding:
///
/// - `data_rows` — the container's full row set. Set once when entering a
///   block's scope (e.g. via `interpret_pure` or `with_data_rows`). Iterated
///   by the `Collection` macro expansion (static arm), `shared_tree_build`,
///   and `pref_field`. Never modified by per-row binding.
///
/// - `current_row` — the single row bound by a collection's per-item loop.
///   Set by `with_row(row)`. Leaf widgets read this via `row()`.
///
/// `row()` prefers `current_row` and falls back to `data_rows[0]`.
/// This makes the "which row am I looking at?" question explicit: after
/// `with_row`, leaves see the bound row; before `with_row`, they see the
/// first container row (or `EMPTY_ROW` if nothing was bound).
#[derive(Clone, Default)]
pub struct RenderContext {
    pub data_rows: Vec<Arc<DataRow>>,
    /// The single row bound by a collection's per-item loop. Set by `with_row`.
    /// Takes precedence over `data_rows[0]` in `row()` / `row_arc()`.
    pub current_row: Option<Arc<DataRow>>,
    /// Shared read-only handle for the bound row's data, supplied by the
    /// collection driver when CDC owns the data. Leaf builders clone this
    /// handle into `ReactiveViewModel::data` so parent and children share
    /// one `MutableState<Arc<DataRow>>` — a single CDC `.set()` (only
    /// callable through `ReactiveRowSet::apply_change`'s private `Mutable`
    /// handle) lights up every subscribed leaf without any tree walk. The
    /// `ReadOnlyMutable` type makes "downstream tries to mutate row data" a
    /// **compile error**, which is what keeps the one-writer invariant from
    /// drifting back to multi-writer in some future refactor. Absent for
    /// interpret-only call sites (MCP, PBT snapshots, design gallery), in
    /// which case `data_mutable()` synthesises a one-shot
    /// `Mutable::new(snapshot).read_only()`.
    pub current_row_mutable: Option<ReadOnlyMutable<Arc<DataRow>>>,
    pub operations: Vec<OperationWiring>,
    /// Input triggers derived from operations. Propagated to ViewModel nodes
    /// so frontends can check them locally on each keystroke.
    pub triggers: Vec<crate::input_trigger::InputTrigger>,
    /// Nesting depth (for indentation in block builders)
    pub depth: usize,
    /// Query nesting depth — tracks recursive query execution to prevent stack overflow.
    pub query_depth: usize,
    /// Live data source for collection builders. When set, shadow builders create
    /// `ReactiveView::new_collection()` with a live streaming pipeline instead of
    /// static snapshots. Set by `watch_live()` before calling `interpret_fn`.
    /// `None` for headless/snapshot consumers (MCP, PBT, TUI).
    pub data_source: Option<Arc<dyn holon_api::ReactiveRowProvider>>,
    /// Container-query allocation: how much space THIS subtree was allotted by
    /// its parent. Refined by layout containers via `with_available_space` before
    /// recursing into children. `None` outside a partitioning container; the
    /// global viewport is merged in as a fallback by `pick_active_variant`.
    pub available_space: Option<AvailableSpace>,
}

impl RenderContext {
    /// The current row's data for ColumnRef resolution.
    ///
    /// Reads `current_row` (set by `with_row` inside a collection's per-item
    /// loop), falling back to `data_rows[0]`, falling back to `EMPTY_ROW`.
    pub fn row(&self) -> &DataRow {
        self.current_row
            .as_deref()
            .or_else(|| self.data_rows.first().map(|a| a.as_ref()))
            .unwrap_or(&EMPTY_ROW)
    }

    /// The current row as an Arc (cheap clone for entity attachment).
    pub fn row_arc(&self) -> Arc<DataRow> {
        self.current_row
            .clone()
            .or_else(|| self.data_rows.first().cloned())
            .unwrap_or_else(|| Arc::clone(&EMPTY_ROW))
    }

    /// Shared `ReadOnlyMutable<Arc<DataRow>>` for the bound row.
    ///
    /// Returns the CDC-owned read-only handle when the collection driver
    /// supplied one (`with_row_mutable`), so all leaves rendered for the
    /// same row share the same `Arc<MutableState>`. The `ReadOnlyMutable`
    /// type makes leaf-side mutation a compile error — only
    /// `ReactiveRowSet::apply_change`, which holds the writable `Mutable`,
    /// can update row data. Falls back to a one-shot
    /// `Mutable::new(row_arc()).read_only()` for interpret-only call sites
    /// with no live data source (MCP, PBT snapshots, design gallery) —
    /// those leaves are rendered once and never updated.
    pub fn data_mutable(&self) -> ReadOnlyMutable<Arc<DataRow>> {
        self.current_row_mutable
            .clone()
            .unwrap_or_else(|| Mutable::new(self.row_arc()).read_only())
    }

    /// Create a child context with new operations.
    /// Automatically derives default input triggers from the operations.
    /// Joins keybindings from the services registry into each OperationDescriptor.
    pub fn with_operations(
        &self,
        mut operations: Vec<OperationWiring>,
        services: &dyn BuilderServices,
    ) -> Self {
        let bindings = services.key_bindings_snapshot();
        if !bindings.is_empty() {
            for op in &mut operations {
                if let Some(chord) = bindings.get(&op.descriptor.name) {
                    op.descriptor.trigger = Some(holon_api::Trigger::KeyChord {
                        chord: chord.clone(),
                    });
                }
            }
        }
        let triggers = crate::input_trigger::default_triggers_for_operations(&operations);
        Self {
            operations,
            triggers,
            ..self.clone()
        }
    }

    /// Bind a single row for leaf widget resolution.
    ///
    /// Sets `current_row` — takes precedence over `data_rows[0]` in `row()`.
    /// Preserves the parent's `data_rows` (container rows stay available for
    /// builders like `pref_field` that search the full set).
    ///
    /// Clears `current_row_mutable` because the snapshot path has no shared
    /// CDC handle. Live drivers must use `with_row_mutable` instead.
    pub fn with_row(&self, row: Arc<DataRow>) -> Self {
        Self {
            current_row: Some(row),
            current_row_mutable: None,
            ..self.clone()
        }
    }

    /// Bind a row via a CDC-owned `ReadOnlyMutable` handle.
    ///
    /// All leaves rendered under this context share the same
    /// `Arc<MutableState>` — when `ReactiveRowSet::apply_change` writes the
    /// row through its private writable `Mutable`, every leaf that holds a
    /// clone of this read-only handle observes the new value via signal
    /// subscription. The `ReadOnlyMutable` type guarantees no leaf can
    /// write — that is the type-level enforcement of "one writer =
    /// `ReactiveRowSet`". The `current_row` snapshot is also set so
    /// synchronous reads (`row()`, `row_arc()`) resolve immediately during
    /// interpretation.
    pub fn with_row_mutable(&self, handle: ReadOnlyMutable<Arc<DataRow>>) -> Self {
        let snapshot = handle.get_cloned();
        Self {
            current_row: Some(snapshot),
            current_row_mutable: Some(handle),
            ..self.clone()
        }
    }

    /// Create a child context with the given container rows.
    ///
    /// Clears `current_row` — the new context is at the container level,
    /// not inside a per-row binding.
    pub fn with_data_rows(&self, data_rows: Vec<Arc<DataRow>>) -> Self {
        Self {
            data_rows,
            current_row: None,
            ..self.clone()
        }
    }

    /// Create a child context with incremented query depth.
    pub fn deeper_query(&self) -> Self {
        Self {
            query_depth: self.query_depth + 1,
            ..self.clone()
        }
    }

    /// Create a child context with incremented nesting depth.
    pub fn indented(&self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self.clone()
        }
    }

    /// Refine the container-query allocation for this subtree.
    ///
    /// Layout containers call this before recursing into their children so
    /// profile variants can key on `available_width_px` / `available_height_px`
    /// for the child's actual allotted space, not the parent's.
    pub fn with_available_space(&self, space: AvailableSpace) -> Self {
        Self {
            available_space: Some(space),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_space() -> AvailableSpace {
        AvailableSpace {
            width_px: 800.0,
            height_px: 600.0,
            width_physical_px: 1600.0,
            height_physical_px: 1200.0,
            scale_factor: 2.0,
        }
    }

    #[test]
    fn with_available_space_then_with_row_preserves_both() {
        let space = sample_space();
        let row: Arc<DataRow> = Arc::new(HashMap::new());
        let ctx = RenderContext::default()
            .with_available_space(space)
            .with_row(row.clone());
        assert_eq!(ctx.available_space, Some(space));
        assert!(ctx.current_row.is_some());
    }

    #[test]
    fn with_row_then_with_available_space_preserves_both() {
        let space = sample_space();
        let row: Arc<DataRow> = Arc::new(HashMap::new());
        let ctx = RenderContext::default()
            .with_row(row)
            .with_available_space(space);
        assert_eq!(ctx.available_space, Some(space));
        assert!(ctx.current_row.is_some());
    }

    #[test]
    fn default_context_has_no_available_space() {
        assert!(RenderContext::default().available_space.is_none());
    }
}
