use super::prelude::*;

holon_macros::widget_builder! {
    raw fn on_hover(ba: BA<'_>) -> ViewModel {
        // Positional children (mirrors `collapsible`): the first is the
        // always-visible trigger; the rest are the content revealed by the
        // frontend only while the trigger's region is hovered. All are
        // interpreted eagerly into `children` (content is cheap — a
        // caption/badge), so the snapshot carries the full tree and hover
        // stays a pure per-frontend view concern.
        let children: Vec<Arc<ViewModel>> = ba.args.positional_exprs.iter()
            .map(|expr| Arc::new((ba.interpret)(expr, ba.ctx)))
            .collect();

        ViewModel {
            children,
            // Per-render-slot hover state (Model.md "Cell vs Mutable", FU-1).
            hovered: Some(futures_signals::signal::Mutable::new(false)),
            data: ba.ctx.data_mutable(),
            ..ViewModel::from_widget("on_hover", std::collections::HashMap::new())
        }
    }
}
