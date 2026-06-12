//! Handles ViewEvents for editable text nodes.
//!
//! Routes trigger events to the appropriate popup provider (command menu,
//! doc links, etc.) and handles text sync for persistence.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_types::OperationWiring;
use holon_api::{EntityName, Value};

use crate::command_provider::CommandProvider;
use crate::input_trigger::ViewEvent;
use crate::link_provider::LinkProvider;
use crate::operations::find_set_field_op;
use crate::popup_menu::{MenuKey, PopupMenu, PopupResult};
use crate::reactive::BuilderServices;

/// Handles ViewEvents for an editable text node.
///
/// Each editable text node gets one of these. It routes ViewEvents to the
/// appropriate handler (popup menu with providers) and returns actions for
/// the frontend to execute.
pub struct ViewEventHandler {
    pub popup: PopupMenu,
    operations: Vec<OperationWiring>,
    /// The field this editable text node is editing (e.g., "content").
    field: String,
    /// The original text value when the handler was created.
    original_value: String,
    /// Pre-resolved operation metadata for the set_field path.
    set_field_entity: Option<EntityName>,
    set_field_op: Option<String>,
    /// Context params (includes row id, etc.)
    context_params: HashMap<String, Value>,
    /// Builder services for async popup providers (doc-link search).
    /// Narrow capability surface — holds everything a `LinkProvider` needs
    /// (runtime handle + popup_query). Replaces the old `FrontendSession`
    /// plumb line that had to be threaded through the entire render path.
    services: Option<Arc<dyn BuilderServices>>,
    /// Whether a per-keystroke Loro content writer (a `Cell<String>` backed
    /// by `MutableText`) is active for this editor. When `true`, the on-blur
    /// `set_field("content", …)` is dropped as a redundant second writer that
    /// would race the Loro projection. When `false` (SqlOnly mode, no Loro
    /// backing, headless/test paths), the on-blur `set_field` is the ONLY
    /// content writer and MUST fire — otherwise content edits are silently
    /// lost. Set via [`Self::set_loro_content_writer`] when the cell attaches.
    loro_content_writer: bool,
}

impl ViewEventHandler {
    pub fn new(
        operations: Vec<OperationWiring>,
        context_params: HashMap<String, Value>,
        field: String,
        original_value: String,
    ) -> Self {
        let op = find_set_field_op(&field, &operations);
        let set_field_entity = op.map(|o| o.entity_name.clone());
        let set_field_op = op.map(|o| o.name.clone());

        Self {
            popup: PopupMenu::new(),
            operations,
            field,
            original_value,
            set_field_entity,
            set_field_op,
            context_params,
            services: None,
            loro_content_writer: false,
        }
    }

    /// Set the `BuilderServices` handle for async popup providers (link search).
    /// Must be called before link triggers will work.
    pub fn set_async_context(&mut self, services: Arc<dyn BuilderServices>) {
        self.services = Some(services);
    }

    /// Declare whether a per-keystroke Loro content writer is active for this
    /// editor (see [`Self::loro_content_writer`]). The GPUI/TUI frontends call
    /// this with `true` once `BuilderServices::editable_text` resolves a
    /// `Cell<String>`; when no cell resolves (SqlOnly mode), it stays `false`
    /// so the on-blur `set_field("content")` remains the content writer.
    pub fn set_loro_content_writer(&mut self, active: bool) {
        self.loro_content_writer = active;
    }

    /// The block (or row) id the controller is editing, read from
    /// `context_params["id"]`. `None` if the context wasn't populated with
    /// an id — the caller should treat that as "no-op" rather than panic.
    pub fn context_id(&self) -> Option<&str> {
        self.context_params.get("id").and_then(|v| v.as_string())
    }

    /// Process a ViewEvent from the frontend's trigger check.
    /// Returns a PopupResult telling the frontend what to do.
    pub fn handle(&mut self, event: ViewEvent) -> HandleResult {
        match event {
            ViewEvent::TriggerFired {
                action,
                filter_text,
                prefix_start,
            } => match action.as_str() {
                "command_menu" => {
                    if !self.popup.is_active() {
                        let provider = Arc::new(
                            CommandProvider::new(
                                self.operations.clone(),
                                self.context_params.clone(),
                            )
                            .with_prefix_start(prefix_start),
                        );
                        let signal = self.popup.activate(provider, &filter_text);
                        HandleResult::Activated { signal }
                    } else {
                        // `filter_text` is the text between the matched "/"
                        // and the cursor — the same value `activate` receives.
                        // The old `current_line[1..]` assumed the slash sat at
                        // line start (it fires mid-line) and byte-sliced into
                        // multibyte leading chars.
                        self.popup.on_text_changed(&filter_text);
                        HandleResult::PopupResult(PopupResult::Updated)
                    }
                }
                "doc_link" => {
                    let Some(services) = self.services.as_ref() else {
                        return HandleResult::PopupResult(PopupResult::NotActive);
                    };
                    if !self.popup.is_active() {
                        let provider = Arc::new(LinkProvider::new(prefix_start, services.clone()));
                        let signal = self.popup.activate(provider, &filter_text);
                        HandleResult::Activated { signal }
                    } else {
                        self.popup.on_text_changed(&filter_text);
                        HandleResult::PopupResult(PopupResult::Updated)
                    }
                }
                _ => HandleResult::PopupResult(PopupResult::NotActive),
            },

            ViewEvent::TriggerDismissed => {
                if self.popup.is_active() {
                    self.popup.dismiss();
                    HandleResult::PopupResult(PopupResult::Dismissed)
                } else {
                    HandleResult::PopupResult(PopupResult::NotActive)
                }
            }

            ViewEvent::TextSync { value } => {
                HandleResult::PopupResult(self.handle_text_sync(value))
            }
        }
    }

    /// Handle Tier 3 text sync (blur). If the value changed and we have a
    /// set_field operation, return Execute with the appropriate params.
    ///
    /// Virtual entities (`virtual:{entity_type}:{parent_id}`) are materialized
    /// via `{entity_type}.create` instead of `set_field`. After creation, the
    /// CDC delivers the real entity and a new virtual row appears at the end.
    ///
    /// **Phase 2 (Loro single-writer):** for `content` on real (non-virtual)
    /// entities, when a Loro content writer is active the per-keystroke
    /// pipeline already writes through `MutableText` → Loro →
    /// `LoroSyncController.on_loro_changed` → SQL, so the on-blur
    /// `set_field("content", …)` is a redundant second writer that races with
    /// the Loro projection — drop it (gated on [`Self::loro_content_writer`]).
    /// But when Loro is NOT the writer (SqlOnly mode: no `Cell` attaches, so
    /// the per-keystroke `vm.has_cell()` branch in `editor_view.rs` is skipped)
    /// the on-blur `set_field` is the ONLY content writer and MUST fire, else
    /// the edit is silently discarded. Virtual-entity creates (which
    /// materialize a new entity, not edit content) are unaffected, and
    /// non-content fields always go through `set_field`.
    fn handle_text_sync(&mut self, new_value: String) -> PopupResult {
        if new_value == self.original_value {
            return PopupResult::NotActive;
        }
        self.original_value = new_value.clone();

        let id = self
            .context_params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("ViewEventHandler context_params missing 'id'")
            .to_string();

        if let Some((entity_type, parent_id)) = parse_virtual_id(&id) {
            if new_value.is_empty() {
                return PopupResult::NotActive;
            }
            let mut params = HashMap::new();
            params.insert("parent_id".into(), Value::String(parent_id));
            params.insert("content".into(), Value::String(new_value));
            return PopupResult::Execute {
                entity_name: EntityName::Named(entity_type),
                op_name: "create".to_string(),
                params,
                strip_prefix_start: None,
            };
        }

        if self.field == "content" && self.loro_content_writer {
            return PopupResult::NotActive;
        }

        let (Some(entity_name), Some(op_name)) = (&self.set_field_entity, &self.set_field_op)
        else {
            return PopupResult::NotActive;
        };

        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(id));
        params.insert("field".into(), Value::String(self.field.clone()));
        params.insert("value".into(), Value::String(new_value));

        PopupResult::Execute {
            entity_name: entity_name.clone(),
            op_name: op_name.clone(),
            params,
            strip_prefix_start: None,
        }
    }

    /// Forward keyboard events to the active popup (if any).
    pub fn on_key(&mut self, key: MenuKey) -> PopupResult {
        if self.popup.is_active() {
            return self.popup.on_key(key);
        }
        PopupResult::NotActive
    }

    /// Whether any overlay (popup menu, autocomplete, etc.) is currently active.
    pub fn is_overlay_active(&self) -> bool {
        self.popup.is_active()
    }
}

/// Parse a virtual entity ID of the form `<entity>:__virtual:<parent_local>`.
///
/// The marker lives in the **local** part of the URI (not the scheme), so
/// `EntityUri::scheme()` returns the real entity type and the profile resolver
/// finds the right profile. We detect "this is a creation slot" by looking
/// for the `:__virtual:` infix.
///
/// Example: `block:__virtual:default-main-panel`
/// → entity_type = `"block"`, parent_id = `"block:default-main-panel"`.
///
/// Returns `(entity_type, parent_id)` or `None` if the ID isn't virtual.
fn parse_virtual_id(id: &str) -> Option<(String, String)> {
    let (scheme, parent_local) = id.split_once(":__virtual:")?;
    if scheme.is_empty() || parent_local.is_empty() {
        return None;
    }
    Some((scheme.to_string(), format!("{scheme}:{parent_local}")))
}

/// Result of handling a ViewEvent.
///
/// Distinguished from `PopupResult` because activation returns a signal
/// that the GPUI layer needs to watch for item updates.
pub enum HandleResult {
    /// A popup was just activated. The signal should be watched via
    /// `cx.spawn` + `for_each` + `cx.notify()`.
    Activated {
        signal: std::pin::Pin<
            Box<
                dyn futures_signals::signal::Signal<Item = Vec<crate::popup_menu::PopupItem>>
                    + Send,
            >,
        >,
    },
    /// A regular popup result (no new signal to watch).
    PopupResult(PopupResult),
}

impl std::fmt::Debug for HandleResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandleResult::Activated { .. } => write!(f, "Activated {{ signal: ... }}"),
            HandleResult::PopupResult(r) => write!(f, "PopupResult({:?})", r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::StubBuilderServices;

    /// Regression guard for the `BuilderServices` refactor. If `doc_link`
    /// activation returns `NotActive` after `set_async_context`, the
    /// `services` handle was lost somewhere between here and the
    /// `LinkProvider` (the refactor reintroduced exactly this kind of
    /// regression before this test existed).
    #[test]
    fn doc_link_trigger_activates_when_services_are_set() {
        let mut h = ViewEventHandler::new(Vec::new(), HashMap::new(), "content".into(), "".into());

        // Before set_async_context: doc_link returns NotActive.
        let result = h.handle(ViewEvent::TriggerFired {
            action: "doc_link".into(),
            filter_text: "foo".into(),
            prefix_start: 0,
        });
        assert!(
            matches!(result, HandleResult::PopupResult(PopupResult::NotActive)),
            "doc_link must be NotActive before services are wired: got {:?}",
            result
        );

        // After set_async_context with a StubBuilderServices (no backend,
        // popup_query returns Err — but the handler doesn't care about the
        // query result, only that a provider could be constructed and the
        // popup activated).
        h.set_async_context(Arc::new(StubBuilderServices::new()));

        let result = h.handle(ViewEvent::TriggerFired {
            action: "doc_link".into(),
            filter_text: "foo".into(),
            prefix_start: 0,
        });
        assert!(
            matches!(result, HandleResult::Activated { .. }),
            "doc_link must Activate after services are wired: got {:?}",
            result
        );
    }
}
