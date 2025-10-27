use super::prelude::*;
use crate::render_context::LayoutHint;

holon_macros::widget_builder! {
    raw fn spacer(ba: BA<'_>) -> ViewModel {
        let width = ba
            .args
            .get_f64("width")
            .or_else(|| ba.args.get_positional_f64(0))
            .map(|v| v as f32)
            .unwrap_or(0.0);
        let height = ba
            .args
            .get_f64("height")
            .or_else(|| ba.args.get_positional_f64(1))
            .map(|v| v as f32)
            .unwrap_or(0.0);
        let color = ba.args.get_string("color").map(|s| s.to_string());

        let mut __props = std::collections::HashMap::new();
        __props.insert("width".to_string(), Value::Float(width as f64));
        __props.insert("height".to_string(), Value::Float(height as f64));
        if let Some(ref c) = color {
            __props.insert("color".to_string(), Value::String(c.clone()));
        }
        ViewModel {
            layout_hint: LayoutHint::Fixed { px: width },
            ..ViewModel::from_widget("spacer", __props)
        }
    }
}
