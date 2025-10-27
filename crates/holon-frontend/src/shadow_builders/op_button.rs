use super::prelude::*;

/// Tappable affordance for a single operation.
///
/// Typical caller: `op_button(col("name"))` inside
/// `row(#{ collection: chain_ops(0), item_template: op_button(col("name")) })`.
///
/// Positional arg 0 is the op name; the remaining fields (`target_id`,
/// `display_name`) are read from the current row produced by `chain_ops` /
/// `ops_of` — both emit rows with columns `name`, `target_id`,
/// `display_name`.
///
/// GPUI owns the `op_name → icon` mapping (hardcoded table in
/// `frontends/gpui/src/render/builders/op_button.rs`). The shadow layer
/// only carries the identity fields the platform tap handler needs to
/// resolve the full `OperationDescriptor` and call
/// `BuilderServices::present_op`.
holon_macros::widget_builder! {
    raw fn op_button(ba: BA<'_>) -> ViewModel {
        let row = ba.ctx.row();
        let op_name = ba
            .args
            .get_positional_string(0)
            .or_else(|| row.get("name").and_then(|v| v.as_string().map(String::from)))
            .expect(
                "op_button: positional op_name required, and row has no 'name' column \
                 to fall back on",
            );
        let target_id = row
            .get("target_id")
            .and_then(|v| v.as_string())
            .map(String::from)
            .expect(
                "op_button: current row has no 'target_id' column — call sites \
                 must drive op_button from a chain_ops/ops_of row source",
            );
        let display_name = row
            .get("display_name")
            .and_then(|v| v.as_string())
            .map(String::from)
            .unwrap_or_else(|| op_name.clone());

        let mut __props = std::collections::HashMap::new();
        __props.insert("op_name".to_string(), Value::String(op_name));
        __props.insert("target_id".to_string(), Value::String(target_id));
        __props.insert("display_name".to_string(), Value::String(display_name));
        ViewModel::from_widget("op_button", __props)
    }
}
