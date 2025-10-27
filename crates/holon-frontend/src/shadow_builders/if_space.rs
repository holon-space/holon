use super::prelude::*;

/// Container-query conditional: pick one of two render expressions based on
/// the current subtree's `available_width_px`.
///
/// ```text
/// if_space(threshold, narrow_expr, wide_expr)
/// ```
///
/// Reads `ctx.available_space.width_px` at interpretation time. If the width
/// is strictly less than `threshold`, interprets `narrow_expr`; otherwise
/// interprets `wide_expr`. When `available_space` is `None` (no viewport
/// known yet), falls back to `wide_expr` — desktop-first defaults.
///
/// Primary use: make the root layout swap its shape between phone, tablet,
/// and desktop without routing through profile resolution. The root render
/// source can chain `if_space` to express three-way breakpoints:
///
/// ```text
/// if_space(600, <phone layout>, if_space(1024, <tablet layout>, <desktop layout>))
/// ```
///
/// On viewport changes, the flat driver re-fires (it subscribes to its
/// `space` Mutable via `map_ref!`), the render expression is re-interpreted,
/// and `if_space` re-evaluates against the fresh `ctx.available_space`.
/// Subtrees whose computed space did not cross a breakpoint keep their
/// widget identity via `stable_cache_key`, preserving transient state.
///
/// Positional arg 0 must be a numeric literal (the threshold in logical px).
/// Positional args 1 and 2 are raw render templates — only the chosen branch
/// is interpreted.
holon_macros::widget_builder! {
    raw fn if_space(ba: BA<'_>) -> ViewModel {
        let threshold = ba
            .args
            .positional
            .get(0)
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(f32::INFINITY);

        // Missing `available_space` → desktop-first: choose the wider branch.
        let is_narrow = ba
            .ctx
            .available_space
            .map(|s| s.width_px < threshold)
            .unwrap_or(false);

        let branch_idx = if is_narrow { 1 } else { 2 };
        match ba.args.positional_exprs.get(branch_idx) {
            Some(expr) => (ba.interpret)(expr, ba.ctx),
            None => ViewModel::empty(),
        }
    }
}
