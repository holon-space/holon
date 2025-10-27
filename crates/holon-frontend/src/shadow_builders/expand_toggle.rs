use super::prelude::*;
use crate::reactive_view_model::LazyReactiveSlot;

holon_macros::widget_builder! {
    raw fn expand_toggle(ba: BA<'_>) -> ViewModel {
        let target_id = ba.ctx.row().get("id")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();

        let expanded = futures_signals::signal::Mutable::new(false);

        let header = ba.args.get_template("header")
            .cloned()
            .unwrap_or_else(|| holon_api::render_types::RenderExpr::FunctionCall {
                name: "text".to_string(),
                args: vec![],
            });

        let header_vm = (ba.interpret)(&header, ba.ctx);
        let children = vec![Arc::new(header_vm)];

        // Lazy materialisation: capture services + template + ctx so the
        // content is interpreted on first expand instead of at build time.
        // Why this matters: claude-history.yaml wraps live_query inside
        // expand_toggle content; interpreting it while collapsed spawns
        // unnecessary FDW fetches. The cache lives for the VM lifetime, so
        // re-collapse + re-expand is instant. `push_down_lazy_slot` carries
        // the cache forward across structural rebuilds.
        let lazy_slot = ba.args.get_template("content").cloned().map(|template| {
            let services_arc = ba.services.clone_arc();
            let ctx = ba.ctx.clone();
            let thunk: Arc<dyn Fn() -> ViewModel + Send + Sync> =
                Arc::new(move || services_arc.interpret(&template, &ctx));
            LazyReactiveSlot::new(expanded.read_only(), thunk)
        });

        let mut __props = std::collections::HashMap::new();
        __props.insert("target_id".to_string(), Value::String(target_id));

        ViewModel {
            expanded: Some(expanded),
            lazy_slot,
            children,
            data: ba.ctx.data_mutable(),
            render_ctx: Some(ba.ctx.clone()),
            ..ViewModel::from_widget("expand_toggle", __props)
        }
    }
}
