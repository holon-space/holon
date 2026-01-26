use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let w = args
        .get_f64("width")
        .or(args.get_f64("w"))
        .unwrap_or(0.0) as f32;
    let h = args
        .get_f64("height")
        .or(args.get_f64("h"))
        .unwrap_or(0.0) as f32;

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        let mut el = ui.element();
        if w > 0.0 {
            el = el.width(fixed(w));
        }
        if h > 0.0 {
            el = el.height(fixed(h));
        }
        if w == 0.0 && h == 0.0 {
            el = el.width(ply_engine::grow!()).height(ply_engine::grow!());
        }
        el.empty();
    })
}
