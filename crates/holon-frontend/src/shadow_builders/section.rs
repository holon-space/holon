use super::prelude::*;

holon_macros::widget_builder! {
    fn section(#[default = "Section"] title: String, children: Collection) {
        ViewModel {
            kind: NodeKind::Section {
                title,
                children: LazyChildren::fully_materialized(children),
            },
            ..Default::default()
        }
    }
}
