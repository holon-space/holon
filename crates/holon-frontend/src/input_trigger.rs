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
pub enum ViewEvent {
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
    TriggerDismissed { action: String },

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
pub fn check_triggers(
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
                        return Some(ViewEvent::TriggerFired {
                            action: action.clone(),
                            filter_text: current_line[after_prefix..end].to_string(),
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
/// When a node has operations, it gets a slash command trigger so the user
/// can discover and invoke them via `/`. This is the default; explicit
/// trigger declarations in the render DSL will override this.
pub fn default_triggers_for_operations(
    _operations: &[holon_api::render_types::OperationWiring],
) -> Vec<InputTrigger> {
    vec![InputTrigger::TextPrefix {
        prefix: "/".to_string(),
        action: "command_menu".to_string(),
        at_line_start: true,
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
}
