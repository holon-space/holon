use super::prelude::*;
use std::collections::HashMap;

holon_macros::widget_builder! {
    raw fn image(ba: BA<'_>) -> ViewModel {
        let path = match ba.args.get_string("path") {
            Some(s) => s.to_string(),
            None => ba.args.get_positional_string(0).unwrap_or_default(),
        };
        let alt = ba.args.get_string("alt")
            .map(|s| s.to_string())
            .unwrap_or_default();

        let mut props = HashMap::new();
        props.insert("path".into(), Value::String(path));
        props.insert("alt".into(), Value::String(alt));
        if let Some(w) = ba.args.get_f64("width") {
            props.insert("width".into(), Value::Float(w));
        }
        if let Some(h) = ba.args.get_f64("height") {
            props.insert("height".into(), Value::Float(h));
        }

        ViewModel::from_widget("image", props)
    }
}
