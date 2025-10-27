//! Framework-agnostic editor controller.
//!
//! Owns the ViewEventHandler and input triggers for an editable text node.
//! Frontends create one per editable field and feed it platform events.
//! The controller returns `EditorAction` values that the frontend executes
//! using its platform-specific APIs.

use std::collections::HashMap;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;

use holon_api::render_types::OperationWiring;
use holon_api::types::EntityName;
use holon_api::{InlineMark, MarkSpan, Value};

use crate::input_trigger::{self, InputTrigger, ViewEvent};
use crate::operations::OperationIntent;
use crate::popup_menu::{MenuKey, PopupItem, PopupResult, PopupState};
use crate::reactive::BuilderServices;
use crate::view_event_handler::{HandleResult, ViewEventHandler};

/// Actions the frontend should execute after calling EditorController methods.
///
/// The controller decides *what* to do; the frontend decides *how* to do it
/// using platform-specific APIs.
pub enum EditorAction {
    /// Nothing to do.
    None,

    /// Re-render the popup overlay (items or selection changed).
    UpdatePopup,

    /// A popup was just activated. The frontend must watch this signal
    /// and call `notify_items_changed()` on each emission to keep the
    /// popup state in sync.
    PopupActivated {
        signal: Pin<Box<dyn futures_signals::signal::Signal<Item = Vec<PopupItem>> + Send>>,
    },

    /// The popup was dismissed. Frontend should hide the overlay.
    PopupDismissed,

    /// Dispatch an operation (slash command selected, text synced on blur, etc.).
    Execute(OperationIntent),

    /// Insert text at a position (wiki-link selected).
    /// `prefix_start` is the column where the trigger prefix started (e.g., `[[`).
    /// Frontend should replace text from `line_start + prefix_start` to `cursor` with `replacement`.
    InsertText {
        replacement: String,
        prefix_start: usize,
    },

    /// Let the parent handle this key (popup is not active).
    /// E.g., MoveUp/MoveDown should propagate to cross-block navigation.
    Propagate,
}

impl std::fmt::Debug for EditorAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::UpdatePopup => write!(f, "UpdatePopup"),
            Self::PopupActivated { .. } => write!(f, "PopupActivated {{ signal: ... }}"),
            Self::PopupDismissed => write!(f, "PopupDismissed"),
            Self::Execute(intent) => write!(f, "Execute({:?})", intent),
            Self::InsertText {
                replacement,
                prefix_start,
            } => {
                write!(
                    f,
                    "InsertText {{ replacement: {:?}, prefix_start: {} }}",
                    replacement, prefix_start
                )
            }
            Self::Propagate => write!(f, "Propagate"),
        }
    }
}

/// Framework-agnostic controller for an editable text field.
///
/// Each editable text node in the ViewModel gets one controller.
/// The frontend creates it during reconciliation and calls its methods
/// from platform event handlers.
pub struct EditorController {
    handler: ViewEventHandler,
    triggers: Vec<InputTrigger>,
}

impl EditorController {
    pub fn new(
        operations: Vec<OperationWiring>,
        triggers: Vec<InputTrigger>,
        context_params: HashMap<String, Value>,
        field: String,
        original_value: String,
    ) -> Self {
        let handler = ViewEventHandler::new(operations, context_params, field, original_value);
        Self { handler, triggers }
    }

    /// Build an EditorController from an EditableText ViewModel node.
    ///
    /// Extracts field, content, operations, triggers, and context params from the node.
    /// Panics if the node is not an EditableText.
    pub fn from_view_model(node: &crate::ViewModel) -> Self {
        let (field, content) = match &node.kind {
            crate::view_model::ViewKind::EditableText { field, content } => {
                (field.clone(), content.clone())
            }
            _ => panic!(
                "EditorController::from_view_model called on non-EditableText node: {:?}",
                node.widget_name()
            ),
        };
        let context_params: HashMap<String, Value> = node
            .entity
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self::new(
            node.operations.clone(),
            node.triggers.clone(),
            context_params,
            field,
            content,
        )
    }

    /// Directly populate popup items (for tests / sync providers).
    pub fn set_popup_items(&mut self, items: Vec<PopupItem>) {
        self.handler.popup.set_items(items);
    }

    /// Enable async providers. `LinkProvider` needs a `BuilderServices`
    /// handle to run doc-link SQL queries against a real backend.
    pub fn set_async_context(&mut self, services: Arc<dyn BuilderServices>) {
        self.handler.set_async_context(services);
    }

    /// Called when the text content changes (every keystroke).
    ///
    /// `current_line` is the line the cursor is on.
    /// `cursor_column` is the cursor's column within that line.
    pub fn on_text_changed(&mut self, current_line: &str, cursor_column: usize) -> EditorAction {
        let view_event = input_trigger::check_triggers(&self.triggers, current_line, cursor_column);

        let result = if let Some(event) = view_event {
            self.handler.handle(event)
        } else if self.handler.is_overlay_active() {
            self.handler.handle(ViewEvent::TriggerDismissed)
        } else {
            return EditorAction::None;
        };

        self.handle_result_to_action(result)
    }

    /// Called when the editor loses focus (blur).
    ///
    /// If the text changed, returns `Execute` with a set_field operation.
    pub fn on_blur(&mut self, current_value: &str) -> EditorAction {
        let result = self.handler.handle(ViewEvent::TextSync {
            value: current_value.to_string(),
        });
        self.handle_result_to_action(result)
    }

    /// Called when a navigation key is pressed (Up/Down/Enter/Escape).
    ///
    /// If the popup is active, the key is routed to the popup.
    /// Otherwise returns `Propagate` so the frontend can handle
    /// cross-block navigation or other default behavior.
    pub fn on_key(&mut self, key: EditorKey) -> EditorAction {
        if !self.handler.is_overlay_active() {
            return match key {
                EditorKey::Enter => EditorAction::None, // let Input handle newline
                EditorKey::Escape => EditorAction::Propagate,
                EditorKey::Up | EditorKey::Down => EditorAction::Propagate,
            };
        }

        let menu_key = match key {
            EditorKey::Up => MenuKey::Up,
            EditorKey::Down => MenuKey::Down,
            EditorKey::Enter => MenuKey::Enter,
            EditorKey::Escape => MenuKey::Escape,
        };

        let result = self.handler.on_key(menu_key);
        self.popup_result_to_action(result)
    }

    /// Whether the popup overlay is currently visible.
    pub fn is_popup_active(&self) -> bool {
        self.handler.is_overlay_active()
    }

    /// Current popup state for rendering. Returns `None` if no popup is active.
    pub fn popup_state(&self) -> Option<PopupState> {
        self.handler.popup.popup_state()
    }

    /// Apply an inline mark over a range of the block's text.
    ///
    /// Range is in Unicode-scalar offsets, half-open `[range.start, range.end)`.
    /// Returns an `Execute(OperationIntent)` for the `apply_mark` operation
    /// on the `block` entity; the frontend dispatches it through its standard
    /// operation pipeline. This is incremental — pre-existing marks of other
    /// keys, or same-key spans on disjoint ranges, are preserved.
    ///
    /// Returns `EditorAction::None` if the controller's context has no `id`
    /// (a programming error in the wiring; logged by callers if needed).
    pub fn apply_mark(&self, range: Range<usize>, mark: &InlineMark) -> EditorAction {
        let Some(id) = self.handler.context_id() else {
            return EditorAction::None;
        };
        let mark_json = serde_json::to_string(mark).expect("InlineMark serialization is total");
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("range_start".into(), Value::Integer(range.start as i64));
        params.insert("range_end".into(), Value::Integer(range.end as i64));
        params.insert("mark_json".into(), Value::String(mark_json));
        EditorAction::Execute(OperationIntent::new(
            EntityName::new("block"),
            "apply_mark".into(),
            params,
        ))
    }

    /// Remove an inline mark over a range of the block's text.
    ///
    /// The `mark` argument's `loro_key()` selects which key to unmark. The
    /// mark's value (e.g. a Link's target) is ignored — `unmark` is a
    /// range-based operation that doesn't need the original value to
    /// identify what to remove.
    pub fn remove_mark(&self, range: Range<usize>, mark: &InlineMark) -> EditorAction {
        let Some(id) = self.handler.context_id() else {
            return EditorAction::None;
        };
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("range_start".into(), Value::Integer(range.start as i64));
        params.insert("range_end".into(), Value::Integer(range.end as i64));
        params.insert("key".into(), Value::String(mark.loro_key().into()));
        EditorAction::Execute(OperationIntent::new(
            EntityName::new("block"),
            "remove_mark".into(),
            params,
        ))
    }

    fn handle_result_to_action(&self, result: HandleResult) -> EditorAction {
        match result {
            HandleResult::Activated { signal } => EditorAction::PopupActivated { signal },
            HandleResult::PopupResult(pr) => self.popup_result_to_action(pr),
        }
    }

    fn popup_result_to_action(&self, result: PopupResult) -> EditorAction {
        match result {
            PopupResult::NotActive => EditorAction::None,
            PopupResult::Updated => EditorAction::UpdatePopup,
            PopupResult::Dismissed => EditorAction::PopupDismissed,
            PopupResult::Execute {
                entity_name,
                op_name,
                params,
            } => EditorAction::Execute(OperationIntent::new(entity_name, op_name, params)),
            PopupResult::InsertText {
                replacement,
                prefix_start,
            } => EditorAction::InsertText {
                replacement,
                prefix_start,
            },
        }
    }
}

/// Abstract keyboard keys that the editor controller handles.
///
/// Frontends map their platform-specific key types to this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKey {
    Up,
    Down,
    Enter,
    Escape,
}

/// Marks that fully cover `[range.start, range.end)`.
///
/// Used by toolbars to drive the "this button is ON" state — Bold appears
/// pressed when every scalar in the selection is Bold. A selection where
/// only part is Bold returns no Bold entry; the toolbar should treat that
/// as "mixed" / "off" depending on its UX choice (this helper deliberately
/// doesn't compute a tri-state since callers' definitions of "mixed" vary).
///
/// Empty selections (`range.start == range.end`) treat the position as a
/// caret: a mark covers the caret iff `mark.start <= pos && mark.end >= pos`.
/// (At the right boundary, `ExpandType::After` marks are reported as active
/// so toolbar state matches what typing-at-the-boundary would inherit.)
pub fn selection_marks(marks: &[MarkSpan], range: Range<usize>) -> Vec<InlineMark> {
    marks
        .iter()
        .filter(|m| m.start <= range.start && m.end >= range.end)
        .map(|m| m.mark.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_trigger::InputTrigger;
    use holon_api::render_types::{OperationDescriptor, OperationParam, TypeHint};
    use holon_api::types::EntityName;

    fn make_op(name: &str, fields: &[&str], params: Vec<OperationParam>) -> OperationWiring {
        OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                name: name.into(),
                display_name: name.into(),
                required_params: params,
                affected_fields: fields.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
        }
    }

    fn param(name: &str) -> OperationParam {
        OperationParam {
            name: name.into(),
            type_hint: TypeHint::String,
            description: String::new(),
        }
    }

    fn test_controller() -> EditorController {
        let ops = vec![
            make_op(
                "set_field",
                &["content"],
                vec![param("id"), param("field"), param("value")],
            ),
            make_op("delete", &["parent_id"], vec![param("id")]),
        ];
        let triggers = vec![
            InputTrigger::TextPrefix {
                prefix: "/".to_string(),
                action: "command_menu".to_string(),
                at_line_start: true,
            },
            InputTrigger::TextPrefix {
                prefix: "[[".to_string(),
                action: "doc_link".to_string(),
                at_line_start: false,
            },
        ];
        let context = HashMap::from([("id".into(), Value::String("block-1".into()))]);
        EditorController::new(ops, triggers, context, "content".into(), "original".into())
    }

    #[test]
    fn normal_text_returns_none() {
        let mut ctrl = test_controller();
        let action = ctrl.on_text_changed("hello world", 11);
        assert!(matches!(action, EditorAction::None));
    }

    #[test]
    fn slash_at_start_activates_popup() {
        let mut ctrl = test_controller();
        let action = ctrl.on_text_changed("/", 1);
        assert!(matches!(action, EditorAction::PopupActivated { .. }));
        assert!(ctrl.is_popup_active());
    }

    #[test]
    fn key_up_propagates_when_no_popup() {
        let mut ctrl = test_controller();
        let action = ctrl.on_key(EditorKey::Up);
        assert!(matches!(action, EditorAction::Propagate));
    }

    #[test]
    fn key_up_updates_popup_when_active() {
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/", 1);
        // Manually set items so popup has something to navigate
        ctrl.handler.popup.set_items(vec![
            PopupItem {
                id: "a".into(),
                label: "A".into(),
                icon: None,
            },
            PopupItem {
                id: "b".into(),
                label: "B".into(),
                icon: None,
            },
        ]);
        let action = ctrl.on_key(EditorKey::Down);
        assert!(matches!(action, EditorAction::UpdatePopup));
    }

    #[test]
    fn escape_dismisses_active_popup() {
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/", 1);
        let action = ctrl.on_key(EditorKey::Escape);
        assert!(matches!(action, EditorAction::PopupDismissed));
        assert!(!ctrl.is_popup_active());
    }

    #[test]
    fn escape_propagates_when_no_popup() {
        let mut ctrl = test_controller();
        let action = ctrl.on_key(EditorKey::Escape);
        assert!(matches!(action, EditorAction::Propagate));
    }

    #[test]
    fn enter_executes_selected_command() {
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/", 1);
        ctrl.handler.popup.set_items(vec![PopupItem {
            id: "delete".into(),
            label: "Delete".into(),
            icon: None,
        }]);
        let action = ctrl.on_key(EditorKey::Enter);
        match action {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.op_name, "delete");
                assert_eq!(intent.params["id"], Value::String("block-1".into()));
            }
            other => panic!("Expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn blur_with_changed_text_executes() {
        let mut ctrl = test_controller();
        let action = ctrl.on_blur("new text");
        match action {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.op_name, "set_field");
                assert_eq!(intent.params["value"], Value::String("new text".into()));
                assert_eq!(intent.params["field"], Value::String("content".into()));
            }
            other => panic!("Expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn blur_with_same_text_returns_none() {
        let mut ctrl = test_controller();
        let action = ctrl.on_blur("original");
        assert!(matches!(action, EditorAction::None));
    }

    #[test]
    fn double_bracket_fires_doc_link() {
        let mut ctrl = test_controller();
        // Without async context, doc_link returns None (no LinkProvider)
        let action = ctrl.on_text_changed("see [[proj", 10);
        assert!(matches!(action, EditorAction::None));
    }

    #[test]
    fn text_change_dismisses_stale_popup() {
        let mut ctrl = test_controller();
        ctrl.on_text_changed("/del", 4); // activates popup
        assert!(ctrl.is_popup_active());
        // Type normal text (no trigger match) → should dismiss
        let action = ctrl.on_text_changed("hello", 5);
        assert!(matches!(action, EditorAction::PopupDismissed));
        assert!(!ctrl.is_popup_active());
    }

    #[test]
    fn apply_mark_emits_apply_mark_intent() {
        let ctrl = test_controller();
        let action = ctrl.apply_mark(0..5, &InlineMark::Bold);
        match action {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.entity_name, EntityName::new("block"));
                assert_eq!(intent.op_name, "apply_mark");
                assert_eq!(intent.params["id"], Value::String("block-1".into()));
                assert_eq!(intent.params["range_start"], Value::Integer(0));
                assert_eq!(intent.params["range_end"], Value::Integer(5));
                let mark_json = intent.params["mark_json"]
                    .as_string()
                    .expect("mark_json string");
                let mark: InlineMark =
                    serde_json::from_str(mark_json).expect("mark_json round-trips");
                assert_eq!(mark, InlineMark::Bold);
            }
            other => panic!("expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn apply_mark_round_trips_link_target() {
        // Link variants carry data (target + label); intent payload must
        // preserve them so the backend reconstitutes the full InlineMark.
        use holon_api::{EntityRef, EntityUri};

        let ctrl = test_controller();
        let mark = InlineMark::Link {
            target: EntityRef::Internal {
                id: EntityUri::block("abc-123"),
            },
            label: "see also".into(),
        };
        let action = ctrl.apply_mark(2..10, &mark);
        let EditorAction::Execute(intent) = action else {
            panic!("expected Execute");
        };
        let mark_json = intent.params["mark_json"].as_string().unwrap();
        let parsed: InlineMark = serde_json::from_str(mark_json).unwrap();
        assert_eq!(parsed, mark);
    }

    #[test]
    fn remove_mark_emits_remove_mark_intent_with_key() {
        let ctrl = test_controller();
        let action = ctrl.remove_mark(3..7, &InlineMark::Italic);
        match action {
            EditorAction::Execute(intent) => {
                assert_eq!(intent.op_name, "remove_mark");
                assert_eq!(intent.params["range_start"], Value::Integer(3));
                assert_eq!(intent.params["range_end"], Value::Integer(7));
                assert_eq!(intent.params["key"], Value::String("italic".into()));
                // No mark_json — remove only needs the key.
                assert!(!intent.params.contains_key("mark_json"));
            }
            other => panic!("expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn selection_marks_returns_marks_fully_covering_range() {
        let marks = vec![
            MarkSpan::new(0, 10, InlineMark::Bold),     // covers selection
            MarkSpan::new(5, 7, InlineMark::Italic),    // doesn't cover selection
            MarkSpan::new(0, 8, InlineMark::Underline), // covers selection
        ];
        let active = selection_marks(&marks, 2..8);
        assert!(active.contains(&InlineMark::Bold));
        assert!(active.contains(&InlineMark::Underline));
        assert!(!active.contains(&InlineMark::Italic));
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn selection_marks_partial_cover_excludes() {
        // Mark must cover the ENTIRE range — half-overlap doesn't count.
        let marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
        let active = selection_marks(&marks, 3..7);
        assert!(active.is_empty(), "Bold over [0,5) does NOT cover [3,7)");
    }

    #[test]
    fn selection_marks_caret_at_mark_boundary() {
        // Empty range = caret. Caret at position 5 inside Bold([0,5)) — the
        // mark's right boundary. Per ExpandType::After, typing here inherits
        // Bold, so the toolbar should show Bold ON.
        let marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
        let active = selection_marks(&marks, 5..5);
        assert!(active.contains(&InlineMark::Bold));
    }
}
