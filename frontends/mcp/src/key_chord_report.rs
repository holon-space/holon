//! Turning a press window's dispatch journal into the `send_key_chord` verdict.
//!
//! Its own module because this classification is the thing that was WRONG:
//! the tool used to answer with the keymap match and call it an outcome. A
//! pure function over `(pressed, matched actions, journal entries)` can be
//! tested for every registry and every outcome without a window, which is what
//! keeps the reply honest.

use holon_frontend::dispatch_journal::DispatchOutcome;
use holon_frontend::dispatch_journal::DispatchedIntent;

/// The verdict and, whenever it is anything but a clean success, the reason.
///
/// Order matters and encodes the honesty rules: a failure anywhere in the
/// press window outranks a match, and an unknown outcome is reported as
/// unknown rather than assumed good.
pub fn classify(
    pressed: bool,
    matched_actions: &[&str],
    dispatched: &[DispatchedIntent],
) -> (&'static str, Option<String>) {
    let describe = |ds: &[DispatchedIntent]| {
        ds.iter()
            .map(|d| format!("{}.{}", d.entity_name, d.op_name))
            .collect::<Vec<_>>()
    };
    let failures: Vec<String> = dispatched
        .iter()
        .filter_map(|d| match &d.outcome {
            DispatchOutcome::Failed(e) => Some(format!("{}.{}: {e}", d.entity_name, d.op_name)),
            _ => None,
        })
        .collect();
    let still_pending: Vec<String> = dispatched
        .iter()
        .filter(|d| d.outcome.is_pending())
        .map(|d| format!("{}.{}", d.entity_name, d.op_name))
        .collect();

    if !pressed {
        return (
            "bound_but_not_dispatched",
            Some("the driver declined to press this chord".to_string()),
        );
    }
    if dispatched.is_empty() {
        return (
            "bound_but_not_dispatched",
            Some(format!(
                "the chord was pressed but nothing reached the dispatcher — {matched_actions:?} is \
                 bound yet nothing ran"
            )),
        );
    }
    if !failures.is_empty() {
        return (
            "dispatched_but_failed",
            Some(format!("the chord ran and the action FAILED: {failures:?}")),
        );
    }
    if !still_pending.is_empty() {
        return (
            "dispatched_outcome_pending",
            Some(format!(
                "the chord ran but {still_pending:?} had not reported an outcome in time — it may \
                 still succeed or fail; this is NOT a success claim"
            )),
        );
    }
    if dispatched
        .iter()
        .any(|d| matched_actions.contains(&d.op_name.as_str()))
    {
        return ("executed", None);
    }
    (
        "dispatched_other_action",
        Some(format!(
            "the chord matched {:?} but the app dispatched {:?} — the match is NOT what ran; treat \
             `dispatched` as the truth",
            matched_actions,
            describe(dispatched)
        )),
    )
}

#[cfg(test)]
mod tests {
    use holon_frontend::dispatch_journal::WINDOW_REGISTRY;

    use super::*;

    fn entry(entity: &str, op: &str, outcome: DispatchOutcome) -> DispatchedIntent {
        DispatchedIntent {
            entity_name: entity.into(),
            op_name: op.into(),
            target: None,
            outcome,
            seq: 0,
        }
    }

    /// The regression this module exists for: undo runs through
    /// `FrontendSession::undo`, never `dispatch_intent`. When the journal
    /// covered only the structural registry, a working cmd+z answered
    /// "nothing ran".
    #[test]
    fn a_window_registry_undo_that_ran_is_executed_not_nothing_ran() {
        let dispatched = [entry(WINDOW_REGISTRY, "undo", DispatchOutcome::Succeeded)];
        let (status, detail) = classify(true, &["undo"], &dispatched);
        assert_eq!(status, "executed", "detail was {detail:?}");
        assert_eq!(detail, None);
    }

    #[test]
    fn a_structural_chord_that_ran_the_wrong_action_is_named_as_such() {
        let dispatched = [entry("block", "split_block", DispatchOutcome::Succeeded)];
        let (status, detail) = classify(true, &["cycle_task_state"], &dispatched);
        assert_eq!(status, "dispatched_other_action");
        let detail = detail.unwrap();
        assert!(detail.contains("cycle_task_state"), "{detail}");
        assert!(detail.contains("block.split_block"), "{detail}");
    }

    /// "Reached the dispatcher" is not "worked". A refused op must not read as
    /// executed just because it was matched and dispatched.
    #[test]
    fn a_matched_action_that_failed_reports_the_failure_not_success() {
        let dispatched = [entry(
            "block",
            "outdent",
            DispatchOutcome::Failed("no parent to outdent to".into()),
        )];
        let (status, detail) = classify(true, &["outdent"], &dispatched);
        assert_eq!(status, "dispatched_but_failed");
        assert!(detail.unwrap().contains("no parent to outdent to"));
    }

    /// A failure anywhere in the press window outranks a matched success — a
    /// window action that only dispatches (turn_into_page) settles Ok while
    /// the operation it caused can still fail.
    #[test]
    fn a_failure_beside_a_matched_success_still_reports_the_failure() {
        let dispatched = [
            entry(
                WINDOW_REGISTRY,
                "turn_into_page",
                DispatchOutcome::Succeeded,
            ),
            entry(
                "block",
                "convert_block_to_page",
                DispatchOutcome::Failed("page name taken".into()),
            ),
        ];
        let (status, detail) = classify(true, &["turn_into_page"], &dispatched);
        assert_eq!(status, "dispatched_but_failed");
        assert!(detail.unwrap().contains("page name taken"));
    }

    #[test]
    fn an_unsettled_outcome_is_reported_as_pending_never_as_success() {
        let dispatched = [entry("block", "indent", DispatchOutcome::Pending)];
        let (status, _) = classify(true, &["indent"], &dispatched);
        assert_eq!(status, "dispatched_outcome_pending");
    }

    #[test]
    fn an_empty_press_window_is_nothing_ran() {
        let (status, detail) = classify(true, &["undo"], &[]);
        assert_eq!(status, "bound_but_not_dispatched");
        assert!(detail.unwrap().contains("nothing ran"));
    }

    #[test]
    fn a_press_the_driver_declined_is_not_credited_with_anything() {
        let (status, detail) = classify(false, &["undo"], &[]);
        assert_eq!(status, "bound_but_not_dispatched");
        assert!(detail.unwrap().contains("declined"));
    }
}
