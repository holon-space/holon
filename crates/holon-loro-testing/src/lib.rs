//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//! @c4 uses holon-loro "production CRDT backend under test" "Rust"
//! @c4 uses holon-pbt-core "PBT cap traits + contribution seam" "Rust"
//!
//! Companion PBT crate for `holon-loro` — the first subsystem to OWN its
//! composed-keystone contributions (co-location Phase 1, plan §5-loro).
//!
//! Owns the Loro storage subsystem's cleanly-decoupled PBT artifacts:
//! - [`LoroBackendComponent`] — the SUT cap component wrapping a real
//!   `holon_loro::LoroBackend` (`SutBackend` + `SutLoroLog` +
//!   `SutLoroTaskState`).
//! - `inv-loro-no-errors` and `inv-loro-children-match-ref` invariant bodies +
//!   their `wire()`s.
//!
//! Exposed to the central fold via [`pbt_contribution`] / [`pbt_footprint`].
//!
//! ## Ref-state independence (plan §4)
//! Nothing here names a concrete reference-state type: the invariant bodies
//! read `Ref*` capabilities generically. A guard integration test
//! (`tests/no_ref_state_dep.rs`) fails the build if this crate ever reaches for
//! the central `ReferenceState` monolith.
//!
//! ## What is NOT here yet (BLOCKED on Phase 1a)
//! `LoroSut`, the 7 CRDT/sync transitions, and the `/loro` correspondence arm
//! stay central: they bind the concrete `ReferenceState` + the monolithic
//! `SutHandle` dispatch trait + central shared helpers. They co-locate once
//! those are lifted into a shared crate (the Phase 1a decoupling).

pub mod component;
pub mod invariants;

pub use component::LoroBackendComponent;
use holon_pbt_core::contribution::CrateId;
use holon_pbt_core::contribution::PbtContribution;
use holon_pbt_core::contribution::PbtFootprint;

/// `holon-loro`'s contribution to the ONE composed PBT: its two Loro-specific
/// invariants. `cap_installers` is empty — the central `compose_sut_*` still
/// constructs [`LoroBackendComponent`] (it needs the live shared backend + sync
/// handle), registering the moved `CapProvider` impl. `generators` is empty:
/// the Loro transitions are BLOCKED on Phase 1a.
pub fn pbt_contribution() -> PbtContribution {
    PbtContribution {
        crate_id: CrateId::Loro,
        invariants: vec![
            invariants::loro_no_errors::wire(),
            invariants::loro_children_match_ref::wire(),
        ],
        cap_installers: Vec::new(),
        generators: Vec::new(),
    }
}

/// Static, boot-free footprint — the ladder-floor enumeration for `holon-loro`.
/// Held in lockstep with [`pbt_contribution`] by the anti-rot test below.
pub fn pbt_footprint() -> PbtFootprint {
    PbtFootprint {
        crate_id: CrateId::Loro,
        invariant_ids: vec!["inv-loro-no-errors", "inv-loro-children-match-ref"],
        transition_kinds: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse-don't-validate anti-rot: the static footprint must list exactly
    /// the live contribution's invariant ids, in order.
    #[test]
    fn footprint_matches_contribution() {
        assert_eq!(
            pbt_footprint().invariant_ids,
            pbt_contribution().invariant_ids(),
            "holon-loro-testing footprint drifted from its live contribution",
        );
    }
}
