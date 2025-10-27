use std::collections::HashMap;

use holon_api::streaming::Change;
use holon_api::widget_spec::{ResolvedRow, WidgetSpec};
use holon_api::{UiEvent, Value, WatchHandle};

/// CDC state accumulator that applies UiEvent deltas to a WidgetSpec.
///
/// Framework-agnostic: the `notify` callback bridges to whatever UI update
/// mechanism the frontend uses (Dioxus signals, WaterUI bindings, etc.).
pub struct CdcState {
    widget_spec: WidgetSpec,
    generation: u64,
    notify: Box<dyn Fn(WidgetSpec) + Send>,
}

impl CdcState {
    pub fn new(initial: WidgetSpec, notify: impl Fn(WidgetSpec) + Send + 'static) -> Self {
        Self {
            widget_spec: initial,
            generation: 0,
            notify: Box::new(notify),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn replace_widget_spec(&mut self, widget_spec: WidgetSpec, generation: u64) {
        self.widget_spec = widget_spec;
        self.generation = generation;
        (self.notify)(self.widget_spec.clone());
    }

    pub fn insert_row(&mut self, data: HashMap<String, Value>) {
        self.widget_spec.data.push(ResolvedRow {
            data,
            profile: None,
        });
        (self.notify)(self.widget_spec.clone());
    }

    pub fn update_row(&mut self, id: &str, new_data: HashMap<String, Value>) {
        if let Some(row) = self
            .widget_spec
            .data
            .iter_mut()
            .find(|r| r.data.get("id").and_then(|v| v.as_string()) == Some(id))
        {
            row.data = new_data;
        }
        (self.notify)(self.widget_spec.clone());
    }

    pub fn remove_row(&mut self, id: &str) {
        self.widget_spec
            .data
            .retain(|r| r.data.get("id").and_then(|v| v.as_string()) != Some(id));
        (self.notify)(self.widget_spec.clone());
    }

    pub fn patch_row(&mut self, id: &str, fields: &[(String, Value)]) {
        if let Some(row) = self
            .widget_spec
            .data
            .iter_mut()
            .find(|r| r.data.get("id").and_then(|v| v.as_string()) == Some(id))
        {
            for (field, value) in fields {
                row.data.insert(field.clone(), value.clone());
            }
        }
        (self.notify)(self.widget_spec.clone());
    }
}

/// Async loop that receives UiEvents and applies them to a CdcState.
///
/// Takes ownership of the `WatchHandle`, keeping both the event receiver and
/// command sender alive for the lifetime of the listener.
pub async fn ui_event_listener(mut watch: WatchHandle, mut state: CdcState) {
    while let Some(event) = watch.recv().await {
        match event {
            UiEvent::Structure {
                widget_spec,
                generation,
            } => {
                tracing::info!(
                    generation,
                    rows = widget_spec.data.len(),
                    "Structural update received"
                );
                state.replace_widget_spec(widget_spec, generation);
            }
            UiEvent::Data { batch, generation } => {
                if state.generation() != generation {
                    tracing::debug!(
                        generation,
                        current = state.generation(),
                        "Discarding stale data event"
                    );
                    continue;
                }
                for map_change in batch.inner.items {
                    match map_change {
                        Change::Created { data, .. } => {
                            state.insert_row(data.data);
                        }
                        Change::Updated { ref id, data, .. } => {
                            state.update_row(id, data.data);
                        }
                        Change::Deleted { ref id, .. } => {
                            state.remove_row(id);
                        }
                        Change::FieldsChanged {
                            ref entity_id,
                            ref fields,
                            ..
                        } => {
                            let patches: Vec<_> = fields
                                .iter()
                                .map(|(name, _old, new)| (name.clone(), new.clone()))
                                .collect();
                            state.patch_row(entity_id, &patches);
                        }
                    }
                }
            }
        }
    }
    tracing::info!("UiEvent stream ended");
}
