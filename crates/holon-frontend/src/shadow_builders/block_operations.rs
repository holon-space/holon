use super::prelude::*;

holon_macros::widget_builder! {
    raw fn block_operations(ba: BA<'_>) -> ViewModel {
        let operations: String = ba
            .ctx
            .operations
            .iter()
            .map(|ow| ow.descriptor.name.clone())
            .collect::<Vec<_>>()
            .join(",");

        ViewModel {
            kind: NodeKind::BlockOperations { operations },
            ..Default::default()
        }
    }
}
