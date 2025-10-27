//! `inv-no-errors` — `SutErrorLog` only, ignores the reference, so it runs
//! whenever an error-log surface is wired (the composed `HeadlessFrontendComponent`
//! hosts it over the production `FrontendSession`'s publish-error tracker).
//! Asserts no app-level error was logged since startup. Storage-only / pure
//! slices don't wire `SutErrorLog`, so it deselects there (disclosed, not faked);
//! its teeth are the E2E frontend slice's real counter and the catch test below.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutErrorLog;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::no_errors::InvNoErrors;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoErrors,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutErrorLog>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Positive: an error-log surface that logged nothing ⇒ selected (a
    /// `SutErrorLog` is wired) and passing.
    #[tokio::test]
    async fn no_errors_passes_when_clean() {
        let sut = error_log_map(FixtureErrorLog::default());
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report.ran_ids().contains(&"inv-no-errors"),
            "wiring SutErrorLog must select inv-no-errors; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "a clean error log must pass: {:?}",
            report.failures(),
        );
    }

    /// Negative containment: without a `SutErrorLog` the invariant is
    /// *deselected* — disclosed, not faked. A backend-only SUT must not silently
    /// "pass" the error check.
    #[tokio::test]
    async fn no_errors_deselected_without_error_log() {
        let blocks = vec![Block::new_text(
            uri("local://r"),
            EntityUri::no_parent(),
            "root",
        )];
        let report = run_selected(
            &composed_invariant_catalog(),
            &fixture_slice(blocks),
            &CapMap::new(),
        )
        .await;

        assert!(
            report.deselected.iter().any(|d| d.0 == "inv-no-errors"),
            "inv-no-errors must be deselected without a SutErrorLog; ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch: an error-log surface with a non-zero counter ⇒ the invariant fires.
    #[tokio::test]
    async fn no_errors_catches_logged_error() {
        let sut = error_log_map(FixtureErrorLog {
            error_count: 2,
            context: vec!["block:demo".to_string()],
        });
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        let failures = report.failures();
        assert!(
            failures.iter().any(|(id, _)| *id == "inv-no-errors"),
            "a logged app error must be caught by inv-no-errors; failures={failures:?}",
        );
    }
}
