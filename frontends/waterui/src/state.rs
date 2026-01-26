use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use holon_api::widget_spec::{ResolvedRow, WidgetSpec};
use holon_api::Value;

pub struct AppState {
    inner: Arc<RwLock<AppStateInner>>,
}

struct AppStateInner {
    widget_spec: WidgetSpec,
    generation: u64,
}

impl AppState {
    pub fn new(widget_spec: WidgetSpec) -> Self {
        Self {
            inner: Arc::new(RwLock::new(AppStateInner {
                widget_spec,
                generation: 0,
            })),
        }
    }

    pub fn clone_handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn widget_spec(&self) -> WidgetSpec {
        self.inner.read().unwrap().widget_spec.clone()
    }

    pub fn replace_widget_spec(&self, widget_spec: WidgetSpec, generation: u64) {
        let mut inner = self.inner.write().unwrap();
        inner.widget_spec = widget_spec;
        inner.generation = generation;
    }

    pub fn generation(&self) -> u64 {
        self.inner.read().unwrap().generation
    }

    pub fn is_current_generation(&self, generation: u64) -> bool {
        self.inner.read().unwrap().generation == generation
    }

    pub fn insert_row(&self, data: HashMap<String, Value>) {
        let mut inner = self.inner.write().unwrap();
        inner.widget_spec.data.push(ResolvedRow {
            data,
            profile: None,
        });
    }

    pub fn update_row(&self, id: &str, new_data: HashMap<String, Value>) {
        let mut inner = self.inner.write().unwrap();
        if let Some(row) = inner
            .widget_spec
            .data
            .iter_mut()
            .find(|r| r.data.get("id").and_then(|v| v.as_string()) == Some(id))
        {
            row.data = new_data;
        }
    }

    pub fn remove_row(&self, id: &str) {
        let mut inner = self.inner.write().unwrap();
        inner
            .widget_spec
            .data
            .retain(|r| r.data.get("id").and_then(|v| v.as_string()) != Some(id));
    }

    pub fn patch_row(&self, id: &str, fields: &[(String, Value)]) {
        let mut inner = self.inner.write().unwrap();
        if let Some(row) = inner
            .widget_spec
            .data
            .iter_mut()
            .find(|r| r.data.get("id").and_then(|v| v.as_string()) == Some(id))
        {
            for (field, value) in fields {
                row.data.insert(field.clone(), value.clone());
            }
        }
    }
}
