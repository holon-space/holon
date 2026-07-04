//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//! @c4 uses holon-turso "production Turso storage backend under test" "Rust"
//! @c4 uses holon-pbt-core "PBT cap traits + contribution seam" "Rust"
//!
//! Companion PBT crate for `holon-turso` — the Turso storage subsystem's slice
//! of the ONE composed-keystone PBT (co-location Phase 2, plan §5-turso).
//!
//! Owns the pure-Turso correspondence ARMS, co-located out of the central
//! table:
//! - The two Turso arms (`block_raw`, `matview`) of the SHARED
//!   [`NonSeedBlocks`] observable (whose struct + `ref_non_seed_blocks` stay on
//!   the pbt-core floor; the `/loro` arm is contributed by
//!   `holon-loro-testing`).
//! - The whole `block_content` / `block_parent` / `advice_matviews` observables
//!   (every arm Turso-owned): observable struct + `impl Observable` +
//!   `ref_project` move here entirely.
//!
//! Exposed to the central fold via [`pbt_contribution`] / [`pbt_footprint`].
//!
//! ## Ref-state independence (plan §4)
//! Nothing here names a concrete reference-state type: the projections read
//! `Ref*` / `Sut*` capabilities generically through the composed `CapMap`. A
//! guard integration test (`tests/no_ref_state_dep.rs`) fails the build if this
//! crate ever reaches for the central `ReferenceState` monolith.
//!
//! ## SUT component stays central (for now)
//! The `SqlProjectionComponent` SUT cap component is still assembled by the
//! central `compose_sut_*`; this Phase co-locates the correspondence ARMS only
//! (exactly like the Loro follow-on moved the `/loro` arm, not the `LoroSut`).
//! So [`pbt_contribution`]'s `cap_installers` and `generators` are empty.

pub mod correspondences;

use holon_pbt_core::contribution::CrateId;
use holon_pbt_core::contribution::PbtContribution;
use holon_pbt_core::contribution::PbtFootprint;

/// `holon-turso`'s contribution to the ONE composed PBT: its six
/// storage-pipeline correspondence arms. `cap_installers` is empty — the
/// central `compose_sut_*` still assembles the `CapMap` (incl. the
/// `SqlProjectionComponent`); `generators` is empty because no transitions have
/// co-located here.
pub fn pbt_contribution() -> PbtContribution {
    PbtContribution {
        crate_id: CrateId::Turso,
        invariants: correspondences::wire_all(),
        cap_installers: Vec::new(),
        generators: Vec::new(),
    }
}

/// Boot-free footprint — the ladder-floor enumeration for `holon-turso`,
/// derived from the live contribution so it cannot drift.
pub fn pbt_footprint() -> PbtFootprint {
    pbt_contribution().footprint()
}
