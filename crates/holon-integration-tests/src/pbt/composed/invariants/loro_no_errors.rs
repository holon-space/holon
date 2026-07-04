//! Selection tests for `inv-loro-no-errors` — the invariant BODY + `wire()` now
//! live in the `holon-loro-testing` companion crate (co-location Phase 1) and
//! reach the composed catalog via the central fold. These fixture-driven
//! selection/catch tests stay here because they exercise the central
//! `composed_invariant_catalog()` + shared fixtures.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::pbt::composed::fixtures::*;

    /// Positive: a Loro store that logged no error ⇒ selected (a `SutLoroLog`
    /// is wired) and passing.
    #[tokio::test]
    async fn loro_no_errors_passes_when_clean() {
        let sut = loro_log_map(FixtureLoroLog::default());
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report.ran_ids().contains(&"inv-loro-no-errors"),
            "wiring SutLoroLog must select inv-loro-no-errors; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "a clean Loro log must pass: {:?}",
            report.failures(),
        );
    }

    /// Negative containment (§2): without a `SutLoroLog` the invariant is
    /// *deselected* — disclosed, not faked. A backend-only SUT must not
    /// silently "pass" the Loro check.
    #[tokio::test]
    async fn loro_no_errors_deselected_without_loro() {
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
            report
                .deselected
                .iter()
                .any(|d| d.0 == "inv-loro-no-errors"),
            "inv-loro-no-errors must be deselected without a SutLoroLog; ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch: a Loro store that logged an error ⇒ the invariant fires. The
    /// fixture injects what a valid CRDT can't (a non-zero error counter).
    #[tokio::test]
    async fn loro_no_errors_catches_logged_error() {
        let sut = loro_log_map(FixtureLoroLog {
            had_errors: true,
            children: HashMap::new(),
        });
        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        let failures = report.failures();
        assert!(
            failures.iter().any(|(id, _)| *id == "inv-loro-no-errors"),
            "a logged Loro error must be caught by inv-loro-no-errors; failures={failures:?}",
        );
    }
}
