use super::prelude::*;
use crate::reactive_view_model::ReactiveSlot;

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

        let content_template = ba.args.get_template("content").cloned();

        // Gate content interpretation on expanded state.
        // Critical: claude-history.yaml wraps live_query inside expand_toggle
        // content. Interpreting it when collapsed spawns unnecessary FDW fetches.
        let content_slot = if expanded.get() {
            if let Some(ref expr) = content_template {
                ReactiveSlot::new((ba.interpret)(expr, ba.ctx))
            } else {
                ReactiveSlot::empty()
            }
        } else {
            ReactiveSlot::empty()
        };

        let mut __props = std::collections::HashMap::new();
        __props.insert("target_id".to_string(), Value::String(target_id));
        if let Some(ref tmpl) = content_template {
            __props.insert("content_template".to_string(),
                Value::String(serde_json::to_string(tmpl).unwrap_or_default()));
        }
        // Wire to the shared per-row signal cell. The only row-derived prop
        // here is `target_id`, which is the row's primary key — it doesn't
        // change across CDC updates (a different id means a different cell).
        // No subscription needed; just share the handle so reads
        // (`entity()`, `row_id()`) reflect the live row state if anything
        // upstream ever queries them.
        ViewModel {
            expanded: Some(expanded),
            slot: Some(content_slot),
            children,
            data: ba.ctx.data_mutable(),
            render_ctx: Some(ba.ctx.clone()),
            ..ViewModel::from_widget("expand_toggle", __props)
        }
    }
}
