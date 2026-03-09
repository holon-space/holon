use super::prelude::*;

pub fn build(_args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element().height(fixed(4.0)).empty();
    })
}
