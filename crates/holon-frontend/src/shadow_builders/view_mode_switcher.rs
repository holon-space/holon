use super::prelude::*;

holon_macros::widget_builder! {
    raw fn view_mode_switcher(ba: BA<'_>) -> ViewModel {
        let modes = ba.args.get_string("modes").unwrap_or("[]").to_string();

        let entity_uri = ba.args.get_string("entity_uri")
            .map(|s| holon_api::EntityUri::from_raw(s))
            .expect("view_mode_switcher requires an `entity_uri` argument");

        // Collect all mode_* templates.
        let mode_templates: std::collections::HashMap<String, holon_api::render_types::RenderExpr> =
            ba.args.templates.iter()
                .filter(|(k, _)| k.starts_with("mode_"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

        // Default mode = first in the modes JSON array.
        let default_mode = serde_json::from_str::<Vec<serde_json::Value>>(&modes)
            .ok()
            .and_then(|arr| arr.first()?.get("name")?.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "tree".to_string());

        let active_mode = futures_signals::signal::Mutable::new(default_mode);

        // Interpret the currently active mode's template into the slot.
        let mode_key = format!("mode_{}", active_mode.get_cloned());
        let child_expr = mode_templates.get(&mode_key)
            .or_else(|| {
                ba.args.templates.iter()
                    .find(|(k, _)| k.starts_with("mode_"))
                    .map(|(_, v)| v)
            });

        let child = match child_expr {
            Some(expr) => (ba.interpret)(expr, ba.ctx),
            None => ViewModel::empty(),
        };

        let mut __props = std::collections::HashMap::new();
        __props.insert("entity_uri".to_string(), Value::String(entity_uri.to_string()));
        __props.insert("modes".to_string(), Value::String(modes));
        __props.insert("active_mode".to_string(), Value::String(active_mode.get_cloned()));
        // Serialize mode_templates into props for snapshot reconstruction.
        for (k, v) in &mode_templates {
            __props.insert(
                format!("tmpl_{k}"),
                Value::String(serde_json::to_string(v).unwrap_or_default()),
            );
        }
        ViewModel {
            slot: Some(crate::reactive_view_model::ReactiveSlot::new(child)),
            render_ctx: Some(ba.ctx.clone()),
            ..ViewModel::from_widget("view_mode_switcher", __props)
        }
    }
}
