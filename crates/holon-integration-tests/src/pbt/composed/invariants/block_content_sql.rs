//! `inv-block-content-matches-ref` — per-block `content` equality between the
//! **SQL projection** (`SutSqlProjection::block_content`, a `block_raw.content`
//! read) and the reference's `RefBlockTree`. The SQL sibling of
//! `inv-block-content-matches-ref/block_raw` (which reads the same content via
//! the typed `SutBackend` snapshot): this one exercises the direct-column SQL
//! read path, the surface `E2ESut` realizes over Turso and which the SQL slice
//! now covers without the monolith.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, SutSqlProjection};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::block_content_matches_ref::InvBlockContentMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlockContentMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutSqlProjection>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::{assert_ref_seeded, run_with_seeded_ref, seed_ref};

    /// Positive: wiring `SutSqlProjection` + `RefBlockTree` selects the SQL
    /// content invariant, which passes when the SQL projection agrees with the
    /// reference.
    #[tokio::test]
    async fn sql_slice_runs_block_content_via_sql_projection() {
        let id = uri("block:c");
        let sut = sql_projection_map(vec![(id.clone(), "content")]);
        let ref_state = seed_ref(vec![Block::new_text(
            id.clone(),
            EntityUri::no_parent(),
            "content",
        )]);
        assert_ref_seeded(&ref_state, &[id]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        assert!(
            report.ran_ids().contains(&"inv-block-content-matches-ref"),
            "wiring SutSqlProjection must select the SQL content invariant; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "SQL content matches the reference, so it passes: {:?}",
            report.failures(),
        );
    }

    /// Negative containment (§2): deselected — disclosed, not faked — when no
    /// `SutSqlProjection` is wired (only a `SutBackend` block store).
    #[tokio::test]
    async fn sql_block_content_deselected_without_sql_projection() {
        let id = uri("block:c");
        let sut = fixture_slice(vec![Block::new_text(
            id.clone(),
            EntityUri::no_parent(),
            "content",
        )]);
        let ref_state = seed_ref(vec![Block::new_text(id, EntityUri::no_parent(), "content")]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        assert!(
            report
                .deselected
                .iter()
                .any(|d| d.0 == "inv-block-content-matches-ref"),
            "without SutSqlProjection the SQL content invariant must be deselected; \
             ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch (doc §6 gate): a SQL `block_raw.content` that diverged from the
    /// reference is caught.
    #[tokio::test]
    async fn sql_block_content_catches_divergence() {
        let id = uri("block:d");
        let sut = sql_projection_map(vec![(id.clone(), "sql-content")]);
        let ref_state = seed_ref(vec![Block::new_text(
            id,
            EntityUri::no_parent(),
            "ref-content",
        )]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-block-content-matches-ref"),
            "the SQL content divergence must be caught; failures={failures:?}",
        );
    }
}
