use super::prelude::*;

holon_macros::widget_builder! {
    fn icon(
        #[default = "circle"] name: String,
        #[default = 16.0] size: f32,
        #[default = ""] color: String,
        #[default = 0.0] mt: f32,
    ) {
        // The props below are exactly what the macro's `resolve_props_from_args`
        // produces for these four typed params, so this full build and a
        // props-only fast-path recompute write the same map. The colour is the
        // one prop that differs: it is validated here instead of being carried
        // through verbatim.
        let color = if color.is_empty() {
            String::new()
        } else {
            match theme_token_prop("icon", "color", &color) {
                Ok(token) => token,
                Err(msg) => return ViewModel::error("icon", msg),
            }
        };
        let mut __props = std::collections::HashMap::new();
        __props.insert("name".to_string(), Value::String(name));
        __props.insert("size".to_string(), Value::Float(size as f64));
        __props.insert("color".to_string(), Value::String(color));
        __props.insert("mt".to_string(), Value::Float(mt as f64));
        ViewModel::from_widget("icon", __props)
    }
}
