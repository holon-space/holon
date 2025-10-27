use super::prelude::*;
use crate::view_model::NodeKind;

holon_macros::widget_builder! {
    raw fn pie_menu(ba: BA<'_>) -> ViewModel {
        let fields = ba.args.get_string("fields").unwrap_or("").to_string();
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };
        ViewModel::from_kind(NodeKind::PieMenu {
            fields,
            child: Box::new(child),
        })
    }
}
