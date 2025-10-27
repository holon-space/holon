use super::prelude::*;

/// Two-slot anchored container — platform shells pin the second slot to the
/// bottom safe-area inset (IME / nav bar / home indicator) and let the first
/// slot consume the remaining space. Intended for the mobile action bar:
///
/// ```rhai
/// bottom_dock(
///     columns(/* main content */),
///     row(#{ gap: 8, collection: chain_ops(0),
///            item_template: op_button(col("name")) }),
/// )
/// ```
///
/// Typed as two slots rather than a variadic last-child-pinned `col` — the
/// "pinned" position is part of the API contract, not a flag on the last
/// entry that would flip silently on append.
holon_macros::widget_builder! {
    raw fn bottom_dock(ba: BA<'_>) -> ViewModel {
        assert_eq!(
            ba.args.positional_exprs.len(),
            2,
            "bottom_dock requires exactly 2 positional args (main, dock); got {}",
            ba.args.positional_exprs.len()
        );
        let main = (ba.interpret)(&ba.args.positional_exprs[0], ba.ctx);
        let dock = (ba.interpret)(&ba.args.positional_exprs[1], ba.ctx);
        ViewModel {
            children: vec![Arc::new(main), Arc::new(dock)],
            ..ViewModel::from_widget("bottom_dock", std::collections::HashMap::new())
        }
    }
}
