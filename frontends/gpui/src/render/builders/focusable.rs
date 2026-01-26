use super::prelude::*;
use holon_frontend::ReactiveViewModel;

/// Focusable wrapper — renders child as-is.
///
/// Focus tracking is handled by `UiState` (set via `handle_cross_block_nav`
/// and read via `BuilderServices::ui_state()`). The `is_focused` predicate
/// drives variant selection in `pick_active_variant` during interpretation.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let child = node.children.first().expect("focusable requires a child");
    super::render(child, ctx)
}
