//! `inv-task-state-storage-coherence` — cross-checks each block's `task_state`
//! between the **SQL projection** (`SutSqlProjection::block_task_state`, a
//! `json_extract(properties,'$.task_state')` read) and the **Loro projection**
//! (`SutLoroTaskState::loro_task_state_of`, the same `properties["task_state"]`
//! scalar off the live CRDT tree). No reference side — both truths come from
//! the SUT, so this catches a Loro↔SQL desync at the data layer before any
//! render bug surfaces. It selects only in a slice that wires **both** caps
//! (the combined SQL+Loro slice), the only non-redundant consumer of
//! `SutLoroTaskState`.

use std::marker::PhantomData;

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutLoroTaskState;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::task_state_storage_coherence::InvTaskStateStorageCoherence;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvTaskStateStorageCoherence::<CapMap>(PhantomData),
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutSqlProjection>(),
                CapId::of::<dyn SutLoroTaskState>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Positive: both projections agree on every block's `task_state` ⇒
    /// selected (both caps wired) and passing.
    #[tokio::test]
    async fn task_state_coherence_passes_when_sql_and_loro_agree() {
        let a = uri("block:a");
        let b = uri("block:b");
        let sut = task_state_maps(
            vec![(a.clone(), "TODO"), (b.clone(), "DONE")],
            vec![(a, "TODO"), (b, "DONE")],
        );

        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report
                .ran_ids()
                .contains(&"inv-task-state-storage-coherence"),
            "wiring SutSqlProjection + SutLoroTaskState must select the coherence invariant; \
             ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.failures().is_empty(),
            "agreeing projections must pass: {:?}",
            report.failures(),
        );
    }

    /// Negative containment (§2): deselected — disclosed, not faked — when only
    /// the SQL projection is wired (no `SutLoroTaskState`). A SQL-only slice
    /// must not silently "pass" a cross-store coherence check it can't
    /// perform.
    #[tokio::test]
    async fn task_state_coherence_deselected_without_loro_task_state() {
        let sut = sql_projection_map(vec![(uri("block:a"), "content")]);

        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        assert!(
            report
                .deselected
                .iter()
                .any(|d| d.0 == "inv-task-state-storage-coherence"),
            "without SutLoroTaskState the coherence invariant must be deselected; ran={:?} \
             deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }

    /// Catch (doc §6 gate): SQL says `TODO`, Loro says `DONE` for the same
    /// block — a desync the synced component pair can't produce, injected
    /// via the fixtures.
    #[tokio::test]
    async fn task_state_coherence_catches_sql_loro_divergence() {
        let a = uri("block:a");
        let sut = task_state_maps(vec![(a.clone(), "TODO")], vec![(a, "DONE")]);

        let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-task-state-storage-coherence"),
            "a SQL↔Loro task_state divergence must be caught; failures={failures:?}",
        );
    }
}

/// Real-SUT non-vacuity teeth (relocated from the deleted
/// `task_state_slice.rs`).
///
/// The ONE PBT (`general_e2e_composed_pbt` / `WideE2E`) selects this invariant
/// in the wide config (its caps are present — the cap-presence guard proves it)
/// and the per-draw floor runs it every tick — but "runs" is satisfied even
/// by a never-toggled tree (both projections read `None` ⇒ trivially coherent).
/// This test is the guard that the check is *meaningful*: a real `ToggleState`
/// over the composed `full_headless` CapMap must move BOTH the SQL
/// `block_raw.properties` projection AND the Loro tag projection `None` →
/// `TODO` in lockstep — exactly the coherence the invariant guards. It is the
/// only test that asserts the **Loro** side (`loro_task_state_of`) moves with
/// the SQL side directly; the frontend-slice toggle teeth only proves
/// `inv-blocks-match-ref/block_raw` catches a SUT-only divergence.
///
/// (The SQL↔Loro *divergence catch* is proven at the fixture level by the
/// `tests` module above; this proves the real write path is non-vacuous.)
#[cfg(test)]
mod real_sut_teeth {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use holon_pbt_core::TransitionImpl;
    use holon_pbt_core::capabilities::SutLoroTaskState;
    use holon_pbt_core::capabilities::SutSqlProjection;

    use crate::pbt::composed::seed_primitives::fixed_ids;
    use crate::pbt::composed::wide_e2e::SETTLE;
    use crate::pbt::composed::wide_e2e::boot_and_seed_wide;
    use crate::pbt::composed::wide_e2e::wide_e2e_ref;
    use crate::pbt::op_write_cap::IdResolver;
    use crate::pbt::transitions::ToggleState;
    use crate::pbt::transitions::toggle_state::CycleTarget;

    /// A real `ToggleState(c1 → TODO)` over the composed `full_headless` CapMap
    /// must land in BOTH stores. Before the toggle both read `None` (plain
    /// seed block); after, both read `"TODO"`. Proves (a) the
    /// `SutMutate::toggle_state` write path is real over composed
    /// components, and (b) the ONE PBT's required coherence check
    /// is exercised non-trivially — guarding against a vacuous green.
    ///
    /// Structured as a plain `#[test]` building its own multi-thread runtime:
    /// the ref is computed **synchronously before** `block_on`, so
    /// `wide_e2e_ref()`'s internal `full_headless_cap_set()` (which builds
    /// + drives its own runtime to extract the cap set) runs outside any
    /// runtime and memoizes its `OnceLock` — avoiding a "runtime within a
    /// runtime" panic that a `#[tokio::test]` would hit.
    #[test]
    fn toggle_state_moves_sql_and_loro_in_lockstep() {
        // Synchronously (outside any runtime): seed the oracle, which initializes the
        // `full_headless_cap_set()` OnceLock via its own short-lived runtime.
        let ref_state = wide_e2e_ref();
        let c1 = fixed_ids().c1;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("multi-thread runtime");
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

        rt.block_on(async {
            let (mut caps, _handle, _scaffold) = boot_and_seed_wide(&resolver, &ref_state).await;

            // Baseline: the seed block carries no task_state in either projection.
            assert_eq!(
                caps.block_task_state(&c1).await,
                None,
                "precondition: seeded c1 has no SQL task_state"
            );
            assert_eq!(
                caps.loro_task_state_of(c1.as_str()).await,
                None,
                "precondition: seeded c1 has no Loro task_state"
            );

            // `boot_and_seed_wide` already focused the page root (`structural-page`),
            // so c1 — a direct child — is a VISIBLE Main row. Production has no
            // block-zoom gesture that would make the child block c1 a focus root of
            // its own (the sidebar only focuses pages), so a click on c1's
            // `state_toggle` is exactly the faithful user gesture. Cycle it to TODO
            // via the real `SutMutate::toggle_state` write path (same dispatch the
            // PBT uses).
            TransitionImpl::apply_to_sut(
                &ToggleState {
                    block_id: c1.clone(),
                    new_state: CycleTarget::Todo,
                },
                &ref_state,
                &mut caps,
            )
            .await;
            tokio::time::sleep(SETTLE).await;

            let sql = caps.block_task_state(&c1).await;
            let loro = caps.loro_task_state_of(c1.as_str()).await;
            assert_eq!(
                sql.as_deref(),
                Some("TODO"),
                "SQL projection must reflect the toggle (loro={loro:?})"
            );
            assert_eq!(
                loro.as_deref(),
                Some("TODO"),
                "Loro projection must reflect the toggle in lockstep with SQL (sql={sql:?})"
            );
        });
    }
}
