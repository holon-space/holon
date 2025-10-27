use super::prelude::*;
use crate::view_model::NodeKind;

holon_macros::widget_builder! {
    raw fn draggable(ba: BA<'_>) -> ViewModel {
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };
        ViewModel::from_kind(NodeKind::Draggable {
            child: Box::new(child),
        })
    }
}
