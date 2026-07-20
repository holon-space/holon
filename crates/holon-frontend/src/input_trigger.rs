use serde::Deserialize;
use serde::Serialize;

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
    /// `at_line_start: true` restricts to column 0 (e.g., `/` for slash
    /// commands). `at_line_start: false` matches anywhere (e.g., `[[` for
    /// doc links, `@` for mentions).
    ///
    /// `word_boundary: true` (only meaningful with `at_line_start: false`)
    /// additionally requires the matched prefix to sit at a word boundary —
    /// line start, or immediately after whitespace. This is what keeps `/` in
    /// a URL (`https://example.com/path`) from firing the command menu, while
    /// still allowing Logseq-style `text /cmd`. `[[` and `@` deliberately leave
    /// this `false`: mid-word `[[foo` / `a@b` are legitimate triggers.
    TextPrefix {
        prefix: String,
        action: String,
        #[serde(default)]
        at_line_start: bool,
        #[serde(default)]
        word_boundary: bool,
    },
}

/// Semantic event sent from the View to the ViewModel layer when a trigger
/// fires.
///
/// The View never interprets these — it just matches triggers and sends events.
/// The ViewModel layer (shared Rust code) handles the actual logic.
#[derive(Debug, Clone)]
pub(crate) enum ViewEvent {
    /// A text trigger fired (e.g., user typed "/" at line start, or "[["
    /// mid-line).
    TriggerFired {
        action: String,
        /// The text after the trigger prefix (e.g., "emb" after "/emb").
        filter_text: String,
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
/// `cursor_byte` is the cursor's BYTE offset within that line (callers with a
/// character column — GPUI's `cursor_position().character` — must convert
/// first; this function slices `current_line` at this offset and panics on a
/// non-char-boundary, fail-loud by design).
///
/// Called on every keystroke — must be extremely cheap.
pub(crate) fn check_triggers(
    triggers: &[InputTrigger],
    current_line: &str,
    cursor_byte: usize,
) -> Option<ViewEvent> {
    for trigger in triggers {
        match trigger {
            InputTrigger::TextPrefix {
                prefix,
                action,
                at_line_start,
                word_boundary,
            } => {
                if *at_line_start {
                    if current_line.starts_with(prefix.as_str()) && cursor_byte >= prefix.len() {
                        return Some(ViewEvent::TriggerFired {
                            action: action.clone(),
                            filter_text: current_line[prefix.len()..].to_string(),
                            prefix_start: 0,
                        });
                    }
                } else {
                    let end = cursor_byte.min(current_line.len());
                    let text_before_cursor = &current_line[..end];
                    if let Some(pos) = text_before_cursor.rfind(prefix.as_str()) {
                        // Word-boundary gate (e.g. `/`): the prefix only counts
                        // at line start or right after whitespace. Kills the
                        // whole URL class (`/` after `s`/`:`/`.`/`/`) while
                        // preserving `text /cmd` and line-start `/cmd`.
                        if *word_boundary && pos > 0 {
                            let prev = current_line[..pos].chars().next_back().unwrap();
                            if !prev.is_whitespace() {
                                continue;
                            }
                        }
                        let after_prefix = pos + prefix.len();
                        let between = &current_line[after_prefix..end];
                        // If the link is already closed (]] between prefix and cursor), skip
                        if prefix == "[[" && between.contains("]]") {
                            continue;
                        }
                        return Some(ViewEvent::TriggerFired {
                            action: action.clone(),
                            filter_text: between.to_string(),
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
/// Link triggers (`[[`) are always included — they work independently of
/// operations.
///
/// The `/` menu fires **mid-line** (`at_line_start: false`), matching LogSeq:
/// typing `/cmd` anywhere in a block opens the command menu, with the text
/// between the last `/` and the cursor as the filter. (Previously
/// line-start-only, which silently dropped slash commands appended to existing
/// block content.)
pub(crate) fn default_triggers_for_operations(
    operations: &[holon_api::render_types::OperationWiring],
) -> Vec<InputTrigger> {
    let mut triggers = always_on_triggers();
    if !operations.is_empty() {
        triggers.push(InputTrigger::TextPrefix {
            prefix: "/".to_string(),
            action: "command_menu".to_string(),
            at_line_start: false,
            word_boundary: true,
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
        word_boundary: false,
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
            word_boundary: false,
        }
    }

    fn link_trigger() -> InputTrigger {
        InputTrigger::TextPrefix {
            prefix: "[[".to_string(),
            action: "doc_link".to_string(),
            at_line_start: false,
            word_boundary: false,
        }
    }

    fn mention_trigger() -> InputTrigger {
        InputTrigger::TextPrefix {
            prefix: "@".to_string(),
            action: "mention".to_string(),
            at_line_start: false,
            word_boundary: false,
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
    fn multibyte_line_with_byte_offset() {
        let triggers = vec![link_trigger()];
        // 'ß' is 2 bytes — cursor after "[[ß" is byte 4. The GPUI caller
        // converts its character column to this byte offset; passing a char
        // count (3) would land inside 'ß' and panic (fail-loud contract).
        let event = check_triggers(&triggers, "[[ß", 4).unwrap();
        match event {
            ViewEvent::TriggerFired { filter_text, .. } => assert_eq!(filter_text, "ß"),
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

    // A mid-line slash trigger, exactly as `default_triggers_for_operations`
    // wires it (at_line_start: false, word-boundary gated for URL safety).
    fn slash_midline_trigger() -> InputTrigger {
        InputTrigger::TextPrefix {
            prefix: "/".to_string(),
            action: "command_menu".to_string(),
            at_line_start: false,
            word_boundary: true,
        }
    }

    #[test]
    fn url_slash_does_not_trigger() {
        // `https://example.com/path` — every `/` is preceded by a non-space
        // char (`:`, `/`, `m`), so no `/` is at a word boundary. The permanent
        // "Type to search…" popup bug: without the guard the trailing
        // `/path` fired command_menu and never dismissed.
        let triggers = vec![slash_midline_trigger()];
        assert!(check_triggers(&triggers, "https://example.com/path", 24).is_none());
    }

    #[test]
    fn slash_after_space_triggers() {
        // Logseq-style "text /cmd": slash after whitespace opens the menu.
        let triggers = vec![slash_midline_trigger()];
        let event = check_triggers(&triggers, "foo /de", 7).unwrap();
        match event {
            ViewEvent::TriggerFired {
                action,
                filter_text,
                prefix_start,
                ..
            } => {
                assert_eq!(action, "command_menu");
                assert_eq!(filter_text, "de");
                assert_eq!(prefix_start, 4);
            }
            _ => panic!("expected TriggerFired"),
        }
    }

    #[test]
    fn slash_at_line_start_midline_triggers() {
        // `/cmd` at column 0 of the line still fires (pos == 0 is a boundary).
        let triggers = vec![slash_midline_trigger()];
        let event = check_triggers(&triggers, "/cmd", 4).unwrap();
        match event {
            ViewEvent::TriggerFired { filter_text, .. } => assert_eq!(filter_text, "cmd"),
            _ => panic!("expected TriggerFired"),
        }
    }

    #[test]
    fn slash_no_space_before_does_not_trigger() {
        // `foo/bar` — slash glued to a word, not a command trigger.
        let triggers = vec![slash_midline_trigger()];
        assert!(check_triggers(&triggers, "foo/bar", 7).is_none());
    }

    #[test]
    fn default_triggers_include_link_always() {
        // Even with no operations, [[ trigger is present
        let triggers = default_triggers_for_operations(&[]);
        assert_eq!(triggers.len(), 1);
        assert!(
            triggers.iter().any(
                |t| matches!(t, InputTrigger::TextPrefix { action, .. } if action == "doc_link")
            )
        );
    }

    #[test]
    fn default_triggers_include_slash_with_ops() {
        use holon_api::render_types::OperationDescriptor;
        use holon_api::render_types::OperationWiring;
        use holon_api::types::EntityName;
        let ops = vec![OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                name: "delete".into(),
                display_name: "Delete".into(),
                id_column: "id".to_string(),
                description: String::new(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                menu_exposure: holon_api::MenuExposure::Listed {
                    surfaces: holon_api::SurfaceSet {
                        slash_menu: true,
                        action_bar: false,
                    },
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
        }];
        let triggers = default_triggers_for_operations(&ops);
        assert_eq!(triggers.len(), 2);
        assert!(triggers.iter().any(
            |t| matches!(t, InputTrigger::TextPrefix { action, .. } if action == "command_menu")
        ));
        assert!(
            triggers.iter().any(
                |t| matches!(t, InputTrigger::TextPrefix { action, .. } if action == "doc_link")
            )
        );
    }
}
