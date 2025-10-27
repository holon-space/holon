use super::prelude::*;

holon_macros::widget_builder! {
    fn editable_text(content: String, #[default = "content"] field: String) {
        ViewModel {
            entity: ba.ctx.row().clone(),
            kind: NodeKind::EditableText { content, field },
            operations: ba.ctx.operations.clone(),
            triggers: ba.ctx.triggers.clone(),
            ..Default::default()
        }
    }
}
