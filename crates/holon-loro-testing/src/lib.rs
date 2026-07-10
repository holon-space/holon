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
//! - [`LoroSut`] — the peer-mesh / Loro-validation SUT (`sut_loro`), plus its
//!   [`quiescence`] and [`peer_ops`] helpers.
//! - The six peer-sync / CRDT [`transitions`] (`AddPeer`, `PeerEdit`,
//!   `PeerCharEdit`, `MergeFromPeer`, `SyncWithPeer`, `CreateStaleLoro`), each
//!   generic over the reference type `R`.
//! - `inv-loro-no-errors` and `inv-loro-children-match-ref` invariant bodies +
//!   their `wire()`s.
//!
//! Exposed to the central fold via [`pbt_contribution`] / [`pbt_footprint`].
//!
//! ## Ref-state independence (plan §4)
//! Nothing here names a concrete reference-state type: the invariant bodies and
//! transitions read `Ref*` capabilities generically. A guard integration test
//! (`tests/no_ref_state_dep.rs`) fails the build if this crate ever reaches for
//! the central `ReferenceState` monolith.
//!
//! ## Dispatch stays central (for now)
//! The `LoroSut` and transitions are co-located here (Phase-1a Step 4), but the
//! central `declare_e2e_transitions!` enum still assembles them (it re-exports
//! the structs) — the strategy is enum-driven, so [`pbt_contribution`]'s
//! `generators` is inert until a later contribution-driven-strategy phase.
//!
//! ## `/loro` correspondence arm co-located (Phase-1a follow-on)
//! The `inv-blocks-match-ref/loro` store arm now lives here
//! ([`invariants::loro_blocks_match_ref`]) as a single-store
//! `Correspondence<NonSeedBlocks>`, contributed through [`pbt_contribution`].
//! This was unblocked by lifting the correspondence framework, the
//! `NonSeedBlocks` observable, and the `block_compare` leaf onto the pbt-core
//! floor; the central table keeps only the not-yet-co-located `block_raw` +
//! `matview` arms (Phase 2).

pub mod component;
#[cfg(test)]
mod convergence_pbt;
pub mod invariants;
pub mod peer_ops;
pub mod quiescence;
pub mod sut_loro;
pub mod transitions;

pub use component::LoroBackendComponent;
use holon_pbt_core::contribution::CrateId;
use holon_pbt_core::contribution::PbtContribution;
use holon_pbt_core::contribution::PbtFootprint;
pub use sut_loro::LoroSut;

/// `holon-loro`'s contribution to the ONE composed PBT: its two Loro-specific
/// invariants. `cap_installers` is empty — the central `compose_sut_*` still
/// constructs [`LoroBackendComponent`] (it needs the live shared backend + sync
/// handle), registering the moved `CapProvider` impl. `generators` is empty by
/// convention (matching the central footprint): the six Loro transitions live
/// in [`transitions`] but are dispatched by the central enum, so nothing is
/// registered through this seam until the strategy becomes contribution-driven.
pub fn pbt_contribution() -> PbtContribution {
    PbtContribution {
        crate_id: CrateId::Loro,
        invariants: vec![
            invariants::loro_no_errors::wire(),
            invariants::loro_children_match_ref::wire(),
            invariants::loro_blocks_match_ref::wire(),
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
        invariant_ids: vec![
            "inv-loro-no-errors",
            "inv-loro-children-match-ref",
            "inv-blocks-match-ref/loro",
        ],
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
