use super::prelude::*;

/// Per-row conditional: render one of two templates based on a row field.
///
/// ```text
/// if_col(field_name, expected_value, then_expr, else_expr)
/// ```
///
/// Reads `ctx.row().get(field_name)` at interpretation time (once per row in
/// a streaming collection), compares it as a string to `expected_value`, and
/// interprets either `then_expr` or `else_expr`.
///
/// Primary motivation: keep `columns` from hand-rolling a drawer loop.
/// Example root layout:
///
/// ```text
/// columns(#{
///   sort_key: col("sequence"),
///   item_template: if_col("collapse-to", "drawer",
///     drawer(id: col("id"), live_block()),
///     live_block()),
/// })
/// ```
///
/// Positional args 0 and 1 must be literal strings (the field name and the
/// expected value). Positional args 2 and 3 are raw render templates — they
/// are interpreted against the current row context when the branch is taken.
holon_macros::widget_builder! {
    raw fn if_col(ba: BA<'_>) -> ViewModel {
        let field = ba
            .args
            .positional
            .get(0)
            .and_then(|v| v.as_string())
            .unwrap_or("");
        let expected = ba
            .args
            .positional
            .get(1)
            .and_then(|v| v.as_string())
            .unwrap_or("");

        let actual = ba
            .ctx
            .row()
            .get(field)
            .and_then(|v| v.as_string())
            .unwrap_or("");

        let branch_idx = if actual == expected { 2 } else { 3 };
        match ba.args.positional_exprs.get(branch_idx) {
            Some(expr) => (ba.interpret)(expr, ba.ctx),
            None => ViewModel::empty(),
        }
    }
}
