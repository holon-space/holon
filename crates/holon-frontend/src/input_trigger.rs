use serde::{Deserialize, Serialize};

/// A declarative input trigger attached to a ViewModel node.
///
/// Triggers define patterns that the View checks locally on every keystroke.
/// The check is O(number of triggers on that node) — typically 1-3.
/// Only when a trigger matches does the View send a `ViewEvent` to the
/// shared ViewModel layer for processing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputTrigger {
    /// Fire when the text at the cursor position matches a prefix.
    /// `at_line_start: true` restricts to column 0 (e.g., `/` for slash commands).
    /// `at_line_start: false` matches anywhere (e.g., `[[` for doc links, `@` for mentions).
    TextPrefix {
        prefix: String,
        action: String,
        #[serde(default)]
        at_line_start: bool,
    },
}

/// Semantic event sent from the View to the ViewModel layer when a trigger fires.
///
/// The View never interprets these — it just matches triggers and sends events.
/// The ViewModel layer (shared Rust code) handles the actual logic.
#[derive(Debug, Clone)]
pub(crate) enum ViewEvent {
    /// A text trigger fired (e.g., user typed "/" at line start, or "[[" mid-line).
    TriggerFired {
        action: String,
        /// The text after the trigger prefix (e.g., "emb" after "/emb").
        filter_text: String,
        /// The current full line text.
        current_line: String,
        /// Column where the prefix starts.
        prefix_start: usize,
    },

    /// Trigger dismissed (text no longer matches the trigger pattern).
    TriggerDismissed,

    /// Text content changed (debounced sync for persistence — Tier 3).
    TextSync { value: String },
}

/// Check a text change against a set of triggers.
/// Returns a `ViewEvent::TriggerFired` for the first matching trigger, or None.
///
/// `current_line` is the line the cursor is on.
/// `cursor_column` is the cursor's column position within that line.
///
/// Called on every keystroke — must be extremely cheap.
pub(crate) fn check_triggers(
    triggers: &[InputTrigger],
    current_line: &str,
    cursor_column: usize,
) -> Option<ViewEvent> {
    for trigger in triggers {
        match trigger {
            InputTrigger::TextPrefix {
                prefix,
                action,
                at_line_start,
            } => {
                if *at_line_start {
                    if current_line.starts_with(prefix.as_str()) && cursor_column >= prefix.len() {
                        return Some(ViewEvent::TriggerFired {
                            action: action.clone(),
                            filter_text: current_line[prefix.len()..].to_string(),
                            current_line: current_line.to_string(),
                            prefix_start: 0,
                        });
                    }
                } else {
                    let end = cursor_column.min(current_line.len());
                    let text_before_cursor = &current_line[..end];
                    if let Some(pos) = text_before_cursor.rfind(prefix.as_str()) {
                        let after_prefix = pos + prefix.len();
                        let between = &current_line[after_prefix..end];
                        // If the link is already closed (]] between prefix and cursor), skip
                        if prefix == "[[" && between.contains("]]") {
                            continue;
                        }
                        return Some(ViewEvent::TriggerFired {
                            action: action.clone(),
                            filter_text: between.to_string(),
                            current_line: current_line.to_string(),
                            prefix_start: pos,
                        });
                    }
                }
            }
        }
    }
    None
}

/// Derive default input triggers from a set of operations.
///
/// Slash commands (`/`) require operations to be present.
/// Link triggers (`[[`) are always included — they work independently of operations.
pub(crate) fn default_triggers_for_operations(
    operations: &[holon_api::render_types::OperationWiring],
) -> Vec<InputTrigger> {
    let mut triggers = always_on_triggers();
    if !operations.is_empty() {
        triggers.push(InputTrigger::TextPrefix {
            prefix: "/".to_string(),
            action: "command_menu".to_string(),
            at_line_start: true,
        });
    }
    triggers
}

/// Triggers that are always active on editable text, regardless of operations.
pub fn always_on_triggers() -> Vec<InputTrigger> {
    vec![InputTrigger::TextPrefix {
        prefix: "[[".to_string(),
        action: "doc_link".to_string(),
        at_line_start: false,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slash_trigger() -> InputTrigger {
        InputTrigger::TextPrefix {
            prefix: "/".to_string(),
            action: "command_menu".to_string(),
            at_line_start: true,
        }
    }

    fn link_trigger() -> InputTrigger {
        InputTrigger::TextPrefix {
            prefix: "[[".to_string(),
            action: "doc_link".to_string(),
            at_line_start: false,
        }
    }

    fn mention_trigger() -> InputTrigger {
        InputTrigger::TextPrefix {
            prefix: "@".to_string(),
            action: "mention".to_string(),
            at_line_start: false,
        }
    }

    #[test]
    fn slash_at_line_start() {
        let triggers = vec![slash_trigger()];
        let event = check_triggers(&triggers, "/embed", 5).unwrap();
        match event {
            ViewEvent::TriggerFired {
                action,
                filter_text,
                prefix_start,
                ..
            } => {
                assert_eq!(action, "command_menu");
                assert_eq!(filter_text, "embed");
                assert_eq!(prefix_start, 0);
            }
            _ => panic!("expected TriggerFired"),
        }
    }

    #[test]
    fn slash_only_at_start() {
        let triggers = vec![slash_trigger()];
        assert!(check_triggers(&triggers, "hello /world", 12).is_none());
    }

    #[test]
    fn double_bracket_anywhere() {
        let triggers = vec![link_trigger()];
        let event = check_triggers(&triggers, "see [[proj", 10).unwrap();
        match event {
            ViewEvent::TriggerFired {
                action,
                filter_text,
                prefix_start,
                ..
            } => {
                assert_eq!(action, "doc_link");
                assert_eq!(filter_text, "proj");
                assert_eq!(prefix_start, 4);
            }
            _ => panic!("expected TriggerFired"),
        }
    }

    #[test]
    fn mention_anywhere() {
        let triggers = vec![mention_trigger()];
        let event = check_triggers(&triggers, "ask @mar", 8).unwrap();
        match event {
            ViewEvent::TriggerFired {
                action,
                filter_text,
                prefix_start,
                ..
            } => {
                assert_eq!(action, "mention");
                assert_eq!(filter_text, "mar");
                assert_eq!(prefix_start, 4);
            }
            _ => panic!("expected TriggerFired"),
        }
    }

    #[test]
    fn no_match_returns_none() {
        let triggers = vec![slash_trigger(), link_trigger()];
        assert!(check_triggers(&triggers, "plain text", 10).is_none());
    }

    #[test]
    fn empty_triggers() {
        assert!(check_triggers(&[], "/anything", 9).is_none());
    }

    #[test]
    fn cursor_before_prefix_no_match() {
        let triggers = vec![slash_trigger()];
        assert!(check_triggers(&triggers, "/embed", 0).is_none());
    }

    #[test]
    fn closed_link_does_not_fire() {
        let triggers = vec![link_trigger()];
        // Link is fully closed: [[proj]] — should NOT fire
        assert!(check_triggers(&triggers, "see [[proj]]", 12).is_none());
    }

    #[test]
    fn unclosed_link_fires() {
        let triggers = vec![link_trigger()];
        // Cursor inside unclosed link: [[proj with cursor at 10
        let event = check_triggers(&triggers, "see [[proj", 10).unwrap();
        match event {
            ViewEvent::TriggerFired { filter_text, .. } => {
                assert_eq!(filter_text, "proj");
            }
            _ => panic!("expected TriggerFired"),
        }
    }

    #[test]
    fn default_triggers_include_link_always() {
        // Even with no operations, [[ trigger is present
        let triggers = default_triggers_for_operations(&[]);
        assert_eq!(triggers.len(), 1);
        assert!(triggers
            .iter()
            .any(|t| matches!(t, InputTrigger::TextPrefix { action, .. } if action == "doc_link")));
    }

    #[test]
    fn default_triggers_include_slash_with_ops() {
        use holon_api::render_types::{OperationDescriptor, OperationWiring};
        use holon_api::types::EntityName;
        let ops = vec![OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                name: "delete".into(),
                display_name: "Delete".into(),
                ..Default::default()
            },
        }];
        let triggers = default_triggers_for_operations(&ops);
        assert_eq!(triggers.len(), 2);
        assert!(triggers.iter().any(
            |t| matches!(t, InputTrigger::TextPrefix { action, .. } if action == "command_menu")
        ));
        assert!(triggers
            .iter()
            .any(|t| matches!(t, InputTrigger::TextPrefix { action, .. } if action == "doc_link")));
    }
}
