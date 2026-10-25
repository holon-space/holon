//! The one name -> executor `Twin` table behind `HOLON_NET_TWIN` (exec nets)
//! and `UFT_LOWER_TWIN` (lowered nets), and the name -> `LowerTwin` table
//! behind `UFT_LOWER_TWIN`.

use holon_net::exec::Twin;
use holon_net::uft::LowerTwin;

const TWINS: &[(&str, Twin)] = &[
    ("hidden-reservation", Twin::HiddenReservation),
    ("mark-by-version", Twin::MarkByVersion),
    ("no-mark", Twin::NoMark),
    ("no-cas", Twin::NoCas),
    ("no-count-bound", Twin::NoCountBound),
    ("unknown-is-failure", Twin::UnknownIsFailure),
    ("unknown-settles-failed", Twin::UnknownSettlesFailed),
    ("stale-done-returns-input", Twin::StaleDoneReturnsInput),
    ("drop-late-done", Twin::DropLateDone),
    ("drop-late-failed", Twin::DropLateFailed),
    ("failed-settles-alone", Twin::FailedSettlesAlone),
    ("value-before-bind", Twin::ValueBeforeBind),
    ("occurrence-not-in-trace", Twin::OccurrenceNotInTrace),
    ("undo-restores-answered", Twin::UndoRestoresAnswered),
    ("settled-from-answer", Twin::SettledFromAnswer),
    ("no-quiescence", Twin::NoQuiescence),
    ("keep-work-failed", Twin::KeepWorkFailed),
];

const LOWER_TWINS: &[(&str, LowerTwin)] = &[
    ("no-ask-hash", LowerTwin::NoAskHash),
    ("no-answer-inhibitor", LowerTwin::NoAnswerInhibitor),
    ("double-answer", LowerTwin::DoubleAnswer),
    ("no-expire", LowerTwin::NoExpire),
    ("ask-while-in-flight", LowerTwin::AskWhileInFlight),
    ("ask-while-retry-pending", LowerTwin::AskWhileRetryPending),
    (
        "ask-while-outcome-unknown",
        LowerTwin::AskWhileOutcomeUnknown,
    ),
];

#[cfg(feature = "naive-twins")]
pub fn lower_twin_by_name(name: &str) -> Option<LowerTwin> {
    LOWER_TWINS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

#[cfg(feature = "naive-twins")]
pub fn executor_twin(name: &str) -> Option<Twin> {
    TWINS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_executor_twin_has_a_name() {
        // Exhaustive: a new `Twin` variant fails to compile until it is listed here.
        fn listed(t: Twin) -> Twin {
            match t {
                Twin::HiddenReservation
                | Twin::MarkByVersion
                | Twin::NoMark
                | Twin::NoCas
                | Twin::NoCountBound
                | Twin::UnknownIsFailure
                | Twin::UnknownSettlesFailed
                | Twin::StaleDoneReturnsInput
                | Twin::DropLateDone
                | Twin::DropLateFailed
                | Twin::FailedSettlesAlone
                | Twin::ValueBeforeBind
                | Twin::OccurrenceNotInTrace
                | Twin::UndoRestoresAnswered
                | Twin::SettledFromAnswer
                | Twin::NoQuiescence
                | Twin::KeepWorkFailed => t,
            }
        }
        let all = [
            Twin::HiddenReservation,
            Twin::MarkByVersion,
            Twin::NoMark,
            Twin::NoCas,
            Twin::NoCountBound,
            Twin::UnknownIsFailure,
            Twin::UnknownSettlesFailed,
            Twin::StaleDoneReturnsInput,
            Twin::DropLateDone,
            Twin::DropLateFailed,
            Twin::FailedSettlesAlone,
            Twin::ValueBeforeBind,
            Twin::OccurrenceNotInTrace,
            Twin::UndoRestoresAnswered,
            Twin::SettledFromAnswer,
            Twin::NoQuiescence,
            Twin::KeepWorkFailed,
        ];
        for t in all {
            let t = listed(t);
            assert!(
                TWINS.iter().any(|(_, listed)| *listed == t),
                "executor twin {t:?} has no name"
            );
        }
    }

    #[test]
    fn every_lowering_twin_has_a_name() {
        // Exhaustive: a new `LowerTwin` variant fails to compile until it is listed
        // here.
        fn listed(t: LowerTwin) -> LowerTwin {
            match t {
                LowerTwin::NoAskHash
                | LowerTwin::NoAnswerInhibitor
                | LowerTwin::DoubleAnswer
                | LowerTwin::NoExpire
                | LowerTwin::AskWhileInFlight
                | LowerTwin::AskWhileRetryPending
                | LowerTwin::AskWhileOutcomeUnknown => t,
            }
        }
        let all = [
            LowerTwin::NoAskHash,
            LowerTwin::NoAnswerInhibitor,
            LowerTwin::DoubleAnswer,
            LowerTwin::NoExpire,
            LowerTwin::AskWhileInFlight,
            LowerTwin::AskWhileRetryPending,
            LowerTwin::AskWhileOutcomeUnknown,
        ];
        for t in all {
            let t = listed(t);
            assert!(
                LOWER_TWINS.iter().any(|(_, listed)| *listed == t),
                "lowering twin {t:?} has no name"
            );
        }
    }
}
