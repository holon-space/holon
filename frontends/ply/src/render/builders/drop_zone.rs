use super::prelude::*;

pub fn build(_: &ResolvedArgs, _: &RenderContext) -> PlyWidget {
    Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element().height(fixed(4.0)).empty();
    })
}
