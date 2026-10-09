use super::prelude::*;

holon_macros::widget_builder! {
    raw fn view_mode_switcher(ba: BA<'_>) -> ViewModel {
        let Some(entity_id) = ba.args.get_string("entity_uri") else {
            return ViewModel::error(
                "view_mode_switcher",
                "view_mode_switcher requires an `entity_uri` argument",
            );
        };
        let entity_uri =
            match crate::render_interpreter::render_spec_block_uri("entity_uri", entity_id) {
                Ok(uri) => uri,
                Err(msg) => return ViewModel::error("view_mode_switcher", msg),
            };

        let modes = match ba.args.named.get("modes") {
            Some(Value::String(json)) => json.clone(),
            other => {
                return ViewModel::error(
                    "view_mode_switcher",
                    format!("`modes:` must be a JSON string listing the modes, got {other:?}"),
                )
            }
        };
        let listed = match crate::reactive_view_model::parse_view_modes(&modes) {
            Ok(listed) if !listed.is_empty() => listed,
            Ok(_) => return ViewModel::error("view_mode_switcher", "`modes:` lists no mode"),
            Err(msg) => return ViewModel::error("view_mode_switcher", msg),
        };

        let mode_templates: std::collections::HashMap<String, holon_api::render_types::RenderExpr> =
            ba.args.templates.iter()
                .filter(|(k, _)| k.starts_with("mode_"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        // Every listed mode is drawn by its template, and every template is
        // reachable from the bar.
        for mode in &listed {
            if !mode_templates.contains_key(&format!("mode_{}", mode.name)) {
                return ViewModel::error(
                    "view_mode_switcher",
                    format!("mode `{}` is listed in `modes:` but has no `mode_{}:` template", mode.name, mode.name),
                );
            }
        }
        for key in mode_templates.keys() {
            if !listed.iter().any(|mode| format!("mode_{}", mode.name) == *key) {
                return ViewModel::error(
                    "view_mode_switcher",
                    format!("`{key}:` is a template for a mode `modes:` does not list"),
                );
            }
        }

        // `default_mode` names the unconditional variant when the backend
        // knows it (`holon::api::block_domain::view_mode_switcher_from_variants`);
        // otherwise the first listed mode.
        let default_mode = match ba.args.named.get("default_mode") {
            None => listed[0].name.clone(),
            Some(Value::String(mode)) if listed.iter().any(|m| m.name == *mode) => mode.clone(),
            Some(other) => {
                return ViewModel::error(
                    "view_mode_switcher",
                    format!("`default_mode:` must name a listed mode, got {other:?}"),
                )
            }
        };
        let child = (ba.interpret)(&mode_templates[&format!("mode_{default_mode}")], ba.ctx);

        let mut __props = std::collections::HashMap::new();
        __props.insert("entity_uri".to_string(), Value::String(entity_uri.to_string()));
        __props.insert("modes".to_string(), Value::String(modes));
        crate::reactive_view_model::mark_active_mode(&mut __props, &default_mode, &child);
        // Serialize mode_templates into props for snapshot reconstruction.
        for (k, v) in &mode_templates {
            let json = match serde_json::to_string(v) {
                Ok(json) => json,
                Err(e) => {
                    return ViewModel::error(
                        "view_mode_switcher",
                        format!("mode template `{k}` has no JSON form: {e}"),
                    )
                }
            };
            __props.insert(format!("tmpl_{k}"), Value::String(json));
        }
        ViewModel {
            slot: Some(crate::reactive_view_model::ReactiveSlot::new(child)),
            render_ctx: Some(ba.ctx.clone()),
            ..ViewModel::from_widget("view_mode_switcher", __props)
        }
    }
}
