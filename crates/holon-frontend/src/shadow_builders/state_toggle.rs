use futures_signals::signal::SignalExt;
use holon_api::render_eval::resolve_states;
use holon_api::render_eval::state_display;

use super::prelude::*;
use crate::reactive_view_model::DropTask;

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

        // Both parsed here, at the DSL boundary, so an unknown word is one
        // refusal rather than a mis-painted control every frontend re-derives.
        let appearance = match ba.args.get_string("appearance") {
            Some(raw) => crate::view_model::StateToggleAppearance::parse(raw)
                .unwrap_or_else(|e| panic!("{e}")),
            None => crate::view_model::StateToggleAppearance::default(),
        };
        let binding = match ba.args.get_string("binding") {
            Some(raw) => {
                crate::view_model::StateToggleBinding::parse(raw).unwrap_or_else(|e| panic!("{e}"))
            }
            None => crate::view_model::StateToggleBinding::default(),
        };

        // `current` carries the bound value at its own type: a state WORD, or a
        // bool. `state_display` speaks only the word vocabulary, so the bool
        // arm has no label to show — its control is the switch itself.
        let bound = field.clone();
        let read_current = move |row: &holon_api::widget_spec::DataRow| -> (Value, String) {
            match binding {
                crate::view_model::StateToggleBinding::Bool => {
                    let on = crate::view_model::bool_from_row_value(&bound, row.get(&bound))
                        .unwrap_or_else(|e| panic!("{e}"));
                    (Value::Boolean(on), String::new())
                }
                crate::view_model::StateToggleBinding::Words => {
                    let word = row.get(&bound).and_then(|v| v.as_string()).unwrap_or("");
                    let (label, _semantic) = state_display(word);
                    (Value::String(word.to_string()), label.to_string())
                }
            }
        };
        let (current, label) = read_current(&row_arc);

        let states = resolve_states(ba.args, ba.ctx.row()).join(",");

        let mt = ba.args.get_f64("mt").unwrap_or(0.0);

        let mut __props = std::collections::HashMap::new();
        __props.insert(
            "appearance".to_string(),
            Value::String(appearance.as_str().to_string()),
        );
        __props.insert(
            "binding".to_string(),
            Value::String(binding.as_str().to_string()),
        );
        __props.insert("field".to_string(), Value::String(field.clone()));
        __props.insert("current".to_string(), current);
        __props.insert("label".to_string(), Value::String(label));
        __props.insert("states".to_string(), Value::String(states));
        __props.insert("mt".to_string(), Value::Float(mt));

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
                let (current, label) = read_current(&row);
                let mut p = props_handle.lock_mut();
                p.insert("current".to_string(), current);
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
