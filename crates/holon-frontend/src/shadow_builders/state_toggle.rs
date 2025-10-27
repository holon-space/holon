use super::prelude::*;
use crate::reactive_view_model::DropTask;
use futures_signals::signal::SignalExt;
use holon_api::render_eval::{resolve_states, state_display};

holon_macros::widget_builder! {
    raw fn state_toggle(ba: BA<'_>) -> ViewModel {
        // state_toggle(col("task_state")): we need the field NAME, not the resolved value.
        let field = ba
            .args
            .get_positional_column_name(0)
            .map(|s| s.to_string())
            .or_else(|| ba.args.get_string("field").map(|s| s.to_string()))
            .or_else(|| ba.args.get_positional_string(0))
            .unwrap_or_else(|| "task_state".to_string());

        let row_arc = ba.ctx.row_arc();
        let current = row_arc
            .get(&field)
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();

        let states = resolve_states(ba.args, ba.ctx.row()).join(",");
        let (label, _semantic) = state_display(&current);
        let label = label.to_string();

        let mut __props = std::collections::HashMap::new();
        __props.insert("field".to_string(), Value::String(field.clone()));
        __props.insert("current".to_string(), Value::String(current));
        __props.insert("label".to_string(), Value::String(label));
        __props.insert("states".to_string(), Value::String(states));

        // Wire the leaf to the shared per-row signal cell. `data_mutable()`
        // returns a `ReadOnlyMutable` clone of the cell owned by
        // `ReactiveRowSet`; cloning shares the same `Arc<MutableState>`.
        // When CDC writes the row through `apply_change`, the subscription
        // below fires and re-derives `current` + `label` on the leaf's
        // own `props` Mutable. No tree walk, no `set_data`, no manual
        // propagation — that's the architectural fix for the
        // task-state-toggle bug.
        let data = ba.ctx.data_mutable();
        let mut vm = ViewModel {
            operations: ba.ctx.operations.clone(),
            data: data.clone(),
            ..ViewModel::from_widget("state_toggle", __props)
        };
        // Skip subscription setup in sync-only contexts (PBT reference
        // model, shadow interpretation): no runtime, nothing would
        // observe live updates anyway. The snapshot baked into `__props`
        // above is the final value those call sites need.
        if let Some(runtime) = ba.services.try_runtime_handle() {
            let props_handle = vm.props.clone();
            let derive = move |row: Arc<holon_api::widget_spec::DataRow>| {
                let new_state = row
                    .get(&field)
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();
                let (label, _) = state_display(&new_state);
                let label = label.to_string();
                let mut p = props_handle.lock_mut();
                p.insert("current".to_string(), Value::String(new_state));
                p.insert("label".to_string(), Value::String(label));
            };
            // The initial signal emission re-sets the same values we
            // already baked into `__props` — a no-op `.insert()` of the
            // same entries. Cheaper than threading `skip(1)` through the
            // futures pipeline.
            let task = runtime.spawn(data.signal_cloned().for_each(move |row| {
                derive(row);
                async {}
            }));
            vm.subscriptions.push(DropTask::new(task));
        }
        vm
    }
}
