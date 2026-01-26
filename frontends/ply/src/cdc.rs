use holon_api::streaming::Change;
use holon_api::UiEvent;

use crate::state::AppState;

/// Apply a single UiEvent to AppState. Returns true if the state changed.
pub fn apply_event(state: &AppState, event: UiEvent) -> bool {
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
            true
        }
        UiEvent::Data { batch, generation } => {
            if !state.is_current_generation(generation) {
                tracing::debug!(
                    generation,
                    current = state.generation(),
                    "Discarding stale data event"
                );
                return false;
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
            true
        }
        UiEvent::CollectionUpdate { .. } => {
            // Lazy expansion — handled by frontend directly
            false
        }
    }
}
