//! `integration_status(col("status"))` — the integration status WORD, painted
//! as one aligned glyph in the colour that state wears.
//!
//! | Status        | Glyph | Colour    |
//! | ------------- | ----- | --------- |
//! | `Connected`   | `●`   | `success` |
//! | `Pending`     | `◐`   | `muted`   |
//! | `Needs auth`  | `⚠`   | `warning` |
//! | `Unavailable` | `○`   | `error`   |
//!
//! A widget rather than a value function, because glyph and colour are two
//! reads of ONE status and a row that carried them as two independent
//! `col("status")` expressions could paint a green `⚠`. It emits a plain `text`
//! node, so every frontend keeps its one text renderer and this file owns only
//! the table.
//!
//! The words are `holon_app::IntegrationStatus::label()`. That crate depends on
//! this one, so the coupling is checked from ITS side
//! (`holon-app/tests/integrations_section_seed.rs`): an unmapped label is a red
//! test there rather than a `?` on someone's screen.

use futures_signals::signal::SignalExt;

use super::prelude::*;
use crate::reactive_view_model::DropTask;

/// An unmapped status is DISCLOSED, not painted like a healthy one.
const UNKNOWN: (&str, &str) = ("?", "error");

/// The glyph and theme colour token for a status word — `None` when the word is
/// not one this table knows.
pub fn status_symbol_and_color(status: &str) -> Option<(&'static str, &'static str)> {
    Some(match status.trim() {
        "Connected" => ("●", "success"),
        "Pending" => ("◐", "muted"),
        "Needs auth" => ("⚠", "warning"),
        "Unavailable" => ("○", "error"),
        _ => return None,
    })
}

fn resolve(status: &str) -> (&'static str, &'static str) {
    status_symbol_and_color(status).unwrap_or_else(|| {
        // The empty word is the collection's TEMPLATE build against an empty
        // data row, not a row of the mirror — warning there would cry wolf on
        // every boot and teach the reader to ignore the warning that matters.
        if status.is_empty() {
            return UNKNOWN;
        }
        tracing::warn!(
            status,
            "integration_status: unmapped status word — painting the disclosed unknown marker. \
             Add it to holon_frontend::shadow_builders::integration_status."
        );
        UNKNOWN
    })
}

/// The glyph's box. Wide enough for the widest of them, and FIXED, so every
/// row's symbol starts at the same x — glyph advances differ, so a
/// content-sized box would stagger the column.
const SYMBOL_BOX_PX: f64 = 18.0;

fn props(status: &str, size: f32, field: Option<&str>) -> std::collections::HashMap<String, Value> {
    let (symbol, color) = resolve(status);
    let mut props = std::collections::HashMap::new();
    props.insert("content".to_string(), Value::String(symbol.to_string()));
    props.insert("color".to_string(), Value::String(color.to_string()));
    props.insert("size".to_string(), Value::Float(size as f64));
    props.insert("bold".to_string(), Value::Boolean(false));
    props.insert("width".to_string(), Value::Float(SYMBOL_BOX_PX));
    if let Some(f) = field {
        props.insert("field".to_string(), Value::String(f.to_string()));
    }
    props
}

holon_macros::widget_builder! {
    fn integration_status(status: String, #[default = 14.0] size: f32) {
        // Bound column captured for the same reason `text` captures it: the
        // status resolves at boot, after the row is first painted, and without
        // a subscription the glyph would freeze at `Pending` forever.
        let field = ba.args.get_positional_column_name(0).map(|s| s.to_string());
        let __props = props(&status, size, field.as_deref());

        let Some(field) = field else {
            return ViewModel::from_widget("text", __props);
        };

        let data = ba.ctx.data_mutable();
        let mut vm = ViewModel {
            data: data.clone(),
            ..ViewModel::from_widget("text", __props)
        };

        if let Some(runtime) = ba.services.try_runtime_handle() {
            let props_handle = vm.props.clone();
            let task = runtime.spawn(data.signal_cloned().for_each(move |row| {
                if let Some(status) = super::prelude::content_from_row(&row, &field) {
                    let (symbol, color) = resolve(&status);
                    let mut props = props_handle.lock_mut();
                    props.insert("content".to_string(), Value::String(symbol.to_string()));
                    props.insert("color".to_string(), Value::String(color.to_string()));
                }
                async {}
            }));
            vm.subscriptions.push(DropTask::new(task));
        }

        vm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_word_has_its_own_glyph() {
        let mapped: Vec<&str> = ["Connected", "Pending", "Needs auth", "Unavailable"]
            .iter()
            .map(|s| status_symbol_and_color(s).expect("mapped status").0)
            .collect();
        let mut distinct = mapped.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            mapped.len(),
            "two statuses sharing a glyph would read as the same state: {mapped:?}"
        );
    }

    #[test]
    fn an_unmapped_status_is_disclosed_not_faked() {
        assert_eq!(status_symbol_and_color("Reticulating"), None);
        assert_eq!(resolve("Reticulating"), UNKNOWN);
    }
}
