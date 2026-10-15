//! The one name -> executor `Twin` table behind `HOLON_NET_TWIN` (exec nets)
//! and `UFT_LOWER_TWIN` (lowered nets).

use holon_net::exec::Twin;

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
}
